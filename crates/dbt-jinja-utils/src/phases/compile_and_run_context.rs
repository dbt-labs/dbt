//! This module contains the functions for initializing the Jinja environment for the compile phase.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Utc};

use crate::functions::build_flat_graph;
use crate::invocation_graph::invocation_graph;
use crate::jinja_environment::JinjaEnv;
use crate::phases::compile::DependencyValidationConfig;
use dbt_adapter::Adapter;
use dbt_adapter::load_store::ResultStore;
use dbt_adapter::relation::RelationObject;
use dbt_common::once_cell_vars::DISPATCH_CONFIG;
use dbt_schemas::filter::{RunFilter, Sample};
use dbt_schemas::schemas::Nodes;
use dbt_schemas::state::{DbtRuntimeConfig, NodeResolverTracker};
use dbt_telemetry::NodeType;
use minijinja::arg_utils::ArgParser;
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Object, ValueMap};
use minijinja::{
    Error as MinijinjaError, ErrorKind as MinijinjaErrorKind, Value as MinijinjaValue,
};
use minijinja::{State, UndefinedBehavior};
use std::rc::Rc;

use dbt_jinja_ctx::{CompileBaseCtx, DummyConfig, JinjaObject, OperationCtx, to_jinja_btreemap};

/// Configure the Jinja environment for the compile phase.
pub fn configure_compile_and_run_jinja_environment(env: &mut JinjaEnv, adapter: Arc<Adapter>) {
    env.set_adapter(adapter);
    env.set_undefined_behavior(UndefinedBehavior::Lenient);
}

/// Build the truly-shared compile/run base ctx — values that are identical
/// across REPL, `run-operation`, per-node compile, and per-node run renders.
/// Leaf ctxs ([`OperationCtx`], `CompileNodeCtx`, `RunNodeCtx`) hold this via
/// `#[serde(flatten)]` and add their scope-specific overlay.
///
/// `defer_nodes`, when supplied (compile/run with `--defer --state`), drives
/// the `defer_relation` field on each deferrable graph node. (#1366)
pub fn build_compile_base_ctx(
    node_resolver: Arc<dyn NodeResolverTracker>,
    package_name: &str,
    nodes: &Nodes,
    defer_nodes: Option<&Nodes>,
    runtime_config: Arc<DbtRuntimeConfig>,
    namespace_keys: Vec<String>,
    result_store: Option<ResultStore>,
) -> CompileBaseCtx {
    // Wrap each per-namespace search order as `Value::from(Vec<String>)` —
    // dispatch lookup downcasts to `Vec<String>` so the underlying Object
    // type must be exactly that, not the `MutableVec<Value>` that
    // serde-serializing a `Vec<String>` produces. Same downcast contract as
    // `ResolveBaseCtx::macro_dispatch_order`.
    let macro_dispatch_order: BTreeMap<String, MinijinjaValue> = DISPATCH_CONFIG
        .get()
        .map(|macro_dispatch_order| {
            macro_dispatch_order
                .read()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), MinijinjaValue::from(v.clone())))
                .collect()
        })
        .unwrap_or_default();

    // Create a BTreeMap for builtins
    let mut builtins = BTreeMap::new();

    // Create base ref function for macros (without validation)
    let ref_function = RefFunction::new_unvalidated(
        node_resolver.clone(),
        package_name.to_owned(),
        runtime_config.clone(),
    );
    let ref_value = MinijinjaValue::from_object(ref_function);
    builtins.insert("ref".to_string(), ref_value.clone());

    // Create source function
    let source_function = SourceFunction::new_unvalidated(
        node_resolver.clone(),
        package_name.to_owned(),
        runtime_config.clone(),
    );
    let source_value = MinijinjaValue::from_object(source_function);
    builtins.insert("source".to_string(), source_value.clone());

    // Create function function
    let function_function = FunctionFunction::new_unvalidated(
        node_resolver.clone(),
        package_name.to_owned(),
        runtime_config.clone(),
    );
    let function_value = MinijinjaValue::from_object(function_function);
    builtins.insert("function".to_string(), function_value.clone());

    // Populate dbt_metadata_envs from OS env vars with prefix DBT_ENV_CUSTOM_ENV_.
    // Mirrors dbt-core behavior so packages can safely iterate .items().
    let meta_envs: BTreeMap<String, MinijinjaValue> =
        dbt_common::constants::collect_dbt_custom_envs()
            .into_iter()
            .map(|(k, v)| (k, MinijinjaValue::from(v)))
            .collect();

    let mut packages: BTreeSet<String> = runtime_config.dependencies.keys().cloned().collect();
    packages.insert(package_name.to_string());

    let result_store = result_store.unwrap_or_default();

    let dbt_namespaces: BTreeMap<String, JinjaObject<DbtNamespace>> = namespace_keys
        .into_iter()
        .map(|key| {
            let value = JinjaObject::new(DbtNamespace::new(&key));
            (key, value)
        })
        .collect();

    CompileBaseCtx {
        macro_dispatch_order,
        ref_fn: ref_value,
        source: source_value,
        function: function_value,
        // Used in macros to gate the sql execution (set to true only after
        // parse stage); e.g. dbt_macro_assets/dbt-adapters/macros/etc/statement.sql.
        execute: true,
        builtins: MinijinjaValue::from_object(builtins),
        dbt_metadata_envs: MinijinjaValue::from_object(meta_envs),
        context: JinjaObject::new(MacroLookupContext::new(
            package_name.to_string(),
            None,
            packages,
        )),
        graph: MinijinjaValue::from_object(LazyFlatGraph::new(nodes, defer_nodes)),
        store_result: MinijinjaValue::from_function(result_store.store_result()),
        load_result: MinijinjaValue::from_function(result_store.load_result()),
        target_package_name: package_name.to_string(),
        node: MinijinjaValue::NONE,
        connection_name: String::new(),
        dbt_namespaces,
    }
}

