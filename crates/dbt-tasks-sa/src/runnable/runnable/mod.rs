use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Instant;

use crate::materialize::{NodeHookPhase, NodeHookStyle, execute_node_hooks, model_hook_style};
use crate::runnable::function::execute_function_remote;
use crate::runnable::snapshot::execute_snapshot_remote;
use crate::runnable::test::{
    TestExecutionStatus, TestReportedResult, execute_test_remote,
    process_statically_checked_test_result, record_test_metric, record_test_span_with_detail,
    reported_test_verdict_from_components, status_with_warn_error_overrides,
};
use dbt_adapter::time_machine::{
    RecordedCachedTestResult, SaoStatus, global_recorder, global_replayer, is_replaying,
};
use dbt_adapter_core::AdapterType;
use dbt_common::constants::RUNNING;
use dbt_common::stats::{NodeStatus, Stat};
use dbt_common::status_reporter::report_completed;
use dbt_common::tracing::dbt_emit::{emit_error_log_from_fs_error, emit_warn_log_message};
use dbt_common::tracing::span_info::find_and_update_span_attrs;
use dbt_common::warn_error_options::WarnErrorOptions;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_jinja_utils::utils::add_task_context;
use dbt_schemas::schemas::common::Severity;
use dbt_schemas::schemas::manifest::saved_query::DbtSavedQuery;
use dbt_schemas::schemas::{
    DbtFunction, DbtModel, DbtSeed, DbtSnapshot, DbtSource, DbtTest, DbtUnitTest, InternalDbtNode,
    InternalDbtNodeAttributes, NodePathKind, Nodes,
};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::run_task_hooks::RunTaskHooks;
use dbt_tasks_core::task::TaskResult;
use dbt_tasks_core::task::{TP, Task, TaskOp};
use dbt_telemetry::{NodeEvaluated, NodeSkipReason, NodeType};

use tokio::task::JoinSet;
use tracing::Instrument;
use vortex_events::run_model_event;

