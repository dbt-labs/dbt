use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::mpsc;

use dbt_adapter::relation::create_relation_from_node;
use dbt_common::FsResult;
use dbt_common::stats::NodeStatus;
use dbt_schemas::schemas::profiles::Execute;
use dbt_schemas::schemas::{DbtSnapshot, InternalDbtNodeAttributes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::show_task_hooks::ShowTaskHooks;
use dbt_tasks_core::task::TaskResult;

use super::{Showable, rendered_sql_for, run_show};

impl Showable for DbtSnapshot {
    fn visit_show<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
        _result_receiver: &'a mut Option<mpsc::Receiver<TaskResult>>,
        show_task_hooks: &'a Arc<dyn ShowTaskHooks>,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move {
            let unique_id = self.__common_attr__.unique_id.clone();

            run_show(
                ctx,
                &unique_id,
                "snapshot",
                false,
                self.__common_attr__.name.as_str(),
                show_task_hooks,
                |ctx| match ctx.inner.execute {
                    Execute::Remote => rendered_sql_for(ctx, unique_id.as_str(), "snapshot"),
                    Execute::Sidecar | Execute::Service => {
                        // Query the materialized snapshot table using canonical FQN
                        let relation = create_relation_from_node(self.node_adapter(), self, None)?;
                        let fqn = relation.get_canonical_fqn()?;
                        Ok(format!("select * from {fqn}"))
                    }
                },
            )
            .await
        })
    }
}
