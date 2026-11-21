//! Run-scoped schema fetching and request fields for column-aware reuse (`compare_selected_columns`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use arrow_schema::Schema;
use dbt_adapter::errors::{AdapterError, AdapterErrorKind, AdapterResult, Cancellable};
use dbt_adapter::relation::create_relation_from_node;
use dbt_adapter::sql_types::TypeOps;
use dbt_adapter_core::AdapterType;
use dbt_common::ErrorCode;
use dbt_common::tracing::dbt_emit::{emit_debug_log_message, emit_warn_log_message};
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_schemas::schemas::{
    DbtModel, DbtSeed, DbtSnapshot, DbtSource, DbtTest, InternalDbtNodeAttributes, Nodes,
};
use dbt_state::column_aware::{
    cached_table_schemas, columns_from_fetch, dependency_kinds, edge_tables,
};
use dbt_state::metadata_cache::{MetadataPrefetchGuard, RunCacheMetadataCache};
use dbt_state::node_state_config::resolve_compare_selected_columns;
use dbt_state::proto::query_cache::table_modified_info::DependencyKind;
use dbt_state::proto::query_cache::{ModelExecutionType, TableSchema};
use dbt_state::request_builder::execution_type_from_input;

use crate::context::TaskRunnerCtx;
use crate::run_cache::run_cache_dev_clone::relation_exists;
use crate::run_cache::run_cache_request::{
    is_microbatch_model, model_execution_type_input, model_is_view,
};
use crate::run_cache::run_cache_service::{
    SqlContextPurpose, effective_full_refresh, has_metadata_address,
    is_no_op_model_materialization, model_takes_full_refresh_bail, relation_for_node,
    should_honor_service_skip,
};

/// Whether `node` compares selected columns: its `state:` config, else the
/// service default. False without a service config.
pub(crate) fn compares_selected_columns(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
) -> bool {
    ctx.inner
        .run_cache_ctx
        .run_cache_service_config
        .as_ref()
        .is_some_and(|config| {
            resolve_compare_selected_columns(node, config.compare_selected_columns)
        })
}

/// Models that materialize a relation, and snapshots.
fn builds_own_relation(node: &dyn InternalDbtNodeAttributes) -> bool {
    match node.as_any().downcast_ref::<DbtModel>() {
        Some(model) => !is_no_op_model_materialization(model.materialized()),
        None => node.as_any().is::<DbtSnapshot>(),
    }
}

/// `semantic_fqn`s of the targets this run's selected models and snapshots
/// build. Selected seeds and tests are not included, so they stay edges.
/// Built once per run.
pub(crate) fn selected_target_fqns(ctx: &TaskRunnerCtx) -> &BTreeSet<String> {
    ctx.inner
        .run_cache_ctx
        .selected_target_fqns
        .get_or_init(|| {
            let adapter_type = ctx.default_adapter_type();
            let nodes = ctx.nodes();
            ctx.inner
                .runnable_set
                .iter()
                .filter_map(|unique_id| nodes.get_node(unique_id))
                .filter(|node| builds_own_relation(*node))
                .filter_map(|node| create_relation_from_node(adapter_type, node, None).ok())
                .map(|relation| relation.semantic_fqn())
                .collect()
        })
}
/// Maps the `semantic_fqn` of every source, seed, non-ephemeral model and
/// snapshot, including deferred relations, to its dependency kind. Conflicts
/// map to `Unknown`. Built once per run.
pub(crate) fn dependency_kinds_by_fqn(ctx: &TaskRunnerCtx) -> &HashMap<String, DependencyKind> {
    ctx.inner.run_cache_ctx.dependency_kinds.get_or_init(|| {
        let local_adapter_type = ctx.default_adapter_type();
        let defer_adapter_type = ctx.resolver_state.adapter_type;
        let nodes = ctx.nodes();
        let typed_nodes = nodes
            .sources
            .values()
            .map(|n| {
                (
                    n.as_ref() as &dyn InternalDbtNodeAttributes,
                    DependencyKind::Source,
                )
            })
            .chain(nodes.seeds.values().map(|n| {
                (
                    n.as_ref() as &dyn InternalDbtNodeAttributes,
                    DependencyKind::Seed,
                )
            }))
            .chain(
                nodes
                    .models
                    .values()
                    .filter(|m| !is_no_op_model_materialization(m.materialized()))
                    .map(|n| {
                        (
                            n.as_ref() as &dyn InternalDbtNodeAttributes,
                            DependencyKind::Model,
                        )
                    }),
            )
            .chain(nodes.snapshots.values().map(|n| {
                (
                    n.as_ref() as &dyn InternalDbtNodeAttributes,
                    DependencyKind::Model,
                )
            }));

        let mut entries = Vec::new();
        for (node, kind) in typed_nodes {
            if let Ok(relation) = create_relation_from_node(local_adapter_type, node, None) {
                entries.push((relation.semantic_fqn(), kind));
            }
            let unique_id = node.common().unique_id.as_str();
            if ctx.inner.runnable_set.contains(unique_id) {
                continue;
            }
            if let Some(deferred) = ctx.resolver_state.get_defer_node_by_id(unique_id)
                && let Ok(relation) = create_relation_from_node(defer_adapter_type, deferred, None)
            {
                entries.push((relation.semantic_fqn(), kind));
            }
        }
        dependency_kinds(entries)
    })
}

/// Relations to prefetch schemas for, keyed by `semantic_fqn`: for each
/// runnable node where `compares` holds, its dependencies this run doesn't
/// build (looking through ephemeral models), plus its own target if it is a
/// non-full-refresh incremental model. A warm-up only: misses are fetched on
/// demand.
pub(crate) fn collect_schema_prefetch_relations(
    adapter_type: AdapterType,
    runnable_set: &BTreeSet<String>,
    runtime_deps: &BTreeMap<String, BTreeSet<String>>,
    nodes: &Nodes,
    compares: impl Fn(&dyn InternalDbtNodeAttributes) -> bool,
    cli_full_refresh: bool,
) -> BTreeMap<String, Arc<dyn BaseRelation>> {
    let mut relations: BTreeMap<String, Arc<dyn BaseRelation>> = BTreeMap::new();
    let mut add = |node: &dyn InternalDbtNodeAttributes| {
        if let Ok(relation) = create_relation_from_node(adapter_type, node, None)
            && has_metadata_address(adapter_type, relation.as_ref())
        {
            relations
                .entry(relation.semantic_fqn())
                .or_insert_with(|| relation.into());
        }
    };

    for node_id in runnable_set {
        let Some(node) = nodes.get_node(node_id) else {
            continue;
        };
        if !compares(node) {
            continue;
        }
        if let Some(model) = node.as_any().downcast_ref::<DbtModel>() {
            let materialized = model.materialized();
            if is_no_op_model_materialization(materialized.clone()) {
                continue;
            }
            if matches!(materialized, DbtMaterialization::Incremental)
                && !effective_full_refresh(cli_full_refresh, model.deprecated_config.full_refresh)
            {
                add(node);
            }
        }

        let mut stack: Vec<&str> = runtime_deps
            .get(node_id)
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        let mut visited: HashSet<&str> = HashSet::new();
        while let Some(dep_id) = stack.pop() {
            if !visited.insert(dep_id) {
                continue;
            }
            let Some(dep) = nodes.get_node(dep_id) else {
                continue;
            };
            let dep_any = dep.as_any();
            if let Some(model) = dep_any.downcast_ref::<DbtModel>()
                && is_no_op_model_materialization(model.materialized())
            {
                stack.extend(
                    model
                        .__base_attr__
                        .depends_on
                        .nodes
                        .iter()
                        .map(String::as_str),
                );
                continue;
            }
            let is_edge = dep_any.is::<DbtSource>()
                || dep_any.is::<DbtSeed>()
                || ((dep_any.is::<DbtModel>() || dep_any.is::<DbtSnapshot>())
                    && !runnable_set.contains(dep_id));
            if is_edge {
                add(dep);
            }
        }
    }

    relations
}