/// Build the operation-scope ctx (REPL / `run-operation` / pre-compile macro
/// evaluation): the shared base values plus a no-op [`DummyConfig`] for
/// `{{ config(...) }}` calls in macros that aren't tied to a specific node.
pub fn build_operation_context(
    node_resolver: Arc<dyn NodeResolverTracker>,
    package_name: &str,
    nodes: &Nodes,
    defer_nodes: Option<&Nodes>,
    runtime_config: Arc<DbtRuntimeConfig>,
    namespace_keys: Vec<String>,
    base_ctx: Option<CompileBaseCtx>,
) -> OperationCtx {
    let base = base_ctx.unwrap_or_else(|| {
        build_compile_base_ctx(
            node_resolver,
            package_name,
            nodes,
            defer_nodes,
            runtime_config,
            namespace_keys,
            None,
        )
    });
    OperationCtx {
        base,
        config: JinjaObject::new(DummyConfig {}),
    }
}

/// Backward-compat shim: today's callers consume the operation-scope context
/// as a `BTreeMap<String, MinijinjaValue>`. Wraps [`build_operation_context`]
/// + [`to_jinja_btreemap`] so call-site migration can land separately.
///
/// New callers should use [`build_operation_context`] or
/// [`build_compile_base_ctx`] directly.
pub fn build_operation_context_btreemap(
    node_resolver: Arc<dyn NodeResolverTracker>,
    package_name: &str,
    nodes: &Nodes,
    defer_nodes: Option<&Nodes>,
    runtime_config: Arc<DbtRuntimeConfig>,
    namespace_keys: Vec<String>,
    base_ctx: Option<CompileBaseCtx>,
) -> BTreeMap<String, MinijinjaValue> {
    to_jinja_btreemap(&build_operation_context(
        node_resolver,
        package_name,
        nodes,
        defer_nodes,
        runtime_config,
        namespace_keys,
        base_ctx,
    ))
}

// `DbtNamespace` moved to `dbt_jinja_ctx::objects::lookup`. Re-exported here
// so existing call sites in this crate (`build_resolve_context`,
// `build_operation_context_btreemap`, etc.) keep importing the type from
// the path they always have. The transitional re-export is removed once
// every call site has been migrated to consume `dbt-jinja-ctx` directly.
pub use dbt_jinja_ctx::DbtNamespace;

/// Context for microbatch ref filtering.
///
/// When this is set on a `RefFunction`, refs to models with event_time
/// configured will be filtered to only include rows within the batch window.
#[derive(Debug, Clone)]
pub struct MicrobatchRefContext {
    /// Start of the batch window (inclusive)
    pub event_time_start: DateTime<Utc>,
    /// End of the batch window (exclusive)
    pub event_time_end: DateTime<Utc>,
    /// Mapping of unique_id -> event_time column name
    pub event_time_mapping: Arc<BTreeMap<String, String>>,
}

