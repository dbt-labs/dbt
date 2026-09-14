use std::sync::Arc;

use crate::context::TaskRunnerCtx;
use crate::run_cache::run_cache_request::{
    DbtProjectInfo, model_clone_table_properties, model_execution_type_input, node_identity,
    snapshot_clone_table_properties, snapshot_execution_type_input,
};
use crate::run_cache::run_cache_service::{
    RunCacheCloneDecision, clone_chain_depth_limit_for_adapter,
    confirm_run_cache_service_execution, execute_run_cache_service_clone, has_metadata_address,
    legacy_freshness, run_cache_metadata_query_options,
};
use crate::run_cache::run_cache_service::{record_dev_clone_decision, replay_dev_clone_decision};
use dbt_adapter::errors::{AdapterErrorKind, Cancellable};
use dbt_adapter::metadata::MetadataAdapter;
use dbt_adapter::relation::create_relation_from_node;
use dbt_adapter_core::AdapterType;
use dbt_common::tracing::dbt_emit::{emit_trace_log_message, emit_warn_log_message};
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::materialization_resolver::MaterializationResolver;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::profiles::Execute;
use dbt_schemas::schemas::properties::StatePreClone;
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_schemas::schemas::{DbtModel, DbtSnapshot, InternalDbtNode, InternalDbtNodeAttributes};
use dbt_state::explain::StateExplainDevClone;
use dbt_state::proto::query_cache::{CloneResponse, ReadyToCloneResponse, clone_response};
use dbt_state::request_builder::{CloneRequestInput, execution_type_from_input};
use dbt_state::service_config::CloneIncrementalInDev;

pub async fn maybe_run_dev_clone_for_node(ctx: &TaskRunnerCtx, node_id: &str) {
    let Some(candidate) = dev_clone_candidate_for_node(ctx, node_id) else {
        return;
    };
    let node = candidate.node();

    if dbt_adapter::time_machine::is_replaying() {
        let Some(clone) = replay_dev_clone_decision(node_id) else {
            return;
        };
        match execute_run_cache_service_clone(ctx, node.as_ref(), &clone, None, false).await {
            Ok(_) => {
                finish_dev_clone(
                    ctx,
                    node.as_ref(),
                    &clone,
                    clone.clone_source().to_string(),
                    clone.clone_target().to_string(),
                )
                .await
            }
            Err(err) => emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "Time-machine dev clone failed for node {node_id}: {err}; executing normally"
                ),
            ),
        }
        return;
    }

    let Some(policy) = dev_clone_policy(ctx, &candidate) else {
        return;
    };

    let Some(client) = ctx
        .inner
        .run_cache_ctx
        .run_cache_service_client
        .as_ref()
        .cloned()
    else {
        return;
    };
    let prepared = match prepare_dev_clone_request(ctx, &candidate, policy).await {
        Ok(Some(prepared)) => prepared,
        Ok(None) => return,
        Err(err) => {
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State dev clone request preparation failed for node {node_id}: {err}; executing normally"
                ),
            );
            return;
        }
    };

    let response = match client.register_clone(prepared.request).await {
        Ok(response) => response,
        Err(dbt_state::service_client::RunCacheServiceError::Disabled) => return,
        Err(err) => {
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State dev clone registration failed for node {node_id}: {err}; executing normally"
                ),
            );
            return;
        }
    };
    let ready_to_clone = match handle_clone_response(response) {
        Ok(r) => r,
        Err(msg) => {
            emit_trace_log_message(|| {
                format!(
                    "dbt State dev clone registration is unable to clone {node_id}: '{msg}'; executing normally"
                )
            });
            return;
        }
    };

    let clone = RunCacheCloneDecision::from_response(&ready_to_clone, 0);
    match execute_run_cache_service_clone(ctx, node.as_ref(), &clone, None, false).await {
        Ok(_) => {
            record_dev_clone_decision(node_id, &clone);
            finish_dev_clone(
                ctx,
                node.as_ref(),
                &clone,
                prepared.clone_source_table,
                prepared.target_table,
            )
            .await;
            let source = clone.clone_source().to_string();
            emit_trace_log_message(|| {
                format!("dbt State dev clone completed for node {node_id} (source {source})")
            });
        }
        Err(err) => {
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State dev clone SQL failed for node {node_id}: {err}; executing normally"
                ),
            );
        }
    }
}

