use dbt_common::FsResult;
use dbt_common::stats::NodeStatus;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::span_manager::SpanTreeRequest;
use dbt_tasks_core::task::{TP, Task};
use dbt_tasks_core::visitor::SkipReason;
use dbt_telemetry::NodeType;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub struct BarrierTask;

impl BarrierTask {
    pub fn new() -> Self {
        Self
    }
}
impl Default for BarrierTask {
    fn default() -> Self {
        Self::new()
    }
}

impl Task for BarrierTask {
    fn run_task<'a>(
        &'a self,
        _ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move { Ok(NodeStatus::Succeeded) })
    }

    fn task_type(&self) -> &str {
        "barrier"
    }

    fn resource_type(&self) -> NodeType {
        NodeType::Unspecified
    }

    fn work_node_id(&self) -> &str {
        "barrier"
    }

    fn dbt_nodes(&self) -> Vec<Arc<dyn InternalDbtNodeAttributes>> {
        Vec::new()
    }

    fn task_phase(&self) -> Option<TP> {
        None
    }

    fn telemetry_request(
        &self,
        _in_dir: &std::path::Path,
        _out_dir: &std::path::Path,
        _skip_reason: Option<&SkipReason>,
    ) -> SpanTreeRequest<FsResult<NodeStatus>, SkipReason> {
        SpanTreeRequest::use_current()
    }
}
