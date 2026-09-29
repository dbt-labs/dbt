use std::sync::Arc;

use async_trait::async_trait;
use dbt_common::{FsResult, stats::NodeStatus};

use crate::run_cache::run_cache_service::RunCacheServiceDecision;
use crate::{context::TaskRunnerCtx, task::TaskResult};
use dbt_schemas::schemas::{DbtUnitTest, InternalDbtNodeAttributes, manifest::DbtSavedQuery};

#[async_trait]
pub trait RunTaskHooks: Send + Sync {
    async fn execute_saved_query(
        &self,
        ctx: &TaskRunnerCtx,
        saved_query: &DbtSavedQuery,
    ) -> FsResult<()>;
    async fn check_sao_cache(
        &self,
        ctx: &mut TaskRunnerCtx,
        node: Arc<dyn InternalDbtNodeAttributes>,
        sql: &str,
    ) -> FsResult<RunCacheServiceDecision>;
    async fn run_lake_compute_sidecar(
        &self,
        ctx: &mut TaskRunnerCtx,
        node: Arc<dyn InternalDbtNodeAttributes>,
        task_result: Option<TaskResult>,
    ) -> FsResult<NodeStatus>;
    async fn run_on_lake_compute(
        &self,
        ctx: &mut TaskRunnerCtx,
        node: Arc<dyn InternalDbtNodeAttributes>,
        task_result: Option<TaskResult>,
    ) -> FsResult<NodeStatus>;
    async fn did_run_unit_test(
        &self,
        ctx: &mut TaskRunnerCtx,
        unit_test: &DbtUnitTest,
        task_result: &TaskResult,
        passed: bool,
        diff_num_rows: usize,
        diff: String,
    ) -> FsResult<()>;
}
