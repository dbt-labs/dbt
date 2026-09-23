use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::Arc;

use dashmap::DashMap;
use serde::Serialize;

use crate::compiler::ast;
use crate::compiler::cfg::build_cfg;
use crate::compiler::codegen::{CodeGenerationProfile, CodeGenerator};
use crate::compiler::instructions::Instructions;
use crate::compiler::lexer::WhitespaceConfig;
use crate::compiler::meta::find_undeclared;
use crate::compiler::parser::{parse, parse_with_listeners};
use crate::compiler::typecheck::FunctionRegistry;
use crate::environment::Environment;
use crate::error::Error;
use crate::listener::{RenderingEventListener, TokenizerEventListener};
use crate::syntax::SyntaxConfig;
use crate::utils::AutoEscape;
use crate::value::{self, Value};
use crate::vm::listeners::TypecheckingEventListener;
use crate::vm::typemeta::TypeChecker;
use crate::vm::{prepare_blocks, Context, State, Vm};
use crate::Type;

/// Callback for auto escape determination
pub type AutoEscapeFunc = dyn Fn(&str) -> AutoEscape + Sync + Send;

/// Internal struct that holds template loading level config values.
#[derive(Clone)]
pub struct TemplateConfig {
    /// The syntax used for the template.
    pub syntax_config: SyntaxConfig,
    /// Controls whitespace behavior.
    pub ws_config: WhitespaceConfig,
    /// The callback that determines the initial auto escaping for templates.
    pub default_auto_escape: Arc<AutoEscapeFunc>,
}

impl TemplateConfig {
    pub(crate) fn new(default_auto_escape: Arc<AutoEscapeFunc>) -> TemplateConfig {
        TemplateConfig {
            syntax_config: SyntaxConfig::default(),
            ws_config: WhitespaceConfig::default(),
            default_auto_escape,
        }
    }
}

/// Represents a handle to a template.
///
/// Templates are stored in the [`Environment`] as bytecode instructions.  With the
/// [`Environment::get_template`] method that is looked up and returned in form of
/// this handle.  Such a template can be cheaply copied as it only holds references.
///
/// To render the [`render`](Template::render) method can be used.
#[derive(Clone)]
pub struct Template<'env: 'source, 'source> {
    env: &'env Environment<'env>,
    pub(crate) compiled: CompiledTemplateRef<'env, 'source>,
}

impl fmt::Debug for Template<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ds = f.debug_struct("Template");
        ds.field("name", &self.name());
        #[cfg(feature = "internal_debug")]
        {
            ds.field("instructions", &self.compiled.instructions);
            ds.field("blocks", &self.compiled.blocks);
        }
        ds.field("initial_auto_escape", &self.compiled.initial_auto_escape);
        ds.finish()
    }
}

impl<'env, 'source> Template<'env, 'source> {
    pub(crate) fn new(
        env: &'env Environment<'env>,
        compiled: CompiledTemplateRef<'env, 'source>,
    ) -> Template<'env, 'source> {
        Template { env, compiled }
    }

    /// Returns the name of the template.
    pub fn name(&self) -> &str {
        self.compiled.instructions.name()
    }

    /// Returns the source code of the template.
    pub fn source(&self) -> &str {
        self.compiled.instructions.source()
    }

