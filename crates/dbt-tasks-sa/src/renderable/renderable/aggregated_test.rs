use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::mpsc;

use dbt_common::FsResult;
use dbt_common::stats::NodeStatus;
use dbt_schemas::schemas::{DbtTest, InternalDbtNodeAttributes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::render_task_hooks::RenderTaskHooks;
use dbt_tasks_core::span_manager::SpanTreeRequest;
use dbt_tasks_core::task::TaskResult;
use dbt_tasks_core::task::{TP, Task};
use dbt_tasks_core::task_spans::create_task_span_for_node;
use dbt_tasks_core::test_aggregation::GenericTestGroup;
use dbt_tasks_core::visitor::SkipReason;
use dbt_telemetry::{ExecutionPhase, NodeType};

use super::RenderTask;

#[derive(Clone)]
pub struct AggregatedTestRenderTask {
    unique_id: String,
    aggregated_task: Arc<RenderTask>,
    member_tests: Vec<Arc<DbtTest>>,
}

impl AggregatedTestRenderTask {
    pub fn new(
        unique_id: String,
        node: Arc<dyn InternalDbtNodeAttributes>,
        member_tests: Vec<Arc<DbtTest>>,
        result_sender: Option<mpsc::SyncSender<TaskResult>>,
        render_task_hooks: Arc<dyn RenderTaskHooks>,
    ) -> Self {
        Self {
            unique_id,
            aggregated_task: Arc::new(RenderTask::new(
                node,
                result_sender,
                None,
                render_task_hooks,
            )),
            member_tests,
        }
    }

    pub fn from_generic_test_group(
        group: &GenericTestGroup,
        result_sender: Option<mpsc::SyncSender<TaskResult>>,
        render_task_hooks: Arc<dyn RenderTaskHooks>,
    ) -> AggregatedTestRenderTask {
        Self::new(
            group.unique_id.clone(),
            group.aggregated_test.clone() as Arc<dyn InternalDbtNodeAttributes>,
            group.member_tests.clone(),
            result_sender,
            render_task_hooks,
        )
    }
}

impl Task for AggregatedTestRenderTask {
    // The inner `RenderTask` acquires the connection gate itself and declines
    // the default outer guard, so this wrapper must decline it too.
    fn run_task<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move {
            let spans = self
                .member_tests
                .iter()
                .map(|test| {
                    create_task_span_for_node(
                        test.as_ref(),
                        ExecutionPhase::Render,
                        ctx.inner.span_manager().as_ref(),
                        &ctx.inner.arg.io.in_dir,
                        &ctx.inner.arg.io.out_dir,
                    )
                })
                .collect::<FsResult<Vec<_>>>()?;
            // TODO(pc): the task will show notification via the LSP due to show_progress and leaks
            // error stats.
            let result = self.aggregated_task.run_task(ctx).await;
            for span in spans {
                ctx.inner.span_manager().handle_task_finished(span, &result);
            }
            result
        })
    }

    fn task_type(&self) -> &str {
        <RenderTask as Task>::task_type(self.aggregated_task.as_ref())
    }

    fn resource_type(&self) -> NodeType {
        <RenderTask as Task>::resource_type(self.aggregated_task.as_ref())
    }

    fn work_node_id(&self) -> &str {
        &self.unique_id
    }

    fn dbt_nodes(&self) -> Vec<Arc<dyn InternalDbtNodeAttributes>> {
        self.member_tests
            .iter()
            .cloned()
            .map(|node| node as Arc<dyn InternalDbtNodeAttributes>)
            .collect()
    }

    fn task_phase(&self) -> Option<TP> {
        Some(TP::Render)
    }

    fn telemetry_request(
        &self,
        _in_dir: &Path,
        _out_dir: &Path,
        _skip_reason: Option<&SkipReason>,
    ) -> SpanTreeRequest<FsResult<NodeStatus>, SkipReason> {
        SpanTreeRequest::use_current()
    }
}