async fn finish_dev_clone(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    clone: &RunCacheCloneDecision,
    source_table: String,
    target_table: String,
) {
    let explain_dev_clone = StateExplainDevClone {
        source_table_fqn: source_table,
        target_table_fqn: target_table.clone(),
    };
    ctx.inner
        .run_cache_ctx
        .run_cache_metadata
        .invalidate_relation_metadata(&target_table);
    ctx.inner
        .run_cache_ctx
        .run_cache_metadata
        .insert_relation_exists(target_table, true);
    ctx.inner
        .run_cache_ctx
        .run_cache_dev_cloned_nodes
        .insert(node.unique_id(), explain_dev_clone);
    confirm_run_cache_service_execution(ctx, node, clone.success_confirmation(), None).await;
}

fn dev_clone_candidate_for_node(ctx: &TaskRunnerCtx, node_id: &str) -> Option<DevCloneCandidate> {
    let defer_nodes = ctx.defer_nodes()?;
    let nodes = ctx.nodes();

    if let Some(model) = nodes.models.get(node_id)
        && model.materialized() == DbtMaterialization::Incremental
        && let Some(deferred_model) = defer_nodes.models.get(node_id)
    {
        return Some(DevCloneCandidate::Model {
            local: Arc::clone(model),
            deferred: Arc::clone(deferred_model),
        });
    }

    if let Some(snapshot) = nodes.snapshots.get(node_id)
        && let Some(deferred_snapshot) = defer_nodes.snapshots.get(node_id)
    {
        return Some(DevCloneCandidate::Snapshot {
            local: Arc::clone(snapshot),
            deferred: Arc::clone(deferred_snapshot),
        });
    }

    None
}

fn dev_clone_policy(
    ctx: &TaskRunnerCtx,
    candidate: &DevCloneCandidate,
) -> Option<CloneIncrementalInDev> {
    if !ctx.inner.run_cache_ctx.run_cache_service_requested
        || ctx.inner.run_cache_ctx.run_cache_service_client.is_none()
        || ctx.defer_nodes().is_none()
        || !ctx.inner.arg.is_runnable()
        || ctx.inner.execute != Execute::Remote
        || ctx.inner.arg.full_refresh
    {
        return None;
    }

    let policy = candidate.pre_clone_policy().or_else(|| {
        ctx.inner
            .run_cache_ctx
            .run_cache_service_config
            .as_ref()
            .map(|config| config.clone_incremental_in_dev)
    })?;
    (policy != CloneIncrementalInDev::Never).then_some(policy)
}

fn state_pre_clone_to_policy(pre_clone: StatePreClone) -> CloneIncrementalInDev {
    match pre_clone {
        StatePreClone::Never => CloneIncrementalInDev::Never,
        StatePreClone::IfMissing => CloneIncrementalInDev::IfTableMissing,
        StatePreClone::Always => CloneIncrementalInDev::Always,
    }
}

enum DevCloneCandidate {
    Model {
        local: Arc<DbtModel>,
        deferred: Arc<DbtModel>,
    },
    Snapshot {
        local: Arc<DbtSnapshot>,
        deferred: Arc<DbtSnapshot>,
    },
}

impl DevCloneCandidate {
    fn node(&self) -> Arc<dyn InternalDbtNodeAttributes> {
        match self {
            Self::Model { local, .. } => local.clone(),
            Self::Snapshot { local, .. } => local.clone(),
        }
    }

