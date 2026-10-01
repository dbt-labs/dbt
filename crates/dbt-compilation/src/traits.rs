use std::collections::HashSet;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use dbt_adapter_core::AdapterType;
use dbt_clap_core::Cli;
use dbt_common::{FsResult, cancellation::CancellationToken, io_args::EvalArgs, path::DbtPath};
use dbt_dag::schedule::Schedule;
use dbt_jinja_utils::{
    jinja_environment::JinjaEnv, listener::JinjaTypeCheckingEventListenerFactory,
};
use dbt_schema_store::store::{DataStore, SchemaStore};
use dbt_schemas::{
    schemas::{Nodes, project::DbtProject},
    state::{Macros, ModelStatus, ResolverState},
};
use dbt_tasks_core::task_runner_hooks::TaskRunnerHooksFactory;
use dbt_tasks_core::{RunTaskResults, RunTasksArgs};

use crate::core::DbtLoadedProject;
use crate::schedule::{DbtProjectCompilationCacheChanges, DbtScheduleDescription};

/// Read surface of a compilation's cache state.
pub trait CompilationCache: Send + Sync {
    fn schema_exists_by_unique_id(&self, unique_id: &str) -> bool;
    fn get_schema_by_unique_id(&self, unique_id: &str) -> Option<arrow_schema::SchemaRef>;
    fn schema_store(&self) -> Arc<SchemaStore>;
    fn data_store(&self) -> Arc<DataStore>;
    fn into_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync>;
}

/// Read surface of a fully compiled dbt project.
#[async_trait]
pub trait CompiledProject: Send + Sync {
    fn resolved_state(&self) -> &ResolverState;
    fn nodes(&self) -> &Nodes;
    fn loaded_project(&self) -> &DbtLoadedProject;
    fn root_project(&self) -> &DbtProject;
    fn adapter_type(&self) -> AdapterType;
    fn has_file_changed(&self, relative_path: &DbtPath) -> bool;
    fn create_jinja_env(&self, arg: &EvalArgs, token: CancellationToken) -> FsResult<JinjaEnv>;
    fn lookup_ref(
        &self,
        maybe_package_name: &Option<String>,
        model_name: &str,
        name: &Option<String>,
        maybe_node_package_name: &Option<String>,
    ) -> Option<(String, ModelStatus)>;
    async fn create_schedule<'a>(
        &self,
        cli: &Cli,
        arg: &EvalArgs,
        schedule_desc: DbtScheduleDescription<'a>,
        exclude_unique_ids: HashSet<String>,
        token: &CancellationToken,
    ) -> FsResult<Schedule<String>>;
    fn macros(&self) -> &Macros;
    fn root_project_name(&self) -> &str;
    fn root_project_id(&self) -> String;
    fn models_count(&self) -> u32;
    fn as_any(&self) -> &dyn std::any::Any;
    fn into_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Send + Sync>;
}

/// Factory for producing compiled projects from source.
#[async_trait]
pub trait CompilationDriver: Send + Sync {
    async fn compile(
        &self,
        arg: &EvalArgs,
        cli: &Cli,
        jinja_type_checking_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
        prev: Option<Arc<dyn CompiledProject>>,
        token: &CancellationToken,
    ) -> FsResult<(
        Arc<dyn CompiledProject>,
        JinjaEnv,
        Option<DbtProjectCompilationCacheChanges>,
    )>;
}

#[allow(clippy::type_complexity)]
pub type RunTasksResult = (Arc<RunTasksArgs>, RunTaskResults, Arc<dyn CompilationCache>);

/// Runs tasks (static analysis, execution) against a compiled project.
#[async_trait]
pub trait TaskExecutionDriver: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    async fn run_tasks(
        &self,
        compiled: &dyn CompiledProject,
        arg: &EvalArgs,
        cli: &Cli,
        start: SystemTime,
        jinja_env: JinjaEnv,
        schedule: &Schedule<String>,
        compilation_cache_changes: Option<&DbtProjectCompilationCacheChanges>,
        previous_cache: Option<Arc<dyn CompilationCache>>,
        jinja_type_checking_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
        task_runner_hooks_factory: &dyn TaskRunnerHooksFactory,
        token: &CancellationToken,
    ) -> FsResult<RunTasksResult>;
}
