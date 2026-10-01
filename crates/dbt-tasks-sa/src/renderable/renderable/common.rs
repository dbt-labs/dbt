use std::sync::Arc;
use std::sync::mpsc;
use std::time::SystemTime;

use dbt_common::FsResult;
use dbt_common::collections::DashMap;
use dbt_common::stats::{NodeStatus, Stat};
use dbt_scheduler::instructions::SqlInstruction;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::TaskResult;

use minijinja::Value as MinijinjaValue;

pub fn handle_render_result(
    res: FsResult<(SqlInstruction, Arc<DashMap<String, MinijinjaValue>>)>,
    unique_id: &str,
    materialization: &DbtMaterialization,
    ctx: &mut TaskRunnerCtx,
    result_sender: &Option<mpsc::SyncSender<TaskResult>>,
) -> FsResult<NodeStatus> {
    let start = SystemTime::now();
    let (sql_instruction, config_map) = match res {
        Err(err) => {
            let thread_id = ctx.thread_id;
            ctx.inner.run_stats.insert(
                unique_id.to_string(),
                Stat::new(
                    unique_id.to_string(),
                    start,
                    None,
                    NodeStatus::Errored,
                    Some(err.to_string()),
                    thread_id,
                ),
            );
            // Sanity, clear the cache because there was an error.
            ctx.inner.compiled_sql_cache.clear(unique_id);
            return Err(err);
        }
        Ok(res) => res,
    };

    // Store the rendered SQL for use in unit test hash computation and for dbt show
    if unique_id.starts_with("model.")
        || unique_id.starts_with("snapshot.")
        || unique_id.starts_with("analysis.")
        || unique_id.starts_with("test.")
    {
        let node_info = dbt_tasks_core::context::RenderedNodeInfo {
            sql: sql_instruction.sql.clone(),
            materialization: materialization.clone(),
        };
        ctx.inner
            .rendered_sql
            .insert(unique_id.to_string(), node_info);
    }

    // Send result via channel instead of storing in DashMap
    if let Some(sender) = result_sender {
        let task_result = TaskResult {
            sql_instruction,
            config_map,
            lp_instruction: None,
        };
        // It is okay if the sender fails, the receiver will error
        let _ = sender.send(task_result);
    }

    Ok(NodeStatus::Succeeded)
}
