use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use dbt_adapter::Adapter;
use dbt_adapter::cache::hydrate_relation_cache_if_not_already_cached;
use dbt_adapter::errors::into_fs_error;
use dbt_adapter::metadata::CatalogAndSchema;
use dbt_adapter::relation::{create_relation, create_relation_from_node};
use dbt_adapter_core::AdapterType;
use dbt_common::create_info_span;
use dbt_common::io_args::EvalArgs;
use dbt_common::io_args::FsCommand;
use dbt_common::node_selector::SelectionCriteria;
use dbt_common::node_selector::{MethodName, selectors_require_manifest};
use dbt_common::static_analysis::is_strict_static_analysis;
use dbt_common::tracing::dbt_emit::{emit_trace_log_message, emit_warn_log_message};
use dbt_common::tracing::span_info::SpanStatusRecorder as _;
use dbt_common::{ErrorCode, FsResult};
use dbt_dag::schedule::Schedule;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_scheduler::node_selector::filter_select_criteria;
use dbt_schema_store::CanonicalFqn;
use dbt_schemas::schemas::IntrospectionKind;
use dbt_schemas::schemas::Nodes;
use dbt_schemas::schemas::OnManifestLoadFailure;
use dbt_schemas::schemas::StateArtifacts;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_schemas::schemas::semantic_layer::semantic_manifest::SemanticManifest;
use dbt_schemas::schemas::{InternalDbtNode, InternalDbtNodeAttributes};
use dbt_schemas::state::{NodeResolverTracker, ResolverState};
use dbt_state::run_cache_defer::RunCacheProfileResolver;
use dbt_tasks_core::run_cache_lifecycle::RunCacheLifecycle;
use dbt_telemetry::{ExecutionPhase, NodeType, PhaseExecuted};

use tracing::Instrument as _;

/// The result of a deferral pass, containing the mapping from deferred relations
/// back to their node unique IDs and the relation rewrites needed for recorded
/// introspection calls.
#[derive(Default)]
pub struct DeferralUpdate {
    /// Maps the **deferred (production) relation's** canonical FQN to the node's
    /// unique ID (e.g. `"model.my_project.my_model"`).
    pub deferred_unique_ids: HashMap<CanonicalFqn, String>,
    /// Maps the **old local relation's** canonical FQN to the **new deferred
    /// (production) relation**, so that recorded `get_relation` /
    /// `get_columns_in_relation` calls can be rewritten to point at the
    /// production relation instead.
    pub relation_remap: HashMap<CanonicalFqn, Arc<dyn BaseRelation>>,
}

#[derive(Default)]
pub struct DeferState {
    pub state_artifacts: Option<Arc<StateArtifacts>>,
    pub defer_nodes: Option<Nodes>,
    pub deferred_unique_ids: HashMap<CanonicalFqn, String>,
}

impl DeferState {
    /// Load defer state from disk, cloud, or reuse a previously loaded state.
    /// Returns the previous state (if available) and the filtered defer nodes
    /// (ephemeral/inline models excluded from deferral consideration).
    // Deferral is done in two phases:
    // Phase 1 (defer_common): Frontier nodes + selected incrementals/snapshots - SA-independent
    // Phase 2 (defer_sa_upstreams): SA-dependent upstream deferral based on SA strictness
    pub async fn load(
        arg: &EvalArgs,
        adapter: Arc<Adapter>,
        schedule: &Schedule<String>,
        resolved_state: &mut ResolverState,
        jinja_env: &JinjaEnv,
        previous_state: Option<Arc<StateArtifacts>>,
        root_project_quoting: ResolvedQuoting,
        create_run_cache: impl AsyncFn() -> FsResult<Arc<RunCacheLifecycle>>,
    ) -> FsResult<Self> {
        let on_failure = if selectors_require_manifest(arg.select.as_ref(), arg.exclude.as_ref()) {
            OnManifestLoadFailure::Warn
        } else {
            OnManifestLoadFailure::Ignore
        };
        let reuse_or_load = |path: &Path,
                             reuse_candidate: Option<Arc<StateArtifacts>>|
         -> FsResult<Option<Arc<StateArtifacts>>> {
            if let Some(prev_state) = reuse_candidate
                && prev_state.state_path.as_path() == path
            {
                return Ok(Some(prev_state));
            }

            StateArtifacts::try_new_with_target_path(
                path,
                root_project_quoting,
                Some(arg.io.out_dir.clone()),
                on_failure,
            )
            .map(|state| Some(Arc::new(state)))
        };

        // Priority 1: explicit --defer-state path
        let state = if let Some(path) = arg.defer_state.as_ref() {
            reuse_or_load(path.as_path(), previous_state)?
        } else {
            // Priority 2: reuse already loaded previous state when available
            previous_state
        };

        // Extract and filter nodes: exclude ephemeral/inline models from deferral consideration
        let mut nodes = state.as_ref().and_then(|s| s.nodes.clone()).map(|mut n| {
            n.models.retain(|_, model| {
                model.__base_attr__.materialized != DbtMaterialization::Ephemeral
                    && model.__base_attr__.materialized != DbtMaterialization::Inline
            });
            n
        });

        // Sources, in priority order: manifest (--defer) wins over dbt State
        // auto-deferral, which synthesizes profile-target defer nodes from the
        // dbt State service config when no manifest-backed state is available.
        // Auto-deferral is opt-in: synthesis failures degrade to "no defer"
        // rather than aborting compilation.
        if nodes.is_none() {
            let run_cache_client = create_run_cache().await?.service_client();

            let unselected_node_ids =
                Self::collect_auto_defer_unselected_node_ids(&resolved_state.nodes, schedule);

            let run_cache_defer_nodes = match RunCacheProfileResolver::synthesize_defer_nodes(
                arg,
                resolved_state,
                jinja_env,
                &unselected_node_ids,
                run_cache_client.clone(),
            )
            .await
            {
                Ok(Some(nodes)) => {
                    let profile_target = resolved_state.dbt_profile.target.clone();
                    emit_trace_log_message(|| {
                        format!(
                            "dbt State auto-deferral synthesized profile-target defer nodes for '{profile_target}'"
                        )
                    });
                    Some(nodes)
                }
                Ok(None) => None,
                Err(err) => {
                    emit_warn_log_message(
                        ErrorCode::StateServiceWarn,
                        format!(
                            "dbt State auto-deferral setup failed: {err}; continuing without synthesized defer state"
                        ),
                    );
                    None
                }
            };
            nodes = run_cache_defer_nodes.or(nodes);
        }

        let deferred: HashMap<CanonicalFqn, String> = if let Some(ref mut nodes) = nodes {
            let deferred = defer_common(arg, resolved_state, nodes, schedule, &adapter).await?;
            rewrite_recorded_relation_calls_with_deferral(resolved_state, &deferred.relation_remap);
            deferred.deferred_unique_ids
        } else {
            HashMap::new()
        };

        Ok(DeferState {
            state_artifacts: state,
            defer_nodes: nodes,
            deferred_unique_ids: deferred,
        })
    }

    pub fn from_previous_state(previous_state: Option<Arc<StateArtifacts>>) -> Self {
        Self {
            state_artifacts: previous_state,
            ..Default::default()
        }
    }

    pub fn collect_auto_defer_unselected_node_ids(
        resolved_state_nodes: &Nodes,
        schedule: &Schedule<String>,
    ) -> BTreeSet<String> {
        // Uses `is_deferrable` to filter to resolver nodes that are model/seed/
        // snapshot/function and non-ephemeral. Defer nodes are unknown at this
        // point, hence passing resolver nodes twice. The actual defer-target
        // existence is confirmed later.
        schedule
            .frontier_nodes
            .iter()
            .filter(|unique_id| {
                resolved_state_nodes
                    .get_node(unique_id)
                    .is_some_and(|node| {
                        is_deferrable(node, resolved_state_nodes, resolved_state_nodes).is_some()
                    })
            })
            .cloned()
            .collect()
    }
}

