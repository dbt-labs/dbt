use std::sync::Arc;

use crate::materialize::{
    materialize_latest_version_pointer, materialize_microbatch_model, materialize_model,
    should_create_latest_version_pointer,
};
use crate::microbatch::{BatchContext, MicrobatchBuilder};
use crate::runnable::cache::cache_materialization_return_value;
use crate::runnable::microbatch::{build_event_time_mapping, is_incremental};
use chrono::{DateTime, Utc};
use dbt_adapter_core::AdapterType;
use dbt_common::FsResult;
use dbt_common::stats::NodeStatus;
use dbt_common::tracing::dbt_emit::emit_error_log_from_fs_error;
use dbt_common::tracing::span_info::{
    find_and_record_span_status_from_attrs, find_and_update_span_attrs,
};
use dbt_common::{ErrorCode, create_info_span, fs_err};
use dbt_jinja_utils::phases::run::{build_run_node_context_from_arc, reset_result_store};
use dbt_schemas::filter::RunFilter;
use dbt_schemas::schemas::common::{DbtBatchSize, DbtIncrementalStrategy, DbtMaterialization};
use dbt_schemas::schemas::{BatchResults, DbtModel};
use dbt_schemas::schemas::{InternalDbtNode, InternalDbtNodeAttributes};
use dbt_tasks_core::context::{TaskRunnerCtx, record_microbatch_metrics};
use dbt_telemetry::{
    ExecutionPhase, MicrobatchBatchSize, MicrobatchEvaluated, NodeEvaluated, NodeEvent,
    NodeOutcome, has_node_warning,
};

use minijinja::Value;
use minijinja::value::OverlayMap;
use tracing::debug;

use dbt_tasks_core::task::TaskResult;

/// Check if a model uses the microbatch incremental strategy.
pub fn try_get_microbatch_model(node: &dyn InternalDbtNodeAttributes) -> Option<&DbtModel> {
    if let Some(model) = node.as_any().downcast_ref::<DbtModel>()
        && model.materialized() == DbtMaterialization::Incremental
        && model.__model_attr__.incremental_strategy == Some(DbtIncrementalStrategy::Microbatch)
    {
        Some(model)
    } else {
        None
    }
}

/// A self-contained microbatch unit, ready to be executed.
#[derive(Clone)]
pub struct MicrobatchExecUnit {
    pub batch_ctx: BatchContext,
    pub raw_sql: Arc<String>,
    pub node: Arc<dyn InternalDbtNodeAttributes>,
    pub run_node_context: Arc<OverlayMap>,
    pub event_time_mapping: Arc<std::collections::BTreeMap<String, String>>,
    pub is_incremental: bool,
}

/// Cap on concurrent batches per model, independent of `--threads`. Keeps us
/// under Snowflake's non-configurable 20-statement lock-waiter limit
/// (dbt-core#15987).
const SNOWFLAKE_MAX_CONCURRENT_MICROBATCH_BATCHES: usize = 16;

/// Per-adapter cap on concurrent batches per model, if any.
fn max_concurrent_microbatch_batches(adapter_type: AdapterType) -> Option<usize> {
    match adapter_type {
        AdapterType::Snowflake => Some(SNOWFLAKE_MAX_CONCURRENT_MICROBATCH_BATCHES),
        _ => None,
    }
}

