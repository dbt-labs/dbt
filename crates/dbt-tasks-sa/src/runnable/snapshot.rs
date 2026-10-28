use crate::materialize::materialize_snapshot;
use crate::runnable::cache::cache_materialization_return_value;
use dbt_common::FsResult;
use dbt_common::stats::NodeStatus;
use dbt_schemas::schemas::{DbtSnapshot, InternalDbtNodeAttributes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::TaskResult;

pub fn execute_snapshot_remote(
    snapshot: &DbtSnapshot,
    ctx: &TaskRunnerCtx,
    task_result: &TaskResult,
) -> FsResult<NodeStatus> {
    let sql_instruction = &task_result.sql_instruction;

    let base_context = ctx.run_base_context(snapshot)?;

    let (relations_map, main_response) = materialize_snapshot(
        &sql_instruction.sql,
        snapshot,
        snapshot.node_adapter(),
        ctx.runtime_config(),
        &ctx.inner.materialization_resolver,
        ctx.env.clone(),
        &base_context,
        &ctx.inner.arg.io,
    )?;
    if let Some(main_response) = main_response {
        ctx.inner
            .main_adapter_responses
            .upsert_sync(snapshot.__common_attr__.unique_id.clone(), main_response);
    }
    let _ = cache_materialization_return_value(ctx.env.clone(), &relations_map);

    Ok(NodeStatus::Succeeded)
}
