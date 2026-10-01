use crate::compiler::instructions::Instructions;
use crate::constants::{CURRENT_PATH, CURRENT_SPAN};
use crate::dispatch_object::{macro_namespace_template_resolver, DispatchObject};
use crate::environment::Environment;
use crate::error::{Error, ErrorKind};
use crate::listener::RenderingEventListener;
use crate::machinery::Span;
use crate::output::Output;
use crate::template::Template;
use crate::utils::{AutoEscape, UndefinedBehavior};
use crate::value::mutable_map::MutableMap;
use crate::value::{ArgType, Value};
use crate::vm::context::Context;

#[cfg(feature = "fuel")]
use crate::vm::fuel::FuelTracker;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;
use std::rc::Rc;

/// When macros are used, the state carries an `id` counter.  Whenever a state is
/// created, the counter is incremented.  This exists because macros can keep a reference
/// to instructions from another state by index.  Without this counter it would
/// be possible for a macro to be called with a different state (different id)
/// which mean we likely panic.
#[cfg(feature = "macros")]
static STATE_ID: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Provides access to the current execution state of the engine.
///
/// A read only reference is passed to filter functions and similar objects to
/// allow limited interfacing with the engine.  The state is useful to look up
/// information about the engine in filter, test or global functions.  It not
/// only provides access to the template environment but also the context
/// variables of the engine, the current auto escaping behavior as well as the
/// auto escape flag.
///
/// In some testing scenarios or more advanced use cases you might need to get
/// a [`State`].  The state is managed as part of the template execution but the
/// initial state can be retrieved via [`Template::new_state`](crate::Template::new_state).
/// The most common way to get hold of the state however is via functions of filters.
///
/// **Notes on lifetimes:** the state object exposes some of the internal
/// lifetimes through the type.  You should always elide these lifetimes
/// as there might be lifetimes added or removed between releases.
pub struct State<'template, 'env> {
    pub(crate) env: &'env Environment<'env>,
    pub(crate) ctx: Context<'env>,
    pub(crate) current_block: Option<&'env str>,
    pub(crate) auto_escape: AutoEscape,
    pub(crate) instructions: &'template Instructions<'env>,
    pub(crate) blocks: BTreeMap<&'env str, BlockStack<'template, 'env>>,
    pub(crate) loaded_templates: BTreeSet<&'env str>,
    #[cfg(feature = "macros")]
    pub(crate) id: isize,
    #[cfg(feature = "macros")]
    pub(crate) macros: std::sync::Arc<Vec<(&'template Instructions<'env>, usize)>>,
    #[cfg(feature = "macros")]
    pub(crate) closure_tracker: std::sync::Arc<crate::vm::closure_object::ClosureTracker>,
    #[cfg(feature = "fuel")]
    pub(crate) fuel_tracker: Option<std::sync::Arc<FuelTracker>>,

    // current program counter, updated by the vm eval loop, is only used for
    // error reporting in filters/functions:
    pub(crate) pc: usize,

    // Location of the call expression that is about to invoke a macro, captured
    // by the vm just before the call (see `record_pending_call_site`) and read
    // when the macro starts executing. Only populated when a listener opts into
    // call-site tracking.
    pub(crate) pending_call_site: Option<(PathBuf, Span)>,
}

impl fmt::Debug for State<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut ds = f.debug_struct("State");
        ds.field("name", &self.instructions.name());
        ds.field("current_block", &self.current_block);
        ds.field("auto_escape", &self.auto_escape);
        ds.field("ctx", &self.ctx);
        ds.field("env", &self.env);
        ds.finish()
    }
}

impl<'template, 'env> State<'template, 'env> {
    /// Creates a new state.
    pub(crate) fn new(
        env: &'env Environment,
        ctx: Context<'env>,
        auto_escape: AutoEscape,
        instructions: &'template Instructions<'env>,
        blocks: BTreeMap<&'env str, BlockStack<'template, 'env>>,
    ) -> State<'template, 'env> {
        State {
            #[cfg(feature = "macros")]
            id: STATE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            env,
            ctx,
            current_block: None,
            auto_escape,
            instructions,
            blocks,
            loaded_templates: BTreeSet::new(),
            #[cfg(feature = "macros")]
            macros: Default::default(),
            #[cfg(feature = "macros")]
            closure_tracker: Default::default(),
            #[cfg(feature = "fuel")]
            fuel_tracker: env.fuel().map(FuelTracker::new),

            pc: 0,
            pending_call_site: None,
        }
    }

