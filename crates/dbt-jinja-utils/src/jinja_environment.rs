use dbt_adapter::{Adapter, column::ColumnStatic, relation::factory::create_static_relation};
use dbt_common::{ErrorCode, FsError, FsResult, fs_err};
use minijinja::{
    Environment, Error as MinijinjaError, State, Template, UndefinedBehavior, Value,
    listener::RenderingEventListener,
    value::{ValueMap, mutable_map::MutableMap},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    rc::Rc,
    sync::{Arc, RwLock},
};
use tracy_client::span;

/// A struct that wraps a Minijinja Expression.
///
/// This is to consolidate the Minijinja::Error to FsError conversion
/// where ever we invokes directly a method from a minijinja::Expression instance in a scope that we need to return a FsResult
pub struct JinjaExpression<'env, 'source>(minijinja::Expression<'env, 'source>);

impl<'env: 'source, 'source> JinjaExpression<'env, 'source> {
    /// Evaluate the expression
    pub fn eval<S: Serialize>(
        &self,
        ctx: S,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> FsResult<Value> {
        let result = self.0.eval(ctx, listeners).map_err(|e| {
            FsError::from_jinja_err(e, "Failed to eval the compiled Jinja expression")
        })?;
        Ok(result)
    }

    /// Evaluate the expression and format matching Jinja stack frames as file-only locations.
    pub fn eval_with_file_only_stack_frame<S: Serialize>(
        &self,
        ctx: S,
        listeners: &[Rc<dyn RenderingEventListener>],
        file_only_stack_frame: &Path,
    ) -> FsResult<Value> {
        let result = self.0.eval(ctx, listeners).map_err(|e| {
            FsError::from_jinja_err_with_file_only_stack_frame(
                e,
                "Failed to eval the compiled Jinja expression",
                file_only_stack_frame,
            )
        })?;
        Ok(result)
    }

    /// Typecheck the expression
    pub fn typecheck(
        &self,
        funcsigns: Arc<minijinja::compiler::typecheck::FunctionRegistry>,
        builtins: Arc<dashmap::DashMap<String, minijinja::Type>>,
        warning_printer: Rc<dyn minijinja::TypecheckingEventListener>,
        typecheck_resolved_context: BTreeMap<String, Value>,
    ) -> FsResult<()> {
        self.0
            .typecheck(
                funcsigns,
                builtins,
                warning_printer,
                typecheck_resolved_context,
            )
            .map_err(|e| {
                Box::new(FsError::from_jinja_err(
                    e,
                    "Failed to typecheck the compiled Jinja expression",
                ))
            })
    }
}

/// A struct that wraps a Minijinja Template.
///
/// This is to consolidate the Minijinja::Error to FsError conversion
/// where ever we invokes directly a method from a minijinja::Template instance in a scope that we need to return a FsResult
pub struct JinjaTemplate<'env, 'source>(Template<'env, 'source>);

impl<'env: 'source, 'source> JinjaTemplate<'env, 'source> {
    /// Evaluates the template into a state
    pub fn eval_to_state<S: Serialize>(
        &self,
        ctx: S,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> FsResult<State<'_, '_>> {
        let result = self
            .0
            .eval_to_state(ctx, listeners)
            .map_err(|e| FsError::from_jinja_err(e, "Failed to render the Jinja template"))?;
        Ok(result)
    }

    /// Typecheck the template
    pub fn typecheck(
        &self,
        funcsigns: Arc<minijinja::compiler::typecheck::FunctionRegistry>,
        builtins: Arc<dashmap::DashMap<String, minijinja::Type>>,
        warning_printer: Rc<dyn minijinja::TypecheckingEventListener>,
        typecheck_resolved_context: BTreeMap<String, Value>,
    ) -> FsResult<()> {
        self.0
            .typecheck(
                funcsigns,
                builtins,
                warning_printer,
                typecheck_resolved_context,
            )
            .map_err(|e| {
                Box::new(FsError::from_jinja_err(
                    e,
                    "Failed to typecheck the compiled Jinja template",
                ))
            })
    }
}

/// A struct that wraps a Minijinja Environment.
#[derive(Clone)]
pub struct JinjaEnv {
    /// The Minijinja Environment instance.
    pub env: Environment<'static>,
    /// The Jinja function registry.
    pub jinja_function_registry: Arc<minijinja::compiler::typecheck::FunctionRegistry>,
    /// Macro unique-ids (as produced by `dbt-parser`'s DAG, e.g.
    /// `macro.dbt_utils.default__unpivot`) known, via the static analysis in
    /// `dbt_parser::resolve::resolve_macros::typecheck_macros`, to reach an
    /// introspective (warehouse-dependent) adapter call -- directly, or
    /// transitively through another macro they call. Populated once, after
    /// typechecking, and shared (via the `Arc<RwLock<_>>`) with every clone
    /// of this `JinjaEnv`; empty (and inert) outside `JinjaRenderMode::Symbolic`.
    pub introspective_macros: Arc<RwLock<HashSet<String>>>,
}

impl AsRef<JinjaEnv> for JinjaEnv {
    fn as_ref(&self) -> &JinjaEnv {
        self
    }
}

/// The `api` namespace (`api.Relation`, `api.Column`) for one adapter.
///
/// Both are bound to the adapter's *type*, and `Relation` additionally to its
/// quoting policy, so a node rendering against a non-default adapter needs its
/// own -- built here rather than duplicated, so the environment-wide default and
/// a per-node override cannot drift.
pub fn adapter_api_value(adapter: &Adapter) -> Value {
    let adapter_type = adapter.adapter_type();
    let mut api_map = BTreeMap::new();
    api_map.insert(
        "Relation".to_string(),
        create_static_relation(adapter_type, adapter.engine().quoting()),
    );
    api_map.insert(
        "Column".to_string(),
        Some(Value::from_object(ColumnStatic::new(adapter_type))),
    );
    Value::from_object(api_map)
}

