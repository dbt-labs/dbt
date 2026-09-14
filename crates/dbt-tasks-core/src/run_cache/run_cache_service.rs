//! dbt State service shadow path for remote task execution.
//!
//! This module translates task-layer state into service requests before a node
//! executes, interprets the service decision, and confirms successful execution
//! back to the service when a request id was returned. The integration is
//! deliberately fail-open: unsupported nodes, missing metadata, service errors,
//! and confirmation failures all fall back to normal execution so service
//! availability does not change command success.
//!
//! The module owns task-specific concerns such as rendered SQL extraction,
//! adapter relation rendering, warehouse metadata lookups, and skip policy.
//! Stable service DTO construction lives in `dbt-state::request_builder`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use tokio::sync::mpsc;

use crate::context::TaskRunnerCtx;
use crate::task::{TaskOp, TaskResult};
use dbt_adapter::AdapterResult;
use dbt_adapter::errors::{AdapterError, AdapterErrorKind, Cancellable, into_fs_error};
use dbt_adapter::metadata::{
    CatalogAndSchema, FreshnessOverride, MetadataAdapter, MetadataFreshness, MetadataQueryOptions,
};
use dbt_adapter::record_batch::RecordBatchExt;
use dbt_adapter::relation::{RelationObject, create_relation, create_relation_from_node};
use dbt_adapter::sql_types::TypeOps;
use dbt_adapter_core::AdapterType;
use dbt_adbc::QueryCtx;
use dbt_common::adapter::dialect_of;
use dbt_common::cancellation::CancellationToken;
use dbt_common::io_args::RunCacheMode;
use dbt_common::stats::NodeStatus;
use dbt_common::tracing::dbt_emit::{
    emit_debug_log_message, emit_trace_log_message, emit_warn_log_message,
};
use dbt_common::tracing::span_info::find_and_update_span_attrs;
use dbt_common::{ErrorCode, FsError, FsResult, fs_err};
use dbt_frontend_common::ident::FullyQualifiedName;
use dbt_frontend_common::named_reference::NamedReference;
use dbt_frontend_common::sources_extractor::SourcesExtractor;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::materialization_resolver::MaterializationResolver;
use dbt_schemas::schemas::common::{DbtMaterialization, ModelFreshnessRules, ResolvedQuoting};
use dbt_schemas::schemas::macros::DbtMacro;
use dbt_schemas::schemas::profiles::DbConfig;
use dbt_schemas::schemas::properties::ModelState;
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_schemas::schemas::{
    DbtModel, DbtSeed, DbtSnapshot, DbtSource, DbtTest, InternalDbtNode, InternalDbtNodeAttributes,
};
use dbt_state::explain::{
    StateExplainLogRecord, StateExplainNode, StateExplainNodeInfo, append_state_explain_log_record,
};
use dbt_state::metadata_cache::{MetadataPrefetchGuard, RunCacheMetadataCache};
use dbt_state::node_session::ExecutionGuard;
use dbt_state::proto::query_cache::{
    ClientTelemetryEvent, ConfirmExecutionRequest, ExplainedDecision, NodeFuncMapping,
    QueryDependency, RecordExecutionsRequest, SkipExecutionResponse, Struct,
    SubmitEnrichedSqlRequest, SubmitSqlResponse, SubmitValuesRequest, TableModifiedInfo, Value,
    submit_sql_response, submit_sql_speculative_response, value::Kind,
};
use dbt_state::request_builder::{
    ExecutionOutcomeInput, SessionEndResult, enriched_sql_prepared_event, session_end_event,
    session_start_event, sql_execution_record_from_submit_request, telemetry_batch,
    values_execution_record_from_submit_request,
};
use dbt_state::service_client::RunCacheServiceError;
use dbt_telemetry::{NodeEvaluated, NodeType};

use crate::run_cache::run_cache_request::{
    DbtProjectInfo, SeedRunCacheRequestContext, SqlRunCacheRequestContext, build_model_sql_request,
    build_seed_values_request, build_snapshot_sql_request, build_test_sql_request,
    is_microbatch_model, node_identity,
};
use chrono::{DateTime, Utc};

pub fn collect_upstream_hashes(ctx: &TaskRunnerCtx, unique_id: &str) -> HashMap<String, String> {
    ctx.inner
        .schedule
        .deps
        .get(unique_id)
        .map(|deps| {
            deps.iter()
                .filter_map(|dep| {
                    ctx.inner
                        .node_hashes
                        .get(dep)
                        .map(|hash| (dep.clone(), hash.value().clone()))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub enum RunCacheServiceDecision {
    Execute {
        after_success: RunCacheAfterSuccess,
        sao_guard: Option<ExecutionGuard>,
    },
    Clone {
        clone: RunCacheCloneDecision,
    },
    Skip {
        status: NodeStatus,
        sao_stored_hash: Option<String>,
        /// Cached data-test result when the skipped node is a data test. The
        /// dispatcher in `runnable/mod.rs` uses it to replace the generic
        /// `ReusedNoChanges` status with a test-shaped status and a
        /// NO-OP-marked stat.
        cached_test_result: Option<CachedTestExecutionResult>,
    },
    Disabled,
}

impl RunCacheServiceDecision {
    fn execute_without_confirmation() -> Self {
        Self::Execute {
            after_success: RunCacheAfterSuccess::None,
            sao_guard: None,
        }
    }

    /// Like [`Self::execute_without_confirmation`], but for a node that
    /// rebuilds its target relation untracked, invalidating its cached epoch.
    fn execute_and_invalidate_freshness() -> Self {
        Self::Execute {
            after_success: RunCacheAfterSuccess::InvalidateFreshness,
            sao_guard: None,
        }
    }

    fn execute_with_confirmation(request_id: String, failed_to_clone: bool) -> Self {
        Self::Execute {
            after_success: RunCacheExecutionConfirmation::new(request_id, failed_to_clone)
                .map(RunCacheAfterSuccess::Confirm)
                .unwrap_or(RunCacheAfterSuccess::None),
            sao_guard: None,
        }
    }

    fn execute_with_record(record: RunCachePendingExecutionRecord) -> Self {
        Self::Execute {
            after_success: RunCacheAfterSuccess::Record(Box::new(record)),
            sao_guard: None,
        }
    }

    /// The authoritative SAO node hash, if this decision carries one.
    ///
    /// `Skip` carries the stored hash from a prior successful run; `Execute`
    /// with an `sao_guard` carries the hash the guard will write on
    /// finalize. Service-only outcomes (no guard / no stored hash) return
    /// `None` because the service is the source of truth in that mode.
    pub fn node_hash(&self) -> Option<String> {
        match self {
            Self::Skip {
                sao_stored_hash, ..
            } => sao_stored_hash.clone(),
            Self::Execute {
                sao_guard: Some(guard),
                ..
            } => Some(guard.node_hash().to_string()),
            _ => None,
        }
    }

    pub async fn finalize(self, ctx: &TaskRunnerCtx) -> FsResult<()> {
        match self {
            RunCacheServiceDecision::Execute {
                sao_guard: Some(guard),
                ..
            } => {
                let upstreams = collect_upstream_hashes(ctx, guard.unique_id());
                guard
                    .finalize(upstreams)
                    .await
                    .map_err(|e| fs_err!(ErrorCode::Generic, "stop_task failed: {}", e))
            }
            _ => Ok(()),
        }
    }
}

impl PartialEq for RunCacheServiceDecision {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                RunCacheServiceDecision::Execute {
                    after_success: a1, ..
                },
                RunCacheServiceDecision::Execute {
                    after_success: a2, ..
                },
            ) => a1 == a2,
            (
                RunCacheServiceDecision::Clone { clone: c1 },
                RunCacheServiceDecision::Clone { clone: c2 },
            ) => c1 == c2,
            (
                RunCacheServiceDecision::Skip {
                    status: s1,
                    sao_stored_hash: h1,
                    cached_test_result: r1,
                },
                RunCacheServiceDecision::Skip {
                    status: s2,
                    sao_stored_hash: h2,
                    cached_test_result: r2,
                },
            ) => s1 == s2 && h1 == h2 && r1 == r2,
            (RunCacheServiceDecision::Disabled, RunCacheServiceDecision::Disabled) => true,
            _ => false,
        }
    }
}

impl std::fmt::Debug for RunCacheServiceDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunCacheServiceDecision::Execute { after_success, .. } => f
                .debug_struct("RunCacheServiceDecision::Execute")
                .field("after_success", after_success)
                .finish(),
            RunCacheServiceDecision::Clone { clone } => f
                .debug_struct("RunCacheServiceDecision::Clone")
                .field("clone", clone)
                .finish(),
            RunCacheServiceDecision::Skip {
                status,
                sao_stored_hash,
                cached_test_result,
            } => f
                .debug_struct("RunCacheServiceDecision::Skip")
                .field("status", status)
                .field("sao_stored_hash", sao_stored_hash)
                .field("cached_test_result", cached_test_result)
                .finish(),
            RunCacheServiceDecision::Disabled => write!(f, "RunCacheServiceDecision::Disabled"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunCacheAfterSuccess {
    None,
    /// The node rebuilt its target without the service observing it — no
    /// decision was sought and no execution will be recorded. Its cached
    /// warehouse metadata now describes pre-build state, so drop it once the
    /// node succeeds.
    InvalidateFreshness,
    Confirm(RunCacheExecutionConfirmation),
    Record(Box<RunCachePendingExecutionRecord>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunCacheExecutionConfirmation {
    request_id: String,
    failed_to_clone: bool,
    execution_results: Option<Struct>,
    execution_runtime_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunCachePendingExecutionRecord {
    input: RunCachePendingExecutionInput,
    /// Set when the record originates from a speculative submit whose verdict
    /// was `ReadyToExecuteUntracked`. Such records were built from a partial
    /// (prefetch-in-flight) dependency snapshot, so their epochs are finalized
    /// against the warm cache before recording, and the `SqlExecution` carries
    /// `from_speculative_submit = true`.
    speculative: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum RunCachePendingExecutionInput {
    Sql(Box<SubmitEnrichedSqlRequest>),
    Values(Box<SubmitValuesRequest>),
}

impl RunCacheExecutionConfirmation {
    fn new(request_id: String, failed_to_clone: bool) -> Option<Self> {
        if request_id.is_empty() {
            None
        } else {
            Some(Self {
                request_id,
                failed_to_clone,
                execution_results: None,
                execution_runtime_ms: None,
            })
        }
    }

    fn with_execution_metadata(
        request_id: String,
        failed_to_clone: bool,
        execution_results: Option<Struct>,
        execution_runtime_ms: Option<i64>,
    ) -> Option<Self> {
        Self::new(request_id, failed_to_clone).map(|mut confirmation| {
            confirmation.execution_results = execution_results;
            confirmation.execution_runtime_ms = execution_runtime_ms;
            confirmation
        })
    }

    /// Attach a `{failures, should_warn, should_error}` payload to this
    /// confirmation. Used by the data-test Confirm path so subsequent runs
    /// can replay the cached verdict.
    pub fn set_test_execution_results(&mut self, result: CachedTestExecutionResult) {
        if self.execution_results.is_none() {
            self.execution_results = Some(build_test_execution_results_struct(result));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CachedTestExecutionResult {
    pub failures: i64,
    pub should_warn: bool,
    pub should_error: bool,
}

/// Build the data-test result payload sent in
/// `ConfirmExecutionRequest.execution_results` so subsequent runs can replay
/// the cached verdict.
pub fn build_test_execution_results_struct(result: CachedTestExecutionResult) -> Struct {
    let fail_value = Value {
        kind: Some(Kind::IntValue(result.failures)),
    };
    let mut fields = HashMap::new();
    fields.insert("failures".to_string(), fail_value);
    fields.insert(
        "should_warn".to_string(),
        Value {
            kind: Some(Kind::BoolValue(result.should_warn)),
        },
    );
    fields.insert(
        "should_error".to_string(),
        Value {
            kind: Some(Kind::BoolValue(result.should_error)),
        },
    );
    Struct { fields }
}

/// Decode the cached data-test result payload from a `SkipExecutionResponse`.
pub fn parse_cached_test_execution_result(
    response: &SkipExecutionResponse,
) -> Option<CachedTestExecutionResult> {
    let results = response.execution_results.as_ref()?;
    Some(CachedTestExecutionResult {
        failures: parse_execution_result_int(results.fields.get("failures")?)?,
        should_warn: parse_execution_result_bool(results.fields.get("should_warn")?)?,
        should_error: parse_execution_result_bool(results.fields.get("should_error")?)?,
    })
}

fn parse_execution_result_int(value: &Value) -> Option<i64> {
    match value.kind.as_ref()? {
        Kind::IntValue(i) => Some(*i),
        Kind::DoubleValue(d) => Some(*d as i64),
        _ => None,
    }
}

fn parse_execution_result_bool(value: &Value) -> Option<bool> {
    match value.kind.as_ref()? {
        Kind::BoolValue(value) => Some(*value),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunCacheCloneDecision {
    request_id: String,
    clone_sqls: Vec<String>,
    clone_source: String,
    clone_target: String,
    required_source_epoch: Option<i64>,
    execution_results: Option<Struct>,
    execution_runtime_ms: Option<i64>,
    freshness_tolerance_seconds: u64,
    explained_decision: Option<ExplainedDecision>,
    transformed_nodes_by_query: HashMap<String, NodeFuncMapping>,
    execution_decision_id: Option<String>,
}

impl RunCacheCloneDecision {
    pub fn from_response(
        response: &dbt_state::proto::query_cache::ReadyToCloneResponse,
        freshness_tolerance_seconds: i64,
    ) -> Self {
        Self {
            request_id: response.request_id.clone(),
            clone_sqls: response.clone_sqls.clone(),
            clone_source: response.clone_source.clone(),
            clone_target: response.clone_target.clone(),
            required_source_epoch: response.clone_required_last_modified_epoch,
            execution_results: response.clone_execution_results.clone(),
            execution_runtime_ms: response.execution_runtime_ms,
            freshness_tolerance_seconds: freshness_tolerance_seconds.max(0) as u64,
            explained_decision: response.explained_decision.clone(),
            transformed_nodes_by_query: response.transformed_nodes_by_query.clone(),
            execution_decision_id: response.execution_decision_id.clone(),
        }
    }

    pub fn success_confirmation(&self) -> Option<RunCacheExecutionConfirmation> {
        RunCacheExecutionConfirmation::with_execution_metadata(
            self.request_id.clone(),
            false,
            self.execution_results.clone(),
            self.execution_runtime_ms,
        )
    }

    pub fn fallback_confirmation(&self) -> Option<RunCacheExecutionConfirmation> {
        RunCacheExecutionConfirmation::new(self.request_id.clone(), true)
    }

    pub fn clone_source(&self) -> &str {
        &self.clone_source
    }

    pub fn clone_target(&self) -> &str {
        &self.clone_target
    }

    /// Project this decision into a serializable form for time-machine recording.
    ///
    /// Only carries the fields that actually drive clone execution/confirmation
    /// (see `execute_run_cache_service_clone`, `success_status`, `success_confirmation`);
    /// the full protobuf `execution_results`/`transformed_nodes_by_query` are
    /// telemetry-only and dropped, and `explained_decision` is reduced to the
    /// single `is_stale` flag consumers actually read.
    pub fn to_recorded(&self) -> dbt_adapter::time_machine::RecordedRunCacheCloneDecision {
        dbt_adapter::time_machine::RecordedRunCacheCloneDecision {
            request_id: self.request_id.clone(),
            clone_sqls: self.clone_sqls.clone(),
            clone_source: self.clone_source.clone(),
            clone_target: self.clone_target.clone(),
            required_source_epoch: self.required_source_epoch,
            execution_runtime_ms: self.execution_runtime_ms,
            freshness_tolerance_seconds: self.freshness_tolerance_seconds,
            is_stale: self
                .explained_decision
                .as_ref()
                .is_some_and(|decision| decision.is_stale),
            execution_decision_id: self.execution_decision_id.clone(),
        }
    }

    /// Reconstruct a decision from a time-machine recording during replay.
    ///
    /// `execution_results` and `transformed_nodes_by_query` are reset to their
    /// empty defaults since replay never confirms back to the live service
    /// (see `confirm_run_cache_service_execution`'s client-absent early return).
    pub fn from_recorded(
        recorded: dbt_adapter::time_machine::RecordedRunCacheCloneDecision,
    ) -> Self {
        Self {
            request_id: recorded.request_id,
            clone_sqls: recorded.clone_sqls,
            clone_source: recorded.clone_source,
            clone_target: recorded.clone_target,
            required_source_epoch: recorded.required_source_epoch,
            execution_results: None,
            execution_runtime_ms: recorded.execution_runtime_ms,
            freshness_tolerance_seconds: recorded.freshness_tolerance_seconds,
            explained_decision: Some(ExplainedDecision {
                is_stale: recorded.is_stale,
                ..Default::default()
            }),
            transformed_nodes_by_query: HashMap::new(),
            execution_decision_id: recorded.execution_decision_id,
        }
    }

    pub fn replay_status(&self) -> NodeStatus {
        if self
            .explained_decision
            .as_ref()
            .is_some_and(|decision| decision.is_stale)
        {
            NodeStatus::ReusedCloned(Some(self.freshness_tolerance_seconds))
        } else {
            NodeStatus::ReusedCloned(None)
        }
    }

    fn success_status(&self) -> NodeStatus {
        self.replay_status()
    }
}

pub fn record_run_cache_clone_decision(node_id: &str, clone: &RunCacheCloneDecision) {
    let Some(recorder) = dbt_adapter::time_machine::global_recorder() else {
        return;
    };
    recorder.record_run_cache_clone(node_id, clone.to_recorded());
}

pub fn record_dev_clone_decision(node_id: &str, clone: &RunCacheCloneDecision) {
    let Some(recorder) = dbt_adapter::time_machine::global_recorder() else {
        return;
    };
    recorder.record_dev_clone(node_id, clone.to_recorded());
}

pub fn replay_run_cache_clone_decision(node_id: &str) -> Option<RunCacheCloneDecision> {
    let replayer = dbt_adapter::time_machine::global_replayer()?;
    let event = replayer.get_run_cache_clone_event_for_phase(node_id, false)?;
    Some(RunCacheCloneDecision::from_recorded(event.clone.clone()))
}

pub fn replay_dev_clone_decision(node_id: &str) -> Option<RunCacheCloneDecision> {
    let replayer = dbt_adapter::time_machine::global_replayer()?;
    let event = replayer.get_run_cache_clone_event_for_phase(node_id, true)?;
    Some(RunCacheCloneDecision::from_recorded(event.clone.clone()))
}

/// If the node is a selected (non-deferred) view model, insert its
/// compiled SQL into the run-scoped traverser cache so downstream
/// models do not remotely fetch the view's DDL.
///
/// Deferred view models are not inserted — they are resolved via the
/// remote fetch performed by [`run_cache_service_before_run`].
/// Non-view materializations are no-ops (their compiled SQL is not
/// view DDL).
pub fn insert_compiled_view_definition(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    task_result: &TaskResult,
) {
    if !ctx.inner.run_cache_ctx.run_cache_service_requested {
        return;
    }
    let Some(traverser) = ctx.inner.run_cache_ctx.view_traverser.as_ref() else {
        return;
    };
    let Some(model) = node.as_any().downcast_ref::<DbtModel>() else {
        return;
    };
    if model.materialized() != DbtMaterialization::View {
        return;
    }
    let compiled_sql = task_result.sql_instruction.sql.as_str();
    if compiled_sql.is_empty() {
        return;
    }

    let adapter_type = node.node_adapter();
    let Ok(relation) = create_relation_from_node(adapter_type, node, None) else {
        return;
    };

    // Deferred nodes are resolved remotely by the start-of-run traversal.
    // Fail closed on canonical-fqn errors: without a cfqn we cannot rule
    // out that this node is deferred, and inserting a deferred view's
    // local compiled SQL would shadow the production definition for
    // downstream lookups keyed by `semantic_fqn`.
    let Ok(cfqn) = relation.get_canonical_fqn() else {
        emit_warn_log_message(
            ErrorCode::StateServiceWarn,
            format!(
                "Skipping compiled view definition insert for node {}: canonical FQN unavailable; cannot determine deferral status",
                node.unique_id()
            ),
        );
        return;
    };
    if ctx
        .inner
        .run_cache_ctx
        .run_cache_deferred_fqns
        .contains(&cfqn.to_string())
    {
        return;
    }

    let Some(dialect) = dialect_of(adapter_type) else {
        return;
    };
    // Derive default_catalog / default_schema by parsing the relation's
    // canonical FQN, mirroring what `fetch_view_definitions` does on the
    // adapter side. `node.database()` / `node.schema()` come from the
    // user's profile and may preserve lowercase, which on Snowflake then
    // produces quoted-lowercase synthetic relations downstream that don't
    // resolve in the warehouse — see `test_transitive_dependencies_tracked`.
    let fqn = relation.semantic_fqn();
    let (default_catalog, default_schema) = match dialect.parse_fqn(&fqn) {
        Ok(parsed) => (
            parsed.catalog().name().to_string(),
            parsed.schema().name().to_string(),
        ),
        Err(_) => (node.database(), node.schema()),
    };
    traverser.insert_view_definition(dbt_adapter::metadata::ViewDefinition {
        fqn,
        definition: compiled_sql.to_string(),
        dialect: adapter_type,
        default_catalog,
        default_schema,
    });
}

/// A clock bootstrapped once per run from a single warehouse SYSDATE() sample.
///
/// `now_ms` returns that sample plus local monotonic elapsed time, so every
/// call after the first is pure arithmetic — no additional warehouse queries.
/// Used in `confirm_run_cache_service_execution` to stamp freshly-executed
/// tables without re-querying `information_schema`.
///
/// `Instant::now()` is recorded *before* the warehouse query fires so that
/// the round-trip time (~50-100 ms) is baked into every subsequent `elapsed`
/// calculation, making `now_ms()` reliably land above `LAST_ALTERED` despite
/// small local-vs-warehouse clock skew on Snowflake/Redshift/BigQuery.
///
/// Databricks is intentionally excluded from the heuristic clock
/// (`heuristic_clock_enabled_for_adapter`).  DESCRIBE HISTORY timestamps have
/// millisecond precision and can land a few milliseconds above H due to clock
/// skew, causing the service to see a dependency as "changed" and fall back
/// to Execute.  For Databricks, confirms use the actual DESCRIBE HISTORY epoch
/// directly via `refresh_final_last_modified_epoch_for_node`, making the
/// stored and submitted epochs identical and the comparison exact.
#[derive(Debug)]
pub struct HeuristicClock {
    start_instant: Instant,
    start_ts_ms: i64,
}

impl HeuristicClock {
    /// Sample the warehouse clock and return a bootstrapped clock, or `None`
    /// if the adapter is not opted in or the query fails.
    pub async fn bootstrap(ctx: &TaskRunnerCtx) -> Option<Self> {
        if !heuristic_clock_enabled_for_adapter(ctx.default_adapter_type()) {
            return None;
        }
        let start_instant = Instant::now();
        let start_ts_ms = warehouse_now_ms(ctx).await?;
        Some(Self {
            start_instant,
            start_ts_ms,
        })
    }

    pub fn now_ms(&self) -> i64 {
        self.start_ts_ms + self.start_instant.elapsed().as_millis() as i64
    }
}

/// Query the warehouse for its current epoch-millisecond timestamp.
/// Returns `None` when the adapter is unsupported or the query fails.
async fn warehouse_now_ms(ctx: &TaskRunnerCtx) -> Option<i64> {
    let sql: &'static str = match ctx.default_adapter_type() {
        AdapterType::Snowflake => "SELECT DATE_PART('epoch_millisecond', SYSDATE())",
        AdapterType::Redshift => "SELECT (EXTRACT(EPOCH FROM GETDATE()) * 1000)::BIGINT",
        AdapterType::Databricks => "SELECT unix_millis(current_timestamp())",
        AdapterType::Bigquery => "SELECT UNIX_MILLIS(CURRENT_TIMESTAMP())",
        _ => return None,
    };
    let ctx_inner = ctx.clone();
    TaskOp::Blocking(Box::new(move || -> Option<i64> {
        let adapter = ctx_inner.env.get_adapter_ref()?;
        let query_ctx = QueryCtx::default().with_desc("dbt State run clock");
        let (_, table) = adapter
            .execute_without_state(Some(&query_ctx), sql, true, None)
            .ok()?;
        table.original_record_batch().first_value_as_i64()
    }))
    .run()
    .await
    .ok()
    .flatten()
}

/// Return `true` for adapters that opt into the heuristic clock.
fn heuristic_clock_enabled_for_adapter(adapter_type: AdapterType) -> bool {
    matches!(
        adapter_type,
        // Databricks is intentionally excluded: DESCRIBE HISTORY timestamps have
        // millisecond precision and can exceed H by a few ms due to local-vs-warehouse
        // clock skew, causing the service to misidentify deps as "changed". Databricks
        // confirms use the actual DESCRIBE HISTORY epoch directly instead, making the
        // stored and submitted epochs consistent.
        AdapterType::Snowflake | AdapterType::Redshift | AdapterType::Bigquery
    )
}

fn has_non_empty_schema(relation: &dyn BaseRelation) -> bool {
    relation
        .schema()
        .is_some_and(|schema| !schema.trim().is_empty())
}

pub(crate) fn has_metadata_address(adapter_type: AdapterType, relation: &dyn BaseRelation) -> bool {
    has_non_empty_schema(relation)
        && (adapter_type != AdapterType::Bigquery
            || relation
                .database()
                .is_some_and(|database| !database.trim().is_empty()))
}

/// Collect every relation and source freshness override needed for the run's
/// metadata prefetch.
///
/// Returns a map of `semantic_fqn → relation` covering every selected node's
/// own target relation plus the relations of all their runtime dependencies
/// (models, snapshots, seeds, sources). Ephemeral and inline models are skipped
/// because they never submit to the service, as are graph-only nodes with no
/// warehouse relation (see `has_non_empty_schema`). A second map carries
/// `FreshnessOverride` entries for sources that declare `loaded_at_query` or
/// `loaded_at_field`; these are passed through to `freshness_with_overrides`
/// so the prefetch uses the same freshness strategy as per-node submits.
///
/// Taking the individual components rather than a `&TaskRunnerCtx` keeps this
/// function unit-testable without a live adapter.
fn collect_global_prefetch_relations(
    adapter_type: AdapterType,
    runnable_set: &BTreeSet<String>,
    runtime_deps: &BTreeMap<String, BTreeSet<String>>,
    nodes: &dbt_schemas::schemas::Nodes,
) -> (
    BTreeMap<String, Arc<dyn BaseRelation>>,
    BTreeMap<String, FreshnessOverride>,
) {
    let mut relations: BTreeMap<String, Arc<dyn BaseRelation>> = BTreeMap::new();
    let mut overrides: BTreeMap<String, FreshnessOverride> = BTreeMap::new();

    for node_id in runnable_set {
        let Some(node) = nodes.get_node(node_id) else {
            continue;
        };
        if let Some(model) = node.as_any().downcast_ref::<DbtModel>() {
            if is_no_op_model_materialization(model.materialized()) {
                continue;
            }
        }

        if let Ok(relation) = create_relation_from_node(adapter_type, node, None)
            && has_metadata_address(adapter_type, relation.as_ref())
        {
            let fqn = relation.semantic_fqn();
            relations.entry(fqn).or_insert_with(|| relation.into());
        }

        let Some(dep_ids) = runtime_deps.get(node_id) else {
            continue;
        };
        for dep_id in dep_ids {
            let Some(dep_node) = nodes.get_node(dep_id) else {
                continue;
            };
            if !dep_node.as_any().is::<DbtModel>()
                && !dep_node.as_any().is::<DbtSnapshot>()
                && !dep_node.as_any().is::<DbtSeed>()
                && !dep_id.starts_with("source.")
            {
                continue;
            }
            let Ok(relation) = create_relation_from_node(adapter_type, dep_node, None) else {
                continue;
            };
            if !has_metadata_address(adapter_type, relation.as_ref()) {
                continue;
            }
            let fqn = relation.semantic_fqn();
            if let Some(source) = dep_node.as_any().downcast_ref::<DbtSource>()
                && let Some(kind) = source_freshness_override(source)
            {
                overrides.insert(fqn.clone(), kind);
            }
            relations.entry(fqn).or_insert_with(|| relation.into());
        }
    }

    (relations, overrides)
}

/// Render a `Query` override's `loaded_at_query` Jinja so macros / `{{ this }}`
/// expand before the SQL is sent; `Field` overrides pass through unchanged.
fn render_freshness_override(
    ovr: FreshnessOverride,
    relation: &Arc<dyn BaseRelation>,
    jinja_env: &JinjaEnv,
    base_context: &BTreeMap<String, minijinja::Value>,
) -> FsResult<FreshnessOverride> {
    let FreshnessOverride::Query(query) = ovr else {
        return Ok(ovr);
    };

    let mut render_context = base_context.clone();
    render_context.insert(
        "this".to_owned(),
        RelationObject::new(relation.clone()).into_value(),
    );
    let require = |part: Option<&str>, name: &str| -> FsResult<String> {
        part.map(str::to_owned).ok_or_else(|| {
            fs_err!(
                ErrorCode::Unexpected,
                "Cannot render loaded_at_query for source '{}': missing {name}",
                relation.semantic_fqn(),
            )
        })
    };
    // Database is optional because some adapters (e.g. BigQuery) render relations without one
    render_context.insert(
        "database".to_owned(),
        minijinja::Value::from(relation.database()),
    );
    render_context.insert(
        "schema".to_owned(),
        minijinja::Value::from(require(relation.schema(), "schema")?),
    );
    render_context.insert(
        "identifier".to_owned(),
        minijinja::Value::from(require(relation.identifier(), "identifier")?),
    );

    let rendered = jinja_env
        .render_named_str(&relation.semantic_fqn(), &query, &render_context, &[])
        .map_err(|e| FsError::from_jinja_err(e, "Failed to render source loaded_at_query"))?;
    Ok(FreshnessOverride::Query(rendered))
}

/// Batch-prefetch `last_modified_epoch` for all selected nodes and their
/// runtime dependencies, warming `run_cache_metadata` before any per-node
/// submit fires.
///
/// After this call, `last_modified_epoch_for_relation` returns a cache hit
/// for every relation in the prefetch set, eliminating the per-node warehouse
/// round-trips that `collect_table_modified_infos` would otherwise incur.
///
/// This runs in the background (see `run_cache_service_before_run`); a failure
/// no longer aborts the run. The error is propagated to the caller, which logs
/// it and lets the per-node paths fall back to their own freshness lookups.
async fn prefetch_global_last_modified_epochs(ctx: &TaskRunnerCtx) -> FsResult<()> {
    let (relations, overrides) = collect_global_prefetch_relations(
        ctx.default_adapter_type(),
        &ctx.inner.runnable_set,
        &ctx.inner.runtime_deps,
        ctx.nodes(),
    );
    if relations.is_empty() {
        return Ok(());
    }
    let mut rendered_overrides = BTreeMap::new();
    for (fqn, ovr) in overrides {
        let ovr = match relations.get(&fqn) {
            Some(relation) => {
                render_freshness_override(ovr, relation, ctx.env.as_ref(), &ctx.inner.base_context)?
            }
            None => ovr,
        };
        rendered_overrides.insert(fqn, ovr);
    }
    let started_at = Instant::now();
    let result = bulk_prefetch_last_modified_by_schema(ctx, &relations, &rendered_overrides).await;
    maybe_warn_slow_metadata_prefetch(ctx, started_at.elapsed());
    result
}

/// Total freshness-prefetch time above which we hint that a dedicated metadata
/// warehouse would help. Mirrors the dbt-core run-cache plugin's threshold.
const SLOW_METADATA_PREFETCH_WARN_THRESHOLD: std::time::Duration =
    std::time::Duration::from_secs(15);

/// Whether the slow-metadata-prefetch hint should fire.
///
/// The `metadata_warehouse` config (and this INFORMATION_SCHEMA-based fetch) is
/// Snowflake-specific, so the hint only applies there. It is pointless once a
/// dedicated warehouse is already configured, since setting it is the fix.
fn should_warn_slow_metadata_prefetch(
    adapter_type: AdapterType,
    options: &MetadataQueryOptions,
    elapsed: std::time::Duration,
) -> bool {
    let has_metadata_warehouse = options
        .warehouse
        .as_deref()
        .is_some_and(|warehouse| !warehouse.is_empty());
    adapter_type == AdapterType::Snowflake
        && !has_metadata_warehouse
        && elapsed >= SLOW_METADATA_PREFETCH_WARN_THRESHOLD
}

/// Hint (at most once per run) that configuring a dedicated metadata warehouse
/// would route these introspection queries to an isolated warehouse and run
/// them with better parallelism, when the freshness prefetch spent a long time
/// in INFORMATION_SCHEMA without one configured.
fn maybe_warn_slow_metadata_prefetch(ctx: &TaskRunnerCtx, elapsed: std::time::Duration) {
    let metadata_options = run_cache_metadata_query_options(ctx);
    if !should_warn_slow_metadata_prefetch(ctx.default_adapter_type(), &metadata_options, elapsed) {
        return;
    }
    emit_warn_log_message(
        ErrorCode::StateServiceWarn,
        format!(
            "Fetching table metadata (e.g., last modified timestamps) from INFORMATION_SCHEMA \
             took {:.1}s. Set the `metadata_warehouse` config to route these introspection \
             queries to a dedicated warehouse. This will lead to better parallelism and reduced \
             contention, resulting in these queries being executed significantly faster.",
            elapsed.as_secs_f64()
        ),
    );
}

/// Prefetch last-modified epochs for the selected relations and warm
/// `run_cache_metadata`.
///
/// Relations with source overrides (`loaded_at_query` / `loaded_at_field`) are
/// handled separately via the per-table path so their custom freshness logic is
/// preserved. Everything else is handed to the metadata adapter's
/// `freshness_all_in_schemas`, which owns the engine-specific strategy (per-schema
/// dumps, metadata-warehouse fan-out, adaptive broad-vs-sequential). Failures are
/// fail-open: a relation whose freshness could not be determined is cached as
/// unknown (`None`), never aborting the run.
async fn cache_unknown_last_modified_epochs(
    cache: &RunCacheMetadataCache,
    relations: &BTreeMap<String, Arc<dyn BaseRelation>>,
) {
    let prefetch_guards = begin_last_modified_prefetches(cache, relations).await;
    for (name, guard) in prefetch_guards {
        guard.insert_last_modified_epoch(name, None);
    }
}

async fn begin_last_modified_prefetches<'a>(
    cache: &'a RunCacheMetadataCache,
    relations: &BTreeMap<String, Arc<dyn BaseRelation>>,
) -> BTreeMap<String, MetadataPrefetchGuard<'a>> {
    let mut guards = relations
        .keys()
        .map(|name| (name.clone(), cache.begin_last_modified_prefetch(name)))
        .collect::<BTreeMap<_, _>>();
    for guard in guards.values_mut() {
        guard.acquire().await;
    }
    guards
}

async fn bulk_prefetch_last_modified_by_schema(
    ctx: &TaskRunnerCtx,
    relations: &BTreeMap<String, Arc<dyn BaseRelation>>,
    overrides: &BTreeMap<String, FreshnessOverride>,
) -> FsResult<()> {
    let cache = &ctx.inner.run_cache_ctx.run_cache_metadata;
    let Some(adapter) = ctx.env.get_adapter_ref() else {
        cache_unknown_last_modified_epochs(cache, relations).await;
        return Ok(());
    };
    let Some(metadata_adapter) = adapter.metadata_adapter() else {
        cache_unknown_last_modified_epochs(cache, relations).await;
        return Ok(());
    };

    let metadata_options = run_cache_metadata_query_options(ctx);

    // Separate relations with overrides — they need the per-table path so
    // their custom freshness queries (loaded_at_query / loaded_at_field) fire.
    let (override_relations, bulk_relations) = split_relations_by_override(relations, overrides);

    // Handle overrides via the existing per-table path.
    if !override_relations.is_empty() {
        refresh_last_modified_epochs(ctx, &override_relations, overrides).await?;
    }

    if bulk_relations.is_empty() {
        return Ok(());
    }

    let prefetch_guards = begin_last_modified_prefetches(cache, &bulk_relations).await;

    // A concurrent caller may have already resolved some of these relations
    // while this call waited on the per-relation locks above (each lock is
    // held until its holder finishes writing). Re-check the cache now that
    // every lock is acquired so this call neither re-queries the warehouse
    // nor clobbers an already-cached value with `None` on its own failure.
    let bulk_relations: BTreeMap<String, Arc<dyn BaseRelation>> = bulk_relations
        .into_iter()
        .filter(|(name, _)| cache.last_modified_epoch(name).is_none())
        .collect();
    if bulk_relations.is_empty() {
        return Ok(());
    }

    // `freshness_all_in_schemas` keys its result by `semantic_fqn`; keep the
    // mapping back to each relation's cache name (rendered relation name).
    let semantic_to_name: BTreeMap<String, String> = bulk_relations
        .iter()
        .map(|(name, rel)| (rel.semantic_fqn(), name.clone()))
        .collect();
    let relation_values: Vec<Arc<dyn BaseRelation>> = bulk_relations.values().cloned().collect();

    // The adapter owns the strategy and is fail-open per group internally; treat
    // a top-level error the same way (unknown freshness for the whole batch) so a
    // metadata failure never aborts the run.
    let freshness = match metadata_adapter
        .freshness_all_in_schemas(
            &relation_values,
            &metadata_options,
            adapter.cancellation_token(),
        )
        .await
    {
        Ok(freshness) => freshness,
        Err(Cancellable::Error(err)) if err.kind() == AdapterErrorKind::ReplayDataMissing => {
            // Older recordings may not contain the new schema-bulk event, but
            // they can contain the existing per-table freshness event.
            legacy_freshness(
                metadata_adapter.as_ref(),
                &relation_values,
                None,
                &metadata_options,
                adapter.cancellation_token(),
            )
            .await
            .map_err(into_fs_error)?
        }
        Err(Cancellable::Cancelled) => return Err(into_fs_error(Cancellable::Cancelled)),
        // A cancelled join/query can surface as `Cancellable::Error` with kind
        // `Cancelled` instead of `Cancellable::Cancelled` — treat it the same
        // way so cancellation always stops the run instead of fail-opening.
        Err(Cancellable::Error(err)) if err.kind() == AdapterErrorKind::Cancelled => {
            return Err(into_fs_error(Cancellable::Error(err)));
        }
        Err(err) => {
            let err = into_fs_error(err);
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State freshness prefetch failed for {} relations: {err}; caching unknown \
                     freshness",
                    semantic_to_name.len()
                ),
            );
            BTreeMap::new()
        }
    };

    for (semantic_fqn, name) in &semantic_to_name {
        let epoch = freshness
            .get(semantic_fqn)
            .map(|m| m.last_altered.timestamp_millis());
        if let Some(guard) = prefetch_guards.get(name) {
            guard.insert_last_modified_epoch(name, epoch);
        }
    }

    Ok(())
}

/// Start-of-run hook. Fires once before any model executes.
///
/// Starts a background task that batch-prefetches `last_modified_epoch` for all
/// selected nodes and their runtime dependencies into `run_cache_metadata`, so
/// per-node submits resolve freshness from the in-process cache instead of
/// issuing individual warehouse queries.
///
/// The prefetch runs concurrently with node execution rather than blocking the
/// whole run: while it is in flight, a node issues a speculative submit using
/// only the dependency epochs already resolved (see `submit_model` and friends).
/// A failed prefetch no longer aborts the run — it is logged and the per-node
/// paths fall back to their own freshness lookups.
///
/// Note: the view-definition traversal that was previously run here as a
/// pre-warm has been removed. It used `collect_view_traversal_roots` (a
/// dep-graph-based approach) to approximate which relations the per-node
/// SQL parser would find. For deferred models with `generate_alias_name`
/// macros this produces incorrect FQNs (prod schema + dev alias) that do
/// not exist in the warehouse, causing hard TABLE_NOT_FOUND failures.
///
/// The per-node path in `collect_query_dependencies` already handles view
/// traversal correctly by parsing compiled SQL directly (matching the
/// plugin's approach), and the traverser's shared cache means each view is
/// fetched at most once per run. Total IO is identical — only the timing
/// changes from eager/pre-run to lazy/per-node.
pub async fn run_cache_service_before_run(ctx: &TaskRunnerCtx) -> AdapterResult<()> {
    run_cache_service_start_telemetry(ctx).await;

    let prefetch = ctx.inner.run_cache_ctx.prefetch.clone();
    prefetch.mark_started();

    let prefetch_ctx = ctx.clone();
    let handle = dbt_common::tracing::spawn_traced(async move {
        if let Err(err) = prefetch_global_last_modified_epochs(&prefetch_ctx).await {
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State dependency last-modified prefetch failed: {err}; \
                     falling back to per-node freshness lookups"
                ),
            );
        }
        prefetch_ctx.inner.run_cache_ctx.prefetch.mark_done();
    });
    prefetch.set_handle(handle).await;

    if let Some(clock) = HeuristicClock::bootstrap(ctx).await {
        let _ = ctx.inner.run_cache_ctx.heuristic_clock.set(clock);
    }
    Ok(())
}

/// Whether the background dependency prefetch has completed.
///
/// Returns `false` before the prefetch is started and while it is in flight;
/// `true` once the background task has finished (or there was nothing to
/// prefetch). A node uses this to decide whether to issue a speculative submit
/// (prefetch still pending) or a regular one (prefetch already complete).
fn is_prefetch_ready(ctx: &TaskRunnerCtx) -> bool {
    let prefetch = &ctx.inner.run_cache_ctx.prefetch;
    prefetch.is_started() && prefetch.is_done()
}

/// Await the background dependency prefetch to completion.
///
/// Joins the background task once: the first call takes and awaits the handle,
/// and subsequent calls find no handle and return without waiting. When the
/// prefetch was never started (no handle was ever set) this also returns
/// immediately. In every case the call is fail-open — a join error or task
/// panic is logged and swallowed, and the prefetch is marked done so callers
/// can always proceed — so it never blocks the command from making progress.
async fn await_prefetch(ctx: &TaskRunnerCtx) {
    let prefetch = &ctx.inner.run_cache_ctx.prefetch;
    if let Err(err) = prefetch.join().await {
        emit_warn_log_message(
            ErrorCode::StateServiceWarn,
            format!(
                "dbt State dependency last-modified prefetch task did not complete cleanly: {err}; \
                 continuing"
            ),
        );
    }
    prefetch.mark_done();
}

pub async fn run_cache_service_start_telemetry(ctx: &TaskRunnerCtx) {
    submit_run_cache_session_start(ctx).await;
}

pub async fn run_cache_service_after_run(ctx: &TaskRunnerCtx, cancelled: bool) {
    let result = if cancelled {
        SessionEndResult::Cancelled
    } else if ctx
        .inner
        .run_stats
        .iter()
        .any(|stat| stat.status == NodeStatus::Errored)
        || ctx
            .inner
            .analyze_stats
            .iter()
            .any(|stat| stat.status == NodeStatus::Errored)
    {
        SessionEndResult::Failure
    } else {
        SessionEndResult::Success
    };
    submit_run_cache_session_end(ctx, result).await;
}

/// `cancelled` distinguishes a run aborted via the run's `CancellationToken`
/// (e.g. a user hitting Ctrl-C) from any other early-return failure, so the
/// dbt State service can tell the two apart in telemetry.
pub async fn run_cache_service_after_run_failed(ctx: &TaskRunnerCtx, cancelled: bool) {
    let result = if cancelled {
        SessionEndResult::Cancelled
    } else {
        SessionEndResult::Failure
    };
    submit_run_cache_session_end(ctx, result).await;
}

/// Number of telemetry events buffered before new events are dropped.
/// Matches the Python dbt State client's `TelemetryDispatcher` queue bound.
const TELEMETRY_QUEUE_CAPACITY: usize = 150;

/// Maximum number of events sent in a single `SubmitTelemetryBatch` RPC.
/// Matches the Python dbt State client's `TelemetryDispatcher` batch bound.
const TELEMETRY_MAX_BATCH_SIZE: usize = 50;

/// Longest a queued event waits before being flushed, absent a full batch.
/// Matches the Python dbt State client's `TelemetryDispatcher` emit interval.
const TELEMETRY_MAX_EMIT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Retries for a batch that failed with a retriable error before its events
/// are dropped. Matches the Python dbt State client's `TelemetryDispatcher`.
const TELEMETRY_MAX_RETRY_COUNT: u32 = 3;

type QueuedTelemetryEvent = (ClientTelemetryEvent, u32);

/// Background dispatcher that batches and submits dbt State telemetry events
/// without blocking node execution, mirroring the async batching design of
/// the Python dbt State client's telemetry dispatcher: events are pushed
/// onto a bounded channel (non-blocking, drop-on-full) and a single
/// background task flushes whatever is queued into one `SubmitTelemetryBatch`
/// RPC per batch, either once `TELEMETRY_MAX_BATCH_SIZE` events have
/// accumulated or `TELEMETRY_MAX_EMIT_INTERVAL` has elapsed, whichever comes
/// first. A batch that fails with a retriable error is re-queued (up to
/// `TELEMETRY_MAX_RETRY_COUNT` times per event); anything else is dropped.
///
/// `flush` closes the channel and awaits the worker so the run's final
/// events (in particular session-end) get a chance to be delivered before
/// the process exits.
pub struct TelemetryDispatcher {
    sender: tokio::sync::Mutex<Option<mpsc::Sender<QueuedTelemetryEvent>>>,
    worker: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl TelemetryDispatcher {
    fn spawn(client: dbt_state::service_client::SharedRunCacheServiceClient) -> Self {
        let (sender, receiver) = mpsc::channel(TELEMETRY_QUEUE_CAPACITY);
        let worker = dbt_common::tracing::spawn_traced(Self::run(client, receiver));
        Self {
            sender: tokio::sync::Mutex::new(Some(sender)),
            worker: tokio::sync::Mutex::new(Some(worker)),
        }
    }

    async fn run(
        client: dbt_state::service_client::SharedRunCacheServiceClient,
        mut receiver: mpsc::Receiver<QueuedTelemetryEvent>,
    ) {
        // Retries go straight back into `buffer`, not through the channel: a
        // `Sender` clone held by this task would stop `recv` from ever
        // returning `None`, hanging `flush()` forever.
        let mut buffer: Vec<QueuedTelemetryEvent> = Vec::new();
        let mut interval = tokio::time::interval(TELEMETRY_MAX_EMIT_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                event = receiver.recv() => {
                    let Some(event) = event else {
                        Self::flush_buffer(&client, &mut buffer).await;
                        break;
                    };
                    buffer.push(event);
                    // `==`, not `>=`: a requeued failure can leave `buffer`
                    // at the cap already, and `>=` would re-flush on every
                    // event after that with no gap between retries.
                    if buffer.len() == TELEMETRY_MAX_BATCH_SIZE {
                        Self::flush_buffer(&client, &mut buffer).await;
                    }
                }
                _ = interval.tick() => {
                    Self::flush_buffer(&client, &mut buffer).await;
                }
            }
        }
    }

    /// Drain `buffer` in chunks of at most `TELEMETRY_MAX_BATCH_SIZE`, one
    /// `SubmitTelemetryBatch` RPC per chunk, stopping at the first chunk that
    /// fails. Fail-open: a retriable failure re-queues that chunk (bounded by
    /// `TELEMETRY_MAX_RETRY_COUNT`); anything else drops it.
    async fn flush_buffer(
        client: &dbt_state::service_client::SharedRunCacheServiceClient,
        buffer: &mut Vec<QueuedTelemetryEvent>,
    ) {
        while !buffer.is_empty() {
            let chunk_size = buffer.len().min(TELEMETRY_MAX_BATCH_SIZE);
            let chunk: Vec<QueuedTelemetryEvent> = buffer.drain(..chunk_size).collect();
            let events = chunk.iter().map(|(event, _)| event.clone()).collect();
            let Err(err) = client.submit_telemetry_batch(telemetry_batch(events)).await else {
                continue;
            };
            if !err.is_retriable_telemetry_submission() {
                emit_trace_log_message(|| {
                    format!(
                        "dbt State service telemetry batch failed: {err}; dropping {} event(s)",
                        chunk.len()
                    )
                });
                return;
            }
            for (event, retry_count) in chunk {
                if retry_count >= TELEMETRY_MAX_RETRY_COUNT {
                    emit_trace_log_message(|| {
                        "dbt State telemetry event exceeded max retries; dropping".to_string()
                    });
                    continue;
                }
                buffer.push((event, retry_count + 1));
            }
            return;
        }
    }

    /// Enqueue an event without blocking on network I/O. Drops the event
    /// (fail-open) if the queue is full or the dispatcher has been flushed.
    async fn send(&self, event: ClientTelemetryEvent) {
        let guard = self.sender.lock().await;
        if let Some(sender) = guard.as_ref() {
            if sender.try_send((event, 0)).is_err() {
                emit_trace_log_message(|| {
                    "dbt State telemetry queue full; dropping event".to_string()
                });
            }
        }
    }

    /// Enqueue a critical event, waiting for capacity instead of dropping it.
    async fn send_critical(&self, event: ClientTelemetryEvent) {
        let sender = self.sender.lock().await.as_ref().cloned();
        if let Some(sender) = sender
            && sender.send((event, 0)).await.is_err()
        {
            emit_trace_log_message(|| {
                "dbt State telemetry dispatcher is closed; dropping critical event".to_string()
            });
        }
    }

    /// Close the queue and await the worker so already-queued events flush
    /// before the caller proceeds. Fail-open: a join error is swallowed.
    async fn flush(&self) {
        self.sender.lock().await.take();
        if let Some(handle) = self.worker.lock().await.take() {
            let _ = handle.await;
        }
    }
}

async fn submit_run_cache_session_end(ctx: &TaskRunnerCtx, result: SessionEndResult) {
    let run_cache_ctx = &ctx.inner.run_cache_ctx;
    if run_cache_ctx
        .telemetry_session_ended
        .swap(true, Ordering::Relaxed)
    {
        return;
    }

    let Some(start) = run_cache_ctx.telemetry_session_start.get() else {
        return;
    };
    let description = match result {
        SessionEndResult::Success => "completed",
        SessionEndResult::Failure => "completed with errors",
        SessionEndResult::Cancelled => "cancelled",
    };
    let event = session_end_event(
        start.elapsed(),
        result,
        description,
        next_telemetry_event_order(ctx),
    );
    submit_run_cache_critical_telemetry_event(ctx, event).await;
    flush_run_cache_telemetry(ctx).await;
}

async fn submit_run_cache_session_start(ctx: &TaskRunnerCtx) {
    let run_cache_ctx = &ctx.inner.run_cache_ctx;
    if run_cache_ctx
        .telemetry_session_start
        .set(Instant::now())
        .is_err()
    {
        return;
    }

    let Some(config) = run_cache_ctx.run_cache_service_config.as_ref() else {
        return;
    };
    let event = session_start_event(config.telemetry_config(), next_telemetry_event_order(ctx));
    submit_run_cache_telemetry_event(ctx, event).await;
}

async fn submit_run_cache_telemetry_event(ctx: &TaskRunnerCtx, event: ClientTelemetryEvent) {
    let Some(client) = ctx.inner.run_cache_ctx.run_cache_service_client.as_ref() else {
        return;
    };
    let dispatcher = ctx
        .inner
        .run_cache_ctx
        .telemetry_dispatcher
        .get_or_init(|| TelemetryDispatcher::spawn(client.clone()));
    dispatcher.send(event).await;
}

async fn submit_run_cache_critical_telemetry_event(
    ctx: &TaskRunnerCtx,
    event: ClientTelemetryEvent,
) {
    let Some(client) = ctx.inner.run_cache_ctx.run_cache_service_client.as_ref() else {
        return;
    };
    let dispatcher = ctx
        .inner
        .run_cache_ctx
        .telemetry_dispatcher
        .get_or_init(|| TelemetryDispatcher::spawn(client.clone()));
    dispatcher.send_critical(event).await;
}

/// Closes the run's telemetry dispatcher (if one was started) and awaits its
/// worker, giving already-queued events — in particular session-end — a
/// chance to reach the service before the command exits.
async fn flush_run_cache_telemetry(ctx: &TaskRunnerCtx) {
    if let Some(dispatcher) = ctx.inner.run_cache_ctx.telemetry_dispatcher.get() {
        dispatcher.flush().await;
    }
}

fn next_telemetry_event_order(ctx: &TaskRunnerCtx) -> i64 {
    ctx.inner
        .run_cache_ctx
        .telemetry_event_order
        .fetch_add(1, Ordering::Relaxed)
}

/// Submits a runnable node to the dbt State service before local execution.
///
/// The returned decision tells the caller either to skip execution with a reused
/// status, or to execute normally with an optional confirmation token to report
/// the final warehouse state after a successful run. All service and request
/// assembly failures are fail-open and return `Execute { after_success: None }`.
pub async fn run_cache_service_before_execution(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    task_result: &TaskResult,
    microbatch_window: Option<(DateTime<Utc>, DateTime<Utc>)>,
) -> RunCacheServiceDecision {
    if !ctx.inner.run_cache_ctx.run_cache_service_requested {
        emit_warn_log_message(
            ErrorCode::StateServiceWarn,
            format!(
                "dbt State service hook reached while service mode is disabled for node {}; executing normally",
                node.unique_id()
            ),
        );
        write_state_explain_node(ctx, node, None);
        return RunCacheServiceDecision::execute_without_confirmation();
    }

    let Some(client) = ctx.inner.run_cache_ctx.run_cache_service_client.as_ref() else {
        emit_warn_log_message(
            ErrorCode::StateServiceWarn,
            format!(
                "dbt State service was requested but no validated client is available for node {}; executing normally",
                node.unique_id()
            ),
        );
        write_state_explain_node(ctx, node, None);
        return RunCacheServiceDecision::execute_without_confirmation();
    };
    if client.is_disabled() {
        write_state_explain_node(ctx, node, None);
        return RunCacheServiceDecision::execute_without_confirmation();
    }

    if !should_honor_service_skip(ctx) {
        let result =
            prepare_write_only_execution_record(ctx, node, task_result, microbatch_window).await;
        write_state_explain_node(ctx, node, None);
        return match result {
            Ok(Some(record)) => RunCacheServiceDecision::execute_with_record(record),
            Ok(None) => {
                let unique_id = node.unique_id();
                emit_trace_log_message(|| {
                    format!(
                        "dbt State service record skipped for node {unique_id}; executing normally"
                    )
                });
                RunCacheServiceDecision::execute_and_invalidate_freshness()
            }
            Err(err) => {
                emit_warn_log_message(
                    ErrorCode::StateServiceWarn,
                    format!(
                        "dbt State service record preparation failed for node {}: {err}; executing normally",
                        node.unique_id()
                    ),
                );
                RunCacheServiceDecision::execute_and_invalidate_freshness()
            }
        };
    }

    let result = if let Some(model) = node.as_any().downcast_ref::<DbtModel>() {
        if is_no_op_model_materialization(model.materialized()) {
            let unique_id = node.unique_id();
            let materialization = model.materialized().to_string();
            emit_trace_log_message(|| {
                format!(
                    "dbt State service submit skipped for no-op model materialization (node {unique_id}, materialization {materialization})"
                )
            });
            write_state_explain_node(ctx, node, None);
            return RunCacheServiceDecision::execute_without_confirmation();
        }
        if model.common().language.as_deref() != Some("sql") {
            record_unsupported_node(node, "non-SQL model");
            write_state_explain_node(ctx, node, None);
            return RunCacheServiceDecision::execute_and_invalidate_freshness();
        }
        submit_model(ctx, model, task_result, client, microbatch_window).await
    } else if let Some(snapshot) = node.as_any().downcast_ref::<DbtSnapshot>() {
        submit_snapshot(ctx, snapshot, task_result, client).await
    } else if let Some(seed) = node.as_any().downcast_ref::<DbtSeed>() {
        submit_seed(ctx, seed, task_result, client).await
    } else if let Some(test) = node.as_any().downcast_ref::<DbtTest>() {
        submit_test(ctx, test, task_result, client).await
    } else {
        record_unsupported_node(node, "unsupported node type");
        write_state_explain_node(ctx, node, None);
        return RunCacheServiceDecision::execute_and_invalidate_freshness();
    };

    match result {
        Ok(Some(RunCacheSubmitResult::Outcome(outcome))) => {
            let decision = record_service_decision(
                node.unique_id().as_str(),
                &outcome.response,
                outcome.freshness_tolerance_seconds,
                should_honor_service_skip(ctx),
            );
            // A node that completed a dev clone earlier in this invocation should
            // surface as "Cloned from other environment" when the service decides
            // Skip, matching the dbt-core plugin's `_dev_cloned_nodes` mapping.
            let is_dev_cloned = ctx
                .inner
                .run_cache_ctx
                .run_cache_dev_cloned_nodes
                .contains_key(node.unique_id().as_str());
            let decision = relabel_skip_for_dev_cloned_node(is_dev_cloned, decision);
            let execution_decision_id =
                state_explain_execution_decision_id(Some(&outcome.response), &decision);
            // Surface the service-side execution decision id on the node's
            // run-phase evaluation span so successful executions can be
            // correlated with the dbt State decision that governed them.
            if let Some(id) = execution_decision_id.as_deref() {
                find_and_update_span_attrs(|attrs: &mut NodeEvaluated| {
                    attrs.state_decision_id = Some(id.to_string());
                });
            }
            write_state_explain_node(ctx, node, execution_decision_id);
            decision
        }
        Ok(Some(RunCacheSubmitResult::ExecuteUntracked(record))) => {
            // A speculative `ReadyToExecuteUntracked` verdict: execute now and
            // record the outcome after the fact. Never a Skip, so no dev-cloned
            // relabel applies. The verdict carries no execution decision id, so
            // the node only gets a local fallback explain record.
            write_state_explain_node(ctx, node, None);
            RunCacheServiceDecision::execute_with_record(record)
        }
        Ok(None) => {
            let unique_id = node.unique_id();
            emit_trace_log_message(|| {
                format!("dbt State service submit skipped for node {unique_id}; executing normally")
            });
            write_state_explain_node(ctx, node, None);
            // Reached by bails that cannot produce a sound record — an
            // unresolved microbatch window, incomplete dependency metadata, an
            // unsupported node type. The node still rebuilds its target, so the
            // prefetched epoch must not survive into downstream submissions.
            RunCacheServiceDecision::execute_and_invalidate_freshness()
        }
        Err(_) if client.is_disabled() => {
            write_state_explain_node(ctx, node, None);
            RunCacheServiceDecision::execute_without_confirmation()
        }
        Err(err) => {
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State service submit failed for node {}: {err}; executing normally",
                    node.unique_id()
                ),
            );
            write_state_explain_node(ctx, node, None);
            // The request failed, so the node rebuilds without tracking; its
            // cached epoch/existence (e.g. from the prefetch, already awaited by
            // the time a submit errors) is now stale. Evict it here so
            // downstream nodes in this run re-query fresh state instead of
            // reusing it — unlike the benign-skip path, which preserves a valid
            // prefetched epoch.
            evict_node_metadata_for_untracked_rebuild(ctx, node);
            RunCacheServiceDecision::execute_without_confirmation()
        }
    }
}

fn write_state_explain_node(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    execution_decision_id: Option<String>,
) {
    let Some(path) = ctx.inner.run_cache_ctx.state_explain_log_path.as_ref() else {
        return;
    };
    let record = StateExplainLogRecord::Node(StateExplainNode {
        node_unique_id: node.unique_id(),
        node_name: node.name(),
        node_info: state_explain_node_info(ctx, node),
        execution_decision_id,
    });
    if let Err(err) = append_state_explain_log_record(path, &record) {
        emit_warn_log_message(
            ErrorCode::StateServiceWarn,
            format!("Failed to write dbt State explain node record: {err}"),
        );
    }
}

fn state_explain_execution_decision_id(
    response: Option<&SubmitSqlResponse>,
    decision: &RunCacheServiceDecision,
) -> Option<String> {
    let response = response?;
    match decision {
        RunCacheServiceDecision::Execute {
            after_success: RunCacheAfterSuccess::Confirm(_),
            ..
        }
        | RunCacheServiceDecision::Clone { .. }
        | RunCacheServiceDecision::Skip { .. } => execution_decision_id_from_response(response),
        RunCacheServiceDecision::Execute { .. } | RunCacheServiceDecision::Disabled => None,
    }
}

fn state_explain_node_info(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
) -> StateExplainNodeInfo {
    let mut node_info =
        state_explain_node_info_for_parts(node.node_adapter(), ctx.inner.arg.full_refresh, node);
    node_info.dev_clone = ctx
        .inner
        .run_cache_ctx
        .run_cache_dev_cloned_nodes
        .get(node.unique_id().as_str())
        .map(|entry| entry.value().clone());
    node_info.deferrals = state_explain_deferrals(ctx, node);
    node_info
}

fn state_explain_deferrals(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
) -> HashMap<String, String> {
    let Some(defer_nodes) = ctx.defer_nodes() else {
        return HashMap::new();
    };

    node.base()
        .depends_on
        .nodes
        .iter()
        .filter_map(|dependency_id| {
            let deferred_node = defer_nodes.get_node(dependency_id)?;
            let deferred_fqn =
                create_relation_from_node(deferred_node.node_adapter(), deferred_node, None)
                    .ok()?
                    .semantic_fqn();
            if !ctx
                .inner
                .run_cache_ctx
                .run_cache_deferred_fqns
                .contains(&deferred_fqn)
            {
                return None;
            }

            let local_node = ctx.nodes().get_node(dependency_id)?;
            let local_fqn = create_relation_from_node(ctx.default_adapter_type(), local_node, None)
                .ok()?
                .semantic_fqn();
            Some((local_fqn, deferred_fqn))
        })
        .collect()
}

fn state_explain_node_info_for_parts(
    adapter_type: AdapterType,
    full_refresh: bool,
    node: &dyn InternalDbtNodeAttributes,
) -> StateExplainNodeInfo {
    let materialized = node.base().materialized.clone();
    // No-op materializations are inlined into their consumers, so they never get
    // a warehouse relation. Reporting one would name a table that cannot exist.
    let is_ephemeral = is_no_op_model_materialization(materialized.clone());
    let fqn = if is_ephemeral {
        String::new()
    } else {
        create_relation_from_node(adapter_type, node, None)
            .map(|relation| relation.semantic_fqn())
            .unwrap_or_default()
    };
    let is_full_refresh = if let Some(model) = node.as_any().downcast_ref::<DbtModel>() {
        effective_full_refresh(full_refresh, model.deprecated_config.full_refresh)
    } else if let Some(snapshot) = node.as_any().downcast_ref::<DbtSnapshot>() {
        effective_full_refresh(full_refresh, snapshot.deprecated_config.full_refresh)
    } else {
        false
    };

    StateExplainNodeInfo {
        fqn,
        node_resource_type: node.resource_type().as_static_ref().to_string(),
        is_view: matches!(
            materialized,
            DbtMaterialization::View | DbtMaterialization::MetricView
        ),
        is_table: matches!(
            materialized,
            DbtMaterialization::Table
                | DbtMaterialization::Incremental
                | DbtMaterialization::Snapshot
                | DbtMaterialization::Seed
        ),
        is_ephemeral,
        is_incremental_or_snapshot: matches!(
            materialized,
            DbtMaterialization::Incremental | DbtMaterialization::Snapshot
        ),
        is_full_refresh,
        dev_clone: None,
        deferrals: HashMap::new(),
    }
}

/// Confirms a successful local execution back to the dbt State service.
///
/// Confirmation is best-effort: if no confirmation token was returned by the
/// pre-execution submit, final metadata is unavailable, or the service RPC
/// fails, the dbt command remains successful and the failure is only logged.
pub async fn confirm_run_cache_service_execution(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    confirmation: Option<RunCacheExecutionConfirmation>,
    execution_runtime_ms: Option<i64>,
) {
    // Without a confirmation token there is nothing to report, so avoid the
    // final metadata refresh.
    let Some(confirmation) = confirmation else {
        return;
    };

    let is_test = node.unique_id().starts_with("test.");
    // Data tests submit with `target_table=None`. The service's DB CHECK
    // `execution_last_modified_epoch_target_table_check` requires
    // `last_modified_epoch=NULL` whenever `target_table=NULL`, so never send
    // the audit relation's epoch on confirm — including when it does exist
    // (the `store_failures_as=table/view` case). Skipping the warehouse
    // lookup also avoids unnecessary work for `store_failures_as=None`
    // where there is no audit relation to query.
    let final_last_modified_epoch = if is_test {
        None
    } else {
        match final_last_modified_epoch_for_node(ctx, node).await {
            Ok(epoch) => epoch,
            Err(err) => {
                emit_warn_log_message(
                    ErrorCode::StateServiceWarn,
                    format!(
                        "dbt State service final metadata lookup failed for node {}: {err}; command remains successful",
                        node.unique_id()
                    ),
                );
                return;
            }
        }
    };

    // Replay must consume the recorded final freshness event and clear the
    // cached pre-build value, but it must not contact the live dbt State service.
    if dbt_adapter::time_machine::is_replaying() {
        return;
    }

    let Some(client) = ctx.inner.run_cache_ctx.run_cache_service_client.as_ref() else {
        emit_warn_log_message(
            ErrorCode::StateServiceWarn,
            format!(
                "dbt State service confirmation skipped because no validated client is available (node {}, request_id {})",
                node.unique_id(),
                confirmation.request_id
            ),
        );
        return;
    };
    let request = match confirmation
        .into_confirm_execution_request(node, final_last_modified_epoch, execution_runtime_ms)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => return,
        Err(err) => {
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State service confirmation metadata lookup failed for node {}: {err}; command remains successful",
                    node.unique_id()
                ),
            );
            return;
        }
    };

    let request_id = request.request_id.clone();
    if let Err(err) = client.confirm_execution(request).await {
        let unique_id = node.unique_id();
        match err {
            RunCacheServiceError::Disabled => {}
            err if err.is_transient_transport_rpc() => {
                emit_trace_log_message(|| {
                    format!(
                        "dbt State service confirmation transport failed for node {unique_id} (request_id {request_id}): {err}; command remains successful"
                    )
                });
            }
            err => {
                emit_warn_log_message(
                    ErrorCode::StateServiceWarn,
                    format!(
                        "dbt State service confirmation failed for node {unique_id} (request_id {request_id}): {err}; command remains successful"
                    ),
                );
            }
        }
    } else {
        let unique_id = node.unique_id();
        emit_trace_log_message(|| {
            format!(
                "dbt State service execution confirmed for node {unique_id} (request_id {request_id})"
            )
        });
    }
}