/// Why a whole `list_relations_schemas` batch failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SchemaFetchError {
    /// The run was cancelled; not worth a warning.
    Cancelled,
    /// Any other batch failure (connection, permissions, ...).
    Failed(String),
}

impl From<Cancellable<AdapterError>> for SchemaFetchError {
    fn from(err: Cancellable<AdapterError>) -> Self {
        match err {
            Cancellable::Cancelled => Self::Cancelled,
            // A cancelled query can surface as an error of kind `Cancelled`.
            Cancellable::Error(err) if err.kind() == AdapterErrorKind::Cancelled => Self::Cancelled,
            Cancellable::Error(err) => Self::Failed(err.to_string()),
        }
    }
}

impl std::fmt::Display for SchemaFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Failed(reason) => f.write_str(reason),
        }
    }
}

/// Fetch rounds per `get_schemas_with` call: the first fetch plus one retry
/// of tables invalidated while their fetch ran.
const MAX_SCHEMA_FETCH_ROUNDS: usize = 2;

/// Fetches the schemas of `relations` not yet cached in `run_cache_metadata`,
/// in one batched `list_relations_schemas` call per round. Tables missing from
/// the result or with unrenderable columns are cached as absent. A table whose
/// own fetch errored is not cached, and neither is anything from a
/// whole-batch failure (which is returned to the caller), so a later request
/// retries: the cache can outlive the invocation. A table
/// invalidated while its fetch runs is not cached from that fetch.
pub(crate) async fn get_schemas(
    ctx: &TaskRunnerCtx,
    relations: &BTreeMap<String, Arc<dyn BaseRelation>>,
) -> Result<(), SchemaFetchError> {
    let cache = &ctx.inner.run_cache_ctx.run_cache_metadata;
    if relations
        .keys()
        .all(|fqn| cache.table_schema(fqn).is_some())
    {
        return Ok(());
    }
    let Some(adapter) = ctx.env.get_adapter_ref() else {
        return Ok(());
    };
    let Some(metadata_adapter) = adapter.metadata_adapter() else {
        return Ok(());
    };
    let metadata_adapter = metadata_adapter.as_ref();
    get_schemas_with(
        cache,
        adapter.engine().type_ops().as_ref(),
        relations,
        |batch| async move {
            metadata_adapter
                .list_relations_schemas(None, None, &batch, None, adapter.cancellation_token())
                .await
                .map_err(SchemaFetchError::from)
        },
    )
    .await
}

/// `get_schemas` with the warehouse fetch passed in. Coalesces with concurrent
/// callers the same way the bulk last-modified prefetch does: it takes each
/// uncached table's per-relation prefetch lock in name order (so overlapping
/// callers can't deadlock), re-checks the cache once it holds them all, and
/// fetches only what is still uncached. A table another caller is fetching is
/// therefore waited on, not fetched again. A caller holds the locks it has
/// while it waits for the rest, so a call overlapping a slow fetch waits for
/// it even for tables that fetch doesn't cover.
pub(crate) async fn get_schemas_with<F, Fut>(
    cache: &RunCacheMetadataCache,
    type_ops: &dyn TypeOps,
    relations: &BTreeMap<String, Arc<dyn BaseRelation>>,
    fetch: F,
) -> Result<(), SchemaFetchError>
where
    F: Fn(Vec<Arc<dyn BaseRelation>>) -> Fut,
    Fut: Future<Output = Result<HashMap<String, AdapterResult<Arc<Schema>>>, SchemaFetchError>>,
{
    // Tables whose own fetch errored: left uncached, and not retried here.
    let mut errored: BTreeSet<&str> = BTreeSet::new();
    for _ in 0..MAX_SCHEMA_FETCH_ROUNDS {
        // `BTreeMap` order is the lock order.
        let mut guards: BTreeMap<&str, MetadataPrefetchGuard<'_>> = relations
            .keys()
            .map(String::as_str)
            .filter(|fqn| !errored.contains(fqn) && cache.table_schema(fqn).is_none())
            .map(|fqn| (fqn, cache.begin_table_schema_prefetch(fqn)))
            .collect();
        if guards.is_empty() {
            break;
        }
        for guard in guards.values_mut() {
            guard.acquire_for_fetch().await;
        }
        // Another caller may have cached some while this one waited.
        guards.retain(|fqn, _| cache.table_schema(fqn).is_none());
        if guards.is_empty() {
            break;
        }

        let batch: Vec<Arc<dyn BaseRelation>> = guards
            .keys()
            .filter_map(|fqn| relations.get(*fqn).cloned())
            .collect();
        let results = match fetch(batch).await {
            Ok(results) => results,
            Err(err) => {
                emit_debug_log_message(format!(
                    "dbt State: schema fetch failed for {} table(s): {err}; sending no schemas for them",
                    guards.len()
                ));
                return Err(err);
            }
        };
        let mut absent = Vec::new();
        let mut invalidated = false;
        for (fqn, guard) in &guards {
            let Some(columns) = columns_from_fetch(type_ops, results.get(*fqn)) else {
                errored.insert(*fqn);
                continue;
            };
            let is_absent = columns.is_none();
            if !guard.insert_table_schema(*fqn, columns) {
                invalidated = true;
            } else if is_absent {
                absent.push(*fqn);
            }
        }
        if !absent.is_empty() {
            emit_debug_log_message(format!(
                "dbt State: schema(s) for table(s) {} not found or column types could not be rendered",
                absent.join(", ")
            ));
        }
        if !invalidated {
            break;
        }
    }
    if !errored.is_empty() {
        emit_debug_log_message(format!(
            "dbt State: schema fetch failed for table(s) {}; sending no schemas for them",
            errored.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    Ok(())
}

/// Whether `node` will send `table_schemas`: it compares selected columns and
/// submits non-view SQL for a decision.
fn wants_table_schemas(ctx: &TaskRunnerCtx, node: &dyn InternalDbtNodeAttributes) -> bool {
    let any = node.as_any();
    let submits_non_view_sql = match any.downcast_ref::<DbtModel>() {
        Some(model) => {
            !model_is_view(model, &ctx.inner.materialization_resolver)
                && !model_takes_full_refresh_bail(ctx.inner.arg.full_refresh, model)
        }
        None => any.is::<DbtSnapshot>() || any.is::<DbtTest>(),
    };
    submits_non_view_sql && compares_selected_columns(ctx, node)
}

/// Starts the background schema prefetch on its own handle, so it never
/// delays `is_prefetch_ready`. A no-op unless a runnable node will send
/// schemas and the run asks the service for decisions.
pub(crate) async fn start_prefetch_schemas(ctx: &TaskRunnerCtx) {
    let run_cache_ctx = &ctx.inner.run_cache_ctx;
    let submits_for_decisions = should_honor_service_skip(ctx)
        && run_cache_ctx
            .run_cache_service_client
            .as_ref()
            .is_some_and(|client| !client.is_disabled());
    if !submits_for_decisions {
        return;
    }
    let nodes = ctx.nodes();
    let wants = |node: &dyn InternalDbtNodeAttributes| wants_table_schemas(ctx, node);
    if !ctx
        .inner
        .runnable_set
        .iter()
        .filter_map(|unique_id| nodes.get_node(unique_id))
        .any(wants)
    {
        return;
    }

    let prefetch = run_cache_ctx.schema_prefetch.clone();
    prefetch.mark_started();
    let relations = collect_schema_prefetch_relations(
        ctx.default_adapter_type(),
        &ctx.inner.runnable_set,
        &ctx.inner.runtime_deps,
        nodes,
        wants,
        ctx.inner.arg.full_refresh,
    );
    if relations.is_empty() {
        prefetch.mark_done();
        return;
    }
    emit_debug_log_message(format!(
        "dbt State: prefetching schemas for {} edge table(s)",
        relations.len()
    ));

    let prefetch_ctx = ctx.clone();
    let handle = dbt_common::tracing::spawn_traced(async move {
        match get_schemas(&prefetch_ctx, &relations).await {
            Ok(()) | Err(SchemaFetchError::Cancelled) => {}
            Err(err) => emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State schema prefetch failed: {err}; column-aware reuse will fetch \
                     schemas on demand"
                ),
            ),
        }
        prefetch_ctx.inner.run_cache_ctx.schema_prefetch.mark_done();
    });
    prefetch.set_handle(handle).await;
}

/// Waits for the schema prefetch, if it was started. A join error is logged,
/// not returned.
pub(crate) async fn await_prefetch_schemas(ctx: &TaskRunnerCtx) {
    let prefetch = &ctx.inner.run_cache_ctx.schema_prefetch;
    if let Err(err) = prefetch.join().await {
        emit_warn_log_message(
            ErrorCode::StateServiceWarn,
            format!("dbt State schema prefetch task did not complete cleanly: {err}; continuing"),
        );
    }
    prefetch.mark_done();
}

/// Whether `target_table_missing` can be true for `node`, so its target is
/// worth looking up: not for `Full` builds or microbatch models.
fn target_may_be_reported_missing(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
) -> bool {
    let Some(model) = node.as_any().downcast_ref::<DbtModel>() else {
        return true;
    };
    if is_microbatch_model(model) {
        return false;
    }
    let full_refresh = effective_full_refresh(
        ctx.inner.arg.full_refresh,
        model.deprecated_config.full_refresh,
    );
    execution_type_from_input(&model_execution_type_input(
        model,
        full_refresh,
        &ctx.inner.materialization_resolver,
    ))
    .map_or(true, |execution_type| {
        execution_type != ModelExecutionType::Full
    })
}

/// The column-aware fields of one SQL submit.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct ColumnAwareFields {
    pub compare_selected_columns: bool,
    pub table_schemas: Vec<TableSchema>,
    pub target_exists: Option<bool>,
}