/// Helper to check if a node is deferrable (exists in both resolver_state and defer_nodes).
fn is_deferrable(
    node: &dyn InternalDbtNodeAttributes,
    resolver_nodes: &Nodes,
    defer_nodes: &Nodes,
) -> Option<String> {
    let unique_id = node.unique_id();
    if (resolver_nodes.models.contains_key(&unique_id)
        || resolver_nodes.seeds.contains_key(&unique_id)
        || resolver_nodes.snapshots.contains_key(&unique_id)
        || resolver_nodes.functions.contains_key(&unique_id))
        && (defer_nodes.models.contains_key(&unique_id)
            || defer_nodes.seeds.contains_key(&unique_id)
            || defer_nodes.snapshots.contains_key(&unique_id)
            || defer_nodes.functions.contains_key(&unique_id))
        && !matches!(node.materialized(), DbtMaterialization::Ephemeral)
    {
        Some(unique_id)
    } else {
        None
    }
}

/// Phase 1: Common deferral (SA-independent), frontier + selected incrementals/snapshots.
/// Defers frontier nodes and selected incrementals/snapshots based on favor_state and relation availability.
///
/// Caller must load defer state via [`load_defer_state`] and extract/filter nodes before calling this.
pub async fn defer_common(
    arg: &EvalArgs,
    resolver_state: &mut ResolverState,
    defer_nodes: &mut Nodes,
    schedule: &Schedule<String>,
    adapter: &Arc<Adapter>,
) -> FsResult<DeferralUpdate> {
    let span = create_info_span(PhaseExecuted::start_general(ExecutionPhase::DeferHydration));

    defer_common_inner(arg, resolver_state, defer_nodes, schedule, adapter)
        .instrument(span.clone())
        .await
        .record_status(&span)
}

async fn defer_common_inner(
    arg: &EvalArgs,
    resolver_state: &mut ResolverState,
    defer_nodes: &mut Nodes,
    schedule: &Schedule<String>,
    adapter: &Arc<Adapter>,
) -> FsResult<DeferralUpdate> {
    // Handle compare command specially
    if arg.command == FsCommand::Extension("compare") {
        let mut refs_to_update = BTreeSet::new();
        for uid in &schedule.selected_nodes {
            if let Some(node) = resolver_state.nodes.get_node(uid)
                && node.package_name() == resolver_state.root_project_name
            {
                refs_to_update.insert(uid.clone());
            }
        }

        let deferred = update_refs_and_sources_with_deferral(
            &refs_to_update,
            defer_nodes,
            resolver_state,
            adapter,
            schedule,
        )?;

        return Ok(deferred);
    }

    // Phase 1: Collect frontier nodes and selected incrementals/snapshots
    let mut refs_to_update = BTreeSet::new();

    // Frontier nodes are immediate candidates for deferral
    for unique_id in &schedule.frontier_nodes {
        if let Some(node) = resolver_state.nodes.get_node(unique_id)
            && let Some(deferrable_id) = is_deferrable(node, &resolver_state.nodes, defer_nodes)
        {
            refs_to_update.insert(deferrable_id);
        }
    }

    // Add dependencies from operations (on_run_start and on_run_end)
    // Operations are always marked as Unsafe, so their dependencies should be deferred
    for operation in resolver_state
        .operations
        .on_run_start
        .iter()
        .chain(&resolver_state.operations.on_run_end)
    {
        for dep_id in &operation.__base_attr__.depends_on.nodes {
            if let Some(dep_node) = resolver_state.nodes.get_node(dep_id)
                && let Some(deferrable_id) =
                    is_deferrable(dep_node, &resolver_state.nodes, defer_nodes)
            {
                // Skip if already deferred in phase 1
                refs_to_update.insert(deferrable_id);
            }
        }
    }

    // Selected incrementals/snapshots need the current warehouse relation for schema merge
    // during compile. They must be deferred before the first schema hydration pass so recorded
    // `get_columns_in_relation(this)` calls are rewritten to the deferred relation.
    //
    // Keep this compile-only: build/run paths use local selected incrementals, and unit test
    // schema resolution reads deferred prod relations from persisted `defer_nodes` directly.
    if arg.command == FsCommand::Compile {
        for unique_id in &schedule.selected_nodes {
            let Some(node) = resolver_state.nodes.get_node(unique_id) else {
                continue;
            };

            if ((matches!(node.resource_type(), NodeType::Model)
                && matches!(node.materialized(), DbtMaterialization::Incremental))
                || matches!(node.resource_type(), NodeType::Snapshot))
                && let Some(deferrable_id) = is_deferrable(node, &resolver_state.nodes, defer_nodes)
            {
                refs_to_update.insert(deferrable_id);
            }
        }
    }

    // Apply favor_state filtering
    if !arg.favor_state {
        refs_to_update =
            filter_by_relation_availability(&refs_to_update, resolver_state, adapter).await;
    }

    let deferred = update_refs_and_sources_with_deferral(
        &refs_to_update,
        defer_nodes,
        resolver_state,
        adapter,
        schedule,
    )?;

    Ok(deferred)
}

/// Phase 2: SA-dependent upstream deferral.
/// Defers upstream dependencies based on SA strictness.
pub async fn defer_sa_upstreams(
    arg: &EvalArgs,
    resolver_state: &mut ResolverState,
    defer_nodes: &mut Nodes,
    deferred: &mut HashMap<CanonicalFqn, String>,
    schedule: &Schedule<String>,
    adapter: &Arc<Adapter>,
) -> FsResult<HashMap<CanonicalFqn, Arc<dyn BaseRelation>>> {
    if arg.command != FsCommand::Compile {
        // Only compile commands need SA-dependent deferral
        return Ok(HashMap::new());
    }
    let mut refs_to_update = BTreeSet::new();

    // Build a set of already-deferred unique IDs for O(1) lookups
    let already_deferred: HashSet<&str> = deferred.values().map(|v| v.as_str()).collect();

    // SA-dependent upstream deferral
    // - Nodes with data introspection (Execute/Unknown): defer all deferrable deps
    // - Other compile-time introspection nodes: defer deps whose schema will not be
    //   produced locally during this compile
    // - Unit tests: defer deps whose schema won't be produced by static analysis
    for unique_id in &schedule.selected_nodes {
        let Some(node) = resolver_state.nodes.get_node(unique_id) else {
            continue;
        };

        let Some(deps) = schedule.deps.get(unique_id) else {
            continue;
        };
        for dep in deps {
            // Skip if already deferred in phase 1
            if already_deferred.contains(dep.as_str()) {
                continue;
            }
            let Some(dep_node) = resolver_state.nodes.get_node(dep) else {
                continue;
            };
            let Some(deferrable_id) = is_deferrable(dep_node, &resolver_state.nodes, defer_nodes)
            else {
                continue;
            };

            if compile_node_requires_deferred_upstream(
                node,
                dep,
                dep_node,
                &schedule.frontier_nodes,
            ) {
                refs_to_update.insert(deferrable_id);
            }
        }
    }

    // Unit tests: defer non-strict deps of the model being tested
    let unit_test_dep_ids: Vec<_> = resolver_state
        .nodes
        .unit_tests
        .iter()
        .filter(|(id, _)| schedule.selected_nodes.contains(*id))
        .filter_map(|(_, ut)| {
            let model_id = format!(
                "model.{}.{}",
                ut.__common_attr__.package_name, ut.__unit_test_attr__.model
            );
            resolver_state.nodes.models.get(&model_id)
        })
        .flat_map(|model| &model.__base_attr__.depends_on.nodes)
        .cloned()
        .collect();

    for dep_id in unit_test_dep_ids {
        let Some(dep_node) = resolver_state.nodes.get_node(&dep_id) else {
            continue;
        };
        let Some(deferrable_id) = is_deferrable(dep_node, &resolver_state.nodes, defer_nodes)
        else {
            continue;
        };
        if already_deferred.contains(deferrable_id.as_str()) {
            continue;
        }
        if !is_strict_static_analysis(*dep_node.base().static_analysis) {
            refs_to_update.insert(deferrable_id);
        }
    }

    // Apply favor_state filtering for phase 2 candidates
    if !arg.favor_state && !refs_to_update.is_empty() {
        refs_to_update =
            filter_by_relation_availability(&refs_to_update, resolver_state, adapter).await;
    }

    // Update refs and extend deferred map
    if !refs_to_update.is_empty() {
        let new_deferred = update_refs_and_sources_with_deferral(
            &refs_to_update,
            defer_nodes,
            resolver_state,
            adapter,
            schedule,
        )?;
        deferred.extend(new_deferred.deferred_unique_ids);
        return Ok(new_deferred.relation_remap);
    }

    Ok(HashMap::new())
}