    /// Renders the template into a string.
    ///
    /// The provided value is used as the initial context for the template.  It
    /// can be any object that implements [`Serialize`](serde::Serialize).  You
    /// can either create your own struct and derive `Serialize` for it or the
    /// [`context!`](crate::context) macro can be used to create an ad-hoc context.
    ///
    /// For very large contexts and to avoid the overhead of serialization of
    /// potentially unused values, you might consider using a dynamic
    /// [`Object`](crate::value::Object) as value.  For more
    /// information see [Map as Context](crate::value::Object#map-as-context).
    ///
    /// ```
    /// # use minijinja::{Environment, context, listener::DefaultRenderingEventListener};
    /// # use std::rc::Rc;
    /// # let mut env = Environment::new();
    /// # env.add_template("hello", "Hello {{ name }}!").unwrap();
    /// let tmpl = env.get_template("hello").unwrap();
    /// println!("{}", tmpl.render(context!(name => "John"), &[Rc::new(DefaultRenderingEventListener::default())]).unwrap());
    /// ```
    ///
    /// To render a single block use [`eval_to_state`](Self::eval_to_state) in
    /// combination with [`State::render_block`].
    ///
    /// **Note on values:** The [`Value`] type implements `Serialize` and can be
    /// efficiently passed to render.  It does not undergo actual serialization.
    pub fn render<S: Serialize>(
        &self,
        ctx: S,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<String, Error> {
        // reduce total amount of code faling under mono morphization into
        // this function, and share the rest in _render.
        self._render(Value::from_serialize(&ctx), listeners)
            .map(|x| x.0)
    }

    /// typechecks the template with the given context.
    #[allow(clippy::too_many_arguments)]
    pub fn typecheck(
        &self,
        funcsigns: Arc<FunctionRegistry>,
        builtins: Arc<DashMap<String, Type>>,
        warning_printer: Rc<dyn TypecheckingEventListener>,
        typecheck_resolved_context: BTreeMap<String, Value>,
    ) -> Result<(), crate::Error> {
        let instructions = &self.compiled.instructions.instructions;
        // build CFG
        let cfg = build_cfg(instructions);
        // create a typechecker
        let mut typechecker = TypeChecker::new(instructions, cfg, funcsigns, builtins);

        typechecker
            .check(warning_printer, typecheck_resolved_context)
            .map_err(|err| {
                crate::Error::new(
                    crate::error::ErrorKind::InvalidOperation,
                    format!("Type checking failed: {err}"),
                )
            })
    }

    /// Like [`render`](Self::render) but also return the evaluated [`State`].
    ///
    /// This can be used to inspect the [`State`] of the template post evaluation
    /// for instance to get fuel consumption numbers or to access globally set
    /// variables.
    ///
    /// ```
    /// # use minijinja::{Environment, context, value::Value, listener::DefaultRenderingEventListener};
    /// # use std::rc::Rc;
    /// # let mut env = Environment::new();
    /// let tmpl = env.template_from_str("{% set x = 42 %}Hello {{ what }}!").unwrap();
    /// let (rv, state) = tmpl.render_and_return_state(context!{ what => "World" }, &[Rc::new(DefaultRenderingEventListener::default())]).unwrap();
    /// assert_eq!(rv, "Hello World!");
    /// assert_eq!(state.lookup("x"), Some(Value::from(42)));
    /// ```
    ///
    /// **Note on values:** The [`Value`] type implements `Serialize` and can be
    /// efficiently passed to render.  It does not undergo actual serialization.
    pub fn render_and_return_state<S: Serialize>(
        &self,
        ctx: S,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<(String, State<'_, 'env>), Error> {
        // reduce total amount of code faling under mono morphization into
        // this function, and share the rest in _render.
        self._render(Value::from_serialize(&ctx), listeners)
    }

    fn _render(
        &self,
        root: Value,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<(String, State<'_, 'env>), Error> {
        self._eval(root, listeners)
            .map(|(rv, state)| (rv.as_str().unwrap_or_default().to_string(), state))
    }

    /// Evaluates the template into a [`State`].
    ///
    /// This evaluates the template, discards the output and returns the final
    /// `State` for introspection.  From there global variables or blocks
    /// can be accessed.  What this does is quite similar to how the engine
    /// internally works with templates that are extended or imported from.
    ///
    /// ```
    /// # use minijinja::{Environment, context, listener::DefaultRenderingEventListener};
    /// # use std::rc::Rc;
    /// # fn test() -> Result<(), minijinja::Error> {
    /// # let mut env = Environment::new();
    /// # env.add_template("hello", "")?;
    /// let tmpl = env.get_template("hello")?;
    /// let state = tmpl.eval_to_state(context!(name => "John"), &[Rc::new(DefaultRenderingEventListener::default())])?;
    /// println!("{:?}", state.exports());
    /// # Ok(()) }
    /// ```
    ///
    /// If you also want to render, use [`render_and_return_state`](Self::render_and_return_state).
    ///
    /// For more information see [`State`].
    pub fn eval_to_state<S: Serialize>(
        &self,
        ctx: S,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<State<'_, 'env>, Error> {
        self.eval_to_state_with_outer_stack_depth(ctx, listeners, 0)
    }

    /// Evaluates the template into a [`State`] with a given outer stack depth.
    pub(crate) fn eval_to_state_with_outer_stack_depth<S: Serialize>(
        &self,
        ctx: S,
        listeners: &[Rc<dyn RenderingEventListener>],
        outer_stack_depth: usize,
    ) -> Result<State<'_, 'env>, Error> {
        let root = Value::from_serialize(&ctx);
        let vm = Vm::new(self.env);
        let state = ok!(vm.eval_with_outer_stack_depth(
            &self.compiled.instructions,
            root,
            &self.compiled.blocks,
            self.compiled.initial_auto_escape,
            listeners,
            outer_stack_depth
        ))
        .1;
        Ok(state)
    }

    fn _eval(
        &self,
        root: Value,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<(Value, State<'_, 'env>), Error> {
        let vm = Vm::new(self.env);

        vm.eval(
            &self.compiled.instructions,
            root,
            &self.compiled.blocks,
            self.compiled.initial_auto_escape,
            listeners,
        )
    }

    /// Returns a set of all undeclared variables in the template.
    ///
    /// This returns a set of all variables that might be looked up
    /// at runtime by the template.  Since this is runs a static
    /// analysis, the actual control flow is not considered.  This
    /// also cannot take into account what happens due to includes,
    /// imports or extending.  If `nested` is set to `true`, then also
    /// nested trivial attribute lookups are considered and returned.
    ///
    /// ```rust
    /// # use minijinja::Environment;
    /// let mut env = Environment::new();
    /// env.add_template("x", "{% set x = foo %}{{ x }}{{ bar.baz }}").unwrap();
    /// let tmpl = env.get_template("x").unwrap();
    /// let undeclared = tmpl.undeclared_variables(false);
    /// // returns ["foo", "bar"]
    /// let undeclared = tmpl.undeclared_variables(true);
    /// // returns ["foo", "bar.baz"]
    /// ```
    pub fn undeclared_variables(&self, nested: bool) -> HashSet<String> {
        match parse(
            self.compiled.instructions.source(),
            self.name(),
            self.compiled.syntax_config.clone(),
            // TODO: this is not entirely great, but good enough for this use case.
            Default::default(),
        ) {
            Ok(ast) => find_undeclared(&ast, nested),
            Err(_) => HashSet::new(),
        }
    }

    /// Creates an empty [`State`] for this template.
    ///
    /// It's very rare that you need to actually do this but it can be useful when
    /// testing values or working with macros or other callable objects from outside
    /// the template environment.
    pub fn new_state(&self) -> State<'_, 'env> {
        State::new(
            self.env,
            Context::new(self.env.recursion_limit()),
            self.compiled.initial_auto_escape,
            &self.compiled.instructions,
            prepare_blocks(&self.compiled.blocks),
        )
    }

    /// Returns the instructions and blocks if the template is loaded from the
    /// environment.
    ///
    /// For templates loaded as string on the environment this API contract
    /// cannot be upheld because the template might not live long enough.  Under
    /// normal circumstances however such a template object would never make it
    /// to the callers of this API as this API is used for including or extending,
    /// both of which should only ever get access to a template from the environment
    /// which holds a borrowed ref.
    #[cfg(feature = "multi_template")]
    pub(crate) fn instructions_and_blocks(
        &self,
    ) -> Result<
        (
            &'env Instructions<'env>,
            &'env BTreeMap<&'env str, Instructions<'env>>,
        ),
        Error,
    > {
        match self.compiled {
            CompiledTemplateRef::Borrowed(x) => Ok((&x.instructions, &x.blocks)),
            CompiledTemplateRef::Owned(_) => Err(Error::new(
                crate::ErrorKind::InvalidOperation,
                "cannot extend or include template not borrowed from environment",
            )),
        }
    }

    /// Returns the initial auto escape setting.
    #[cfg(feature = "multi_template")]
    pub(crate) fn initial_auto_escape(&self) -> AutoEscape {
        self.compiled.initial_auto_escape
    }
}

#[derive(Clone)]
pub(crate) enum CompiledTemplateRef<'env: 'source, 'source> {
    Owned(Arc<CompiledTemplate<'source>>),
    Borrowed(&'env CompiledTemplate<'source>),
}

impl<'source> Deref for CompiledTemplateRef<'_, 'source> {
    type Target = CompiledTemplate<'source>;

    fn deref(&self) -> &Self::Target {
        match self {
            CompiledTemplateRef::Owned(ref x) => x,
            CompiledTemplateRef::Borrowed(x) => x,
        }
    }
}

/// Represents a compiled template in memory.
pub struct CompiledTemplate<'source> {
    /// The root instructions.
    pub instructions: Instructions<'source>,
    /// Block local instructions.
    pub blocks: BTreeMap<&'source str, Instructions<'source>>,
    /// Optional size hint for string rendering.
    pub buffer_size_hint: usize,
    /// The syntax config that created it.
    pub syntax_config: SyntaxConfig,
    /// The initial setting of auto escaping.
    pub initial_auto_escape: AutoEscape,
}

impl fmt::Debug for CompiledTemplate<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ds = f.debug_struct("CompiledTemplate");
        #[cfg(feature = "internal_debug")]
        {
            ds.field("instructions", &self.instructions);
            ds.field("blocks", &self.blocks);
        }
        ds.finish()
    }
}

impl<'source> CompiledTemplate<'source> {
    /// Creates a compiled template from name and source using the given settings.
    pub fn new(
        name: &'source str,
        source: &'source str,
        config: &TemplateConfig,
        filename: Option<String>,
        profile: CodeGenerationProfile,
    ) -> Result<CompiledTemplate<'source>, Error> {
        Self::_new_impl(name, source, config, filename, profile, &[], None)
    }