/// Prepare microbatch batches for execution.
///
/// Returns concurrency groups: `[[first], [middle...], [last]]`.
/// Groups must be executed sequentially; batches within a group can run in parallel.
/// TODO(chasewalden): The "default" behavior is auto-detected on use of `{{ this }}` in the model
/// - `{{ this }}` used -> default to `false`
/// - `{{ this }}` not used -> default to `true`
///   See: https://docs.getdbt.com/docs/build/parallel-batch-execution#how-parallel-batch-execution-works
///   The actual logic from dbt-core: https://github.com/dbt-labs/dbt-core/blob/2857d45cc02c0b01cb018ec26be1a9079e1634a0/core/dbt/task/run.py#L413-L427
///
/// `retry_windows` is the previous run's failed batches. When set, those windows
/// are what runs, and every batch is incremental so a retry cannot replace the table.
pub async fn prepare_microbatch_batches(
    node: Arc<dyn InternalDbtNodeAttributes>,
    ctx: &TaskRunnerCtx,
    task_result: &TaskResult,
    retry_windows: Option<&[(String, String)]>,
) -> FsResult<Vec<Vec<MicrobatchExecUnit>>> {
    let base_context = ctx.run_base_context(node.as_ref())?;

    let sql_header = task_result
        .config_map
        .read_sync("sql_header", |_, v| v.clone());

    let model = node
        .as_any()
        .downcast_ref::<DbtModel>()
        .expect("MicrobatchTask node must be a DbtModel");
    let unique_id = &model.__common_attr__.unique_id;

    let raw_sql = Arc::new(model.__common_attr__.raw_code.clone().ok_or_else(|| {
        fs_err!(
            ErrorCode::InvalidConfig,
            "Microbatch model {} has no raw_code populated. Raw code is required for batch-aware ref filtering.",
            unique_id,
        )
    })?);

    // A partial-success retry runs the saved failed windows, not the lookback
    // window. Lookback would skip a failed day that is older than `lookback`.
    let (batches, is_incremental) = if let Some(windows) = retry_windows {
        let batch_builder = MicrobatchBuilder::from_config(
            model.deprecated_config.batch_size.clone(),
            model.deprecated_config.begin,
            model.deprecated_config.lookback,
        )?;
        (batches_from_windows(&batch_builder, windows)?, true)
    } else {
        let (batch_builder, start_time, end_time, is_incremental) =
            resolve_batch_window(model, ctx).await?;
        (
            batch_builder.build_batches(start_time, end_time),
            is_incremental,
        )
    };

    if batches.is_empty() {
        debug!("Microbatch model {unique_id} has no batches to process");
        return Ok(vec![]);
    }

    debug!(
        "Executing microbatch model {} with {} batches",
        unique_id,
        batches.len(),
    );

    let event_time_mapping = Arc::new(build_event_time_mapping(model, ctx.nodes()));

    let run_node_context = Arc::new(
        build_run_node_context_from_arc(
            node.clone(),
            &model.deprecated_config,
            model.node_adapter(),
            None,
            &base_context,
            &ctx.inner.arg.io,
            ExecutionPhase::Run,
            sql_header,
            ctx.runtime_config().dependencies.keys().cloned().collect(),
        )
        .0,
    );

    // Group batches: [[first], [mid1, mid2, ...], [last]]
    let concurrent = model.deprecated_config.concurrent_batches.unwrap_or(false);
    let groups: Vec<Vec<MicrobatchExecUnit>> = batches
        .chunk_by(|l, r| !l.is_first() && !r.is_last() && concurrent)
        .map(|group| {
            group
                .iter()
                .map(|batch| MicrobatchExecUnit {
                    batch_ctx: batch.clone(),
                    raw_sql: raw_sql.clone(),
                    node: node.clone(),
                    run_node_context: run_node_context.clone(),
                    event_time_mapping: event_time_mapping.clone(),
                    is_incremental,
                })
                .collect()
        })
        .collect();

    let groups = match max_concurrent_microbatch_batches(model.node_adapter()) {
        Some(max_size) => cap_group_sizes(groups, max_size),
        None => groups,
    };
    Ok(groups)
}