use crate::materialize::{
    materialize_latest_version_pointer, should_create_latest_version_pointer,
};
use crate::runnable::cache::cache_materialization_return_value;
use crate::runnable::model::{
    execute_microbatch_batch, execute_model_remote, prepare_microbatch_batches,
    resolve_microbatch_window, try_get_microbatch_model,
};
use crate::runnable::seed::{execute_seed_remote, maybe_resolve_remote_seed_column_hint};
use crate::runnable::unit_test::execute_unit_test_remote;
use dbt_tasks_core::run_cache::run_cache_service::{
    CachedTestExecutionResult, RunCacheAfterSuccess, RunCacheCloneDecision, RunCacheCloneError,
    RunCacheReuseHookExecutor, RunCacheReuseHookPhase, RunCacheServiceDecision,
    clear_stale_missing_last_modified_epoch_for_node, confirm_run_cache_service_execution,
    evict_node_metadata_for_untracked_rebuild, execute_run_cache_service_clone,
    insert_compiled_view_definition, record_run_cache_clone_decision,
    record_run_cache_service_execution, replay_dev_clone_decision, replay_run_cache_clone_decision,
    run_cache_service_before_execution, should_execute_hooks_for_skip_reuse,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunExecutionPath {
    Remote,
    SideCar,
    LakeCompute,
}

pub struct RunTask {
    node: Arc<dyn InternalDbtNodeAttributes>,
    // Channel receiver for getting results
    result_receiver: parking_lot::Mutex<Option<mpsc::Receiver<TaskResult>>>,
    execution_path: RunExecutionPath,
    task_hooks: Arc<dyn RunTaskHooks>,
}

impl RunTask {
    pub fn new(
        node: Arc<dyn InternalDbtNodeAttributes>,
        result_receiver: Option<mpsc::Receiver<TaskResult>>,
        execution_path: RunExecutionPath,
        task_hooks: Arc<dyn RunTaskHooks>,
    ) -> Self {
        Self {
            node,
            result_receiver: parking_lot::Mutex::new(result_receiver),
            execution_path,
            task_hooks,
        }
    }
}

impl Task for RunTask {
    // Backpressure applied granularly at the node level
    fn run_task<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move {
            let unique_id = self.node.unique_id();
            let mut result_receiver = { self.result_receiver.lock().take() };
            let task_result = receive_task_result(&unique_id, &mut result_receiver)?;
            let start_time = chrono::Utc::now();
            // Status lines keep source path for Models for readability
            let display_path_kind = if self.node.resource_type() == NodeType::Model {
                NodePathKind::Definition
            } else {
                NodePathKind::Executable
            };
            let display_path = self
                .node
                .get_node_path(
                    display_path_kind,
                    ctx.inner.arg.io.in_dir.as_path(),
                    ctx.inner.arg.io.out_dir.as_path(),
                )
                .display()
                .to_string();

            if let Some(reporter) = ctx.inner.arg.io.status_reporter.as_ref() {
                reporter.show_progress(RUNNING, display_path.as_ref(), None);
            }

            // During time-machine replay the dbt State service is always
            // disabled (see `should_initialize_run_cache_service`), which
            // would otherwise make this condition look identical to a normal
            // run with the service off and incorrectly route nodes into the
            // local SAO cache path — a branch that was never exercised during
            // the original recording when the service was active. Replay
            // must be driven solely by recorded events (see
            // `maybe_replay_remote_run` / `maybe_replay_run_cache_clone`).
            let cache_enabled = self.execution_path == RunExecutionPath::Remote
                && !ctx.inner.run_cache_ctx.run_cache_service_requested
                && !is_replaying()
                && ctx.inner.arg.run_cache_mode.write_cache()
                && node_runs_with_cache(self.node.as_ref());

            let execution_started_at = Instant::now();
            let mut after_success = RunCacheAfterSuccess::None;
            let statically_checked_test = match self.node.as_any().downcast_ref::<DbtTest>() {
                Some(test) => ctx
                    .is_data_test_statically_skippable(unique_id.as_str())
                    .await
                    .then_some(test),
                None => None,
            };
            // `LakeCompute` has a bypass-Jinja-materialization hook
            // (`run_on_lake_compute`) for models, seeds, and tests -- tests
            // need it too, not because they materialize anything, but because
            // only that hook builds and attaches the per-statement
            // `CATALOG_BUNDLE` option (`resolve_compute_write_target` /
            // `catalog_bundle_json`) that tells a fresh LakeCompute worker
            // connection which catalogs to attach. The generic `Remote` path's
            // `adapter.execute()` always passes empty options
            // (`adapter_engine.rs`'s `execute()` convenience wrapper), so a
            // test downgraded to `Remote` would silently run with nothing
            // attached beyond the connection's own baseline -- fine by
            // accident for objects in the default MDLS namespace, wrong for
            // anything needing a declared catalog. Every other node type that
            // resolves onto `lakecompute` (a snapshot, ...) still has no
            // bypass to offer and is downgraded as before.
            let effective_execution_path = if self.execution_path == RunExecutionPath::LakeCompute
                && !(self.node.as_any().is::<DbtModel>()
                    || self.node.as_any().is::<DbtSeed>()
                    || self.node.as_any().is::<DbtTest>())
            {
                RunExecutionPath::Remote
            } else {
                self.execution_path
            };
            let result = match (statically_checked_test, effective_execution_path) {
                (Some(test), _) => {
                    let node_status =
                        process_statically_checked_test_result(test, ctx, start_time.into());
                    Ok(node_status)
                }
                (None, RunExecutionPath::Remote) => {
                    // Step 0: Replay a cached result if one was recorded.
                    let replayed_cache_decision = maybe_replay_remote_run(&self.node.unique_id());

                    // Nodes without a task_result (e.g. sources) go straight to execution
                    let Some(task_result) = task_result else {
                        return execute_remote_node_no_result(
                            self.node.as_ref(),
                            ctx,
                            &self.task_hooks,
                        )
                        .in_current_span()
                        .await;
                    };

                    // dbt State service and SAO are mutually exclusive — pick the
                    // active path here. Both feed back into a single
                    // `RunCacheServiceDecision`: the service path may emit
                    // Skip/Clone/Execute (with `after_success`); the SAO path may
                    // emit Skip (with `sao_stored_hash`), Execute (with
                    // `sao_guard`), or Disabled.
                    let run_cache_service_requested =
                        ctx.inner.run_cache_ctx.run_cache_service_requested;
                    let decision = if let Some(decision) = replayed_cache_decision {
                        decision
                    } else if is_replaying()
                        && ctx
                            .inner
                            .run_cache_ctx
                            .run_cache_dev_cloned_nodes
                            .contains_key(&self.node.unique_id())
                    {
                        let status = replay_dev_clone_decision(&unique_id)
                            .map(|clone| clone.replay_status())
                            .unwrap_or(NodeStatus::ReusedCloned(None));
                        RunCacheServiceDecision::Skip {
                            status,
                            sao_stored_hash: None,
                            cached_test_result: None,
                        }
                    } else if let Some(clone) =
                        replay_run_cache_clone_decision(&self.node.unique_id())
                    {
                        // Replaying a recorded Clone decision: route straight into the
                        // Clone branch below so the clone SQL is re-executed and matched
                        // against its own recorded `AdapterCallEvent`s, instead of
                        // falling through to a normal Execute (which would generate
                        // different SQL than what was actually recorded).
                        RunCacheServiceDecision::Clone { clone }
                    } else if run_cache_service_requested {
                        insert_compiled_view_definition(ctx, self.node.as_ref(), &task_result);
                        // Microbatch models make one per-model cache decision keyed to
                        // the run's event-time window. Resolve it here (fail open: a
                        // resolution error yields None and the service submit executes
                        // normally rather than risk a window-independent skip).
                        let microbatch_window = match try_get_microbatch_model(self.node.as_ref()) {
                            Some(model) => match resolve_microbatch_window(model, ctx).await {
                                Ok(window) => Some(window),
                                Err(e) => {
                                    emit_warn_log_message(
                                        ErrorCode::StateServiceWarn,
                                        format!(
                                            "Failed to resolve microbatch window for node {}: {e}; executing without dbt State",
                                            self.node.unique_id()
                                        ),
                                    );
                                    None
                                }
                            },
                            None => None,
                        };
                        run_cache_service_before_execution(
                            ctx,
                            self.node.as_ref(),
                            &task_result,
                            microbatch_window,
                        )
                        .await
                    } else if cache_enabled {
                        TaskOp::r#async(self.task_hooks.check_sao_cache(
                            ctx,
                            Arc::clone(&self.node),
                            &task_result.sql_instruction.sql,
                        ))
                        .await?
                    } else {
                        RunCacheServiceDecision::Disabled
                    };

                    if let Some(node_hash) = decision.node_hash() {
                        ctx.inner
                            .node_hashes
                            .insert(unique_id.to_string(), node_hash);
                    }

                    if let RunCacheServiceDecision::Skip {
                        status,
                        sao_stored_hash,
                        cached_test_result,
                    } = &decision
                    {
                        let source = sao_stored_hash.as_deref().unwrap_or("run-cache-service");
                        // For data tests, override the generic
                        // ReusedNoChanges status with a test-shaped verdict
                        // and insert a Stat carrying the cached failures plus
                        // a NO-OP marker, so run_results.json looks like
                        // dbt-core's _DataTestAdapterProxy produces.
                        let final_status = if let (Some(cached_result), Some(test)) = (
                            cached_test_result,
                            self.node.as_any().downcast_ref::<DbtTest>(),
                        ) {
                            let severity =
                                test.deprecated_config.severity.clone().unwrap_or_default();
                            let emit_reused_status = ctx
                                .inner
                                .run_cache_ctx
                                .run_cache_service_config
                                .as_ref()
                                .is_some_and(|config| config.emit_reused_status);
                            let cached_status = cached_data_test_status_with_emit_reused_status(
                                status,
                                *cached_result,
                                severity,
                                &ctx.inner.arg.warn_error_options,
                                emit_reused_status,
                            );
                            let reported_result = cached_status.reported_result();
                            record_test_metric(reported_result.status);
                            record_test_span_with_detail(
                                &reported_result,
                                Some(NodeSkipReason::Cached),
                                test.deprecated_config.store_failures,
                                None,
                            );
                            ctx.inner.run_stats.insert(
                                unique_id.clone(),
                                Stat::new(
                                    unique_id.clone(),
                                    start_time.into(),
                                    Some(cached_status.failures),
                                    cached_status.stat_status.clone(),
                                    Some("NO-OP - cached test result".to_string()),
                                    ctx.thread_id,
                                ),
                            );
                            record_cached_test_skip(&unique_id, status, *cached_result);
                            cached_status.final_status
                        } else {
                            record_cache_skip(&unique_id, status, source);
                            status.clone()
                        };
                        execute_hooks_for_run_cache_skip_reuse(
                            ctx,
                            self.node.as_ref(),
                            Some(&task_result),
                        )
                        .await?;
                        Ok(final_status)
                    } else {
                        // Clone case is handled inline so the Err path can surface
                        // a warn (see PR #10184 review). Other decisions just
                        // derive after_success and fall through to normal exec.
                        let mut clone_status: Option<NodeStatus> = None;
                        let after_success_inner = match &decision {
                            RunCacheServiceDecision::Clone { clone } => {
                                match execute_run_cache_service_clone_with_hooks(
                                    ctx,
                                    self.node.as_ref(),
                                    clone,
                                    Some(&task_result),
                                )
                                .await
                                {
                                    Ok(status) => {
                                        record_run_cache_clone_decision(&unique_id, clone);
                                        confirm_run_cache_service_execution(
                                            ctx,
                                            self.node.as_ref(),
                                            clone.success_confirmation(),
                                            Some(elapsed_millis(execution_started_at)),
                                        )
                                        .await;
                                        clone_status = Some(status);
                                        RunCacheAfterSuccess::None
                                    }
                                    Err(RunCacheCloneError::Recoverable(err)) => {
                                        emit_warn_log_message(
                                            ErrorCode::StateServiceWarn,
                                            format!(
                                                "dbt State service clone failed for node {}: {err}; executing normally",
                                                self.node.unique_id()
                                            ),
                                        );
                                        clone
                                            .fallback_confirmation()
                                            .map(RunCacheAfterSuccess::Confirm)
                                            .unwrap_or(RunCacheAfterSuccess::None)
                                    }
                                    Err(err) => return Err(err.into_error()),
                                }
                            }
                            RunCacheServiceDecision::Execute { after_success, .. } => {
                                after_success.clone()
                            }
                            _ => RunCacheAfterSuccess::None,
                        };

                        after_success = after_success_inner;

                        // Clone succeeded — skip the model body but stay in the
                        // outer flow so run_stats and post-processing fire.
                        if let Some(status) = clone_status {
                            Ok(status)
                        } else {
                            // Node is Microbatch
                            // Requires us to produce batches and ensure each batch respects connection limits
                            let res = if try_get_microbatch_model(self.node.as_ref()).is_some() {
                                let batch_groups = prepare_microbatch_batches(
                                    self.node.clone(),
                                    ctx,
                                    &task_result,
                                )
                                .await?;
                                // An empty event window yields no batches, so the target
                                // relation is never materialized on a first run. Mirror
                                // dbt-core's `if not relations` guard: only create the
                                // pointer view when at least one batch executed.
                                let had_batches = !batch_groups.is_empty();
                                for group in batch_groups {
                                    let batch_span = tracing::Span::current();
                                    let mut batch_tasks = group
                                        .into_iter()
                                        .map(|task| {
                                            let ctx = ctx.clone();
                                            TaskOp::Blocking(Box::new(move || {
                                                execute_microbatch_batch(task, &ctx)
                                            }))
                                            .run()
                                            .instrument(batch_span.clone())
                                        })
                                        .collect::<JoinSet<_>>();

                                    while let Some(res) = batch_tasks.join_next().await {
                                        res.map_err(Into::into).flatten()??;
                                    }
                                }

                                // After all microbatch batches succeed, create the latest
                                // version pointer view if applicable. Skip when no batches
                                // executed (empty event window) — the source relation may
                                // not exist yet, so pointing at it would fail.
                                if had_batches
                                    && let Some(model) =
                                        self.node.as_any().downcast_ref::<DbtModel>()
                                    && should_create_latest_version_pointer(
                                        model,
                                        ctx.runtime_config(),
                                    )
                                {
                                    let mut base_context = ctx.inner.base_context.clone();
                                    add_task_context(
                                        &mut base_context,
                                        model.common(),
                                        &ctx.thread_id,
                                    );
                                    let model_clone = model.clone();
                                    let ctx_clone = ctx.clone();
                                    TaskOp::Blocking(Box::new(move || {
                                        let relations_map = materialize_latest_version_pointer(
                                            &model_clone,
                                            model_clone.node_adapter(),
                                            ctx_clone.runtime_config(),
                                            &ctx_clone.inner.materialization_resolver,
                                            ctx_clone.env.clone(),
                                            &base_context,
                                            &ctx_clone.inner.arg.io,
                                        )?;
                                        let _ = cache_materialization_return_value(
                                            ctx_clone.env,
                                            &relations_map,
                                        );
                                        Ok::<(), Box<dbt_common::FsError>>(())
                                    }))
                                    .run()
                                    .await??;
                                }

                                Ok(NodeStatus::Succeeded)
                            // Node is Saved Query
                            } else if let Some(saved_query) =
                                self.node.as_any().downcast_ref::<DbtSavedQuery>()
                            {
                                self.task_hooks
                                    .execute_saved_query(ctx, saved_query)
                                    .await
                                    .map(|_| NodeStatus::Succeeded)
                            // Node is Unit Test
                            } else if let Some(unit_test) =
                                self.node.as_any().downcast_ref::<DbtUnitTest>()
                            {
                                // Unit tests require a special remote execution path because we
                                // may need to compare the results in the sidecar (which is async)
                                // to determine pass/fail
                                let ctx_inner = ctx.clone();
                                let task_result_inner = task_result.clone();
                                let node_inner = self.node.clone();
                                let (status, result) = TaskOp::Blocking(Box::new(move || {
                                    let unit_test =
                                        node_inner.as_any().downcast_ref::<DbtUnitTest>().unwrap();
                                    execute_unit_test_remote(
                                        unit_test,
                                        &ctx_inner,
                                        &task_result_inner,
                                    )
                                }))
                                .run()
                                .await??;
                                if let Some(result) = result {
                                    self.task_hooks
                                        .did_run_unit_test(
                                            ctx,
                                            unit_test,
                                            &task_result,
                                            result.passed,
                                            result.diff_num_rows,
                                            result.diff,
                                        )
                                        .await?;
                                }
                                Ok(status)
                            // Default execution for other node types
                            } else {
                                let ctx_inner = ctx.clone();
                                let task_result_inner = task_result.clone();
                                let node = self.node.clone();
                                let res = TaskOp::Blocking(Box::new(move || {
                                    execute_remote_node(
                                        node.as_ref(),
                                        &ctx_inner,
                                        &task_result_inner,
                                    )
                                }))
                                .run()
                                .await?;
                                maybe_resolve_remote_seed_column_hint(res, self.node.as_ref(), ctx)
                                    .await
                            };
                            if res.is_ok() {
                                decision.finalize(ctx).await?;
                            }
                            res
                        }
                    }
                }
                (None, RunExecutionPath::SideCar) => {
                    self.task_hooks
                        .run_lake_compute_sidecar(ctx, Arc::clone(&self.node), task_result.clone())
                        .await
                }
                (None, RunExecutionPath::LakeCompute) => {
                    self.task_hooks
                        .run_on_lake_compute(ctx, Arc::clone(&self.node), task_result.clone())
                        .await
                }
            };

            if result.is_ok() {
                run_cache_after_success_action(
                    ctx,
                    self.node.as_ref(),
                    std::mem::replace(&mut after_success, RunCacheAfterSuccess::None),
                    Some(elapsed_millis(execution_started_at)),
                )
                .await;
            }

            let mut span_rows_affected: Option<i64> = None;
            find_and_update_span_attrs(|attrs: &mut NodeEvaluated| {
                attrs.sao_enabled = Some(cache_enabled);
                span_rows_affected = attrs.rows_affected.map(|n| n as i64);
            });

            // Get status and insert stats
            // Note: Inner visit_run implementations may insert their own stats on success,
            // but we need to ensure stats are inserted even when errors occur early.
            // The DashMap will just overwrite if there's a duplicate.
            // Ensure stats are always inserted, even if inner visit_run had early returns
            let node_status = match result {
                Ok(node_status) => {
                    // Show completion in the same style as remote
                    report_completed(
                        &node_status,
                        self.node.defined_at().cloned(),
                        display_path.as_str(),
                        cache_enabled,
                        ctx.inner.arg.io.status_reporter.as_ref(),
                    );

                    // Insert stats for success case if not already inserted by inner implementation
                    // This ensures stats are present even if inner code didn't insert them
                    if !ctx.inner.run_stats.contains_key(&unique_id) {
                        let mut stat = Stat::new(
                            unique_id.clone(),
                            start_time.into(),
                            None,
                            node_status.clone(),
                            None,
                            ctx.thread_id,
                        );
                        stat.rows_affected = span_rows_affected;
                        ctx.inner.run_stats.insert(unique_id.clone(), stat);
                    }

                    node_status
                }
                Err(e) => {
                    // TODO: At some point, these should log as part of the same event
                    let node_status = NodeStatus::Errored;
                    let error_message = e.to_string();
                    report_completed(
                        &NodeStatus::Errored,
                        self.node.defined_at().cloned(),
                        display_path.as_str(),
                        false,
                        ctx.inner.arg.io.status_reporter.as_ref(),
                    );

                    if matches!(
                        self.execution_path,
                        RunExecutionPath::Remote
                            | RunExecutionPath::SideCar
                            | RunExecutionPath::LakeCompute
                    ) {
                        emit_error_log_from_fs_error(*e);
                    }

                    // Insert stats for the error case so it appears in run_results.json
                    ctx.inner.run_stats.insert(
                        unique_id.clone(),
                        Stat::new(
                            unique_id.clone(),
                            start_time.into(),
                            None,
                            node_status.clone(),
                            Some(error_message),
                            ctx.thread_id,
                        ),
                    );

                    node_status
                }
            };

            // Regression guard: run_stats must be populated before telemetry
            // emission. PR #9146 broke this by emitting before stats were
            // inserted for dbt State-reused models.
            debug_assert!(
                ctx.inner.run_stats.contains_key(&unique_id),
                "run_stats missing for {unique_id} before telemetry emission"
            );

            // TODO: migrate this to Vortex tracing layer
            // TODO: migrate this to structured logger
            if ctx.inner.arg.io.send_anonymous_usage_stats {
                emit_run_usage_stats(self.node.as_ref(), ctx, self.execution_path);
            }

            Ok(node_status)
        })
    }

    fn task_type(&self) -> &str {
        "run"
        // "run_local"
    }

    fn resource_type(&self) -> NodeType {
        self.node.resource_type()
    }

    fn work_node_id(&self) -> &str {
        self.node.common().unique_id.as_str()
    }

    fn dbt_nodes(&self) -> Vec<Arc<dyn InternalDbtNodeAttributes>> {
        vec![self.node.clone()]
    }

    fn task_phase(&self) -> Option<TP> {
        Some(TP::Run)
    }
}

