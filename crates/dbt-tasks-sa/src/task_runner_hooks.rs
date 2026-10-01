use std::sync::Arc;

use async_trait::async_trait;
use dbt_adapter::Adapter;
use dbt_common::FsError;
use dbt_common::FsResult;
use dbt_common::cancellation::CancellationToken;
use dbt_dag::schedule::Schedule;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_pretty_table::batches_to_json_rows;
use dbt_pretty_table::make_column_names;
use dbt_schema_store::DataStoreTrait;
use dbt_schema_store::store::SchemaStore;
use dbt_schemas::schemas::ResolvedCloudConfig;
use dbt_schemas::schemas::StateArtifacts;
use dbt_schemas::state::ResolverState;
use dbt_tasks_core::Preview;
use dbt_tasks_core::RunTasksArgs;
use dbt_tasks_core::ShowableResults;
use dbt_tasks_core::StoreableResults;
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::context_factory::ExtendedTaskRunnerCtxFactory;
use dbt_tasks_core::metricflow::MetricflowClient;
use dbt_tasks_core::task::Task;
use petgraph::Graph;

use crate::context::EmptyExtendedTaskRunnerCtxFactory;

pub use dbt_tasks_core::task_runner_hooks::{TaskRunnerHooks, TaskRunnerHooksFactory};

// TODO: implement this
pub struct DefaultTaskRunnerHooksFactory;

impl TaskRunnerHooksFactory for DefaultTaskRunnerHooksFactory {
    fn create(
        &self,
        _cloud_config: Option<ResolvedCloudConfig>,
        _previous_state: Option<Arc<StateArtifacts>>,
        _adapter: Arc<Adapter>,
        resolved_state: Arc<ResolverState>,
        _jinja_env: Arc<JinjaEnv>,
        _schema_store: Arc<SchemaStore>,
        _data_store: Arc<dyn DataStoreTrait>,
        _metricflow_server_client: Option<Arc<dyn MetricflowClient>>,
    ) -> Box<dyn TaskRunnerHooks> {
        Box::new(DefaultTaskRunnerHooks { resolved_state })
    }
}

struct DefaultTaskRunnerHooks {
    resolved_state: Arc<ResolverState>,
}

#[async_trait]
impl TaskRunnerHooks for DefaultTaskRunnerHooks {
    fn resolved_state(&self) -> &ResolverState {
        &self.resolved_state
    }

    async fn create_extended_ctx_factory(
        &self,
        _run_task_args: &Arc<RunTasksArgs>,
    ) -> FsResult<Box<dyn ExtendedTaskRunnerCtxFactory>> {
        Ok(Box::new(EmptyExtendedTaskRunnerCtxFactory))
    }

    fn show_taskgraph(&self, _graph: &Graph<Arc<dyn Task>, ()>) {
        // TODO: implement show_taskgraph
    }

    fn will_run(&self, _run_task_args: &RunTasksArgs, _schedule: &Schedule<String>) {
        // No-op
    }

    async fn did_register_schemas(
        &self,
        _registered_schemas: bool,
        _run_task_args: &RunTasksArgs,
        _schedule: &Schedule<String>,
        _ctx: &mut TaskRunnerCtx,
    ) -> Result<(), Box<FsError>> {
        // TODO: implement cache invalidation logic here
        Ok(())
    }

    fn collect_showables(&self, _ctx: &mut TaskRunnerCtx) -> Vec<Box<dyn ShowableResults>> {
        vec![]
    }

    fn collect_preview(&self, ctx: &mut TaskRunnerCtx) -> Option<Result<Preview, String>> {
        let results = {
            let mut guard = ctx.inner.preview_results.lock();
            guard.take().map(|(batches, schema)| {
                let columns = make_column_names(schema.as_ref());
                let rows = batches_to_json_rows(&batches);
                (columns, rows)
            })
        };

        let error = ctx.inner.preview_error.lock().take();
        match (results, error) {
            (_, Some(e)) => Some(Err(e)),
            (Some((columns, rows)), None) => Some(Ok(Preview { columns, rows })),
            (None, None) => None,
        }
    }

    fn collect_storeables(
        &mut self,
        _run_task_args: &RunTasksArgs,
        _ctx: &mut TaskRunnerCtx,
    ) -> Vec<Box<dyn StoreableResults>> {
        // No-op
        vec![]
    }

    async fn did_collect_all_run_task_results(
        &self,
        _run_task_args: &RunTasksArgs,
        ctx: &mut TaskRunnerCtx,
        token: &CancellationToken,
    ) {
        if ctx.inner.run_cache_ctx.run_cache_service_requested {
            dbt_tasks_core::run_cache::run_cache_service::run_cache_service_after_run(
                ctx,
                token.is_cancelled(),
            )
            .await;
        }
    }

    async fn will_visit_taskgraph(
        &self,
        _run_task_args: &Arc<RunTasksArgs>,
        _schedule: &Schedule<String>,
        _has_dynamic_closure: bool,
        _on_run_start_sqls: &[String],
        _graph: &Graph<Arc<dyn Task>, ()>,
        ctx: &mut TaskRunnerCtx,
        _token: &CancellationToken,
    ) -> FsResult<()> {
        if ctx.inner.run_cache_ctx.run_cache_service_requested {
            dbt_tasks_core::run_cache::run_cache_service::run_cache_service_start_telemetry(ctx)
                .await;
        }
        Ok(())
    }

    async fn did_visit_taskgraph(
        &self,
        _run_task_args: &Arc<RunTasksArgs>,
        _schedule: &Schedule<String>,
        _graph: &Graph<Arc<dyn Task>, ()>,
        _ctx: &mut TaskRunnerCtx,
        _token: &CancellationToken,
    ) -> FsResult<()> {
        // No-op
        Ok(())
    }
}