    /// Records the location of the call expression that is about to invoke a
    /// macro, so the macro can be reported with its call site. `span` is the
    /// raw span of the call instruction; it is offset into the current template
    /// context to match the locations used in error stack traces.
    ///
    /// This is a no-op unless a listener opts into call-site tracking, to avoid
    /// the path clone on the hot call path during normal rendering.
    pub(crate) fn record_pending_call_site(
        &mut self,
        listeners: &[Rc<dyn RenderingEventListener>],
        span: &Span,
    ) {
        if listeners.iter().any(|l| l.tracks_macro_call_sites()) {
            self.pending_call_site = Some((
                self.ctx.current_path.clone(),
                span.with_offset(&self.ctx.current_span),
            ));
        }
    }

    /// Helper method to attach span information to an error.
    /// This reduces code duplication by centralizing the common pattern of
    /// `error.with_span(&state.ctx.current_path.clone(), &span.with_offset(&state.ctx.current_span))`.
    #[inline]
    pub(crate) fn with_span_error(&self, error: Error, span: &Span) -> Error {
        error.with_span(
            &self.ctx.current_path.clone(),
            &span.with_offset(&self.ctx.current_span),
        )
    }

    /// Returns a [Span] corresponding to the current instruction.
    pub fn current_span(&self) -> Span {
        self.instructions.get_span(self.pc).map_or_else(
            || self.ctx.current_span,
            |s| s.with_offset(&self.ctx.current_span),
        )
    }

    /// Returns the raw [Span] of the current instruction without applying the context offset.
    /// This is useful for tracking source locations within the current template file.
    pub fn current_instruction_span(&self) -> Span {
        self.instructions.get_span(self.pc).unwrap_or_default()
    }

    /// Returns the current path of the template being rendered.
    pub fn current_path(&self) -> &PathBuf {
        &self.ctx.current_path
    }

    /// Returns a [Span] of the context.
    pub fn current_span_of_context(&self) -> Span {
        self.ctx.current_span
    }

    /// Creates an empty state for an environment.
    pub fn new_for_env(env: &'env Environment) -> State<'env, 'env> {
        State::new(
            env,
            Context::new(env.recursion_limit()),
            AutoEscape::None,
            &crate::compiler::instructions::EMPTY_INSTRUCTIONS,
            BTreeMap::new(),
        )
    }