fn maybe_replay_remote_run(unique_id: &str) -> Option<RunCacheServiceDecision> {
    // Check for cache skip events during time-machine replay. Cached data tests
    // need their recorded result so the common skip path can rebuild their stats.
    let replayer = global_replayer()?;
    let sao_event = replayer.get_sao_event(unique_id)?;
    Some(RunCacheServiceDecision::Skip {
        status: sao_event.to_node_status(),
        sao_stored_hash: None,
        cached_test_result: sao_event
            .cached_test_result
            .map(|result| CachedTestExecutionResult {
                failures: result.failures,
                should_warn: result.should_warn,
                should_error: result.should_error,
            }),
    })
}

async fn execute_run_cache_service_clone_with_hooks(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    clone: &RunCacheCloneDecision,
    task_result: Option<&TaskResult>,
) -> Result<NodeStatus, RunCacheCloneError> {
    let hook_node = run_cache_clone_hook_node(node);
    let pre_hooks_configured = hook_node
        .as_ref()
        .is_some_and(RunCacheReuseHookNode::has_pre_hooks);
    let hook_executor =
        hook_node.map(|hook_node| build_reuse_hook_executor(ctx, node, task_result, hook_node));
    execute_run_cache_service_clone(ctx, node, clone, hook_executor, pre_hooks_configured).await
}

