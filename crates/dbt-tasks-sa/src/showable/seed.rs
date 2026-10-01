use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::mpsc;

use dbt_common::FsResult;
use dbt_common::stats::NodeStatus;
use dbt_schemas::schemas::DbtSeed;
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::show_task_hooks::ShowTaskHooks;

use dbt_tasks_core::task::TaskResult;

use super::{Showable, run_show};

impl Showable for DbtSeed {
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
                "seed",
                false,
                self.__common_attr__.name.as_str(),
                show_task_hooks,
                |_| {
                    let table_name = format!(
                        "{}.{}.{}",
                        self.__base_attr__.database,
                        self.__base_attr__.schema,
                        self.__base_attr__.alias
                    );
                    Ok(format!("select * from {table_name}"))
                },
            )
            .await
        })
    }
}