impl JinjaEnv {
    /// Create a new JinjaEnv.
    pub fn new(env: Environment<'static>) -> Self {
        Self {
            env,
            jinja_function_registry: Arc::new(BTreeMap::new()),
            introspective_macros: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Replace the set of macro unique-ids known to reach an introspective
    /// adapter call. See `introspective_macros`'s doc comment.
    pub fn set_introspective_macros(&self, macros: HashSet<String>) {
        *self.introspective_macros.write().unwrap() = macros;
    }

    /// Whether `unique_id` is known to reach an introspective adapter call.
    /// See `introspective_macros`'s doc comment.
    pub fn is_introspective_macro(&self, unique_id: &str) -> bool {
        self.introspective_macros
            .read()
            .unwrap()
            .contains(unique_id)
    }

    /// Create a new empty state.
    pub fn empty_state(&self) -> State<'_, '_> {
        self.env.empty_state()
    }

    /// Create a new state with a pre-interned string map.
    pub fn new_state_with_context(&self, ctx: BTreeMap<String, Value>) -> State<'_, '_> {
        self.env.new_state_with_context(MutableMap::from(
            ctx.into_iter()
                .map(|(k, v)| (Value::from(k), v))
                .collect::<ValueMap>(),
        ))
    }

    /// Render a template from a string.
    pub fn render_str<S: Serialize>(
        &self,
        source: &str,
        ctx: S,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> FsResult<String> {
        let _span = span!("render_str");
        let result = self
            .env
            .render_str(source, ctx, listeners)
            .map_err(|e| FsError::from_jinja_err(e, "Failed to render the Jinja str"))?;
        Ok(result)
    }

    /// Render named template from a string.
    pub fn render_named_str<S: Serialize>(
        &self,
        name: &str,
        source: &str,
        ctx: S,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<String, MinijinjaError> {
        self.env.render_named_str(name, source, ctx, listeners)
    }

    /// Get a global variable.
    pub fn get_global(&self, name: &str) -> Option<Value> {
        self.env.get_global(name)
    }

    /// Register a global value (e.g. a helper function) available to every
    /// template rendered by this environment. Clone the environment first if
    /// you want the global scoped to a single render pass.
    pub fn add_global(&mut self, name: &str, value: Value) {
        self.env.add_global(name.to_string(), value);
    }

    /// Compile an expression.
    pub fn compile_expression<'a>(&self, expr: &'a str) -> FsResult<JinjaExpression<'_, 'a>> {
        Ok(JinjaExpression(self.env.compile_expression(expr).map_err(
            |e| FsError::from_jinja_err(e, "Failed to compile Jinja expression"),
        )?))
    }

    /// Compile a template from a string.
    pub fn template_from_str<'a>(&self, source: &'a str) -> FsResult<JinjaTemplate<'_, 'a>> {
        Ok(JinjaTemplate(self.env.template_from_str(source).map_err(
            |e| FsError::from_jinja_err(e, "Failed to compile Jinja template"),
        )?))
    }

    /// Set the adapter
    pub(crate) fn set_adapter(&mut self, adapter: Arc<Adapter>) {
        self.env.add_global("api", adapter_api_value(&adapter));

        // Add the adapter type to the environment for easy access
        self.env
            .add_global("dialect", Value::from(adapter.adapter_type().to_string()));
        self.env.add_global("adapter", adapter.as_value());
    }

    /// Get the adapter from the environment
    pub fn get_adapter(&self) -> Option<Arc<Adapter>> {
        let adapter = self.env.get_global("adapter")?;
        adapter.downcast_object::<Adapter>()
    }

    /// Get the adapter reference from the environment.
    pub fn get_adapter_ref(&self) -> Option<&Adapter> {
        let adapter = self.env.get_global_ref("adapter")?;
        adapter.downcast_object_ref::<Adapter>()
    }

    /// Get the adapter from the environment
    pub fn get_base_adapter(&self) -> Option<Arc<Adapter>> {
        self.get_adapter().map(|bridge| bridge as Arc<Adapter>)
    }

    /// Set the undefined behavior for the environment.
    pub(crate) fn set_undefined_behavior(&mut self, behavior: UndefinedBehavior) {
        self.env.set_undefined_behavior(behavior);
    }

    /// Check if a template exists.
    pub fn has_template(&self, name: &str) -> bool {
        self.env.get_template(name).is_ok()
    }

    /// Get a template from the environment.
    pub fn get_template(&self, name: &str) -> FsResult<JinjaTemplate<'_, '_>> {
        if !self.has_template(name) {
            return Err(fs_err!(
                ErrorCode::JinjaError,
                "Template not found: {}",
                name
            ));
        }
        let result = self
            .env
            .get_template(name)
            .map_err(|e| FsError::from_jinja_err(e, "Failed to get template"))?;
        Ok(JinjaTemplate(result))
    }

    /// Get the dbt and adapters namespace for `dialect`.
    pub fn get_dbt_and_adapters_namespace(&self, dialect: &str) -> Arc<ValueMap> {
        self.env.get_dbt_and_adapters_namespace(dialect)
    }

    /// The full `dialect -> namespace` map, for passing into a typecheck context.
    pub fn get_dbt_and_adapters_namespaces(&self) -> Arc<ValueMap> {
        self.env.get_dbt_and_adapters_namespaces()
    }

    /// Get the target context.
    pub fn get_target_context(&self) -> Arc<BTreeMap<String, String>> {
        self.env
            .get_global("target")
            .unwrap_or_default()
            .downcast_object::<BTreeMap<String, String>>()
            .unwrap_or_default()
    }
}