    fn pre_clone_policy(&self) -> Option<CloneIncrementalInDev> {
        match self {
            Self::Model { local, .. } => local
                .__model_attr__
                .state
                .as_ref()
                .and_then(|state| state.pre_clone.clone())
                .map(state_pre_clone_to_policy),
            Self::Snapshot { local, .. } => local
                .__snapshot_attr__
                .state
                .as_ref()
                .and_then(|state| state.pre_clone.clone())
                .map(state_pre_clone_to_policy),
        }
    }

    fn local(&self) -> &dyn InternalDbtNodeAttributes {
        match self {
            Self::Model { local, .. } => local.as_ref(),
            Self::Snapshot { local, .. } => local.as_ref(),
        }
    }

    fn deferred(&self) -> &dyn InternalDbtNodeAttributes {
        match self {
            Self::Model { deferred, .. } => deferred.as_ref(),
            Self::Snapshot { deferred, .. } => deferred.as_ref(),
        }
    }

    fn execution_type(
        &self,
        materialization_resolver: &MaterializationResolver,
    ) -> FsResult<dbt_state::proto::query_cache::ModelExecutionType> {
        let execution_type =
            match self {
                Self::Model { local, .. } => execution_type_from_input(
                    &model_execution_type_input(local, false, materialization_resolver),
                ),
                Self::Snapshot { local, .. } => {
                    execution_type_from_input(&snapshot_execution_type_input(local, false))
                }
            }
            .map_err(|err| {
                fs_err!(
                    ErrorCode::Generic,
                    "Failed to build dbt State dev clone execution type: {}",
                    err
                )
            })?;
        Ok(execution_type)
    }

    fn table_properties(&self) -> Option<dbt_state::proto::query_cache::TableProperties> {
        match self {
            Self::Model { local, .. } => model_clone_table_properties(local),
            Self::Snapshot { local, .. } => snapshot_clone_table_properties(local),
        }
    }

    /// Derive the source table type string sent in the CloneRequest from the dbt node's
    /// config — mirroring dbt-core's `get_relation_table_type` (see
    /// run-cache/clients/dbt_run_cache/src/dbt_run_cache/adapters/snowflake.py:42-80).
    ///
    /// We can't read this from warehouse introspection because Fusion's `RelationType` enum
    /// has no `Transient` variant, so the warehouse-observed type always comes through as
    /// "table" — and Snowflake refuses to clone a TRANSIENT source into a permanent TABLE.
    fn clone_source_table_type(&self, adapter_type: AdapterType) -> Option<String> {
        if adapter_type != AdapterType::Snowflake {
            return None;
        }
        let (materialized, transient) = match self {
            Self::Model { local, .. } => (
                local.as_ref().base().materialized.clone(),
                local
                    .deprecated_config
                    .__warehouse_specific_config__
                    .transient,
            ),
            Self::Snapshot { local, .. } => (
                local.as_ref().base().materialized.clone(),
                local
                    .deprecated_config
                    .__warehouse_specific_config__
                    .transient,
            ),
        };
        match materialized {
            DbtMaterialization::DynamicTable => Some(
                if transient == Some(true) {
                    "TRANSIENT DYNAMIC TABLE"
                } else {
                    "DYNAMIC TABLE"
                }
                .to_string(),
            ),
            DbtMaterialization::Table
            | DbtMaterialization::Incremental
            | DbtMaterialization::Snapshot => Some(
                // dbt-snowflake defaults to TRANSIENT unless the user explicitly opts out.
                if transient.unwrap_or(true) {
                    "TRANSIENT TABLE"
                } else {
                    "TABLE"
                }
                .to_string(),
            ),
            // Live-verified against Snowflake (account ktb38830, 2026-09-01): `CREATE
            // INTERACTIVE TABLE ... CLONE ...` is not valid DDL in any form (bare, CREATE OR
            // REPLACE, cross-schema/database, AT/BEFORE time travel).
            DbtMaterialization::InteractiveTable => None,
            _ => None,
        }
    }
}

struct PreparedDevClone {
    request: dbt_state::proto::query_cache::CloneRequest,
    target_table: String,
    clone_source_table: String,
}

