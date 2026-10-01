//! This module contains the scope guard for resolving models.

use chrono::TimeZone;
use chrono_tz::{Europe::London, Tz};
use dbt_adapter::{AdapterType, load_store::ResultStore};
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::serde_utils::convert_yml_to_dash_map;
use dbt_common::{FsResult, dashmap::DashMap, serde_utils::convert_yml_to_value_map};
use dbt_schemas::schemas::InternalDbtNode;
use dbt_schemas::{
    schemas::{InternalDbtNodeAttributes, common::DbtMaterialization, telemetry::NodeType},
    state::{DbtRuntimeConfig, NodeResolverTracker, ResolverState},
};
use minijinja::constants::{CURRENT_EXECUTION_PHASE, CURRENT_PATH, CURRENT_SPAN, DIALECT};
use minijinja::{Value as MinijinjaValue, machinery::Span};
use minijinja_contrib::modules::{py_datetime::datetime::PyDateTime, pytz::PytzTimezone};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use dbt_jinja_ctx::{
    CompileNodeCtx, JinjaObject, MacroLookupContext, to_jinja_btreemap, to_model_context_map,
};

use crate::phases::compile_and_run_context::{FunctionFunction, SourceFunction};
use dbt_schemas::schemas::project::ConfigKeys;

use super::super::compile_and_run_context::RefFunction;
use super::compile_config::CompileConfig;

/// The name of the repl model
pub const REPL_MODEL_NAME: &str = "__repl__";

/// Resolve the configured `model-paths` (resource roots) for a node's package,
/// falling back to the dbt default (`models`) when unset. Dependency packages
/// carry their own runtime config, so look those up separately from the root
/// project. Used to derive the resource-relative `model.path` Jinja value.
fn model_paths_for_package(runtime_config: &DbtRuntimeConfig, package_name: &str) -> Vec<String> {
    let paths = if runtime_config.inner.project_name == package_name {
        runtime_config.inner.model_paths.clone()
    } else {
        runtime_config
            .dependencies
            .get(package_name)
            .map(|cfg| cfg.inner.model_paths.clone())
            .unwrap_or_default()
    };
    if paths.is_empty() {
        vec!["models".to_string()]
    } else {
        paths
    }
}

/// Configure ref validation behavior
#[derive(Debug, Clone)]
pub struct DependencyValidationConfig {
    /// Expressed as `unique_id`s
    pub allowed_dependencies: Arc<BTreeSet<String>>,
    /// Whether to skip validation
    pub skip_validation: bool,
    /// What kind of node is being validated?
    pub node_type: NodeType,
    /// `unique_id` of the node whose dependencies are being validated, if any
    pub current_node_unique_id: Option<String>,
}

impl DependencyValidationConfig {
    /// Make a new config struct for a given node
    pub fn new_for_node(node: &impl InternalDbtNode) -> DependencyValidationConfig {
        DependencyValidationConfig {
            node_type: node.resource_type(),
            current_node_unique_id: Some(node.common().unique_id.clone()),
            ..Self::default()
        }
    }

    /// Make an unvalidated config for an unspecified node type
    pub fn new_unvalidated() -> DependencyValidationConfig {
        Self::default()
    }

    /// Make a validated config for an unspecified node type
    pub fn new_validated() -> DependencyValidationConfig {
        DependencyValidationConfig {
            skip_validation: false,
            ..Self::default()
        }
    }

    /// Allow these `unique_id`s in the dependency allowlist. Additive.
    pub fn allow_dependencies(
        mut self,
        addl_deps: impl IntoIterator<Item = impl Into<String>>,
    ) -> DependencyValidationConfig {
        let mut deps = Arc::unwrap_or_clone(self.allowed_dependencies);
        deps.extend(addl_deps.into_iter().map(|s| s.into()));
        self.allowed_dependencies = Arc::new(deps);
        self
    }

    /// Disable validation
    pub fn skip_validation(mut self) -> DependencyValidationConfig {
        self.skip_validation = true;
        self
    }

    /// Enable validation
    pub fn validate(mut self) -> DependencyValidationConfig {
        self.skip_validation = false;
        self
    }
}

impl Default for DependencyValidationConfig {
    fn default() -> Self {
        DependencyValidationConfig {
            allowed_dependencies: Arc::new(BTreeSet::new()),
            skip_validation: true,
            node_type: NodeType::Unspecified,
            current_node_unique_id: None,
        }
    }
}