/// Split any group larger than `max_size` into consecutive sub-groups of at
/// most `max_size` elements each, preserving order.
fn cap_group_sizes<T: Clone>(groups: Vec<Vec<T>>, max_size: usize) -> Vec<Vec<T>> {
    groups
        .into_iter()
        .flat_map(|group| {
            group
                .chunks(max_size)
                .map(<[T]>::to_vec)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Resolve the run-level event-time window `(start, end)` for a microbatch model.
///
/// This is the same window `prepare_microbatch_batches` uses to compute the batch
/// set: `end` comes from `--event-time-end` (else `now`), and `start` from
/// `--event-time-start`, or — for an incremental run — by offsetting back from
/// `end` by the model's `lookback` batches (bounded below by `begin`), all keyed
/// off the model's `batch_size`. `--sample`, if passed, further clamps the window
/// to its bounds. The run cache folds it into the model-level cache key so
/// re-running an unchanged window is a whole-model no-op while a different
/// window executes. Mirrors the dbt-core plugin's `_resolve_microbatch_window`.
pub async fn resolve_microbatch_window(
    model: &DbtModel,
    ctx: &TaskRunnerCtx,
) -> FsResult<(DateTime<Utc>, DateTime<Utc>)> {
    let (_, start_time, end_time, _) = resolve_batch_window(model, ctx).await?;
    Ok((start_time, end_time))
}

/// Compute the `(start, end)` batch window for a microbatch model, clamped to
/// `--sample`'s bounds if passed. Shared by `prepare_microbatch_batches` and
/// `resolve_microbatch_window` so the two stay in sync by construction.
async fn resolve_batch_window(
    model: &DbtModel,
    ctx: &TaskRunnerCtx,
) -> FsResult<(MicrobatchBuilder, DateTime<Utc>, DateTime<Utc>, bool)> {
    let batch_builder = MicrobatchBuilder::from_config(
        model.deprecated_config.batch_size.clone(),
        model.deprecated_config.begin.as_deref(),
        model.deprecated_config.lookback,
    )?;

    // Model-level `full_refresh` config overrides the CLI `--full-refresh` flag.
    // Matches dbt-core's `_is_incremental` function
    // https://github.com/dbt-labs/dbt-core/blob/ecefc59e660eae4b34194ee4150fd4302836f4ea/core/dbt/task/run.py#L745-L746
    let full_refresh = model
        .deprecated_config
        .full_refresh
        .unwrap_or(ctx.inner.arg.full_refresh);
    let is_incremental = is_incremental(
        model,
        full_refresh,
        ctx.adapter_for(model)?,
        ctx.env.clone(),
    )
    .await;

    let end_time = batch_builder.build_end_time(ctx.inner.arg.event_time_end.clone())?;
    let start_time = batch_builder.build_start_time(
        Some(end_time),
        ctx.inner.arg.event_time_start.clone(),
        is_incremental,
    )?;

    // `--sample` bounds the batch window the same way it bounds the SQL-level
    // event-time filter for non-microbatch refs (see `RunFilter::sample_times`).
    let sample = RunFilter::try_from(false, ctx.inner.arg.sample.clone())?;
    let (start_time, end_time) = clamp_to_sample(&batch_builder, start_time, end_time, &sample);

    Ok((batch_builder, start_time, end_time, is_incremental))
}

/// Clamp a batch window to `sample`'s bounds, if any are set. `sample_start` floors
/// to the batch boundary it falls in (inclusive); `sample_end` ceils to the next
/// boundary (exclusive), mirroring `build_start_time`/`build_end_time`'s own rounding.
fn clamp_to_sample(
    batch_builder: &MicrobatchBuilder,
    start_time: DateTime<Utc>,
    end_time: DateTime<Utc>,
    sample: &RunFilter,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let (sample_start, sample_end) = sample.sample_times();

    let start_time = match sample_start {
        Some(sample_start) => start_time.max(batch_builder.truncate_timestamp(sample_start)),
        None => start_time,
    };
    let end_time = match sample_end {
        Some(sample_end) => end_time.min(batch_builder.ceiling_timestamp(sample_end)),
        None => end_time,
    };

    (start_time, end_time)
}

/// `(event_time_start, event_time_end)` pair stored in `run_results.json` `batch_results`.
pub fn batch_window(batch: &BatchContext) -> (String, String) {
    let stamp = |ts: DateTime<Utc>| ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    (stamp(batch.event_time_start), stamp(batch.event_time_end))
}

/// Sort batch windows the way dbt-core does before writing `batch_results`.
pub fn microbatch_batch_results(
    mut successful: Vec<(String, String)>,
    mut failed: Vec<(String, String)>,
) -> BatchResults {
    successful.sort();
    failed.sort();
    BatchResults { successful, failed }
}

/// Failed windows from a partial-success run.
///
/// `None` when the last run had no successes: retry then rebuilds the normal
/// window, so a full refresh can still start over from `begin`.
pub fn retry_failed_windows(prior: &BatchResults) -> Option<&[(String, String)]> {
    if prior.successful.is_empty() || prior.failed.is_empty() {
        None
    } else {
        Some(prior.failed.as_slice())
    }
}

/// Rebuild batch contexts from `run_results.json` windows.
pub fn batches_from_windows(
    builder: &MicrobatchBuilder,
    windows: &[(String, String)],
) -> FsResult<Vec<BatchContext>> {
    let mut parsed = Vec::with_capacity(windows.len());
    for (start, end) in windows {
        let start = DateTime::parse_from_rfc3339(start)
            .map_err(|error| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Unable to parse batch start '{start}': {error}"
                )
            })?
            .with_timezone(&Utc);
        let end = DateTime::parse_from_rfc3339(end)
            .map_err(|error| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Unable to parse batch end '{end}': {error}"
                )
            })?
            .with_timezone(&Utc);
        parsed.push((start, end));
    }
    parsed.sort_by_key(|(start, _)| *start);
    let total = parsed.len();
    Ok(parsed
        .into_iter()
        .enumerate()
        .map(|(index, (start, end))| {
            BatchContext::new(builder.batch_id(start), start, end, index, total)
        })
        .collect())
}