impl MicrobatchRefContext {
    /// Create a new MicrobatchRefContext.
    pub fn new(
        event_time_start: DateTime<Utc>,
        event_time_end: DateTime<Utc>,
        event_time_mapping: Arc<BTreeMap<String, String>>,
    ) -> Self {
        Self {
            event_time_start,
            event_time_end,
            event_time_mapping,
        }
    }

    /// Get the event_time column for a given unique_id, if configured.
    pub fn get_event_time(&self, unique_id: &str) -> Option<&str> {
        self.event_time_mapping.get(unique_id).map(|s| s.as_str())
    }

    /// Create a RunFilter for this batch context and a specific event_time column.
    pub fn to_run_filter(&self) -> RunFilter {
        RunFilter {
            empty: false,
            sample: Some(Sample {
                start: Some(self.event_time_start),
                end: Some(self.event_time_end),
            }),
        }
    }
}

/// Function for resolving ref() calls in Jinja templates.
///
/// This function supports:
/// - Basic ref resolution: `ref('model_name')`
/// - Package-qualified refs: `ref('package_name', 'model_name')`
/// - Versioned refs: `ref('model_name', version=1)`
/// - Microbatch-aware filtering when `microbatch_context` is set
#[derive(Debug)]
pub struct RefFunction {
    node_resolver: Arc<dyn NodeResolverTracker>,
    package_name: String,
    runtime_config: Arc<DbtRuntimeConfig>,
    /// Validation configuration
    validation_config: DependencyValidationConfig,
    /// Optional microbatch context for filtering refs during batch execution
    microbatch_context: Option<MicrobatchRefContext>,
    /// The unique_id of the node that owns this ref context.
    /// Used for O(1) defer decisions via `NodeResolver::prefers_deferred`.
    current_node_unique_id: String,
}

impl RefFunction {
    /// Create a new RefFunction without validation (for base context)
    pub fn new_unvalidated(
        node_resolver: Arc<dyn NodeResolverTracker>,
        package_name: String,
        runtime_config: Arc<DbtRuntimeConfig>,
    ) -> Self {
        Self {
            node_resolver,
            package_name,
            runtime_config,
            // Default; no validation
            validation_config: DependencyValidationConfig::default(),
            microbatch_context: None,
            current_node_unique_id: String::new(),
        }
    }

    /// Create a new RefFunction with validation (for node context)
    pub fn new_with_validation(
        node_resolver: Arc<dyn NodeResolverTracker>,
        package_name: String,
        runtime_config: Arc<DbtRuntimeConfig>,
        validation_config: DependencyValidationConfig,
        current_node_unique_id: String,
    ) -> Self {
        Self {
            node_resolver,
            package_name,
            runtime_config,
            validation_config,
            microbatch_context: None,
            current_node_unique_id,
        }
    }