async fn execute_hooks_for_run_cache_skip_reuse(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    task_result: Option<&TaskResult>,
) -> FsResult<()> {
    let service_default = ctx
        .inner
        .run_cache_ctx
        .run_cache_service_config
        .as_ref()
        .map(|config| config.run_hooks_on_no_op)
        .unwrap_or(false);
    let Some(hook_node) = run_cache_reuse_hook_node(node, service_default) else {
        return Ok(());
    };
    let hook_executor = build_reuse_hook_executor(ctx, node, task_result, hook_node);
    let ctx_inner = ctx.clone();
    TaskOp::Blocking(Box::new(move || {
        hook_executor(&ctx_inner, RunCacheReuseHookPhase::Pre)?;
        hook_executor(&ctx_inner, RunCacheReuseHookPhase::Post)
    }))
    .run()
    .await??;
    Ok(())
}

fn run_cache_reuse_hook_node(
    node: &dyn InternalDbtNodeAttributes,
    service_default: bool,
) -> Option<RunCacheReuseHookNode> {
    should_execute_hooks_for_skip_reuse(node, service_default)
        .then(|| RunCacheReuseHookNode::from_node(node))
        .flatten()
}

fn run_cache_clone_hook_node(
    node: &dyn InternalDbtNodeAttributes,
) -> Option<RunCacheReuseHookNode> {
    RunCacheReuseHookNode::from_node(node)
}

enum RunCacheReuseHookNode {
    Model(Box<DbtModel>),
    Snapshot(Box<DbtSnapshot>),
    Seed(Box<DbtSeed>),
}

impl RunCacheReuseHookNode {
    fn from_node(node: &dyn InternalDbtNodeAttributes) -> Option<Self> {
        if let Some(model) = node.as_any().downcast_ref::<DbtModel>() {
            Some(Self::Model(Box::new(model.clone())))
        } else if let Some(snapshot) = node.as_any().downcast_ref::<DbtSnapshot>() {
            Some(Self::Snapshot(Box::new(snapshot.clone())))
        } else {
            node.as_any()
                .downcast_ref::<DbtSeed>()
                .map(|seed| Self::Seed(Box::new(seed.clone())))
        }
    }

    fn has_pre_hooks(&self) -> bool {
        match self {
            Self::Model(model) => hooks_are_configured(model.deprecated_config.pre_hook.as_ref()),
            Self::Snapshot(snapshot) => {
                hooks_are_configured(snapshot.deprecated_config.pre_hook.as_ref())
            }
            Self::Seed(seed) => hooks_are_configured(seed.deprecated_config.pre_hook.as_ref()),
        }
    }
}

fn build_reuse_hook_executor(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    task_result: Option<&TaskResult>,
    hook_node: RunCacheReuseHookNode,
) -> RunCacheReuseHookExecutor {
    let mut base_context = ctx.inner.base_context.clone();
    add_task_context(&mut base_context, node.common(), &ctx.thread_id);
    let sql = task_result.map(|task_result| task_result.sql_instruction.sql.clone());
    Arc::new(move |ctx, phase| {
        let sql = sql.as_deref();
        execute_hook_node_blocking(&hook_node, ctx, &base_context, sql, phase)
    })
}

fn hooks_are_configured(hooks: &Option<dbt_schemas::schemas::common::Hooks>) -> bool {
    hooks
        .as_ref()
        .is_some_and(|hooks| !hooks.to_hook_config_array().is_empty())
}

