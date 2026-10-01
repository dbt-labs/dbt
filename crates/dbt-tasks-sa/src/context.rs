use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use dbt_adapter_core::AdapterType;
use dbt_common::FsError;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_tasks_core::context::{ExtendedCtx, TaskRunnerCtx};
use dbt_tasks_core::context_factory::ExtendedTaskRunnerCtxFactory;
use dbt_tasks_core::task::Task;
use dbt_tasks_core::{AdhocRunner, RunTasksArgs};

use crate::run_adhoc::RemoteAdhocRunner;

pub struct EmptyExtendedTaskRunnerCtxImpl;

impl ExtendedCtx for EmptyExtendedTaskRunnerCtxImpl {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }

    fn on_test_failure(
        &self,
        _ctx: &TaskRunnerCtx,
        _node: &Arc<dyn Task>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async {})
    }

    fn is_sidecar(&self) -> bool {
        false
    }
}

pub struct EmptyExtendedTaskRunnerCtxFactory;

impl ExtendedTaskRunnerCtxFactory for EmptyExtendedTaskRunnerCtxFactory {
    fn adhoc_runner(
        &self,
        env: Arc<JinjaEnv>,
        adapter_type: AdapterType,
        args: Arc<RunTasksArgs>,
        _root_project_name: String,
    ) -> Arc<dyn AdhocRunner> {
        Arc::new(RemoteAdhocRunner {
            env,
            adapter_type,
            query_id: args.query_id.clone(),
        })
    }

    fn build(
        self: Box<Self>,
        _run_cache_enabled: bool,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn ExtendedCtx>, Box<FsError>>> + Send>> {
        Box::pin(async { Ok(Box::new(EmptyExtendedTaskRunnerCtxImpl) as Box<dyn ExtendedCtx>) })
    }
}