    /// Create a new RefFunction with microbatch context for filtering refs.
    ///
    /// This is used during microbatch execution to filter refs to models
    /// with event_time configured to only include rows within the batch window.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_microbatch_context(
        node_resolver: Arc<dyn NodeResolverTracker>,
        package_name: String,
        runtime_config: Arc<DbtRuntimeConfig>,
        validation_config: DependencyValidationConfig,
        microbatch_context: MicrobatchRefContext,
        current_node_unique_id: String,
    ) -> Self {
        Self {
            node_resolver,
            package_name,
            runtime_config,
            validation_config,
            microbatch_context: Some(microbatch_context),
            current_node_unique_id,
        }
    }

    /// Set the microbatch context on this RefFunction.
    ///
    /// Returns a new RefFunction with the microbatch context set.
    pub fn with_microbatch_context(self, microbatch_context: MicrobatchRefContext) -> Self {
        Self {
            microbatch_context: Some(microbatch_context),
            ..self
        }
    }

    fn resolve_args(
        &self,
        args: &[MinijinjaValue],
    ) -> Result<(Option<String>, String, Option<String>), MinijinjaError> {
        if args.is_empty() || args.len() > 4 {
            return Err(MinijinjaError::new(
                MinijinjaErrorKind::InvalidOperation,
                "invalid number of arguments for ref macro",
            ));
        }
        let mut parser = ArgParser::new(args, None);
        // If there are two positional args, the first is the package name and the second is the model name
        let arg0 = parser.get::<String>("")?;
        let arg1 = parser.get_optional::<String>("");
        let (namespace, model_name) = match (arg0, arg1) {
            (namespace, Some(model_name)) => (Some(namespace), model_name),
            (model_name, None) => (None, model_name),
        };
        let version = parser.consume_optional_either_from_kwargs::<String>("version", "v");

        let package_name = namespace;

        if let Some(v) = version {
            Ok((package_name, model_name, Some(v)))
        } else {
            Ok((package_name, model_name, None))
        }
    }

    /// Validate that the referenced model is in the allowed dependencies
    fn validate_dependency(
        &self,
        unique_id: &str,
        package_name: &Option<String>,
        model_name: &str,
    ) -> Result<(), MinijinjaError> {
        if self.validation_config.skip_validation {
            return Ok(());
        }

        if self
            .validation_config
            .allowed_dependencies
            .contains(unique_id)
        {
            Ok(())
        } else {
            // Construct the ref string for the error message
            let ref_string = if let Some(pkg) = package_name {
                format!("{{{{ ref('{pkg}', '{model_name}') }}}}")
            } else {
                format!("{{{{ ref('{model_name}') }}}}")
            };

            if self.validation_config.node_type == NodeType::UnitTest {
                let unit_test_name = self
                    .validation_config
                    .current_node_unique_id
                    .as_deref()
                    .and_then(|uid| uid.rsplit('.').next())
                    .unwrap_or("<unknown>");
                let ref_call = if let Some(pkg) = package_name {
                    format!("ref('{pkg}', '{model_name}')")
                } else {
                    format!("ref('{model_name}')")
                };
                Err(MinijinjaError::new(
                    MinijinjaErrorKind::InvalidOperation,
                    format!(
                        "Unit test '{unit_test_name}' references {{{{ {ref_call} }}}}, \
but this dependency was not mocked.\n\n\
Add it to the unit test's `given` block:\n  \
- input: {ref_call}\n    \
rows: [...]\n\n\
Or remove the ref() from the model if it's unused."
                    ),
                ))
            } else {
                Err(MinijinjaError::new(
                    MinijinjaErrorKind::InvalidOperation,
                    format!(
                        "dbt was unable to infer all dependencies for the model \"{model_name}\". This typically happens when ref() is placed within a conditional block.
To fix this, add the following hint to the top of the model \"{model_name}\":
-- depends_on: {ref_string}"
                    ),
                ))
            }
        }
    }
}

impl Object for RefFunction {
    fn call(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        args: &[MinijinjaValue],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        let (package_name, model_name, version) = self.resolve_args(args)?;

        match self.node_resolver.lookup_ref(
            &package_name,
            &model_name,
            &version,
            &Some(self.package_name.clone()),
        ) {
            Ok((unique_id, relation, _, deferred_relation)) => {
                // Validate that this ref is allowed (only if validation is configured)
                self.validate_dependency(&unique_id, &package_name, &model_name)?;
                // Use phase-aware defer logic: check if we should use the deferred
                // (production) relation for this specific upstream node.
                let prefers = self
                    .node_resolver
                    .prefers_deferred(&self.current_node_unique_id, &unique_id);
                let resolved_relation = match (prefers, deferred_relation) {
                    (true, Some(deferred)) => deferred,
                    _ => relation,
                };

                for listener in listeners {
                    listener.on_ref_or_source_resolved(&unique_id);
                }

                // Apply microbatch filtering if we have a microbatch context and
                // the referenced model has event_time configured
                if let Some(ref microbatch_ctx) = self.microbatch_context {
                    if let Some(event_time_col) = microbatch_ctx.get_event_time(&unique_id) {
                        // Extract the RelationObject and apply the filter
                        if let Some(relation_obj) = resolved_relation
                            .as_object()
                            .and_then(|obj| obj.downcast_ref::<RelationObject>())
                        {
                            // TODO(chasewalden): What happens if there is already a run filter due to `--sample`?
                            //  What if the existing run filter's event_time_col doesn't agree with the one we pass?
                            let filtered = relation_obj.with_filter(
                                microbatch_ctx.to_run_filter(),
                                Some(event_time_col.to_string()),
                            );
                            return Ok(filtered.into_value());
                        }
                    }
                }

                Ok(resolved_relation)
            }
            Err(_) => Err(MinijinjaError::new(
                MinijinjaErrorKind::NonKey,
                format!(
                    "ref not found for package: {}, model: {}, version: {:?}",
                    self.package_name, model_name, version
                ),
            )),
        }
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        method: &str,
        args: &[MinijinjaValue],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        match method {
            "id" => {
                let (package_name, model_name, version) = self.resolve_args(args)?;
                match self.node_resolver.lookup_ref(
                    &package_name,
                    &model_name,
                    &version,
                    &Some(self.package_name.clone()),
                ) {
                    Ok((unique_id, _, _, _)) => {
                        // Validate that this ref is allowed (only if validation is configured)
                        self.validate_dependency(&unique_id, &package_name, &model_name)?;
                        Ok(MinijinjaValue::from(unique_id))
                    }
                    Err(_) => Err(MinijinjaError::new(
                        MinijinjaErrorKind::NonKey,
                        format!(
                            "ref not found for package: {}, model: {}, version: {:?}",
                            self.package_name, model_name, version
                        ),
                    )),
                }
            }
            _ => Err(MinijinjaError::new(
                MinijinjaErrorKind::UnknownMethod,
                format!("No method named '{method}' on ref objects"),
            )),
        }
    }

    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        match key.as_str()? {
            "config" => Some(MinijinjaValue::from_dyn_object(self.runtime_config.clone())),
            "function_name" => Some(MinijinjaValue::from("ref")),
            _ => None,
        }
    }
}