async fn prepare_dev_clone_request(
    ctx: &TaskRunnerCtx,
    candidate: &DevCloneCandidate,
    policy: CloneIncrementalInDev,
) -> FsResult<Option<PreparedDevClone>> {
    let target_relation =
        create_relation_from_node(ctx.default_adapter_type(), candidate.local(), None)?;
    let target_relation: Arc<dyn BaseRelation> = target_relation.into();
    if !has_metadata_address(ctx.default_adapter_type(), target_relation.as_ref()) {
        return Ok(None);
    }
    let target_table = target_relation.semantic_fqn();

    if policy == CloneIncrementalInDev::IfTableMissing {
        match relation_exists(ctx, &target_table, target_relation.clone()).await {
            Some(false) => {}
            Some(true) => {
                let unique_id = candidate.local().unique_id();
                let target = target_table.clone();
                emit_trace_log_message(|| {
                    format!(
                        "dbt State dev clone skipped because target relation already exists (node {unique_id}, target {target})"
                    )
                });
                return Ok(None);
            }
            None => {
                let unique_id = candidate.local().unique_id();
                let target = target_table.clone();
                emit_trace_log_message(|| {
                    format!(
                        "dbt State dev clone skipped because target existence metadata is unavailable (node {unique_id}, target {target})"
                    )
                });
                return Ok(None);
            }
        }
    }

    let source_relation =
        create_relation_from_node(ctx.default_adapter_type(), candidate.deferred(), None)?;
    let source_relation: Arc<dyn BaseRelation> = source_relation.into();
    if !has_metadata_address(ctx.default_adapter_type(), source_relation.as_ref()) {
        return Ok(None);
    }
    let clone_source_table = source_relation.semantic_fqn();
    if target_table == clone_source_table {
        let unique_id = candidate.local().unique_id();
        let relation = target_table.clone();
        emit_trace_log_message(|| {
            format!(
                "dbt State dev clone skipped because source and target relations are identical (node {unique_id}, relation {relation})"
            )
        });
        return Ok(None);
    }

    let source_exists =
        fetch_relation_exists(ctx, &clone_source_table, source_relation.clone()).await;
    if source_exists == Some(false) {
        let unique_id = candidate.local().unique_id();
        let source = clone_source_table.clone();
        emit_trace_log_message(|| {
            format!(
                "dbt State dev clone skipped because deferred source relation is missing (node {unique_id}, source {source})"
            )
        });
        return Ok(None);
    }
    let source_last_modified_epoch =
        last_modified_epoch(ctx, &clone_source_table, source_relation.clone()).await;

    let project_info = DbtProjectInfo::from(ctx);

    let request = CloneRequestInput {
        target_table: target_table.clone(),
        dialect: ctx.default_adapter_type().to_string(),
        default_catalog: candidate.local().database(),
        execution_type: candidate.execution_type(&ctx.inner.materialization_resolver)?,
        clone_source_table: clone_source_table.clone(),
        clone_source_last_modified_epoch: source_last_modified_epoch,
        labels: node_identity(candidate.local()).labels(),
        clone_source_table_type: candidate.clone_source_table_type(ctx.default_adapter_type()),
        table_properties: candidate.table_properties(),
        clone_chain_depth_limit: clone_chain_depth_limit_for_adapter(
            ctx.default_adapter_type(),
            false,
            ctx.dbt_profile().allow_clones,
        ),
        table_namespace: project_info.table_namespace,
    }
    .into_proto();

    Ok(Some(PreparedDevClone {
        request,
        target_table,
        clone_source_table,
    }))
}

async fn relation_exists(
    ctx: &TaskRunnerCtx,
    name: &str,
    relation: Arc<dyn BaseRelation>,
) -> Option<bool> {
    if !has_metadata_address(ctx.default_adapter_type(), relation.as_ref()) {
        return None;
    }
    if let Some(exists) = ctx
        .inner
        .run_cache_ctx
        .run_cache_metadata
        .relation_exists(name)
    {
        return Some(exists);
    }
    fetch_relation_exists(ctx, name, relation).await
}