/// Records a successful local execution directly through the dbt State service.
///
/// Recording is best-effort and only used by write-only mode, where dbt State
/// lookup must be bypassed entirely. Missing final metadata or RPC failures are
/// logged and do not change dbt command success.
pub async fn record_run_cache_service_execution(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    record: Option<RunCachePendingExecutionRecord>,
    execution_runtime_ms: Option<i64>,
) {
    let Some(record) = record else {
        return;
    };

    let Some(client) = ctx.inner.run_cache_ctx.run_cache_service_client.as_ref() else {
        emit_warn_log_message(
            ErrorCode::StateServiceWarn,
            format!(
                "dbt State service record skipped for node {} because no validated client is available",
                node.unique_id()
            ),
        );
        return;
    };
    let request = match record
        .into_record_executions_request(ctx, node, execution_runtime_ms)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => return,
        Err(err) => {
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State service record metadata lookup failed for node {}: {err}; command remains successful",
                    node.unique_id()
                ),
            );
            return;
        }
    };

    if let Err(err) = client.record_executions(request).await {
        let unique_id = node.unique_id();
        match err {
            RunCacheServiceError::Disabled => {}
            err if err.is_transient_transport_rpc() => {
                emit_trace_log_message(|| {
                    format!(
                        "dbt State service record transport failed for node {unique_id}: {err}; command remains successful"
                    )
                });
            }
            err => {
                emit_warn_log_message(
                    ErrorCode::StateServiceWarn,
                    format!(
                        "dbt State service record failed for node {unique_id}: {err}; command remains successful"
                    ),
                );
            }
        }
    } else {
        let unique_id = node.unique_id();
        emit_trace_log_message(|| {
            format!("dbt State service execution recorded for node {unique_id}")
        });
    }
}

pub async fn execute_run_cache_service_clone(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    clone: &RunCacheCloneDecision,
    hook_executor: Option<RunCacheReuseHookExecutor>,
    pre_hooks_configured: bool,
) -> FsResult<NodeStatus, RunCacheCloneError> {
    verify_clone_source_freshness(ctx, node, clone)
        .await
        .map_err(RunCacheCloneError::Recoverable)?;
    if clone.clone_sqls.is_empty() {
        return Err(RunCacheCloneError::Recoverable(fs_err!(
            ErrorCode::Generic,
            "dbt State service clone response did not include clone SQL"
        )));
    }

    let target_relation = create_relation_from_node(node.node_adapter(), node, None)
        .map_err(RunCacheCloneError::Fatal)?;
    let target_relation: Arc<dyn BaseRelation> = target_relation.into();

    let mut clone_sqls = clone.clone_sqls.clone();
    let target_is_view = target_relation_is_view(ctx, &target_relation).await;
    if let Some(drop_sql) = drop_stale_view_sql(
        node.materialized(),
        target_is_view,
        &target_relation.semantic_fqn(),
    ) {
        clone_sqls.insert(0, drop_sql);
    }
    let node_unique_id = node.unique_id();
    let node_adapter_type = node.node_adapter();
    let ctx_inner = ctx.clone();
    let target_relation: Arc<dyn BaseRelation> =
        relation_from_rendered_name(node, &clone.clone_target)
            .map_err(RunCacheCloneError::Fatal)?;
    let drop_target_relation = target_relation.clone();
    let clone_result = TaskOp::Blocking(Box::new(move || {
        if let Some(hook_executor) = &hook_executor {
            hook_executor(&ctx_inner, RunCacheReuseHookPhase::Pre)
                .map_err(RunCacheCloneError::Fatal)?;
        }

        // Drop any relation currently occupying the clone target before
        // running the clone SQL. `create ... clone` fails when the target
        // already exists as a different relation type (for example a table
        // cloned over a view)
        drop_clone_target_before_clone(&ctx_inner, drop_target_relation);

        execute_clone_sqls_blocking(&ctx_inner, node_adapter_type, &node_unique_id, &clone_sqls)
            .map_err(|err| {
                if pre_hooks_configured {
                    RunCacheCloneError::Fatal(err)
                } else {
                    RunCacheCloneError::Recoverable(err)
                }
            })?;

        if let Some(hook_executor) = &hook_executor {
            hook_executor(&ctx_inner, RunCacheReuseHookPhase::Post)
                .map_err(RunCacheCloneError::Fatal)?;
        }
        Ok(())
    }))
    .run()
    .await
    .map_err(RunCacheCloneError::Recoverable)?;
    clone_result?;

    ctx.inner
        .run_cache_ctx
        .run_cache_metadata
        .invalidate_relation_metadata(&target_relation.semantic_fqn());
    cache_cloned_relation(ctx, node).map_err(RunCacheCloneError::Fatal)?;
    Ok(clone.success_status())
}

/// Clone DDL can't replace a relation of a different object kind, so a stale VIEW left at
/// the target by a prior materialization must be dropped first.
fn drop_stale_view_sql(
    materialized: DbtMaterialization,
    target_is_view: bool,
    target_relation: &str,
) -> Option<String> {
    (target_is_view && materialized != DbtMaterialization::View)
        .then(|| format!("drop view if exists {target_relation}"))
}

/// Fail-open on lookup errors. Uses `list_relations_in_parallel` (same primitive as
/// `relations_exist`) rather than `freshness`, which some adapters (e.g. Redshift) can't
/// reliably report for views since it depends on a last-altered timestamp.
async fn target_relation_is_view(
    ctx: &TaskRunnerCtx,
    target_relation: &Arc<dyn BaseRelation>,
) -> bool {
    let Some(adapter) = ctx.env.get_adapter_ref() else {
        return false;
    };
    let Some(metadata_adapter) = adapter.metadata_adapter() else {
        return false;
    };
    let semantic_fqn = target_relation.semantic_fqn();
    let catalog_schema = CatalogAndSchema::from(target_relation);
    let db_schemas = [catalog_schema.clone()];
    let Ok(listed) = metadata_adapter
        .list_relations_in_parallel(&db_schemas, adapter.cancellation_token())
        .await
    else {
        return false;
    };
    let Some(Ok(schema_relations)) = listed.get(&catalog_schema) else {
        return false;
    };
    schema_relations
        .iter()
        .find(|candidate| candidate.semantic_fqn() == semantic_fqn)
        .and_then(|relation| relation.relation_type())
        == Some(RelationType::View)
}

pub enum RunCacheCloneError {
    Recoverable(Box<FsError>),
    Fatal(Box<FsError>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunCacheReuseHookPhase {
    Pre,
    Post,
}

pub type RunCacheReuseHookExecutor =
    Arc<dyn Fn(&TaskRunnerCtx, RunCacheReuseHookPhase) -> FsResult<()> + Send + Sync>;

impl RunCacheCloneError {
    pub fn into_error(self) -> Box<FsError> {
        match self {
            Self::Recoverable(err) | Self::Fatal(err) => err,
        }
    }
}

impl std::fmt::Display for RunCacheCloneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Recoverable(err) | Self::Fatal(err) => err.fmt(f),
        }
    }
}

struct RunCacheSubmitOutcome {
    response: SubmitSqlResponse,
    /// Freshness tolerance window (seconds) that Fusion sent with the request.
    /// Used to format the "within lag tolerance of …" message when the
    /// service admits a candidate despite a stale upstream. Echoing the local
    /// value avoids a proto round-trip — the service already evaluates against
    /// the same number.
    freshness_tolerance_seconds: i64,
}

/// Outcome of a per-node submit helper.
///
/// Most submits produce a service `response` fed to `record_service_decision`.
/// The speculative `ReadyToExecuteUntracked` verdict has no counterpart there:
/// the node executes now and its outcome is recorded after the fact, so the
/// helper hands back the pending record directly. It is never a Skip/Clone, so
/// it bypasses both the decision translation and the dev-cloned relabel.
enum RunCacheSubmitResult {
    // Boxed: `RunCacheSubmitOutcome` carries a full `SubmitSqlResponse`, far
    // larger than the `ExecuteUntracked` variant, so box it to keep the enum small.
    Outcome(Box<RunCacheSubmitOutcome>),
    ExecuteUntracked(RunCachePendingExecutionRecord),
}

impl RunCacheSubmitResult {
    fn outcome(response: SubmitSqlResponse, freshness_tolerance_seconds: i64) -> Self {
        Self::Outcome(Box::new(RunCacheSubmitOutcome {
            response,
            freshness_tolerance_seconds,
        }))
    }
}

impl RunCacheExecutionConfirmation {
    async fn into_confirm_execution_request(
        self,
        node: &dyn InternalDbtNodeAttributes,
        last_modified_epoch: Option<i64>,
        execution_runtime_ms: Option<i64>,
    ) -> FsResult<Option<ConfirmExecutionRequest>> {
        // Data tests submit with `target_table=None` and the caller always
        // passes `last_modified_epoch=None` for them (the service's DB CHECK
        // `execution_last_modified_epoch_target_table_check` requires
        // `last_modified_epoch=NULL` whenever `target_table=NULL`), so let
        // test confirms through with `None` rather than skipping them — the
        // service still needs to record the execution to serve future Skips.
        let is_test = node.unique_id().starts_with("test.");
        if last_modified_epoch.is_none() && !is_test {
            let unique_id = node.unique_id();
            let request_id = self.request_id.clone();
            emit_trace_log_message(|| {
                format!(
                    "dbt State service confirmation skipped because final last-modified metadata is unavailable (node {unique_id}, request_id {request_id})"
                )
            });
            return Ok(None);
        }

        Ok(Some(ConfirmExecutionRequest {
            request_id: self.request_id,
            last_modified_epoch,
            failed_to_clone: self.failed_to_clone,
            table_type: table_type_for_node(node).await?,
            execution_results: self.execution_results,
            execution_runtime_ms: self.execution_runtime_ms.or(execution_runtime_ms),
            labels: node_identity(node).labels(),
        }))
    }
}

