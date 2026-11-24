use std::sync::Arc;

use async_trait::async_trait;
use dbt_adapter::Adapter;
use dbt_common::FsError;
use dbt_common::FsResult;
use dbt_common::cancellation::CancellationToken;
use dbt_dag::schedule::Schedule;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schema_store::DataStoreTrait;
use dbt_schema_store::store::SchemaStore;
use dbt_schemas::schemas::ResolvedCloudConfig;
use dbt_schemas::schemas::StateArtifacts;
use dbt_schemas::state::ResolverState;
use petgraph::Graph;

use crate::RunTasksArgs;
use crate::ShowableResults;
use crate::StoreableResults;
use crate::context::TaskRunnerCtx;
use crate::context_factory::ExtendedTaskRunnerCtxFactory;
use crate::metricflow::MetricflowClient;
use crate::task::Task;
use dbt_schemas::schemas::Preview;

#[async_trait]
pub trait TaskRunnerHooks: Send + Sync {
    fn resolved_state(&self) -> &ResolverState;

    async fn create_extended_ctx_factory(
        &self,
        run_task_args: &Arc<RunTasksArgs>,
    ) -> FsResult<Box<dyn ExtendedTaskRunnerCtxFactory>>;

    fn show_taskgraph(&self, graph: &Graph<Arc<dyn Task>, ()>);

    fn will_run(&self, run_task_args: &RunTasksArgs, schedule: &Schedule<String>);

    async fn did_register_schemas(
        &self,
        registered_schemas: bool,
        run_task_args: &RunTasksArgs,
        schedule: &Schedule<String>,
        ctx: &mut TaskRunnerCtx,
    ) -> Result<(), Box<FsError>>;

    fn collect_showables(&self, ctx: &mut TaskRunnerCtx) -> Vec<Box<dyn ShowableResults>>;

    fn collect_preview(&self, ctx: &mut TaskRunnerCtx) -> Option<Result<Preview, String>>;

    fn collect_storeables(
        &mut self,
        run_task_args: &RunTasksArgs,
        ctx: &mut TaskRunnerCtx,
    ) -> Vec<Box<dyn StoreableResults>>;

    /// Runs at the end of a run, once its results are collected, so an implementation can
    /// release what it set up for the run.
    ///
    /// It also runs after Ctrl+C or `--fail-fast` cancelled the run, with `token` cancelled. The
    /// command then has `TIMEOUT_AFTER_CTRL_C` in total, counted from the cancel, and
    /// run_results.json is only written after this returns. So when `token.is_cancelled()`, skip
    /// anything that can wait long on a warehouse, the network or another process. The task
    /// runner stops waiting for this call after a short timeout, which only takes effect at an
    /// `.await`: never block the thread here.
    async fn did_collect_all_run_task_results(
        &self,
        run_task_args: &RunTasksArgs,
        ctx: &mut TaskRunnerCtx,
        token: &CancellationToken,
    );

    async fn will_visit_taskgraph(
        &self,
        run_task_args: &Arc<RunTasksArgs>,
        schedule: &Schedule<String>,
        has_dynamic_closure: bool,
        on_run_start_sqls: &[String],
        graph: &Graph<Arc<dyn Task>, ()>,
        ctx: &mut TaskRunnerCtx,
        token: &CancellationToken,
    ) -> FsResult<()>;

    async fn did_visit_taskgraph(
        &self,
        run_task_args: &Arc<RunTasksArgs>,
        schedule: &Schedule<String>,
        graph: &Graph<Arc<dyn Task>, ()>,
        ctx: &mut TaskRunnerCtx,
        token: &CancellationToken,
    ) -> FsResult<()>;
}

pub trait TaskRunnerHooksFactory: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    fn create(
        &self,
        cloud_config: Option<ResolvedCloudConfig>,
        previous_state: Option<Arc<StateArtifacts>>,
        adapter: Arc<Adapter>,
        resolved_state: Arc<ResolverState>,
        jinja_env: Arc<JinjaEnv>,
        schema_store: Arc<SchemaStore>,
        data_store: Arc<dyn DataStoreTrait>,
        metricflow_server_client: Option<Arc<dyn MetricflowClient>>,
    ) -> Box<dyn TaskRunnerHooks>;
}