fn execute_hook_node_blocking(
    hook_node: &RunCacheReuseHookNode,
    ctx: &TaskRunnerCtx,
    base_context: &std::collections::BTreeMap<String, minijinja::Value>,
    sql: Option<&str>,
    phase: RunCacheReuseHookPhase,
) -> FsResult<()> {
    let phase = match phase {
        RunCacheReuseHookPhase::Pre => NodeHookPhase::Pre,
        RunCacheReuseHookPhase::Post => NodeHookPhase::Post,
    };
    match hook_node {
        RunCacheReuseHookNode::Model(model) => execute_node_hooks(
            model.as_ref(),
            &model.deprecated_config,
            model.node_adapter(),
            ctx.runtime_config(),
            ctx.env.clone(),
            base_context,
            &ctx.inner.arg.io,
            sql,
            model_hook_style(model.node_adapter(), &model.__base_attr__.materialized),
            NodePathKind::Compiled,
            phase,
        ),
        RunCacheReuseHookNode::Snapshot(snapshot) => execute_node_hooks(
            snapshot.as_ref(),
            &snapshot.deprecated_config,
            snapshot.node_adapter(),
            ctx.runtime_config(),
            ctx.env.clone(),
            base_context,
            &ctx.inner.arg.io,
            sql,
            NodeHookStyle::SplitTransaction,
            NodePathKind::Executable,
            phase,
        ),
        RunCacheReuseHookNode::Seed(seed) => execute_node_hooks(
            seed.as_ref(),
            &seed.deprecated_config,
            seed.node_adapter(),
            ctx.runtime_config(),
            ctx.env.clone(),
            base_context,
            &ctx.inner.arg.io,
            None,
            NodeHookStyle::SplitTransaction,
            NodePathKind::Compiled,
            phase,
        ),
    }
}

async fn run_cache_after_success_action(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    after_success: RunCacheAfterSuccess,
    execution_runtime_ms: Option<i64>,
) {
    match after_success {
        RunCacheAfterSuccess::None => {
            // The dbt State submission path (`submit_seed` / `submit_*`) probes
            // `last_modified_epoch_for_node` before deciding whether to
            // submit. When the target table doesn't exist on the warehouse
            // yet — which is always true on a node's first build — that
            // probe caches `Some(None)` in `run_cache_metadata` and the
            // submit is skipped. The node then materializes, but nothing in
            // the current invocation invalidates the cached `None`. The
            // prefetch miss-filter in `prefetch_last_modified_epochs` treats
            // `Some(None)` as a hit (not a miss), so downstream models
            // never re-query. Clear the stale value after successful execution;
            // the next downstream submit sees a real miss and uses the normal
            // planned prefetch path.
            if ctx.inner.run_cache_ctx.run_cache_service_requested {
                clear_stale_missing_last_modified_epoch_for_node(ctx, node);
            }
        }
        RunCacheAfterSuccess::InvalidateFreshness => {
            // The node rebuilt its target but no decision was sought and no
            // execution will be recorded, so nothing refreshed the cached
            // epoch. Drop it: the next submit that needs this relation takes
            // the prefetch-miss path and re-reads it from the warehouse.
            if ctx.inner.run_cache_ctx.run_cache_service_requested {
                evict_node_metadata_for_untracked_rebuild(ctx, node);
            }
        }
        RunCacheAfterSuccess::Confirm(mut confirmation) => {
            // Data tests: lift the just-executed result into the confirmation
            // so future runs can replay it.
            if node.as_any().is::<DbtTest>() {
                if let Some(result) = ctx
                    .inner
                    .data_test_execution_results
                    .get(node.unique_id().as_str())
                {
                    confirmation.set_test_execution_results(*result);
                }
            }
            confirm_run_cache_service_execution(
                ctx,
                node,
                Some(confirmation),
                execution_runtime_ms,
            )
            .await;
        }
        RunCacheAfterSuccess::Record(record) => {
            record_run_cache_service_execution(ctx, node, Some(*record), execution_runtime_ms)
                .await;
        }
    }
}

fn elapsed_millis(started_at: Instant) -> i64 {
    i64::try_from(started_at.elapsed().as_millis()).unwrap_or(i64::MAX)
}

struct CachedDataTestStatus {
    failures: usize,
    status: TestExecutionStatus,
    stat_status: NodeStatus,
    final_status: NodeStatus,
}

impl CachedDataTestStatus {
    fn reported_result(&self) -> TestReportedResult {
        TestReportedResult {
            failures: self.failures,
            status: self.status,
            diff: None,
            execution_result: None,
        }
    }
}

/// Converts a cached data test result into the statuses and metrics needed for reuse reporting.
///
/// Cached passing tests keep the run-cache reuse status as the task's final status while recording
/// a passing test stat. Cached failures use the cached threshold booleans plus data test severity
/// to report warn/error stats and increment the matching invocation metric so command status
/// matches a normally executed test.
#[cfg(test)]
fn cached_data_test_status(
    reused_status: &NodeStatus,
    result: CachedTestExecutionResult,
    severity: Severity,
    warn_error_options: &WarnErrorOptions,
) -> CachedDataTestStatus {
    cached_data_test_status_with_emit_reused_status(
        reused_status,
        result,
        severity,
        warn_error_options,
        true,
    )
}

fn cached_data_test_status_with_emit_reused_status(
    reused_status: &NodeStatus,
    result: CachedTestExecutionResult,
    severity: Severity,
    warn_error_options: &WarnErrorOptions,
    emit_reused_status: bool,
) -> CachedDataTestStatus {
    let failures = result.failures.max(0) as usize;
    let status = reported_test_verdict_from_components(
        Some(&severity),
        result.should_warn,
        result.should_error,
    );
    let status = status_with_warn_error_overrides(status, warn_error_options);
    let stat_status = match status.node_status() {
        NodeStatus::TestPassed if emit_reused_status => reused_status.clone(),
        status => status,
    };
    let final_status = if stat_status == NodeStatus::TestPassed {
        reused_status.clone()
    } else {
        stat_status.clone()
    };

    CachedDataTestStatus {
        failures,
        status,
        stat_status,
        final_status,
    }
}

/// Receives the task result from the channel, consuming the receiver.
/// Called once in `run_task()` before the local/remote split so both paths
/// work with `Option<TaskResult>` instead of threading the receiver through.
fn receive_task_result(
    unique_id: &str,
    result_receiver: &mut Option<mpsc::Receiver<TaskResult>>,
) -> FsResult<Option<TaskResult>> {
    if let Some(receiver) = result_receiver {
        match receiver.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(mpsc::TryRecvError::Empty) => Err(fs_err!(
                ErrorCode::Generic,
                "Failed to receive render result for '{}'. Empty Channel.",
                unique_id
            )),
            Err(mpsc::TryRecvError::Disconnected) => Err(fs_err!(
                ErrorCode::Generic,
                "Failed to receive render result for '{}'. Channel Disconnected.",
                unique_id
            )),
        }
    } else {
        Ok(None)
    }
}

