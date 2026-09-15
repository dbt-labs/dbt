use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::Adapter;
use dbt_common::FsResult;
use dbt_common::io_args::{EvalArgs, Phases};
use dbt_common::io_utils::checkpoint_maybe_exit;
use dbt_dag::schedule::Schedule;
use dbt_freshness::freshness::{
    FreshnessTimestamps, build_freshness_artifact, build_sources_artifact, emit_freshness_stats,
    freshness_results_to_context, run_freshness, should_write_sources_json, write_freshness_json,
    write_freshness_results_parquet, write_sources_json,
};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schemas::schemas::DbtCommandExecutionArtifacts;
use dbt_schemas::state::ResolverState;
use dbt_tasks_core::PreTaskRunData;
use dbt_tasks_sa::run_operation::run_operation_on_run;
use minijinja::Value;

/// Runs the freshness command to completion: on-run-start operations, the
/// freshness checks themselves, stats output, artifact writes (`sources.json` when
/// sources were selected, `freshness.json` unless `sources_only`, and with
/// `--write-metadata` the parquet layer), on-run-end operations with the results in
/// context, and the freshness-phase checkpoint.
///
/// The command has no task graph, so none of this can be left to the task
/// runner; it all happens before the runner starts.
///
/// Both spellings set `phase` to [`Phases::Freshness`], so the
/// checkpoint ends the invocation and the returned value is rarely observed.
/// `artifacts_sink` is how the artifact reaches library callers instead: it is filled
/// in before the checkpoint, so it survives that early exit.
///
/// `sources_only` marks the nested `dbt source freshness` spelling.
pub async fn run_freshness_command(
    arg: &EvalArgs,
    jinja_env: &Arc<JinjaEnv>,
    resolved_state: &ResolverState,
    schedule: &Schedule<String>,
    adapter: Arc<Adapter>,
    base_context: &BTreeMap<String, Value>,
    artifacts_sink: &mut DbtCommandExecutionArtifacts,
    sources_only: bool,
) -> FsResult<Option<Box<dyn PreTaskRunData>>> {
    let empty_results: Vec<()> = vec![];
    for operation in resolved_state.operations.on_run_start.iter() {
        run_operation_on_run(
            operation,
            &arg.io,
            &None,
            &None,
            None,
            jinja_env,
            base_context,
            Value::from_serialize(&empty_results),
        )
        .await?;
    }

    let results = run_freshness(
        &arg.io,
        schedule,
        resolved_state,
        adapter,
        jinja_env,
        true,
        arg.check_all,
        sources_only,
        &arg.resource_types,
        &arg.exclude_resource_types,
    )
    .await?;
    emit_freshness_stats(&arg.io, &results);

    // `dbt source freshness` always writes `sources.json`, even with an empty
    // result set, matching dbt-core. The `selection_contains_source` guard only
    // applies to the unified `dbt freshness` spelling: a model-only
    // `dbt freshness --select some_model` would otherwise replace a good artifact
    // with an empty one, and downstream consumers read this file.
    if should_write_sources_json(
        sources_only,
        schedule,
        resolved_state,
        &arg.resource_types,
        &arg.exclude_resource_types,
    ) {
        let sources_artifact = artifacts_sink.sources.insert(build_sources_artifact(
            &arg.io.invocation_id,
            resolved_state,
            &results,
        ));
        write_sources_json(&arg.io.out_dir, &arg.io.in_dir, sources_artifact)?;
    }
    // `dbt source freshness` writes only `sources.json`.
    if !sources_only {
        let freshness_artifact =
            build_freshness_artifact(&arg.io.invocation_id, resolved_state, &results);
        write_freshness_json(&arg.io.out_dir, &arg.io.in_dir, &freshness_artifact)?;
    }
    if arg.write_metadata {
        write_freshness_results_parquet(&arg.io, resolved_state, &results);
    }

    let freshness_context_results = freshness_results_to_context(resolved_state, &results);
    for operation in resolved_state.operations.on_run_end.iter() {
        run_operation_on_run(
            operation,
            &arg.io,
            &None,
            &None,
            None,
            jinja_env,
            base_context,
            Value::from_serialize(&freshness_context_results),
        )
        .await?;
    }

    checkpoint_maybe_exit(arg, Phases::Freshness)?;

    Ok(Some(Box::new(FreshnessTimestamps(results))))
}