async fn fetch_relation_exists(
    ctx: &TaskRunnerCtx,
    name: &str,
    relation: Arc<dyn BaseRelation>,
) -> Option<bool> {
    if !has_metadata_address(ctx.default_adapter_type(), relation.as_ref()) {
        return None;
    }
    let adapter = ctx.env.get_adapter_ref()?;
    let metadata_adapter: Arc<dyn MetadataAdapter> = Arc::from(adapter.metadata_adapter()?);
    let semantic_fqn = relation.semantic_fqn();
    let metadata_options = run_cache_metadata_query_options(ctx);
    let token = adapter.cancellation_token();
    let cache = &ctx.inner.run_cache_ctx.run_cache_metadata;
    let result = cache
        .get_or_try_insert_relation_exists(name, {
            let token = token.clone();
            move || {
                let relation = Arc::clone(&relation);
                let semantic_fqn = semantic_fqn.clone();
                let metadata_options = metadata_options.clone();
                let metadata_adapter = Arc::clone(&metadata_adapter);
                let token = token.clone();
                async move {
                    let relations = [relation];
                    let existence = match metadata_adapter
                        .relations_exist_with_options(&relations, &metadata_options, token.clone())
                        .await
                    {
                        Ok(existence) => Ok(existence),
                        Err(Cancellable::Error(err))
                            if err.kind() == AdapterErrorKind::ReplayDataMissing => {
                            metadata_adapter.relations_exist(&relations, token).await
                        }
                        Err(err) => Err(err),
                    }
                    .map_err(|err| {
                        emit_trace_log_message(|| {
                            format!(
                                "dbt State dev clone relation existence lookup failed (relation {name}): {err:?}"
                            )
                        });
                        format!("{err:?}")
                    })?;
                    existence.get(&semantic_fqn).copied().ok_or_else(|| {
                        format!("relation {name} missing from metadata response")
                    })
                }
            }
        })
        .await;
    if result.is_err() && token.is_cancelled() {
        // A cancelled run isn't a genuine lookup failure; don't leave it
        // poisoning the cache for other in-flight lookups of this relation.
        cache.remove_relation_exists_error(name);
    }
    result.ok()
}

async fn last_modified_epoch(
    ctx: &TaskRunnerCtx,
    name: &str,
    relation: Arc<dyn BaseRelation>,
) -> Option<i64> {
    if !has_metadata_address(ctx.default_adapter_type(), relation.as_ref()) {
        return None;
    }
    if let Some(epoch) = ctx
        .inner
        .run_cache_ctx
        .run_cache_metadata
        .last_modified_epoch(name)
    {
        return epoch;
    }

    let adapter = ctx.env.get_adapter_ref()?;
    let metadata_adapter: Arc<dyn MetadataAdapter> = Arc::from(adapter.metadata_adapter()?);
    let semantic_fqn = relation.semantic_fqn();
    let metadata_options = run_cache_metadata_query_options(ctx);
    let token = adapter.cancellation_token();
    let cache = &ctx.inner.run_cache_ctx.run_cache_metadata;
    let result = cache
        .get_or_try_insert_last_modified_epoch(name, {
            let token = token.clone();
            move || {
                let relation = Arc::clone(&relation);
                let semantic_fqn = semantic_fqn.clone();
                let metadata_options = metadata_options.clone();
                let metadata_adapter = Arc::clone(&metadata_adapter);
                let token = token.clone();
                async move {
                    let relations = [relation];
                    let freshness = legacy_freshness(
                        metadata_adapter.as_ref(),
                        &relations,
                        None,
                        &metadata_options,
                        token,
                    )
                    .await
                    .map_err(|err| {
                        emit_trace_log_message(|| {
                            format!(
                                "dbt State dev clone source freshness lookup failed (relation {name}): {err:?}"
                            )
                        });
                        format!("{err:?}")
                    })?;
                    Ok::<Option<i64>, String>(
                        freshness
                            .get(&semantic_fqn)
                            .map(|metadata| metadata.last_altered.timestamp_millis()),
                    )
                }
            }
        })
        .await;
    if result.is_err() && token.is_cancelled() {
        // A cancelled run isn't a genuine lookup failure; don't leave it
        // poisoning the cache for other in-flight lookups of this relation.
        cache.remove_last_modified_epoch_error(name);
    }
    result.ok().flatten()
}