/// Build a compile model context (wrapper for build_compile_node_context_inner)
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn build_compile_node_context<T>(
    model: &T,
    resolver_state: &ResolverState,
    base_context: &BTreeMap<String, MinijinjaValue>,
    ref_validation_config: DependencyValidationConfig,
) -> FsResult<(
    BTreeMap<String, MinijinjaValue>,
    Arc<DashMap<String, MinijinjaValue>>,
)>
where
    T: InternalDbtNodeAttributes + ?Sized,
{
    build_compile_node_context_inner(
        model,
        resolver_state.adapter_type,
        base_context,
        &resolver_state.root_project_name,
        resolver_state.node_resolver.clone(),
        resolver_state.runtime_config.clone(),
        ref_validation_config,
    )
}

/// Build a compile model context
/// Returns a context and the current relation
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn build_compile_node_context_inner<T>(
    model: &T,
    adapter_type: AdapterType,
    base_context: &BTreeMap<String, MinijinjaValue>,
    root_project_name: &str,
    node_resolver: Arc<dyn NodeResolverTracker>,
    runtime_config: Arc<DbtRuntimeConfig>,
    ref_validation_config: DependencyValidationConfig,
) -> FsResult<(
    BTreeMap<String, MinijinjaValue>,
    Arc<DashMap<String, MinijinjaValue>>,
)>
where
    T: InternalDbtNodeAttributes + ?Sized,
{
    let mut base_builtins = if let Some(builtins) = base_context.get("builtins") {
        builtins
            .as_object()
            .unwrap()
            .downcast_ref::<BTreeMap<String, MinijinjaValue>>()
            .unwrap()
            .clone()
    } else {
        BTreeMap::new()
    };
    let mut ctx = base_context.clone();

    let this_relation = match model.resource_type() {
        NodeType::UnitTest => {
            let ref_name = model
                .base()
                .refs
                .first()
                .cloned()
                .map(|r| r.name)
                .expect("Unit test must have a dependency");
            let (_, this_relation, _, _) = node_resolver.lookup_ref(
                &Some(model.common().package_name.clone()),
                &ref_name,
                &None,
                &None,
            )?;
            this_relation
        }
        NodeType::Model => {
            let ref_name = model.common().name.clone();
            // for repl, we use the just create a relation on spot using model passed in.
            if ref_name == REPL_MODEL_NAME {
                dbt_adapter::relation::RelationObject::new(Arc::from(
                    dbt_adapter::relation::do_create_relation(
                        adapter_type,
                        model.base().database.clone(),
                        model.base().schema.clone(),
                        Some(model.base().alias.clone()),
                        None,
                        model.base().quoting,
                    )
                    .unwrap(),
                ))
                .into_value()
            } else if matches!(model.materialized(), DbtMaterialization::Ephemeral) {
                // Build `this` directly as the physical relation
                // (database.schema.identifier) rather than using the relation the
                // resolver stores for this node. The stored relation is typed so
                // that `ref()` to this node inlines it as a `__dbt__cte__<name>`
                // CTE; reusing it here would make bare `{{ this }}` render as that
                // same self-referential CTE. Constructing a fresh relation with no
                // type keeps `ref()` untouched and mirrors how the run phase builds
                // `this`.
                dbt_adapter::relation::RelationObject::new(Arc::from(
                    dbt_adapter::relation::do_create_relation(
                        adapter_type,
                        model.base().database.clone(),
                        model.base().schema.clone(),
                        Some(model.base().alias.clone()),
                        None,
                        model.base().quoting,
                    )
                    .unwrap(),
                ))
                .into_value()
            } else {
                let version = model.version().map(|v| v.to_string());
                // Bind `this` by identity. Sharing a name with another node
                // (e.g. a legacy snapshot) makes `ref()` ambiguous, but must
                // not stop a model from resolving its own relation.
                let (this_relation, deferred_relation) = node_resolver.lookup_self_relation(
                    &model.common().unique_id,
                    &model.common().package_name,
                    &ref_name,
                    &version,
                )?;

                if let Some(deferred_relation_value) = deferred_relation
                    && (matches!(*model.base().static_analysis, StaticAnalysisKind::Unsafe)
                        || model.introspection().is_unsafe())
                {
                    deferred_relation_value
                } else {
                    this_relation
                }
            }
        }
        _ => dbt_adapter::relation::RelationObject::new(Arc::from(
            dbt_adapter::relation::do_create_relation(
                adapter_type,
                model.base().database.clone(),
                model.base().schema.clone(),
                Some(model.base().alias.clone()),
                None,
                model.base().quoting,
            )
            .unwrap(),
        ))
        .into_value(),
    };
    let config_map = Arc::new(convert_yml_to_dash_map(model.serialized_config()));

    // Get valid config keys based on resource type
    let valid_keys = match model.resource_type() {
        NodeType::Model => dbt_schemas::schemas::project::ModelConfig::valid_field_names(),
        NodeType::Seed => dbt_schemas::schemas::project::SeedConfig::valid_field_names(),
        NodeType::Test => dbt_schemas::schemas::project::DataTestConfig::valid_field_names(),
        NodeType::Snapshot => dbt_schemas::schemas::project::SnapshotConfig::valid_field_names(),
        NodeType::Source => dbt_schemas::schemas::project::SourceConfig::valid_field_names(),
        NodeType::UnitTest => dbt_schemas::schemas::project::UnitTestConfig::valid_field_names(),
        NodeType::Function => dbt_schemas::schemas::project::FunctionConfig::valid_field_names(),
        _ => {
            // For other types, use an empty set to avoid warnings
            std::collections::HashSet::new()
        }
    };

    let compile_config = CompileConfig {
        config: config_map.clone(),
        valid_keys,
    };
    let config_value = MinijinjaValue::from_object(compile_config.clone());
    base_builtins.insert(
        "config".to_string(),
        MinijinjaValue::from_object(compile_config),
    );

    // Add model depends_on to dependency allowlist
    let validation_config_with_depends_on =
        ref_validation_config.allow_dependencies(&model.base().depends_on.nodes);

    let ref_function = RefFunction::new_with_validation(
        node_resolver.clone(),
        model.common().package_name.clone(),
        runtime_config.clone(),
        validation_config_with_depends_on.clone(),
        model.common().unique_id.clone(),
    );
    let ref_value = MinijinjaValue::from_object(ref_function);
    base_builtins.insert("ref".to_string(), ref_value.clone());

    // Create validated function function with dependency checking
    let function_function = FunctionFunction::new_with_validation(
        node_resolver.clone(),
        model.common().package_name.clone(),
        runtime_config.clone(),
        validation_config_with_depends_on.clone(),
    );
    let function_value = MinijinjaValue::from_object(function_function);
    base_builtins.insert("function".to_string(), function_value.clone());

    // Recreate source function with the node's package_name (not root project's)
    let source_function = SourceFunction::new_with_validation(
        node_resolver.clone(),
        model.common().package_name.clone(),
        runtime_config.clone(),
        validation_config_with_depends_on,
    );
    let source_value = MinijinjaValue::from_object(source_function);
    base_builtins.insert("source".to_string(), source_value.clone());

    let mut model_map = convert_yml_to_value_map(model.serialize());
    // dbt-core exposes `model.path` relative to the resource root (e.g.
    // `staging/orders.sql`), not package-relative (`models/staging/orders.sql`).
    // Fusion stores the package-relative form on the node (it is used for file
    // I/O and parse caching), so derive the resource-relative form here for the
    // Jinja `model.path` value to match dbt-core.
    if model.resource_type() == NodeType::Model {
        let model_paths = model_paths_for_package(&runtime_config, &model.common().package_name);
        let jinja_path = dbt_common::path::strip_resource_paths(&model.common().path, &model_paths);
        model_map.insert(
            "path".to_owned(),
            MinijinjaValue::from(jinja_path.to_string_lossy().into_owned()),
        );
    }
    model_map.insert(
        "batch".to_owned(),
        MinijinjaValue::from_object(init_batch_context()),
    );

    let result_store = ResultStore::default();

    let mut packages = runtime_config
        .dependencies
        .keys()
        .cloned()
        .collect::<BTreeSet<String>>();
    packages.insert(root_project_name.to_string());

    // Build the typed per-node overlay. Object-typed slots are wrapped via
    // `MinijinjaValue::from_object(...)` HERE rather than typed as concrete
    // types in `CompileNodeCtx`, because going through serde can change the
    // underlying Object's concrete type. `builtins` must keep its BTreeMap
    // shape for downstream downcasts, while `model` is intentionally mutable
    // to match dbt Core's dict behavior.
    let overlay = CompileNodeCtx {
        base: None,
        this: this_relation,
        database: model.base().database.to_string(),
        schema: model.base().schema.to_string(),
        identifier: model.base().alias.clone(),
        config: config_value,
        ref_fn: ref_value,
        source: source_value,
        function: function_value,
        builtins: MinijinjaValue::from_object(base_builtins),
        model: MinijinjaValue::from_dyn_object(to_model_context_map(model_map)),
        store_result: MinijinjaValue::from_function(result_store.store_result()),
        load_result: MinijinjaValue::from_function(result_store.load_result()),
        store_raw_result: MinijinjaValue::from_function(result_store.store_raw_result()),
        target_package_name: model.common().package_name.clone(),
        target_unique_id: model.common().unique_id.clone(),
        context: JinjaObject::new(MacroLookupContext::new(
            root_project_name.to_string(),
            None,
            packages,
        )),
        current_path: model
            .common()
            .original_file_path
            .clone()
            .to_string_lossy()
            .into_owned(),
        current_span: MinijinjaValue::from_serialize(Span::default()),
        current_execution_phase: "render".to_string(),
    };

    ctx.insert(
        CURRENT_PATH.to_string(),
        MinijinjaValue::from(model.common().original_file_path.clone().to_string_lossy()),
    );

    ctx.insert(
        CURRENT_SPAN.to_string(),
        MinijinjaValue::from_serialize(Span::default()),
    );

    ctx.insert(
        CURRENT_EXECUTION_PHASE.to_string(),
        MinijinjaValue::from("render"),
    );

    // A node that selected an adapter via `+adapter` renders against that
    // adapter's dialect. Inserted into the node's own context so macro dispatch
    // resolves per node. Always present now that the adapter is resolved at parse:
    // an unannotated node carries the target default rather than nothing.
    ctx.insert(
        DIALECT.to_string(),
        // `as_ref()` deliberately, not `to_string()`: the namespace is keyed
        // with `as_ref()`, and for AdapterType those are separate strum impls.
        MinijinjaValue::from(model.node_adapter().as_ref()),
    );

    // Today's caller still consumes `BTreeMap<String, MinijinjaValue>`. We
    // serialize the typed overlay and `.extend(...)` onto the base — same
    // last-write-wins shadowing semantic the original BTreeMap-based code
    // produced. PR 9 (cleanup) flows the typed struct directly through
    // `render_named_str<S: Serialize>` and drops the conversion.
    ctx.extend(to_jinja_btreemap(&overlay));
    Ok((ctx, config_map))
}