/// Builds the column-aware fields for one SQL submit.
///
/// - Views and nodes without the flag get the defaults.
/// - Speculative requests, execution records and write-only runs send the flag
///   only: no schemas and no target lookup.
/// - Otherwise it sends the schemas of the edge tables (`seen_tables` minus
///   this run's targets, keeping the node's own) and whether the target exists.
///
/// `relations` are the relations the SQL reads, used to fetch uncached
/// schemas. Fails open: tables without a schema are left out.
pub(crate) async fn collect_column_aware_fields(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    is_view: bool,
    purpose: SqlContextPurpose,
    seen_tables: &BTreeSet<String>,
    relations: &BTreeMap<String, Arc<dyn BaseRelation>>,
) -> ColumnAwareFields {
    if is_view || !compares_selected_columns(ctx, node) {
        return ColumnAwareFields::default();
    }
    let flag_only = ColumnAwareFields {
        compare_selected_columns: true,
        ..ColumnAwareFields::default()
    };
    // Records keep the flag so their tables still carry dependency kinds.
    if purpose != SqlContextPurpose::Decision || !should_honor_service_skip(ctx) {
        return flag_only;
    }

    // Data tests submit with no target table (see `build_test_sql_request`).
    let own_target = if node.as_any().is::<DbtTest>() {
        None
    } else {
        relation_for_node(node).ok()
    };
    let own_fqn = own_target.as_ref().map(|(fqn, _)| fqn.as_str());

    let edges = edge_tables(
        seen_tables,
        selected_target_fqns(ctx),
        own_fqn.unwrap_or_default(),
    );
    let mut table_schemas = Vec::new();
    if !edges.is_empty() {
        await_prefetch_schemas(ctx).await;
        let adapter_type = ctx.default_adapter_type();
        let to_fetch: BTreeMap<String, Arc<dyn BaseRelation>> = edges
            .iter()
            .filter_map(|fqn| {
                let relation = relations.get(fqn).or_else(|| {
                    own_target
                        .as_ref()
                        .filter(|(own, _)| own == fqn)
                        .map(|(_, relation)| relation)
                })?;
                has_metadata_address(adapter_type, relation.as_ref())
                    .then(|| (fqn.clone(), Arc::clone(relation)))
            })
            .collect();
        // Fail-open: a failed fetch just leaves its tables out.
        let _ = get_schemas(ctx, &to_fetch).await;
        table_schemas = cached_table_schemas(&ctx.inner.run_cache_ctx.run_cache_metadata, &edges);
    }

    let target_exists = match own_target {
        Some((fqn, relation)) if target_may_be_reported_missing(ctx, node) => {
            relation_exists(ctx, &fqn, relation).await
        }
        _ => None,
    };

    if !table_schemas.is_empty() {
        let columns: usize = table_schemas.iter().map(|t| t.columns.len()).sum();
        emit_debug_log_message(format!(
            "dbt State: sending schemas for {} table(s) ({columns} column(s)) with {}",
            table_schemas.len(),
            node.unique_id()
        ));
    }

    ColumnAwareFields {
        compare_selected_columns: true,
        table_schemas,
        target_exists,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_cache::run_cache_service::tests::{
        EmptySourcesExtractor, RecordingRunCacheClient, test_task_runner_ctx_with_nodes,
    };
    use arrow_schema::{DataType, Field};
    use dbt_adapter::sql_types::DefaultTypeOps;
    use dbt_common::io_args::RunCacheMode;
    use dbt_schemas::schemas::common::ResolvedQuoting;
    use dbt_schemas::schemas::properties::{DataTestState, ModelState};
    use dbt_state::proto::query_cache::ColumnInfo;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    const M1: &str = "model.pkg.m1";
    const M2: &str = "model.pkg.m2";
    const U: &str = "model.pkg.u";
    const E: &str = "model.pkg.e";
    const S: &str = "source.pkg.raw.s";
    const SD: &str = "seed.pkg.sd";
    const SN: &str = "snapshot.pkg.sn";

    fn model(unique_id: &str, alias: &str, mat: DbtMaterialization) -> Arc<DbtModel> {
        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = unique_id.to_string();
        model.__common_attr__.language = Some("sql".to_string());
        model.__base_attr__.database = "db".to_string();
        model.__base_attr__.schema = "analytics".to_string();
        model.__base_attr__.alias = alias.to_string();
        model.__base_attr__.materialized = mat;
        Arc::new(model)
    }

    fn source(unique_id: &str, alias: &str, quoting: ResolvedQuoting) -> Arc<DbtSource> {
        let mut source = DbtSource::default();
        source.__common_attr__.unique_id = unique_id.to_string();
        source.__base_attr__.database = "db".to_string();
        source.__base_attr__.schema = "raw".to_string();
        source.__base_attr__.alias = alias.to_string();
        source.__base_attr__.quoting = quoting;
        Arc::new(source)
    }

    fn seed(unique_id: &str, alias: &str) -> Arc<DbtSeed> {
        let mut seed = DbtSeed::default();
        seed.__common_attr__.unique_id = unique_id.to_string();
        seed.__base_attr__.database = "db".to_string();
        seed.__base_attr__.schema = "seeds".to_string();
        seed.__base_attr__.alias = alias.to_string();
        seed.__base_attr__.materialized = DbtMaterialization::Seed;
        Arc::new(seed)
    }

    fn snapshot(unique_id: &str, alias: &str) -> Arc<DbtSnapshot> {
        let mut snapshot = DbtSnapshot::default();
        snapshot.__common_attr__.unique_id = unique_id.to_string();
        snapshot.__base_attr__.database = "db".to_string();
        snapshot.__base_attr__.schema = "snapshots".to_string();
        snapshot.__base_attr__.alias = alias.to_string();
        snapshot.__base_attr__.materialized = DbtMaterialization::Snapshot;
        Arc::new(snapshot)
    }

    /// Selected incremental `m1` reads source `s`, unselected model `u`,
    /// selected table model `m2` and selected seed `sd`.
    fn prefetch_nodes() -> Nodes {
        let mut nodes = Nodes::default();
        for m in [
            model(M1, "m1", DbtMaterialization::Incremental),
            model(M2, "m2", DbtMaterialization::Table),
            model(U, "u", DbtMaterialization::Table),
        ] {
            nodes.models.insert(m.__common_attr__.unique_id.clone(), m);
        }
        nodes
            .sources
            .insert(S.to_string(), source(S, "s", ResolvedQuoting::default()));
        nodes.seeds.insert(SD.to_string(), seed(SD, "sd"));
        nodes
    }

    fn ids(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn prefetch_deps() -> BTreeMap<String, BTreeSet<String>> {
        BTreeMap::from([(M1.to_string(), ids(&[S, U, M2, SD]))])
    }

    fn fqn(nodes: &Nodes, unique_id: &str) -> String {
        let node = nodes.get_node(unique_id).expect("node");
        create_relation_from_node(AdapterType::Snowflake, node, None)
            .expect("relation")
            .semantic_fqn()
    }

    fn fqns(nodes: &Nodes, unique_ids: &[&str]) -> BTreeSet<String> {
        unique_ids.iter().map(|id| fqn(nodes, id)).collect()
    }

    fn keys(relations: &BTreeMap<String, Arc<dyn BaseRelation>>) -> BTreeSet<String> {
        relations.keys().cloned().collect()
    }

    fn ctx_with(nodes: Nodes, selected: &[&str]) -> TaskRunnerCtx {
        test_task_runner_ctx_with_nodes(
            None,
            RunCacheMode::ReadWrite,
            false,
            Arc::new(EmptySourcesExtractor),
            nodes,
            ids(selected),
        )
    }

    #[test]
    fn prefetch_relations_cover_unselected_deps_and_incremental_targets() {
        let nodes = prefetch_nodes();
        let relations = collect_schema_prefetch_relations(
            AdapterType::Snowflake,
            &ids(&[M1, M2, SD]),
            &prefetch_deps(),
            &nodes,
            |_| true,
            false,
        );
        assert_eq!(keys(&relations), fqns(&nodes, &[S, U, SD, M1]));
    }

    #[test]
    fn prefetch_relations_skip_nodes_without_flag() {
        let nodes = prefetch_nodes();
        let relations = collect_schema_prefetch_relations(
            AdapterType::Snowflake,
            &ids(&[M1, M2, SD]),
            &prefetch_deps(),
            &nodes,
            |_| false,
            false,
        );
        assert!(relations.is_empty());
    }

    #[test]
    fn prefetch_relations_skip_full_refresh_incremental_target() {
        let nodes = prefetch_nodes();
        let relations = collect_schema_prefetch_relations(
            AdapterType::Snowflake,
            &ids(&[M1, M2, SD]),
            &prefetch_deps(),
            &nodes,
            |_| true,
            true,
        );
        assert_eq!(keys(&relations), fqns(&nodes, &[S, U, SD]));
    }

    #[test]
    fn prefetch_relations_resolve_through_ephemeral_deps() {
        let mut nodes = prefetch_nodes();
        let mut ephemeral = DbtModel::clone(&model(E, "e", DbtMaterialization::Ephemeral));
        ephemeral.__base_attr__.depends_on.nodes = vec![U.to_string()];
        nodes.models.insert(E.to_string(), Arc::new(ephemeral));
        let relations = collect_schema_prefetch_relations(
            AdapterType::Snowflake,
            &ids(&[M2]),
            &BTreeMap::from([(M2.to_string(), ids(&[E]))]),
            &nodes,
            |_| true,
            false,
        );
        assert_eq!(keys(&relations), fqns(&nodes, &[U]));
    }

    #[test]
    fn prefetch_relations_are_keyed_by_semantic_fqn() {
        let quoted = source(
            S,
            "events",
            ResolvedQuoting {
                database: false,
                schema: false,
                identifier: true,
            },
        );
        let expected = create_relation_from_node(AdapterType::Snowflake, quoted.as_ref(), None)
            .expect("relation")
            .semantic_fqn();
        let mut nodes = Nodes::default();
        nodes.sources.insert(S.to_string(), quoted);
        let m2 = model(M2, "m2", DbtMaterialization::Table);
        nodes.models.insert(M2.to_string(), m2);
        let relations = collect_schema_prefetch_relations(
            AdapterType::Snowflake,
            &ids(&[M2]),
            &BTreeMap::from([(M2.to_string(), ids(&[S]))]),
            &nodes,
            |_| true,
            false,
        );
        assert_eq!(keys(&relations), BTreeSet::from([expected]));
    }

    #[test]
    fn selected_target_fqns_excludes_seeds() {
        let mut nodes = prefetch_nodes();
        nodes
            .models
            .insert(E.to_string(), model(E, "e", DbtMaterialization::Ephemeral));
        nodes.snapshots.insert(SN.to_string(), snapshot(SN, "sn"));
        let expected = fqns(&nodes, &[M1, SN]);
        let ctx = ctx_with(nodes, &[M1, E, SD, SN]);
        assert_eq!(selected_target_fqns(&ctx), &expected);
    }

    #[test]
    fn dependency_kinds_by_fqn_maps_node_types() {
        let mut nodes = prefetch_nodes();
        nodes
            .models
            .insert(E.to_string(), model(E, "e", DbtMaterialization::Ephemeral));
        nodes.snapshots.insert(SN.to_string(), snapshot(SN, "sn"));
        let ephemeral_fqn = fqn(&nodes, E);
        let (s, sd, u, sn) = (
            fqn(&nodes, S),
            fqn(&nodes, SD),
            fqn(&nodes, U),
            fqn(&nodes, SN),
        );
        let ctx = ctx_with(nodes, &[M1]);
        let kinds = dependency_kinds_by_fqn(&ctx);
        assert_eq!(kinds.get(&s), Some(&DependencyKind::Source));
        assert_eq!(kinds.get(&sd), Some(&DependencyKind::Seed));
        assert_eq!(kinds.get(&u), Some(&DependencyKind::Model));
        assert_eq!(kinds.get(&sn), Some(&DependencyKind::Model));
        assert!(!kinds.contains_key(&ephemeral_fqn));
    }

    #[test]
    fn schema_fetch_error_separates_cancellation_from_failure() {
        use dbt_adapter::errors::{AdapterError, AdapterErrorKind};
        assert_eq!(
            SchemaFetchError::from(Cancellable::Cancelled),
            SchemaFetchError::Cancelled
        );
        assert_eq!(
            SchemaFetchError::from(Cancellable::Error(AdapterError::new(
                AdapterErrorKind::Cancelled,
                "query cancelled",
            ))),
            SchemaFetchError::Cancelled
        );
        let failed = SchemaFetchError::from(Cancellable::Error(AdapterError::new(
            AdapterErrorKind::Internal,
            "boom",
        )));
        assert!(
            matches!(&failed, SchemaFetchError::Failed(reason) if reason.contains("boom")),
            "{failed:?}"
        );
        assert!(failed.to_string().contains("boom"));
    }

    #[dbt_runtime::test]
    async fn get_schemas_without_adapter_is_ok_and_caches_nothing() {
        let nodes = prefetch_nodes();
        let relations = collect_schema_prefetch_relations(
            AdapterType::Snowflake,
            &ids(&[M1, M2, SD]),
            &prefetch_deps(),
            &nodes,
            |_| true,
            false,
        );
        assert!(!relations.is_empty());
        let ctx = ctx_with(nodes, &[M1, M2, SD]);
        assert_eq!(get_schemas(&ctx, &relations).await, Ok(()));
        let cache = &ctx.inner.run_cache_ctx.run_cache_metadata;
        assert!(relations.keys().all(|k| cache.table_schema(k).is_none()));
    }

    type FetchResult = Result<HashMap<String, AdapterResult<Arc<Schema>>>, SchemaFetchError>;

    const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    /// `{s}`, keyed by `semantic_fqn`.
    fn source_relations() -> BTreeMap<String, Arc<dyn BaseRelation>> {
        let nodes = prefetch_nodes();
        let node = nodes.get_node(S).expect("node");
        let relation: Arc<dyn BaseRelation> =
            create_relation_from_node(AdapterType::Snowflake, node, None)
                .expect("relation")
                .into();
        BTreeMap::from([(relation.semantic_fqn(), relation)])
    }

    fn snowflake_ops() -> DefaultTypeOps {
        DefaultTypeOps::new(AdapterType::Snowflake)
    }

    /// A one-column `Ok` schema for each relation in `batch`.
    fn one_column_schemas(batch: &[Arc<dyn BaseRelation>]) -> FetchResult {
        Ok(batch
            .iter()
            .map(|relation| {
                (
                    relation.semantic_fqn(),
                    Ok(Arc::new(Schema::new(vec![Field::new(
                        "ID",
                        DataType::Int64,
                        true,
                    )]))),
                )
            })
            .collect())
    }

    /// Polls `fut` while yielding a few times; panics if it completes.
    async fn assert_stays_pending<F: Future>(fut: std::pin::Pin<&mut F>) {
        tokio::select! {
            biased;
            _ = fut => panic!("finished while another caller's fetch was held"),
            _ = async {
                for _ in 0..5 {
                    tokio::task::yield_now().await;
                }
            } => {}
        }
    }

    #[dbt_runtime::test]
    async fn concurrent_get_schemas_issue_one_fetch() {
        let cache = RunCacheMetadataCache::new();
        let ops = snowflake_ops();
        let relations = source_relations();
        let s = relations.keys().next().unwrap().clone();
        let calls = AtomicUsize::new(0);
        let started = Notify::new();
        let release = Notify::new();
        let fetch = |batch: Vec<Arc<dyn BaseRelation>>| {
            let (calls, started, release) = (&calls, &started, &release);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                started.notify_one();
                release.notified().await;
                one_column_schemas(&batch)
            }
        };

        let a = get_schemas_with(&cache, &ops, &relations, fetch);
        let b = async {
            started.notified().await;
            let mut b = std::pin::pin!(get_schemas_with(&cache, &ops, &relations, fetch));
            assert_stays_pending(b.as_mut()).await;
            release.notify_waiters();
            b.await
        };
        let (a, b) = tokio::time::timeout(TEST_TIMEOUT, async { tokio::join!(a, b) })
            .await
            .expect("no hang");
        assert_eq!(a, Ok(()));
        assert_eq!(b, Ok(()));
        assert!(matches!(cache.table_schema(&s), Some(Some(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[dbt_runtime::test]
    async fn get_schemas_refetches_after_invalidation_during_fetch() {
        let cache = RunCacheMetadataCache::new();
        let ops = snowflake_ops();
        let relations = source_relations();
        let s = relations.keys().next().unwrap().clone();
        let calls = AtomicUsize::new(0);
        let fetch = |batch: Vec<Arc<dyn BaseRelation>>| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                cache.invalidate_relation_metadata(&s);
            }
            async move { one_column_schemas(&batch) }
        };
        let got = tokio::time::timeout(
            TEST_TIMEOUT,
            get_schemas_with(&cache, &ops, &relations, fetch),
        )
        .await
        .expect("no hang");
        assert_eq!(got, Ok(()));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(matches!(cache.table_schema(&s), Some(Some(_))));
    }

    #[dbt_runtime::test]
    async fn get_schemas_gives_up_on_a_table_invalidated_every_round() {
        let cache = RunCacheMetadataCache::new();
        let ops = snowflake_ops();
        let relations = source_relations();
        let s = relations.keys().next().unwrap().clone();
        let calls = AtomicUsize::new(0);
        let fetch = |batch: Vec<Arc<dyn BaseRelation>>| {
            calls.fetch_add(1, Ordering::SeqCst);
            cache.invalidate_relation_metadata(&s);
            async move { one_column_schemas(&batch) }
        };
        let got = tokio::time::timeout(
            TEST_TIMEOUT,
            get_schemas_with(&cache, &ops, &relations, fetch),
        )
        .await
        .expect("no hang");
        assert_eq!(got, Ok(()));
        assert_eq!(calls.load(Ordering::SeqCst), MAX_SCHEMA_FETCH_ROUNDS);
        assert!(cache.table_schema(&s).is_none());
    }

    #[dbt_runtime::test]
    async fn get_schemas_waiter_refetches_after_fetcher_fails() {
        let cache = RunCacheMetadataCache::new();
        let ops = snowflake_ops();
        let relations = source_relations();
        let s = relations.keys().next().unwrap().clone();
        let calls = AtomicUsize::new(0);
        let started = Notify::new();
        let release = Notify::new();
        let fetch = |batch: Vec<Arc<dyn BaseRelation>>| {
            let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
            let (started, release) = (&started, &release);
            async move {
                if first {
                    started.notify_one();
                    release.notified().await;
                    return Err(SchemaFetchError::Failed("boom".into()));
                }
                one_column_schemas(&batch)
            }
        };

        let a = get_schemas_with(&cache, &ops, &relations, fetch);
        let b = async {
            started.notified().await;
            let mut b = std::pin::pin!(get_schemas_with(&cache, &ops, &relations, fetch));
            assert_stays_pending(b.as_mut()).await;
            release.notify_waiters();
            b.await
        };
        let (a, b) = tokio::time::timeout(TEST_TIMEOUT, async { tokio::join!(a, b) })
            .await
            .expect("no hang");
        assert_eq!(a, Err(SchemaFetchError::Failed("boom".into())));
        assert_eq!(b, Ok(()));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(matches!(cache.table_schema(&s), Some(Some(_))));
    }

    #[dbt_runtime::test]
    async fn get_schemas_waiter_wakes_when_fetcher_is_aborted() {
        let cache = Arc::new(RunCacheMetadataCache::new());
        let relations = source_relations();
        let s = relations.keys().next().unwrap().clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());

        let a_handle = tokio::spawn({
            let cache = Arc::clone(&cache);
            let relations = relations.clone();
            let calls = Arc::clone(&calls);
            let started = Arc::clone(&started);
            let ops = snowflake_ops();
            async move {
                let fetch = |_batch: Vec<Arc<dyn BaseRelation>>| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    let started = Arc::clone(&started);
                    async move {
                        started.notify_one();
                        std::future::pending::<FetchResult>().await
                    }
                };
                get_schemas_with(&cache, &ops, &relations, fetch).await
            }
        });
        started.notified().await;

        let ops = snowflake_ops();
        let fetch = |batch: Vec<Arc<dyn BaseRelation>>| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { one_column_schemas(&batch) }
        };
        let mut b = std::pin::pin!(get_schemas_with(&cache, &ops, &relations, fetch));
        assert_stays_pending(b.as_mut()).await;
        a_handle.abort();
        let got = tokio::time::timeout(TEST_TIMEOUT, b)
            .await
            .expect("no hang");
        assert_eq!(got, Ok(()));
        assert!(a_handle.await.unwrap_err().is_cancelled());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(matches!(cache.table_schema(&s), Some(Some(_))));
    }

    #[dbt_runtime::test]
    async fn get_schemas_all_cached_issues_no_fetch() {
        let cache = RunCacheMetadataCache::new();
        let ops = snowflake_ops();
        let relations = source_relations();
        let s = relations.keys().next().unwrap().clone();
        cache.insert_table_schema(&s, None);
        let calls = AtomicUsize::new(0);
        let fetch = |batch: Vec<Arc<dyn BaseRelation>>| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { one_column_schemas(&batch) }
        };
        let got = tokio::time::timeout(
            TEST_TIMEOUT,
            get_schemas_with(&cache, &ops, &relations, fetch),
        )
        .await
        .expect("no hang");
        assert_eq!(got, Ok(()));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// Relations for `unique_ids` from `prefetch_nodes`, keyed by `semantic_fqn`.
    fn relations_for(unique_ids: &[&str]) -> BTreeMap<String, Arc<dyn BaseRelation>> {
        let nodes = prefetch_nodes();
        unique_ids
            .iter()
            .map(|id| {
                let relation: Arc<dyn BaseRelation> = create_relation_from_node(
                    AdapterType::Snowflake,
                    nodes.get_node(id).expect("node"),
                    None,
                )
                .expect("relation")
                .into();
                (relation.semantic_fqn(), relation)
            })
            .collect()
    }

    /// A waiter's freshness snapshot is taken once it holds the lock, so an
    /// invalidation while it waits doesn't throw away its (fresh) fetch.
    #[dbt_runtime::test]
    async fn get_schemas_waiter_keeps_fetch_after_invalidation_while_waiting() {
        let cache = RunCacheMetadataCache::new();
        let ops = snowflake_ops();
        let relations = source_relations();
        let s = relations.keys().next().unwrap().clone();
        let calls = AtomicUsize::new(0);
        let started = Notify::new();
        let release = Notify::new();
        let fetch = |batch: Vec<Arc<dyn BaseRelation>>| {
            let first = calls.fetch_add(1, Ordering::SeqCst) == 0;
            let (started, release) = (&started, &release);
            async move {
                if first {
                    started.notify_one();
                    release.notified().await;
                }
                one_column_schemas(&batch)
            }
        };

        let a = get_schemas_with(&cache, &ops, &relations, fetch);
        let b = async {
            started.notified().await;
            let mut b = std::pin::pin!(get_schemas_with(&cache, &ops, &relations, fetch));
            assert_stays_pending(b.as_mut()).await;
            // The table rebuilds during `a`'s fetch, while `b` waits.
            cache.invalidate_relation_metadata(&s);
            release.notify_waiters();
            b.await
        };
        let (a, b) = tokio::time::timeout(TEST_TIMEOUT, async { tokio::join!(a, b) })
            .await
            .expect("no hang");
        assert_eq!(a, Ok(()));
        assert_eq!(b, Ok(()));
        assert!(matches!(cache.table_schema(&s), Some(Some(_))));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "a's stale fetch is dropped; b's fetch is kept and a reuses it"
        );
    }

    /// Overlapping multi-table callers don't deadlock, and the second fetches
    /// only the table the first didn't cover.
    #[dbt_runtime::test]
    async fn overlapping_get_schemas_fetch_only_uncovered_tables() {
        let cache = RunCacheMetadataCache::new();
        let ops = snowflake_ops();
        let first = relations_for(&[S, U]);
        let second = relations_for(&[U, SD]);
        let batches = std::sync::Mutex::new(Vec::<BTreeSet<String>>::new());
        let started = Notify::new();
        let release = Notify::new();
        let fetch = |batch: Vec<Arc<dyn BaseRelation>>| {
            let first_call = {
                let mut batches = batches.lock().unwrap();
                batches.push(batch.iter().map(|r| r.semantic_fqn()).collect());
                batches.len() == 1
            };
            let (started, release) = (&started, &release);
            async move {
                if first_call {
                    started.notify_one();
                    release.notified().await;
                }
                one_column_schemas(&batch)
            }
        };

        let a = get_schemas_with(&cache, &ops, &first, fetch);
        let b = async {
            started.notified().await;
            let mut b = std::pin::pin!(get_schemas_with(&cache, &ops, &second, fetch));
            assert_stays_pending(b.as_mut()).await;
            release.notify_waiters();
            b.await
        };
        let (a, b) = tokio::time::timeout(TEST_TIMEOUT, async { tokio::join!(a, b) })
            .await
            .expect("no hang");
        assert_eq!(a, Ok(()));
        assert_eq!(b, Ok(()));
        let batches = batches.into_inner().unwrap();
        assert_eq!(
            batches,
            vec![keys(&first), keys(&relations_for(&[SD]))],
            "the second caller skips the table the first fetched"
        );
        for fqn in first.keys().chain(second.keys()) {
            assert!(matches!(cache.table_schema(fqn), Some(Some(_))), "{fqn}");
        }
    }

    /// A table whose own fetch errors is left uncached (a later call retries)
    /// and isn't retried within the call; the rest of the batch is cached.
    #[dbt_runtime::test]
    async fn get_schemas_leaves_per_table_errors_uncached() {
        let cache = RunCacheMetadataCache::new();
        let ops = snowflake_ops();
        let relations = relations_for(&[S, U]);
        let s = fqn(&prefetch_nodes(), S);
        let calls = AtomicUsize::new(0);
        let fetch = |batch: Vec<Arc<dyn BaseRelation>>| {
            calls.fetch_add(1, Ordering::SeqCst);
            let s = s.clone();
            async move {
                let mut results = one_column_schemas(&batch)?;
                results.insert(
                    s,
                    Err(AdapterError::new(AdapterErrorKind::Internal, "denied")),
                );
                Ok(results)
            }
        };
        let got = tokio::time::timeout(
            TEST_TIMEOUT,
            get_schemas_with(&cache, &ops, &relations, fetch),
        )
        .await
        .expect("no hang");
        assert_eq!(got, Ok(()));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(cache.table_schema(&s), None, "errored table is not cached");
        let u = fqn(&prefetch_nodes(), U);
        assert!(matches!(cache.table_schema(&u), Some(Some(_))));

        // The next call retries only the errored table.
        get_schemas_with(&cache, &ops, &relations, |batch| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(batch.len(), 1);
            async move { one_column_schemas(&batch) }
        })
        .await
        .unwrap();
        assert!(matches!(cache.table_schema(&s), Some(Some(_))));
    }

    #[dbt_runtime::test]
    async fn await_prefetch_schemas_returns_when_never_started() {
        let ctx = ctx_with(prefetch_nodes(), &[M1]);
        await_prefetch_schemas(&ctx).await;
        assert!(ctx.inner.run_cache_ctx.schema_prefetch.is_done());
    }

    /// A ctx with a service client, so only the flag decides whether the
    /// schema prefetch starts.
    fn ctx_with_client(
        nodes: Nodes,
        selected: &[&str],
        flag: bool,
        mode: RunCacheMode,
        full_refresh: bool,
    ) -> TaskRunnerCtx {
        let mut ctx = test_task_runner_ctx_with_nodes(
            Some(Arc::new(RecordingRunCacheClient::default())),
            mode,
            full_refresh,
            Arc::new(EmptySourcesExtractor),
            nodes,
            ids(selected),
        );
        Arc::get_mut(&mut ctx.inner)
            .expect("sole owner of inner right after construction")
            .run_cache_ctx
            .run_cache_service_config
            .as_mut()
            .expect("config")
            .compare_selected_columns = flag;
        ctx
    }

    #[dbt_runtime::test]
    async fn column_aware_fields_record_only_send_flag_only() {
        let ctx = flagged_ctx();
        let s = fqn(ctx.nodes(), S);
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_table_schema(s, Some(Arc::from(vec![column("ID")])));
        let prefetch = &ctx.inner.run_cache_ctx.schema_prefetch;
        prefetch.mark_started();
        prefetch
            .set_handle(tokio::spawn(std::future::pending::<()>()))
            .await;

        let fields = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            collect_for_m1(&ctx, false, SqlContextPurpose::ExecutionRecord, &[S]),
        )
        .await
        .expect("a record never waits on the schema prefetch");
        assert!(fields.compare_selected_columns);
        assert!(
            fields.table_schemas.is_empty(),
            "execution records carry no schemas"
        );
        assert_eq!(fields.target_exists, None);
    }

    #[dbt_runtime::test]
    async fn start_prefetch_schemas_skips_full_refresh_bail_models() {
        // m1 is incremental: under full refresh it only records. sd is a seed.
        let ctx = ctx_with_client(
            prefetch_nodes(),
            &[M1, SD],
            true,
            RunCacheMode::ReadWrite,
            true,
        );
        start_prefetch_schemas(&ctx).await;
        assert!(!ctx.inner.run_cache_ctx.schema_prefetch.is_started());
    }

    #[dbt_runtime::test]
    async fn start_prefetch_schemas_runs_for_full_refresh_table_models() {
        // m2 is a table model: full refresh doesn't bail, so it still asks for a decision.
        let ctx = ctx_with_client(prefetch_nodes(), &[M2], true, RunCacheMode::ReadWrite, true);
        start_prefetch_schemas(&ctx).await;
        assert!(ctx.inner.run_cache_ctx.schema_prefetch.is_started());
    }

    #[dbt_runtime::test]
    async fn start_prefetch_schemas_is_noop_without_flag() {
        let ctx = ctx_with_client(
            prefetch_nodes(),
            &[M1, M2, SD],
            false,
            RunCacheMode::ReadWrite,
            false,
        );
        start_prefetch_schemas(&ctx).await;
        assert!(!ctx.inner.run_cache_ctx.schema_prefetch.is_started());
    }

    #[dbt_runtime::test]
    async fn start_prefetch_schemas_is_noop_in_write_only_mode() {
        let ctx = ctx_with_client(
            prefetch_nodes(),
            &[M1, M2, SD],
            true,
            RunCacheMode::WriteOnly,
            false,
        );
        start_prefetch_schemas(&ctx).await;
        assert!(!ctx.inner.run_cache_ctx.schema_prefetch.is_started());
    }

    #[test]
    fn dependency_kinds_by_fqn_includes_deferred_relations_of_unselected_nodes() {
        let nodes = prefetch_nodes();
        let mut defer_nodes = Nodes::default();
        let mut prod_u = DbtModel::clone(&nodes.models[U]);
        prod_u.__base_attr__.schema = "prod".to_string();
        let mut prod_m1 = DbtModel::clone(&nodes.models[M1]);
        prod_m1.__base_attr__.schema = "prod".to_string();
        defer_nodes.models.insert(U.to_string(), Arc::new(prod_u));
        defer_nodes.models.insert(M1.to_string(), Arc::new(prod_m1));
        let prod_u_fqn = fqn(&defer_nodes, U);
        let prod_m1_fqn = fqn(&defer_nodes, M1);

        let mut ctx = ctx_with(nodes, &[M1]);
        Arc::get_mut(&mut ctx.resolver_state)
            .expect("sole owner of resolver state right after construction")
            .defer_nodes = Some(defer_nodes);
        let kinds = dependency_kinds_by_fqn(&ctx);
        assert_eq!(kinds.get(&prod_u_fqn), Some(&DependencyKind::Model));
        assert!(
            !kinds.contains_key(&prod_m1_fqn),
            "selected nodes are built locally, not deferred"
        );
    }

    fn compare_selected_columns_state(flag: bool) -> ModelState {
        ModelState {
            lag_tolerance: None,
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
            ignore_external_modifications: None,
            compare_selected_columns: Some(flag),
        }
    }

    /// `prefetch_nodes` with `m1`'s config setting the flag, and a ctx that
    /// selects `m1` and `m2`. Nothing is fetched: the ctx has no adapter.
    fn flagged_ctx() -> TaskRunnerCtx {
        let mut nodes = prefetch_nodes();
        let mut m1 = DbtModel::clone(&nodes.models[M1]);
        m1.__model_attr__.state = Some(compare_selected_columns_state(true));
        nodes.models.insert(M1.to_string(), Arc::new(m1));
        ctx_with(nodes, &[M1, M2])
    }

    fn column(name: &str) -> ColumnInfo {
        ColumnInfo {
            name: name.to_string(),
            r#type: "NUMBER(38,0)".to_string(),
        }
    }

    async fn collect_for_m1(
        ctx: &TaskRunnerCtx,
        is_view: bool,
        purpose: SqlContextPurpose,
        seen: &[&str],
    ) -> ColumnAwareFields {
        let nodes = ctx.nodes();
        let seen_tables = fqns(nodes, seen);
        let node = nodes.get_node(M1).expect("m1");
        collect_column_aware_fields(ctx, node, is_view, purpose, &seen_tables, &BTreeMap::new())
            .await
    }

    #[dbt_runtime::test]
    async fn column_aware_fields_off_for_views() {
        let ctx = flagged_ctx();
        let s = fqn(ctx.nodes(), S);
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_table_schema(s, Some(Arc::from(vec![column("ID")])));
        let fields = collect_for_m1(&ctx, true, SqlContextPurpose::Decision, &[S]).await;
        assert!(!fields.compare_selected_columns);
        assert!(fields.table_schemas.is_empty());
        assert_eq!(fields.target_exists, None);
    }

    #[dbt_runtime::test]
    async fn column_aware_fields_speculative_omit_schemas() {
        let ctx = flagged_ctx();
        let s = fqn(ctx.nodes(), S);
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_table_schema(s, Some(Arc::from(vec![column("ID")])));
        let fields = collect_for_m1(&ctx, false, SqlContextPurpose::Speculative, &[S]).await;
        assert!(fields.compare_selected_columns);
        assert!(fields.table_schemas.is_empty());
        assert_eq!(fields.target_exists, None);
    }

    #[dbt_runtime::test]
    async fn column_aware_fields_send_cached_edge_schemas() {
        let ctx = flagged_ctx();
        let (s, u) = (fqn(ctx.nodes(), S), fqn(ctx.nodes(), U));
        let cache = &ctx.inner.run_cache_ctx.run_cache_metadata;
        cache.insert_table_schema(
            s.clone(),
            Some(Arc::from(vec![column("ID"), column("AMOUNT")])),
        );
        cache.insert_table_schema(u, None);
        let fields = collect_for_m1(&ctx, false, SqlContextPurpose::Decision, &[S, U, M2]).await;
        assert!(fields.compare_selected_columns);
        let names: Vec<&str> = fields
            .table_schemas
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, vec![s.as_str()]);
        assert_eq!(fields.table_schemas[0].columns.len(), 2);
    }

    #[dbt_runtime::test]
    async fn column_aware_fields_first_run_incremental() {
        let ctx = flagged_ctx();
        let m1 = fqn(ctx.nodes(), M1);
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_table_schema(m1.clone(), None);
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_relation_exists(m1.clone(), false);
        let fields = collect_for_m1(&ctx, false, SqlContextPurpose::Decision, &[M1]).await;
        assert!(fields.compare_selected_columns);
        assert!(
            fields.table_schemas.iter().all(|t| t.name != m1),
            "an absent own target sends no schema"
        );
        assert_eq!(fields.target_exists, Some(false));
    }

    #[dbt_runtime::test]
    async fn column_aware_fields_send_own_target_schema_when_sql_reads_it() {
        let ctx = flagged_ctx();
        let m1 = fqn(ctx.nodes(), M1);
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_table_schema(m1.clone(), Some(Arc::from(vec![column("ID")])));
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_relation_exists(m1.clone(), true);
        let fields = collect_for_m1(&ctx, false, SqlContextPurpose::Decision, &[M1]).await;
        let names: Vec<&str> = fields
            .table_schemas
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(names, vec![m1.as_str()], "own target stays an edge");
        assert_eq!(fields.target_exists, Some(true));
    }

    #[dbt_runtime::test]
    async fn column_aware_fields_write_only_send_flag_only() {
        let mut nodes = prefetch_nodes();
        let mut m1 = DbtModel::clone(&nodes.models[M1]);
        m1.__model_attr__.state = Some(compare_selected_columns_state(true));
        nodes.models.insert(M1.to_string(), Arc::new(m1));
        let ctx = test_task_runner_ctx_with_nodes(
            None,
            RunCacheMode::WriteOnly,
            false,
            Arc::new(EmptySourcesExtractor),
            nodes,
            ids(&[M1, M2]),
        );
        let s = fqn(ctx.nodes(), S);
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_table_schema(s, Some(Arc::from(vec![column("ID")])));
        let fields = collect_for_m1(&ctx, false, SqlContextPurpose::Decision, &[S]).await;
        assert!(fields.compare_selected_columns);
        assert!(
            fields.table_schemas.is_empty(),
            "execution records carry no schemas"
        );
        assert_eq!(fields.target_exists, None);
    }

    #[dbt_runtime::test]
    async fn column_aware_fields_skip_target_lookup_for_full_builds() {
        let mut nodes = prefetch_nodes();
        let mut m2 = DbtModel::clone(&nodes.models[M2]);
        m2.__model_attr__.state = Some(compare_selected_columns_state(true));
        nodes.models.insert(M2.to_string(), Arc::new(m2));
        let ctx = ctx_with(nodes, &[M1, M2]);
        let m2 = fqn(ctx.nodes(), M2);
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_relation_exists(m2, false);
        let node = ctx.nodes().get_node(M2).expect("m2");
        let fields = collect_column_aware_fields(
            &ctx,
            node,
            false,
            SqlContextPurpose::Decision,
            &BTreeSet::new(),
            &BTreeMap::new(),
        )
        .await;
        assert!(fields.compare_selected_columns);
        assert_eq!(
            fields.target_exists, None,
            "a table model builds as FULL, so its target is never reported missing"
        );
    }

    #[dbt_runtime::test]
    async fn column_aware_fields_no_target_lookup_for_tests() {
        let mut nodes = prefetch_nodes();
        let mut test = DbtTest::default();
        test.__common_attr__.unique_id = "test.pkg.t".to_string();
        test.__base_attr__.database = "db".to_string();
        test.__base_attr__.schema = "audit".to_string();
        test.__base_attr__.alias = "t".to_string();
        test.__test_attr__.state = Some(DataTestState {
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            compare_unrendered_code: None,
            compare_selected_columns: Some(true),
        });
        let test = Arc::new(test);
        nodes.tests.insert("test.pkg.t".to_string(), test.clone());
        let ctx = ctx_with(nodes, &[M1]);
        let s = fqn(ctx.nodes(), S);
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_table_schema(s.clone(), Some(Arc::from(vec![column("ID")])));
        let fields = collect_column_aware_fields(
            &ctx,
            test.as_ref(),
            false,
            SqlContextPurpose::Decision,
            &BTreeSet::from([s.clone()]),
            &BTreeMap::new(),
        )
        .await;
        assert!(fields.compare_selected_columns);
        assert_eq!(fields.table_schemas.len(), 1);
        assert_eq!(
            fields.target_exists, None,
            "data tests have no target table"
        );
    }

    #[dbt_runtime::test]
    async fn start_prefetch_schemas_with_flag_fails_open_without_adapter() {
        let ctx = ctx_with_client(
            prefetch_nodes(),
            &[M1, M2, SD],
            true,
            RunCacheMode::ReadWrite,
            false,
        );

        start_prefetch_schemas(&ctx).await;
        let prefetch = &ctx.inner.run_cache_ctx.schema_prefetch;
        assert!(prefetch.is_started());
        await_prefetch_schemas(&ctx).await;
        assert!(prefetch.is_done());
        assert!(
            ctx.inner
                .run_cache_ctx
                .run_cache_metadata
                .table_schema(&fqn(ctx.nodes(), S))
                .is_none(),
            "nothing is cached without an adapter"
        );
        assert!(
            !ctx.inner.run_cache_ctx.prefetch.is_started(),
            "the schema prefetch never touches the last-modified prefetch"
        );
    }
}