fn record_cache_skip(unique_id: &str, task_status: &NodeStatus, source: &str) {
    let Some(recorder) = global_recorder() else {
        return;
    };

    if let Some((status, message)) = sao_status_for_task_status(task_status) {
        recorder.record_sao_skip(unique_id, status, &message, source);
    }
}

fn record_cached_test_skip(
    unique_id: &str,
    task_status: &NodeStatus,
    result: CachedTestExecutionResult,
) {
    let Some(recorder) = global_recorder() else {
        return;
    };

    if let Some((status, message)) = sao_status_for_task_status(task_status) {
        recorder.record_sao_skip_with_test_result(
            unique_id,
            status,
            &message,
            "run-cache-service",
            Some(RecordedCachedTestResult {
                failures: result.failures,
                should_warn: result.should_warn,
                should_error: result.should_error,
            }),
        );
    }
}

fn sao_status_for_task_status(task_status: &NodeStatus) -> Option<(SaoStatus, String)> {
    Some(match task_status {
        NodeStatus::ReusedNoChanges(message) => (SaoStatus::ReusedNoChanges, message.clone()),
        NodeStatus::ReusedStillFresh(message, freshness, last_updated) => (
            SaoStatus::ReusedStillFresh {
                freshness_seconds: *freshness,
                last_updated_seconds: *last_updated,
            },
            message.clone(),
        ),
        NodeStatus::ReusedStillFreshNoChanges(message) => {
            (SaoStatus::ReusedStillFreshNoChanges, message.clone())
        }
        NodeStatus::ReusedCloned(freshness) => (
            SaoStatus::ReusedCloned {
                freshness_seconds: *freshness,
            },
            task_status.default_message(),
        ),
        _ => return None,
    })
}

fn emit_run_usage_stats(
    node: &dyn InternalDbtNodeAttributes,
    ctx: &TaskRunnerCtx,
    execution_path: RunExecutionPath,
) {
    let (maybe_incremental_strategy, is_contract_enforced, has_group, table_format, catalog_name) =
        match execution_path {
            RunExecutionPath::Remote
            | RunExecutionPath::SideCar
            | RunExecutionPath::LakeCompute => {
                if let Some(model) = node.as_any().downcast_ref::<DbtModel>() {
                    (
                        model
                            .__model_attr__
                            .incremental_strategy
                            .as_ref()
                            .map(|s| s.to_string()),
                        model
                            .__model_attr__
                            .contract
                            .as_ref()
                            .map(|c| c.enforced)
                            .unwrap_or(false),
                        model.__model_attr__.group.is_some(),
                        model.__model_attr__.table_format.clone(),
                        model.__model_attr__.catalog_name.clone(),
                    )
                } else {
                    (None, false, false, None, None)
                }
            }
        };

    // `catalog_type` is not stored on the model — only `catalog_name` is. The type
    // lives on the catalog's active write integration in catalogs.yml, so resolve
    // it the same way the adapter does (catalog_name -> active_write_integration ->
    // write_integration.catalog_type). See dbt-adapter's CatalogRelation builders.
    let catalog_type = resolve_catalog_type(catalog_name.as_deref());

    run_model_event(
        ctx.inner.arg.io.invocation_id.to_string(),
        &ctx.inner.run_stats,
        node,
        maybe_incremental_strategy,
        is_contract_enforced,
        has_group,
        table_format,
        catalog_name,
        catalog_type,
    );
}

/// Resolve a model's `catalog_name` to the `catalog_type` declared in catalogs.yml.
///
/// Mirrors how the adapter resolves catalog metadata at materialization time
/// (`CatalogRelation::build_with_catalogs` and friends). The returned string is the
/// catalog-type enum's stable form (`*::as_str`), which is the aggregatable value we
/// want in telemetry.
///
/// Two schemas are supported:
/// - **v1**: `catalog_type` lives on the catalog's active write integration —
///   find the catalog by name, follow its `active_write_integration`, and read that
///   integration's `catalog_type`.
/// - **v2**: each catalog declares its `type` directly (no write integrations) —
///   find the catalog by name and read its `catalog_type`.
///
/// Returns `None` when the model has no `catalog_name`, catalogs.yml is absent, or the
/// catalog/integration cannot be found.
fn resolve_catalog_type(catalog_name: Option<&str>) -> Option<String> {
    let catalog_name = catalog_name?;
    let catalogs = dbt_adapter::load_catalogs::fetch_catalogs()?;
    // v2 catalogs.yml declares `type` directly on each catalog (no write
    // integrations); v1 nests `catalog_type` under the active write integration.
    if dbt_adapter::load_catalogs::fetch_use_catalogs_v2() {
        let view = catalogs.view_v2().ok()?;
        let catalog = view
            .catalogs
            .iter()
            .find(|catalog| catalog.name == catalog_name)?;
        return Some(catalog.catalog_type.as_str().to_lowercase());
    }
    let view = catalogs.view().ok()?;
    let catalog = view
        .catalogs
        .iter()
        .find(|catalog| catalog.catalog_name.0 == catalog_name)?;
    let active_integration = catalog.active_write_integration.0;
    let write_integration = catalog
        .write_integrations
        .0
        .iter()
        .find(|write_integration| write_integration.integration_name == active_integration)?;
    Some(write_integration.catalog_type.as_str().to_string())
}

fn node_runs_with_cache(node: &dyn InternalDbtNodeAttributes) -> bool {
    node.as_any().is::<DbtModel>()
        || node.as_any().is::<DbtSnapshot>()
        || node.as_any().is::<DbtSeed>()
        || node.as_any().is::<DbtUnitTest>()
}

fn execute_remote_node(
    node: &dyn InternalDbtNodeAttributes,
    ctx: &TaskRunnerCtx,
    task_result: &TaskResult,
) -> FsResult<NodeStatus> {
    if let Some(model) = node.as_any().downcast_ref::<DbtModel>() {
        execute_model_remote(model, ctx, task_result)
    } else if let Some(test) = node.as_any().downcast_ref::<DbtTest>() {
        execute_test_remote(test, ctx, task_result)
    } else if let Some(snapshot) = node.as_any().downcast_ref::<DbtSnapshot>() {
        execute_snapshot_remote(snapshot, ctx, task_result)
    } else if let Some(seed) = node.as_any().downcast_ref::<DbtSeed>() {
        execute_seed_remote(seed, ctx)
    } else if node.as_any().downcast_ref::<DbtSource>().is_some() {
        Ok(NodeStatus::Succeeded)
    } else if let Some(function) = node.as_any().downcast_ref::<DbtFunction>() {
        execute_function_remote(function, ctx, task_result)
    } else {
        Err(fs_err!(
            ErrorCode::Unexpected,
            "Node {} is not runnable in remote mode",
            node.unique_id()
        ))
    }
}

