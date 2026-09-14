use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dbt_adapter::Adapter;
use dbt_adapter::engine::SidecarClient;
use dbt_common::FsError;
use dbt_common::FsResult;
use dbt_common::cancellation::CancellationToken;
use dbt_common::io_args::EvalArgs;
use dbt_common::io_args::IoArgs;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_dag::schedule::Schedule;
use dbt_defer::DeferState;
use dbt_schema_store::store::SchemaStore;
use dbt_schemas::schemas::profiles::Execute;
use dbt_schemas::state::ResolverState;
use dbt_tasks_core::metricflow::MetricflowClient;
use dbt_tasks_core::static_analysis_buckets::StaticAnalysisBuckets;

use crate::config::CompilationConfig;

#[derive(Clone)]
pub struct SchemaHydrationArgs<'a> {
    pub io: &'a IoArgs,
    pub global_static_analysis: Option<StaticAnalysisKind>,
    pub execute_mode: Execute,
    pub scope: SchemaHydrationScope,
    /// `--infer-schemas`: a node whose schema we failed to download should
    /// stay eligible for static analysis instead of having it disabled, so
    /// binding can fall back to inferring the schema from usage.
    pub infer_schemas: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaHydrationScope {
    FrontierOnly,
    Full,
}

#[derive(Clone)]
pub struct SchemaHydrationDownloadWarning {
    pub err: Arc<FsError>,
    pub unique_id: String,
}

#[derive(Clone, Default)]
pub struct SchemaHydrationState {
    pub fetched_schema_fqns: HashSet<String>,
    pub download_warnings_by_fqn: HashMap<String, SchemaHydrationDownloadWarning>,
}

#[async_trait::async_trait]
pub trait SchemaHydrator: Send + Sync {
    async fn hydrate_schemas(
        self: Box<Self>,
        arg: &EvalArgs,
        schedule: &Schedule<String>,
        resolved_state: &mut ResolverState,
        schema_hydration_state: &mut SchemaHydrationState,
        defer_state: &mut DeferState,
        token: CancellationToken,
    ) -> FsResult<Box<dyn StaticAnalysisBuckets>>;
}

pub trait SchemaHydratorFactory: Send + Sync {
    fn create(
        &self,
        adapter: Arc<Adapter>,
        execute_mode: Execute,
        compilation_config: CompilationConfig,
        schema_store: Arc<SchemaStore>,
        sidecar_client: Option<Arc<dyn SidecarClient>>,
        metricflow_server_client: Option<Arc<dyn MetricflowClient>>,
    ) -> Box<dyn SchemaHydrator>;
}