impl RunCachePendingExecutionRecord {
    fn sql(request: SubmitEnrichedSqlRequest) -> Self {
        Self {
            input: RunCachePendingExecutionInput::Sql(Box::new(request)),
            speculative: false,
        }
    }

    fn sql_speculative(request: SubmitEnrichedSqlRequest) -> Self {
        Self {
            input: RunCachePendingExecutionInput::Sql(Box::new(request)),
            speculative: true,
        }
    }

    fn values(request: SubmitValuesRequest) -> Self {
        Self {
            input: RunCachePendingExecutionInput::Values(Box::new(request)),
            speculative: false,
        }
    }

    async fn into_record_executions_request(
        self,
        ctx: &TaskRunnerCtx,
        node: &dyn InternalDbtNodeAttributes,
        execution_runtime_ms: Option<i64>,
    ) -> FsResult<Option<RecordExecutionsRequest>> {
        let speculative = self.speculative;

        // A speculative record was built from a partial dependency snapshot
        // while the prefetch was still in flight. Finalize the dependency epochs
        // against the now-warm cache before recording, so the persisted record
        // reflects the real upstream state rather than the speculative one.
        let input = match self.input {
            RunCachePendingExecutionInput::Sql(request) if speculative => {
                RunCachePendingExecutionInput::Sql(Box::new(
                    finalize_speculative_sql_request(ctx, node, *request).await,
                ))
            }
            other => other,
        };

        let last_modified_epoch = final_last_modified_epoch_for_node(ctx, node).await?;
        let Some(last_modified_epoch) = last_modified_epoch else {
            let unique_id = node.unique_id();
            emit_trace_log_message(|| {
                format!(
                    "dbt State service record skipped for node {unique_id} because final last-modified metadata is unavailable"
                )
            });
            return Ok(None);
        };

        let outcome = ExecutionOutcomeInput {
            last_modified_epoch: Some(last_modified_epoch),
            table_type: table_type_for_node(node).await?,
            execution_runtime_ms,
        };
        let record = match input {
            RunCachePendingExecutionInput::Sql(request) => {
                sql_execution_record_from_submit_request(*request, outcome, speculative)
            }
            RunCachePendingExecutionInput::Values(request) => {
                values_execution_record_from_submit_request(*request, outcome)
            }
        };

        Ok(Some(RecordExecutionsRequest {
            records: vec![record],
        }))
    }
}

/// Finalize the dependency last-modified epochs on a speculative SQL request.
///
/// A speculative request is built from cache reads only while the global
/// prefetch is still running, so some dependency epochs may be unset. Before the
/// outcome is recorded, this refreshes those epochs to their real values so the
/// persisted record reflects the true upstream state rather than the partial
/// speculative snapshot:
/// 1. await the global prefetch so the cache is fully warm;
/// 2. reconstruct a relation for each recorded table name and re-fetch any
///    epochs still missing from the cache, using the node's source-freshness
///    overrides so sources keep their custom freshness strategy;
/// 3. re-read the cache per table name, using the resolved entry whenever the
///    relation is present — including an explicit resolved-unknown, which clears
///    the epoch to unset — and falling back to the existing speculative epoch
///    only on a genuine cache miss.
///
/// Table names are canonical `semantic_fqn` strings, the same key the metadata
/// cache and the miss-refresh planner use.
async fn finalize_speculative_sql_request(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    mut request: SubmitEnrichedSqlRequest,
) -> SubmitEnrichedSqlRequest {
    await_prefetch(ctx).await;
    if request.tables.is_empty() {
        return request;
    }

    // Reconstruct a relation per recorded table name so the miss-refresh can
    // issue real warehouse lookups for anything the global prefetch didn't
    // resolve (e.g. parser-discovered raw references outside the DAG).
    let mut relations: BTreeMap<String, Arc<dyn BaseRelation>> = BTreeMap::new();
    for table in &request.tables {
        if let Ok(relation) = relation_from_rendered_name(node, &table.name) {
            relations.insert(table.name.clone(), relation);
        }
    }

    let overrides = collect_source_freshness_overrides(ctx, node);
    prefetch_last_modified_epochs(ctx, &relations, &overrides).await;

    for table in &mut request.tables {
        // `last_modified_epoch` is `Some(resolved)` when the cache has an entry
        // for the relation (`resolved` may be `None` — a resolved-unknown that
        // must clear the speculative epoch to unset) and `None` on a genuine
        // miss. Only a genuine miss keeps the best-effort speculative value.
        if let Some(resolved) = ctx
            .inner
            .run_cache_ctx
            .run_cache_metadata
            .last_modified_epoch(&table.name)
        {
            table.last_modified_epoch = resolved;
        }
    }
    request
}

/// Collect source-freshness overrides for a node's runtime dependencies.
///
/// Mirrors the override collection in `collect_table_modified_infos`: sources
/// declaring `loaded_at_query` or `loaded_at_field` get a `FreshnessOverride`
/// keyed by the relation's `semantic_fqn` — the key `freshness_with_overrides`
/// and the miss-refresh planner expect. Query takes precedence over field
/// (matching the dbt-core plugin); empty strings from the deserializer are
/// treated as "not set".
fn collect_source_freshness_overrides(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
) -> BTreeMap<String, FreshnessOverride> {
    let mut overrides: BTreeMap<String, FreshnessOverride> = BTreeMap::new();
    let Some(deps) = ctx.inner.runtime_deps.get(node.unique_id().as_str()) else {
        return overrides;
    };
    for dep_id in deps {
        let Some(dep_node) = ctx.nodes().get_node(dep_id) else {
            continue;
        };
        let Some(source) = dep_node.as_any().downcast_ref::<DbtSource>() else {
            continue;
        };
        let Ok((name, _)) = relation_for_node(dep_node) else {
            continue;
        };
        if let Some(kind) = source_freshness_override(source) {
            overrides.insert(name, kind);
        }
    }
    overrides
}

/// The source-freshness override a source declares via `loaded_at_query` /
/// `loaded_at_field`, if any. Query takes precedence over field (matching the
/// dbt-core plugin); empty strings from the deserializer are treated as unset.
fn source_freshness_override(source: &DbtSource) -> Option<FreshnessOverride> {
    let trimmed_nonempty = |s: &str| {
        let t = s.trim();
        (!t.is_empty()).then(|| t.to_string())
    };
    source
        .__source_attr__
        .loaded_at_query
        .as_deref()
        .and_then(trimmed_nonempty)
        .map(FreshnessOverride::Query)
        .or_else(|| {
            source
                .__source_attr__
                .loaded_at_field
                .as_deref()
                .and_then(trimmed_nonempty)
                .map(FreshnessOverride::Field)
        })
}

/// Guard against cloning from a source that was modified out-of-band since the
/// service issued its Clone decision.
///
/// Mirrors the plugin's check (run_cache.py lines 776-792): abort if the
/// source's current epoch is **greater than** the required epoch, meaning an
/// external write happened after the service recorded the state. Equality and
/// less-than are both fine — equal means nothing changed; less-than covers the
/// case where the confirmed epoch was a heuristic that landed slightly above the
/// true `LAST_ALTERED` value.
async fn verify_clone_source_freshness(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    clone: &RunCacheCloneDecision,
) -> FsResult<()> {
    let Some(required_epoch) = clone.required_source_epoch else {
        return Ok(());
    };
    if clone.clone_source.is_empty() {
        return Err(fs_err!(
            ErrorCode::Generic,
            "dbt State service clone response requires source freshness verification but clone_source is empty"
        ));
    }

    let source_relation = relation_from_rendered_name(node, &clone.clone_source)?;
    ctx.inner
        .run_cache_ctx
        .run_cache_metadata
        .remove_last_modified_epoch(&clone.clone_source);
    let actual_epoch =
        refresh_last_modified_epoch_for_relation(ctx, &clone.clone_source, source_relation).await?;
    match actual_epoch {
        Some(actual_epoch) if actual_epoch <= required_epoch => Ok(()),
        Some(actual_epoch) => Err(fs_err!(
            ErrorCode::Generic,
            "dbt State service clone source was modified since the cache decision for {}: \
             required epoch <= {}, found {}; falling back to execution",
            clone.clone_source,
            required_epoch,
            actual_epoch
        )),
        None => Err(fs_err!(
            ErrorCode::Generic,
            "dbt State service clone source freshness unavailable for {}",
            clone.clone_source
        )),
    }
}

fn relation_from_rendered_name(
    node: &dyn InternalDbtNodeAttributes,
    rendered_name: &str,
) -> FsResult<Arc<dyn BaseRelation>> {
    let Some(dialect) = dialect_of(node.node_adapter()) else {
        return Err(fs_err!(
            ErrorCode::Generic,
            "dbt State service clone source parsing is unsupported for adapter {:?}",
            node.node_adapter()
        ));
    };
    let fqn = FullyQualifiedName::parse(rendered_name, dialect).map_err(|err| {
        fs_err!(
            ErrorCode::Generic,
            "Failed to parse dbt State clone relation {}: {}",
            rendered_name,
            err
        )
    })?;
    Ok(create_relation(
        node.node_adapter(),
        fqn.catalog().to_value(),
        fqn.schema().to_value(),
        Some(fqn.table().to_value()),
        None,
        node.quoting(),
    )?
    .into())
}

/// Drop any relation currently occupying the clone target before the clone SQL
/// runs. Mirrors the dbt State Python client (query-cache PR #1092): a
/// `create ... clone` fails when the target already exists as a different
/// relation type (for example a table cloned over a view). Reads the existing
/// relation (with its true type) from the adapter's relation cache — the same
/// source dbt's `adapter.get_relation` consults — and drops it via the adapter,
/// which also evicts the cascade cache entry.
///
/// Best-effort: if the target is absent, untyped, or the drop fails, this logs
/// and returns so the clone still runs and the existing recoverable/fatal
/// handling applies unchanged.
fn drop_clone_target_before_clone(ctx: &TaskRunnerCtx, target_relation: Arc<dyn BaseRelation>) {
    let Ok(adapter) = ctx.adapter_store().get(target_relation.adapter_type()) else {
        return;
    };

    let jinja_state = ctx
        .env
        .new_state_with_context(ctx.inner.base_context.clone());

    let (Some(database), Some(schema), Some(identifier)) = (
        target_relation.database(),
        target_relation.schema(),
        target_relation.identifier(),
    ) else {
        return;
    };

    let maybe_existing = match adapter.get_relation(
        &jinja_state,
        database,
        schema,
        identifier,
        false,
    ) {
        Ok(value) => value.downcast_object::<RelationObject>().map(|r| r.inner()),
        Err(err) => {
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State clone could not inspect the existing relation at target {} before cloning: {err}; attempting the clone anyway",
                    target_relation.semantic_fqn()
                ),
            );
            return;
        }
    };

    let Some(existing) = maybe_existing else {
        return;
    };

    if matches!(existing.relation_type(), Some(rt) if rt == RelationType::Table) {
        // if the existing relation is already a table; dont drop it
        // the server clone_sql's already are the equivalent of `CREATE OR REPLACE TABLE.. CLONE..`
        // so we can avoid the overhead of an extra drop call
        return;
    }

    let args = [RelationObject::new(existing).into_value()];
    if let Err(err) = adapter.drop_relation(&jinja_state, &args) {
        let target = target_relation.semantic_fqn();
        emit_warn_log_message(
            ErrorCode::StateServiceWarn,
            format!(
                "dbt State clone could not drop the existing relation at target {target} before cloning: {err}; attempting the clone anyway"
            ),
        );
    }
}

fn execute_clone_sqls_blocking(
    ctx: &TaskRunnerCtx,
    adapter_type: AdapterType,
    node_unique_id: &str,
    clone_sqls: &[String],
) -> FsResult<()> {
    let Ok(adapter) = ctx.adapter_store().get(adapter_type) else {
        return Err(fs_err!(
            ErrorCode::Generic,
            "dbt State service clone cannot execute because no adapter is available"
        ));
    };
    let query_ctx = QueryCtx::default()
        .with_node_id(node_unique_id.to_string())
        .with_desc("dbt State clone");
    for sql in clone_sqls {
        adapter
            .execute_without_state(Some(&query_ctx), sql, false, None)
            .map_err(|err| into_fs_error(Cancellable::Error(err)))?;
    }
    Ok(())
}

fn cache_cloned_relation(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
) -> FsResult<()> {
    if let Some(base_adapter) = ctx.env.get_base_adapter() {
        let relation = create_relation_from_node(ctx.default_adapter_type(), node, None)?;
        let _ = base_adapter.cache_added(&ctx.env.empty_state(), relation.into());
    }
    Ok(())
}

async fn submit_model(
    ctx: &TaskRunnerCtx,
    model: &DbtModel,
    task_result: &TaskResult,
    client: &dbt_state::service_client::SharedRunCacheServiceClient,
    microbatch_window: Option<(DateTime<Utc>, DateTime<Utc>)>,
) -> FsResult<Option<RunCacheSubmitResult>> {
    let full_refresh = effective_full_refresh(
        ctx.inner.arg.full_refresh,
        model.deprecated_config.full_refresh,
    );
    // A microbatch model's cache key is only sound with its event-time window
    // folded in (every batch shares one target table and query hash). If the
    // window could not be resolved there is nothing sound to write down either,
    // so make no decision *and* record nothing — the `Ok(None)` path only
    // invalidates the target's cached freshness. Checked ahead of full refresh
    // so that a full-refreshed microbatch model is never recorded on the
    // strength of a window we could not resolve. Mirrors the plugin's
    // `_resolve_microbatch_window` bypass.
    if is_microbatch_model(model) && microbatch_window.is_none() {
        record_submit_skipped(model, "unresolved microbatch window");
        return Ok(None);
    }

    if full_refresh && full_refresh_blocks_model_submit(model.materialized()) {
        // `--full-refresh` is an explicit instruction to rebuild, so this node
        // must never be skipped. The service cannot enforce that for us: the
        // request carries no full-refresh flag, only `execution_type=FULL`, so
        // a daily full-refresh job would match yesterday's FULL record whenever
        // the sources happened not to move and skip the rebuild the user asked
        // for. Hence the client-side bail.
        //
        // That is a reason not to seek a *decision*, not a reason to hide the
        // execution. The node still rewrites its target, so record it: going
        // dark leaves the service blind to the rebuild and leaves the cached
        // last-modified epoch describing pre-build state, which downstream
        // nodes in this run then report and are incorrectly skipped on.
        record_submit_skipped(model, "full refresh");
        return Ok(
            prepare_write_only_execution_record(ctx, model, task_result, microbatch_window)
                .await?
                .map(RunCacheSubmitResult::ExecuteUntracked),
        );
    }

    submit_sql_with_speculation(
        ctx,
        model,
        task_result.sql_instruction.sql.clone(),
        matches!(
            model.materialized(),
            DbtMaterialization::View | DbtMaterialization::MetricView
        ),
        full_refresh,
        microbatch_window,
        client,
        |context| {
            build_model_sql_request(
                model,
                context,
                &ctx.inner.materialization_resolver,
                create_macro_resolver(ctx),
            )
        },
    )
    .await
}

/// Submit a SQL node, using the speculative fast-path while the global
/// dependency prefetch is still in flight.
///
/// When the prefetch is already complete this awaits it (a no-op) and submits
/// regularly. Otherwise it builds a speculative request from cache reads only
/// and branches on the verdict:
/// - `SkipExecution` / `ReadyToClone` — returned as a synthetic regular
///   response (identical inner types) so the caller's `record_service_decision`
///   handles it unchanged. Sound because a missing dependency epoch can only
///   make a candidate look staler, never fresher.
/// - `ReadyToExecuteUntracked` — the node must execute now; a speculative
///   pending record is returned so the outcome is recorded after the fact with
///   finalized epochs. The prefetch is intentionally not awaited here.
/// - `Undecided` / RPC error / empty — await the prefetch and resubmit a
///   regular (non-speculative) request.
#[allow(clippy::too_many_arguments)]
async fn submit_sql_with_speculation(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    sql: String,
    is_view: bool,
    full_refresh: bool,
    microbatch_window: Option<(DateTime<Utc>, DateTime<Utc>)>,
    client: &dbt_state::service_client::SharedRunCacheServiceClient,
    build_request: impl Fn(SqlRunCacheRequestContext) -> FsResult<SubmitEnrichedSqlRequest>,
) -> FsResult<Option<RunCacheSubmitResult>> {
    // Client-generated so `EnrichedSqlPrepared` telemetry doesn't depend on
    // the server response carrying a request id (`SkipExecution` and
    // `ReadyToExecuteUntracked` don't).
    let request_id = dbt_state::service_client::new_request_id();

    if is_prefetch_ready(ctx) {
        return build_and_submit_non_speculative(
            ctx,
            node,
            sql,
            is_view,
            full_refresh,
            microbatch_window,
            client,
            &build_request,
            request_id,
            true,
        )
        .await;
    }

    let request_start = Instant::now();
    let Some((request, freshness_tolerance_seconds)) = build_sql_request(
        ctx,
        node,
        &sql,
        is_view,
        full_refresh,
        microbatch_window,
        true,
        &build_request,
    )
    .await?
    else {
        return Ok(None);
    };
    let telemetry = enriched_sql_prepared_telemetry_input(&request);
    let prepare_duration = request_start.elapsed();

    // Re-check readiness at the last responsible moment. `build_sql_request`
    // blocks on the view-definition traversal (fetching view DDL from the
    // warehouse) while the background prefetch runs concurrently, so the
    // prefetch often completes during that window. The decision above was made
    // against a now-stale snapshot: if the prefetch is ready, submit
    // non-speculatively with fully-resolved epochs instead of wasting a
    // speculative round-trip that would only come back `Undecided` and force a
    // resubmit. `is_prefetch_ready` is a cheap atomic read, and the rebuild
    // reuses the traverser's warm cache, so no view definition is re-fetched.
    if is_prefetch_ready(ctx) {
        return build_and_submit_non_speculative(
            ctx,
            node,
            sql,
            is_view,
            full_refresh,
            microbatch_window,
            client,
            &build_request,
            request_id,
            true,
        )
        .await;
    }

    // Report the speculative attempt once the final path is known. If the
    // prefetch completed while building the request, the regular path above
    // is the only submission and reports its own event.
    emit_enriched_sql_prepared_telemetry(
        ctx,
        request_id.clone(),
        prepare_duration,
        telemetry,
        None,
    )
    .await;

    let unique_id = node.unique_id();
    let verdict = client
        .submit_enriched_sql_speculative(request.clone())
        .await;
    match verdict {
        Ok(response) => match response.response {
            Some(submit_sql_speculative_response::Response::SkipExecution(skip)) => {
                emit_trace_log_message(|| {
                    format!("dbt State speculative decision: skip execution for node {unique_id}")
                });
                Ok(Some(RunCacheSubmitResult::outcome(
                    SubmitSqlResponse {
                        response: Some(submit_sql_response::Response::SkipExecution(skip)),
                    },
                    freshness_tolerance_seconds,
                )))
            }
            Some(submit_sql_speculative_response::Response::ReadyToClone(clone)) => {
                emit_trace_log_message(|| {
                    format!("dbt State speculative decision: ready to clone for node {unique_id}")
                });
                Ok(Some(RunCacheSubmitResult::outcome(
                    SubmitSqlResponse {
                        response: Some(submit_sql_response::Response::ReadyToClone(clone)),
                    },
                    freshness_tolerance_seconds,
                )))
            }
            Some(submit_sql_speculative_response::Response::ReadyToExecuteUntracked(_)) => {
                emit_trace_log_message(|| {
                    format!(
                        "dbt State speculative decision: ready to execute untracked for node {unique_id}; recording after execution"
                    )
                });
                Ok(Some(RunCacheSubmitResult::ExecuteUntracked(
                    RunCachePendingExecutionRecord::sql_speculative(request),
                )))
            }
            Some(submit_sql_speculative_response::Response::Undecided(_)) | None => {
                emit_trace_log_message(|| {
                    format!(
                        "dbt State speculative decision: undecided for node {unique_id}; awaiting prefetch and resubmitting"
                    )
                });
                await_prefetch(ctx).await;
                build_and_submit_non_speculative(
                    ctx,
                    node,
                    sql,
                    is_view,
                    full_refresh,
                    microbatch_window,
                    client,
                    &build_request,
                    request_id,
                    false,
                )
                .await
            }
        },
        Err(err) => {
            // Swallowed here rather than reported as a prepare-error: the
            // speculative RPC is a best-effort fast path, and the resubmit
            // below still reports its own failures.
            emit_trace_log_message(|| {
                format!(
                    "dbt State speculative submit failed for node {unique_id}: {err}; awaiting prefetch and resubmitting"
                )
            });
            await_prefetch(ctx).await;
            build_and_submit_non_speculative(
                ctx,
                node,
                sql,
                is_view,
                full_refresh,
                microbatch_window,
                client,
                &build_request,
                request_id,
                false,
            )
            .await
        }
    }
}

/// Build and issue a regular (non-speculative) SQL submit, awaiting the global
/// prefetch first so freshness resolves from the warm cache. Idempotent with
/// respect to the prefetch await, so it is safe to call after the speculative
/// path already awaited it.
///
/// `request_id` is the attempt's client-generated id (see
/// `submit_sql_with_speculation`). `report_prepared_success` is `false` when
/// this is a resubmit after a speculative attempt already reported success
/// telemetry for the same id — a failure here is still reported either way.
#[allow(clippy::too_many_arguments)]
async fn build_and_submit_non_speculative(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    sql: String,
    is_view: bool,
    full_refresh: bool,
    microbatch_window: Option<(DateTime<Utc>, DateTime<Utc>)>,
    client: &dbt_state::service_client::SharedRunCacheServiceClient,
    build_request: &impl Fn(SqlRunCacheRequestContext) -> FsResult<SubmitEnrichedSqlRequest>,
    request_id: String,
    report_prepared_success: bool,
) -> FsResult<Option<RunCacheSubmitResult>> {
    await_prefetch(ctx).await;
    let request_start = Instant::now();
    let Some((request, freshness_tolerance_seconds)) = build_sql_request(
        ctx,
        node,
        &sql,
        is_view,
        full_refresh,
        microbatch_window,
        false,
        build_request,
    )
    .await?
    else {
        return Ok(None);
    };
    let telemetry = enriched_sql_prepared_telemetry_input(&request);
    let prepare_duration = request_start.elapsed();

    // Success only if `report_prepared_success` (the sole/first build for
    // this attempt); failure always, even on a resubmit.
    let response = match client.submit_enriched_sql(request).await {
        Ok(response) => {
            if report_prepared_success {
                emit_enriched_sql_prepared_telemetry(
                    ctx,
                    request_id,
                    prepare_duration,
                    telemetry,
                    None,
                )
                .await;
            }
            response
        }
        Err(err) => {
            emit_enriched_sql_prepared_telemetry(
                ctx,
                request_id,
                request_start.elapsed(),
                telemetry,
                Some(err.error_type_label().to_string()),
            )
            .await;
            return Err(fs_err!(
                ErrorCode::Generic,
                "dbt State service SubmitEnrichedSQL failed: {}",
                err
            ));
        }
    };
    Ok(Some(RunCacheSubmitResult::outcome(
        response,
        freshness_tolerance_seconds,
    )))
}

/// Build a SQL submit request, returning `None` when required metadata is
/// missing (the submit is then skipped and the node executes normally).
#[allow(clippy::too_many_arguments)]
async fn build_sql_request(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    sql: &str,
    is_view: bool,
    full_refresh: bool,
    microbatch_window: Option<(DateTime<Utc>, DateTime<Utc>)>,
    speculative: bool,
    build_request: &impl Fn(SqlRunCacheRequestContext) -> FsResult<SubmitEnrichedSqlRequest>,
) -> FsResult<Option<(SubmitEnrichedSqlRequest, i64)>> {
    let mut context = build_sql_context(
        ctx,
        node,
        sql.to_string(),
        is_view,
        full_refresh,
        speculative,
    )
    .await?;
    if !context.metadata_complete {
        record_submit_skipped(node, "missing metadata");
        return Ok(None);
    }
    context.request.microbatch_window = microbatch_window;
    let freshness_tolerance_seconds = context.request.freshness_tolerance_seconds;
    let request = build_request(context.request)?;
    Ok(Some((request, freshness_tolerance_seconds)))
}

/// Builds the dbt State record input for write-only mode without contacting the
/// service.
///
/// Write-only must never ask the service for a dbt State decision. This prepares
/// the SQL or seed payload before execution, then the caller records it through
/// `RecordExecutions` only after the node succeeds and final metadata is
/// available.
async fn prepare_write_only_execution_record(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    task_result: &TaskResult,
    microbatch_window: Option<(DateTime<Utc>, DateTime<Utc>)>,
) -> FsResult<Option<RunCachePendingExecutionRecord>> {
    // Write-only makes no speculative decision: it only records executions and
    // needs fully resolved dependency freshness. Block on the background prefetch
    // so the record is built from the warm cache rather than racing it with
    // redundant per-node freshness queries.
    await_prefetch(ctx).await;

    if let Some(model) = node.as_any().downcast_ref::<DbtModel>() {
        if is_no_op_model_materialization(model.materialized()) {
            record_submit_skipped(model, "no-op model materialization");
            return Ok(None);
        }
        if model.common().language.as_deref() != Some("sql") {
            record_unsupported_node(node, "non-SQL model");
            return Ok(None);
        }
        // See `submit_model`: a microbatch model can only be recorded soundly with
        // its resolved window folded into the key (kept even under --full-refresh).
        if is_microbatch_model(model) && microbatch_window.is_none() {
            record_submit_skipped(model, "unresolved microbatch window");
            return Ok(None);
        }
        let full_refresh = effective_full_refresh(
            ctx.inner.arg.full_refresh,
            model.deprecated_config.full_refresh,
        );
        let mut context = build_sql_context(
            ctx,
            model,
            task_result.sql_instruction.sql.clone(),
            matches!(
                model.materialized(),
                DbtMaterialization::View | DbtMaterialization::MetricView
            ),
            full_refresh,
            false,
        )
        .await?;
        if !context.metadata_complete {
            record_submit_skipped(model, "missing metadata");
            return Ok(None);
        }
        context.request.microbatch_window = microbatch_window;
        remove_cache_decision_fields(&mut context.request);
        Ok(Some(RunCachePendingExecutionRecord::sql(
            build_model_sql_request(
                model,
                context.request,
                &ctx.inner.materialization_resolver,
                create_macro_resolver(ctx),
            )?,
        )))
    } else if let Some(snapshot) = node.as_any().downcast_ref::<DbtSnapshot>() {
        let mut context = build_sql_context(
            ctx,
            snapshot,
            task_result.sql_instruction.sql.clone(),
            false,
            effective_full_refresh(
                ctx.inner.arg.full_refresh,
                snapshot.deprecated_config.full_refresh,
            ),
            false,
        )
        .await?;
        if !context.metadata_complete {
            record_submit_skipped(snapshot, "missing metadata");
            return Ok(None);
        }
        remove_cache_decision_fields(&mut context.request);
        Ok(Some(RunCachePendingExecutionRecord::sql(
            build_snapshot_sql_request(snapshot, context.request, create_macro_resolver(ctx))?,
        )))
    } else if let Some(seed) = node.as_any().downcast_ref::<DbtSeed>() {
        let request = build_seed_values_request(
            seed,
            SeedRunCacheRequestContext {
                adapter_type: seed.node_adapter(),
                dialect: run_cache_dialect(ctx),
                last_modified_epoch: None,
                clone_time_travel_limit: None,
                clone_table_properties: None,
                clone_chain_depth_limit: clone_chain_depth_limit_for_seed(ctx, seed),
                dbt_project_info: DbtProjectInfo::from(ctx),
            },
            create_macro_resolver(ctx),
        )?;
        Ok(Some(RunCachePendingExecutionRecord::values(request)))
    } else {
        record_unsupported_node(node, "unsupported node type");
        Ok(None)
    }
}

async fn submit_snapshot(
    ctx: &TaskRunnerCtx,
    snapshot: &DbtSnapshot,
    task_result: &TaskResult,
    client: &dbt_state::service_client::SharedRunCacheServiceClient,
) -> FsResult<Option<RunCacheSubmitResult>> {
    let full_refresh = effective_full_refresh(
        ctx.inner.arg.full_refresh,
        snapshot.deprecated_config.full_refresh,
    );
    submit_sql_with_speculation(
        ctx,
        snapshot,
        task_result.sql_instruction.sql.clone(),
        false,
        full_refresh,
        None,
        client,
        |context| build_snapshot_sql_request(snapshot, context, create_macro_resolver(ctx)),
    )
    .await
}

async fn submit_seed(
    ctx: &TaskRunnerCtx,
    seed: &DbtSeed,
    task_result: &TaskResult,
    client: &dbt_state::service_client::SharedRunCacheServiceClient,
) -> FsResult<Option<RunCacheSubmitResult>> {
    if effective_full_refresh(
        ctx.inner.arg.full_refresh,
        seed.deprecated_config.full_refresh,
    ) {
        // See `submit_model`: an explicit rebuild must not be skipped, but it
        // must still be observed, or downstream nodes report pre-reload state.
        record_submit_skipped(seed, "full refresh");
        return Ok(
            prepare_write_only_execution_record(ctx, seed, task_result, None)
                .await?
                .map(RunCacheSubmitResult::ExecuteUntracked),
        );
    }

    // Seeds never speculate; make sure the target's freshness has been resolved
    // by the background prefetch before reading it from the cache.
    await_prefetch(ctx).await;

    // Mirrors dbt-core's `_build_submit_values_request`: always submit, even
    // when the target table doesn't exist yet. The service treats a None
    // `last_modified_epoch` as `target_table_exists=false` and returns
    // ReadyToExecute, then ConfirmExecution registers the run after the seed
    // materializes. Bailing out on first run would delay registration by a
    // build and break the two-run dbt State cycle the dbt-core plugin implements.
    let last_modified_epoch = last_modified_epoch_for_node(ctx, seed).await?;
    let clone_time_travel_limit = ctx
        .inner
        .run_cache_ctx
        .run_cache_service_config
        .as_ref()
        .and_then(|config| config.clone_time_travel_limit_seconds);
    let request = build_seed_values_request(
        seed,
        SeedRunCacheRequestContext {
            adapter_type: seed.node_adapter(),
            dialect: run_cache_dialect(ctx),
            last_modified_epoch,
            clone_time_travel_limit,
            clone_table_properties: None,
            clone_chain_depth_limit: clone_chain_depth_limit_for_seed(ctx, seed),
            dbt_project_info: DbtProjectInfo::from(ctx),
        },
        create_macro_resolver(ctx),
    )?;

    let response = client.submit_values(request).await.map_err(|e| {
        fs_err!(
            ErrorCode::Generic,
            "dbt State service SubmitValues failed: {}",
            e
        )
    })?;
    Ok(Some(RunCacheSubmitResult::outcome(response, 0)))
}

/// Mirrors dbt-core's `_DataTestAdapterProxy._on_data_test_query`: submit a
/// data test's count(*) SQL with `execution_type=DbtDataTest`. The cached
/// `{failures, should_warn, should_error}` payload flows back through
/// `SkipExecutionResponse.execution_results` and is decoded by
/// `parse_cached_test_execution_result`. On `ReadyToExecute`, the dispatcher
/// confirms after the test runs (see `set_test_execution_results`).
async fn submit_test(
    ctx: &TaskRunnerCtx,
    test: &DbtTest,
    task_result: &TaskResult,
    client: &dbt_state::service_client::SharedRunCacheServiceClient,
) -> FsResult<Option<RunCacheSubmitResult>> {
    submit_sql_with_speculation(
        ctx,
        test,
        task_result.sql_instruction.sql.clone(),
        false, // tests aren't views
        false, // full_refresh is meaningless for tests
        None,
        client,
        |context| build_test_sql_request(test, context, create_macro_resolver(ctx)),
    )
    .await
}

struct EnrichedSqlPreparedTelemetryInput {
    target_table_fqn: Option<String>,
    num_dependencies: Option<i64>,
    num_view_dependencies: Option<i64>,
    labels: HashMap<String, String>,
}

fn enriched_sql_prepared_telemetry_input(
    request: &SubmitEnrichedSqlRequest,
) -> EnrichedSqlPreparedTelemetryInput {
    EnrichedSqlPreparedTelemetryInput {
        target_table_fqn: request.target_table.clone(),
        num_dependencies: i64::try_from(
            request
                .tables
                .iter()
                .filter(|table| Some(table.name.as_str()) != request.target_table.as_deref())
                .count(),
        )
        .ok(),
        num_view_dependencies: i64::try_from(request.query_dependencies.len()).ok(),
        labels: request.labels.clone(),
    }
}

/// Emit an `EnrichedSqlPrepared` telemetry event for one submission attempt.
/// `request_id` is a client-generated id correlating this event with the
/// attempt, independent of whatever the server ends up deciding (or whether
/// it responds at all) — see `submit_sql_with_speculation`.
async fn emit_enriched_sql_prepared_telemetry(
    ctx: &TaskRunnerCtx,
    request_id: String,
    duration: std::time::Duration,
    input: EnrichedSqlPreparedTelemetryInput,
    error_type: Option<String>,
) {
    let event = enriched_sql_prepared_event(
        request_id,
        duration,
        input.target_table_fqn,
        input.num_dependencies,
        input.num_view_dependencies,
        error_type,
        input.labels,
        next_telemetry_event_order(ctx),
    );
    submit_run_cache_telemetry_event(ctx, event).await;
}

struct BuiltSqlRunCacheContext {
    request: SqlRunCacheRequestContext,
    metadata_complete: bool,
}

async fn build_sql_context(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    sql: String,
    is_view: bool,
    full_refresh: bool,
    speculative: bool,
) -> FsResult<BuiltSqlRunCacheContext> {
    let Some(config) = ctx.inner.run_cache_ctx.run_cache_service_config.as_ref() else {
        return Err(fs_err!(
            ErrorCode::Generic,
            "dbt State service config is unavailable"
        ));
    };

    let query_dependencies = collect_query_dependencies(ctx, node, &sql, is_view).await?;
    let tables = collect_table_modified_infos(
        ctx,
        node,
        is_view,
        &query_dependencies.seen_tables,
        &query_dependencies.parser_seen_relations,
        &query_dependencies.unresolvable_tables,
        speculative,
    )
    .await?;
    let metadata_complete = tables.metadata_complete && query_dependencies.metadata_complete;
    let lenient_dependencies = build_lenient_dependencies(
        config.enable_lenient_dependencies,
        &ctx.inner.run_cache_ctx.run_cache_deferred_fqns,
        &tables.tables,
        &query_dependencies.dependencies,
    );

    let stale_upstream_policy = stale_upstream_policy_for_node(node);

    let active_profile = ctx.dbt_profile();
    let is_targeting_prod = config.is_defer_to_target(active_profile);
    let clone_chain_depth_limit = clone_chain_depth_limit_for_adapter(
        node.node_adapter(),
        is_targeting_prod,
        active_profile.allow_clones,
    );

    Ok(BuiltSqlRunCacheContext {
        request: SqlRunCacheRequestContext {
            adapter_type: node.node_adapter(),
            dialect: run_cache_dialect(ctx),
            sql,
            tables: tables.tables,
            query_dependencies: query_dependencies.dependencies,
            freshness_tolerance_seconds: request_freshness_tolerance_seconds_for_node(
                node,
                is_view,
                config.freshness_tolerance_seconds,
            ),
            lenient_dependencies,
            tolerate_nondeterminism: resolve_tolerate_nondeterminism(
                node,
                config.tolerate_nondeterminism,
            ),
            compare_unrendered_code: resolve_compare_unrendered_code(
                node,
                config.compare_unrendered_code,
            ),
            full_refresh,
            clone_time_travel_limit: config.clone_time_travel_limit_seconds,
            clone_table_properties: None,
            clone_chain_depth_limit,
            default_schema: node.schema(),
            stale_upstream_policy,
            // Populated by the model submit paths for microbatch models; other
            // node types never carry a window.
            microbatch_window: None,
            dbt_project_info: DbtProjectInfo::from(ctx),
        },
        metadata_complete,
    })
}

/// The full `ModelState` (5 keys) for models and snapshots. Data tests carry the 2-key
/// `DataTestState` instead (see the field-level accessors below), so 5-key-only configs such as
/// `lag_tolerance`/`pre_clone`/`execute_hooks_on_any_reuse` correctly return nothing for tests.
fn model_state_for_node(node: &dyn InternalDbtNodeAttributes) -> Option<&ModelState> {
    let any = node.as_any();
    if let Some(model) = any.downcast_ref::<DbtModel>() {
        model.__model_attr__.state.as_ref()
    } else if let Some(snapshot) = any.downcast_ref::<DbtSnapshot>() {
        snapshot.__snapshot_attr__.state.as_ref()
    } else {
        None
    }
}

/// `require_fresh_data_from`, honored by models and snapshots (via `ModelState`) and data tests (via
/// `DataTestState`).
fn require_fresh_data_from_for_node(
    node: &dyn InternalDbtNodeAttributes,
) -> Option<&dbt_schemas::schemas::common::UpdatesOn> {
    if let Some(state) = model_state_for_node(node) {
        return state.require_fresh_data_from.as_ref();
    }
    node.as_any()
        .downcast_ref::<DbtTest>()
        .and_then(|test| test.__test_attr__.state.as_ref())
        .and_then(|state| state.require_fresh_data_from.as_ref())
}

/// `evaluate_volatile_sql`, honored by models and snapshots (via `ModelState`) and data tests (via
/// `DataTestState`).
fn evaluate_volatile_sql_for_node(node: &dyn InternalDbtNodeAttributes) -> Option<bool> {
    if let Some(state) = model_state_for_node(node) {
        return state.evaluate_volatile_sql;
    }
    node.as_any()
        .downcast_ref::<DbtTest>()
        .and_then(|test| test.__test_attr__.state.as_ref())
        .and_then(|state| state.evaluate_volatile_sql)
}