/// Helper to filter refs_to_update by relation availability (favor_state=false logic).
enum AvailabilityCandidate {
    Relation(Arc<dyn BaseRelation>),
    Function(Box<dyn BaseRelation>),
}

async fn filter_by_relation_availability(
    refs_to_update: &BTreeSet<String>,
    resolver_state: &ResolverState,
    adapter: &Arc<Adapter>,
) -> BTreeSet<String> {
    let mut dep_id_to_candidate: HashMap<String, AvailabilityCandidate> = HashMap::new();
    let mut relation_candidates = Vec::new();
    let mut catalog_schemas: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    refs_to_update.iter().for_each(|dep_id| {
        let Some(node) = resolver_state.nodes.get_node(dep_id) else {
            return;
        };
        if matches!(node.resource_type(), NodeType::Function) {
            let Some(dbt_function) = resolver_state.nodes.functions.get(dep_id) else {
                return;
            };
            let Ok(relation) =
                create_relation_from_node(adapter.adapter_type(), dbt_function.as_ref(), None)
            else {
                return;
            };
            let cs = CatalogAndSchema::from(relation.as_ref());
            catalog_schemas
                .entry(cs.resolved_catalog)
                .or_default()
                .insert(cs.resolved_schema);
            dep_id_to_candidate.insert(dep_id.clone(), AvailabilityCandidate::Function(relation));
            return;
        }

        let Ok(relation) = create_relation_from_node(adapter.adapter_type(), node, None) else {
            return;
        };
        let relation: Arc<dyn BaseRelation> = relation.into();
        relation_candidates.push(relation.clone());
        dep_id_to_candidate.insert(dep_id.clone(), AvailabilityCandidate::Relation(relation));
    });

    let _ = hydrate_relation_cache_if_not_already_cached(
        &relation_candidates,
        adapter,
        "downloading deferred relations",
    )
    .await;
    let available_functions = query_available_functions(&catalog_schemas, adapter).await;

    let relation_cache = adapter.engine().relation_cache();
    dep_id_to_candidate
        .into_iter()
        .filter_map(|(dep_id, candidate)| match candidate {
            AvailabilityCandidate::Relation(relation) => {
                let relation_schema = CatalogAndSchema::from(&relation);
                let schema_is_cached = relation_cache.contains_full_schema(&relation_schema);
                let cache_has_relation = relation_cache.contains_relation(relation.as_ref());
                (schema_is_cached && !cache_has_relation).then_some(dep_id)
            }
            AvailabilityCandidate::Function(relation) => {
                (!available_functions.contains(&relation.semantic_fqn())).then_some(dep_id)
            }
        })
        .collect()
}

/// Query the metadata adapter for functions that actually exist in the relevant schemas.
/// Returns a set of `semantic_fqn` strings for relation-based matching, using the same
/// quoting policy as `list_relations` so that normalization is adapter-correct.
async fn query_available_functions(
    catalog_schemas: &BTreeMap<String, BTreeSet<String>>,
    adapter: &Arc<Adapter>,
) -> HashSet<String> {
    if catalog_schemas.is_empty() {
        return HashSet::new();
    }
    let Some(metadata_adapter) = adapter.metadata_adapter() else {
        return HashSet::new();
    };

    let Ok(udfs) = metadata_adapter
        .list_user_defined_functions(catalog_schemas, adapter.cancellation_token())
        .await
        .map_err(into_fs_error)
    else {
        return HashSet::new();
    };

    udfs.into_iter()
        .filter_map(|udf| {
            let parts: Vec<&str> = udf.name.split('.').collect();
            let [catalog, schema, name] = parts.as_slice() else {
                return None;
            };
            let relation = create_relation(
                adapter.adapter_type(),
                catalog.trim().to_string(),
                schema.trim().to_string(),
                Some(name.trim().to_string()),
                None,
                adapter.engine().quoting(),
            )
            .ok()?;
            Some(relation.semantic_fqn())
        })
        .collect()
}

/// Returns `true` when a deferred node's `__base_attr__.{database,schema}`
/// must be rewritten onto the local project copy so that downstream consumers
/// of `base_attr` see the deferred (typically prod) identity.
///
/// Per-kind rules:
/// - Models: rewrite for frontier (dependency) nodes and for selected
///   incremental models.  Other selected models keep their local schema so
///   that static analysis can register their CFQN correctly.
/// - Seeds: rewrite for frontier only. Selected seeds keep their local
///   schema for the same CFQN-registration reason as models.
/// - Snapshots: always rewrite. Snapshots need production attributes so the
///   existing warehouse schema can be merged with the local one during
///   analysis.
/// - Functions: always rewrite. The UDF registry (populated by
///   `task_runner::set_function_registry` via
///   `crate::udf::dbt_function_to_function`) is keyed by FQN; without this
///   rewrite strict static analysis fails with
///   `dbt0209: No function <deferred_fqn>` in CI runs with schema overrides
///   (see #9049 follow-up).
///
/// This predicate is the mirror of the "swap the relation?" predicate used
/// by the Jinja layer in
/// [`fs/sa/crates/dbt-jinja-utils/src/node_resolver.rs`]'s
/// `set_deferred_relation`: `is_frontier || is_incremental_or_snapshot`,
/// with functions following an unconditional early-return path.  The two
/// must stay in lockstep — if one layer rewrites and the other doesn't,
/// compiled SQL (Jinja) and the binder's state (base_attr) disagree and
/// strict static analysis raises `dbt0209`.
fn should_defer_base_attr(node: &dyn InternalDbtNodeAttributes, is_frontier: bool) -> bool {
    match node.resource_type() {
        NodeType::Function | NodeType::Snapshot => true,
        NodeType::Seed => is_frontier,
        NodeType::Model => {
            is_frontier || matches!(node.materialized(), DbtMaterialization::Incremental)
        }
        _ => false,
    }
}

/// Apply deferral to a single node: rewrite Jinja `ref()` to render the
/// deferred FQN, and (when [`should_defer_base_attr`] says so) rewrite the
/// local node's `__base_attr__.{database,schema}` to match.
///
/// The two rewrites must stay in lockstep — the Jinja side drives what ends
/// up in compiled SQL, and the `base_attr` side drives what downstream
/// consumers (the schema store's `Frontier` init and the UDF registry built
/// by `task_runner::set_function_registry`) use to resolve that SQL. The
/// customer-reported `dbt0209: No function <deferred_fqn>` regression from
/// #9049 was exactly a case where only the Jinja side was updated for
/// functions; wrapping the two calls in a single helper makes that class of
/// drift impossible.
fn apply_deferral_to_node<N>(
    node_resolver: &mut dyn NodeResolverTracker,
    local_nodes: &mut BTreeMap<String, Arc<N>>,
    unique_id: &str,
    deferred_node: &N,
    adapter_type: AdapterType,
    is_frontier: bool,
) -> FsResult<()>
where
    N: InternalDbtNode + InternalDbtNodeAttributes + Clone,
{
    node_resolver.update_ref_with_deferral(deferred_node, adapter_type, is_frontier)?;
    if should_defer_base_attr(deferred_node, is_frontier)
        && let Some(node) = local_nodes.get_mut(unique_id)
    {
        let mut_node = Arc::make_mut(node);
        let deferred_base = deferred_node.base();
        let base = mut_node.base_mut();
        base.database = deferred_base.database.clone();
        base.schema = deferred_base.schema.clone();
    }
    Ok(())
}