/// Function for resolving source() calls in Jinja templates.
#[derive(Debug)]
pub struct SourceFunction {
    node_resolver: Arc<dyn NodeResolverTracker>,
    package_name: String,
    runtime_config: Arc<DbtRuntimeConfig>,
    microbatch_context: Option<MicrobatchRefContext>,
    validation_config: DependencyValidationConfig,
}

impl SourceFunction {
    /// Create a new Source with microbatch context for filtering sources.
    ///
    /// This is used during microbatch execution to filter sources to models
    /// with event_time configured to only include rows within the batch window.
    pub fn new_with_microbatch_context(
        node_resolver: Arc<dyn NodeResolverTracker>,
        package_name: String,
        runtime_config: Arc<DbtRuntimeConfig>,
        microbatch_context: MicrobatchRefContext,
    ) -> Self {
        Self {
            node_resolver,
            package_name,
            runtime_config,
            microbatch_context: Some(microbatch_context),
            validation_config: DependencyValidationConfig::default(),
        }
    }
}

impl SourceFunction {
    /// Construct a new `SourceFunction` from a `NodeResolver` and package name, with
    /// no ref validation
    pub fn new_unvalidated(
        node_resolver: Arc<dyn NodeResolverTracker>,
        package_name: String,
        runtime_config: Arc<DbtRuntimeConfig>,
    ) -> Self {
        Self {
            node_resolver,
            package_name,
            runtime_config,
            microbatch_context: None,
            validation_config: DependencyValidationConfig::default(),
        }
    }

    /// Construct a `SourceFunction` with dependency validation (used for unit tests).
    pub fn new_with_validation(
        node_resolver: Arc<dyn NodeResolverTracker>,
        package_name: String,
        runtime_config: Arc<DbtRuntimeConfig>,
        validation_config: DependencyValidationConfig,
    ) -> Self {
        Self {
            node_resolver,
            package_name,
            runtime_config,
            microbatch_context: None,
            validation_config,
        }
    }

    /// Validate that the referenced source is in the allowed dependencies.
    fn validate_dependency(
        &self,
        unique_id: &str,
        source_name: &str,
        table_name: &str,
    ) -> Result<(), MinijinjaError> {
        if self.validation_config.skip_validation
            || self.validation_config.node_type != NodeType::UnitTest
            || self
                .validation_config
                .allowed_dependencies
                .contains(unique_id)
        {
            return Ok(());
        }

        let unit_test_name = self
            .validation_config
            .current_node_unique_id
            .as_deref()
            .and_then(|uid| uid.rsplit('.').next())
            .unwrap_or("<unknown>");
        Err(MinijinjaError::new(
            MinijinjaErrorKind::InvalidOperation,
            format!(
                "Unit test '{unit_test_name}' references \
{{{{ source('{source_name}', '{table_name}') }}}}, but this dependency was not mocked.\n\n\
Add it to the unit test's `given` block:\n  \
- input: source('{source_name}', '{table_name}')\n    \
rows: [...]\n\n\
Or remove the source() from the model if it's unused."
            ),
        ))
    }
}

