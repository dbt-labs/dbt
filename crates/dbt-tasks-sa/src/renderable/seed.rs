/// Seed registration happens in `pre_register_seeds` before the task graph.
/// This impl verifies the seed was registered successfully (schema exists in
/// the store). If not, returns `Errored` to propagate failure through the
/// task graph skip mechanism. The actual error was already emitted by
/// `pre_register_seeds`.
use std::sync::Arc;

use dbt_common::FsResult;
use dbt_common::collections::DashMap;
use dbt_common::stats::NodeStatus;
use dbt_scheduler::instructions::SqlInstruction;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::TaskResult;

pub fn run_seed_render(
    node: Arc<dyn InternalDbtNodeAttributes>,
    ctx: TaskRunnerCtx,
    result_sender: Option<std::sync::mpsc::SyncSender<TaskResult>>,
) -> FsResult<NodeStatus> {
    let relation =
        dbt_adapter::relation::create_relation_from_node(node.node_adapter(), node.as_ref(), None)?;
    let cfqn = relation.get_canonical_fqn()?;
    if !ctx.schema_cache.exists(&cfqn) {
        // Don't emit a new error — pre_register_seeds already logged it.
        return Ok(NodeStatus::Errored);
    }

    if let Some(sender) = result_sender {
        let _ = sender.send(TaskResult {
            sql_instruction: SqlInstruction::default(),
            config_map: Arc::new(DashMap::default()),
            lp_instruction: None,
        });
    }
    Ok(NodeStatus::Succeeded)
}