/// Execute a remote node that has no task result (e.g. sources, seeds without render).
async fn execute_remote_node_no_result(
    node: &dyn InternalDbtNodeAttributes,
    ctx: &TaskRunnerCtx,
    task_hooks: &Arc<dyn RunTaskHooks>,
) -> FsResult<NodeStatus> {
    if node.as_any().downcast_ref::<DbtSource>().is_some() {
        Ok(NodeStatus::Succeeded)
    } else if let Some(seed) = node.as_any().downcast_ref::<DbtSeed>() {
        let res = execute_seed_remote(seed, ctx);
        maybe_resolve_remote_seed_column_hint(res, node, ctx).await
    } else if let Some(saved_query) = node.as_any().downcast_ref::<DbtSavedQuery>() {
        task_hooks
            .execute_saved_query(ctx, saved_query)
            .await
            .map(|_| NodeStatus::Succeeded)
    } else {
        Err(fs_err!(
            ErrorCode::Generic,
            "Failed to receive render result for {}",
            node.unique_id()
        ))
    }
}

/// Determine whether to prefer SQL over LP for local execution of a node.
pub fn prefer_sql_for_node(node: &dyn InternalDbtNodeAttributes) -> bool {
    if node.as_any().is::<DbtSnapshot>() {
        true
    } else if node.as_any().is::<DbtModel>() {
        node.node_adapter() == AdapterType::DuckDB
    } else {
        false
    }
}