/// This run's windows, plus successes carried from the partial run being retried.
pub fn finalize_microbatch_results(
    successful: Vec<(String, String)>,
    failed: Vec<(String, String)>,
    carried_successes: Option<&[(String, String)]>,
) -> BatchResults {
    let mut results = microbatch_batch_results(successful, failed);
    if let Some(carried) = carried_successes {
        results.successful.extend(carried.iter().cloned());
        results.successful.sort();
        results.successful.dedup();
    }
    results
}

/// Run-results message and status for a finished microbatch model.
///
/// A mix of outcomes is `partial success`. The node stays [`NodeStatus::Errored`]
/// so downstreams skip; `None` leaves the status as `success` or `error`.
pub fn microbatch_result_annotation(
    batch_results: &BatchResults,
) -> (Option<String>, Option<String>) {
    let succeeded = batch_results.successful.len();
    let failed = batch_results.failed.len();
    if succeeded > 0 && failed > 0 {
        let total = succeeded + failed;
        (
            Some(format!("PARTIAL SUCCESS ({succeeded}/{total})")),
            Some("partial success".to_string()),
        )
    } else {
        (None, None)
    }
}

/// Outcome of one microbatch. A materialization failure is [`Self::Failed`], not
/// an `Err`, so later batches still run.
pub enum MicrobatchBatchOutcome {
    Succeeded((String, String)),
    Failed {
        window: (String, String),
        error: Box<dbt_common::FsError>,
    },
    /// The batch did not run because the invocation is stopping.
    /// Kept as `Ok` so a sibling that already finished is not dropped.
    Canceled((String, String)),
}

/// Create the `MicrobatchEvaluated` span of one batch under the current span.
pub fn create_microbatch_span(mb_unit: &MicrobatchExecUnit) -> tracing::Span {
    let model = mb_unit
        .node
        .as_any()
        .downcast_ref::<DbtModel>()
        .expect("MicrobatchTask node must be a DbtModel");
    let batch_size = match model.deprecated_config.batch_size {
        Some(DbtBatchSize::Hour) => MicrobatchBatchSize::Hour,
        Some(DbtBatchSize::Day) => MicrobatchBatchSize::Day,
        Some(DbtBatchSize::Month) => MicrobatchBatchSize::Month,
        Some(DbtBatchSize::Year) => MicrobatchBatchSize::Year,
        None => MicrobatchBatchSize::Unspecified,
    };
    let batch = &mb_unit.batch_ctx;
    create_info_span(MicrobatchEvaluated::start(
        mb_unit.node.unique_id(),
        mb_unit.node.schema(),
        mb_unit.node.alias(),
        batch.id.clone(),
        batch_size,
        batch.batch_index as u32 + 1,
        batch.total_batches as u32,
        batch
            .event_time_start
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        batch
            .event_time_end
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    ))
}