    /// Returns a reference to the current environment.
    #[inline(always)]
    pub fn env(&self) -> &Environment<'_> {
        self.env
    }

    /// Returns true during either the hidden Parse phase
    /// or phases after Compile (including it)
    pub fn is_execute(&self) -> bool {
        self.lookup("execute", &[])
            .map(|v| v.is_true())
            .unwrap_or_default()
    }

    /// Returns the base context of the state and add file_stack to it.
    /// This should always be a mutable map wrapped in a Value:from_object
    pub fn get_base_context(&self) -> Value {
        let base = self.ctx.clone_base();
        // Add file_stack to the base value
        if let Some(obj) = base.as_object() {
            if let Some(map) = obj.downcast_ref::<MutableMap>() {
                let map = map.clone();
                map.insert(
                    Value::from(CURRENT_PATH),
                    Value::from(self.ctx.current_path.clone().to_string_lossy()),
                );
                map.insert(
                    Value::from(CURRENT_SPAN),
                    Value::from_serialize(self.ctx.current_span),
                );
                Value::from_object(map)
            } else {
                Value::from_object(MutableMap::new())
            }
        } else {
            Value::from_object(MutableMap::new())
        }
    }

    /// Returns the base context of the state and add file_stack to it.
    /// This should always be a mutable map wrapped in a Value:from_object
    pub fn get_base_context_with_path_and_span(&self, path: &Value, span: &Value) -> Value {
        let base = self.ctx.clone_base();
        // Add file_stack to the base value
        if let Some(obj) = base.as_object() {
            if let Some(map) = obj.downcast_ref::<MutableMap>() {
                let map = map.clone();
                map.insert(Value::from(CURRENT_PATH), path.clone());
                map.insert(Value::from(CURRENT_SPAN), span.clone());
                Value::from_object(map)
            } else {
                Value::from_object(MutableMap::new())
            }
        } else {
            Value::from_object(MutableMap::new())
        }
    }

    /// Returns the name of the current template.
    pub fn name(&self) -> &str {
        self.instructions.name()
    }

    /// Returns the current value of the auto escape flag.
    #[inline(always)]
    pub fn auto_escape(&self) -> AutoEscape {
        self.auto_escape
    }

    /// Returns the current undefined behavior.
    #[inline(always)]
    pub fn undefined_behavior(&self) -> UndefinedBehavior {
        self.env.undefined_behavior()
    }

    /// Returns the name of the innermost block.
    #[inline(always)]
    pub fn current_block(&self) -> Option<&str> {
        self.current_block
    }

    /// Looks up a variable by name in the context.
    ///
    /// # Note on Closures
    ///
    /// Macros and call blocks analyze which variables are referenced and
    /// create closures for them.  This means that unless a variable is defined
    /// as a [global](Environment::add_global) in the environment or it was
    /// referenced by a macro, this method won't be able to find it.
    #[inline(always)]
    pub fn lookup(
        &self,
        name: &str,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Option<Value> {
        if let Some(val) = self.ctx.load(self.env, name) {
            return Some(val);
        }

        macro_namespace_template_resolver(self, name, &mut Vec::new()).and_then(|template_name| {
            listeners
                .iter()
                .for_each(|l| l.on_macro_dependency(&template_name));
            if let Some((pkg, macro_name)) = template_name.split_once('.') {
                Some(Value::from_object(DispatchObject {
                    macro_name: macro_name.to_string(),
                    package_name: Some(pkg.to_string()),
                    strict: true,
                    auto_execute: false,
                    context: Some(self.get_base_context()),
                }))
            } else {
                None
            }
        })
    }

    /// Looks up a global macro and calls it, converting the result into a string.
    ///
    /// This looks up a value as [`lookup`](Self::lookup) does and calls it
    /// with the passed args.
    #[cfg(feature = "macros")]
    #[cfg_attr(docsrs, doc(cfg(feature = "macros")))]
    pub fn call_macro(
        &self,
        name: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<String, Error> {
        self.call_macro_raw(name, args, listeners).map(Into::into)
    }

    /// Looks up a global macro and calls it without converting the result into a string.
    ///
    /// This looks up a value as [`lookup`](Self::lookup) does and calls it
    /// with the passed args.
    #[cfg(feature = "macros")]
    #[cfg_attr(docsrs, doc(cfg(feature = "macros")))]
    pub fn call_macro_raw(
        &self,
        name: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        let f = ok!(self.lookup(name, listeners).ok_or_else(|| Error::new(
            crate::error::ErrorKind::UnknownFunction,
            "macro not found"
        )));
        f.call(self, args, listeners)
    }

    /// Renders a block with the given name into a string.
    ///
    /// This method works like [`Template::render`](crate::Template::render) but
    /// it only renders a specific block in the template.  The first argument is
    /// the name of the block.
    ///
    /// This renders only the block `hi` in the template:
    ///
    /// ```
    /// # use minijinja::{Environment, context, listener::DefaultRenderingEventListener};
    /// # use std::rc::Rc;
    /// # fn test() -> Result<(), minijinja::Error> {
    /// # let mut env = Environment::new();
    /// # env.add_template("hello", "{% block hi %}Hello {{ name }}!{% endblock %}")?;
    /// let tmpl = env.get_template("hello")?;
    /// let rv = tmpl
    ///     .eval_to_state(context!(name => "John"), &[Rc::new(DefaultRenderingEventListener::default())])?
    ///     .render_block("hi", &[Rc::new(DefaultRenderingEventListener::default())])?;
    /// println!("{}", rv);
    /// # Ok(()) }
    /// ```
    ///
    /// Note that rendering a block is a stateful operation.  If an error
    /// is returned the module has to be re-created as the internal state
    /// can end up corrupted.  This also means you can only render blocks
    /// if you have a mutable reference to the state which is not possible
    /// from within filters or similar.
    #[cfg(feature = "multi_template")]
    #[cfg_attr(docsrs, doc(cfg(feature = "multi_template")))]
    pub fn render_block(
        &mut self,
        block: &str,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<String, Error> {
        let value = crate::vm::Vm::new(self.env).call_block(block, self, listeners)?;
        Ok(value.as_str().unwrap().to_string())
    }

    /// Returns a list of the names of all exports (top-level variables).
    pub fn exports(&self) -> Vec<&str> {
        self.ctx.exports().keys().copied().collect()
    }

    /// Fetches a template by name with path joining.
    ///
    /// This works like [`Environment::get_template`] with the difference that the lookup
    /// undergoes path joining.  If the environment has a configured path joining callback,
    /// it will be invoked with the name of the current template as parent template.
    ///
    /// For more information see [`Environment::set_path_join_callback`].
    pub fn get_template(&self, name: &str) -> Result<Template<'env, 'env>, Error> {
        self.env
            .get_template(&self.env.join_template_path(name, self.name()))
    }

    /// Invokes a filter with some arguments.
    ///
    /// ```
    /// # use minijinja::Environment;
    /// # let mut env = Environment::new();
    /// # env.add_filter("upper", |x: &str| x.to_uppercase());
    /// # let tmpl = env.template_from_str("").unwrap();
    /// # let state = tmpl.new_state();
    /// let rv = state.apply_filter("upper", &["hello world".into()]).unwrap();
    /// assert_eq!(rv.as_str(), Some("HELLO WORLD"));
    /// ```
    pub fn apply_filter(&self, filter: &str, args: &[Value]) -> Result<Value, Error> {
        match self.env.get_filter(filter) {
            Some(filter) => filter.apply_to(self, args),
            None => Err(Error::from(ErrorKind::UnknownFilter)),
        }
    }

    /// Invokes a test function on a value.
    ///
    /// ```
    /// # use minijinja::Environment;
    /// # let mut env = Environment::new();
    /// # env.add_test("even", |x: i32| x % 2 == 0);
    /// # let tmpl = env.template_from_str("").unwrap();
    /// # let state = tmpl.new_state();
    /// let rv = state.perform_test("even", &[42i32.into()]).unwrap();
    /// assert!(rv);
    /// ```
    pub fn perform_test(&self, test: &str, args: &[Value]) -> Result<bool, Error> {
        match self.env.get_test(test) {
            Some(test) => test.perform(self, args),
            None => Err(Error::from(ErrorKind::UnknownTest)),
        }
    }

    /// Formats a value to a string using the formatter on the environment.
    ///
    /// ```
    /// # use minijinja::{value::Value, Environment};
    /// # let mut env = Environment::new();
    /// # let tmpl = env.template_from_str("").unwrap();
    /// # let state = tmpl.new_state();
    /// let rv = state.format(Value::from(42)).unwrap();
    /// assert_eq!(rv, "42");
    /// ```
    pub fn format(&self, value: Value) -> Result<String, Error> {
        let mut rv = String::new();
        let mut out = Output::with_string(&mut rv);
        self.env.format(&value, self, &mut out).map(|_| rv)
    }

    /// Returns the fuel levels.
    ///
    /// When the fuel feature is enabled, during evaluation the template will keep
    /// track of how much fuel it has consumed.  If the fuel tracker is turned on
    /// the returned value will be `Some((consumed, remaining))`.  If fuel tracking
    /// is not enabled, `None` is returned instead.
    #[cfg(feature = "fuel")]
    #[cfg_attr(docsrs, doc(cfg(feature = "fuel")))]
    pub fn fuel_levels(&self) -> Option<(u64, u64)> {
        self.fuel_tracker
            .as_ref()
            .map(|x| (x.consumed(), x.remaining()))
    }
}

impl<'a> ArgType<'a> for &State<'_, '_> {
    type Output = &'a State<'a, 'a>;

    fn from_value(_value: Option<&'a Value>) -> Result<Self::Output, Error> {
        Err(Error::new(
            ErrorKind::InvalidOperation,
            "cannot use state type in this position",
        ))
    }

    fn from_state_and_value(
        state: Option<&'a State>,
        _value: Option<&'a Value>,
    ) -> Result<(Self::Output, usize), Error> {
        match state {
            None => Err(Error::new(ErrorKind::InvalidOperation, "state unavailable")),
            Some(state) => Ok((state, 0)),
        }
    }
}

/// Tracks a block and it's parents for super.
#[derive(Default)]
pub(crate) struct BlockStack<'template, 'env> {
    instructions: Vec<&'template Instructions<'env>>,
    depth: usize,
}

impl<'template, 'env> BlockStack<'template, 'env> {
    pub fn new(instructions: &'template Instructions<'env>) -> BlockStack<'template, 'env> {
        BlockStack {
            instructions: vec![instructions],
            depth: 0,
        }
    }

    pub fn instructions(&self) -> &'template Instructions<'env> {
        self.instructions.get(self.depth).copied().unwrap()
    }

    pub fn push(&mut self) -> bool {
        if self.depth + 1 < self.instructions.len() {
            self.depth += 1;
            true
        } else {
            false
        }
    }

    #[track_caller]
    pub fn pop(&mut self) {
        self.depth = self.depth.checked_sub(1).unwrap()
    }

    #[cfg(feature = "multi_template")]
    pub fn append_instructions(&mut self, instructions: &'template Instructions<'env>) {
        self.instructions.push(instructions);
    }
}