impl Object for SourceFunction {
    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        match key.as_str()? {
            "config" => Some(MinijinjaValue::from_dyn_object(self.runtime_config.clone())),
            "function_name" => Some(MinijinjaValue::from("source")),
            _ => None,
        }
    }

    fn call(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        args: &[MinijinjaValue],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        let mut parser = ArgParser::new(args, None);
        let num_args = parser.positional_len() + parser.kwargs_len();
        let (source_name, table_name) = match num_args {
            0 | 1 => Err(MinijinjaError::new(
                MinijinjaErrorKind::MissingArgument,
                "source macro requires 2 arguments: source name and table name",
            )),
            2 => {
                let source_name_value = parser.get::<MinijinjaValue>("source_name")?;
                let table_name_value = parser.get::<MinijinjaValue>("table_name")?;
                let source_name = source_name_value.as_str().ok_or_else(|| {
                    MinijinjaError::new(
                        MinijinjaErrorKind::InvalidOperation,
                        format!(
                            "The source name (first) argument to source() must be a string, got {}",
                            source_name_value.kind()
                        ),
                    )
                })?;
                let table_name = table_name_value.as_str().ok_or_else(|| {
                    MinijinjaError::new(
                        MinijinjaErrorKind::InvalidOperation,
                        format!(
                            "The table name (second) argument to source() must be a string, got {}",
                            table_name_value.kind()
                        ),
                    )
                })?;
                Ok((source_name.to_string(), table_name.to_string()))
            }
            _ => Err(MinijinjaError::new(
                MinijinjaErrorKind::TooManyArguments,
                "source",
            )),
        }?;
        match self
            .node_resolver
            .lookup_source(&self.package_name, &source_name, &table_name)
        {
            Ok((unique_id, relation, _)) => {
                for listener in listeners {
                    listener.on_ref_or_source_resolved(&unique_id);
                }

                self.validate_dependency(&unique_id, &source_name, &table_name)?;

                // Apply microbatch filtering if we have a microbatch context and
                // the referenced source has event_time configured
                if let Some(ref microbatch_ctx) = self.microbatch_context {
                    if let Some(event_time_col) = microbatch_ctx.get_event_time(&unique_id) {
                        // Extract the RelationObject and apply the filter
                        if let Some(relation_obj) = relation
                            .as_object()
                            .and_then(|obj| obj.downcast_ref::<RelationObject>())
                        {
                            let filtered = relation_obj.with_filter(
                                microbatch_ctx.to_run_filter(),
                                Some(event_time_col.to_string()),
                            );
                            return Ok(filtered.into_value());
                        }
                    }
                }

                Ok(relation)
            }
            Err(_) => Err(MinijinjaError::new(
                MinijinjaErrorKind::NonKey,
                format!(
                    "Source not found for source name: {source_name}, table name: {table_name}"
                ),
            )),
        }
    }
}

#[derive(Debug)]
pub struct FunctionFunction {
    node_resolver: Arc<dyn NodeResolverTracker>,
    package_name: String,
    runtime_config: Arc<DbtRuntimeConfig>,
    validation_config: DependencyValidationConfig,
}

impl FunctionFunction {
    /// Create a new FunctionFunction without validation (for base context)
    pub fn new_unvalidated(
        node_resolver: Arc<dyn NodeResolverTracker>,
        package_name: String,
        runtime_config: Arc<DbtRuntimeConfig>,
    ) -> Self {
        Self {
            node_resolver,
            package_name,
            runtime_config,
            validation_config: DependencyValidationConfig::default(),
        }
    }

    /// Create a new FunctionFunction with validation (for node context)
    pub fn new_with_validation(
        node_resolver: Arc<dyn NodeResolverTracker>,
        package_name: String,
        runtime_config: Arc<DbtRuntimeConfig>,
        validation_config: DependencyValidationConfig,
    ) -> Self {
        Self {
            node_resolver,
            package_name,
            runtime_config,
            validation_config,
        }
    }

    fn resolve_args(
        &self,
        args: &[MinijinjaValue],
    ) -> Result<(Option<String>, String), MinijinjaError> {
        if args.is_empty() || args.len() > 3 {
            return Err(MinijinjaError::new(
                MinijinjaErrorKind::InvalidOperation,
                "invalid number of arguments for function macro",
            ));
        }
        let mut parser = ArgParser::new(args, None);
        // If there are two positional args, the first is the package name and the second is the function name
        let arg0 = parser.get::<String>("")?;
        let arg1 = parser.get_optional::<String>("");
        let (namespace, function_name) = match (arg0, arg1) {
            (namespace, Some(function_name)) => (Some(namespace), function_name),
            (function_name, None) => (None, function_name),
        };

        let package_name = namespace;

        Ok((package_name, function_name))
    }