#[allow(deprecated)] // Reads legacy oneof variants for backward-compat with older servers.
fn handle_clone_response(response: CloneResponse) -> Result<ReadyToCloneResponse, String> {
    let error = response
        .unable_to_clone
        .and_then(|r| {
            r.explained_decision
                .map(|d| d.clone_rejection_reason().as_str_name())
        })
        .unwrap_or("Unable to clone");

    response.ready_to_clone.map_or_else(
        || match response.response {
            Some(clone_response::Response::ReadyToCloneV1(response)) => Ok(response),
            Some(clone_response::Response::UntrackableClone(response)) => {
                Ok(ReadyToCloneResponse {
                    clone_sqls: response.clone_sqls,
                    clone_source: response.clone_source,
                    clone_target: response.clone_target,
                    explained_decision: response.explained_decision,
                    transformed_nodes_by_query: response.transformed_nodes_by_query,
                    ..Default::default()
                })
            }
            None => Err(error.to_owned()),
        },
        Ok,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_common::io_args::StaticAnalysisKind;
    use dbt_schemas::schemas::common::ResolvedQuoting;
    use dbt_schemas::schemas::nodes::AdapterAttr;
    use dbt_schemas::schemas::project::WarehouseSpecificNodeConfig;
    use dbt_schemas::schemas::project::{ModelConfig, SnapshotConfig};
    use dbt_schemas::schemas::serde::StringOrArrayOfStrings;
    use dbt_schemas::schemas::{
        CommonAttributes, DbtModelAttr, DbtSnapshotAttr, NodeBaseAttributes,
    };
    use dbt_state::proto::query_cache::{ModelExecutionType, TableProperties};
    use dbt_yaml::Spanned;
    use indexmap::IndexMap;
    use std::collections::BTreeMap;

    /// A resolver with no macros: built-in materialization names resolve to no
    /// user-defined macro, so `is_custom_materialization` is false — matching
    /// the built-in materializations these tests use.
    fn test_resolver() -> MaterializationResolver {
        MaterializationResolver::new(&BTreeMap::new(), "jaffle_shop")
    }

    #[test]
    fn dev_clone_candidate_uses_model_state_pre_clone_policy() {
        let mut local = make_model(
            "dev",
            "analytics_dev",
            "orders",
            DbtMaterialization::Incremental,
        );
        local.__model_attr__.state = Some(dbt_schemas::schemas::properties::ModelState {
            lag_tolerance: None,
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: Some(StatePreClone::Always),
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
        });
        let candidate = DevCloneCandidate::Model {
            local: Arc::new(local),
            deferred: Arc::new(make_model(
                "prod",
                "analytics",
                "orders",
                DbtMaterialization::Incremental,
            )),
        };

        assert_eq!(
            candidate.pre_clone_policy(),
            Some(CloneIncrementalInDev::Always)
        );
    }

    #[test]
    fn clone_source_table_type_returns_none_for_interactive_table_because_snowflake_cannot_clone_it()
     {
        // See `clone_source_table_type`: Snowflake cannot clone an interactive table.
        let mut local = make_model(
            "dev",
            "analytics_dev",
            "orders",
            DbtMaterialization::InteractiveTable,
        );
        let candidate = DevCloneCandidate::Model {
            local: Arc::new(local.clone()),
            deferred: Arc::new(make_model(
                "prod",
                "analytics",
                "orders",
                DbtMaterialization::InteractiveTable,
            )),
        };
        assert_eq!(
            candidate.clone_source_table_type(AdapterType::Snowflake),
            None
        );

        local
            .deprecated_config
            .__warehouse_specific_config__
            .transient = Some(true);
        let candidate = DevCloneCandidate::Model {
            local: Arc::new(local),
            deferred: Arc::new(make_model(
                "prod",
                "analytics",
                "orders",
                DbtMaterialization::InteractiveTable,
            )),
        };
        assert_eq!(
            candidate.clone_source_table_type(AdapterType::Snowflake),
            None
        );
    }

    #[test]
    fn dev_clone_candidate_uses_snapshot_state_pre_clone_policy() {
        let mut local = make_snapshot("dev", "snapshots_dev", "orders_snapshot");
        local.__snapshot_attr__.state = Some(dbt_schemas::schemas::properties::ModelState {
            lag_tolerance: None,
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: Some(StatePreClone::IfMissing),
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
        });
        let candidate = DevCloneCandidate::Snapshot {
            local: Arc::new(local),
            deferred: Arc::new(make_snapshot("prod", "snapshots", "orders_snapshot")),
        };

        assert_eq!(
            candidate.pre_clone_policy(),
            Some(CloneIncrementalInDev::IfTableMissing)
        );
    }

    #[test]
    #[allow(deprecated)]
    fn ready_to_clone_prefers_current_clone_response_field() {
        let response = CloneResponse {
            ready_to_clone: Some(ReadyToCloneResponse {
                request_id: "current".to_string(),
                ..Default::default()
            }),
            unable_to_clone: None,
            response: Some(clone_response::Response::ReadyToCloneV1(
                ReadyToCloneResponse {
                    request_id: "legacy".to_string(),
                    ..Default::default()
                },
            )),
        };

        assert_eq!(
            handle_clone_response(response).unwrap().request_id,
            "current"
        );
    }

    #[test]
    #[allow(deprecated)]
    fn ready_to_clone_supports_legacy_oneof_response() {
        let response = CloneResponse {
            ready_to_clone: None,
            unable_to_clone: None,
            response: Some(clone_response::Response::ReadyToCloneV1(
                ReadyToCloneResponse {
                    request_id: "legacy".to_string(),
                    clone_sqls: vec!["create table target clone source".to_string()],
                    ..Default::default()
                },
            )),
        };

        let ready = handle_clone_response(response).unwrap();
        assert_eq!(ready.request_id, "legacy");
        assert_eq!(ready.clone_sqls, vec!["create table target clone source"]);
    }

    #[test]
    fn dev_clone_candidate_builds_model_clone_request() {
        let local = Arc::new(make_model(
            "dev",
            "analytics_dev",
            "orders",
            DbtMaterialization::Incremental,
        ));
        let deferred = Arc::new(make_model(
            "prod",
            "analytics",
            "orders",
            DbtMaterialization::Incremental,
        ));
        let candidate = DevCloneCandidate::Model { local, deferred };
        let target_relation =
            create_relation_from_node(AdapterType::Snowflake, candidate.local(), None).unwrap();
        let source_relation =
            create_relation_from_node(AdapterType::Snowflake, candidate.deferred(), None).unwrap();

        let request = CloneRequestInput {
            target_table: target_relation.semantic_fqn(),
            dialect: "snowflake".to_string(),
            default_catalog: candidate.local().database(),
            execution_type: candidate.execution_type(&test_resolver()).unwrap(),
            clone_source_table: source_relation.semantic_fqn(),
            clone_source_last_modified_epoch: Some(123),
            labels: node_identity(candidate.local()).labels(),
            clone_source_table_type: Some("table".to_string()),
            table_properties: candidate.table_properties(),
            clone_chain_depth_limit: None,
            table_namespace: Some("adapter-unique-id".to_string()),
        }
        .into_proto();

        // semantic_fqn always quotes; with the test's all-true quote policy
        // it keeps the literal casing rather than normalizing.
        assert!(request.target_table.contains("\"analytics_dev\""));
        assert!(request.clone_source_table.contains("\"analytics\""));
        assert_eq!(request.default_catalog, "dev");
        assert_eq!(request.execution_type, ModelExecutionType::Merge as i32);
        assert_eq!(request.clone_source_last_modified_epoch, Some(123));
        assert_eq!(request.table_namespace(), "adapter-unique-id");
        assert_eq!(
            request.labels.get("dbt_node_unique_id").map(String::as_str),
            Some("model.jaffle_shop.orders")
        );
        assert_eq!(
            request.table_properties,
            Some(TableProperties {
                hours_to_expiration: Some(12),
                partition_expiration_days: None,
            })
        );
        assert!(request.clone_chain_depth_limit.is_none())
    }

    #[test]
    fn dev_clone_candidate_builds_snapshot_execution_type() {
        let candidate = DevCloneCandidate::Snapshot {
            local: Arc::new(make_snapshot("dev", "snapshots_dev", "orders_snapshot")),
            deferred: Arc::new(make_snapshot("prod", "snapshots", "orders_snapshot")),
        };

        assert_eq!(
            candidate.execution_type(&test_resolver()).unwrap(),
            ModelExecutionType::Snapshot
        );
        assert_eq!(
            candidate
                .table_properties()
                .and_then(|properties| properties.partition_expiration_days),
            Some(7)
        );
    }

    fn make_common(unique_id: &str, name: &str) -> CommonAttributes {
        CommonAttributes {
            name: name.to_string(),
            unique_id: unique_id.to_string(),
            package_name: "jaffle_shop".to_string(),
            fqn: vec!["jaffle_shop".to_string(), name.to_string()],
            tags: vec![],
            meta: IndexMap::new(),
            ..Default::default()
        }
    }

    fn make_base(
        database: &str,
        schema: &str,
        alias: &str,
        materialized: DbtMaterialization,
    ) -> NodeBaseAttributes {
        NodeBaseAttributes {
            database: database.to_string(),
            schema: schema.to_string(),
            alias: alias.to_string(),
            materialized,
            quoting: ResolvedQuoting::trues(),
            static_analysis: Spanned::new(StaticAnalysisKind::On),
            enabled: true,
            ..Default::default()
        }
    }

    fn make_model(
        database: &str,
        schema: &str,
        alias: &str,
        materialized: DbtMaterialization,
    ) -> DbtModel {
        DbtModel {
            __common_attr__: make_common("model.jaffle_shop.orders", "orders"),
            __base_attr__: make_base(database, schema, alias, materialized),
            __model_attr__: DbtModelAttr::default(),
            __adapter_attr__: AdapterAttr::default(),
            deprecated_config: ModelConfig {
                incremental_strategy: Some(
                    dbt_schemas::schemas::common::DbtIncrementalStrategy::Merge,
                ),
                unique_key: Some(dbt_schemas::schemas::common::DbtUniqueKey::Single(
                    "id".to_string(),
                )),
                merge_update_columns: Some(StringOrArrayOfStrings::String("status".to_string())),
                __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                    hours_to_expiration: dbt_common::serde_utils::Omissible::Present(Some(
                        dbt_schemas::schemas::serde::StringOrInteger::Integer(12),
                    )),
                    ..Default::default()
                },
                ..Default::default()
            },
            __other__: Default::default(),
        }
    }

    fn make_snapshot(database: &str, schema: &str, alias: &str) -> DbtSnapshot {
        DbtSnapshot {
            __common_attr__: make_common("snapshot.jaffle_shop.orders_snapshot", "orders_snapshot"),
            __base_attr__: make_base(database, schema, alias, DbtMaterialization::Snapshot),
            __snapshot_attr__: DbtSnapshotAttr::default(),
            __adapter_attr__: AdapterAttr::default(),
            deprecated_config: SnapshotConfig {
                __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                    partition_expiration_days: Some(7),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }
}