fn update_refs_and_sources_with_deferral(
    refs_to_update: &BTreeSet<String>,
    defer_nodes: &mut Nodes,
    resolver_state: &mut ResolverState,
    adapter: &Arc<Adapter>,
    schedule: &Schedule<String>,
) -> FsResult<DeferralUpdate> {
    let node_resolver = Arc::get_mut(&mut resolver_state.node_resolver)
        .unwrap_or_else(|| panic!("Expected mutable reference to node_resolver"));

    // Collect in order to later fetch the schemas for these nodes (they are effectively sources)
    let mut deferred_unique_ids = HashMap::new();
    let mut relation_remap = HashMap::new();

    for dep_id in refs_to_update {
        let is_frontier = schedule.frontier_nodes.contains(dep_id);
        let current_relation = resolver_state
            .nodes
            .get_node(dep_id)
            .and_then(|node| create_relation_from_node(adapter.adapter_type(), node, None).ok())
            .map(Arc::<dyn BaseRelation>::from);

        // Try to find the node in any of the defer collections and apply the
        // deferral via [`apply_deferral_to_node`], which rewrites Jinja `ref()`
        // plus (when [`should_defer_base_attr`] says so) the local node's
        // `__base_attr__.{database,schema}`. Per-kind rules live on
        // `should_defer_base_attr`'s doc comment.
        let relation = if let Some(defer_model) = defer_nodes.models.get(dep_id) {
            apply_deferral_to_node(
                node_resolver,
                &mut resolver_state.nodes.models,
                dep_id,
                defer_model.as_ref(),
                adapter.adapter_type(),
                is_frontier,
            )?;
            create_relation_from_node(adapter.adapter_type(), defer_model.as_ref(), None)?
        } else if let Some(defer_seed) = defer_nodes.seeds.get(dep_id) {
            apply_deferral_to_node(
                node_resolver,
                &mut resolver_state.nodes.seeds,
                dep_id,
                defer_seed.as_ref(),
                adapter.adapter_type(),
                is_frontier,
            )?;
            create_relation_from_node(adapter.adapter_type(), defer_seed.as_ref(), None)?
        } else if let Some(defer_snapshot) = defer_nodes.snapshots.get(dep_id) {
            apply_deferral_to_node(
                node_resolver,
                &mut resolver_state.nodes.snapshots,
                dep_id,
                defer_snapshot.as_ref(),
                adapter.adapter_type(),
                is_frontier,
            )?;
            create_relation_from_node(adapter.adapter_type(), defer_snapshot.as_ref(), None)?
        } else if let Some(defer_function) = defer_nodes.functions.get(dep_id) {
            apply_deferral_to_node(
                node_resolver,
                &mut resolver_state.nodes.functions,
                dep_id,
                defer_function.as_ref(),
                adapter.adapter_type(),
                is_frontier,
            )?;
            create_relation_from_node(adapter.adapter_type(), defer_function.as_ref(), None)?
        } else {
            continue;
        };
        let new_relation = Arc::<dyn BaseRelation>::from(relation);
        deferred_unique_ids.insert(new_relation.get_canonical_fqn()?, dep_id.to_string());
        if let Some(old_relation) = current_relation
            && let Ok(old_cfqn) = old_relation.get_canonical_fqn()
        {
            relation_remap.insert(old_cfqn, Arc::clone(&new_relation));
        }
    }
    Ok(DeferralUpdate {
        deferred_unique_ids,
        relation_remap,
    })
}

pub fn rewrite_recorded_relation_calls_with_deferral(
    resolver_state: &mut ResolverState,
    relation_remap: &HashMap<CanonicalFqn, Arc<dyn BaseRelation>>,
) {
    rewrite_relation_call_map(&mut resolver_state.get_relation_calls, relation_remap);
    rewrite_relation_call_map(
        &mut resolver_state.get_columns_in_relation_calls,
        relation_remap,
    );
}

fn rewrite_relation_call_map(
    calls: &mut BTreeMap<String, Vec<Arc<dyn BaseRelation>>>,
    relation_remap: &HashMap<CanonicalFqn, Arc<dyn BaseRelation>>,
) {
    if relation_remap.is_empty() {
        return;
    }

    for relations in calls.values_mut() {
        for relation in relations.iter_mut() {
            if let Ok(cfqn) = relation.get_canonical_fqn()
                && let Some(rewritten) = relation_remap.get(&cfqn)
            {
                *relation = Arc::clone(rewritten);
            }
        }
    }
}

fn modified_nodes(
    nodes: &Nodes,
    previous_state: Option<&StateArtifacts>,
    adapter_type: AdapterType,
) -> FsResult<BTreeSet<String>> {
    // find all modified nodes and all of their children
    let criteria = SelectionCriteria::new(
        MethodName::State,
        vec![],
        "modified".to_string(),
        true,
        None,
        Some(u32::MAX),
        None,
        None,
    );
    filter_select_criteria(nodes, &criteria, previous_state, adapter_type)
}

/// Generate a semantic manifest using deferred relations for semantic resources
/// when `favor_state` is set or when those resources are unmodified. This
/// matches dbt-mantle behavior:
/// https://github.com/dbt-labs/dbt-mantle/blob/16a1c6f5863ef9e29a9799de8b8ce0b97242fd7d/core/dbt/task/runnable.py#L832
///
/// This intentionally does not mirror normal dbt deferral exactly. For models,
/// `--defer` can decide between target and state relations after checking whether
/// the target relation already exists in the warehouse. Semantic manifests are
/// generated during parsing, before we have a live warehouse connection, so we
/// cannot do that existence check here. As a result, semantic resources use a
/// parse-time approximation instead:
/// - `favor_state=true`: always point at deferred/state relations.
/// - `favor_state=false`: keep modified resources on local target relations and
///   point unmodified resources at deferred/state relations.
///
/// That keeps modified semantic nodes aligned with the development graph while
/// still letting unchanged semantic resources resolve through state/prod.
pub fn get_deferred_semantic_manifest(
    arg: &EvalArgs,
    resolved_state: &ResolverState,
    previous_state: Option<Arc<StateArtifacts>>,
    defer_nodes: &Nodes,
) -> FsResult<SemanticManifest> {
    let modified_nodes = modified_nodes(
        &resolved_state.nodes,
        previous_state.as_deref(),
        resolved_state.adapter_type,
    )?;

    Ok(build_deferred_semantic_manifest(
        &resolved_state.nodes,
        defer_nodes,
        arg.favor_state,
        &modified_nodes,
    ))
}

fn build_deferred_semantic_manifest(
    current_nodes: &Nodes,
    defer_nodes: &Nodes,
    favor_state: bool,
    modified_nodes: &BTreeSet<String>,
) -> SemanticManifest {
    // Only clone the fields that SemanticManifest::from actually uses:
    // The timespine model, semantic models, metrics, and saved queries.
    let mut nodes = Nodes {
        models: current_nodes
            .models
            .iter()
            .filter(|(_, m)| m.__model_attr__.time_spine.is_some())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        semantic_models: current_nodes.semantic_models.clone(),
        metrics: current_nodes.metrics.clone(),
        saved_queries: current_nodes.saved_queries.clone(),
        ..Default::default()
    };

    // Replace Time Spine model with deferred relation if the favor-state flag is passed or
    // if the model is unmodified
    for (unique_id, current_node) in nodes.models.iter_mut() {
        if !favor_state && modified_nodes.contains(unique_id) {
            continue;
        }

        if let Some(deferred_node) = defer_nodes.models.get(unique_id) {
            let curr = Arc::make_mut(current_node);
            curr.__base_attr__.database = deferred_node.__base_attr__.database.clone();
            curr.__base_attr__.schema = deferred_node.__base_attr__.schema.clone();
        }
    }

    // A semantic model's `node_relation` is just a copy of its underlying
    // ref'd model's relation (see `resolve_semantic_models.rs`), so whether to
    // defer it must be keyed off that underlying model's modified status, not
    // the semantic model's own unique_id (which changes whenever the YAML
    // itself is edited, even if the model it points at is unmodified). This
    // mirrors dbt-mantle's `_get_unmodified_models`, which restricts the
    // modified set to `NodeType.Model` nodes.
    let modified_models: BTreeSet<String> = modified_nodes
        .iter()
        .filter(|id| current_nodes.models.contains_key(*id))
        .cloned()
        .collect();

    // Semantic manifests are emitted before warehouse introspection, so unlike
    // normal model deferral we cannot ask whether the target relation exists.
    // `favor_state` therefore acts as the explicit opt-in to always prefer the
    // deferred semantic relation; otherwise only semantic models whose
    // underlying model is unmodified are rewritten to state/prod.
    for (unique_id, current_node) in nodes.semantic_models.iter_mut() {
        let underlying_model_id = current_node.__base_attr__.depends_on.nodes.first();
        if !favor_state && underlying_model_id.is_none_or(|id| modified_models.contains(id)) {
            continue;
        }

        if let Some(deferred_node) = defer_nodes.semantic_models.get(unique_id) {
            let curr = Arc::make_mut(current_node);
            curr.__semantic_model_attr__.node_relation =
                deferred_node.__semantic_model_attr__.node_relation.clone();
        }
    }

    // Saved query exports follow the same parse-time rule as semantic models so
    // the semantic manifest stays internally consistent: modified nodes keep
    // their local export target unless `favor_state` forces state/prod.
    for (unique_id, current_node) in nodes.saved_queries.iter_mut() {
        if !favor_state && modified_nodes.contains(unique_id) {
            continue;
        }

        if let Some(deferred_node) = defer_nodes.saved_queries.get(unique_id) {
            let curr = Arc::make_mut(current_node);
            curr.__saved_query_attr__.exports = deferred_node.__saved_query_attr__.exports.clone();
            curr.__base_attr__.database = deferred_node.__base_attr__.database.clone();
            curr.__base_attr__.schema = deferred_node.__base_attr__.schema.clone();
            curr.__base_attr__.alias = deferred_node.__base_attr__.alias.clone();
        }
    }

    SemanticManifest::from(&nodes)
}