    /// Validate that the referenced function is in the allowed dependencies
    fn validate_dependency(
        &self,
        unique_id: &str,
        package_name: &Option<String>,
        function_name: &str,
    ) -> Result<(), MinijinjaError> {
        if self.validation_config.skip_validation {
            return Ok(());
        }

        if self
            .validation_config
            .allowed_dependencies
            .contains(unique_id)
        {
            Ok(())
        } else {
            // Construct the function string for the error message
            let function_string = if let Some(pkg) = package_name {
                format!("{{{{ function('{pkg}', '{function_name}') }}}}")
            } else {
                format!("{{{{ function('{function_name}') }}}}")
            };

            Err(MinijinjaError::new(
                MinijinjaErrorKind::InvalidOperation,
                format!(
                    "dbt was unable to infer all dependencies for the function \"{function_name}\". This typically happens when function() is placed within a conditional block.
To fix this, add the following hint to the top of the model:
-- depends_on: {function_string}"
                ),
            ))
        }
    }
}

impl Object for FunctionFunction {
    fn call(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        args: &[MinijinjaValue],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        let (package_name, function_name) = self.resolve_args(args)?;

        match self.node_resolver.lookup_function(
            &package_name,
            &function_name,
            &Some(self.package_name.clone()),
        ) {
            Ok((unique_id, function_call, _)) => {
                // Validate that this function is allowed (only if validation is configured)
                self.validate_dependency(&unique_id, &package_name, &function_name)?;
                Ok(function_call)
            }
            Err(_) => Err(MinijinjaError::new(
                MinijinjaErrorKind::NonKey,
                format!(
                    "function not found for package: {}, function: {}",
                    self.package_name, function_name
                ),
            )),
        }
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        method: &str,
        args: &[MinijinjaValue],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        match method {
            "id" => {
                let (package_name, function_name) = self.resolve_args(args)?;
                match self.node_resolver.lookup_function(
                    &package_name,
                    &function_name,
                    &Some(self.package_name.clone()),
                ) {
                    Ok((unique_id, _relation, _)) => {
                        // Validate that this function is allowed (only if validation is configured)
                        self.validate_dependency(&unique_id, &package_name, &function_name)?;
                        Ok(MinijinjaValue::from(unique_id.as_str()))
                    }
                    Err(_) => Err(MinijinjaError::new(
                        MinijinjaErrorKind::NonKey,
                        format!(
                            "function not found for package: {}, function: {}",
                            self.package_name, function_name
                        ),
                    )),
                }
            }
            _ => Err(MinijinjaError::new(
                MinijinjaErrorKind::UnknownMethod,
                format!("No method named '{method}' on function objects"),
            )),
        }
    }

    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        match key.as_str()? {
            "config" => Some(MinijinjaValue::from_dyn_object(self.runtime_config.clone())),
            "function_name" => Some(MinijinjaValue::from("function")),
            _ => None,
        }
    }
}

// `MacroLookupContext` moved to `dbt_jinja_ctx::objects::lookup`. Re-exported
// here so existing call sites in this crate (`build_operation_context_btreemap`,
// the parse-phase resolve-model context, etc.) keep importing from the path
// they always have. The transitional re-export is removed once every call
// site has been migrated to consume `dbt-jinja-ctx` directly.
pub use dbt_jinja_ctx::MacroLookupContext;

/// This is a lazy-loaded flat graph object that builds the flat graph from
/// `nodes` on first access. `defer_nodes`, when present, is used to populate
/// each deferrable node's `defer_relation` key (#1366).
#[derive(Debug)]
struct LazyFlatGraph {
    nodes: Nodes,
    defer_nodes: Option<Nodes>,
    graph: OnceLock<MinijinjaValue>,
}

impl LazyFlatGraph {
    pub fn new(nodes: &Nodes, defer_nodes: Option<&Nodes>) -> Self {
        // TODO: We don't want to clone the top level maps either -- make the
        // caller pass in Arc<Nodes> instead
        Self {
            nodes: nodes.clone(),
            defer_nodes: defer_nodes.cloned(),
            graph: OnceLock::new(),
        }
    }

