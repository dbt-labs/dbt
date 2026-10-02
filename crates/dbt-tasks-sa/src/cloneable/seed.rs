use std::future::Future;
use std::pin::Pin;

use dbt_common::FsResult;
use dbt_common::stats::NodeStatus;
use dbt_jinja_utils::utils::add_task_context;
use dbt_schemas::schemas::{DbtSeed, InternalDbtNode, InternalDbtNodeAttributes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::TaskOp;

use crate::cloneable::Cloneable;
use crate::materialize::materialize_clone;
use crate::runnable::cache::cache_materialization_return_value;

impl Cloneable for DbtSeed {
    fn execute<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move {
            let mut base_context = ctx.base_context_for_adapter(self.node_adapter())?;

            add_task_context(&mut base_context, self.common(), &ctx.thread_id);

            let adapter_type = self.node_adapter();
            let jinja_env = ctx.jinja_env_for_adapter(adapter_type)?;
            let node = self.clone();
            let ctx_inner = ctx.clone();
            let materialize_env = jinja_env.clone();

            let result = TaskOp::Blocking(Box::new(move || {
                materialize_clone(
                    &node,
                    &node.deprecated_config,
                    adapter_type,
                    ctx_inner.runtime_config(),
                    ctx_inner.defer_nodes(),
                    &ctx_inner.inner.materialization_resolver,
                    materialize_env,
                    &base_context,
                    &ctx_inner.inner.arg.io,
                    None,
                )
            }))
            .run()
            .await??;

            let _ = cache_materialization_return_value(jinja_env, &result);

            Ok(NodeStatus::Succeeded)
        })
    }
}
