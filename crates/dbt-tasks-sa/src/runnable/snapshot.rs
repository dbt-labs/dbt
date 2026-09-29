use crate::materialize::materialize_snapshot;
use crate::runnable::cache::cache_materialization_return_value;
use dbt_common::FsResult;
use dbt_common::stats::NodeStatus;
use dbt_jinja_utils::utils::add_task_context;
use dbt_schemas::schemas::{DbtSnapshot, InternalDbtNode, InternalDbtNodeAttributes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::TaskResult;

pub fn execute_snapshot_remote(
    snapshot: &DbtSnapshot,
    ctx: &TaskRunnerCtx,
    task_result: &TaskResult,
) -> FsResult<NodeStatus> {
    let sql_instruction = &task_result.sql_instruction;

    let mut base_context = ctx.inner.base_context.clone();
    add_task_context(&mut base_context, snapshot.common(), &ctx.thread_id);

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
            .insert(snapshot.__common_attr__.unique_id.clone(), main_response);
    }
    let _ = cache_materialization_return_value(ctx.env.clone(), &relations_map);

    Ok(NodeStatus::Succeeded)
}