/// Execute a single microbatch task within its `MicrobatchEvaluated` span and
/// record its outcome there.
///
/// A materialization failure is returned as [`MicrobatchBatchOutcome::Failed`]
/// so the rest of the batches still run. `canceled` is set only when the run
/// itself is stopping; that batch is skipped.
pub fn execute_microbatch_batch(
    mb_unit: MicrobatchExecUnit,
    ctx: &TaskRunnerCtx,
    canceled: bool,
) -> FsResult<MicrobatchBatchOutcome> {
    let window = batch_window(&mb_unit.batch_ctx);
    let result = if canceled {
        Err(fs_err!(
            ErrorCode::OperationCanceled,
            "Batch {} canceled",
            mb_unit.batch_ctx.id
        ))
    } else {
        materialize_microbatch_batch(mb_unit, ctx)
    };

    // Runs within the batch span, which the worker entered on submission.
    find_and_record_span_status_from_attrs(|batch: &mut MicrobatchEvaluated| {
        batch.set_node_outcome(match &result {
            Ok(()) => NodeOutcome::Success,
            Err(error)
                if matches!(
                    error.code,
                    ErrorCode::OperationCanceled | ErrorCode::TaskCancelled
                ) =>
            {
                NodeOutcome::Canceled
            }
            Err(_) => NodeOutcome::Error,
        });
        batch.dbt_core_event_code = Some("Q046".to_string());
    });

    match result {
        Ok(()) => Ok(MicrobatchBatchOutcome::Succeeded(window)),
        Err(error)
            if matches!(
                error.code,
                ErrorCode::OperationCanceled | ErrorCode::TaskCancelled
            ) =>
        {
            Ok(MicrobatchBatchOutcome::Canceled(window))
        }
        Err(error) => {
            // Logged here, inside the batch span. The model result records the
            // window and must not log this error a second time.
            emit_error_log_from_fs_error(error.clone_without_backtrace());
            Ok(MicrobatchBatchOutcome::Failed { window, error })
        }
    }
}