/// `compare_unrendered_code`, honored by models and snapshots (via `ModelState`) and data tests
/// (via `DataTestState`).
fn compare_unrendered_code_for_node(node: &dyn InternalDbtNodeAttributes) -> Option<bool> {
    if let Some(state) = model_state_for_node(node) {
        return state.compare_unrendered_code;
    }
    node.as_any()
        .downcast_ref::<DbtTest>()
        .and_then(|test| test.__test_attr__.state.as_ref())
        .and_then(|state| state.compare_unrendered_code)
}

/// Per-node override for the service's `compare_unrendered_code` wire flag. Unlike
/// `tolerate_nondeterminism` there is no inversion and no legacy `meta` form: the node config
/// wins over the service default, otherwise the default stands.
fn resolve_compare_unrendered_code(
    node: &dyn InternalDbtNodeAttributes,
    service_default: bool,
) -> bool {
    let node_override = compare_unrendered_code_for_node(node);
    let resolved = node_override.unwrap_or(service_default);

    // Logs the override separately from the result so a surprising `false` distinguishes
    // "the node never set it" from "the service default lost to an explicit node value".
    emit_trace_log_message(|| {
        format!(
            "dbt State compare_unrendered_code={resolved} for node {} (node config {}, service default {service_default})",
            node.unique_id(),
            match node_override {
                Some(v) => v.to_string(),
                None => "unset".to_owned(),
            },
        )
    });

    resolved
}

pub fn should_execute_hooks_for_skip_reuse(
    node: &dyn InternalDbtNodeAttributes,
    service_default: bool,
) -> bool {
    model_state_for_node(node)
        .and_then(|state| state.execute_hooks_on_any_reuse)
        .unwrap_or(service_default)
}

fn freshness_tolerance_seconds_for_node(
    node: &dyn InternalDbtNodeAttributes,
    service_default: i64,
) -> i64 {
    let state_lag_tolerance = model_state_for_node(node)
        .and_then(|state| state.lag_tolerance.as_ref())
        .and_then(freshness_rule_to_seconds);

    let legacy_build_after = node
        .as_any()
        .downcast_ref::<DbtModel>()
        .and_then(|model| model.__model_attr__.freshness.as_ref())
        .and_then(|freshness| freshness.build_after.as_ref())
        .and_then(freshness_rule_to_seconds);

    state_lag_tolerance
        .or(legacy_build_after)
        .unwrap_or(service_default)
}

fn request_freshness_tolerance_seconds_for_node(
    node: &dyn InternalDbtNodeAttributes,
    is_view: bool,
    service_default: i64,
) -> i64 {
    // Data tests never carry `lag_tolerance` (their `DataTestState` has only `require_fresh_data_from`
    // and `evaluate_volatile_sql`), so their freshness tolerance stays 0. `require_fresh_data_from`
    // for tests is honored separately via `stale_upstream_policy_for_node`.
    if node.resource_type() == NodeType::Test || is_view {
        0
    } else {
        freshness_tolerance_seconds_for_node(node, service_default)
    }
}

fn freshness_rule_to_seconds(rule: &ModelFreshnessRules) -> Option<i64> {
    (rule.count.is_some() && rule.period.is_some()).then(|| rule.to_seconds())
}

/// Per-node override for the dbt State service's `tolerate_nondeterminism`
/// wire flag. The aligned `state.evaluate_volatile_sql` config takes
/// precedence and maps inversely: evaluating volatile SQL means the service
/// should not tolerate nondeterminism for reuse. The legacy
/// `meta["run_cache_tolerate_nondeterminism"]` form is retained as a fallback
/// for compatibility.
fn resolve_tolerate_nondeterminism(
    node: &dyn InternalDbtNodeAttributes,
    service_default: bool,
) -> bool {
    if let Some(evaluate_volatile_sql) = evaluate_volatile_sql_for_node(node) {
        return !evaluate_volatile_sql;
    }

    const KEY: &str = "run_cache_tolerate_nondeterminism";
    let Some(value) = node.meta().get(KEY).cloned() else {
        return service_default;
    };
    if let Some(b) = value.as_bool() {
        return b;
    }
    if let Some(i) = value.as_i64() {
        return i != 0;
    }
    if let Some(s) = value.as_str() {
        match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "1" | "on" => return true,
            "false" | "no" | "0" | "off" => return false,
            _ => {}
        }
    }
    emit_warn_log_message(
        ErrorCode::StateServiceWarn,
        format!(
            "Ignoring meta.{KEY} on node {}: value is not a bool, int, or recognized string",
            node.unique_id()
        ),
    );
    service_default
}

/// Translate the node's freshness policy into the dbt State service's wire
/// enum. The aligned `state.require_fresh_data_from` config takes precedence.
/// Legacy `freshness.build_after.updates_on` remains as a fallback for
/// compatibility. ANY = every upstream must be within tolerance; ALL = at
/// least one upstream must be within tolerance.
fn stale_upstream_policy_for_node(
    node: &dyn InternalDbtNodeAttributes,
) -> dbt_state::proto::query_cache::StaleUpstreamPolicy {
    use dbt_schemas::schemas::common::UpdatesOn;
    use dbt_state::proto::query_cache::StaleUpstreamPolicy;

    let updates_on = require_fresh_data_from_for_node(node).or_else(|| {
        node.as_any()
            .downcast_ref::<DbtModel>()
            .and_then(|model| model.__model_attr__.freshness.as_ref())
            .and_then(|freshness| freshness.build_after.as_ref())
            .and_then(|build_after| build_after.updates_on.as_ref())
    });

    match updates_on {
        Some(UpdatesOn::All) => StaleUpstreamPolicy::All,
        Some(UpdatesOn::Any) | None => StaleUpstreamPolicy::Any,
    }
}

/// Seeds skip `build_sql_context`, so derive the limit the way it does.
fn clone_chain_depth_limit_for_seed(ctx: &TaskRunnerCtx, seed: &DbtSeed) -> Option<i64> {
    let is_targeting_prod = ctx
        .inner
        .run_cache_ctx
        .run_cache_service_config
        .as_ref()
        .map(|config| config.is_defer_to_target(ctx.dbt_profile()))
        .unwrap_or(false);
    clone_chain_depth_limit_for_adapter(
        seed.node_adapter(),
        is_targeting_prod,
        ctx.dbt_profile().allow_clones,
    )
}

pub(crate) fn clone_chain_depth_limit_for_adapter(
    adapter_type: AdapterType,
    is_targeting_prod: bool,
    allow_clones: bool,
) -> Option<i64> {
    // if cloning is disabled by the active target's config, send a limit of 0 to
    // exclude clone candidates regardless of adapter
    if !allow_clones {
        return Some(0);
    }

    // the Python implementation hardcodes the default on each adapter
    // ref: https://github.com/fivetran/query-cache/blob/14dddc260082af4bc6a9800743ddbe5ccb03ba67/clients/dbt_state/src/dbt_state/run_cache.py#L1911
    match adapter_type {
        AdapterType::Databricks => Some(1), // ref: https://github.com/fivetran/query-cache/blob/14dddc260082af4bc6a9800743ddbe5ccb03ba67/clients/dbt_state/src/dbt_state/adapters/databricks.py#L37
        AdapterType::Bigquery => Some(3), // ref: https://github.com/fivetran/query-cache/blob/14dddc260082af4bc6a9800743ddbe5ccb03ba67/clients/dbt_state/src/dbt_state/adapters/bigquery.py#L21
        _ => None,
    }
    .map(|default_limit: i64| {
        if is_targeting_prod {
            // for prod, we send a limit of n-1 so there is always room for 1 more dev clone
            // note: if the default limits ever get adjusted (or new ones added)
            // make sure that this does not return a negative number
            default_limit.saturating_sub(1_i64)
        } else {
            default_limit
        }
    })
}

fn metadata_query_options_for_warehouses(
    profile_warehouse: Option<String>,
    legacy_service_warehouse: Option<String>,
) -> MetadataQueryOptions {
    MetadataQueryOptions {
        warehouse: profile_warehouse.or(legacy_service_warehouse),
        ..MetadataQueryOptions::default()
    }
}

pub(crate) fn run_cache_metadata_query_options(ctx: &TaskRunnerCtx) -> MetadataQueryOptions {
    let profile_warehouse = match &ctx.dbt_profile().default_db_config() {
        DbConfig::Snowflake(config) => config.metadata_warehouse.clone(),
        _ => None,
    };
    let legacy_service_warehouse = ctx
        .inner
        .run_cache_ctx
        .run_cache_service_config
        .as_ref()
        .and_then(|config| config.snowflake_metadata_warehouse.clone());

    // Adaptive broad-vs-sequential freshness fetch: read from the Snowflake
    // target config, defaulting to `true` (adaptive on). Only the Snowflake
    // no-metadata-warehouse strategy consults it; other adapters ignore it.
    let adaptive_metadata_fetch = match &ctx.dbt_profile().default_db_config() {
        DbConfig::Snowflake(config) => config.adaptive_metadata_fetch.unwrap_or(true),
        _ => true,
    };

    MetadataQueryOptions {
        adaptive_metadata_fetch,
        ..metadata_query_options_for_warehouses(profile_warehouse, legacy_service_warehouse)
    }
}

/// Returns deferred dependencies that should be treated leniently by the dbt
/// State service for this specific request.
///
/// Auto-deferral can rewrite unselected upstreams to the configured `defer_to`
/// target. When those upstreams appear in the submitted table freshness or view
/// query-dependency metadata, marking them lenient tells the service they were
/// intentionally deferred. The result is limited to dependencies present in the
/// request so unrelated deferred nodes do not affect the dbt State decision.
fn build_lenient_dependencies(
    enable_lenient_dependencies: bool,
    deferred_fqns: &BTreeSet<String>,
    tables: &[TableModifiedInfo],
    query_dependencies: &[QueryDependency],
) -> Vec<String> {
    if !enable_lenient_dependencies {
        return Vec::new();
    }

    let request_dependencies = tables
        .iter()
        .map(|table| table.name.as_str())
        .chain(
            query_dependencies
                .iter()
                .map(|dependency| dependency.name.as_str()),
        )
        .collect::<BTreeSet<_>>();

    deferred_fqns
        .iter()
        .filter(|fqn| request_dependencies.contains(fqn.as_str()))
        .cloned()
        .collect()
}

fn create_macro_resolver<'a>(ctx: &'a TaskRunnerCtx) -> impl Fn(&str) -> Option<&'a DbtMacro> {
    |macro_id| ctx.resolver_state().macros.macros.get(macro_id)
}

struct CollectedTableModifiedInfos {
    tables: Vec<TableModifiedInfo>,
    metadata_complete: bool,
}

struct CollectedViewQueryDependencies {
    dependencies: Vec<QueryDependency>,
    /// Leaf-table closure produced by the view traversal. Empty when the
    /// model has no parseable upstream refs in its compiled SQL or its
    /// upstreams are all views. Failure paths use `incomplete()`, which
    /// trips `metadata_complete = false` and skips the cache submit
    /// entirely — so this field always reflects a real traversal result.
    seen_tables: BTreeSet<String>,
    /// Upstream relations to backfill into `collect_table_modified_infos`'s
    /// relation map. Two sources, both keyed by `semantic_fqn()` (the same
    /// canonical scheme the DAG-deps loop in `collect_table_modified_infos`
    /// uses):
    ///   1. The SQL parser's view of the model's own compiled SQL — picks up
    ///      raw `from <schema>.<table>` references with no `ref()`/`source()`
    ///      that have no DAG edge but were syntactically observed.
    ///   2. View-traversal leaves — the non-view base tables reached by
    ///      recursing through upstream view DDL. Without these,
    ///      `last_modified_epoch` for a transitive base table is never sent,
    ///      and the service's freshness check defaults to "fresh" on the
    ///      NULL/NULL match path (see `test_transitive_dependencies_tracked`).
    parser_seen_relations: BTreeMap<String, Arc<dyn BaseRelation>>,
    /// A subset of `seen_tables` whose view definition could not be fetched.
    unresolvable_tables: BTreeSet<String>,
    metadata_complete: bool,
}

impl CollectedViewQueryDependencies {
    fn complete(
        dependencies: Vec<QueryDependency>,
        seen_tables: BTreeSet<String>,
        parser_seen_relations: BTreeMap<String, Arc<dyn BaseRelation>>,
        unresolvable_tables: BTreeSet<String>,
    ) -> Self {
        Self {
            dependencies,
            seen_tables,
            parser_seen_relations,
            unresolvable_tables,
            metadata_complete: true,
        }
    }

    fn incomplete() -> Self {
        Self {
            dependencies: Vec::new(),
            seen_tables: BTreeSet::new(),
            parser_seen_relations: BTreeMap::new(),
            unresolvable_tables: BTreeSet::new(),
            metadata_complete: false,
        }
    }

    /// Empty, completed result used for views.
    ///
    /// Views are re-evaluated on every read, so the dbt State service only
    /// checks the view's own `last_modified_epoch` and SQL hash to decide
    /// reuse — upstream view DDL and base-table freshness are irrelevant.
    /// Mirrors the dbt-state Python plugin's view path
    /// (clients/dbt_state/src/dbt_state/run_cache.py:1116-1146), which sends
    /// `query_dependencies=[]`. `metadata_complete` must stay `true` so the
    /// submit isn't silently skipped.
    fn for_view() -> Self {
        Self::complete(
            Vec::new(),
            BTreeSet::new(),
            BTreeMap::new(),
            BTreeSet::new(),
        )
    }
}

async fn collect_table_modified_infos(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    target_only: bool,
    leaf_tables: &BTreeSet<String>,
    parser_seen_relations: &BTreeMap<String, Arc<dyn BaseRelation>>,
    unresolvable_tables: &BTreeSet<String>,
    speculative: bool,
) -> FsResult<CollectedTableModifiedInfos> {
    let mut relations = BTreeMap::new();
    let mut metadata_complete = true;

    let (target_name, target_relation) = relation_for_node(node)?;
    relations.insert(target_name.clone(), target_relation);

    let mut freshness_overrides: BTreeMap<String, FreshnessOverride> = BTreeMap::new();

    if !target_only && let Some(deps) = ctx.inner.runtime_deps.get(node.unique_id().as_str()) {
        for dep_id in deps {
            let Some(dep_node) = ctx.nodes().get_node(dep_id) else {
                continue;
            };
            if dep_node.as_any().is::<DbtModel>()
                || dep_node.as_any().is::<DbtSnapshot>()
                || dep_node.as_any().is::<DbtSeed>()
                || dep_id.starts_with("source.")
            {
                if let Ok((name, relation)) = relation_for_node(dep_node) {
                    // For sources with `loaded_at_query` or `loaded_at_field` set,
                    // build an override entry keyed by the relation's semantic_fqn —
                    // that's what MetadataAdapter::freshness_with_overrides expects.
                    // Empty strings come from the deserializer for absent values, so
                    // treat empty as "not set" (matches the dbt-core plugin guard).
                    if let Some(source) = dep_node.as_any().downcast_ref::<DbtSource>()
                        && let Some(kind) = source_freshness_override(source)
                    {
                        let kind = render_freshness_override(
                            kind,
                            &relation,
                            ctx.env.as_ref(),
                            &ctx.inner.base_context,
                        )?;
                        freshness_overrides.insert(relation.semantic_fqn(), kind);
                    }
                    relations.insert(name, relation);
                } else {
                    metadata_complete = false;
                }
            }
        }
    }

    // Backfill any tables the SQL parser saw but the DAG didn't declare
    // (raw `from <schema>.<table>` references with no `ref()`/`source()`).
    // Without this, those upstreams' `last_modified_epoch` never reaches the
    // dbt State service, so the service can't detect drift and `is_stale` is
    // always false. Mirrors dbt-core's plugin, which sources `tables`
    // directly from sqlglot's `find_tables` rather than the manifest DAG.
    if !target_only {
        for (fqn, relation) in parser_seen_relations {
            relations
                .entry(fqn.clone())
                .or_insert_with(|| Arc::clone(relation));
        }
    }

    // Only leaf tables (plus the target) go into the request's `tables`
    // field. Upstream views are published via `query_dependencies`; if we
    // also emitted them here, the dbt State server would prefer the table
    // entry's stored `semantic_hash` over recursing into the view's DDL,
    // hiding transitive DDL changes (see test_run_upstream_view_model_changes).
    let leaf_table_relations: BTreeMap<String, Arc<dyn BaseRelation>> = relations
        .iter()
        .filter(|(fqn, rel)| {
            leaf_tables.contains(rel.semantic_fqn().as_str()) || *fqn == &target_name
        })
        .map(|(fqn, rel)| (fqn.clone(), Arc::clone(rel)))
        .collect();

    apply_unresolvable_last_modified_overrides(
        &ctx.inner.run_cache_ctx.run_cache_metadata,
        ctx.inner.run_cache_ctx.heuristic_clock.get(),
        unresolvable_tables,
        &freshness_overrides,
    );

    // Speculative builds run while the global prefetch is still in flight, so
    // they must not issue their own blocking per-node warehouse queries. They
    // read whatever the prefetch has already resolved and leave cache misses
    // unset; the service treats a missing epoch as "now", keeping any Skip/Clone
    // verdict sound (a missing dep can only look staler, never fresher). Missing
    // epochs must not mark the metadata incomplete here — a speculative submit
    // tolerates unresolved dependencies.
    if !speculative {
        prefetch_last_modified_epochs(ctx, &leaf_table_relations, &freshness_overrides).await;
    }

    let mut table_infos = Vec::new();
    for (name, _) in leaf_table_relations {
        let last_modified_epoch = last_modified_epoch_for_relation(ctx, &name).await?;
        if last_modified_epoch.is_some() || name != target_name {
            table_infos.push(TableModifiedInfo {
                name,
                last_modified_epoch,
            });
        }
    }

    Ok(CollectedTableModifiedInfos {
        tables: table_infos,
        metadata_complete,
    })
}

fn apply_unresolvable_last_modified_overrides(
    cache: &RunCacheMetadataCache,
    clock: Option<&HeuristicClock>,
    unresolvable_tables: &BTreeSet<String>,
    freshness_overrides: &BTreeMap<String, FreshnessOverride>,
) {
    let Some(clock) = clock else {
        for name in unresolvable_tables {
            cache.remove_last_modified_epoch(name);
        }
        return;
    };

    let mut unresolvable_without_override = Vec::new();
    for name in unresolvable_tables {
        cache.remove_last_modified_epoch(name);
        if !freshness_overrides.contains_key(name) {
            unresolvable_without_override.push(name.clone());
        }
    }

    if unresolvable_without_override.is_empty() {
        return;
    }

    emit_warn_log_message(
        ErrorCode::StateServiceWarn,
        format!(
            "Could not determine freshness for {}; treating as modified. Configure loaded_at_field or loaded_at_query to set freshness timestamp.",
            unresolvable_without_override.join(", ")
        ),
    );

    let now_ms = clock.now_ms();
    for name in unresolvable_without_override {
        cache.insert_last_modified_epoch(name, Some(now_ms));
    }
}

async fn last_modified_epoch_for_node(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
) -> FsResult<Option<i64>> {
    let (name, _) = relation_for_node(node)?;
    last_modified_epoch_for_relation(ctx, &name).await
}

pub async fn refresh_final_last_modified_epoch_for_node(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
) -> FsResult<Option<i64>> {
    let (name, relation) = relation_for_node(node)?;
    ctx.inner
        .run_cache_ctx
        .run_cache_metadata
        .remove_last_modified_epoch(&name);
    refresh_last_modified_epoch_for_relation(ctx, &name, relation).await
}

pub fn clear_stale_missing_last_modified_epoch_for_node(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
) {
    if let Ok((name, _)) = relation_for_node(node) {
        // Only clear a known-stale placeholder (`Some(None)`), cached when the
        // submit-time probe found no target table yet. A real cached epoch
        // (`Some(Some(_))`) is still valid and may be relied on by sibling nodes
        // later in this same run.
        let run_cache_metadata = &ctx.inner.run_cache_ctx.run_cache_metadata;
        if matches!(run_cache_metadata.last_modified_epoch(&name), Some(None)) {
            run_cache_metadata.remove_last_modified_epoch(&name);
        }
    }
}

/// Evicts a node's cached warehouse metadata (last-modified epoch and existence)
/// when it rebuilds without the dbt State service observing it.
///
/// An untracked rebuild makes the cached value stale; without eviction,
/// downstream nodes in the same invocation report the relation's pre-build
/// state to the service and are incorrectly skipped. The next submit that needs
/// the relation treats the gap as a prefetch miss and re-queries it. Mirrors
/// the plugin's `clear_cache([target_table])`.
///
/// Unlike [`clear_stale_missing_last_modified_epoch_for_node`] (the benign-skip
/// path), this clears a real cached epoch too. Prefer recording the execution
/// where a sound record can be built — see `submit_model` — which refreshes the
/// epoch instead of dropping it and keeps the service's view complete.
pub fn evict_node_metadata_for_untracked_rebuild(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
) {
    if let Ok((name, _)) = relation_for_node(node) {
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .invalidate_relation_metadata(&name);
    }
}

fn stamp_final_last_modified_epoch_for_node_heuristic(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
) -> Option<i64> {
    let clock = ctx.inner.run_cache_ctx.heuristic_clock.get()?;
    let epoch = clock.now_ms();
    // Mirror the plugin's `clear_cache` + `cache_last_modified_epoch(table, H)`:
    // replace the stale prefetch value (often None for newly-created tables) with H
    // so downstream submissions in the same run have metadata_complete=true.
    let Ok((name, _)) = relation_for_node(node) else {
        return None;
    };
    ctx.inner
        .run_cache_ctx
        .run_cache_metadata
        .remove_last_modified_epoch(&name);
    ctx.inner
        .run_cache_ctx
        .run_cache_metadata
        .insert_last_modified_epoch(&name, Some(epoch));
    Some(epoch)
}

/// Resolves a node's post-execution last-modified epoch, updating the local
/// metadata cache to the same value it returns.
///
/// Callers persist the returned epoch to the dbt State service, so the cached
/// and persisted values must be identical. If they diverge, downstream nodes in
/// this run report a dependency epoch the service never recorded and the
/// dependency reads as changed, costing reuse for no correctness gain.
///
/// Prefers the heuristic clock, which costs no warehouse round-trip, and falls
/// back to a targeted refresh on adapters that opt out (see [`HeuristicClock`]).
async fn final_last_modified_epoch_for_node(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
) -> FsResult<Option<i64>> {
    if let Some(epoch) = stamp_final_last_modified_epoch_for_node_heuristic(ctx, node) {
        return Ok(Some(epoch));
    }
    refresh_final_last_modified_epoch_for_node(ctx, node).await
}

async fn last_modified_epoch_for_relation(
    ctx: &TaskRunnerCtx,
    name: &str,
) -> FsResult<Option<i64>> {
    Ok(ctx
        .inner
        .run_cache_ctx
        .run_cache_metadata
        .last_modified_epoch(name)
        .flatten())
}

async fn fetch_last_modified_epoch_for_relation(
    ctx: TaskRunnerCtx,
    relation: Arc<dyn BaseRelation>,
) -> FsResult<Option<i64>> {
    let Some(adapter) = ctx.env.get_adapter_ref() else {
        return Ok(None);
    };
    let Some(metadata_adapter) = adapter.metadata_adapter() else {
        return Ok(None);
    };
    if !has_metadata_address(ctx.default_adapter_type(), relation.as_ref()) {
        return Ok(None);
    }

    // This helper runs inside the cache's per-relation single-flight lock. Do
    // not route through the coordinated refresh planner here: its prefetch
    // guards use the same key and would wait on the lock held by the caller.
    let metadata_options = run_cache_metadata_query_options(&ctx);
    let relations = [relation];
    let freshness = legacy_freshness(
        metadata_adapter.as_ref(),
        &relations,
        None,
        &metadata_options,
        adapter.cancellation_token(),
    )
    .await
    .map_err(into_fs_error)?;
    let semantic_fqn = relations[0].semantic_fqn();
    Ok(freshness
        .get(&semantic_fqn)
        .map(|metadata| metadata.last_altered.timestamp_millis()))
}

pub(crate) async fn legacy_freshness(
    metadata_adapter: &dyn MetadataAdapter,
    relations: &[Arc<dyn BaseRelation>],
    overrides: Option<&BTreeMap<String, FreshnessOverride>>,
    options: &MetadataQueryOptions,
    token: CancellationToken,
) -> Result<BTreeMap<String, MetadataFreshness>, Cancellable<AdapterError>> {
    let grouped = match overrides {
        Some(overrides) => {
            match metadata_adapter
                .freshness_with_overrides_and_options(relations, overrides, options, token.clone())
                .await
            {
                Err(Cancellable::Error(error))
                    if error.kind() == AdapterErrorKind::ReplayDataMissing =>
                {
                    metadata_adapter
                        .freshness_with_overrides(relations, overrides, token.clone())
                        .await
                }
                result => result,
            }
        }
        None => {
            match metadata_adapter
                .freshness_with_options(relations, options, token.clone())
                .await
            {
                Err(Cancellable::Error(error))
                    if error.kind() == AdapterErrorKind::ReplayDataMissing =>
                {
                    metadata_adapter.freshness(relations, token.clone()).await
                }
                result => result,
            }
        }
    };
    match grouped {
        Ok(freshness) => return Ok(freshness),
        Err(Cancellable::Error(error)) if error.kind() == AdapterErrorKind::ReplayDataMissing => {}
        Err(err) => return Err(err),
    }

    let mut freshness = BTreeMap::new();
    for relation in relations {
        let relation_fqn = relation.semantic_fqn();
        let relation_overrides = overrides.map(|overrides| {
            overrides
                .get(&relation_fqn)
                .cloned()
                .into_iter()
                .map(|override_| (relation_fqn.clone(), override_))
                .collect::<BTreeMap<_, _>>()
        });
        let result = match relation_overrides.as_ref() {
            Some(relation_overrides) => {
                match metadata_adapter
                    .freshness_with_overrides_and_options(
                        std::slice::from_ref(relation),
                        relation_overrides,
                        options,
                        token.clone(),
                    )
                    .await
                {
                    Ok(values) => Ok(values),
                    Err(Cancellable::Error(err))
                        if err.kind() == AdapterErrorKind::ReplayDataMissing =>
                    {
                        match metadata_adapter
                            .freshness_with_overrides(
                                std::slice::from_ref(relation),
                                relation_overrides,
                                token.clone(),
                            )
                            .await
                        {
                            Ok(values) => Ok(values),
                            Err(Cancellable::Error(err))
                                if err.kind() == AdapterErrorKind::ReplayDataMissing =>
                            {
                                match metadata_adapter
                                    .freshness_with_options(
                                        std::slice::from_ref(relation),
                                        options,
                                        token.clone(),
                                    )
                                    .await
                                {
                                    Ok(values) => Ok(values),
                                    Err(Cancellable::Error(err))
                                        if err.kind() == AdapterErrorKind::ReplayDataMissing =>
                                    {
                                        metadata_adapter
                                            .freshness(
                                                std::slice::from_ref(relation),
                                                token.clone(),
                                            )
                                            .await
                                    }
                                    Err(err) => Err(err),
                                }
                            }
                            Err(err) => Err(err),
                        }
                    }
                    Err(err) => Err(err),
                }
            }
            None => {
                match metadata_adapter
                    .freshness_with_options(std::slice::from_ref(relation), options, token.clone())
                    .await
                {
                    Err(Cancellable::Error(err))
                        if err.kind() == AdapterErrorKind::ReplayDataMissing =>
                    {
                        metadata_adapter
                            .freshness(std::slice::from_ref(relation), token.clone())
                            .await
                    }
                    result => result,
                }
            }
        };
        match result {
            Ok(values) => freshness.extend(values),
            Err(Cancellable::Error(err)) if err.kind() == AdapterErrorKind::ReplayDataMissing => {}
            Err(err) => return Err(err),
        }
    }
    Ok(freshness)
}

async fn refresh_last_modified_epoch_for_relation(
    ctx: &TaskRunnerCtx,
    name: &str,
    relation: Arc<dyn BaseRelation>,
) -> FsResult<Option<i64>> {
    let cache = &ctx.inner.run_cache_ctx.run_cache_metadata;
    let lookup_name = name.to_owned();
    let fetch_ctx = ctx.clone();
    let token = ctx
        .env
        .get_adapter_ref()
        .map(|adapter| adapter.cancellation_token());
    let result = cache
        .get_or_try_insert_last_modified_epoch(&lookup_name, move || {
            fetch_last_modified_epoch_for_relation(fetch_ctx.clone(), Arc::clone(&relation))
        })
        .await;
    if result.is_err() && token.is_some_and(|token| token.is_cancelled()) {
        // A cancelled run isn't a genuine lookup failure; don't leave it
        // poisoning the cache for other in-flight lookups of this relation.
        cache.remove_last_modified_epoch_error(&lookup_name);
    }
    result
}

async fn prefetch_last_modified_epochs(
    ctx: &TaskRunnerCtx,
    relations: &BTreeMap<String, Arc<dyn BaseRelation>>,
    overrides: &BTreeMap<String, FreshnessOverride>,
) {
    let misses = relations
        .iter()
        .filter(|(name, _)| {
            ctx.inner
                .run_cache_ctx
                .run_cache_metadata
                .last_modified_epoch(name)
                .is_none()
        })
        .map(|(name, relation)| (name.clone(), Arc::clone(relation)))
        .collect::<BTreeMap<_, _>>();
    if misses.is_empty() {
        return;
    }
    // Cache misses at submit time indicate the global prefetch didn't cover
    // these relations (legitimate for post-clone invalidations and SQL-parser
    // discovered refs) or that the schema dump returned empty. Log so this
    // is visible when diagnosing unexpected per-node warehouse query volume.
    emit_debug_log_message(format!(
        "dbt State per-node freshness query for {} relation(s) not in prefetch cache: {}",
        misses.len(),
        misses.keys().cloned().collect::<Vec<_>>().join(", ")
    ));
    let refresh_result =
        refresh_planned_last_modified_misses(ctx, &misses, overrides, ctx.default_adapter_type())
            .await;
    if let Err(err) = refresh_result {
        emit_trace_log_message(|| format!("dbt State metadata prefetch failed: {err}"));
    }
}

async fn refresh_planned_last_modified_misses(
    ctx: &TaskRunnerCtx,
    misses: &BTreeMap<String, Arc<dyn BaseRelation>>,
    overrides: &BTreeMap<String, FreshnessOverride>,
    adapter_type: AdapterType,
) -> FsResult<()> {
    let plan = plan_last_modified_miss_refresh(adapter_type, misses, overrides);
    if !plan.unaddressable_relations.is_empty() {
        let cache = &ctx.inner.run_cache_ctx.run_cache_metadata;
        cache_unknown_last_modified_epochs(cache, &plan.unaddressable_relations).await;
    }
    if !plan.targeted_relations.is_empty() {
        refresh_last_modified_epochs(ctx, &plan.targeted_relations, overrides).await?;
    }
    if !plan.schema_prefetch_relations.is_empty() {
        bulk_prefetch_last_modified_by_schema(ctx, &plan.schema_prefetch_relations, overrides)
            .await?;
    }
    Ok(())
}

/// Splits submit-time last-modified cache misses by the warehouse query path
/// they should use.
struct LastModifiedMissRefreshPlan {
    targeted_relations: BTreeMap<String, Arc<dyn BaseRelation>>,
    schema_prefetch_relations: BTreeMap<String, Arc<dyn BaseRelation>>,
    /// Relations with no valid metadata address; cached as unknown (`None`)
    /// rather than dropped, so they don't re-trigger a miss on every lookup.
    unaddressable_relations: BTreeMap<String, Arc<dyn BaseRelation>>,
}

/// BigQuery normal misses use the schema prefetch path to avoid high fanout
/// per-table `__TABLES__` queries. A singleton miss can still trigger one
/// schema scan; that is an intentional fix-forward tradeoff for correctness.
/// Overrides stay targeted because their freshness comes from custom
/// `loaded_at_*` logic.
fn plan_last_modified_miss_refresh(
    adapter_type: AdapterType,
    misses: &BTreeMap<String, Arc<dyn BaseRelation>>,
    overrides: &BTreeMap<String, FreshnessOverride>,
) -> LastModifiedMissRefreshPlan {
    let (addressable, unaddressable_relations): (BTreeMap<_, _>, BTreeMap<_, _>) = misses
        .iter()
        .map(|(name, relation)| (name.clone(), Arc::clone(relation)))
        .partition(|(_, relation)| has_metadata_address(adapter_type, relation.as_ref()));

    if matches!(adapter_type, AdapterType::Bigquery) {
        let (targeted_relations, schema_prefetch_relations) =
            split_relations_by_override(&addressable, overrides);
        LastModifiedMissRefreshPlan {
            targeted_relations,
            schema_prefetch_relations,
            unaddressable_relations,
        }
    } else {
        LastModifiedMissRefreshPlan {
            targeted_relations: addressable,
            schema_prefetch_relations: BTreeMap::new(),
            unaddressable_relations,
        }
    }
}

/// Returns `(override_relations, bulk_relations)` so callers can keep source
/// freshness overrides out of schema-level metadata scans.
fn split_relations_by_override(
    relations: &BTreeMap<String, Arc<dyn BaseRelation>>,
    overrides: &BTreeMap<String, FreshnessOverride>,
) -> (
    BTreeMap<String, Arc<dyn BaseRelation>>,
    BTreeMap<String, Arc<dyn BaseRelation>>,
) {
    relations
        .iter()
        .map(|(k, v)| (k.clone(), Arc::clone(v)))
        .partition(|(name, _)| overrides.contains_key(name.as_str()))
}

async fn refresh_last_modified_epochs(
    ctx: &TaskRunnerCtx,
    relations: &BTreeMap<String, Arc<dyn BaseRelation>>,
    overrides: &BTreeMap<String, FreshnessOverride>,
) -> FsResult<()> {
    let cache = &ctx.inner.run_cache_ctx.run_cache_metadata;
    let Some(adapter) = ctx.env.get_adapter_ref() else {
        cache_unknown_last_modified_epochs(cache, relations).await;
        return Ok(());
    };
    let Some(metadata_adapter) = adapter.metadata_adapter() else {
        cache_unknown_last_modified_epochs(cache, relations).await;
        return Ok(());
    };

    for grouped_relations in group_relations_by_database_and_schema(relations).into_values() {
        let prefetch_guards = begin_last_modified_prefetches(cache, &grouped_relations).await;

        // A concurrent caller may have already resolved some of these
        // relations while this call waited on the per-relation locks above
        // (each lock is held until its holder finishes writing). Re-check the
        // cache now that every lock is acquired so this call neither
        // re-queries the warehouse nor clobbers an already-cached value with
        // `None` on its own failure.
        let grouped_relations: BTreeMap<String, Arc<dyn BaseRelation>> = grouped_relations
            .into_iter()
            .filter(|(name, _)| cache.last_modified_epoch(name).is_none())
            .collect();
        if grouped_relations.is_empty() {
            continue;
        }

        let grouped_names = grouped_relations.keys().collect::<BTreeSet<_>>();
        let grouped_overrides = overrides
            .iter()
            .filter(|(name, _)| grouped_names.contains(name))
            .map(|(name, override_)| (name.clone(), override_.clone()))
            .collect::<BTreeMap<_, _>>();
        let semantic_to_name = grouped_relations
            .iter()
            .map(|(name, relation)| (relation.semantic_fqn(), name.clone()))
            .collect::<BTreeMap<_, _>>();
        let relation_values = grouped_relations.values().cloned().collect::<Vec<_>>();
        let metadata_options = run_cache_metadata_query_options(ctx);
        let freshness = match metadata_adapter
            .freshness_with_overrides_and_options(
                &relation_values,
                &grouped_overrides,
                &metadata_options,
                adapter.cancellation_token(),
            )
            .await
        {
            Ok(freshness) => freshness,
            Err(Cancellable::Error(err)) if err.kind() == AdapterErrorKind::ReplayDataMissing => {
                // Older recordings do not contain this override-prefetch event;
                // replay the legacy per-table freshness event instead.
                legacy_freshness(
                    metadata_adapter.as_ref(),
                    &relation_values,
                    Some(&grouped_overrides),
                    &metadata_options,
                    adapter.cancellation_token(),
                )
                .await
                .map_err(into_fs_error)?
            }
            Err(err) => return Err(into_fs_error(err)),
        };

        for (semantic_fqn, name) in semantic_to_name {
            let epoch = freshness
                .get(&semantic_fqn)
                .map(|metadata| metadata.last_altered.timestamp_millis());
            if let Some(guard) = prefetch_guards.get(&name) {
                guard.insert_last_modified_epoch(&name, epoch);
            }
        }
    }
    Ok(())
}

/// Groups relations by `(database, schema)` using each relation's *resolved*
/// (normalized-if-unquoted) database/schema string, not the raw
/// `dbt_project.yml` casing.
///
/// This matters because the resulting keys are threaded through
/// `bulk_prefetch_last_modified_by_schema` into
/// `MetadataAdapter::freshness_all_in_schema`, which builds a literal
/// `WHERE table_schema = '{schema}'` clause. On Snowflake, unquoted
/// identifiers are stored uppercase in `INFORMATION_SCHEMA.TABLES`, so a
/// lowercase schema name from the project would silently fail to match and
/// fall back to the slower per-relation path. Resolving here — once, at the
/// grouping step — makes every downstream schema-dump query correct by
/// construction, without needing a case-insensitive comparison at the SQL
/// layer.
///
/// This mirrors the dbt-core plugin's `prefetch_last_modified_epochs`
/// (`clients/dbt_state/src/dbt_state/adapters/snowflake.py`), which calls
/// `self._to_fqn(raw_fqn)` — sqlglot's dialect-aware
/// `normalize_identifiers()` + `quote_identifiers()` — before grouping
/// schemas into `schemas_by_catalog`, so the `INFORMATION_SCHEMA.TABLES`
/// filter built in `_fetch_last_modified_epochs_from_schemas_in_catalog`
/// never sees an unresolved schema name. `schema_as_resolved_str()` /
/// `database_as_resolved_str()` (`dbt-schemas/src/schemas/relations/base.rs`)
/// are Fusion's native equivalent of that normalize-then-quote step.
fn group_relations_by_database_and_schema(
    relations: &BTreeMap<String, Arc<dyn BaseRelation>>,
) -> BTreeMap<(Option<String>, Option<String>), BTreeMap<String, Arc<dyn BaseRelation>>> {
    let mut grouped = BTreeMap::new();
    for (name, relation) in relations {
        grouped
            .entry((
                relation.database_as_resolved_str().ok(),
                relation.schema_as_resolved_str().ok(),
            ))
            .or_insert_with(BTreeMap::new)
            .insert(name.clone(), Arc::clone(relation));
    }
    grouped
}