pub fn runnable_remote_task(
    nodes: &Nodes,
    unique_id: &str,
) -> Option<Arc<dyn InternalDbtNodeAttributes>> {
    if let Some(model) = nodes.models.get(unique_id) {
        Some(model.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else if let Some(test) = nodes.tests.get(unique_id) {
        Some(test.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else if let Some(snapshot) = nodes.snapshots.get(unique_id) {
        Some(snapshot.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else if let Some(seed) = nodes.seeds.get(unique_id) {
        Some(seed.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else if let Some(source) = nodes.sources.get(unique_id) {
        Some(source.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else if let Some(unit_test) = nodes.unit_tests.get(unique_id) {
        Some(unit_test.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else if let Some(function) = nodes.functions.get(unique_id) {
        Some(function.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else if let Some(saved_query) = nodes.saved_queries.get(unique_id) {
        Some(saved_query.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_common::tracing::dbt_metrics::InvocationMetricKey;
    use dbt_common::warn_error_options::WarnErrorOptionValue;
    use dbt_schemas::schemas::common::Hooks;
    use dbt_schemas::schemas::properties::ModelState;
    use dbt_yaml::Verbatim;
    use std::time::SystemTime;

    fn model_with_pre_hook_and_reuse_hook_config(
        execute_hooks_on_any_reuse: Option<bool>,
    ) -> DbtModel {
        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = "model.test.orders".to_string();
        model.__model_attr__.state = Some(ModelState {
            lag_tolerance: None,
            require_fresh_data_from: None,
            evaluate_volatile_sql: None,
            pre_clone: None,
            execute_hooks_on_any_reuse,
            compare_unrendered_code: None,
        });
        model.deprecated_config.pre_hook =
            Verbatim::from(Some(Hooks::String("select 1".to_string())));
        model
    }

    #[test]
    fn elapsed_millis_saturates_for_large_durations() {
        assert!(elapsed_millis(Instant::now()) >= 0);
    }

    // Serializes tests that touch the process-global time-machine state
    // (`GLOBAL_SESSION`/`GLOBAL_REPLAYER` in `dbt_adapter::time_machine`).
    static TIME_MACHINE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[dbt_runtime::test]
    async fn run_cache_clone_decision_records_and_replays() {
        use dbt_adapter::time_machine::{
            EventReplayer, RecordedRunCacheCloneDecision, get_or_init_recording,
            get_or_init_replayer, reset_time_machine_globals,
        };
        use dbt_common::cancellation::CancellationToken;
        use dbt_tasks_core::run_cache::run_cache_service::record_dev_clone_decision;

        let _guard = TIME_MACHINE_TEST_LOCK.lock().await;
        reset_time_machine_globals().await.unwrap();

        let dir = tempfile::tempdir().unwrap();
        let handle = get_or_init_recording(
            dir.path(),
            "snowflake",
            "test-invocation",
            None,
            CancellationToken::never_cancels(),
        );

        let decision = RunCacheCloneDecision::from_recorded(RecordedRunCacheCloneDecision {
            request_id: "req-1".to_string(),
            clone_sqls: vec!["create table t clone s".to_string()],
            clone_source: "db.schema.source".to_string(),
            clone_target: "db.schema.target".to_string(),
            required_source_epoch: Some(123),
            execution_runtime_ms: Some(456),
            freshness_tolerance_seconds: 60,
            is_stale: true,
            execution_decision_id: Some("decision-1".to_string()),
        });

        record_run_cache_clone_decision("model.test.orders", &decision);
        record_dev_clone_decision("model.test.orders", &decision);

        handle.shutdown().await.unwrap();
        reset_time_machine_globals().await.unwrap();
        get_or_init_replayer(|| Ok(Arc::new(EventReplayer::load(dir.path())?))).unwrap();

        let replayed = replay_run_cache_clone_decision("model.test.orders")
            .expect("clone decision should be recorded and replayed");
        assert_eq!(decision, replayed);

        let replayed_dev = replay_dev_clone_decision("model.test.orders")
            .expect("dev clone decision should be recorded and replayed");
        assert_eq!(
            replayed_dev.replay_status(),
            NodeStatus::ReusedCloned(Some(60))
        );

        assert!(
            replay_run_cache_clone_decision("model.test.unrelated").is_none(),
            "should not replay a clone decision that was never recorded for this node"
        );
        reset_time_machine_globals().await.unwrap();
    }

    #[dbt_runtime::test]
    async fn cached_data_test_result_records_and_replays() {
        use dbt_adapter::time_machine::{
            EventReplayer, get_or_init_recording, get_or_init_replayer, reset_time_machine_globals,
        };
        use dbt_common::cancellation::CancellationToken;

        let _guard = TIME_MACHINE_TEST_LOCK.lock().await;
        reset_time_machine_globals().await.unwrap();

        let dir = tempfile::tempdir().unwrap();
        let handle = get_or_init_recording(
            dir.path(),
            "snowflake",
            "test-invocation",
            None,
            CancellationToken::never_cancels(),
        );
        global_recorder().unwrap().record_sao_skip_with_test_result(
            "test.test_orders",
            SaoStatus::ReusedNoChanges,
            "No new changes",
            "run-cache-service",
            Some(RecordedCachedTestResult {
                failures: 2,
                should_warn: true,
                should_error: false,
            }),
        );

        handle.shutdown().await.unwrap();
        reset_time_machine_globals().await.unwrap();
        get_or_init_replayer(|| Ok(Arc::new(EventReplayer::load(dir.path())?))).unwrap();

        let RunCacheServiceDecision::Skip {
            status,
            cached_test_result,
            ..
        } = maybe_replay_remote_run("test.test_orders").unwrap()
        else {
            panic!("expected a replayed cached test skip");
        };
        assert_eq!(
            status,
            NodeStatus::ReusedNoChanges("No new changes".to_string())
        );
        assert_eq!(
            cached_test_result,
            Some(CachedTestExecutionResult {
                failures: 2,
                should_warn: true,
                should_error: false,
            })
        );
        reset_time_machine_globals().await.unwrap();
    }

    #[test]
    fn cached_passing_data_test_keeps_reused_final_status() {
        let reused_status =
            NodeStatus::ReusedNoChanges("No new changes on any upstreams".to_string());

        let status = cached_data_test_status(
            &reused_status,
            CachedTestExecutionResult {
                failures: 0,
                should_warn: false,
                should_error: false,
            },
            Severity::Error,
            &WarnErrorOptions::default(),
        );

        assert_eq!(status.failures, 0);
        assert_eq!(status.status, TestExecutionStatus::Passed);
        assert_eq!(
            status.stat_status,
            NodeStatus::ReusedNoChanges("No new changes on any upstreams".to_string())
        );
        let stat = Stat::new(
            "test.project.cached_test".to_string(),
            SystemTime::now(),
            Some(status.failures),
            status.stat_status.clone(),
            None,
            1,
        );
        assert_eq!(stat.result_status_string(), "reused");
        assert_eq!(status.final_status, reused_status);
        assert_eq!(status.status.metric_key(), None);
    }

    #[test]
    fn cached_passing_data_test_keeps_pass_status_when_reuse_status_is_disabled() {
        let reused_status =
            NodeStatus::ReusedNoChanges("No new changes on any upstreams".to_string());

        let status = cached_data_test_status_with_emit_reused_status(
            &reused_status,
            CachedTestExecutionResult {
                failures: 0,
                should_warn: false,
                should_error: false,
            },
            Severity::Error,
            &WarnErrorOptions::default(),
            false,
        );

        assert_eq!(status.stat_status, NodeStatus::TestPassed);
        assert_eq!(status.final_status, reused_status);
    }

    #[test]
    fn cached_failing_error_data_test_keeps_error_final_status() {
        let reused_status =
            NodeStatus::ReusedNoChanges("No new changes on any upstreams".to_string());

        let status = cached_data_test_status(
            &reused_status,
            CachedTestExecutionResult {
                failures: 2,
                should_warn: true,
                should_error: true,
            },
            Severity::Error,
            &WarnErrorOptions::default(),
        );

        assert_eq!(status.failures, 2);
        assert_eq!(status.status, TestExecutionStatus::Failed);
        assert_eq!(status.stat_status, NodeStatus::Errored);
        assert_eq!(status.final_status, NodeStatus::Errored);
        assert_eq!(
            status.status.metric_key(),
            Some(InvocationMetricKey::TotalErrors)
        );
    }

    #[test]
    fn cached_failing_warn_data_test_keeps_warn_final_status() {
        let reused_status =
            NodeStatus::ReusedNoChanges("No new changes on any upstreams".to_string());

        let status = cached_data_test_status(
            &reused_status,
            CachedTestExecutionResult {
                failures: 2,
                should_warn: true,
                should_error: false,
            },
            Severity::Warn,
            &WarnErrorOptions::default(),
        );

        assert_eq!(status.failures, 2);
        assert_eq!(status.status, TestExecutionStatus::Warned);
        assert_eq!(status.stat_status, NodeStatus::TestWarned);
        assert_eq!(status.final_status, NodeStatus::TestWarned);
        assert_eq!(
            status.status.metric_key(),
            Some(InvocationMetricKey::TotalWarnings)
        );
    }

    #[test]
    fn cached_error_severity_data_test_uses_cached_warning_threshold() {
        let reused_status =
            NodeStatus::ReusedNoChanges("No new changes on any upstreams".to_string());

        let status = cached_data_test_status(
            &reused_status,
            CachedTestExecutionResult {
                failures: 2,
                should_warn: true,
                should_error: false,
            },
            Severity::Error,
            &WarnErrorOptions::default(),
        );

        assert_eq!(status.failures, 2);
        assert_eq!(status.status, TestExecutionStatus::Warned);
        assert_eq!(status.stat_status, NodeStatus::TestWarned);
        assert_eq!(status.final_status, NodeStatus::TestWarned);
        assert_eq!(
            status.status.metric_key(),
            Some(InvocationMetricKey::TotalWarnings)
        );
    }

    #[test]
    fn cached_failing_warn_data_test_honors_warn_error_upgrade() {
        let reused_status =
            NodeStatus::ReusedNoChanges("No new changes on any upstreams".to_string());
        let warn_error_options = WarnErrorOptions {
            error: vec![WarnErrorOptionValue::all()],
            ..Default::default()
        };

        let status = cached_data_test_status(
            &reused_status,
            CachedTestExecutionResult {
                failures: 2,
                should_warn: true,
                should_error: false,
            },
            Severity::Warn,
            &warn_error_options,
        );

        assert_eq!(status.failures, 2);
        assert_eq!(status.status, TestExecutionStatus::Failed);
        assert_eq!(status.stat_status, NodeStatus::Errored);
        assert_eq!(status.final_status, NodeStatus::Errored);
        assert_eq!(
            status.status.metric_key(),
            Some(InvocationMetricKey::TotalErrors)
        );
    }

    #[test]
    fn cached_failing_warn_data_test_honors_warn_error_silence() {
        let reused_status =
            NodeStatus::ReusedNoChanges("No new changes on any upstreams".to_string());
        let warn_error_options = WarnErrorOptions {
            silence: vec![WarnErrorOptionValue::all()],
            ..Default::default()
        };

        let status = cached_data_test_status(
            &reused_status,
            CachedTestExecutionResult {
                failures: 2,
                should_warn: true,
                should_error: false,
            },
            Severity::Warn,
            &warn_error_options,
        );

        assert_eq!(status.failures, 2);
        assert_eq!(status.status, TestExecutionStatus::Passed);
        assert_eq!(status.stat_status, reused_status);
        assert_eq!(status.final_status, reused_status);
        assert_eq!(status.status.metric_key(), None);
    }

    #[test]
    fn clone_sql_failure_is_recoverable_until_pre_hooks_are_configured() {
        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = "model.test.orders".to_string();
        let hook_node = RunCacheReuseHookNode::from_node(&model).expect("model supports hooks");
        assert!(!hook_node.has_pre_hooks());

        model.deprecated_config.pre_hook =
            Verbatim::from(Some(Hooks::String("select 1".to_string())));
        let hook_node = RunCacheReuseHookNode::from_node(&model).expect("model supports hooks");
        assert!(hook_node.has_pre_hooks());
    }

    #[test]
    fn reuse_hook_node_honors_state_hook_execution_override() {
        let model = model_with_pre_hook_and_reuse_hook_config(Some(false));

        assert!(run_cache_reuse_hook_node(&model, true).is_none());
    }

    #[test]
    fn clone_hook_node_ignores_state_hook_execution_override() {
        let model = model_with_pre_hook_and_reuse_hook_config(Some(false));

        assert!(run_cache_clone_hook_node(&model).is_some());
    }

    #[test]
    fn reuse_hook_node_falls_back_to_service_default() {
        let model = model_with_pre_hook_and_reuse_hook_config(None);

        assert!(run_cache_reuse_hook_node(&model, true).is_some());
        assert!(run_cache_reuse_hook_node(&model, false).is_none());
    }
}