fn materialize_microbatch_batch(mb_unit: MicrobatchExecUnit, ctx: &TaskRunnerCtx) -> FsResult<()> {
    let model = mb_unit
        .node
        .as_any()
        .downcast_ref::<DbtModel>()
        .expect("MicrobatchTask node must be a DbtModel");

    // Inject incremental override flags per batch
    let mut ctx_for_batch = (*mb_unit.run_node_context).clone();

    // Give each batch its own statement-result registry. The shared
    // run_node_context bakes in a single ResultStore whose store/load closures
    // are cloned (Arc) into every batch; with concurrent_batches the batches
    // interleave store/load for the same statement name (e.g.
    // get_columns_in_relation) and collide with MacroResultAlreadyLoadedError.
    let result_store = reset_result_store(&mut ctx_for_batch);

    if !mb_unit.batch_ctx.is_first() || mb_unit.is_incremental {
        ctx_for_batch.insert(
            "is_incremental".to_string(),
            Value::from_function(|_args: &[Value]| Ok(Value::from(true))),
        );
        ctx_for_batch.insert(
            "should_full_refresh".to_string(),
            Value::from_function(|_args: &[Value]| Ok(Value::from(false))),
        );
    }

    let adapter = ctx.adapter_for(model)?;
    match materialize_microbatch_model(
        &mb_unit.raw_sql,
        model,
        ctx.node_resolver(),
        ctx.runtime_config(),
        &ctx.inner.materialization_resolver,
        ctx.env.clone(),
        &mb_unit.batch_ctx,
        &adapter,
        ctx_for_batch,
        mb_unit.event_time_mapping,
        &ctx.inner.arg.io,
    ) {
        Ok(relations_map) => {
            let main_response = result_store.main_adapter_response();
            record_microbatch_metrics(
                &ctx.inner.microbatch_execution_metrics,
                &model.__common_attr__.unique_id,
                main_response.as_ref(),
            );
            if let Some(main_response) = main_response {
                ctx.inner
                    .main_adapter_responses
                    .upsert_sync(model.__common_attr__.unique_id.clone(), main_response);
            }
            let _ = cache_materialization_return_value(&adapter, ctx.env.clone(), &relations_map);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub fn execute_model_remote(
    model: &DbtModel,
    ctx: &TaskRunnerCtx,
    task_result: &TaskResult,
) -> FsResult<NodeStatus> {
    let base_context = ctx.run_base_context(model)?;

    // return early if the node is a ephemeral or inline model since we don't need to execute it.
    if model.materialized() == DbtMaterialization::Ephemeral
        || model.materialized() == DbtMaterialization::Inline
    {
        return Ok(NodeStatus::NoOp);
    }

    let sql_header = task_result
        .config_map
        .read_sync("sql_header", |_, v| v.clone());

    let adapter = ctx.adapter_for(model)?;
    // Traditional warehouse execution via Jinja materialization macros
    match materialize_model(
        &task_result.sql_instruction.sql,
        ctx.nodes()
            .get_node_owned(&model.common().unique_id)
            .expect("model must be present in the node graph"),
        &adapter,
        ctx.runtime_config(),
        &ctx.inner.materialization_resolver,
        ctx.env.clone(),
        &base_context,
        &ctx.inner.arg.io,
        sql_header,
    ) {
        Ok((relations_map, main_response)) => {
            if let Some(main_response) = main_response {
                ctx.inner
                    .main_adapter_responses
                    .upsert_sync(model.__common_attr__.unique_id.clone(), main_response);
            }
            let _ = cache_materialization_return_value(&adapter, ctx.env.clone(), &relations_map);
        }
        Err(e) => {
            return Err(e);
        }
    }

    // After successful materialization, create the latest version pointer view if applicable
    if should_create_latest_version_pointer(model, ctx.runtime_config()) {
        let relations_map = materialize_latest_version_pointer(
            model,
            model.node_adapter(),
            ctx.runtime_config(),
            &ctx.inner.materialization_resolver,
            ctx.env.clone(),
            &base_context,
            &ctx.inner.arg.io,
        )?;
        let _ = cache_materialization_return_value(&adapter, ctx.env.clone(), &relations_map);
    }

    let mut had_warning = false;
    find_and_update_span_attrs(|attrs: &mut NodeEvaluated| {
        had_warning = has_node_warning(NodeEvent::Evaluated(attrs));
    });
    if had_warning {
        Ok(NodeStatus::SucceededWithWarning)
    } else {
        Ok(NodeStatus::Succeeded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use dbt_schemas::filter::Sample;
    use dbt_schemas::schemas::common::DbtBatchSize;

    fn dt(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        DateTime::<Utc>::from_naive_utc_and_offset(
            NaiveDate::from_ymd_opt(year, month, day)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
            Utc,
        )
    }

    fn sample(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> RunFilter {
        RunFilter {
            empty: false,
            sample: Some(Sample { start, end }),
        }
    }

    #[test]
    fn no_sample_leaves_window_untouched() {
        let builder = MicrobatchBuilder::new(DbtBatchSize::Day, dt(2024, 1, 1), 1);
        let (start, end) = clamp_to_sample(
            &builder,
            dt(2024, 8, 4),
            dt(2026, 8, 5),
            &RunFilter::default(),
        );
        assert_eq!(start, dt(2024, 8, 4));
        assert_eq!(end, dt(2026, 8, 5));
    }

    #[test]
    fn sample_narrows_a_wide_begin_to_now_window() {
        // Regression for #15798: a `begin`-to-now window spanning ~2 years must be
        // narrowed to roughly the `--sample` bound, not built in full.
        let builder = MicrobatchBuilder::new(DbtBatchSize::Day, dt(2024, 8, 4), 1);
        let filter = sample(Some(dt(2026, 7, 6)), Some(dt(2026, 8, 5)));

        let (start, end) = clamp_to_sample(&builder, dt(2024, 8, 4), dt(2026, 8, 5), &filter);

        assert_eq!(start, dt(2026, 7, 6));
        assert_eq!(end, dt(2026, 8, 5));
    }

    #[test]
    fn sample_bound_outside_window_has_no_effect() {
        // A sample window wider than the computed window must not widen it back out.
        let builder = MicrobatchBuilder::new(DbtBatchSize::Day, dt(2024, 8, 4), 1);
        let filter = sample(Some(dt(2020, 1, 1)), Some(dt(2030, 1, 1)));

        let (start, end) = clamp_to_sample(&builder, dt(2026, 7, 1), dt(2026, 8, 1), &filter);

        assert_eq!(start, dt(2026, 7, 1));
        assert_eq!(end, dt(2026, 8, 1));
    }

    #[test]
    fn sample_end_ceils_to_next_batch_boundary() {
        // A sample end mid-batch must still include that batch, matching
        // `build_end_time`'s own ceiling of `now()`.
        let builder = MicrobatchBuilder::new(DbtBatchSize::Day, dt(2024, 1, 1), 1);
        let mid_batch_end = dt(2026, 7, 15) + chrono::Duration::hours(6);
        let filter = sample(None, Some(mid_batch_end));

        let (_, end) = clamp_to_sample(&builder, dt(2024, 1, 1), dt(2026, 8, 5), &filter);

        assert_eq!(end, dt(2026, 7, 16));
    }

    #[test]
    fn cap_group_sizes_splits_oversized_groups() {
        let groups = vec![vec![1], vec![2, 3, 4, 5, 6, 7], vec![8]];
        assert_eq!(
            cap_group_sizes(groups, 3),
            vec![vec![1], vec![2, 3, 4], vec![5, 6, 7], vec![8]]
        );
    }

    #[test]
    fn cap_group_sizes_leaves_small_groups_untouched() {
        let groups = vec![vec![1], vec![2, 3], vec![4]];
        assert_eq!(cap_group_sizes(groups.clone(), 16), groups);
    }

    #[test]
    fn batch_window_is_the_inclusive_start_and_exclusive_end() {
        let batch = BatchContext::new("20200102".to_string(), dt(2020, 1, 2), dt(2020, 1, 3), 1, 3);
        assert_eq!(
            batch_window(&batch),
            (
                "2020-01-02T00:00:00Z".to_string(),
                "2020-01-03T00:00:00Z".to_string(),
            )
        );
    }

    #[test]
    fn mixed_batch_results_are_partial_success() {
        let results = microbatch_batch_results(
            vec![(
                "2020-01-03T00:00:00Z".to_string(),
                "2020-01-04T00:00:00Z".to_string(),
            )],
            vec![
                (
                    "2020-01-02T00:00:00Z".to_string(),
                    "2020-01-03T00:00:00Z".to_string(),
                ),
                (
                    "2020-01-01T00:00:00Z".to_string(),
                    "2020-01-02T00:00:00Z".to_string(),
                ),
            ],
        );
        assert_eq!(results.failed[0].0, "2020-01-01T00:00:00Z");
        assert_eq!(
            microbatch_result_annotation(&results),
            (
                Some("PARTIAL SUCCESS (1/3)".to_string()),
                Some("partial success".to_string()),
            )
        );
    }

    #[test]
    fn every_batch_failing_stays_an_error() {
        let results = microbatch_batch_results(
            vec![],
            vec![(
                "2020-01-01T00:00:00Z".to_string(),
                "2020-01-02T00:00:00Z".to_string(),
            )],
        );
        assert_eq!(microbatch_result_annotation(&results), (None, None));
    }

    #[test]
    fn retry_reruns_only_the_failed_window_and_keeps_older_successes() {
        let prior = BatchResults {
            successful: vec![
                (
                    "2024-01-01T00:00:00Z".to_string(),
                    "2024-01-02T00:00:00Z".to_string(),
                ),
                (
                    "2024-01-03T00:00:00Z".to_string(),
                    "2024-01-04T00:00:00Z".to_string(),
                ),
            ],
            failed: vec![(
                "2024-01-02T00:00:00Z".to_string(),
                "2024-01-03T00:00:00Z".to_string(),
            )],
        };
        let windows = retry_failed_windows(&prior).unwrap();
        let builder = MicrobatchBuilder::new(DbtBatchSize::Day, dt(2024, 1, 1), 1);
        let batches = batches_from_windows(&builder, windows).unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].id, "20240102");
        assert!(batches[0].is_first());

        // The retried window succeeds. The January 1 and 3 successes stay in the artifact.
        let results = finalize_microbatch_results(
            vec![(
                "2024-01-02T00:00:00Z".to_string(),
                "2024-01-03T00:00:00Z".to_string(),
            )],
            vec![],
            Some(&prior.successful),
        );
        assert!(results.failed.is_empty());
        assert_eq!(results.successful.len(), 3);
        assert_eq!(results.successful[0].0, "2024-01-01T00:00:00Z");
    }

    #[test]
    fn every_batch_failing_does_not_narrow_the_retry() {
        let prior = BatchResults {
            successful: vec![],
            failed: vec![(
                "2024-01-02T00:00:00Z".to_string(),
                "2024-01-03T00:00:00Z".to_string(),
            )],
        };
        assert!(retry_failed_windows(&prior).is_none());
    }

    #[test]
    fn only_snowflake_caps_concurrent_batches() {
        assert_eq!(
            max_concurrent_microbatch_batches(AdapterType::Snowflake),
            Some(SNOWFLAKE_MAX_CONCURRENT_MICROBATCH_BATCHES)
        );
        assert_eq!(
            max_concurrent_microbatch_batches(AdapterType::Redshift),
            None
        );
    }
}
