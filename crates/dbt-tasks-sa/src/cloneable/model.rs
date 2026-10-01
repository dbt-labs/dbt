use std::future::Future;
use std::pin::Pin;

use dbt_common::FsResult;
use dbt_common::stats::NodeStatus;
use dbt_jinja_utils::utils::add_task_context;
use dbt_schemas::schemas::{DbtModel, InternalDbtNode, InternalDbtNodeAttributes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::TaskOp;

use crate::cloneable::Cloneable;
use crate::materialize::materialize_clone;
use crate::runnable::cache::cache_materialization_return_value;

impl Cloneable for DbtModel {
    fn execute<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move {
            let mut base_context = ctx.inner.base_context.clone();

            add_task_context(&mut base_context, self.common(), &ctx.thread_id);

            let adapter_type = self.node_adapter();
            let node = self.clone();
            let ctx_inner = ctx.clone();

            let result = TaskOp::Blocking(Box::new(move || {
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
            }))
            .run()
            .await??;

            let _ = cache_materialization_return_value(ctx.env.clone(), &result);

            Ok(NodeStatus::Succeeded)
        })
    }
}