/// Update the node resolver's ref lookup table so that `{{ ref('model') }}` compiles to
/// production relation names from the state manifest if the relation does not exist in the dev
/// schema, without modifying `base_attr.database/schema`. Local static analysis is therefore unaffected.
///
/// Nodes already handled by `defer_common` (incrementals, snapshots, frontier nodes) are
/// skipped via `already_deferred_unique_ids` to avoid a redundant second write.
pub async fn update_ref_lookups_from_state(
    resolver_state: &mut ResolverState,
    defer_nodes: &Nodes,
    already_deferred_unique_ids: &HashSet<String>,
    adapter: &Arc<Adapter>,
    favor_state: bool,
) -> FsResult<()> {
    let adapter_type = adapter.adapter_type();

    // Collect local keys first (immutable borrow) before taking mutable node_resolver.
    let local_models: HashSet<String> = resolver_state.nodes.models.keys().cloned().collect();
    let local_seeds: HashSet<String> = resolver_state.nodes.seeds.keys().cloned().collect();
    let local_snapshots: HashSet<String> = resolver_state.nodes.snapshots.keys().cloned().collect();
    let local_functions: HashSet<String> = resolver_state.nodes.functions.keys().cloned().collect();

    // Candidate nodes: local nodes present in the defer state that `defer_common` didn't already handle.
    let mut candidates: BTreeSet<String> = BTreeSet::new();
    for uid in defer_nodes.models.keys() {
        if !already_deferred_unique_ids.contains(uid) && local_models.contains(uid) {
            candidates.insert(uid.clone());
        }
    }
    for uid in defer_nodes.seeds.keys() {
        if !already_deferred_unique_ids.contains(uid) && local_seeds.contains(uid) {
            candidates.insert(uid.clone());
        }
    }
    for uid in defer_nodes.snapshots.keys() {
        if !already_deferred_unique_ids.contains(uid) && local_snapshots.contains(uid) {
            candidates.insert(uid.clone());
        }
    }
    for uid in defer_nodes.functions.keys() {
        if !already_deferred_unique_ids.contains(uid) && local_functions.contains(uid) {
            candidates.insert(uid.clone());
        }
    }

    let to_defer = if favor_state {
        candidates
    } else {
        filter_by_relation_availability(&candidates, resolver_state, adapter).await
    };

    let node_resolver = Arc::get_mut(&mut resolver_state.node_resolver)
        .expect("Expected mutable reference to node_resolver for update_ref_lookups_from_state");

    for (uid, node) in &defer_nodes.models {
        if to_defer.contains(uid) {
            node_resolver.update_ref_with_deferral(node.as_ref(), adapter_type, true)?;
        }
    }
    for (uid, node) in &defer_nodes.seeds {
        if to_defer.contains(uid) {
            node_resolver.update_ref_with_deferral(node.as_ref(), adapter_type, true)?;
        }
    }
    for (uid, node) in &defer_nodes.snapshots {
        if to_defer.contains(uid) {
            node_resolver.update_ref_with_deferral(node.as_ref(), adapter_type, true)?;
        }
    }
    for (uid, node) in &defer_nodes.functions {
        if to_defer.contains(uid) {
            node_resolver.update_ref_with_deferral(node.as_ref(), adapter_type, true)?;
        }
    }
    Ok(())
}

/// Pre-compute defer context and store it on the `NodeResolver` for O(1) per-ref evaluation.
///
/// Must be called BEFORE `build_compiler_env` (which clones the Arc) and AFTER
/// defer phases + SA classification have finished.
pub fn set_defer_context_on_resolver(
    resolver_state: &mut ResolverState,
    sorted_nodes: &[String],
    frontier_nodes: &BTreeSet<String>,
) {
    // Build the 3 collections from resolver_state.nodes (immutable borrow)
    // before touching node_resolver (mutable borrow).
    let mut node_introspections = HashMap::new();
    let mut has_analyzed_schema = HashSet::new();
    let mut nodes_materialized = HashSet::new();

    for uid in sorted_nodes {
        let is_frontier_or_source = frontier_nodes.contains(uid) || uid.starts_with("source.");

        if let Some(node) = resolver_state.nodes.get_node(uid) {
            node_introspections.insert(uid.clone(), node.introspection());

            if node_will_produce_local_analyzed_schema(uid, node, frontier_nodes) {
                has_analyzed_schema.insert(uid.clone());
            }

            // nodes_materialized: selected (in sorted_nodes), not frontier, not source
            if !is_frontier_or_source {
                nodes_materialized.insert(uid.clone());
            }
        }
    }

    let node_resolver = Arc::get_mut(&mut resolver_state.node_resolver)
        .expect("Expected mutable reference to node_resolver for set_defer_context");
    node_resolver.set_defer_context(node_introspections, has_analyzed_schema, nodes_materialized);
}

/// Only when the node is selected and its static analysis is set to strict,
/// it will produce a locally analyzed schema.
pub fn node_will_produce_local_analyzed_schema(
    unique_id: &str,
    node: &dyn InternalDbtNodeAttributes,
    frontier_nodes: &BTreeSet<String>,
) -> bool {
    node_is_selected(unique_id, frontier_nodes)
        && is_strict_static_analysis(*node.base().static_analysis)
}

pub fn node_is_selected(unique_id: &str, frontier_nodes: &BTreeSet<String>) -> bool {
    !frontier_nodes.contains(unique_id) && !unique_id.starts_with("source.")
}

