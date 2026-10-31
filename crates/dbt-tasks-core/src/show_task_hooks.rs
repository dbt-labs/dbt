use arrow::array::RecordBatch;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use dbt_common::FsResult;
use dbt_scheduler::instructions::LpInstruction;

use crate::context::TaskRunnerCtx;

#[async_trait]
pub trait ShowTaskHooks: Send + Sync {
    /// Execute a show query via the worker backend (sidecar).
    async fn run_show_query_batches(
        &self,
        ctx: &mut TaskRunnerCtx,
        unique_id: &str,
        sql: String,
        limit: Option<u64>,
    ) -> Option<FsResult<(Vec<RecordBatch>, SchemaRef)>>;

    /// Produce the SQL string for a `dbt show` on a model when running via
    /// the sidecar backend.
    fn get_model_sidecar_sql(
        &self,
        ctx: &TaskRunnerCtx,
        lp_instruction: Option<&LpInstruction>,
        model_path: &std::path::Path,
        out_dir: &std::path::Path,
        canonical_fqn: &str,
    ) -> Option<FsResult<String>>;
}

pub struct DefaultShowTaskHooks;

#[async_trait]
impl ShowTaskHooks for DefaultShowTaskHooks {
    async fn run_show_query_batches(
        &self,
        _ctx: &mut TaskRunnerCtx,
        _unique_id: &str,
        _sql: String,
        _limit: Option<u64>,
    ) -> Option<FsResult<(Vec<RecordBatch>, SchemaRef)>> {
        None
    }

    fn get_model_sidecar_sql(
        &self,
        _ctx: &TaskRunnerCtx,
        _lp_instruction: Option<&LpInstruction>,
        _model_path: &std::path::Path,
        _out_dir: &std::path::Path,
        _canonical_fqn: &str,
    ) -> Option<FsResult<String>> {
        None
    }
}
