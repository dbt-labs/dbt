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
use dbt_common::tracing::span_info::find_and_update_span_attrs;
use dbt_common::{ErrorCode, fs_err};
use dbt_jinja_utils::phases::run::{build_run_node_context, reset_result_store};
use dbt_jinja_utils::utils::add_task_context;
use dbt_schemas::filter::RunFilter;
use dbt_schemas::schemas::DbtModel;
use dbt_schemas::schemas::common::{DbtIncrementalStrategy, DbtMaterialization};
use dbt_schemas::schemas::{InternalDbtNode, InternalDbtNodeAttributes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_telemetry::{ExecutionPhase, NodeEvaluated, NodeEvent, has_node_warning};

use minijinja::Value;
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
    pub run_node_context: Arc<std::collections::BTreeMap<String, Value>>,
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
/// TODO(chasewalden): `dbt retry` can be used to re-process only the failed batches.
///  Seems like the `retry` subcommand doesn't exist in `fs` though...
pub async fn prepare_microbatch_batches(
    node: Arc<dyn InternalDbtNodeAttributes>,
    ctx: &TaskRunnerCtx,
    task_result: &TaskResult,
) -> FsResult<Vec<Vec<MicrobatchExecUnit>>> {
    let mut base_context = ctx.inner.base_context.clone();

    add_task_context(&mut base_context, node.common(), &ctx.thread_id);

    let sql_header = task_result
        .config_map
        .get("sql_header")
        .map(|v| v.value().clone());

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

    let (batch_builder, start_time, end_time, is_incremental) =
        resolve_batch_window(model, ctx).await?;
    let batches = batch_builder.build_batches(start_time, end_time);

    if batches.is_empty() {
        debug!(
            "Microbatch model {} has no batches to process (start={}, end={})",
            unique_id, start_time, end_time
        );
        return Ok(vec![]);
    }

    debug!(
        "Executing microbatch model {} with {} batches (start={}, end={})",
        unique_id,
        batches.len(),
        start_time,
        end_time
    );

    let event_time_mapping = Arc::new(build_event_time_mapping(model, ctx.nodes()));

    let run_node_context = Arc::new(
        build_run_node_context(
            model,
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
    let is_incremental =
        is_incremental(model, full_refresh, model.node_adapter(), ctx.env.clone()).await;

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

/// Execute a single microbatch task.
pub fn execute_microbatch_batch(mb_unit: MicrobatchExecUnit, ctx: &TaskRunnerCtx) -> FsResult<()> {
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

    match materialize_microbatch_model(
        &mb_unit.raw_sql,
        model,
        ctx.node_resolver(),
        ctx.runtime_config(),
        &ctx.inner.materialization_resolver,
        ctx.env.clone(),
        &mb_unit.batch_ctx,
        model.node_adapter(),
        ctx_for_batch,
        mb_unit.event_time_mapping,
        &ctx.inner.arg.io,
    ) {
        Ok(relations_map) => {
            if let Some(main_response) = result_store.main_adapter_response() {
                ctx.inner
                    .main_adapter_responses
                    .insert(model.__common_attr__.unique_id.clone(), main_response);
            }
            let _ = cache_materialization_return_value(ctx.env.clone(), &relations_map);
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
    let mut base_context = ctx.inner.base_context.clone();

    add_task_context(&mut base_context, model.common(), &ctx.thread_id);

    // return early if the node is a ephemeral or inline model since we don't need to execute it.
    if model.materialized() == DbtMaterialization::Ephemeral
        || model.materialized() == DbtMaterialization::Inline
    {
        return Ok(NodeStatus::NoOp);
    }

    let sql_header = task_result
        .config_map
        .get("sql_header")
        .map(|v| v.value().clone());

    // Traditional warehouse execution via Jinja materialization macros
    match materialize_model(
        &task_result.sql_instruction.sql,
        model,
        model.node_adapter(),
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
                    .insert(model.__common_attr__.unique_id.clone(), main_response);
            }
            let _ = cache_materialization_return_value(ctx.env.clone(), &relations_map);
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
        let _ = cache_materialization_return_value(ctx.env.clone(), &relations_map);
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
