use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::mpsc;

use dbt_adapter::relation::create_relation_from_node;
use dbt_common::FsResult;
use dbt_common::stats::NodeStatus;
use dbt_schemas::schemas::DbtModel;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::profiles::Execute;
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::show_task_hooks::ShowTaskHooks;
use dbt_tasks_core::task::TaskResult;

use super::{Showable, rendered_sql_for, run_show};

impl Showable for DbtModel {
    fn visit_show<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
        result_receiver: &'a mut Option<mpsc::Receiver<TaskResult>>,
        show_task_hooks: &'a Arc<dyn ShowTaskHooks>,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move {
            let unique_id = self.__common_attr__.unique_id.clone();
            // Inline models have materialization = Inline, check that instead of name
            let is_inline = self.materialized() == DbtMaterialization::Inline;
            let (node_name, show_unique_id) = if is_inline {
                ("inline_query", "sql_operation.inline")
            } else {
                (self.__common_attr__.name.as_str(), unique_id.as_str())
            };

            // Receive the task result from the previous phase (analyze) if available
            let task_result = if let Some(receiver) = result_receiver.as_mut() {
                receiver.recv().ok()
            } else {
                None
            };

            run_show(
                ctx,
                show_unique_id,
                "model",
                is_inline,
                node_name,
                show_task_hooks,
                |ctx| {
                    // For inline models, always use rendered SQL (they don't materialize to a table)
                    if is_inline {
                        return rendered_sql_for(ctx, unique_id.as_str(), "model");
                    }

                    match ctx.inner.execute {
                        Execute::Remote => rendered_sql_for(ctx, unique_id.as_str(), "model"),
                        Execute::Sidecar | Execute::Service => {
                            let lp_plan = task_result
                                .as_ref()
                                .and_then(|r| r.lp_instruction.as_ref())
                                .map(|lp| &lp.plan);
                            let relation =
                                create_relation_from_node(self.node_adapter(), self, None)?;
                            let canonical_fqn = relation.get_canonical_fqn()?.to_string();
                            if let Some(result) = show_task_hooks.get_model_sidecar_sql(
                                ctx,
                                lp_plan,
                                &self.__common_attr__.path,
                                &ctx.inner.arg.io.out_dir,
                                &canonical_fqn,
                            ) {
                                result
                            } else {
                                Ok(format!("select * from {canonical_fqn}"))
                            }
                        }
                    }
                },
            )
            .await
        })
    }
}
