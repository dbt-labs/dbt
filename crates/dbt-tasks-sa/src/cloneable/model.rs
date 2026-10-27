use std::future::Future;
use std::pin::Pin;

use dbt_common::FsResult;
use dbt_common::stats::NodeStatus;
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::run_blocking_task_operation;

use crate::cloneable::Cloneable;
use crate::materialize::materialize_clone;
use crate::runnable::cache::cache_materialization_return_value;

impl Cloneable for DbtModel {
    fn execute<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move {
            let base_context = ctx.run_base_context(self)?;

            let adapter_type = self.node_adapter();
            let node = self.clone();
            let ctx_inner = ctx.clone();

            let result = run_blocking_task_operation(move || {
                materialize_clone(
                    &node,
                    &node.deprecated_config,
                    adapter_type,
                    ctx_inner.runtime_config(),
                    ctx_inner.defer_nodes(),
                    &ctx_inner.inner.materialization_resolver,
                    ctx_inner.env.clone(),
                    &base_context,
                    &ctx_inner.inner.arg.io,
                    None,
                )
            })
            .await??;

            let _ = cache_materialization_return_value(ctx.env.clone(), &result);

            Ok(NodeStatus::Succeeded)
        })
    }
}