/// Derives the SQL table-type keyword for a node (e.g. `"TRANSIENT TABLE"` /
/// `"TABLE"` / `"DYNAMIC TABLE"` / `"INTERACTIVE TABLE"` on Snowflake) that
/// downstream callers send to the dbt State service for clone-SQL composition.
///
/// We derive this from the dbt node config, not from warehouse
/// introspection, mirroring the dbt-core plugin's
/// `get_relation_table_type` (see
/// `run-cache/clients/dbt_run_cache/src/dbt_run_cache/adapters/snowflake.py`).
/// Two reasons we follow that design:
///
///   * On Snowflake the warehouse query Fusion uses for bulk relation
///     listing (`SHOW OBJECTS`) does not expose the transient/permanent
///     bit at all — `kind` is `'TABLE'` for both, and there is no
///     `is_transient` column in the result set. Reaching for the bit via
///     `INFORMATION_SCHEMA.TABLES.table_type` is possible but adds
///     round-trips.
///   * The config IS the source of truth: dbt-snowflake's materialization
///     macros read `config.transient` to decide between
///     `CREATE [OR REPLACE] TRANSIENT TABLE` and `CREATE [OR REPLACE]
///     TABLE`. Going to the warehouse just round-trips the same value
///     through a lossy serializer.
///
/// Returns `None` for adapters that don't need this keyword (everyone
/// except Snowflake today) and for node kinds whose materialization isn't
/// table-like (views, ephemerals, tests, sources, ...). In those cases the
/// dbt State service falls back to its default of `TABLE`.
fn config_derived_table_type(
    node: &dyn InternalDbtNodeAttributes,
    adapter_type: AdapterType,
) -> Option<String> {
    if adapter_type != AdapterType::Snowflake {
        return None;
    }
    let (materialized, transient) = if let Some(model) = node.as_any().downcast_ref::<DbtModel>() {
        (
            model.base().materialized.clone(),
            model
                .deprecated_config
                .__warehouse_specific_config__
                .transient,
        )
    } else if let Some(snapshot) = node.as_any().downcast_ref::<DbtSnapshot>() {
        (
            snapshot.base().materialized.clone(),
            snapshot
                .deprecated_config
                .__warehouse_specific_config__
                .transient,
        )
    } else {
        return None;
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
        // dbt-snowflake defaults table/incremental/snapshot to TRANSIENT
        // unless the user explicitly opts out via `transient: false`.
        DbtMaterialization::Table
        | DbtMaterialization::Incremental
        | DbtMaterialization::Snapshot => Some(
            if transient.unwrap_or(true) {
                "TRANSIENT TABLE"
            } else {
                "TABLE"
            }
            .to_string(),
        ),
        // `TRANSIENT INTERACTIVE TABLE` is not valid DDL, so `transient` is ignored here.
        DbtMaterialization::InteractiveTable => Some("INTERACTIVE TABLE".to_string()),
        _ => None,
    }
}

async fn table_type_for_node(node: &dyn InternalDbtNodeAttributes) -> FsResult<Option<String>> {
    Ok(config_derived_table_type(node, node.node_adapter()))
}

fn relation_for_node(
    node: &dyn InternalDbtNodeAttributes,
) -> FsResult<(String, Arc<dyn BaseRelation>)> {
    let relation = create_relation_from_node(node.node_adapter(), node, None)?;
    // Canonical (`semantic_fqn`) key so lenient-dependency matching, the
    // metadata cache, and the wire payload all agree regardless of how the
    // relation's database/schema/identifier were originally cased.
    let name = relation.semantic_fqn();
    Ok((name, relation.into()))
}

async fn collect_query_dependencies(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    sql: &str,
    is_view: bool,
) -> FsResult<CollectedViewQueryDependencies> {
    if is_view {
        return Ok(CollectedViewQueryDependencies::for_view());
    }
    let relations = match parse_sql_relations_for_run_cache(
        ctx,
        sql,
        &node.database(),
        &node.schema(),
        node.base().quoting_ignore_case,
    ) {
        Ok(relations) => relations,
        Err(err) => return query_dependencies_for_parse_error(ctx, node, sql, err).await,
    };
    if relations.is_empty() {
        // A custom materialization's compiled SQL isn't necessarily a query,
        // so an empty SQL-derived result isn't proof of no upstreams. Fall
        // back to the manifest's depends_on graph. Built-ins are exempt:
        // their compiled SQL is always a real query, so an empty result is
        // trustworthy.
        if node_uses_custom_materialization(node, &ctx.inner.materialization_resolver) {
            let manifest_relations = cacheable_manifest_dependency_relations(ctx, node);
            if !manifest_relations.is_empty() {
                return collect_query_dependencies_from_relations(ctx, node, manifest_relations)
                    .await;
            }
        }
        return Ok(CollectedViewQueryDependencies::complete(
            Vec::new(),
            BTreeSet::new(),
            BTreeMap::new(),
            BTreeSet::new(),
        ));
    }

    collect_query_dependencies_from_relations(ctx, node, relations).await
}

async fn collect_query_dependencies_from_relations(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    relations: BTreeMap<String, Arc<dyn BaseRelation>>,
) -> FsResult<CollectedViewQueryDependencies> {
    let Some(traverser) = ctx.inner.run_cache_ctx.view_traverser.as_deref() else {
        return Ok(CollectedViewQueryDependencies::incomplete());
    };
    let Some(adapter) = ctx.env.get_adapter_ref() else {
        return Ok(CollectedViewQueryDependencies::incomplete());
    };

    let relation_values = relations.values().cloned().collect::<Vec<_>>();
    let traversal = match traverser
        .traverse(&relation_values, adapter.cancellation_token())
        .await
    {
        Ok(traversal) => traversal,
        Err(err) => {
            let unique_id = node.unique_id();
            emit_trace_log_message(|| {
                format!(
                    "dbt State view dependency enrichment failed for node {unique_id}: {err}; continuing without query dependencies"
                )
            });
            return Ok(CollectedViewQueryDependencies::incomplete());
        }
    };

    let seen_tables = traversal.seen_tables;
    let unresolvable_tables = traversal.unresolvable_tables;
    let dependencies = traversal
        .view_definitions
        .into_values()
        .map(|definition| QueryDependency {
            name: definition.fqn,
            query: definition.definition,
            default_catalog: definition.default_catalog,
            default_schema: definition.default_schema,
        })
        .collect();
    let mut parser_seen_relations = relations;
    for (fqn, leaf_relation) in traversal.leaf_relations {
        parser_seen_relations.entry(fqn).or_insert(leaf_relation);
    }
    Ok(CollectedViewQueryDependencies::complete(
        dependencies,
        seen_tables,
        parser_seen_relations,
        unresolvable_tables,
    ))
}

async fn query_dependencies_for_parse_error(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    sql: &str,
    err: Box<FsError>,
) -> FsResult<CollectedViewQueryDependencies> {
    if !node_uses_custom_materialization(node, &ctx.inner.materialization_resolver) {
        return Err(err);
    }

    let unique_id = node.unique_id();
    let upstreams = match extract_standalone_expression_upstreams_for_run_cache(
        node.node_adapter(),
        sql,
        &node.database(),
        &node.schema(),
        node.base().quoting_ignore_case,
        ctx.inner.sources_extractor.as_ref(),
    ) {
        Ok(upstreams) => upstreams,
        Err(standalone_err) => {
            emit_trace_log_message(|| {
                format!(
                    "dbt State dependency extraction incomplete for custom materialization {unique_id}: {err}; standalone expression extraction also failed: {standalone_err}"
                )
            });
            return Ok(CollectedViewQueryDependencies::incomplete());
        }
    };
    let relations = if upstreams.is_empty() {
        BTreeMap::new()
    } else {
        let Some(adapter) = ctx.env.get_adapter_ref() else {
            return Ok(CollectedViewQueryDependencies::incomplete());
        };
        relations_from_upstreams(
            node.node_adapter(),
            upstreams,
            adapter.engine().type_ops().as_ref(),
        )?
    };

    if relations.is_empty() {
        // Same fallback as in `collect_query_dependencies`: an empty
        // expression-parse result isn't proof of no upstreams.
        let manifest_relations = cacheable_manifest_dependency_relations(ctx, node);
        if !manifest_relations.is_empty() {
            return collect_query_dependencies_from_relations(ctx, node, manifest_relations).await;
        }

        emit_trace_log_message(|| {
            format!(
                "dbt State dependency extraction accepted standalone SQL expression for custom materialization {unique_id}"
            )
        });
        return Ok(CollectedViewQueryDependencies::complete(
            Vec::new(),
            BTreeSet::new(),
            BTreeMap::new(),
            BTreeSet::new(),
        ));
    }
    collect_query_dependencies_from_relations(ctx, node, relations).await
}

/// Resolves a node's manifest depends_on graph into relations, restricted
/// to cacheable node kinds. Fallback dependency source when SQL-derived
/// extraction finds none.
fn cacheable_manifest_dependency_relations(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
) -> BTreeMap<String, Arc<dyn BaseRelation>> {
    let mut relations = BTreeMap::new();
    for dep_id in &node.base().depends_on.nodes {
        let Some(dep_node) = ctx.nodes().get_node(dep_id) else {
            continue;
        };
        if !is_cacheable_resource_type(dep_node.resource_type()) {
            continue;
        }
        let Ok((name, relation)) = relation_for_node(dep_node) else {
            continue;
        };
        relations.insert(name, relation);
    }
    relations
}

/// Mirrors the `is_cacheable` filter used elsewhere in the run-cache path:
/// only these node kinds participate in dependency freshness checks.
fn is_cacheable_resource_type(resource_type: NodeType) -> bool {
    matches!(
        resource_type,
        NodeType::Source | NodeType::Model | NodeType::Snapshot | NodeType::Seed
    )
}

fn node_uses_custom_materialization(
    node: &dyn InternalDbtNodeAttributes,
    materialization_resolver: &MaterializationResolver,
) -> bool {
    node.as_any()
        .downcast_ref::<DbtModel>()
        .is_some_and(|model| {
            materialization_resolver
                .is_custom_materialization(&model.materialized().to_string(), model.node_adapter())
        })
}

fn parse_sql_relations_for_run_cache(
    ctx: &TaskRunnerCtx,
    sql: &str,
    default_catalog: &str,
    default_schema: &str,
    quoted_name_ignore_case: bool,
) -> FsResult<BTreeMap<String, Arc<dyn BaseRelation>>> {
    let Some(adapter) = ctx.env.get_adapter_ref() else {
        return Ok(BTreeMap::new());
    };
    let type_ops = adapter.engine().type_ops().as_ref();
    let sources_extractor = ctx.inner.sources_extractor.as_ref();
    parse_sql_relations_for_adapter(
        ctx.default_adapter_type(),
        sql,
        default_catalog,
        default_schema,
        quoted_name_ignore_case,
        sources_extractor,
        type_ops,
    )
}

fn parse_sql_relations_for_adapter(
    adapter_type: AdapterType,
    sql: &str,
    default_catalog: &str,
    default_schema: &str,
    quoted_name_ignore_case: bool,
    sources_extractor: &dyn SourcesExtractor,
    type_ops: &dyn TypeOps,
) -> FsResult<BTreeMap<String, Arc<dyn BaseRelation>>> {
    let upstreams = match sources_extractor.extract_upstreams(
        adapter_type,
        sql,
        default_catalog,
        default_schema,
        quoted_name_ignore_case,
    ) {
        Ok(upstreams) => upstreams,
        Err(statement_err) => sources_extractor
            .extract_standalone_expression_upstreams(
                adapter_type,
                sql,
                default_catalog,
                default_schema,
                quoted_name_ignore_case,
            )
            .map_err(|expression_err| {
                fs_err!(
                    ErrorCode::Generic,
                    "failed to extract upstreams from SQL as statement or expression: statement parse failed: {statement_err}; expression parse failed: {expression_err}"
                )
            })?,
    };

    relations_from_upstreams(adapter_type, upstreams, type_ops)
}

fn extract_standalone_expression_upstreams_for_run_cache(
    adapter_type: AdapterType,
    sql: &str,
    default_catalog: &str,
    default_schema: &str,
    quoted_name_ignore_case: bool,
    sources_extractor: &dyn SourcesExtractor,
) -> FsResult<Vec<NamedReference<FullyQualifiedName>>> {
    sources_extractor
        .extract_standalone_expression_upstreams(
            adapter_type,
            sql,
            default_catalog,
            default_schema,
            quoted_name_ignore_case,
        )
        .map_err(|e| {
            fs_err!(
                ErrorCode::Generic,
                "failed to extract upstreams from SQL: {e}"
            )
        })
}

fn relations_from_upstreams(
    adapter_type: AdapterType,
    upstreams: Vec<NamedReference<FullyQualifiedName>>,
    type_ops: &dyn TypeOps,
) -> FsResult<BTreeMap<String, Arc<dyn BaseRelation>>> {
    let mut relations = BTreeMap::new();
    for upstream in upstreams.into_iter() {
        if upstream.table().as_str().starts_with('@') {
            continue;
        }
        let relation = create_relation(
            adapter_type,
            upstream.catalog().to_string(),
            upstream.schema().to_string(),
            Some(upstream.table().to_string()),
            None,
            quoting_for_upstream(&upstream, type_ops),
        )?;
        // Canonical key so a parser-seen relation collapses against the same
        // upstream surfaced via DAG dependencies (`relation_for_node`) and the
        // deferred-FQN set, regardless of the casing in the compiled SQL.
        let name = relation.semantic_fqn();
        relations.insert(name, relation.into());
    }

    Ok(relations)
}

fn quoting_for_upstream(
    upstream: &NamedReference<FullyQualifiedName>,
    type_ops: &dyn TypeOps,
) -> ResolvedQuoting {
    ResolvedQuoting {
        database: type_ops.need_quotes_for_ident(upstream.catalog().as_str()),
        schema: type_ops.need_quotes_for_ident(upstream.schema().as_str()),
        identifier: type_ops.need_quotes_for_ident(upstream.table().as_str()) || upstream.is_prefix,
    }
}

fn run_cache_dialect(ctx: &TaskRunnerCtx) -> String {
    ctx.default_adapter_type().to_string()
}

fn should_honor_service_skip(ctx: &TaskRunnerCtx) -> bool {
    effective_run_cache_service_use_cache(
        &ctx.inner.arg.run_cache_mode,
        ctx.inner.run_cache_ctx.run_cache_service_requested,
    )
}

fn remove_cache_decision_fields(context: &mut SqlRunCacheRequestContext) {
    context.freshness_tolerance_seconds = 0;
    context.lenient_dependencies.clear();
    context.tolerate_nondeterminism = false;
    context.clone_time_travel_limit = None;
    context.clone_table_properties = None;
}

fn effective_full_refresh(cli_full_refresh: bool, config_full_refresh: Option<bool>) -> bool {
    config_full_refresh.unwrap_or(cli_full_refresh)
}

fn full_refresh_blocks_model_submit(materialization: DbtMaterialization) -> bool {
    matches!(
        materialization,
        DbtMaterialization::Incremental
            | DbtMaterialization::MaterializedView
            | DbtMaterialization::MetricView
            | DbtMaterialization::DynamicTable
            | DbtMaterialization::StreamingTable
            | DbtMaterialization::InteractiveTable
    )
}

fn effective_run_cache_service_use_cache(
    run_cache_mode: &RunCacheMode,
    service_requested: bool,
) -> bool {
    run_cache_mode.use_cache()
        || (service_requested && matches!(run_cache_mode, RunCacheMode::Noop))
}

fn is_no_op_model_materialization(materialization: DbtMaterialization) -> bool {
    matches!(
        materialization,
        DbtMaterialization::Ephemeral | DbtMaterialization::Inline
    )
}

fn record_service_decision(
    unique_id: &str,
    response: &SubmitSqlResponse,
    freshness_tolerance_seconds: i64,
    honor_skip: bool,
) -> RunCacheServiceDecision {
    match response.response.as_ref() {
        Some(submit_sql_response::Response::ReadyToExecute(response)) => {
            let request_id = response.request_id.clone();
            emit_trace_log_message(|| {
                format!(
                    "dbt State service decision: ready to execute (node {unique_id}, request_id {request_id})"
                )
            });
            RunCacheServiceDecision::execute_with_confirmation(response.request_id.clone(), false)
        }
        Some(submit_sql_response::Response::SkipExecution(response)) => {
            if !honor_skip {
                emit_trace_log_message(|| {
                    format!(
                        "dbt State service decision: skip ignored in write-only mode for node {unique_id}; executing normally"
                    )
                });
                return RunCacheServiceDecision::execute_without_confirmation();
            }

            emit_trace_log_message(|| {
                format!("dbt State service decision: skip execution for node {unique_id}")
            });
            // For data tests, parse the cached result out of the service's
            // `execution_results` so the dispatcher in `runnable/mod.rs` can
            // replace the generic `ReusedNoChanges` status with a test-shaped
            // verdict and a NO-OP-marked stat.
            let is_test = unique_id.starts_with("test.");
            let cached_test_result = if is_test {
                parse_cached_test_execution_result(response)
            } else {
                None
            };
            if is_test && cached_test_result.is_none() {
                emit_trace_log_message(|| {
                    format!(
                        "dbt State service data test skip ignored because no cached test result was returned for node {unique_id}; executing normally"
                    )
                });
                return RunCacheServiceDecision::execute_without_confirmation();
            }
            let status = if is_test {
                NodeStatus::ReusedNoChanges("No new changes on any upstreams".to_string())
            } else {
                skip_node_status_from_response(response, freshness_tolerance_seconds)
            };
            RunCacheServiceDecision::Skip {
                status,
                sao_stored_hash: None,
                cached_test_result,
            }
        }
        Some(submit_sql_response::Response::ReadyToClone(response)) => {
            if honor_skip {
                let request_id = response.request_id.clone();
                let clone_source = response.clone_source.clone();
                let clone_target = response.clone_target.clone();
                emit_trace_log_message(|| {
                    format!(
                        "dbt State service decision: ready to clone (node {unique_id}, request_id {request_id}, clone_source {clone_source}, clone_target {clone_target})"
                    )
                });
                RunCacheServiceDecision::Clone {
                    clone: RunCacheCloneDecision::from_response(
                        response,
                        freshness_tolerance_seconds,
                    ),
                }
            } else {
                let request_id = response.request_id.clone();
                emit_trace_log_message(|| {
                    format!(
                        "dbt State service decision: clone ignored in write-only mode (node {unique_id}, request_id {request_id}); executing normally"
                    )
                });
                RunCacheServiceDecision::execute_with_confirmation(
                    response.request_id.clone(),
                    true,
                )
            }
        }
        None => {
            emit_trace_log_message(|| {
                format!(
                    "dbt State service decision: empty response ignored for node {unique_id}; executing normally"
                )
            });
            RunCacheServiceDecision::execute_without_confirmation()
        }
    }
}

/// Extract the service-side explain decision id from a submit response.
pub(crate) fn execution_decision_id_from_response(response: &SubmitSqlResponse) -> Option<String> {
    match response.response.as_ref()? {
        submit_sql_response::Response::ReadyToExecute(response) => {
            response.execution_decision_id.clone()
        }
        submit_sql_response::Response::SkipExecution(response) => {
            response.execution_decision_id.clone()
        }
        submit_sql_response::Response::ReadyToClone(response) => {
            response.execution_decision_id.clone()
        }
    }
}

/// Map a `SkipExecutionResponse` to the [`NodeStatus`] used for downstream
/// reporting. When the service admitted a candidate despite at least one
/// upstream having changed (`explained_decision.is_stale == true`), emit
/// [`NodeStatus::ReusedStillFresh`] so the terminal/run_results message
/// reads "New changes detected..." instead of "No new changes on any
/// upstreams".
///
/// `freshness_tolerance_seconds` is the same value Fusion sent in the request,
/// echoed locally to fill the formatter's `lag_tolerance` slot. The "last updated"
/// magnitude is not visible to Fusion (only the service sees the cached-side
/// per-dep timestamps), so it is reported as 0.
fn skip_node_status_from_response(
    response: &SkipExecutionResponse,
    freshness_tolerance_seconds: i64,
) -> NodeStatus {
    let is_stale = response
        .explained_decision
        .as_ref()
        .map(|d| d.is_stale)
        .unwrap_or(false);
    if !is_stale {
        return NodeStatus::ReusedNoChanges("No new changes on any upstreams".to_string());
    }

    let tolerance_secs = freshness_tolerance_seconds.max(0) as u64;
    let message = format!(
        "New changes detected within lag tolerance of {}",
        humantime::format_duration(std::time::Duration::from_secs(tolerance_secs)),
    );
    NodeStatus::ReusedStillFresh(message, tolerance_secs, 0)
}

/// If `is_dev_cloned` and the service decided Skip, rewrite the status to a
/// structured clone-from-cache cache reason so run_results matches the dbt-core
/// plugin (`run_cache.py:_process_query_cache_response`).
fn relabel_skip_for_dev_cloned_node(
    is_dev_cloned: bool,
    decision: RunCacheServiceDecision,
) -> RunCacheServiceDecision {
    let RunCacheServiceDecision::Skip {
        status,
        sao_stored_hash,
        cached_test_result,
    } = decision
    else {
        return decision;
    };
    if !is_dev_cloned {
        return RunCacheServiceDecision::Skip {
            status,
            sao_stored_hash,
            cached_test_result,
        };
    }
    let relabelled = match status {
        NodeStatus::ReusedNoChanges(_) => NodeStatus::ReusedCloned(None),
        NodeStatus::ReusedStillFresh(_, tolerance_secs, _) => {
            NodeStatus::ReusedCloned(Some(tolerance_secs))
        }
        other => other,
    };
    RunCacheServiceDecision::Skip {
        status: relabelled,
        sao_stored_hash,
        cached_test_result,
    }
}

fn record_unsupported_node(node: &dyn InternalDbtNodeAttributes, reason: &'static str) {
    let unique_id = node.unique_id();
    emit_trace_log_message(|| {
        format!("dbt State service submit skipped for node {unique_id}: {reason}")
    });
}