fn init_batch_context() -> BTreeMap<String, MinijinjaValue> {
    // TODO: batch map should have valid event_time_start and event_time_end
    // for now, we are just using now
    let datetime = London.with_ymd_and_hms(2025, 1, 1, 1, 1, 1).unwrap();
    let mut batch_map = BTreeMap::new();
    batch_map.insert("id".to_string(), MinijinjaValue::from(""));
    batch_map.insert(
        "event_time_start".to_string(),
        MinijinjaValue::from_object(PyDateTime::new_aware(
            datetime,
            Some(PytzTimezone::new(Tz::UTC)),
        )),
    );
    batch_map.insert(
        "event_time_end".to_string(),
        MinijinjaValue::from_object(PyDateTime::new_aware(
            datetime,
            Some(PytzTimezone::new(Tz::UTC)),
        )),
    );
    batch_map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_resolver::NodeResolver;
    use dbt_common::ErrorCode;
    use dbt_schemas::schemas::nodes::{
        CommonAttributes, DbtModel, DbtSnapshot, NodeBaseAttributes,
    };
    use dbt_schemas::state::ModelStatus;

    const ADAPTER_TYPE: AdapterType = AdapterType::Snowflake;

    fn common(unique_id: &str) -> CommonAttributes {
        CommonAttributes {
            unique_id: unique_id.to_string(),
            name: "barbers".to_string(),
            package_name: "my_new_project".to_string(),
            ..Default::default()
        }
    }

    fn colliding_model() -> DbtModel {
        DbtModel {
            __common_attr__: common("model.my_new_project.barbers"),
            __base_attr__: NodeBaseAttributes {
                alias: "barbers".to_string(),
                materialized: DbtMaterialization::View,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// A model and a legacy snapshot sharing a name, as dbt-core accepts.
    fn insert_colliding_snapshot(node_resolver: &mut NodeResolver) {
        let snapshot = DbtSnapshot {
            __common_attr__: common("snapshot.my_new_project.barbers"),
            __base_attr__: NodeBaseAttributes {
                alias: "barbers_snapshot".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };

        node_resolver
            .insert_ref(&snapshot, ADAPTER_TYPE, ModelStatus::Enabled, false)
            .expect("insert snapshot ref");
    }

    fn resolver_with_collision(model: &DbtModel) -> NodeResolver {
        let mut node_resolver = NodeResolver::default();
        node_resolver
            .insert_ref(model, ADAPTER_TYPE, ModelStatus::Enabled, false)
            .expect("insert model ref");
        insert_colliding_snapshot(&mut node_resolver);
        node_resolver
    }

    fn build_ctx(
        model: &DbtModel,
        node_resolver: NodeResolver,
    ) -> FsResult<BTreeMap<String, MinijinjaValue>> {
        build_compile_node_context_inner(
            model,
            ADAPTER_TYPE,
            &BTreeMap::new(),
            "my_new_project",
            Arc::new(node_resolver),
            Arc::new(DbtRuntimeConfig::default()),
            DependencyValidationConfig::new_unvalidated(),
        )
        .map(|(ctx, _)| ctx)
    }

    // A name shared with a snapshot must not stop a model from binding its own
    // `this` — dbt-core builds such projects (dbt-labs/fs#12879).
    #[test]
    fn colliding_node_name_still_resolves_own_this() {
        let model = colliding_model();
        let ctx = build_ctx(&model, resolver_with_collision(&model))
            .expect("model must resolve its own `this` despite the name collision");

        let this = ctx.get("this").expect("`this` must be bound").to_string();
        assert!(
            this.contains("barbers") && !this.contains("barbers_snapshot"),
            "`this` bound to the wrong node's relation: {this}"
        );
    }

    // `merge` extends the ref vectors in place, so identity resolution reads
    // merged records without any second structure to keep in sync.
    #[test]
    fn merged_resolver_still_resolves_own_this() {
        let model = colliding_model();
        let mut node_resolver = NodeResolver::default();
        node_resolver.merge(resolver_with_collision(&model));

        let ctx = build_ctx(&model, node_resolver)
            .expect("a merged resolver must still resolve the model's own `this`");

        let this = ctx.get("this").expect("`this` must be bound").to_string();
        assert!(
            this.contains("barbers") && !this.contains("barbers_snapshot"),
            "`this` bound to the wrong node's relation: {this}"
        );
    }

    // Identity resolution must not smuggle a disabled node past the
    // enabled/disabled distinction `lookup_ref` enforces.
    #[test]
    fn disabled_self_node_is_reported_not_resolved() {
        let model = colliding_model();
        let mut node_resolver = NodeResolver::default();
        node_resolver
            .insert_ref(&model, ADAPTER_TYPE, ModelStatus::Disabled, false)
            .expect("insert disabled model ref");

        let err = build_ctx(&model, node_resolver).expect_err("a disabled node must not resolve");

        assert_eq!(err.code, ErrorCode::DisabledDependency);
    }

    #[test]
    fn disabled_self_node_with_colliding_snapshot_is_reported() {
        let model = colliding_model();
        let mut node_resolver = NodeResolver::default();
        node_resolver
            .insert_ref(&model, ADAPTER_TYPE, ModelStatus::Disabled, false)
            .expect("insert disabled model ref");
        insert_colliding_snapshot(&mut node_resolver);

        let err = build_ctx(&model, node_resolver)
            .expect_err("a colliding snapshot must not replace a disabled model's `this`");

        assert_eq!(err.code, ErrorCode::DisabledDependency);
        assert!(
            err.to_string()
                .contains("Attempted to use disabled ref 'barbers'"),
            "unexpected error: {err}"
        );
    }

    // The collision is still a genuine ambiguity for user-written `ref()`,
    // which has no unique_id to disambiguate with.
    #[test]
    fn colliding_node_name_still_makes_user_ref_ambiguous() {
        let model = colliding_model();
        let err = resolver_with_collision(&model)
            .lookup_ref(
                &Some("my_new_project".to_string()),
                "barbers",
                &None,
                &Some("my_new_project".to_string()),
            )
            .expect_err("ref('barbers') must stay ambiguous");

        assert_eq!(err.code, ErrorCode::InvalidConfig);
        assert!(
            err.to_string().contains("Found ambiguous ref('barbers')"),
            "unexpected error: {err}"
        );
    }

    // An unresolvable `this` must be reported, not `.expect()`ed into a panic.
    #[test]
    fn unresolvable_this_is_reported_not_panicked() {
        let model = colliding_model();
        let err = build_ctx(&model, NodeResolver::default())
            .expect_err("an unregistered model must not resolve");

        assert_eq!(err.code, ErrorCode::DependencyNotFound);
    }

    #[test]
    fn missing_self_node_with_colliding_snapshot_is_reported() {
        let model = colliding_model();
        let mut node_resolver = NodeResolver::default();
        insert_colliding_snapshot(&mut node_resolver);

        let err = build_ctx(&model, node_resolver)
            .expect_err("a colliding snapshot must not replace a missing model's `this`");

        assert_eq!(err.code, ErrorCode::DependencyNotFound);
        assert!(
            err.to_string()
                .contains("Ref 'barbers' not found in project"),
            "unexpected error: {err}"
        );
    }
}