fn compile_node_requires_deferred_upstream(
    current_node: &dyn InternalDbtNodeAttributes,
    upstream_id: &str,
    upstream_node: &dyn InternalDbtNodeAttributes,
    frontier_nodes: &BTreeSet<String>,
) -> bool {
    match current_node.introspection() {
        IntrospectionKind::None => false,
        IntrospectionKind::Execute | IntrospectionKind::Unknown => true,
        _ => !node_will_produce_local_analyzed_schema(upstream_id, upstream_node, frontier_nodes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_common::io_args::StaticAnalysisKind;
    use dbt_jinja_utils::node_resolver::NodeResolver;
    use dbt_schemas::schemas::DbtModel;
    use dbt_schemas::schemas::manifest::saved_query::{
        DbtSavedQuery, DbtSavedQueryAttr, SavedQueryExport, SavedQueryExportConfig,
        SavedQueryParams,
    };
    use dbt_schemas::schemas::manifest::semantic_model::{
        DbtSemanticModel, DbtSemanticModelAttr, NodeRelation,
    };
    use dbt_schemas::schemas::project::SavedQueryConfig;
    use dbt_schemas::schemas::{CommonAttributes, NodeBaseAttributes};
    use dbt_schemas::state::NodeResolverTracker;

    fn create_common_attr(name: &str) -> CommonAttributes {
        CommonAttributes {
            unique_id: name.to_string(),
            name: name.to_string(),
            package_name: "test".to_string(),
            fqn: vec!["test".to_string(), name.to_string()],
            path: format!("{name}.sql").into(),
            original_file_path: format!("{name}.sql").into(),
            raw_code: Some("SELECT 1".to_string()),
            patch_path: None,
            checksum: Default::default(),
            language: Some("sql".to_string()),
            description: None,
            ..Default::default()
        }
    }

    fn create_base_attr(name: &str) -> NodeBaseAttributes {
        NodeBaseAttributes {
            database: "test_db".to_string(),
            schema: "test_schema".to_string(),
            alias: name.to_string(),
            relation_name: Some(format!("test_db.test_schema.{name}")),
            ..Default::default()
        }
    }

    fn make_semantic_model_with_depends_on(
        unique_id: &str,
        name: &str,
        schema: &str,
        alias: &str,
        depends_on_nodes: &[&str],
    ) -> Arc<DbtSemanticModel> {
        Arc::new(DbtSemanticModel {
            __common_attr__: CommonAttributes {
                unique_id: unique_id.to_string(),
                name: name.to_string(),
                package_name: "test".to_string(),
                fqn: vec!["test".to_string(), name.to_string()],
                path: format!("{name}.yml").into(),
                original_file_path: format!("{name}.yml").into(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                database: "dbt".to_string(),
                schema: schema.to_string(),
                alias: alias.to_string(),
                depends_on: dbt_schemas::schemas::common::NodeDependsOn {
                    nodes: depends_on_nodes.iter().map(|s| s.to_string()).collect(),
                    ..Default::default()
                },
                ..Default::default()
            },
            __semantic_model_attr__: DbtSemanticModelAttr {
                node_relation: Some(NodeRelation {
                    alias: alias.to_string(),
                    schema_name: schema.to_string(),
                    database: Some("dbt".to_string()),
                    relation_name: Some(format!("dbt.{schema}.{alias}")),
                }),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    /// Minimal underlying `.sql` model that a semantic model's
    /// `depends_on.nodes` points at, for exercising the
    /// underlying-model-modified-status keying in
    /// `build_deferred_semantic_manifest`.
    fn make_underlying_model(unique_id: &str, name: &str, schema: &str) -> Arc<DbtModel> {
        Arc::new(DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: unique_id.to_string(),
                ..create_common_attr(name)
            },
            __base_attr__: NodeBaseAttributes {
                schema: schema.to_string(),
                ..create_base_attr(name)
            },
            ..Default::default()
        })
    }

    fn make_saved_query(
        unique_id: &str,
        name: &str,
        export_schema: &str,
        export_alias: &str,
    ) -> Arc<DbtSavedQuery> {
        Arc::new(DbtSavedQuery {
            __common_attr__: CommonAttributes {
                unique_id: unique_id.to_string(),
                name: name.to_string(),
                package_name: "test".to_string(),
                fqn: vec!["test".to_string(), name.to_string()],
                path: format!("{name}.yml").into(),
                original_file_path: format!("{name}.yml").into(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                database: "dbt".to_string(),
                schema: export_schema.to_string(),
                alias: export_alias.to_string(),
                ..Default::default()
            },
            __saved_query_attr__: DbtSavedQueryAttr {
                query_params: SavedQueryParams::default(),
                exports: vec![SavedQueryExport {
                    name: "export".to_string(),
                    config: SavedQueryExportConfig {
                        export_as: Default::default(),
                        schema_name: Some(export_schema.to_string()),
                        alias: Some(export_alias.to_string()),
                        database: Some("dbt".to_string()),
                    },
                    unrendered_config: Default::default(),
                }],
                ..Default::default()
            },
            deprecated_config: SavedQueryConfig::default(),
            __other__: Default::default(),
        })
    }

    /// Unique ID of the underlying `.sql` model that the `orders` semantic
    /// model in [`make_semantic_manifest_nodes`] depends on, matching a
    /// realistic manifest where a semantic model's `depends_on.nodes` always
    /// points at the model it wraps.
    const ORDERS_UNDERLYING_MODEL_ID: &str = "model.test.fct_orders";

    fn make_semantic_manifest_nodes(semantic_schema: &str, export_schema: &str) -> Nodes {
        let mut nodes = Nodes {
            project_name: Some("test".to_string()),
            ..Default::default()
        };
        nodes.models.insert(
            ORDERS_UNDERLYING_MODEL_ID.to_string(),
            make_underlying_model(ORDERS_UNDERLYING_MODEL_ID, "fct_orders", semantic_schema),
        );
        nodes.semantic_models.insert(
            "semantic_model.test.orders".to_string(),
            make_semantic_model_with_depends_on(
                "semantic_model.test.orders",
                "orders",
                semantic_schema,
                "fct_orders",
                &[ORDERS_UNDERLYING_MODEL_ID],
            ),
        );
        nodes.saved_queries.insert(
            "saved_query.test.orders_export".to_string(),
            make_saved_query(
                "saved_query.test.orders_export",
                "orders_export",
                export_schema,
                "orders_export",
            ),
        );
        nodes
    }

    fn manifest_schemas(manifest: SemanticManifest) -> (String, String) {
        let semantic_schema = manifest.semantic_models[0]
            .node_relation
            .as_ref()
            .expect("semantic model should have a node relation")
            .schema_name
            .clone();
        let export_schema = manifest.saved_queries[0].exports[0]
            .config
            .schema_name
            .clone()
            .expect("saved query export should have a schema");
        (semantic_schema, export_schema)
    }

    #[test]
    fn test_deferred_semantic_manifest_defers_saved_query_exports_for_unmodified_nodes() {
        let current_nodes = make_semantic_manifest_nodes("dev_schema", "dev_schema");
        let defer_nodes = make_semantic_manifest_nodes("prod_schema", "prod_schema");

        let (semantic_schema, export_schema) = manifest_schemas(build_deferred_semantic_manifest(
            &current_nodes,
            &defer_nodes,
            false,
            &BTreeSet::new(),
        ));

        assert_eq!(semantic_schema, "prod_schema");
        assert_eq!(export_schema, "prod_schema");
    }

    #[test]
    fn test_deferred_semantic_manifest_keeps_current_saved_query_exports_when_modified_without_favor_state()
     {
        let current_nodes = make_semantic_manifest_nodes("dev_schema", "dev_schema");
        let defer_nodes = make_semantic_manifest_nodes("prod_schema", "prod_schema");
        // The semantic model defers to its *underlying model's* modified
        // status (see `ORDERS_UNDERLYING_MODEL_ID`), not its own unique_id;
        // the saved query still keys off its own unique_id.
        let modified_nodes = BTreeSet::from([
            ORDERS_UNDERLYING_MODEL_ID.to_string(),
            "saved_query.test.orders_export".to_string(),
        ]);

        let (semantic_schema, export_schema) = manifest_schemas(build_deferred_semantic_manifest(
            &current_nodes,
            &defer_nodes,
            false,
            &modified_nodes,
        ));

        assert_eq!(semantic_schema, "dev_schema");
        assert_eq!(export_schema, "dev_schema");
    }

    #[test]
    fn test_deferred_semantic_manifest_favors_state_for_modified_saved_query_exports() {
        let current_nodes = make_semantic_manifest_nodes("dev_schema", "dev_schema");
        let defer_nodes = make_semantic_manifest_nodes("prod_schema", "prod_schema");
        let modified_nodes = BTreeSet::from([
            "semantic_model.test.orders".to_string(),
            "saved_query.test.orders_export".to_string(),
        ]);

        let (semantic_schema, export_schema) = manifest_schemas(build_deferred_semantic_manifest(
            &current_nodes,
            &defer_nodes,
            true,
            &modified_nodes,
        ));

        assert_eq!(semantic_schema, "prod_schema");
        assert_eq!(export_schema, "prod_schema");
    }

    /// Regression test for dbt-core#15640: a semantic model's `node_relation`
    /// is just a copy of its underlying ref'd `.sql` model's relation. If a
    /// PR only edits the semantic model's YAML (e.g. adds a dimension) the
    /// semantic model's own `unique_id` shows up in `state:modified+`, but
    /// the underlying model it depends on may be completely unmodified (and
    /// therefore never rebuilt locally when defer is enabled in CI). In that
    /// case the deferred semantic manifest must still point at the
    /// state/prod relation for the semantic model, not the local target
    /// relation, since the local target was never created.
    ///
    /// Before the fix, deferral was keyed off the semantic model's own
    /// unique_id being in `modified_nodes`, so this case incorrectly kept
    /// the semantic model pinned to its local (never-built) relation.
    #[test]
    fn test_deferred_semantic_manifest_defers_semantic_model_when_only_yaml_modified() {
        let underlying_model_id = "model.test.fct_orders";

        let mut current_nodes = Nodes {
            project_name: Some("test".to_string()),
            ..Default::default()
        };
        current_nodes.models.insert(
            underlying_model_id.to_string(),
            make_underlying_model(underlying_model_id, "fct_orders", "dev_schema"),
        );
        current_nodes.semantic_models.insert(
            "semantic_model.test.orders".to_string(),
            make_semantic_model_with_depends_on(
                "semantic_model.test.orders",
                "orders",
                "dev_schema",
                "fct_orders",
                &[underlying_model_id],
            ),
        );

        let mut defer_nodes = Nodes {
            project_name: Some("test".to_string()),
            ..Default::default()
        };
        defer_nodes.models.insert(
            underlying_model_id.to_string(),
            make_underlying_model(underlying_model_id, "fct_orders", "prod_schema"),
        );
        defer_nodes.semantic_models.insert(
            "semantic_model.test.orders".to_string(),
            make_semantic_model_with_depends_on(
                "semantic_model.test.orders",
                "orders",
                "prod_schema",
                "fct_orders",
                &[underlying_model_id],
            ),
        );

        // Only the semantic model's own unique_id is "modified" (YAML-only
        // change, e.g. a new dimension). The underlying `.sql` model it
        // depends on is unmodified.
        let modified_nodes = BTreeSet::from(["semantic_model.test.orders".to_string()]);

        let manifest = build_deferred_semantic_manifest(
            &current_nodes,
            &defer_nodes,
            /* favor_state */ false,
            &modified_nodes,
        );

        let semantic_schema = manifest.semantic_models[0]
            .node_relation
            .as_ref()
            .expect("semantic model should have a node relation")
            .schema_name
            .clone();

        assert_eq!(
            semantic_schema, "prod_schema",
            "semantic model relation should defer to state/prod when only its own YAML \
             (not the underlying ref'd model) was modified"
        );
    }

    #[test]
    fn test_is_deferrable() {
        let make_model = |name: &str, mat: DbtMaterialization| -> Arc<DbtModel> {
            Arc::new(DbtModel {
                __common_attr__: create_common_attr(name),
                __base_attr__: {
                    let mut base = create_base_attr(name);
                    base.materialized = mat;
                    base
                },
                ..Default::default()
            })
        };

        // Setup: resolver_nodes and defer_nodes both contain model1 (table)
        let mut resolver_nodes = Nodes::default();
        let mut defer_nodes = Nodes::default();

        resolver_nodes.models.insert(
            "model1".to_string(),
            make_model("model1", DbtMaterialization::Table),
        );
        defer_nodes.models.insert(
            "model1".to_string(),
            make_model("model1", DbtMaterialization::Table),
        );

        // Model present in both → deferrable
        let node = resolver_nodes.models.get("model1").unwrap();
        assert_eq!(
            is_deferrable(node.as_ref(), &resolver_nodes, &defer_nodes),
            Some("model1".to_string())
        );

        // Model only in resolver (not in defer) → not deferrable
        resolver_nodes.models.insert(
            "model2".to_string(),
            make_model("model2", DbtMaterialization::Table),
        );
        let node = resolver_nodes.models.get("model2").unwrap();
        assert_eq!(
            is_deferrable(node.as_ref(), &resolver_nodes, &defer_nodes),
            None
        );

        // Ephemeral model present in both → not deferrable (excluded by materialization)
        resolver_nodes.models.insert(
            "model3".to_string(),
            make_model("model3", DbtMaterialization::Ephemeral),
        );
        defer_nodes.models.insert(
            "model3".to_string(),
            make_model("model3", DbtMaterialization::Ephemeral),
        );
        let node = resolver_nodes.models.get("model3").unwrap();
        assert_eq!(
            is_deferrable(node.as_ref(), &resolver_nodes, &defer_nodes),
            None
        );
    }

    // --- should_defer_base_attr --------------------------------------------
    //
    // These tests pin the per-kind rule encoded in `should_defer_base_attr`.
    // That predicate must stay in lockstep with the Jinja-layer swap
    // predicate in `NodeResolver::set_deferred_relation`
    // (`is_frontier || is_incremental_or_snapshot`, with functions taking an
    // unconditional early-return path): if the two drift, compiled SQL
    // (Jinja) and binder state (`base_attr`) disagree and strict static
    // analysis raises `dbt0209`.

    fn make_plain_model(name: &str, mat: DbtMaterialization) -> DbtModel {
        DbtModel {
            __common_attr__: create_common_attr(name),
            __base_attr__: NodeBaseAttributes {
                materialized: mat,
                ..create_base_attr(name)
            },
            ..Default::default()
        }
    }

    fn make_plain_seed(name: &str) -> dbt_schemas::schemas::DbtSeed {
        dbt_schemas::schemas::DbtSeed {
            __common_attr__: create_common_attr(name),
            __base_attr__: create_base_attr(name),
            ..Default::default()
        }
    }

    fn make_plain_snapshot(name: &str) -> dbt_schemas::schemas::DbtSnapshot {
        dbt_schemas::schemas::DbtSnapshot {
            __common_attr__: create_common_attr(name),
            __base_attr__: NodeBaseAttributes {
                materialized: DbtMaterialization::Snapshot,
                ..create_base_attr(name)
            },
            ..Default::default()
        }
    }

    fn make_plain_function(name: &str) -> dbt_schemas::schemas::DbtFunction {
        dbt_schemas::schemas::DbtFunction {
            __common_attr__: create_common_attr(name),
            __base_attr__: create_base_attr(name),
            ..Default::default()
        }
    }

    #[test]
    fn test_should_defer_base_attr_frontier_model() {
        let model = make_plain_model("m", DbtMaterialization::View);
        assert!(should_defer_base_attr(&model, /* is_frontier */ true));
    }

    #[test]
    fn test_should_defer_base_attr_selected_non_incremental_model_keeps_local() {
        let model = make_plain_model("m", DbtMaterialization::View);
        assert!(!should_defer_base_attr(
            &model, /* is_frontier */ false
        ));
    }

    #[test]
    fn test_should_defer_base_attr_selected_incremental_model_rewrites() {
        // Selected incremental models still need the deferred `base_attr`
        // so that schema-merge during analysis compares against prod.
        let model = make_plain_model("m", DbtMaterialization::Incremental);
        assert!(should_defer_base_attr(&model, /* is_frontier */ false));
    }

    #[test]
    fn test_should_defer_base_attr_frontier_seed() {
        let seed = make_plain_seed("s");
        assert!(should_defer_base_attr(&seed, /* is_frontier */ true));
    }

    #[test]
    fn test_should_defer_base_attr_selected_seed_keeps_local() {
        let seed = make_plain_seed("s");
        assert!(!should_defer_base_attr(&seed, /* is_frontier */ false));
    }

    #[test]
    fn test_should_defer_base_attr_snapshot_always_rewrites() {
        let snapshot = make_plain_snapshot("sn");
        assert!(should_defer_base_attr(
            &snapshot, /* is_frontier */ true
        ));
        assert!(should_defer_base_attr(
            &snapshot, /* is_frontier */ false
        ));
    }

    #[test]
    fn test_should_defer_base_attr_function_always_rewrites() {
        // Regression against #9049: functions were only being updated at
        // the Jinja layer, not on `base_attr`.  Deferred functions must
        // always rewrite `base_attr` so the UDF registry built by
        // `task_runner::set_function_registry` matches the deferred FQN
        // that `{{ ref('fn') }}` emits into compiled SQL.
        let function = make_plain_function("uuidv7");
        assert!(should_defer_base_attr(
            &function, /* is_frontier */ true
        ));
        assert!(should_defer_base_attr(
            &function, /* is_frontier */ false
        ));
    }

    fn make_model_with_sa(name: &str, sa: StaticAnalysisKind) -> Arc<DbtModel> {
        Arc::new(DbtModel {
            __common_attr__: create_common_attr(name),
            __base_attr__: {
                let mut base = create_base_attr(name);
                base.static_analysis = sa.into();
                base
            },
            ..Default::default()
        })
    }

    /// Helper: build a NodeResolver with given compile_or_test flag,
    /// then call `set_defer_context` with the given nodes/sorted/frontier,
    /// and return it for testing `prefers_deferred`.
    fn build_resolver_with_defer_context(
        compile_or_test: bool,
        nodes: &Nodes,
        sorted_nodes: &[String],
        frontier_nodes: &BTreeSet<String>,
    ) -> NodeResolver {
        let mut resolver = NodeResolver {
            compile_or_test,
            ..Default::default()
        };

        let mut node_introspections = HashMap::new();
        let mut has_analyzed_schema = HashSet::new();
        let mut nodes_materialized = HashSet::new();

        for uid in sorted_nodes {
            let is_frontier_or_source = frontier_nodes.contains(uid) || uid.starts_with("source.");

            if let Some(node) = nodes.get_node(uid) {
                node_introspections.insert(uid.clone(), node.introspection());

                if !is_frontier_or_source && is_strict_static_analysis(*node.base().static_analysis)
                {
                    has_analyzed_schema.insert(uid.clone());
                }

                if !is_frontier_or_source {
                    nodes_materialized.insert(uid.clone());
                }
            }
        }

        resolver.set_defer_context(node_introspections, has_analyzed_schema, nodes_materialized);
        resolver
    }

    #[test]
    fn test_prefers_deferred_compile_introspection_none() {
        let nodes = Nodes::default();
        let sorted: Vec<String> = vec!["model.a".into()];
        let frontier = BTreeSet::new();

        let resolver = build_resolver_with_defer_context(true, &nodes, &sorted, &frontier);
        // Node with IntrospectionKind::None → nothing deferred
        // (node_introspections won't contain the current_node since nodes is empty,
        //  so it defaults to IntrospectionKind::None)
        assert!(!resolver.prefers_deferred("current_node", "model.a"));
        assert!(!resolver.prefers_deferred("current_node", "model.unknown"));
    }

    #[test]
    fn test_prefers_deferred_compile_strict_upstream() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.strict_model".into(),
            make_model_with_sa("strict_model", StaticAnalysisKind::Strict),
        );
        let sorted = vec!["model.strict_model".to_string()];
        let frontier = BTreeSet::new();

        let mut resolver = build_resolver_with_defer_context(true, &nodes, &sorted, &frontier);
        // Set current_node to have UpstreamSchema introspection
        resolver
            .node_introspections
            .insert("current_node".into(), IntrospectionKind::UpstreamSchema);

        // Strict, non-frontier → has analyzed schema → NOT deferred
        assert!(!resolver.prefers_deferred("current_node", "model.strict_model"));
    }

    #[test]
    fn test_prefers_deferred_compile_off_upstream() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.off_model".into(),
            make_model_with_sa("off_model", StaticAnalysisKind::Off),
        );
        let sorted = vec!["model.off_model".to_string()];
        let frontier = BTreeSet::new();

        let mut resolver = build_resolver_with_defer_context(true, &nodes, &sorted, &frontier);
        resolver
            .node_introspections
            .insert("current_node".into(), IntrospectionKind::UpstreamSchema);

        // Off → no analyzed schema → deferred
        assert!(resolver.prefers_deferred("current_node", "model.off_model"));
    }

    #[test]
    fn test_prefers_deferred_compile_baseline_upstream() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.baseline_model".into(),
            make_model_with_sa("baseline_model", StaticAnalysisKind::Baseline),
        );
        let sorted = vec!["model.baseline_model".to_string()];
        let frontier = BTreeSet::new();

        let mut resolver = build_resolver_with_defer_context(true, &nodes, &sorted, &frontier);
        resolver
            .node_introspections
            .insert("current_node".into(), IntrospectionKind::UpstreamSchema);

        // Baseline → no analyzed schema → deferred
        assert!(resolver.prefers_deferred("current_node", "model.baseline_model"));
    }

    #[test]
    fn test_prefers_deferred_compile_frontier_strict() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.frontier".into(),
            make_model_with_sa("frontier", StaticAnalysisKind::Strict),
        );
        let sorted = vec!["model.frontier".to_string()];
        let frontier: BTreeSet<String> = ["model.frontier".to_string()].into();

        let mut resolver = build_resolver_with_defer_context(true, &nodes, &sorted, &frontier);
        resolver
            .node_introspections
            .insert("current_node".into(), IntrospectionKind::UpstreamSchema);

        // Frontier even if strict → deferred
        assert!(resolver.prefers_deferred("current_node", "model.frontier"));
    }

    #[test]
    fn test_prefers_deferred_compile_source() {
        let nodes = Nodes::default();
        let sorted = vec!["source.my_source".to_string()];
        let frontier = BTreeSet::new();

        let mut resolver = build_resolver_with_defer_context(true, &nodes, &sorted, &frontier);
        resolver
            .node_introspections
            .insert("current_node".into(), IntrospectionKind::UpstreamSchema);

        // Source → deferred (not in has_analyzed_schema)
        assert!(resolver.prefers_deferred("current_node", "source.my_source"));
    }

    #[test]
    fn test_prefers_deferred_compile_unsafe_introspection() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.strict_model".into(),
            make_model_with_sa("strict_model", StaticAnalysisKind::Strict),
        );
        let sorted = vec!["model.strict_model".to_string()];
        let frontier = BTreeSet::new();

        let mut resolver = build_resolver_with_defer_context(true, &nodes, &sorted, &frontier);
        // Unsafe introspection → always defer everything
        resolver
            .node_introspections
            .insert("current_node".into(), IntrospectionKind::Execute);

        assert!(resolver.prefers_deferred("current_node", "model.strict_model"));
    }

    #[test]
    fn test_prefers_deferred_run_non_frontier() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.a".into(),
            make_model_with_sa("a", StaticAnalysisKind::Strict),
        );
        let sorted = vec!["model.a".to_string()];
        let frontier = BTreeSet::new();

        let resolver = build_resolver_with_defer_context(false, &nodes, &sorted, &frontier);
        // Run path, non-frontier → not deferred (in nodes_materialized)
        assert!(!resolver.prefers_deferred("any", "model.a"));
    }

    #[test]
    fn test_prefers_deferred_run_frontier() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.f".into(),
            make_model_with_sa("f", StaticAnalysisKind::Strict),
        );
        let sorted = vec!["model.f".to_string()];
        let frontier: BTreeSet<String> = ["model.f".to_string()].into();

        let resolver = build_resolver_with_defer_context(false, &nodes, &sorted, &frontier);
        // Run path, frontier → deferred (not in nodes_materialized)
        assert!(resolver.prefers_deferred("any", "model.f"));
    }

    #[test]
    fn test_prefers_deferred_run_source() {
        let nodes = Nodes::default();
        let sorted = vec!["source.s".to_string()];
        let frontier = BTreeSet::new();

        let resolver = build_resolver_with_defer_context(false, &nodes, &sorted, &frontier);
        // Run path, source → deferred (not in nodes_materialized)
        assert!(resolver.prefers_deferred("any", "source.s"));
    }

    #[test]
    fn test_collect_auto_defer_unselected_node_ids() {
        let frontier_id = "model.test.frontier_model".to_string();
        let ephemeral_id = "model.test.ephemeral_model".to_string();
        let selected_id = "model.test.selected_model".to_string();

        let make_model = |unique_id: &str, materialization: DbtMaterialization| -> Arc<DbtModel> {
            Arc::new(DbtModel {
                __common_attr__: create_common_attr(unique_id),
                __base_attr__: {
                    let mut base = create_base_attr(unique_id);
                    base.materialized = materialization;
                    base
                },
                ..Default::default()
            })
        };

        let mut nodes = Nodes::default();
        nodes.models.insert(
            frontier_id.clone(),
            make_model(&frontier_id, DbtMaterialization::Table),
        );
        nodes.models.insert(
            ephemeral_id.clone(),
            make_model(&ephemeral_id, DbtMaterialization::Ephemeral),
        );
        nodes.models.insert(
            selected_id.clone(),
            make_model(&selected_id, DbtMaterialization::Table),
        );

        let schedule = Schedule {
            frontier_nodes: BTreeSet::from([frontier_id.clone()]),
            selected_nodes: BTreeSet::from([selected_id]),
            ..Default::default()
        };

        let result = DeferState::collect_auto_defer_unselected_node_ids(&nodes, &schedule);

        assert_eq!(result, BTreeSet::from([frontier_id]));
    }
}