    /// Creates a compiled template and notifies tokenizer listeners during parsing.
    ///
    /// If `ast_visitor` is `Some`, this parse's AST is reused for it instead of parsing
    /// `source` again.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_tokenizer_listeners(
        name: &'source str,
        source: &'source str,
        config: &TemplateConfig,
        filename: Option<String>,
        profile: CodeGenerationProfile,
        source_listeners: &[Rc<dyn TokenizerEventListener>],
        ast_visitor: Option<&mut dyn FnMut(&ast::Stmt<'_>)>,
    ) -> Result<CompiledTemplate<'source>, Error> {
        Self::_new_impl(
            name,
            source,
            config,
            filename,
            profile,
            source_listeners,
            ast_visitor,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn _new_impl(
        name: &'source str,
        source: &'source str,
        config: &TemplateConfig,
        filename: Option<String>,
        profile: CodeGenerationProfile,
        source_listeners: &[Rc<dyn TokenizerEventListener>],
        ast_visitor: Option<&mut dyn FnMut(&ast::Stmt<'_>)>,
    ) -> Result<CompiledTemplate<'source>, Error> {
        // the parser/compiler combination can create constants in which case
        // we can probably benefit from the value optimization a bit.
        let _guard = value::value_optimization();
        let ast = ok!(parse_with_listeners(
            source,
            name,
            config.syntax_config.clone(),
            config.ws_config,
            source_listeners,
        ));
        if let Some(visitor) = ast_visitor {
            visitor(&ast);
        }
        let mut gen = CodeGenerator::new_with_filename(name, source, filename, profile);
        gen.compile_stmt(&ast)?;
        let buffer_size_hint = gen.buffer_size_hint();
        let (instructions, blocks) = gen.finish();
        Ok(CompiledTemplate {
            instructions,
            blocks,
            buffer_size_hint,
            syntax_config: config.syntax_config.clone(),
            initial_auto_escape: (config.default_auto_escape)(name),
        })
    }
}
