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

    let mut base_context = ctx.base_context_for_adapter(snapshot.node_adapter())?;
    add_task_context(&mut base_context, snapshot.common(), &ctx.thread_id);
    let jinja_env = ctx.jinja_env_for_adapter(snapshot.node_adapter())?;

    let (relations_map, main_response) = materialize_snapshot(
        &sql_instruction.sql,
        snapshot,
        snapshot.node_adapter(),
        ctx.runtime_config(),
        &ctx.inner.materialization_resolver,
        jinja_env.clone(),
        &base_context,
        &ctx.inner.arg.io,
    )?;
    if let Some(main_response) = main_response {
        ctx.inner
            .main_adapter_responses
            .insert(snapshot.__common_attr__.unique_id.clone(), main_response);
    }
    let _ = cache_materialization_return_value(jinja_env, &relations_map);

    Ok(NodeStatus::Succeeded)
}