    fn get_graph(&self) -> &MinijinjaValue {
        self.graph.get_or_init(|| {
            // Seed the flat graph into the invocation-wide `graph` mapping
            // rather than into a private map, so scratch state written by
            // macros during parsing (and by other base contexts) survives.
            // `update` merges, so only the derived flat-graph keys (`nodes`,
            // `sources`, `macros`, …) are overwritten. dbt-labs/fs#13454.
            let shared = invocation_graph();
            let flat = build_flat_graph(&self.nodes, self.defer_nodes.as_ref());
            shared.update(&ValueMap::from(flat));
            MinijinjaValue::from_dyn_object(shared)
        })
    }
}

impl Object for LazyFlatGraph {
    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        self.get_graph().as_object().unwrap().get_value(key)
    }

    fn repr(self: &Arc<Self>) -> minijinja::value::ObjectRepr {
        self.get_graph().as_object().unwrap().repr()
    }

    fn enumerate(self: &Arc<Self>) -> minijinja::value::Enumerator {
        self.get_graph().as_object().unwrap().enumerate()
    }

    fn enumerator_len(self: &Arc<Self>) -> Option<usize> {
        self.get_graph().as_object().unwrap().enumerator_len()
    }

    fn is_true(self: &Arc<Self>) -> bool {
        self.get_graph().as_object().unwrap().is_true()
    }

    fn is_mutable(self: &Arc<Self>) -> bool {
        self.get_graph().as_object().unwrap().is_mutable()
    }

    fn call(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        args: &[MinijinjaValue],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        self.get_graph()
            .as_object()
            .unwrap()
            .call(state, args, listeners)
    }

    fn call_method(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        method: &str,
        args: &[MinijinjaValue],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        self.get_graph()
            .as_object()
            .unwrap()
            .call_method(state, method, args, listeners)
    }

    fn render(self: &Arc<Self>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        Self: Sized + 'static,
    {
        self.get_graph().as_object().unwrap().render(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::state::DummyNodeResolverTracker;
    use dbt_test_utils::TestEnvGuard;
    use std::env;

    #[test]
    fn test_dbt_metadata_envs_populated_from_env() {
        // Isolate env to avoid interference across tests
        let _guard = TestEnvGuard::new(&[], &["DBT_ENV_CUSTOM_ENV_"]);

        // Arrange: set a custom env var following dbt-core convention
        const ENV_KEY: &str = "DBT_ENV_CUSTOM_ENV_FOO";
        // Ensure clean state
        unsafe {
            #[allow(clippy::disallowed_methods)]
            env::remove_var(ENV_KEY);
            #[allow(clippy::disallowed_methods)]
            env::set_var(ENV_KEY, "bar");
        }

        let node_resolver = Arc::new(DummyNodeResolverTracker);
        let nodes = Nodes::default();
        let runtime_config = Arc::new(DbtRuntimeConfig::default());

        // Act
        let ctx = build_compile_base_ctx(
            node_resolver,
            "test_pkg",
            &nodes,
            None,
            runtime_config,
            vec![],
            None,
        );

        // Cleanup env to avoid side effects
        unsafe {
            #[allow(clippy::disallowed_methods)]
            env::remove_var(ENV_KEY);
        }

        // Assert
        let meta = ctx.dbt_metadata_envs;
        let map = meta
            .as_object()
            .and_then(|o| o.downcast_ref::<BTreeMap<String, MinijinjaValue>>())
            .expect("dbt_metadata_envs should be a map");

        assert_eq!(
            map.get("FOO").and_then(|v| v.as_str()),
            Some("bar"),
            "Expected dbt_metadata_envs['FOO']='bar'"
        );
    }

    #[test]
    fn build_operation_context_includes_config_as_dummy_config() {
        let node_resolver = Arc::new(DummyNodeResolverTracker);
        let nodes = Nodes::default();
        let runtime_config = Arc::new(DbtRuntimeConfig::default());

        let ctx = build_operation_context(
            node_resolver,
            "test_pkg",
            &nodes,
            None,
            runtime_config,
            vec![],
            None,
        );

        let registered = to_jinja_btreemap(&ctx);
        let config = registered
            .get("config")
            .expect("config must be present in OperationCtx");
        let downcast = config
            .as_object()
            .and_then(|obj| obj.downcast::<DummyConfig>());
        assert!(
            downcast.is_some(),
            "OperationCtx.config must be a DummyConfig Object"
        );
    }
}