fn record_submit_skipped(node: &dyn InternalDbtNodeAttributes, reason: &'static str) {
    let unique_id = node.unique_id();
    emit_trace_log_message(|| {
        format!("dbt State service submit skipped for node {unique_id}: {reason}")
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_adapter::{Adapter, AdapterBuilder, AdapterImpl, AdapterStore};
    use dbt_common::path::DbtPath;
    use dbt_schemas::state::ProfileAdapter;
    use indexmap::IndexMap;
    use std::any::Any;
    use std::future::Future;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;
    use std::sync::Mutex;

    use crate::RunTasksArgs;
    use crate::context::{ExtendedCtx, RunCacheCtx, TaskRunnerCtxInner};
    use arrow::array::RecordBatch;
    use arrow_schema::{Schema, SchemaRef};
    use dbt_adapter::sql_types::DefaultTypeOps;
    use dbt_adapter::stmt_splitter::DefaultStmtSplitter;
    use dbt_common::cancellation::never_cancels;
    use dbt_common::collections::DashMap;
    use dbt_common::io_args::RunCacheMode;
    use dbt_common::{CompiledSpans, MacroSpan};
    use dbt_dag::schedule::Schedule;
    use dbt_frontend_common::FullyQualifiedName;
    use dbt_frontend_common::error::{
        CodeLocation, ErrorCode as FrontendErrorCode, FrontendError, FrontendResult,
    };
    use dbt_frontend_common::named_reference::NamedReference;
    use dbt_frontend_common::sources_extractor::SourcesExtractor;
    use dbt_frontend_common::span::ReclassifySpan;
    use dbt_jinja_utils::jinja_environment::JinjaEnv;
    use dbt_jinja_utils::listener::DefaultRenderingEventListenerFactory;
    use dbt_schema_store::mock_store::{MockDataStore, MockSchemaStore};
    use dbt_schemas::schemas::Nodes;
    use dbt_schemas::schemas::common::{FreshnessPeriod, ResolvedQuoting, UpdatesOn};
    use dbt_schemas::schemas::macros::DbtMacro;
    use dbt_schemas::schemas::profiles::{Execute, SnowflakeDbConfig};
    use dbt_schemas::schemas::properties::{DataTestState, ModelFreshness, ModelState};
    use dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING;
    use dbt_schemas::state::{
        DbtProfile, DbtRuntimeConfig, DummyNodeResolverTracker, Macros, Operations, RenderResults,
        ResolverState,
    };
    use dbt_state::metadata_cache::RunCacheMetadataCache;
    use dbt_state::proto::query_cache::{
        ConfirmExecutionResponse, ReadyToCloneResponse, ReadyToExecuteResponse,
        ReadyToExecuteUntrackedResponse, RecordExecutionsRequest, RecordExecutionsResponse,
        SubmitSqlSpeculativeResponse, UndecidedResponse, execution_record,
    };
    use dbt_state::service_client::{
        ClientVersionStatus, RunCacheServiceClient, RunCacheServiceError,
    };
    use dbt_state::service_config::RunCacheServiceConfig;

    fn model_with_state(state: ModelState) -> DbtModel {
        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = "model.test.orders".to_string();
        model.__model_attr__.state = Some(state);
        model
    }

    #[test]
    fn state_explain_node_info_identifies_incremental_model() {
        let model = state_explain_model(DbtMaterialization::Incremental);

        let info = state_explain_node_info_for_parts(AdapterType::Postgres, true, &model);

        assert_eq!(info.node_resource_type, "model");
        assert_eq!(info.fqn, "\"analytics\".\"marts\".\"orders\"");
        assert!(!info.is_view);
        assert!(info.is_table);
        assert!(info.is_incremental_or_snapshot);
        assert!(info.is_full_refresh);
    }

    #[test]
    fn state_explain_node_info_reports_ephemeral_without_a_relation() {
        let model = state_explain_model(DbtMaterialization::Ephemeral);

        let info = state_explain_node_info_for_parts(AdapterType::Postgres, false, &model);

        assert!(info.is_ephemeral);
        assert!(!info.is_table);
        assert!(!info.is_view);
        // Ephemeral models are inlined into their consumers, so naming a
        // warehouse relation for them would name a table that cannot exist.
        assert!(info.fqn.is_empty());
    }

    #[test]
    fn state_explain_node_info_identifies_seed_and_snapshot() {
        let seed = state_explain_seed();
        let snapshot = state_explain_snapshot();

        let seed_info = state_explain_node_info_for_parts(AdapterType::Postgres, true, &seed);
        let snapshot_info =
            state_explain_node_info_for_parts(AdapterType::Postgres, false, &snapshot);

        assert_eq!(seed_info.node_resource_type, "seed");
        assert!(seed_info.is_table);
        assert!(!seed_info.is_incremental_or_snapshot);
        assert!(!seed_info.is_full_refresh);
        assert_eq!(snapshot_info.node_resource_type, "snapshot");
        assert!(snapshot_info.is_incremental_or_snapshot);
    }

    #[test]
    fn ready_to_execute_confirms_after_execution() {
        assert_eq!(
            record_service_decision("model.test.orders", &ready_to_execute_response(), 0, true),
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::Confirm(RunCacheExecutionConfirmation {
                    request_id: "execute-request".to_string(),
                    failed_to_clone: false,
                    execution_results: None,
                    execution_runtime_ms: None,
                }),
                sao_guard: None
            }
        );
    }

    #[test]
    fn groups_snowflake_relations_by_resolved_case_not_raw_case() {
        // Two relations that dbt_project.yml would produce with different casing for
        // the *same* physical Snowflake schema/database (unquoted identifiers, so
        // Snowflake itself stores/reports them uppercase in INFORMATION_SCHEMA.TABLES).
        // They must land in the same (database, schema) bucket, otherwise the
        // schema-level freshness dump built from these groups uses a schema string
        // that can't match INFORMATION_SCHEMA's stored casing (see
        // dbt-state-snowflake-schema-case-bug.md).
        let unquoted = ResolvedQuoting {
            database: false,
            schema: false,
            identifier: false,
        };

        let relation_lower = create_relation(
            AdapterType::Snowflake,
            "analytics_dev".to_string(),
            "c24_data_quality_prod".to_string(),
            Some("orders".to_string()),
            None,
            unquoted,
        )
        .unwrap();
        let relation_upper = create_relation(
            AdapterType::Snowflake,
            "ANALYTICS_DEV".to_string(),
            "C24_DATA_QUALITY_PROD".to_string(),
            Some("customers".to_string()),
            None,
            unquoted,
        )
        .unwrap();

        let mut relations: BTreeMap<String, Arc<dyn BaseRelation>> = BTreeMap::new();
        relations.insert("model.test.orders".to_string(), relation_lower.into());
        relations.insert("model.test.customers".to_string(), relation_upper.into());

        let grouped = group_relations_by_database_and_schema(&relations);

        assert_eq!(
            grouped.len(),
            1,
            "relations differing only by unquoted-identifier casing must be grouped \
             together, since Snowflake resolves them to the same physical schema; \
             mirrors the dbt-core plugin's prefetch_last_modified_epochs, which \
             normalizes each raw_fqn via self._to_fqn() (sqlglot normalize_identifiers \
             + quote_identifiers) before adding its schema to schemas_by_catalog"
        );
        let ((db, schema), group) = grouped.iter().next().unwrap();
        assert_eq!(db.as_deref(), Some("ANALYTICS_DEV"));
        assert_eq!(schema.as_deref(), Some("C24_DATA_QUALITY_PROD"));
        assert_eq!(group.len(), 2);
    }

    #[test]
    fn skip_response_is_honored_in_read_write_mode() {
        assert!(matches!(
            record_service_decision("model.test.orders", &skip_execution_response(), 0, true),
            RunCacheServiceDecision::Skip {
                status: NodeStatus::ReusedNoChanges(_),
                sao_stored_hash: None,
                cached_test_result: None,
            }
        ));
    }

    #[test]
    fn data_test_skip_without_cached_result_executes() {
        assert_eq!(
            record_service_decision(
                "test.test.not_null_orders_order_date.abc123",
                &skip_execution_response(),
                0,
                true,
            ),
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::None,
                sao_guard: None,
            }
        );
    }

    #[test]
    fn data_test_skip_with_cached_result_is_honored() {
        match record_service_decision(
            "test.test.not_null_orders_order_date.abc123",
            &skip_execution_response_with_test_result(CachedTestExecutionResult {
                failures: 2,
                should_warn: true,
                should_error: false,
            }),
            0,
            true,
        ) {
            RunCacheServiceDecision::Skip {
                status: NodeStatus::ReusedNoChanges(_),
                sao_stored_hash: None,
                cached_test_result: Some(result),
            } => {
                assert_eq!(result.failures, 2);
                assert!(result.should_warn);
                assert!(!result.should_error);
            }
            other => panic!("expected cached data-test skip, got {other:?}"),
        }
    }

    #[test]
    fn data_test_skip_with_incomplete_cached_result_executes() {
        let response = SubmitSqlResponse {
            response: Some(submit_sql_response::Response::SkipExecution(
                SkipExecutionResponse {
                    execution_results: Some(test_execution_results_with_failures_only(0)),
                    ..Default::default()
                },
            )),
        };

        assert_eq!(
            record_service_decision(
                "test.test.not_null_orders_order_date.abc123",
                &response,
                0,
                true,
            ),
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::None,
                sao_guard: None,
            }
        );
    }

    #[test]
    fn stale_data_test_skip_with_cached_result_reports_no_changes() {
        let response = SubmitSqlResponse {
            response: Some(submit_sql_response::Response::SkipExecution(
                SkipExecutionResponse {
                    explained_decision: Some(ExplainedDecision {
                        is_stale: true,
                        ..Default::default()
                    }),
                    execution_results: Some(build_test_execution_results_struct(
                        CachedTestExecutionResult {
                            failures: 0,
                            should_warn: false,
                            should_error: false,
                        },
                    )),
                    ..Default::default()
                },
            )),
        };

        assert!(matches!(
            record_service_decision(
                "test.test.not_null_orders_order_date.abc123",
                &response,
                3600,
                true,
            ),
            RunCacheServiceDecision::Skip {
                status: NodeStatus::ReusedNoChanges(_),
                sao_stored_hash: None,
                cached_test_result: Some(CachedTestExecutionResult {
                    failures: 0,
                    should_warn: false,
                    should_error: false,
                }),
            }
        ));
    }

    #[test]
    fn stale_skip_response_emits_still_fresh_with_message() {
        let response = SubmitSqlResponse {
            response: Some(submit_sql_response::Response::SkipExecution(
                SkipExecutionResponse {
                    explained_decision: Some(ExplainedDecision {
                        is_stale: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )),
        };
        match record_service_decision("model.test.orders", &response, 3600, true) {
            RunCacheServiceDecision::Skip {
                status: NodeStatus::ReusedStillFresh(message, tolerance, _),
                sao_stored_hash: None,
                cached_test_result: None,
            } => {
                assert!(
                    message.contains("New changes detected"),
                    "message did not advertise stale-skip: {message}"
                );
                assert_eq!(tolerance, 3600);
            }
            other => panic!("expected ReusedStillFresh, got {other:?}"),
        }
    }

    #[test]
    fn relabel_skip_for_dev_cloned_node_rewrites_still_fresh_to_clone_still_fresh() {
        let original = RunCacheServiceDecision::Skip {
            status: NodeStatus::ReusedStillFresh(
                "New changes detected within lag tolerance of 1h".to_string(),
                3600,
                42,
            ),
            sao_stored_hash: None,
            cached_test_result: None,
        };

        match relabel_skip_for_dev_cloned_node(true, original) {
            RunCacheServiceDecision::Skip {
                status: NodeStatus::ReusedCloned(Some(tolerance)),
                ..
            } => {
                assert_eq!(tolerance, 3600);
            }
            other => panic!("expected ReusedCloned(Some(_)), got {other:?}"),
        }
    }

    #[test]
    fn relabel_skip_for_dev_cloned_node_rewrites_reused_no_changes_to_clone() {
        let original = RunCacheServiceDecision::Skip {
            status: NodeStatus::ReusedNoChanges("No new changes on any upstreams".to_string()),
            sao_stored_hash: None,
            cached_test_result: None,
        };

        match relabel_skip_for_dev_cloned_node(true, original) {
            RunCacheServiceDecision::Skip {
                status: NodeStatus::ReusedCloned(None),
                ..
            } => {}
            other => panic!("expected ReusedCloned(None), got {other:?}"),
        }
    }

    #[test]
    fn relabel_skip_for_dev_cloned_node_passes_through_when_not_dev_cloned() {
        let make = || RunCacheServiceDecision::Skip {
            status: NodeStatus::ReusedNoChanges("No new changes on any upstreams".to_string()),
            sao_stored_hash: None,
            cached_test_result: None,
        };
        let relabelled = relabel_skip_for_dev_cloned_node(false, make());
        assert_eq!(relabelled, make());
    }

    #[test]
    fn relabel_skip_for_dev_cloned_node_passes_through_non_skip_decision() {
        let make = || RunCacheServiceDecision::Execute {
            after_success: RunCacheAfterSuccess::None,
            sao_guard: None,
        };
        let relabelled = relabel_skip_for_dev_cloned_node(true, make());
        assert_eq!(relabelled, make());
    }

    #[test]
    fn skip_response_is_ignored_in_write_only_mode() {
        assert_eq!(
            record_service_decision("model.test.orders", &skip_execution_response(), 0, false),
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::None,
                sao_guard: None,
            }
        );
    }

    #[test]
    fn full_refresh_blocks_only_affected_model_materializations() {
        assert!(full_refresh_blocks_model_submit(
            DbtMaterialization::Incremental
        ));
        assert!(full_refresh_blocks_model_submit(
            DbtMaterialization::InteractiveTable
        ));
        assert!(full_refresh_blocks_model_submit(
            DbtMaterialization::MetricView
        ));
        assert!(!full_refresh_blocks_model_submit(DbtMaterialization::View));
        assert!(!full_refresh_blocks_model_submit(DbtMaterialization::Table));
        assert!(!full_refresh_blocks_model_submit(
            DbtMaterialization::Unknown("custom_table".to_string())
        ));
    }

    #[test]
    fn config_derived_table_type_treats_interactive_table_as_never_transient() {
        let mut model = make_model(
            "model.test.orders",
            "db",
            "dbt_test",
            "orders",
            DbtMaterialization::InteractiveTable,
        );
        assert_eq!(
            config_derived_table_type(model.as_ref(), AdapterType::Snowflake),
            Some("INTERACTIVE TABLE".to_string())
        );

        Arc::get_mut(&mut model)
            .unwrap()
            .deprecated_config
            .__warehouse_specific_config__
            .transient = Some(true);
        assert_eq!(
            config_derived_table_type(model.as_ref(), AdapterType::Snowflake),
            Some("INTERACTIVE TABLE".to_string())
        );
    }

    #[test]
    fn no_op_model_materializations_are_not_submitted() {
        assert!(is_no_op_model_materialization(
            DbtMaterialization::Ephemeral
        ));
        assert!(is_no_op_model_materialization(DbtMaterialization::Inline));
        assert!(!is_no_op_model_materialization(DbtMaterialization::View));
        assert!(!is_no_op_model_materialization(DbtMaterialization::Table));
    }

    #[dbt_runtime::test]
    async fn custom_materialization_submits_compiled_sql_as_custom_execution_type() {
        let client = Arc::new(RecordingRunCacheClient::default());
        let ctx = test_task_runner_ctx(Some(
            client.clone() as dbt_state::service_client::SharedRunCacheServiceClient
        ));
        let model = make_model(
            "model.test.user_name_only_model",
            "db",
            "dbt_test",
            "user_name_only_model",
            DbtMaterialization::Unknown("custom_table".to_string()),
        );

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("'alice'"),
            None,
        )
        .await;

        assert_eq!(
            client.submitted_sql(),
            vec!["'alice'".to_string()],
            "custom materializations should mirror dbt-core/query-cache by submitting compiled model SQL"
        );
        assert_eq!(
            client.submitted_execution_types(),
            vec![dbt_state::proto::query_cache::ModelExecutionType::DbtCustom as i32],
            "custom materializations should be marked DBT_CUSTOM"
        );
        assert!(matches!(
            decision,
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::Confirm(_),
                sao_guard: None,
            }
        ));
    }

    #[dbt_runtime::test]
    async fn custom_materialization_submits_compiled_sql_during_full_refresh() {
        let client = Arc::new(RecordingRunCacheClient::default());
        let ctx = test_task_runner_ctx_with_mode_and_full_refresh(
            Some(client.clone() as dbt_state::service_client::SharedRunCacheServiceClient),
            RunCacheMode::ReadWrite,
            true,
        );
        let model = make_model(
            "model.test.user_name_only_model",
            "db",
            "dbt_test",
            "user_name_only_model",
            DbtMaterialization::Unknown("custom_table".to_string()),
        );

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("'alice'"),
            None,
        )
        .await;

        assert_eq!(client.submitted_sql(), vec!["'alice'".to_string()]);
        assert!(matches!(
            decision,
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::Confirm(_),
                sao_guard: None,
            }
        ));
    }

    #[dbt_runtime::test]
    async fn full_refresh_microbatch_without_window_invalidates_instead_of_recording() {
        let client = Arc::new(RecordingRunCacheClient::default());
        let ctx = test_task_runner_ctx_with_mode_and_full_refresh(
            Some(client.clone() as dbt_state::service_client::SharedRunCacheServiceClient),
            RunCacheMode::ReadWrite,
            true,
        );
        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = "model.test.user_name_model".to_string();
        model.__common_attr__.language = Some("sql".to_string());
        model.__base_attr__.database = "db".to_string();
        model.__base_attr__.schema = "dbt_test".to_string();
        model.__base_attr__.alias = "user_name_model".to_string();
        model.__base_attr__.materialized = DbtMaterialization::Incremental;
        model.__model_attr__.incremental_strategy =
            Some(dbt_schemas::schemas::common::DbtIncrementalStrategy::Microbatch);
        let model = Arc::new(model);

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("select 'alice' as first_name"),
            None,
        )
        .await;

        // Full refresh alone would be recorded, but an unresolved window leaves
        // nothing sound to write down: invalidate only, never record.
        assert!(
            matches!(
                decision,
                RunCacheServiceDecision::Execute {
                    after_success: RunCacheAfterSuccess::InvalidateFreshness,
                    sao_guard: None,
                }
            ),
            "expected execute-and-invalidate decision, got {decision:?}"
        );
        assert_eq!(client.submitted_count(), 0);
    }

    #[dbt_runtime::test]
    async fn full_refresh_incremental_model_records_execution_without_a_decision() {
        let client = Arc::new(RecordingRunCacheClient::default());
        let ctx = test_task_runner_ctx_with_mode_and_full_refresh(
            Some(client.clone() as dbt_state::service_client::SharedRunCacheServiceClient),
            RunCacheMode::ReadWrite,
            true,
        );
        let model = make_model(
            "model.test.user_name_model",
            "db",
            "dbt_test",
            "user_name_model",
            DbtMaterialization::Incremental,
        );

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("select 'alice' as first_name"),
            None,
        )
        .await;

        match decision {
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::Record(record),
                sao_guard: None,
            } => assert!(
                !record.speculative,
                "the record is built from a full snapshot, not a speculative one"
            ),
            other => panic!("expected execute-with-record decision, got {other:?}"),
        }
        assert_eq!(
            client.submitted_count(),
            0,
            "the full-refresh CTAS must never seed a skip or clone decision"
        );
        assert_eq!(client.speculative_submitted_count(), 0);
    }

    #[dbt_runtime::test]
    async fn unresolved_microbatch_window_invalidates_freshness_after_execution() {
        let client = Arc::new(RecordingRunCacheClient::default());
        let ctx = test_task_runner_ctx(Some(
            client.clone() as dbt_state::service_client::SharedRunCacheServiceClient
        ));
        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = "model.test.user_name_model".to_string();
        model.__common_attr__.language = Some("sql".to_string());
        model.__base_attr__.database = "db".to_string();
        model.__base_attr__.schema = "dbt_test".to_string();
        model.__base_attr__.alias = "user_name_model".to_string();
        model.__base_attr__.materialized = DbtMaterialization::Incremental;
        model.__model_attr__.incremental_strategy =
            Some(dbt_schemas::schemas::common::DbtIncrementalStrategy::Microbatch);
        let model = Arc::new(model);

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("select 'alice' as first_name"),
            None,
        )
        .await;

        // No sound record is possible without the event-time window, so the
        // rebuild goes unobserved and the cached epoch must not survive it.
        assert!(
            matches!(
                decision,
                RunCacheServiceDecision::Execute {
                    after_success: RunCacheAfterSuccess::InvalidateFreshness,
                    sao_guard: None,
                }
            ),
            "expected execute-and-invalidate decision, got {decision:?}"
        );
        assert_eq!(client.submitted_count(), 0);
    }

    #[dbt_runtime::test]
    async fn custom_materialization_parse_failure_without_manifest_deps_is_incomplete() {
        let ctx = test_task_runner_ctx_with_mode_and_sources_extractor(
            None,
            RunCacheMode::ReadWrite,
            false,
            Arc::new(FailingSourcesExtractor),
        );
        let model = make_model(
            "model.test.user_name_only_model",
            "db",
            "dbt_test",
            "user_name_only_model",
            DbtMaterialization::Unknown("custom_table".to_string()),
        );

        let deps = query_dependencies_for_parse_error(
            &ctx,
            model.as_ref(),
            "'alice' from raw.users",
            synthetic_extraction_error(),
        )
        .await
        .expect("custom materialization parse failures should fail open");

        assert!(!deps.metadata_complete);
        assert!(deps.dependencies.is_empty());
        assert!(deps.seen_tables.is_empty());
        assert!(deps.parser_seen_relations.is_empty());
    }

    #[dbt_runtime::test]
    async fn custom_materialization_standalone_expression_parse_failure_is_complete() {
        let ctx = test_task_runner_ctx_with_mode_and_sources_extractor(
            None,
            RunCacheMode::ReadWrite,
            false,
            Arc::new(ExpressionOnlySourcesExtractor),
        );
        let model = make_model(
            "model.test.user_name_only_model",
            "db",
            "dbt_test",
            "user_name_only_model",
            DbtMaterialization::Unknown("custom_table".to_string()),
        );

        let deps = query_dependencies_for_parse_error(
            &ctx,
            model.as_ref(),
            "'alice'",
            synthetic_extraction_error(),
        )
        .await
        .expect("expression-only custom materialization SQL should be cacheable");

        assert!(deps.metadata_complete);
        assert!(deps.dependencies.is_empty());
        assert!(deps.seen_tables.is_empty());
        assert!(deps.parser_seen_relations.is_empty());
    }

    #[test]
    fn parse_sql_relations_for_adapter_accepts_standalone_expression() {
        let type_ops = DefaultTypeOps::new(AdapterType::Snowflake);
        let relations = parse_sql_relations_for_adapter(
            AdapterType::Snowflake,
            "(select max(id) from raw.users)",
            "db",
            "dbt_test",
            false,
            &ExpressionOnlySourcesExtractor,
            &type_ops,
        )
        .expect("expression-only SQL should produce run-cache relations");

        assert!(relations.contains_key(&fqn_of("db", "raw", "users")));
    }

    #[dbt_runtime::test]
    async fn custom_materialization_standalone_expression_with_no_upstreams_is_complete() {
        let ctx = test_task_runner_ctx_with_mode_and_sources_extractor(
            None,
            RunCacheMode::ReadWrite,
            false,
            Arc::new(EmptySourcesExtractor),
        );
        let model = make_model(
            "model.test.user_name_only_model",
            "db",
            "dbt_test",
            "user_name_only_model",
            DbtMaterialization::Unknown("custom_table".to_string()),
        );

        let deps = query_dependencies_for_parse_error(
            &ctx,
            model.as_ref(),
            "'alice'",
            synthetic_extraction_error(),
        )
        .await
        .expect("expression-only SQL without upstreams should be cacheable");

        assert!(deps.metadata_complete);
        assert!(deps.dependencies.is_empty());
        assert!(deps.seen_tables.is_empty());
        assert!(deps.parser_seen_relations.is_empty());
    }

    #[dbt_runtime::test]
    async fn custom_materialization_bare_relation_reference_falls_back_to_manifest_deps() {
        let mut model = make_model(
            "model.test.copy_target",
            "db",
            "analytics",
            "copy_target",
            DbtMaterialization::Unknown("custom_table".to_string()),
        );
        Arc::make_mut(&mut model)
            .__base_attr__
            .depends_on
            .nodes
            .push("model.test.copy_source".to_string());
        let upstream = make_model(
            "model.test.copy_source",
            "db",
            "analytics",
            "copy_source",
            DbtMaterialization::Table,
        );
        let ctx = test_task_runner_ctx_with_nodes(
            None,
            RunCacheMode::ReadWrite,
            false,
            Arc::new(EmptySourcesExtractor),
            nodes_from(vec![model.clone(), upstream], vec![]),
            ["model.test.copy_target".to_string()].into_iter().collect(),
        );

        let deps =
            collect_query_dependencies(&ctx, model.as_ref(), "db.analytics.copy_source", false)
                .await
                .expect("bare relation reference for a custom materialization should be cacheable");

        assert!(
            !deps.metadata_complete || !deps.seen_tables.is_empty(),
            "must not silently report zero dependencies when depends_on.nodes has a real upstream"
        );
        assert!(deps.dependencies.is_empty());
    }

    #[test]
    fn cacheable_manifest_dependency_relations_resolves_only_cacheable_deps() {
        let mut model = make_model(
            "model.test.copy_target",
            "db",
            "analytics",
            "copy_target",
            DbtMaterialization::Unknown("custom_table".to_string()),
        );
        Arc::make_mut(&mut model)
            .__base_attr__
            .depends_on
            .nodes
            .push("model.test.copy_source".to_string());
        Arc::make_mut(&mut model)
            .__base_attr__
            .depends_on
            .nodes
            .push("exposure.test.some_dashboard".to_string());
        let upstream = make_model(
            "model.test.copy_source",
            "db",
            "analytics",
            "copy_source",
            DbtMaterialization::Table,
        );
        let ctx = test_task_runner_ctx_with_nodes(
            None,
            RunCacheMode::ReadWrite,
            false,
            Arc::new(EmptySourcesExtractor),
            nodes_from(vec![model.clone(), upstream], vec![]),
            ["model.test.copy_target".to_string()].into_iter().collect(),
        );

        let relations = cacheable_manifest_dependency_relations(&ctx, model.as_ref());

        assert_eq!(relations.len(), 1);
        assert!(relations.contains_key(&fqn_of("db", "analytics", "copy_source")));
    }

    #[dbt_runtime::test]
    async fn custom_materialization_parse_error_expression_empty_falls_back_to_manifest_deps() {
        let mut model = make_model(
            "model.test.copy_target",
            "db",
            "analytics",
            "copy_target",
            DbtMaterialization::Unknown("custom_table".to_string()),
        );
        Arc::make_mut(&mut model)
            .__base_attr__
            .depends_on
            .nodes
            .push("model.test.copy_source".to_string());
        let upstream = make_model(
            "model.test.copy_source",
            "db",
            "analytics",
            "copy_source",
            DbtMaterialization::Table,
        );
        let ctx = test_task_runner_ctx_with_nodes(
            None,
            RunCacheMode::ReadWrite,
            false,
            Arc::new(EmptySourcesExtractor),
            nodes_from(vec![model.clone(), upstream], vec![]),
            ["model.test.copy_target".to_string()].into_iter().collect(),
        );

        let deps = query_dependencies_for_parse_error(
            &ctx,
            model.as_ref(),
            "db.analytics.copy_source",
            synthetic_extraction_error(),
        )
        .await
        .expect("bare relation reference for a custom materialization should be cacheable");

        assert!(
            !deps.metadata_complete || !deps.seen_tables.is_empty(),
            "must not silently report zero dependencies when depends_on.nodes has a real upstream"
        );
        assert!(deps.dependencies.is_empty());
    }

    #[dbt_runtime::test]
    async fn custom_materialization_parse_failure_with_manifest_deps_is_incomplete() {
        let mut model = make_model(
            "model.test.user_name_model",
            "db",
            "dbt_test",
            "user_name_model",
            DbtMaterialization::Unknown("custom_table".to_string()),
        );
        Arc::make_mut(&mut model)
            .__base_attr__
            .depends_on
            .nodes
            .push("source.test.raw.users".to_string());
        let source = make_source("source.test.raw.users", "db", "raw", "users");
        let ctx = test_task_runner_ctx_with_nodes(
            None,
            RunCacheMode::ReadWrite,
            false,
            Arc::new(FailingSourcesExtractor),
            nodes_from(vec![model.clone()], vec![source]),
            ["model.test.user_name_model".to_string()]
                .into_iter()
                .collect(),
        );

        let deps = query_dependencies_for_parse_error(
            &ctx,
            model.as_ref(),
            "'alice' from raw.users",
            synthetic_extraction_error(),
        )
        .await
        .expect("custom materialization parse failures should fail open");

        assert!(!deps.metadata_complete);
        assert!(deps.dependencies.is_empty());
        assert!(deps.seen_tables.is_empty());
        assert!(deps.parser_seen_relations.is_empty());
    }

    #[dbt_runtime::test]
    async fn built_in_materialization_parse_failure_returns_error() {
        let ctx = test_task_runner_ctx(None);
        let model = make_model(
            "model.test.user_name_model",
            "db",
            "dbt_test",
            "user_name_model",
            DbtMaterialization::Table,
        );

        assert!(
            query_dependencies_for_parse_error(
                &ctx,
                model.as_ref(),
                "'alice'",
                synthetic_extraction_error(),
            )
            .await
            .is_err()
        );
    }

    #[dbt_runtime::test]
    async fn custom_materialization_compiled_sql_honors_dbt_state_skip() {
        let client = Arc::new(RecordingRunCacheClient::with_response(
            skip_execution_response(),
        ));
        let ctx = test_task_runner_ctx(Some(
            client.clone() as dbt_state::service_client::SharedRunCacheServiceClient
        ));
        let model = make_model(
            "model.test.user_name_only_model",
            "db",
            "dbt_test",
            "user_name_only_model",
            DbtMaterialization::Unknown("custom_table".to_string()),
        );

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("'alice'"),
            None,
        )
        .await;

        assert_eq!(client.submitted_sql(), vec!["'alice'".to_string()]);
        assert!(matches!(
            decision,
            RunCacheServiceDecision::Skip {
                status: NodeStatus::ReusedNoChanges(_),
                ..
            }
        ));
    }

    #[dbt_runtime::test]
    async fn custom_materialization_compiled_sql_records_in_write_only_mode() {
        let client = Arc::new(RecordingRunCacheClient::default());
        let ctx = test_task_runner_ctx_with_mode(
            Some(client.clone() as dbt_state::service_client::SharedRunCacheServiceClient),
            RunCacheMode::WriteOnly,
        );
        let model = make_model(
            "model.test.user_name_only_model",
            "db",
            "dbt_test",
            "user_name_only_model",
            DbtMaterialization::Unknown("custom_table".to_string()),
        );

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("'alice'"),
            None,
        )
        .await;

        assert_eq!(
            client.submitted_sql(),
            Vec::<String>::new(),
            "write-only custom materializations must prepare a record without asking for a service decision"
        );
        assert!(matches!(
            decision,
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::Record(_),
                sao_guard: None,
            }
        ));
    }

    #[dbt_runtime::test]
    async fn confirm_without_confirmation_does_not_refresh_final_metadata() {
        let ctx = test_task_runner_ctx(None);
        let model = make_model(
            "model.test.user_name_model",
            "db",
            "dbt_test",
            "user_name_model",
            DbtMaterialization::Table,
        );
        let target_fqn = fqn_of("db", "dbt_test", "user_name_model");
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_last_modified_epoch(&target_fqn, Some(7));
        ctx.inner
            .run_cache_ctx
            .heuristic_clock
            .set(HeuristicClock {
                start_ts_ms: 1_700_000_000_000,
                start_instant: Instant::now(),
            })
            .unwrap();

        confirm_run_cache_service_execution(&ctx, model.as_ref(), None, None).await;

        assert_eq!(
            ctx.inner
                .run_cache_ctx
                .run_cache_metadata
                .last_modified_epoch(&target_fqn),
            Some(Some(7))
        );
    }

    #[dbt_runtime::test]
    async fn final_last_modified_epoch_caches_the_value_it_returns() {
        let ctx = test_task_runner_ctx(None);
        let model = make_model(
            "model.test.user_name_model",
            "db",
            "dbt_test",
            "user_name_model",
            DbtMaterialization::Incremental,
        );
        let target_fqn = fqn_of("db", "dbt_test", "user_name_model");
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_last_modified_epoch(&target_fqn, Some(7));
        ctx.inner
            .run_cache_ctx
            .heuristic_clock
            .set(HeuristicClock {
                start_ts_ms: 1_700_000_000_000,
                start_instant: Instant::now(),
            })
            .unwrap();

        let epoch = final_last_modified_epoch_for_node(&ctx, model.as_ref())
            .await
            .expect("resolving the final epoch must not fail");

        // Callers persist the returned epoch while downstream nodes read the
        // cached one; a divergence makes the dependency read as changed.
        assert!(epoch.is_some_and(|epoch| epoch >= 1_700_000_000_000));
        assert_eq!(
            ctx.inner
                .run_cache_ctx
                .run_cache_metadata
                .last_modified_epoch(&target_fqn),
            Some(epoch)
        );
    }

    #[test]
    fn heuristic_stamp_replaces_stale_final_metadata() {
        let ctx = test_task_runner_ctx(None);
        let model = make_model(
            "model.test.user_name_model",
            "db",
            "dbt_test",
            "user_name_model",
            DbtMaterialization::Table,
        );
        let target_fqn = fqn_of("db", "dbt_test", "user_name_model");
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_last_modified_epoch(&target_fqn, None);
        ctx.inner
            .run_cache_ctx
            .heuristic_clock
            .set(HeuristicClock {
                start_ts_ms: 1_700_000_000_000,
                start_instant: Instant::now(),
            })
            .unwrap();

        let epoch = stamp_final_last_modified_epoch_for_node_heuristic(&ctx, model.as_ref());

        assert!(epoch.is_some());
        assert_eq!(
            ctx.inner
                .run_cache_ctx
                .run_cache_metadata
                .last_modified_epoch(&target_fqn),
            epoch.map(Some)
        );
    }

    #[test]
    fn clear_stale_missing_epoch_removes_stale_missing_value() {
        let ctx = test_task_runner_ctx(None);
        let model = make_model(
            "model.test.user_name_model",
            "db",
            "dbt_test",
            "user_name_model",
            DbtMaterialization::Table,
        );
        let target_fqn = fqn_of("db", "dbt_test", "user_name_model");
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_last_modified_epoch(&target_fqn, None);

        clear_stale_missing_last_modified_epoch_for_node(&ctx, model.as_ref());

        assert_eq!(
            ctx.inner
                .run_cache_ctx
                .run_cache_metadata
                .last_modified_epoch(&target_fqn),
            None
        );
    }

    #[test]
    fn clear_stale_missing_epoch_preserves_valid_cached_epoch() {
        let ctx = test_task_runner_ctx(None);
        let model = make_model(
            "model.test.user_name_model",
            "db",
            "dbt_test",
            "user_name_model",
            DbtMaterialization::Table,
        );
        let target_fqn = fqn_of("db", "dbt_test", "user_name_model");
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_last_modified_epoch(&target_fqn, Some(1_700_000_000_000));

        clear_stale_missing_last_modified_epoch_for_node(&ctx, model.as_ref());

        assert_eq!(
            ctx.inner
                .run_cache_ctx
                .run_cache_metadata
                .last_modified_epoch(&target_fqn),
            Some(Some(1_700_000_000_000))
        );
    }

    #[test]
    fn evict_node_metadata_for_untracked_rebuild_clears_valid_epoch_and_existence() {
        let ctx = test_task_runner_ctx(None);
        let model = make_model(
            "model.test.user_name_model",
            "db",
            "dbt_test",
            "user_name_model",
            DbtMaterialization::Table,
        );
        let target_fqn = fqn_of("db", "dbt_test", "user_name_model");
        // A real (valid-looking) cached epoch and existence, which the benign
        // skip path would preserve — but a failed request means the rebuild was
        // unobserved, so both must be evicted here.
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_last_modified_epoch(&target_fqn, Some(1_700_000_000_000));
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_relation_exists(&target_fqn, true);

        evict_node_metadata_for_untracked_rebuild(&ctx, model.as_ref());

        assert_eq!(
            ctx.inner
                .run_cache_ctx
                .run_cache_metadata
                .last_modified_epoch(&target_fqn),
            None
        );
        assert_eq!(
            ctx.inner
                .run_cache_ctx
                .run_cache_metadata
                .relation_exists(&target_fqn),
            None
        );
    }

    #[dbt_runtime::test]
    async fn table_materialization_submits_rendered_sql_before_materialization() {
        let client = Arc::new(RecordingRunCacheClient::default());
        let ctx = test_task_runner_ctx(Some(
            client.clone() as dbt_state::service_client::SharedRunCacheServiceClient
        ));
        let model = make_model(
            "model.test.user_name_model",
            "db",
            "dbt_test",
            "user_name_model",
            DbtMaterialization::Table,
        );

        assert!(matches!(
            run_cache_service_before_execution(
                &ctx,
                model.as_ref(),
                &task_result_with_sql("select 'alice' as first_name"),
                None,
            )
            .await,
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::Confirm(_),
                sao_guard: None,
            }
        ));
        assert_eq!(
            client.submitted_sql(),
            vec!["select 'alice' as first_name".to_string()]
        );
    }

    #[dbt_runtime::test]
    async fn unresolved_upstream_last_modified_keeps_metadata_complete() {
        let mut model = make_model(
            "model.test.fact_orders",
            "db",
            "analytics",
            "fact_orders",
            DbtMaterialization::Table,
        );
        Arc::make_mut(&mut model)
            .__base_attr__
            .depends_on
            .nodes
            .push("source.test.raw.orders".to_string());
        let source = make_source("source.test.raw.orders", "db", "raw", "orders");
        let ctx = test_task_runner_ctx_with_nodes(
            None,
            RunCacheMode::ReadWrite,
            false,
            Arc::new(EmptySourcesExtractor),
            nodes_from(vec![model.clone()], vec![source]),
            ["model.test.fact_orders".to_string()].into_iter().collect(),
        );
        let target_fqn = fqn_of("db", "analytics", "fact_orders");
        let upstream_fqn = fqn_of("db", "raw", "orders");
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_last_modified_epoch(&target_fqn, Some(123));
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_last_modified_epoch(&upstream_fqn, None);

        let tables = collect_table_modified_infos(
            &ctx,
            model.as_ref(),
            false,
            &BTreeSet::from([upstream_fqn.clone()]),
            &BTreeMap::new(),
            &BTreeSet::new(),
            false,
        )
        .await
        .unwrap();

        assert!(tables.metadata_complete);
        assert!(tables.tables.contains(&TableModifiedInfo {
            name: target_fqn,
            last_modified_epoch: Some(123),
        }));
        assert!(tables.tables.contains(&TableModifiedInfo {
            name: upstream_fqn,
            last_modified_epoch: None,
        }));
    }

    // ── speculative submit tests ─────────────────────────────────────────────

    /// A model with a single upstream dependency whose last-modified epoch is
    /// deliberately left unresolved (a genuine cache miss). The target's epoch
    /// is pre-resolved so only the upstream exercises the miss path.
    fn ctx_model_with_unresolved_upstream() -> (TaskRunnerCtx, Arc<DbtModel>, String) {
        let mut model = make_model(
            "model.test.fact_orders",
            "db",
            "analytics",
            "fact_orders",
            DbtMaterialization::Table,
        );
        Arc::make_mut(&mut model)
            .__base_attr__
            .depends_on
            .nodes
            .push("source.test.raw.orders".to_string());
        let source = make_source("source.test.raw.orders", "db", "raw", "orders");
        let ctx = test_task_runner_ctx_with_nodes(
            None,
            RunCacheMode::ReadWrite,
            false,
            Arc::new(EmptySourcesExtractor),
            nodes_from(vec![model.clone()], vec![source]),
            ["model.test.fact_orders".to_string()].into_iter().collect(),
        );
        let target_fqn = fqn_of("db", "analytics", "fact_orders");
        let upstream_fqn = fqn_of("db", "raw", "orders");
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_last_modified_epoch(&target_fqn, Some(123));
        (ctx, model, upstream_fqn)
    }

    #[dbt_runtime::test]
    async fn speculative_build_leaves_unresolved_upstream_unrefreshed() {
        let (ctx, model, upstream_fqn) = ctx_model_with_unresolved_upstream();

        let tables = collect_table_modified_infos(
            &ctx,
            model.as_ref(),
            false,
            &BTreeSet::from([upstream_fqn.clone()]),
            &BTreeMap::new(),
            &BTreeSet::new(),
            true,
        )
        .await
        .unwrap();

        assert!(tables.metadata_complete);
        assert!(tables.tables.contains(&TableModifiedInfo {
            name: upstream_fqn.clone(),
            last_modified_epoch: None,
        }));
        // A speculative build issues no blocking per-node freshness query, so an
        // unresolved upstream stays a genuine cache miss (never inserted).
        assert_eq!(
            ctx.inner
                .run_cache_ctx
                .run_cache_metadata
                .last_modified_epoch(&upstream_fqn),
            None,
        );
    }

    #[dbt_runtime::test]
    async fn non_speculative_build_refreshes_unresolved_upstream() {
        let (ctx, model, upstream_fqn) = ctx_model_with_unresolved_upstream();

        let tables = collect_table_modified_infos(
            &ctx,
            model.as_ref(),
            false,
            &BTreeSet::from([upstream_fqn.clone()]),
            &BTreeMap::new(),
            &BTreeSet::new(),
            false,
        )
        .await
        .unwrap();

        assert!(tables.metadata_complete);
        assert!(tables.tables.contains(&TableModifiedInfo {
            name: upstream_fqn.clone(),
            last_modified_epoch: None,
        }));
        // A non-speculative build resolves the miss with a blocking freshness
        // lookup; with no adapter wired in the test env it records an explicit
        // unresolved entry, distinguishing it from the speculative peek above.
        assert_eq!(
            ctx.inner
                .run_cache_ctx
                .run_cache_metadata
                .last_modified_epoch(&upstream_fqn),
            Some(None),
        );
    }

    fn speculative_test_model() -> Arc<DbtModel> {
        make_model(
            "model.test.user_name_model",
            "db",
            "dbt_test",
            "user_name_model",
            DbtMaterialization::Table,
        )
    }

    #[dbt_runtime::test]
    async fn speculative_skip_verdict_returns_skip_without_regular_submit() {
        let client = Arc::new(RecordingRunCacheClient::with_speculative_response(
            speculative_skip_response(),
        ));
        let ctx = test_task_runner_ctx(Some(
            client.clone() as dbt_state::service_client::SharedRunCacheServiceClient
        ));
        let model = speculative_test_model();

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("select 'alice' as first_name"),
            None,
        )
        .await;

        assert!(matches!(
            decision,
            RunCacheServiceDecision::Skip {
                status: NodeStatus::ReusedNoChanges(_),
                ..
            }
        ));
        assert_eq!(client.speculative_submitted_count(), 1);
        assert_eq!(
            client.submitted_count(),
            0,
            "a speculative skip verdict must not fall through to a regular submit"
        );
    }

    #[dbt_runtime::test]
    async fn speculative_clone_verdict_returns_clone_without_regular_submit() {
        let client = Arc::new(RecordingRunCacheClient::with_speculative_response(
            speculative_clone_response(),
        ));
        let ctx = test_task_runner_ctx(Some(
            client.clone() as dbt_state::service_client::SharedRunCacheServiceClient
        ));
        let model = speculative_test_model();

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("select 'alice' as first_name"),
            None,
        )
        .await;

        let RunCacheServiceDecision::Clone { clone } = decision else {
            panic!("expected clone decision from speculative verdict");
        };
        assert_eq!(clone.request_id, "clone-request");
        assert_eq!(client.speculative_submitted_count(), 1);
        assert_eq!(
            client.submitted_count(),
            0,
            "a speculative clone verdict must not fall through to a regular submit"
        );
    }

    #[dbt_runtime::test]
    async fn speculative_untracked_verdict_executes_and_records_speculatively() {
        let client = Arc::new(RecordingRunCacheClient::with_speculative_response(
            speculative_untracked_response(),
        ));
        let ctx = test_task_runner_ctx(Some(
            client.clone() as dbt_state::service_client::SharedRunCacheServiceClient
        ));
        let model = speculative_test_model();

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("select 'alice' as first_name"),
            None,
        )
        .await;

        match decision {
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::Record(record),
                sao_guard: None,
            } => {
                assert!(
                    record.speculative,
                    "an untracked-execute verdict must record after the fact from the speculative request"
                );
            }
            other => panic!("expected execute-with-record decision, got {other:?}"),
        }
        assert_eq!(client.speculative_submitted_count(), 1);
        assert_eq!(
            client.submitted_count(),
            0,
            "the untracked-execute path executes now and does not issue a regular submit"
        );
        // The untracked-execute path intentionally does not await the prefetch.
        assert!(
            !ctx.inner.run_cache_ctx.prefetch.is_done(),
            "the untracked-execute path must not block on the prefetch"
        );
    }

    #[dbt_runtime::test]
    async fn speculative_undecided_verdict_falls_back_to_regular_submit() {
        let client = Arc::new(RecordingRunCacheClient::with_speculative_response(
            speculative_undecided_response(),
        ));
        let ctx = test_task_runner_ctx(Some(
            client.clone() as dbt_state::service_client::SharedRunCacheServiceClient
        ));
        let model = speculative_test_model();

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("select 'alice' as first_name"),
            None,
        )
        .await;

        assert!(matches!(
            decision,
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::Confirm(_),
                sao_guard: None,
            }
        ));
        assert_eq!(client.speculative_submitted_count(), 1);
        assert_eq!(
            client.submitted_count(),
            1,
            "an undecided verdict must fall back to a regular submit"
        );
        assert!(
            ctx.inner.run_cache_ctx.prefetch.is_done(),
            "the fallback path awaits the prefetch before resubmitting"
        );
    }

    #[dbt_runtime::test]
    async fn speculative_rpc_error_falls_back_to_regular_submit() {
        // A default client answers the speculative RPC with `Disabled`.
        let client = Arc::new(RecordingRunCacheClient::default());
        let ctx = test_task_runner_ctx(Some(
            client.clone() as dbt_state::service_client::SharedRunCacheServiceClient
        ));
        let model = speculative_test_model();

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("select 'alice' as first_name"),
            None,
        )
        .await;

        assert!(matches!(
            decision,
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::Confirm(_),
                sao_guard: None,
            }
        ));
        assert_eq!(client.speculative_submitted_count(), 1);
        assert_eq!(
            client.submitted_count(),
            1,
            "a speculative RPC error must gracefully fall back to a regular submit"
        );
    }

    #[dbt_runtime::test]
    async fn prefetch_ready_skips_speculation_and_uses_regular_submit() {
        // Canned speculative skip verdict that must never be consulted because
        // the prefetch is already complete.
        let client = Arc::new(RecordingRunCacheClient::with_speculative_response(
            speculative_skip_response(),
        ));
        let ctx = test_task_runner_ctx(Some(
            client.clone() as dbt_state::service_client::SharedRunCacheServiceClient
        ));
        ctx.inner.run_cache_ctx.prefetch.mark_started();
        ctx.inner.run_cache_ctx.prefetch.mark_done();
        let model = speculative_test_model();

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("select 'alice' as first_name"),
            None,
        )
        .await;

        assert!(matches!(
            decision,
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::Confirm(_),
                sao_guard: None,
            }
        ));
        assert_eq!(
            client.speculative_submitted_count(),
            0,
            "a ready prefetch must skip the speculative RPC entirely"
        );
        assert_eq!(client.submitted_count(), 1);
    }

    #[dbt_runtime::test]
    async fn prefetch_completing_during_request_build_skips_speculation() {
        // Regression test for the last-responsible-moment re-check. The
        // speculative-vs-regular decision is taken at entry, before
        // `build_sql_request` runs. In production `build_sql_request` blocks on
        // the view-definition traversal (inside `build_sql_context`), during
        // which the background prefetch frequently completes. This test drives
        // the same observable condition deterministically: the prefetch is in
        // flight at entry but becomes ready before `build_sql_request` returns.
        // The node must then re-check and submit non-speculatively, leaving the
        // canned skip verdict below unconsulted.
        let client = Arc::new(RecordingRunCacheClient::with_speculative_response(
            speculative_skip_response(),
        ));
        let shared: dbt_state::service_client::SharedRunCacheServiceClient = client.clone();
        let ctx = test_task_runner_ctx(Some(shared.clone()));
        // Started but not yet done: the entry-point readiness check sees the
        // prefetch in flight and enters the speculative branch.
        ctx.inner.run_cache_ctx.prefetch.mark_started();
        let model = speculative_test_model();

        // The `build_request` closure is invoked at the end of
        // `build_sql_request`, after `build_sql_context` returns (that is where
        // the view traversal runs in production). Marking the prefetch done here
        // models it completing by the time the request build finishes — exactly
        // the state the re-check inspects. `build_and_submit_non_speculative`
        // invokes the closure a second time; `mark_done` is idempotent.
        let build = |context| {
            ctx.inner.run_cache_ctx.prefetch.mark_done();
            build_model_sql_request(
                model.as_ref(),
                context,
                &ctx.inner.materialization_resolver,
                |_| None,
            )
        };

        let result = submit_sql_with_speculation(
            &ctx,
            model.as_ref(),
            "select 'alice' as first_name".to_string(),
            false,
            false,
            None,
            &shared,
            build,
        )
        .await
        .unwrap();

        assert!(result.is_some());
        assert_eq!(
            client.speculative_submitted_count(),
            0,
            "a prefetch that completes during the build must skip the speculative RPC"
        );
        assert_eq!(
            client.submitted_count(),
            1,
            "the node must fall through to a single non-speculative submit"
        );
    }

    #[dbt_runtime::test]
    async fn write_only_mode_skips_speculation_and_produces_record() {
        let client = Arc::new(RecordingRunCacheClient::with_speculative_response(
            speculative_skip_response(),
        ));
        let ctx = test_task_runner_ctx_with_mode(
            Some(client.clone() as dbt_state::service_client::SharedRunCacheServiceClient),
            RunCacheMode::WriteOnly,
        );
        let model = speculative_test_model();

        let decision = run_cache_service_before_execution(
            &ctx,
            model.as_ref(),
            &task_result_with_sql("select 'alice' as first_name"),
            None,
        )
        .await;

        match decision {
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::Record(record),
                sao_guard: None,
            } => {
                assert!(
                    !record.speculative,
                    "a write-only record is built from a full snapshot, not a speculative one"
                );
            }
            other => panic!("expected execute-with-record decision, got {other:?}"),
        }
        assert_eq!(
            client.speculative_submitted_count(),
            0,
            "write-only mode never asks the service for a decision"
        );
        assert_eq!(client.submitted_count(), 0);
    }

    #[test]
    fn is_prefetch_ready_requires_started_and_done() {
        let ctx = test_task_runner_ctx(None);
        assert!(
            !is_prefetch_ready(&ctx),
            "not ready before the prefetch is started"
        );
        ctx.inner.run_cache_ctx.prefetch.mark_started();
        assert!(
            !is_prefetch_ready(&ctx),
            "not ready while the prefetch is in flight"
        );
        ctx.inner.run_cache_ctx.prefetch.mark_done();
        assert!(
            is_prefetch_ready(&ctx),
            "ready once the prefetch is both started and done"
        );
    }

    #[dbt_runtime::test]
    async fn finalize_speculative_request_fills_real_epochs_and_marks_flag() {
        let ctx = test_task_runner_ctx(None);
        let model = speculative_test_model();

        // One dependency the warm cache has resolved to a real epoch, and one
        // the post-prefetch refresh resolves as unknown (the test env has no
        // adapter, so the miss refresh caches an explicit `Some(None)`).
        let warm_fqn = fqn_of("db", "analytics", "warm_dep");
        let cold_fqn = fqn_of("db", "raw", "cold_dep");
        ctx.inner
            .run_cache_ctx
            .run_cache_metadata
            .insert_last_modified_epoch(&warm_fqn, Some(999));

        // The speculative request carries partial epochs: a stale placeholder for
        // the warm dep and a best-effort value for the cold dep.
        let request = SubmitEnrichedSqlRequest {
            tables: vec![
                TableModifiedInfo {
                    name: warm_fqn.clone(),
                    last_modified_epoch: Some(111),
                },
                TableModifiedInfo {
                    name: cold_fqn.clone(),
                    last_modified_epoch: Some(222),
                },
            ],
            ..Default::default()
        };

        let finalized = finalize_speculative_sql_request(&ctx, model.as_ref(), request).await;

        assert_eq!(
            finalized.tables,
            vec![
                // Refreshed to the real epoch the warm cache now holds.
                TableModifiedInfo {
                    name: warm_fqn.clone(),
                    last_modified_epoch: Some(999),
                },
                // The post-prefetch refresh resolved this as unknown, so the
                // stale speculative value is cleared to unset rather than kept.
                TableModifiedInfo {
                    name: cold_fqn.clone(),
                    last_modified_epoch: None,
                },
            ],
        );

        // The recorded execution built from a finalized speculative request must
        // carry `from_speculative_submit = true`.
        let outcome = ExecutionOutcomeInput {
            last_modified_epoch: Some(123),
            table_type: None,
            execution_runtime_ms: None,
        };
        let record = sql_execution_record_from_submit_request(finalized, outcome, true);
        let Some(execution_record::Input::EnrichedSql(sql)) = record.input else {
            panic!("expected an enriched-SQL execution record");
        };
        assert!(sql.from_speculative_submit);
    }

    #[test]
    fn env_requested_service_uses_read_write_when_cli_mode_is_noop() {
        assert!(effective_run_cache_service_use_cache(
            &RunCacheMode::Noop,
            true
        ));
        assert!(!effective_run_cache_service_use_cache(
            &RunCacheMode::Noop,
            false
        ));
        assert!(!effective_run_cache_service_use_cache(
            &RunCacheMode::WriteOnly,
            true
        ));
        assert!(effective_run_cache_service_use_cache(
            &RunCacheMode::ReadWrite,
            false
        ));
    }

    #[test]
    fn metadata_query_options_prefers_profile_metadata_warehouse() {
        let options = metadata_query_options_for_warehouses(
            Some("profile_wh".to_string()),
            Some("legacy_wh".to_string()),
        );

        assert_eq!(options.warehouse.as_deref(), Some("profile_wh"));
    }

    #[test]
    fn metadata_query_options_falls_back_to_legacy_service_warehouse() {
        let options = metadata_query_options_for_warehouses(None, Some("legacy_wh".to_string()));

        assert_eq!(options.warehouse.as_deref(), Some("legacy_wh"));
    }

    #[test]
    fn warns_on_slow_snowflake_prefetch_without_metadata_warehouse() {
        let no_warehouse = MetadataQueryOptions::default();
        let with_warehouse = MetadataQueryOptions {
            warehouse: Some("META_WH".to_string()),
            ..MetadataQueryOptions::default()
        };
        let slow = SLOW_METADATA_PREFETCH_WARN_THRESHOLD;
        let fast = SLOW_METADATA_PREFETCH_WARN_THRESHOLD - std::time::Duration::from_secs(1);

        // Slow, Snowflake, no dedicated warehouse → hint.
        assert!(should_warn_slow_metadata_prefetch(
            AdapterType::Snowflake,
            &no_warehouse,
            slow
        ));

        // A dedicated warehouse is already the fix → stay quiet.
        assert!(!should_warn_slow_metadata_prefetch(
            AdapterType::Snowflake,
            &with_warehouse,
            slow
        ));

        // Fast prefetch → nothing to hint.
        assert!(!should_warn_slow_metadata_prefetch(
            AdapterType::Snowflake,
            &no_warehouse,
            fast
        ));

        // The `metadata_warehouse` config is Snowflake-specific.
        assert!(!should_warn_slow_metadata_prefetch(
            AdapterType::Bigquery,
            &no_warehouse,
            slow
        ));
    }

    #[test]
    fn execute_hooks_on_any_reuse_uses_state_config_for_skip_reuse() {
        let model = model_with_state(ModelState {
            lag_tolerance: None,
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: Some(true),
            compare_unrendered_code: None,
        });

        assert!(should_execute_hooks_for_skip_reuse(&model, false));
    }

    #[test]
    fn skip_reuse_hooks_fall_back_to_service_default() {
        let model = model_with_state(ModelState {
            lag_tolerance: None,
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
        });

        assert!(should_execute_hooks_for_skip_reuse(&model, true));
    }

    #[test]
    fn freshness_tolerance_uses_state_lag_tolerance() {
        let model = model_with_state(ModelState {
            lag_tolerance: Some(ModelFreshnessRules {
                count: Some(2),
                period: Some(FreshnessPeriod::hour),
                updates_on: None,
            }),
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
        });

        assert_eq!(freshness_tolerance_seconds_for_node(&model, 2700), 7200);
    }

    #[test]
    fn freshness_tolerance_falls_back_to_legacy_build_after() {
        let mut model = model_with_state(ModelState {
            lag_tolerance: None,
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
        });
        model.__model_attr__.freshness = Some(ModelFreshness {
            build_after: Some(ModelFreshnessRules {
                count: Some(1),
                period: Some(FreshnessPeriod::day),
                updates_on: None,
            }),
            ..Default::default()
        });

        assert_eq!(freshness_tolerance_seconds_for_node(&model, 2700), 86400);
    }

    #[test]
    fn freshness_tolerance_prefers_state_lag_tolerance_over_legacy_build_after() {
        let mut model = model_with_state(ModelState {
            lag_tolerance: Some(ModelFreshnessRules {
                count: Some(2),
                period: Some(FreshnessPeriod::hour),
                updates_on: None,
            }),
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
        });
        model.__model_attr__.freshness = Some(ModelFreshness {
            build_after: Some(ModelFreshnessRules {
                count: Some(1),
                period: Some(FreshnessPeriod::day),
                updates_on: None,
            }),
            ..Default::default()
        });

        assert_eq!(freshness_tolerance_seconds_for_node(&model, 2700), 7200);
    }

    #[test]
    fn request_freshness_tolerance_for_data_tests_is_always_zero() {
        let test = DbtTest::default();

        assert_eq!(
            request_freshness_tolerance_seconds_for_node(&test, false, 2700),
            0
        );
    }

    #[test]
    fn state_require_fresh_data_from_overrides_legacy_updates_on() {
        let mut model = model_with_state(ModelState {
            lag_tolerance: None,
            require_fresh_data_from: Some(UpdatesOn::Any),
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
        });
        model.__model_attr__.freshness = Some(ModelFreshness {
            build_after: Some(ModelFreshnessRules {
                count: Some(1),
                period: Some(FreshnessPeriod::hour),
                updates_on: Some(UpdatesOn::All),
            }),
            ..Default::default()
        });

        assert_eq!(
            stale_upstream_policy_for_node(&model),
            dbt_state::proto::query_cache::StaleUpstreamPolicy::Any
        );
    }

    #[test]
    fn state_evaluate_volatile_sql_true_disables_tolerating_nondeterminism() {
        let mut model = model_with_state(ModelState {
            lag_tolerance: None,
            require_fresh_data_from: None,
            evaluate_volatile_sql: Some(true),
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
        });
        model.__common_attr__.meta.insert(
            "run_cache_tolerate_nondeterminism".to_string(),
            dbt_yaml::Value::Bool(true, dbt_yaml::Span::default()),
        );

        assert!(!resolve_tolerate_nondeterminism(&model, true));
    }

    fn snapshot_with_state(state: ModelState) -> DbtSnapshot {
        let mut snapshot = DbtSnapshot::default();
        snapshot.__common_attr__.unique_id = "snapshot.test.orders_snapshot".to_string();
        snapshot.__snapshot_attr__.state = Some(state);
        snapshot
    }

    #[test]
    fn snapshot_state_config_is_honored_by_run_cache() {
        // Snapshots carry the full `ModelState` (5 keys); every run-cache accessor must read it.
        let snapshot = snapshot_with_state(ModelState {
            lag_tolerance: Some(ModelFreshnessRules {
                count: Some(2),
                period: Some(FreshnessPeriod::hour),
                updates_on: None,
            }),
            require_fresh_data_from: Some(UpdatesOn::All),
            evaluate_volatile_sql: Some(true),
            pre_clone: None,
            execute_hooks_on_any_reuse: Some(true),
            compare_unrendered_code: None,
        });

        assert!(should_execute_hooks_for_skip_reuse(&snapshot, false));
        assert_eq!(freshness_tolerance_seconds_for_node(&snapshot, 2700), 7200);
        assert!(!resolve_tolerate_nondeterminism(&snapshot, true));
        assert_eq!(
            stale_upstream_policy_for_node(&snapshot),
            dbt_state::proto::query_cache::StaleUpstreamPolicy::All
        );
    }

    fn data_test_with_state(state: DataTestState) -> DbtTest {
        let mut test = DbtTest::default();
        test.__common_attr__.unique_id = "test.test.not_null_orders_id".to_string();
        test.__test_attr__.state = Some(state);
        test
    }

    #[test]
    fn data_test_state_config_is_honored_by_run_cache() {
        // Data tests carry the restricted `DataTestState` (2 keys). The two supported configs are
        // honored; the 5-key-only configs (e.g. execute_hooks_on_any_reuse) are not modeled and fall
        // back to the service default.
        let test = data_test_with_state(DataTestState {
            require_fresh_data_from: Some(UpdatesOn::All),
            evaluate_volatile_sql: Some(true),
            compare_unrendered_code: None,
        });

        assert!(!resolve_tolerate_nondeterminism(&test, true));
        assert_eq!(
            stale_upstream_policy_for_node(&test),
            dbt_state::proto::query_cache::StaleUpstreamPolicy::All
        );
        assert!(should_execute_hooks_for_skip_reuse(&test, true));
        assert!(!should_execute_hooks_for_skip_reuse(&test, false));
    }

    #[test]
    fn compare_unrendered_code_falls_back_to_service_default() {
        let model = model_with_state(ModelState {
            lag_tolerance: None,
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: None,
        });

        assert!(resolve_compare_unrendered_code(&model, true));
        assert!(!resolve_compare_unrendered_code(&model, false));
        // A node with no `state:` block at all resolves the same way.
        assert!(resolve_compare_unrendered_code(&DbtModel::default(), true));
    }

    #[test]
    fn compare_unrendered_code_node_config_wins_over_service_default() {
        // Both directions: the node config overrides the default whichever way it points.
        let enabled = model_with_state(ModelState {
            lag_tolerance: None,
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: Some(true),
        });
        let disabled = model_with_state(ModelState {
            lag_tolerance: None,
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: Some(false),
        });

        assert!(resolve_compare_unrendered_code(&enabled, false));
        assert!(!resolve_compare_unrendered_code(&disabled, true));
    }

    #[test]
    fn compare_unrendered_code_resolves_for_snapshots_and_data_tests() {
        let snapshot = snapshot_with_state(ModelState {
            lag_tolerance: None,
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse: None,
            compare_unrendered_code: Some(true),
        });
        let test = data_test_with_state(DataTestState {
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            compare_unrendered_code: Some(true),
        });

        assert!(resolve_compare_unrendered_code(&snapshot, false));
        assert!(resolve_compare_unrendered_code(&test, false));
    }

    #[test]
    fn lenient_dependencies_follow_config_and_final_deferred_fqns() {
        let deferred_fqns = BTreeSet::from([
            "prod.analytics.customers".to_string(),
            "prod.analytics.orders".to_string(),
            "prod.analytics.unrelated".to_string(),
        ]);
        let tables = vec![TableModifiedInfo {
            name: "prod.analytics.customers".to_string(),
            last_modified_epoch: Some(123),
        }];
        let query_dependencies = vec![QueryDependency {
            name: "prod.analytics.orders".to_string(),
            query: "select * from prod.raw.orders".to_string(),
            default_catalog: "prod".to_string(),
            default_schema: "analytics".to_string(),
        }];

        assert_eq!(
            build_lenient_dependencies(true, &deferred_fqns, &tables, &query_dependencies),
            vec![
                "prod.analytics.customers".to_string(),
                "prod.analytics.orders".to_string(),
            ]
        );
        assert_eq!(
            build_lenient_dependencies(true, &deferred_fqns, &[], &[]),
            Vec::<String>::new()
        );
        assert!(
            build_lenient_dependencies(false, &deferred_fqns, &tables, &query_dependencies)
                .is_empty()
        );
    }

    #[test]
    fn collected_view_query_dependencies_for_view_is_empty_and_complete() {
        // The view fast-path in `collect_query_dependencies` must produce no
        // upstream dependencies, no seen tables, and no parser relations, while
        // still marking the result complete so the submit isn't skipped.
        // Matches the dbt-state Python plugin's view path
        // (clients/dbt_state/src/dbt_state/run_cache.py:1116-1146).
        let deps = CollectedViewQueryDependencies::for_view();

        assert!(deps.dependencies.is_empty());
        assert!(deps.seen_tables.is_empty());
        assert!(deps.parser_seen_relations.is_empty());
        assert!(deps.metadata_complete);
    }

    #[test]
    fn lenient_dependencies_can_use_query_dependencies_without_tables() {
        let deferred_fqns = BTreeSet::from([
            "prod.analytics.customers".to_string(),
            "prod.analytics.orders".to_string(),
        ]);
        let query_dependencies = vec![QueryDependency {
            name: "prod.analytics.orders".to_string(),
            query: "select * from prod.raw.orders".to_string(),
            default_catalog: "prod".to_string(),
            default_schema: "analytics".to_string(),
        }];

        assert_eq!(
            build_lenient_dependencies(true, &deferred_fqns, &[], &query_dependencies),
            vec!["prod.analytics.orders".to_string()]
        );
    }

    #[test]
    fn ready_to_clone_returns_clone_decision_in_read_write_mode() {
        let decision =
            record_service_decision("model.test.orders", &ready_to_clone_response(), 0, true);

        let RunCacheServiceDecision::Clone { clone } = decision else {
            panic!("expected clone decision");
        };
        assert_eq!(clone.request_id, "clone-request");
        assert_eq!(clone.clone_sqls, vec!["create table target clone source"]);
        assert_eq!(clone.clone_source, "source");
        assert_eq!(clone.clone_target, "target");
        assert_eq!(clone.required_source_epoch, Some(123));
        assert_eq!(clone.execution_runtime_ms, Some(456));
        assert!(clone.execution_results.is_some());
        assert_eq!(
            clone.success_confirmation(),
            Some(RunCacheExecutionConfirmation {
                request_id: "clone-request".to_string(),
                failed_to_clone: false,
                execution_results: clone.execution_results.clone(),
                execution_runtime_ms: Some(456),
            })
        );
        assert_eq!(
            clone.fallback_confirmation(),
            Some(RunCacheExecutionConfirmation {
                request_id: "clone-request".to_string(),
                failed_to_clone: true,
                execution_results: None,
                execution_runtime_ms: None,
            })
        );
    }

    #[test]
    fn ready_to_clone_stale_decision_reports_clone_still_fresh_status() {
        let mut response = ready_to_clone_response();
        let Some(submit_sql_response::Response::ReadyToClone(clone_response)) =
            response.response.as_mut()
        else {
            panic!("expected clone response");
        };
        clone_response.explained_decision = Some(ExplainedDecision {
            is_stale: true,
            ..Default::default()
        });

        let decision = record_service_decision("model.test.orders", &response, 3600, true);
        let RunCacheServiceDecision::Clone { clone } = decision else {
            panic!("expected clone decision");
        };
        assert_eq!(clone.success_status(), NodeStatus::ReusedCloned(Some(3600)));
    }

    #[test]
    fn ready_to_clone_is_ignored_in_write_only_mode() {
        assert_eq!(
            record_service_decision("model.test.orders", &ready_to_clone_response(), 0, false),
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::Confirm(RunCacheExecutionConfirmation {
                    request_id: "clone-request".to_string(),
                    failed_to_clone: true,
                    execution_results: None,
                    execution_runtime_ms: None,
                }),
                sao_guard: None,
            }
        );
    }

    #[test]
    fn empty_response_executes_without_confirmation() {
        assert_eq!(
            record_service_decision("model.test.orders", &empty_response(), 0, true),
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::None,
                sao_guard: None,
            }
        );
    }

    fn ready_to_execute_response() -> SubmitSqlResponse {
        SubmitSqlResponse {
            response: Some(submit_sql_response::Response::ReadyToExecute(
                ReadyToExecuteResponse {
                    request_id: "execute-request".to_string(),
                    ..Default::default()
                },
            )),
        }
    }

    fn skip_execution_response() -> SubmitSqlResponse {
        SubmitSqlResponse {
            response: Some(submit_sql_response::Response::SkipExecution(
                SkipExecutionResponse::default(),
            )),
        }
    }

    fn skip_execution_response_with_test_result(
        result: CachedTestExecutionResult,
    ) -> SubmitSqlResponse {
        SubmitSqlResponse {
            response: Some(submit_sql_response::Response::SkipExecution(
                SkipExecutionResponse {
                    execution_results: Some(build_test_execution_results_struct(result)),
                    ..Default::default()
                },
            )),
        }
    }

    fn test_execution_results_with_failures_only(failures: i64) -> Struct {
        let mut fields = HashMap::new();
        fields.insert(
            "failures".to_string(),
            Value {
                kind: Some(Kind::IntValue(failures)),
            },
        );
        Struct { fields }
    }

    fn ready_to_clone_response() -> SubmitSqlResponse {
        SubmitSqlResponse {
            response: Some(submit_sql_response::Response::ReadyToClone(
                ReadyToCloneResponse {
                    request_id: "clone-request".to_string(),
                    clone_sqls: vec!["create table target clone source".to_string()],
                    clone_source: "source".to_string(),
                    clone_target: "target".to_string(),
                    clone_required_last_modified_epoch: Some(123),
                    clone_execution_results: Some(Struct::default()),
                    execution_runtime_ms: Some(456),
                    ..Default::default()
                },
            )),
        }
    }

    fn empty_response() -> SubmitSqlResponse {
        SubmitSqlResponse { response: None }
    }

    fn speculative_skip_response() -> SubmitSqlSpeculativeResponse {
        SubmitSqlSpeculativeResponse {
            response: Some(submit_sql_speculative_response::Response::SkipExecution(
                SkipExecutionResponse::default(),
            )),
        }
    }

    fn speculative_clone_response() -> SubmitSqlSpeculativeResponse {
        SubmitSqlSpeculativeResponse {
            response: Some(submit_sql_speculative_response::Response::ReadyToClone(
                ReadyToCloneResponse {
                    request_id: "clone-request".to_string(),
                    clone_sqls: vec!["create table target clone source".to_string()],
                    clone_source: "source".to_string(),
                    clone_target: "target".to_string(),
                    clone_required_last_modified_epoch: Some(123),
                    clone_execution_results: Some(Struct::default()),
                    execution_runtime_ms: Some(456),
                    ..Default::default()
                },
            )),
        }
    }

    fn speculative_untracked_response() -> SubmitSqlSpeculativeResponse {
        SubmitSqlSpeculativeResponse {
            response: Some(
                submit_sql_speculative_response::Response::ReadyToExecuteUntracked(
                    ReadyToExecuteUntrackedResponse {},
                ),
            ),
        }
    }

    fn speculative_undecided_response() -> SubmitSqlSpeculativeResponse {
        SubmitSqlSpeculativeResponse {
            response: Some(submit_sql_speculative_response::Response::Undecided(
                UndecidedResponse {},
            )),
        }
    }

    struct RecordingRunCacheClient {
        submitted: Mutex<Vec<SubmitEnrichedSqlRequest>>,
        speculative_submitted: Mutex<Vec<SubmitEnrichedSqlRequest>>,
        response: SubmitSqlResponse,
        /// Canned speculative verdict. `None` makes the speculative RPC report
        /// `Disabled` (the trait default), mirroring a server that does not
        /// answer speculatively so the caller falls back to a regular submit.
        speculative_response: Option<SubmitSqlSpeculativeResponse>,
    }

    impl Default for RecordingRunCacheClient {
        fn default() -> Self {
            Self {
                submitted: Mutex::new(Vec::new()),
                speculative_submitted: Mutex::new(Vec::new()),
                response: ready_to_execute_response(),
                speculative_response: None,
            }
        }
    }

    impl RecordingRunCacheClient {
        fn with_response(response: SubmitSqlResponse) -> Self {
            Self {
                response,
                ..Self::default()
            }
        }

        fn with_speculative_response(speculative_response: SubmitSqlSpeculativeResponse) -> Self {
            Self {
                speculative_response: Some(speculative_response),
                ..Self::default()
            }
        }

        fn submitted_sql(&self) -> Vec<String> {
            self.submitted
                .lock()
                .unwrap()
                .iter()
                .map(|request| request.sql.clone())
                .collect()
        }

        fn submitted_execution_types(&self) -> Vec<i32> {
            self.submitted
                .lock()
                .unwrap()
                .iter()
                .map(|request| request.execution_type)
                .collect()
        }

        fn submitted_count(&self) -> usize {
            self.submitted.lock().unwrap().len()
        }

        fn speculative_submitted_count(&self) -> usize {
            self.speculative_submitted.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl RunCacheServiceClient for RecordingRunCacheClient {
        async fn validate_client_version(
            &self,
        ) -> Result<ClientVersionStatus, RunCacheServiceError> {
            Ok(ClientVersionStatus::Supported)
        }

        async fn submit_enriched_sql(
            &self,
            request: SubmitEnrichedSqlRequest,
        ) -> Result<SubmitSqlResponse, RunCacheServiceError> {
            self.submitted.lock().unwrap().push(request);
            Ok(self.response.clone())
        }

        async fn submit_enriched_sql_speculative(
            &self,
            request: SubmitEnrichedSqlRequest,
        ) -> Result<SubmitSqlSpeculativeResponse, RunCacheServiceError> {
            self.speculative_submitted.lock().unwrap().push(request);
            match &self.speculative_response {
                Some(response) => Ok(response.clone()),
                None => Err(RunCacheServiceError::Disabled),
            }
        }

        async fn submit_values(
            &self,
            _request: SubmitValuesRequest,
        ) -> Result<SubmitSqlResponse, RunCacheServiceError> {
            Err(RunCacheServiceError::Disabled)
        }

        async fn confirm_execution(
            &self,
            _request: ConfirmExecutionRequest,
        ) -> Result<ConfirmExecutionResponse, RunCacheServiceError> {
            Err(RunCacheServiceError::Disabled)
        }

        async fn record_executions(
            &self,
            _request: RecordExecutionsRequest,
        ) -> Result<RecordExecutionsResponse, RunCacheServiceError> {
            Err(RunCacheServiceError::Disabled)
        }
    }

    /// `RunCacheServiceError` isn't `Clone` (and can't be made so from this
    /// crate — the type is foreign), so `TelemetryTestClient` stores which
    /// error to manufacture rather than a reusable instance.
    #[derive(Clone, Copy)]
    enum TestFailure {
        Retriable,
        NonRetriable,
    }

    impl TestFailure {
        fn build(self) -> RunCacheServiceError {
            match self {
                Self::Retriable => RunCacheServiceError::Rpc(tonic::Status::unavailable("down")),
                Self::NonRetriable => {
                    RunCacheServiceError::Rpc(tonic::Status::invalid_argument("bad request"))
                }
            }
        }
    }

    /// Test double for `TelemetryDispatcher` tests: records every batch it's
    /// asked to submit, and fails the first `fail_count` calls with a
    /// configurable error before succeeding.
    #[derive(Default)]
    struct TelemetryTestClient {
        batches: Mutex<Vec<Vec<ClientTelemetryEvent>>>,
        fail_count: std::sync::atomic::AtomicUsize,
        failure: Option<TestFailure>,
    }

    impl TelemetryTestClient {
        fn failing(fail_count: usize, failure: TestFailure) -> Self {
            Self {
                batches: Mutex::new(Vec::new()),
                fail_count: std::sync::atomic::AtomicUsize::new(fail_count),
                failure: Some(failure),
            }
        }

        fn batches(&self) -> Vec<Vec<ClientTelemetryEvent>> {
            self.batches.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl RunCacheServiceClient for TelemetryTestClient {
        async fn validate_client_version(
            &self,
        ) -> Result<ClientVersionStatus, RunCacheServiceError> {
            Ok(ClientVersionStatus::Supported)
        }

        async fn submit_enriched_sql(
            &self,
            _request: SubmitEnrichedSqlRequest,
        ) -> Result<SubmitSqlResponse, RunCacheServiceError> {
            Err(RunCacheServiceError::Disabled)
        }

        async fn submit_values(
            &self,
            _request: SubmitValuesRequest,
        ) -> Result<SubmitSqlResponse, RunCacheServiceError> {
            Err(RunCacheServiceError::Disabled)
        }

        async fn confirm_execution(
            &self,
            _request: ConfirmExecutionRequest,
        ) -> Result<ConfirmExecutionResponse, RunCacheServiceError> {
            Err(RunCacheServiceError::Disabled)
        }

        async fn submit_telemetry_batch(
            &self,
            request: dbt_state::proto::query_cache::SubmitTelemetryBatchRequest,
        ) -> Result<dbt_state::proto::query_cache::SubmitTelemetryBatchResponse, RunCacheServiceError>
        {
            if self
                .fail_count
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    (count > 0).then(|| count - 1)
                })
                .is_ok()
            {
                let failure = self.failure.unwrap_or(TestFailure::Retriable);
                return Err(failure.build());
            }
            self.batches.lock().unwrap().push(request.events);
            Ok(dbt_state::proto::query_cache::SubmitTelemetryBatchResponse { success: true })
        }
    }

    fn test_telemetry_event(event_order: i64) -> ClientTelemetryEvent {
        session_start_event(Struct::default(), event_order)
    }

    #[test]
    fn enriched_sql_telemetry_counts_dependencies_without_existing_target() {
        let request = SubmitEnrichedSqlRequest {
            target_table: Some("db.schema.target".to_string()),
            tables: vec![TableModifiedInfo {
                name: "db.schema.dependency".to_string(),
                last_modified_epoch: Some(1),
            }],
            ..Default::default()
        };

        assert_eq!(
            enriched_sql_prepared_telemetry_input(&request).num_dependencies,
            Some(1)
        );
    }

    #[dbt_runtime::test]
    async fn flush_buffer_delivers_and_clears_buffer_on_success() {
        let client: dbt_state::service_client::SharedRunCacheServiceClient =
            Arc::new(TelemetryTestClient::default());
        let mut buffer: Vec<QueuedTelemetryEvent> =
            (0..3).map(|i| (test_telemetry_event(i), 0)).collect();

        TelemetryDispatcher::flush_buffer(&client, &mut buffer).await;

        assert!(buffer.is_empty(), "successful flush must drain the buffer");
    }

    #[dbt_runtime::test]
    async fn flush_buffer_caps_each_rpc_at_max_batch_size() {
        let client_arc = Arc::new(TelemetryTestClient::default());
        let client: dbt_state::service_client::SharedRunCacheServiceClient = client_arc.clone();
        let total = TELEMETRY_MAX_BATCH_SIZE * 2 + 20;
        let mut buffer: Vec<QueuedTelemetryEvent> = (0..total as i64)
            .map(|i| (test_telemetry_event(i), 0))
            .collect();

        TelemetryDispatcher::flush_buffer(&client, &mut buffer).await;

        assert!(buffer.is_empty());
        let batches = client_arc.batches();
        assert!(
            batches
                .iter()
                .all(|batch| batch.len() <= TELEMETRY_MAX_BATCH_SIZE),
            "no single SubmitTelemetryBatch RPC may exceed the batch cap: {batches:?}",
        );
        assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), total);
    }

    #[dbt_runtime::test]
    async fn flush_buffer_requeues_retriable_failures_with_incremented_retry_count() {
        let client: dbt_state::service_client::SharedRunCacheServiceClient = Arc::new(
            TelemetryTestClient::failing(usize::MAX, TestFailure::Retriable),
        );
        let mut buffer: Vec<QueuedTelemetryEvent> = vec![(test_telemetry_event(0), 0)];

        TelemetryDispatcher::flush_buffer(&client, &mut buffer).await;
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer[0].1, 1, "retriable failure increments retry_count");

        TelemetryDispatcher::flush_buffer(&client, &mut buffer).await;
        assert_eq!(buffer[0].1, 2);

        TelemetryDispatcher::flush_buffer(&client, &mut buffer).await;
        assert_eq!(buffer[0].1, 3);

        // One more attempt exceeds `TELEMETRY_MAX_RETRY_COUNT`: dropped, not requeued.
        TelemetryDispatcher::flush_buffer(&client, &mut buffer).await;
        assert!(
            buffer.is_empty(),
            "event exceeding max retries must be dropped"
        );
    }

    #[dbt_runtime::test]
    async fn flush_buffer_drops_batch_on_non_retriable_error() {
        let client: dbt_state::service_client::SharedRunCacheServiceClient = Arc::new(
            TelemetryTestClient::failing(usize::MAX, TestFailure::NonRetriable),
        );
        let mut buffer: Vec<QueuedTelemetryEvent> = vec![(test_telemetry_event(0), 0)];

        TelemetryDispatcher::flush_buffer(&client, &mut buffer).await;

        assert!(
            buffer.is_empty(),
            "a non-retriable error must drop the batch outright, not requeue it"
        );
    }

    #[dbt_runtime::test]
    async fn dispatcher_flush_terminates_instead_of_hanging() {
        // Regression test: `flush()` closes the dispatcher's `Sender` and
        // awaits the worker task, which only exits once
        // `mpsc::Receiver::recv` returns `None` — which requires every
        // `Sender` clone, including any the worker task itself might hold,
        // to be dropped. If the worker ever holds on to a `Sender` clone
        // (e.g. to requeue retries through the channel instead of the local
        // buffer), this hangs forever.
        let client = Arc::new(TelemetryTestClient::default());
        let dispatcher = TelemetryDispatcher::spawn(client.clone());
        dispatcher.send(test_telemetry_event(0)).await;
        dispatcher.send(test_telemetry_event(1)).await;

        tokio::time::timeout(std::time::Duration::from_secs(5), dispatcher.flush())
            .await
            .expect("flush() must terminate, not hang");

        assert_eq!(client.batches().iter().flatten().count(), 2);
    }

    #[derive(Debug)]
    struct TestExtendedCtx;

    impl ExtendedCtx for TestExtendedCtx {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }

        fn into_any(self: Box<Self>) -> Box<dyn Any> {
            self
        }

        fn on_test_failure(
            &self,
            _ctx: &TaskRunnerCtx,
            _node: &Arc<dyn crate::task::Task>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            Box::pin(async {})
        }

        fn is_sidecar(&self) -> bool {
            false
        }
    }

    struct TestCompiledSqlCache;

    impl crate::CompiledSqlCache for TestCompiledSqlCache {
        fn get_compiled_sql_path(
            &self,
            _io: &dbt_common::io_args::IoArgs,
            common: &dbt_schemas::schemas::CommonAttributes,
        ) -> PathBuf {
            Path::new("target").join(&common.unique_id)
        }

        fn try_get_compiled_sql(
            &self,
            _io: &dbt_common::io_args::IoArgs,
            _common: &dbt_schemas::schemas::CommonAttributes,
        ) -> Option<(String, Vec<MacroSpan>, Vec<ReclassifySpan>)> {
            None
        }

        fn set_compiled_sql(
            &self,
            _io: &dbt_common::io_args::IoArgs,
            _common: &dbt_schemas::schemas::CommonAttributes,
            _rendered_sql_maybe_with_cte: &str,
            _spans: &dyn CompiledSpans,
        ) -> FsResult<()> {
            Ok(())
        }

        fn clear(&self, _unique_id: &str) {}
    }

    struct TestAdhocRunner;

    impl crate::AdhocRunner for TestAdhocRunner {
        fn run_adhoc<'a>(
            self: Arc<Self>,
            _instruction: &'a dbt_scheduler::instructions::Instruction,
            _rendered_sql: &'a str,
            _unique_id: Option<&'a str>,
            _connection: &'a mut Option<Box<dyn dbt_adbc::Connection>>,
        ) -> Pin<Box<dyn Future<Output = FsResult<(Vec<RecordBatch>, SchemaRef)>> + Send + 'a>>
        {
            Box::pin(async { Ok((Vec::new(), Arc::new(Schema::empty()))) })
        }
    }

    struct EmptySourcesExtractor;

    impl SourcesExtractor for EmptySourcesExtractor {
        fn extract_upstreams(
            &self,
            _adapter_type: AdapterType,
            _sql: &str,
            _default_catalog: &str,
            _default_schema: &str,
            _quoted_name_ignore_case: bool,
        ) -> FrontendResult<Vec<NamedReference<FullyQualifiedName>>> {
            Ok(Vec::new())
        }

        fn extract_standalone_expression_upstreams(
            &self,
            _adapter_type: AdapterType,
            _sql: &str,
            _default_catalog: &str,
            _default_schema: &str,
            _quoted_name_ignore_case: bool,
        ) -> FrontendResult<Vec<NamedReference<FullyQualifiedName>>> {
            Ok(Vec::new())
        }
    }

    struct FailingSourcesExtractor;

    fn synthetic_extraction_error() -> Box<FsError> {
        fs_err!(ErrorCode::Generic, "synthetic extraction failure")
    }

    impl SourcesExtractor for FailingSourcesExtractor {
        fn extract_upstreams(
            &self,
            _adapter_type: AdapterType,
            _sql: &str,
            _default_catalog: &str,
            _default_schema: &str,
            _quoted_name_ignore_case: bool,
        ) -> FrontendResult<Vec<NamedReference<FullyQualifiedName>>> {
            Err(Box::new(synthetic_frontend_error()))
        }

        fn extract_standalone_expression_upstreams(
            &self,
            _adapter_type: AdapterType,
            _sql: &str,
            _default_catalog: &str,
            _default_schema: &str,
            _quoted_name_ignore_case: bool,
        ) -> FrontendResult<Vec<NamedReference<FullyQualifiedName>>> {
            Err(Box::new(synthetic_frontend_error()))
        }
    }

    struct ExpressionOnlySourcesExtractor;

    impl SourcesExtractor for ExpressionOnlySourcesExtractor {
        fn extract_upstreams(
            &self,
            _adapter_type: AdapterType,
            _sql: &str,
            _default_catalog: &str,
            _default_schema: &str,
            _quoted_name_ignore_case: bool,
        ) -> FrontendResult<Vec<NamedReference<FullyQualifiedName>>> {
            Err(Box::new(synthetic_frontend_error()))
        }

        fn extract_standalone_expression_upstreams(
            &self,
            _adapter_type: AdapterType,
            sql: &str,
            _default_catalog: &str,
            _default_schema: &str,
            _quoted_name_ignore_case: bool,
        ) -> FrontendResult<Vec<NamedReference<FullyQualifiedName>>> {
            if sql == "'alice'" {
                Ok(Vec::new())
            } else if sql == "(select max(id) from raw.users)" {
                Ok(vec![FullyQualifiedName::new("db", "raw", "users").into()])
            } else {
                Err(Box::new(synthetic_frontend_error()))
            }
        }
    }

    fn synthetic_frontend_error() -> FrontendError {
        FrontendError::new(
            FrontendErrorCode::Unexpected,
            CodeLocation::default(),
            "synthetic extraction failure",
        )
    }

    fn task_result_with_sql(sql: &str) -> TaskResult {
        TaskResult {
            sql_instruction: dbt_scheduler::instructions::SqlInstruction {
                sql: sql.to_string(),
                ..Default::default()
            },
            config_map: Arc::new(DashMap::default()),
            lp_instruction: None,
        }
    }

    fn test_task_runner_ctx(
        run_cache_service_client: Option<dbt_state::service_client::SharedRunCacheServiceClient>,
    ) -> TaskRunnerCtx {
        test_task_runner_ctx_with_mode(run_cache_service_client, RunCacheMode::ReadWrite)
    }

    fn test_task_runner_ctx_with_mode(
        run_cache_service_client: Option<dbt_state::service_client::SharedRunCacheServiceClient>,
        run_cache_mode: RunCacheMode,
    ) -> TaskRunnerCtx {
        test_task_runner_ctx_with_mode_and_full_refresh(
            run_cache_service_client,
            run_cache_mode,
            false,
        )
    }

    fn test_task_runner_ctx_with_mode_and_full_refresh(
        run_cache_service_client: Option<dbt_state::service_client::SharedRunCacheServiceClient>,
        run_cache_mode: RunCacheMode,
        full_refresh: bool,
    ) -> TaskRunnerCtx {
        test_task_runner_ctx_with_mode_and_sources_extractor(
            run_cache_service_client,
            run_cache_mode,
            full_refresh,
            Arc::new(EmptySourcesExtractor),
        )
    }

    fn test_task_runner_ctx_with_mode_and_sources_extractor(
        run_cache_service_client: Option<dbt_state::service_client::SharedRunCacheServiceClient>,
        run_cache_mode: RunCacheMode,
        full_refresh: bool,
        sources_extractor: Arc<dyn SourcesExtractor>,
    ) -> TaskRunnerCtx {
        test_task_runner_ctx_with_nodes(
            run_cache_service_client,
            run_cache_mode,
            full_refresh,
            sources_extractor,
            Nodes::default(),
            BTreeSet::new(),
        )
    }

    fn test_task_runner_ctx_with_nodes(
        run_cache_service_client: Option<dbt_state::service_client::SharedRunCacheServiceClient>,
        run_cache_mode: RunCacheMode,
        full_refresh: bool,
        sources_extractor: Arc<dyn SourcesExtractor>,
        nodes: Nodes,
        selected_nodes: BTreeSet<String>,
    ) -> TaskRunnerCtx {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .try_init();
        let span = tracing::error_span!("run_cache_service_test");
        let _guard = span.enter();

        let resolver_state = Arc::new(test_resolver_state_with_nodes(nodes));
        let args = RunTasksArgs {
            run_cache_mode,
            run_cache_service: true,
            full_refresh,
            ..Default::default()
        };
        let schedule = Schedule {
            selected_nodes,
            ..Default::default()
        };

        let inner = TaskRunnerCtxInner::new(
            Arc::new(args),
            "worker-0".to_string(),
            schedule,
            BTreeMap::new(),
            DashMap::default(),
            Box::new(TestExtendedCtx),
            Arc::new(TestCompiledSqlCache),
            Arc::new(TestAdhocRunner),
            &resolver_state,
            Default::default(),
            Arc::new(crate::span_manager::SpanManager::new_empty()),
            Execute::Remote,
            test_adapter_store(),
            sources_extractor,
            RunCacheCtx {
                run_cache_metadata: Arc::new(RunCacheMetadataCache::new()),
                run_cache_dev_cloned_nodes: DashMap::default(),
                run_cache_deferred_fqns: BTreeSet::new(),
                run_cache_service_requested: true,
                run_cache_service_config: Some(RunCacheServiceConfig::disabled()),
                run_cache_service_client,
                state_explain_log_path: None,
                view_traverser: None,
                heuristic_clock: std::sync::OnceLock::new(),
                prefetch: Default::default(),
                telemetry_event_order: std::sync::atomic::AtomicI64::new(0),
                telemetry_session_start: std::sync::OnceLock::new(),
                telemetry_session_ended: std::sync::atomic::AtomicBool::new(false),
                telemetry_dispatcher: std::sync::OnceLock::new(),
            },
        );

        TaskRunnerCtx {
            inner: Arc::new(inner),
            env: Arc::new(JinjaEnv::new(minijinja::Environment::new())),
            schema_cache: Arc::new(MockSchemaStore::new()),
            data_store: Arc::new(MockDataStore::new()),
            resolver_state,
            rendering_listener_factory: Arc::new(DefaultRenderingEventListenerFactory::default()),
            thread_id: 0,
        }
    }

    fn test_adapter_store() -> Arc<AdapterStore> {
        let build: AdapterBuilder = Box::new(|adapter_type| {
            let adapter = AdapterImpl::new_mock(
                adapter_type,
                BTreeMap::new(),
                DEFAULT_RESOLVED_QUOTING,
                Arc::new(DefaultTypeOps::new(adapter_type)),
                Arc::new(DefaultStmtSplitter),
            );
            Ok(Arc::new(Adapter::new(
                Arc::new(adapter),
                None,
                never_cancels(),
            )))
        });
        Arc::new(
            AdapterStore::new(vec![AdapterType::Snowflake], AdapterType::Snowflake, build)
                .expect("valid store"),
        )
    }

    fn clone_target(relation_type: Option<RelationType>) -> Arc<dyn BaseRelation> {
        create_relation(
            AdapterType::Snowflake,
            "database".to_string(),
            "schema".to_string(),
            Some("target".to_string()),
            relation_type,
            DEFAULT_RESOLVED_QUOTING,
        )
        .expect("valid relation")
        .into()
    }

    fn cached_clone_target_relation(
        ctx: &TaskRunnerCtx,
        adapter: &Adapter,
        relation_type: RelationType,
    ) -> Arc<dyn BaseRelation> {
        let relation = clone_target(Some(relation_type));
        let jinja_state = ctx
            .env
            .new_state_with_context(ctx.inner.base_context.clone());
        adapter
            .cache_added(&jinja_state, Arc::clone(&relation))
            .expect("cache relation");
        relation
    }

    #[test]
    fn drop_clone_target_before_clone_keeps_existing_table() {
        let ctx = test_task_runner_ctx(None);
        let adapter = ctx
            .adapter_store()
            .get(AdapterType::Snowflake)
            .expect("mock adapter");
        let target = cached_clone_target_relation(&ctx, &adapter, RelationType::Table);
        let cached = adapter
            .engine()
            .relation_cache()
            .get_relation(target.as_ref())
            .expect("table is cached before clone");
        assert_eq!(cached.relation().relation_type(), Some(RelationType::Table));

        drop_clone_target_before_clone(&ctx, Arc::clone(&target));

        let existing = adapter
            .engine()
            .relation_cache()
            .get_relation(target.as_ref())
            .expect("cached table remains");
        assert_eq!(
            existing.relation().relation_type(),
            Some(RelationType::Table)
        );
    }

    #[test]
    fn drop_clone_target_before_clone_drops_existing_view() {
        let ctx = test_task_runner_ctx(None);
        let adapter = ctx
            .adapter_store()
            .get(AdapterType::Snowflake)
            .expect("mock adapter");
        let target = cached_clone_target_relation(&ctx, &adapter, RelationType::View);
        let cached = adapter
            .engine()
            .relation_cache()
            .get_relation(target.as_ref())
            .expect("view is cached before clone");
        assert_eq!(cached.relation().relation_type(), Some(RelationType::View));

        drop_clone_target_before_clone(&ctx, Arc::clone(&target));

        assert!(
            adapter
                .engine()
                .relation_cache()
                .get_relation(target.as_ref())
                .is_none(),
            "existing view is removed from the relation cache"
        );
    }

    #[dbt_runtime::worker_test]
    fn drop_clone_target_before_clone_ignores_missing_target() {
        let ctx = test_task_runner_ctx(None);
        let adapter = ctx
            .adapter_store()
            .get(AdapterType::Snowflake)
            .expect("mock adapter");
        let target = clone_target(None);

        drop_clone_target_before_clone(&ctx, Arc::clone(&target));

        assert!(
            adapter
                .engine()
                .relation_cache()
                .get_relation(target.as_ref())
                .is_none()
        );
    }

    fn test_resolver_state_with_nodes(nodes: Nodes) -> ResolverState {
        // Register a root-project `custom_table` materialization so the
        // materialization resolver classifies models materialized as
        // `custom_table` as custom (root/imported macro shadowing), matching
        // what the custom-materialization tests below exercise.
        let mut macros = Macros::default();
        let custom_table_mat = DbtMacro {
            name: "materialization_custom_table_default".to_string(),
            package_name: "test".to_string(),
            unique_id: "macro.test.materialization_custom_table_default".to_string(),
            ..Default::default()
        };
        macros
            .macros
            .insert(custom_table_mat.unique_id.clone(), custom_table_mat);

        let user_defined_schema_registry = dbt_schemas::state::hydrate_user_defined_schema_registry(
            &nodes,
            AdapterType::Snowflake,
        );

        ResolverState {
            root_project_name: "test".to_string(),
            adapter_type: AdapterType::Snowflake,
            nodes,
            disabled_nodes: Nodes::default(),
            macros,
            operations: Operations::default(),
            dbt_profile: {
                let db_config = DbConfig::Snowflake(Box::<SnowflakeDbConfig>::default());
                let default_adapter = db_config.adapter_type();
                let adapters =
                    IndexMap::from([(default_adapter, ProfileAdapter::single(db_config))]);
                DbtProfile {
                    profile: "default".to_string(),
                    target: "dev".to_string(),
                    defer_to_target: None,
                    allow_clones: true,
                    adapters,
                    default_adapter,
                    schema: "dbt_test".to_string(),
                    database: "db".to_string(),
                    relative_profile_path: PathBuf::new(),
                    threads: None,
                }
            },
            cloud_config: None,
            render_results: RenderResults::default(),
            node_resolver: Arc::new(DummyNodeResolverTracker),
            get_relation_calls: Default::default(),
            get_columns_in_relation_calls: Default::default(),
            patterned_dangling_sources: Default::default(),
            run_started_at: Utc::now().with_timezone(&chrono_tz::UTC),
            runtime_config: Arc::new(DbtRuntimeConfig::default()),
            manifest_path_configs: BTreeMap::new(),
            manifest_selectors: BTreeMap::new(),
            resolved_selectors: Default::default(),
            root_project_quoting: ResolvedQuoting::default(),
            defer_nodes: None,
            nodes_with_resolution_errors: Default::default(),
            nodes_with_access_errors: Default::default(),
            semantic_layer_spec_is_legacy: false,
            test_name_truncations: Default::default(),
            user_defined_schema_registry,
        }
    }

    // ── collect_global_prefetch_relations tests ──────────────────────────────

    fn make_model(
        unique_id: &str,
        db: &str,
        schema: &str,
        alias: &str,
        mat: DbtMaterialization,
    ) -> Arc<DbtModel> {
        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = unique_id.to_string();
        model.__common_attr__.language = Some("sql".to_string());
        model.__base_attr__.database = db.to_string();
        model.__base_attr__.schema = schema.to_string();
        model.__base_attr__.alias = alias.to_string();
        model.__base_attr__.materialized = mat;
        Arc::new(model)
    }

    fn make_source(unique_id: &str, db: &str, schema: &str, alias: &str) -> Arc<DbtSource> {
        let mut source = DbtSource::default();
        source.__common_attr__.unique_id = unique_id.to_string();
        source.__base_attr__.database = db.to_string();
        source.__base_attr__.schema = schema.to_string();
        source.__base_attr__.alias = alias.to_string();
        source.__base_attr__.materialized = DbtMaterialization::View;
        Arc::new(source)
    }

    fn nodes_from(models: Vec<Arc<DbtModel>>, sources: Vec<Arc<DbtSource>>) -> Nodes {
        let mut nodes = Nodes::default();
        for m in models {
            nodes.models.insert(m.__common_attr__.unique_id.clone(), m);
        }
        for s in sources {
            nodes.sources.insert(s.__common_attr__.unique_id.clone(), s);
        }
        nodes
    }

    fn fqn_of(db: &str, schema: &str, alias: &str) -> String {
        create_relation(
            AdapterType::Snowflake,
            db.to_string(),
            schema.to_string(),
            Some(alias.to_string()),
            None,
            ResolvedQuoting::default(),
        )
        .unwrap()
        .semantic_fqn()
    }

    #[test]
    fn bigquery_submit_time_misses_plan_schema_prefetch_except_overrides() {
        let bulk_relation: Arc<dyn BaseRelation> = create_relation(
            AdapterType::Bigquery,
            "db".to_string(),
            "analytics".to_string(),
            Some("orders".to_string()),
            None,
            ResolvedQuoting::default(),
        )
        .unwrap()
        .into();
        let override_relation: Arc<dyn BaseRelation> = create_relation(
            AdapterType::Bigquery,
            "db".to_string(),
            "analytics".to_string(),
            Some("source_events".to_string()),
            None,
            ResolvedQuoting::default(),
        )
        .unwrap()
        .into();
        let bulk_name = bulk_relation.semantic_fqn();
        let override_name = override_relation.semantic_fqn();
        let relations = BTreeMap::from([
            (bulk_name.clone(), bulk_relation),
            (override_name.clone(), override_relation),
        ]);
        let overrides = BTreeMap::from([(
            override_name.clone(),
            FreshnessOverride::Field("loaded_at".to_string()),
        )]);

        let bigquery_plan =
            plan_last_modified_miss_refresh(AdapterType::Bigquery, &relations, &overrides);
        assert_eq!(bigquery_plan.schema_prefetch_relations.len(), 1);
        assert!(
            bigquery_plan
                .schema_prefetch_relations
                .contains_key(&bulk_name)
        );
        assert_eq!(bigquery_plan.targeted_relations.len(), 1);
        assert!(
            bigquery_plan
                .targeted_relations
                .contains_key(&override_name)
        );

        let snowflake_plan =
            plan_last_modified_miss_refresh(AdapterType::Snowflake, &relations, &overrides);
        assert!(snowflake_plan.schema_prefetch_relations.is_empty());
        assert_eq!(snowflake_plan.targeted_relations.len(), 2);
        assert!(snowflake_plan.targeted_relations.contains_key(&bulk_name));
        assert!(
            snowflake_plan
                .targeted_relations
                .contains_key(&override_name)
        );
    }

    #[test]
    fn bigquery_submit_time_plan_skips_projectless_relations() {
        let relation: Arc<dyn BaseRelation> = create_relation(
            AdapterType::Bigquery,
            String::new(),
            "analytics".to_string(),
            Some("orders".to_string()),
            None,
            ResolvedQuoting::default(),
        )
        .unwrap()
        .into();
        let name = relation.semantic_fqn();
        let plan = plan_last_modified_miss_refresh(
            AdapterType::Bigquery,
            &BTreeMap::from([(name, relation)]),
            &BTreeMap::new(),
        );

        assert!(plan.targeted_relations.is_empty());
        assert!(plan.schema_prefetch_relations.is_empty());
    }

    #[test]
    fn final_refresh_uses_bigquery_schema_prefetch_plan() {
        let relation: Arc<dyn BaseRelation> = create_relation(
            AdapterType::Bigquery,
            "db".to_string(),
            "analytics".to_string(),
            Some("orders".to_string()),
            None,
            ResolvedQuoting::default(),
        )
        .unwrap()
        .into();
        let name = relation.semantic_fqn();
        let relations = BTreeMap::from([(name.clone(), relation)]);
        let overrides = BTreeMap::new();

        let bigquery_plan =
            plan_last_modified_miss_refresh(AdapterType::Bigquery, &relations, &overrides);
        assert!(bigquery_plan.targeted_relations.is_empty());
        assert!(bigquery_plan.schema_prefetch_relations.contains_key(&name));

        let snowflake_plan =
            plan_last_modified_miss_refresh(AdapterType::Snowflake, &relations, &overrides);
        assert!(snowflake_plan.schema_prefetch_relations.is_empty());
        assert!(snowflake_plan.targeted_relations.contains_key(&name));
    }

    #[test]
    fn prefetch_skips_projectless_bigquery_relations() {
        let model = make_model(
            "model.pkg.orders",
            "",
            "analytics",
            "orders",
            DbtMaterialization::Table,
        );
        let nodes = nodes_from(vec![model], vec![]);
        let relations = collect_global_prefetch_relations(
            AdapterType::Bigquery,
            &BTreeSet::from(["model.pkg.orders".to_string()]),
            &BTreeMap::new(),
            &nodes,
        )
        .0;

        assert!(relations.is_empty());
    }

    #[test]
    fn prefetch_keeps_cross_project_bigquery_relations_distinct() {
        let model = make_model(
            "model.pkg.orders",
            "project_a",
            "analytics",
            "orders",
            DbtMaterialization::Table,
        );
        let source = make_source("source.pkg.raw.events", "project_b", "raw", "events");
        let runnable_set = BTreeSet::from(["model.pkg.orders".to_string()]);
        let runtime_deps = BTreeMap::from([(
            "model.pkg.orders".to_string(),
            BTreeSet::from(["source.pkg.raw.events".to_string()]),
        )]);
        let nodes = nodes_from(vec![model], vec![source]);

        let (relations, _) = collect_global_prefetch_relations(
            AdapterType::Bigquery,
            &runnable_set,
            &runtime_deps,
            &nodes,
        );

        assert_eq!(relations.len(), 2);
        assert!(relations.keys().any(|fqn| fqn.contains("project_a")));
        assert!(relations.keys().any(|fqn| fqn.contains("project_b")));
    }

    #[test]
    fn prefetch_includes_selected_node_and_source_dep() {
        let model = make_model(
            "model.pkg.orders",
            "db",
            "analytics",
            "orders",
            DbtMaterialization::Table,
        );
        let source = make_source("source.pkg.raw.events", "db", "raw", "events");

        let runnable_set: BTreeSet<String> = ["model.pkg.orders".to_string()].into_iter().collect();
        let runtime_deps: BTreeMap<String, BTreeSet<String>> = [(
            "model.pkg.orders".to_string(),
            ["source.pkg.raw.events".to_string()].into_iter().collect(),
        )]
        .into_iter()
        .collect();
        let nodes = nodes_from(vec![model], vec![source]);

        let (relations, overrides) = collect_global_prefetch_relations(
            AdapterType::Snowflake,
            &runnable_set,
            &runtime_deps,
            &nodes,
        );

        assert!(
            relations.contains_key(&fqn_of("db", "analytics", "orders")),
            "selected model target should be included"
        );
        assert!(
            relations.contains_key(&fqn_of("db", "raw", "events")),
            "source dep should be included"
        );
        assert!(overrides.is_empty());
    }

    fn make_exposure(unique_id: &str) -> Arc<dbt_schemas::schemas::nodes::DbtExposure> {
        let mut exposure = dbt_schemas::schemas::nodes::DbtExposure::default();
        exposure.__common_attr__.unique_id = unique_id.to_string();
        Arc::new(exposure)
    }

    #[test]
    fn prefetch_keeps_real_relations_alongside_an_exposure() {
        let model = make_model(
            "model.pkg.orders",
            "db",
            "analytics",
            "orders",
            DbtMaterialization::Table,
        );
        let exposure = make_exposure("exposure.pkg.dashboard");
        let mut nodes = nodes_from(vec![model], vec![]);
        nodes
            .exposures
            .insert(exposure.__common_attr__.unique_id.clone(), exposure);

        let runnable_set: BTreeSet<String> = [
            "model.pkg.orders".to_string(),
            "exposure.pkg.dashboard".to_string(),
        ]
        .into_iter()
        .collect();

        let (relations, _) = collect_global_prefetch_relations(
            AdapterType::Snowflake,
            &runnable_set,
            &BTreeMap::new(),
            &nodes,
        );

        assert_eq!(
            relations.keys().collect::<Vec<_>>(),
            vec![&fqn_of("db", "analytics", "orders")],
            "the exposure must be dropped without disturbing real relations"
        );
    }

    #[test]
    fn non_empty_schema_check_ignores_database() {
        // BigQuery renders relations without a database; only a missing schema
        // makes the metadata query unbuildable.
        let no_database = create_relation(
            AdapterType::Bigquery,
            String::new(),
            "analytics".to_string(),
            Some("orders".to_string()),
            None,
            ResolvedQuoting::default(),
        )
        .unwrap();
        assert!(has_non_empty_schema(no_database.as_ref()));
        assert!(!has_metadata_address(
            AdapterType::Bigquery,
            no_database.as_ref()
        ));

        let no_schema = create_relation(
            AdapterType::Snowflake,
            "db".to_string(),
            String::new(),
            Some("orders".to_string()),
            None,
            ResolvedQuoting::default(),
        )
        .unwrap();
        assert!(!has_non_empty_schema(no_schema.as_ref()));
        assert!(!has_metadata_address(
            AdapterType::Snowflake,
            no_schema.as_ref()
        ));
    }

    #[test]
    fn prefetch_skips_ephemeral_nodes() {
        let ephemeral = make_model(
            "model.pkg.eph",
            "db",
            "analytics",
            "eph",
            DbtMaterialization::Ephemeral,
        );
        let runnable_set: BTreeSet<String> = ["model.pkg.eph".to_string()].into_iter().collect();
        let runtime_deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let nodes = nodes_from(vec![ephemeral], vec![]);

        let (relations, _) = collect_global_prefetch_relations(
            AdapterType::Snowflake,
            &runnable_set,
            &runtime_deps,
            &nodes,
        );

        assert!(relations.is_empty(), "ephemeral nodes should be excluded");
    }

    #[test]
    fn prefetch_includes_source_freshness_overrides() {
        let model = make_model(
            "model.pkg.fact",
            "db",
            "analytics",
            "fact",
            DbtMaterialization::Incremental,
        );
        let mut source = DbtSource::default();
        source.__common_attr__.unique_id = "source.pkg.raw.users".to_string();
        source.__base_attr__.database = "db".to_string();
        source.__base_attr__.schema = "raw".to_string();
        source.__base_attr__.alias = "users".to_string();
        source.__base_attr__.materialized = DbtMaterialization::View;
        source.__source_attr__.loaded_at_field = Some("updated_at".to_string());
        let source = Arc::new(source);

        let runnable_set: BTreeSet<String> = ["model.pkg.fact".to_string()].into_iter().collect();
        let runtime_deps: BTreeMap<String, BTreeSet<String>> = [(
            "model.pkg.fact".to_string(),
            ["source.pkg.raw.users".to_string()].into_iter().collect(),
        )]
        .into_iter()
        .collect();
        let nodes = nodes_from(vec![model], vec![source]);

        let (_, overrides) = collect_global_prefetch_relations(
            AdapterType::Snowflake,
            &runnable_set,
            &runtime_deps,
            &nodes,
        );

        let source_fqn = fqn_of("db", "raw", "users");
        assert!(
            overrides.contains_key(&source_fqn),
            "source with loaded_at_field should produce an override"
        );
        assert!(
            matches!(overrides[&source_fqn], FreshnessOverride::Field(_)),
            "override kind should be Field"
        );
    }

    // Regression #15499: a `loaded_at_query` Jinja macro / `{{ this }}` must be
    // rendered before it reaches the warehouse, else the raw `{{ ... }}` errors.
    #[test]
    fn render_freshness_override_renders_loaded_at_query_macro() {
        let relation: Arc<dyn BaseRelation> = create_relation(
            AdapterType::Snowflake,
            "db".to_string(),
            "raw".to_string(),
            Some("customers".to_string()),
            None,
            ResolvedQuoting::default(),
        )
        .unwrap()
        .into();

        let mut mj = minijinja::Environment::new();
        mj.add_function(
            "get_from_watermark_table",
            || "select max(updated_at) from db.dbt_jyeo.watermark",
        );
        let jinja_env = JinjaEnv::new(mj);
        let base_context = BTreeMap::new();

        // Macro call is expanded, not sent raw.
        let rendered = render_freshness_override(
            FreshnessOverride::Query("{{ get_from_watermark_table() }}".to_string()),
            &relation,
            &jinja_env,
            &base_context,
        )
        .unwrap();
        assert!(
            matches!(rendered, FreshnessOverride::Query(q) if q == "select max(updated_at) from db.dbt_jyeo.watermark"),
            "loaded_at_query macro should be rendered"
        );

        // `{{ this }}` resolves to the source relation.
        let rendered = render_freshness_override(
            FreshnessOverride::Query("select max(ts) from {{ this }}".to_string()),
            &relation,
            &jinja_env,
            &base_context,
        )
        .unwrap();
        let expected = format!("select max(ts) from {}", relation.render_self_as_str());
        assert!(
            matches!(rendered, FreshnessOverride::Query(q) if q == expected),
            "{{{{ this }}}} should resolve to the relation"
        );

        // Field overrides are passed through untouched.
        let rendered = render_freshness_override(
            FreshnessOverride::Field("updated_at".to_string()),
            &relation,
            &jinja_env,
            &base_context,
        )
        .unwrap();
        assert!(matches!(rendered, FreshnessOverride::Field(f) if f == "updated_at"));

        // A relation missing a required component (here, identifier) errors
        // rather than rendering an unqualified name into the warehouse SQL.
        let no_identifier: Arc<dyn BaseRelation> = create_relation(
            AdapterType::Snowflake,
            "db".to_string(),
            "raw".to_string(),
            None,
            None,
            ResolvedQuoting::default(),
        )
        .unwrap()
        .into();
        assert!(
            render_freshness_override(
                FreshnessOverride::Query("select 1".to_string()),
                &no_identifier,
                &jinja_env,
                &base_context,
            )
            .is_err(),
            "missing identifier should error, not render an unqualified name"
        );
    }

    #[test]
    fn prefetch_deduplicates_shared_deps() {
        let model_a = make_model(
            "model.pkg.a",
            "db",
            "analytics",
            "a",
            DbtMaterialization::Table,
        );
        let model_b = make_model(
            "model.pkg.b",
            "db",
            "analytics",
            "b",
            DbtMaterialization::Table,
        );
        let shared_source = make_source("source.pkg.raw.shared", "db", "raw", "shared");

        let runnable_set: BTreeSet<String> = ["model.pkg.a".to_string(), "model.pkg.b".to_string()]
            .into_iter()
            .collect();
        let runtime_deps: BTreeMap<String, BTreeSet<String>> = [
            (
                "model.pkg.a".to_string(),
                ["source.pkg.raw.shared".to_string()].into_iter().collect(),
            ),
            (
                "model.pkg.b".to_string(),
                ["source.pkg.raw.shared".to_string()].into_iter().collect(),
            ),
        ]
        .into_iter()
        .collect();
        let nodes = nodes_from(vec![model_a, model_b], vec![shared_source]);

        let (relations, _) = collect_global_prefetch_relations(
            AdapterType::Snowflake,
            &runnable_set,
            &runtime_deps,
            &nodes,
        );

        // 2 selected models + 1 shared source = 3 unique entries
        assert_eq!(relations.len(), 3);
        assert!(relations.contains_key(&fqn_of("db", "raw", "shared")));
    }

    // ── HeuristicClock tests ─────────────────────────────────────────────────

    #[test]
    fn heuristic_clock_now_ms_equals_start_when_no_time_elapsed() {
        // Immediately after construction elapsed is ~0 ms, so now_ms should
        // equal start_ts_ms + HEURISTIC_CLOCK_SKEW_BUFFER_MS.
        let start_ts_ms: i64 = 1_700_000_000_000;
        let clock = HeuristicClock {
            start_ts_ms,
            start_instant: Instant::now(),
        };
        assert_eq!(clock.now_ms(), start_ts_ms);
    }

    #[test]
    fn heuristic_clock_now_ms_advances_monotonically() {
        let start_ts_ms: i64 = 1_700_000_000_000;
        let clock = HeuristicClock {
            start_ts_ms,
            start_instant: Instant::now(),
        };
        let first = clock.now_ms();
        // Spin briefly so at least one millisecond passes.
        let deadline = Instant::now() + std::time::Duration::from_millis(5);
        while Instant::now() < deadline {}
        let second = clock.now_ms();
        assert!(
            second >= first,
            "now_ms should be non-decreasing: first={first}, second={second}"
        );
    }

    #[test]
    fn heuristic_clock_now_ms_reflects_start_offset() {
        // Build a clock with a known start, then manually verify the arithmetic
        // by constructing a second clock with a start 1000 ms later and
        // checking that the difference between their now_ms values is ~1000.
        let base: i64 = 1_700_000_000_000;
        let clock_a = HeuristicClock {
            start_ts_ms: base,
            start_instant: Instant::now(),
        };
        let clock_b = HeuristicClock {
            start_ts_ms: base + 1_000,
            start_instant: Instant::now(),
        };
        // Both clocks were created at approximately the same real time, so
        // their elapsed values are nearly equal. The difference in now_ms
        // should therefore be close to 1000 ms.
        let diff = clock_b.now_ms() - clock_a.now_ms();
        assert!(
            (990..=1010).contains(&diff),
            "expected diff ~1000ms, got {diff}"
        );
    }

    #[test]
    fn heuristic_clock_once_lock_set_and_get() {
        let lock: std::sync::OnceLock<HeuristicClock> = std::sync::OnceLock::new();
        assert!(lock.get().is_none(), "lock should be empty before set");
        lock.set(HeuristicClock {
            start_ts_ms: 42_000,
            start_instant: Instant::now(),
        })
        .unwrap();
        let clock = lock.get().expect("clock should be set");
        assert_eq!(clock.now_ms(), 42_000);
    }

    #[test]
    fn unresolvable_last_modified_uses_heuristic_clock() {
        let cache = RunCacheMetadataCache::new();
        let fqn = r#""DB"."S"."SECURE_VIEW""#.to_string();
        cache.insert_last_modified_epoch(&fqn, Some(123));
        let clock = HeuristicClock {
            start_ts_ms: 1_700_000_000_000,
            start_instant: Instant::now(),
        };

        apply_unresolvable_last_modified_overrides(
            &cache,
            Some(&clock),
            &BTreeSet::from([fqn.clone()]),
            &BTreeMap::new(),
        );

        assert_eq!(
            cache.last_modified_epoch(&fqn).flatten(),
            Some(1_700_000_000_000)
        );
    }

    #[test]
    fn unresolvable_last_modified_does_not_replace_source_freshness_override() {
        let cache = RunCacheMetadataCache::new();
        let fqn = r#""DB"."S"."SECURE_VIEW""#.to_string();
        cache.insert_last_modified_epoch(&fqn, Some(123));
        let clock = HeuristicClock {
            start_ts_ms: 1_700_000_000_000,
            start_instant: Instant::now(),
        };
        let overrides = BTreeMap::from([(
            fqn.clone(),
            FreshnessOverride::Field("loaded_at".to_string()),
        )]);

        apply_unresolvable_last_modified_overrides(
            &cache,
            Some(&clock),
            &BTreeSet::from([fqn.clone()]),
            &overrides,
        );

        assert_eq!(cache.last_modified_epoch(&fqn), None);
    }

    #[test]
    fn unresolvable_last_modified_leaves_cache_empty_when_clock_missing() {
        let cache = RunCacheMetadataCache::new();
        let fqn = r#""DB"."S"."SECURE_VIEW""#.to_string();
        cache.insert_last_modified_epoch(&fqn, Some(123));

        apply_unresolvable_last_modified_overrides(
            &cache,
            None,
            &BTreeSet::from([fqn.clone()]),
            &BTreeMap::new(),
        );

        assert_eq!(cache.last_modified_epoch(&fqn), None);
    }

    #[test]
    fn execution_decision_id_from_response_extracts_service_ids() {
        let mut ready = ready_to_execute_response();
        let Some(submit_sql_response::Response::ReadyToExecute(response)) = ready.response.as_mut()
        else {
            panic!("expected ready response");
        };
        response.execution_decision_id = Some("ready-id".to_string());

        let mut skip = skip_execution_response();
        let Some(submit_sql_response::Response::SkipExecution(response)) = skip.response.as_mut()
        else {
            panic!("expected skip response");
        };
        response.execution_decision_id = Some("skip-id".to_string());

        let mut clone = ready_to_clone_response();
        let Some(submit_sql_response::Response::ReadyToClone(response)) = clone.response.as_mut()
        else {
            panic!("expected clone response");
        };
        response.execution_decision_id = Some("clone-id".to_string());

        assert_eq!(
            execution_decision_id_from_response(&ready).as_deref(),
            Some("ready-id")
        );
        assert_eq!(
            execution_decision_id_from_response(&skip).as_deref(),
            Some("skip-id")
        );
        assert_eq!(
            execution_decision_id_from_response(&clone).as_deref(),
            Some("clone-id")
        );
        assert!(execution_decision_id_from_response(&empty_response()).is_none());
    }

    #[test]
    fn state_explain_execution_decision_id_allows_missing_service_response() {
        let mut ready = ready_to_execute_response();
        let Some(submit_sql_response::Response::ReadyToExecute(response)) = ready.response.as_mut()
        else {
            panic!("expected ready response");
        };
        response.execution_decision_id = Some("ready-id".to_string());
        let ready_decision = record_service_decision("model.test.orders", &ready, 0, true);

        assert_eq!(
            state_explain_execution_decision_id(Some(&ready), &ready_decision).as_deref(),
            Some("ready-id")
        );
        assert!(state_explain_execution_decision_id(None, &ready_decision).is_none());
    }

    #[test]
    fn state_explain_execution_decision_id_ignores_data_test_skip_fallback() {
        let mut response = skip_execution_response();
        let Some(submit_sql_response::Response::SkipExecution(skip)) = response.response.as_mut()
        else {
            panic!("expected skip response");
        };
        skip.execution_decision_id = Some("skip-id".to_string());

        let decision = record_service_decision(
            "test.test.not_null_orders_order_date.abc123",
            &response,
            0,
            true,
        );

        assert_eq!(
            decision,
            RunCacheServiceDecision::Execute {
                after_success: RunCacheAfterSuccess::None,
                sao_guard: None,
            }
        );
        assert!(state_explain_execution_decision_id(Some(&response), &decision).is_none());
    }

    #[test]
    fn state_explain_execution_decision_id_keeps_honored_skip() {
        let mut response = skip_execution_response_with_test_result(CachedTestExecutionResult {
            failures: 0,
            should_warn: false,
            should_error: false,
        });
        let Some(submit_sql_response::Response::SkipExecution(skip)) = response.response.as_mut()
        else {
            panic!("expected skip response");
        };
        skip.execution_decision_id = Some("skip-id".to_string());

        let decision = record_service_decision(
            "test.test.not_null_orders_order_date.abc123",
            &response,
            0,
            true,
        );

        assert_eq!(
            state_explain_execution_decision_id(Some(&response), &decision).as_deref(),
            Some("skip-id")
        );
    }

    #[test]
    fn clone_chain_depth_limit_for_adapter_returns_n_minus_1_for_prod() {
        assert_eq!(
            clone_chain_depth_limit_for_adapter(AdapterType::Databricks, true, true),
            Some(0)
        );
        assert_eq!(
            clone_chain_depth_limit_for_adapter(AdapterType::Bigquery, true, true),
            Some(2)
        );
    }

    #[test]
    fn clone_chain_depth_limit_for_adapter_returns_default_for_non_prod() {
        assert_eq!(
            clone_chain_depth_limit_for_adapter(AdapterType::Databricks, false, true),
            Some(1)
        );
        assert_eq!(
            clone_chain_depth_limit_for_adapter(AdapterType::Bigquery, false, true),
            Some(3)
        );
    }

    #[test]
    fn clone_chain_depth_limit_for_adapter_none_for_adapters_with_no_limits() {
        assert_eq!(
            clone_chain_depth_limit_for_adapter(AdapterType::Snowflake, true, true),
            None
        );
        assert_eq!(
            clone_chain_depth_limit_for_adapter(AdapterType::Snowflake, false, true),
            None
        );
    }

    #[test]
    fn clone_chain_depth_limit_for_adapter_zero_when_clones_disallowed() {
        // allow_clones=false overrides to 0 regardless of adapter or prod/dev direction,
        // including adapters that otherwise have no default limit at all.
        assert_eq!(
            clone_chain_depth_limit_for_adapter(AdapterType::Databricks, true, false),
            Some(0)
        );
        assert_eq!(
            clone_chain_depth_limit_for_adapter(AdapterType::Bigquery, false, false),
            Some(0)
        );
        assert_eq!(
            clone_chain_depth_limit_for_adapter(AdapterType::Snowflake, true, false),
            Some(0)
        );
        assert_eq!(
            clone_chain_depth_limit_for_adapter(AdapterType::Snowflake, false, false),
            Some(0)
        );
    }

    fn test_task_runner_ctx_with_allow_clones(allow_clones: bool) -> TaskRunnerCtx {
        let mut ctx = test_task_runner_ctx(None);
        Arc::get_mut(&mut ctx.inner)
            .expect("sole owner of inner right after construction")
            .dbt_profile = Arc::new(DbtProfile {
            allow_clones,
            ..ctx.dbt_profile().clone()
        });
        ctx
    }

    /// Seed body hashing reads the CSV off disk relative to the project root,
    /// so the write-only record can't be built without pointing the ctx at one.
    fn set_project_root(ctx: &mut TaskRunnerCtx, project_root: &Path) {
        let inner =
            Arc::get_mut(&mut ctx.inner).expect("sole owner of inner right after construction");
        Arc::get_mut(&mut inner.arg)
            .expect("sole owner of args right after construction")
            .io
            .in_dir = project_root.to_path_buf();
    }

    fn test_seed_on_adapter(adapter: AdapterType) -> DbtSeed {
        let mut seed = DbtSeed::default();
        seed.__base_attr__.adapter = adapter;
        seed
    }

    fn seed_with_csv(project_root: &Path) -> DbtSeed {
        std::fs::create_dir_all(project_root.join("seeds")).unwrap();
        std::fs::write(
            project_root.join("seeds").join("cities.csv"),
            "id,name\n1,Philadelphia\n",
        )
        .unwrap();

        let mut seed = test_seed_on_adapter(AdapterType::Snowflake);
        seed.__common_attr__.unique_id = "seed.test.cities".to_string();
        seed.__common_attr__.name = "cities".to_string();
        seed.__common_attr__.original_file_path = DbtPath::from("seeds/cities.csv");
        set_state_explain_base(&mut seed.__base_attr__, DbtMaterialization::Seed, "cities");
        seed
    }

    fn values_request(record: RunCachePendingExecutionRecord) -> SubmitValuesRequest {
        match record.input {
            RunCachePendingExecutionInput::Values(request) => *request,
            RunCachePendingExecutionInput::Sql(_) => panic!("a seed submits values, not SQL"),
        }
    }

    async fn seed_write_only_request(allow_clones: bool) -> SubmitValuesRequest {
        let project_root = tempfile::tempdir().unwrap();
        let seed = seed_with_csv(project_root.path());
        let mut ctx = test_task_runner_ctx_with_allow_clones(allow_clones);
        set_project_root(&mut ctx, project_root.path());

        let record =
            prepare_write_only_execution_record(&ctx, &seed, &task_result_with_sql(""), None)
                .await
                .unwrap()
                .expect("a seed always produces a write-only record");
        values_request(record)
    }

    #[dbt_runtime::test]
    async fn seed_request_sends_zero_clone_chain_depth_limit_when_clones_disallowed() {
        // Regression test for https://github.com/dbt-labs/dbt-core/issues/16136.
        // Asserted on the built request rather than on the derivation: the bug
        // was that seeds hardcoded `clone_chain_depth_limit: None`, so a test of
        // the helper alone stays green through a revert. On Snowflake (no adapter
        // default) `Some(0)` can only come from `allow_clones: false`.
        assert_eq!(
            seed_write_only_request(false).await.clone_chain_depth_limit,
            Some(0)
        );
    }

    #[dbt_runtime::test]
    async fn seed_request_omits_clone_chain_depth_limit_when_clones_allowed() {
        assert_eq!(
            seed_write_only_request(true).await.clone_chain_depth_limit,
            None
        );
    }

    #[test]
    fn clone_chain_depth_limit_for_seed_uses_the_seed_adapter() {
        // The ctx default is Snowflake (no limit), so a Databricks seed proves
        // the node's own adapter decides, not `ctx.default_adapter_type()`.
        let ctx = test_task_runner_ctx_with_allow_clones(true);
        let seed = test_seed_on_adapter(AdapterType::Databricks);
        assert_eq!(clone_chain_depth_limit_for_seed(&ctx, &seed), Some(1));
    }

    #[test]
    fn drop_stale_view_sql_drops_when_target_is_a_view_and_materialization_is_not() {
        // Regression test: a target left over as a VIEW by a prior materialization
        // must be dropped before `CREATE OR REPLACE TABLE ... CLONE ...` can succeed.
        assert_eq!(
            drop_stale_view_sql(DbtMaterialization::Table, true, "dev.model_a"),
            Some("drop view if exists dev.model_a".to_string())
        );
        assert_eq!(
            drop_stale_view_sql(DbtMaterialization::Incremental, true, "dev.model_a"),
            Some("drop view if exists dev.model_a".to_string())
        );
    }

    #[test]
    fn drop_stale_view_sql_is_none_when_target_is_not_a_view() {
        assert_eq!(
            drop_stale_view_sql(DbtMaterialization::Table, false, "dev.model_a"),
            None
        );
    }

    #[test]
    fn drop_stale_view_sql_is_none_when_materialization_is_itself_a_view() {
        // Cloning a view target as a view is not the mismatch this guards against.
        assert_eq!(
            drop_stale_view_sql(DbtMaterialization::View, true, "dev.model_a"),
            None
        );
    }

    fn state_explain_model(materialized: DbtMaterialization) -> DbtModel {
        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = "model.test.orders".to_string();
        model.__common_attr__.name = "orders".to_string();
        set_state_explain_base(&mut model.__base_attr__, materialized, "orders");
        model
    }

    fn state_explain_seed() -> DbtSeed {
        let mut seed = DbtSeed::default();
        seed.__common_attr__.unique_id = "seed.test.cities".to_string();
        seed.__common_attr__.name = "cities".to_string();
        set_state_explain_base(&mut seed.__base_attr__, DbtMaterialization::Seed, "cities");
        seed
    }

    fn state_explain_snapshot() -> DbtSnapshot {
        let mut snapshot = DbtSnapshot::default();
        snapshot.__common_attr__.unique_id = "snapshot.test.orders_snapshot".to_string();
        snapshot.__common_attr__.name = "orders_snapshot".to_string();
        set_state_explain_base(
            &mut snapshot.__base_attr__,
            DbtMaterialization::Snapshot,
            "orders_snapshot",
        );
        snapshot
    }

    fn set_state_explain_base(
        base: &mut dbt_schemas::schemas::nodes::NodeBaseAttributes,
        materialized: DbtMaterialization,
        alias: &str,
    ) {
        base.database = "analytics".to_string();
        base.schema = "marts".to_string();
        base.alias = alias.to_string();
        base.materialized = materialized;
        base.quoting = ResolvedQuoting::trues();
    }
}
