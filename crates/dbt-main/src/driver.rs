use std::sync::Arc;
use std::time::SystemTime;

use dbt_clap_core::Cli;
use dbt_common::{FsResult, cancellation::CancellationToken, io_args::EvalArgs};
use dbt_compilation::schedule::DbtProjectCompilationCacheChanges;
use dbt_compilation::traits::{CompilationCache, CompilationDriver, CompiledProject};
use dbt_compilation::traits::{RunTasksResult, TaskExecutionDriver};
use dbt_dag::schedule::Schedule;
use dbt_features::feature_stack::FeatureStack;
use dbt_jinja_utils::{
    jinja_environment::JinjaEnv, listener::JinjaTypeCheckingEventListenerFactory,
};
use dbt_schemas::schemas::DbtCommandExecutionArtifacts;

use dbt_tasks_core::task_runner_hooks::TaskRunnerHooksFactory;

use crate::compilation::{DbtProjectCompilation, DbtProjectCompilationCacheState};

pub struct DbtCompilationDriver {
    feature_stack: Arc<FeatureStack>,
}

impl DbtCompilationDriver {
    pub fn new(feature_stack: Arc<FeatureStack>) -> Self {
        Self { feature_stack }
    }
}

pub struct DbtTaskExecutionDriver {
    feature_stack: Arc<FeatureStack>,
}

impl DbtTaskExecutionDriver {
    pub fn new(feature_stack: Arc<FeatureStack>) -> Self {
        Self { feature_stack }
    }
}

#[async_trait::async_trait]
impl CompilationDriver for DbtCompilationDriver {
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
    )> {
        let prev_concrete: Option<Arc<DbtProjectCompilation>> =
            prev.and_then(|p| p.into_any_arc().downcast::<DbtProjectCompilation>().ok());

        let (compilation, jinja_env, cache_changes) = DbtProjectCompilation::initialize_server(
            &self.feature_stack,
            arg,
            cli,
            jinja_type_checking_factory,
            prev_concrete,
            token,
        )
        .await?;

        let compiled: Arc<dyn CompiledProject> = Arc::new(compilation);
        Ok((compiled, jinja_env, cache_changes))
    }
}

#[async_trait::async_trait]
impl TaskExecutionDriver for DbtTaskExecutionDriver {
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
    ) -> FsResult<RunTasksResult> {
        let compiled = compiled
            .as_any()
            .downcast_ref::<DbtProjectCompilation>()
            .expect("expected DbtProjectCompilation");

        let previous_cache_state: Option<Arc<DbtProjectCompilationCacheState>> = previous_cache
            .and_then(|c| {
                c.into_any_arc()
                    .downcast::<DbtProjectCompilationCacheState>()
                    .ok()
            });

        let result = compiled
            .run_tasks(
                arg,
                cli,
                start,
                jinja_env,
                Arc::clone(&self.feature_stack),
                schedule.clone(),
                compilation_cache_changes,
                previous_cache_state,
                jinja_type_checking_factory,
                task_runner_hooks_factory,
                token,
                Default::default(),
                &mut DbtCommandExecutionArtifacts::default(),
            )
            .await?;

        let (run_task_args, run_task_results, _jinja_env, _adapter, cache_state) = result;

        let cache: Arc<dyn CompilationCache> = cache_state;
        Ok((run_task_args, run_task_results, cache))
    }
}
