mod model;
mod seed;
mod snapshot;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use dbt_common::FsResult;
use dbt_common::constants::CLONING;
use dbt_common::stats::{NodeStatus, Stat};
use dbt_common::status_reporter::report_completed;
use dbt_schemas::schemas::{InternalDbtNodeAttributes, NodePathKind, Nodes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::{TP, Task};
use dbt_telemetry::NodeType;

pub struct RunCloneTask {
    task: Arc<dyn Cloneable>,
}

impl RunCloneTask {
    pub fn new(task: Arc<dyn Cloneable>) -> Self {
        Self { task }
    }
}

impl Task for RunCloneTask {
    fn run_task<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move {
            let display_path = self
                .task
                .get_node_path(
                    NodePathKind::Definition,
                    ctx.inner.arg.io.in_dir.as_path(),
                    ctx.inner.arg.io.out_dir.as_path(),
                )
                .display()
                .to_string();

            if let Some(reporter) = ctx.inner.arg.io.status_reporter.as_ref() {
                reporter.show_progress(CLONING, display_path.as_ref(), None);
            }

            let result = self.task.visit_run(ctx).await;

            match &result {
                Ok(node_status) => {
                    report_completed(
                        node_status,
                        self.task.defined_at().cloned(),
                        display_path.as_str(),
                        false,
                        ctx.inner.arg.io.status_reporter.as_ref(),
                    );
                }
                Err(_) => {
                    report_completed(
                        &NodeStatus::Errored,
                        self.task.defined_at().cloned(),
                        display_path.as_str(),
                        false,
                        ctx.inner.arg.io.status_reporter.as_ref(),
                    );
                }
            }

            result
        })
    }

    fn task_type(&self) -> &str {
        "run"
    }

    fn resource_type(&self) -> NodeType {
        self.task.resource_type()
    }

    fn work_node_id(&self) -> &str {
        self.task.common().unique_id.as_str()
    }

    fn dbt_nodes(&self) -> Vec<Arc<dyn InternalDbtNodeAttributes>> {
        let node: Arc<dyn InternalDbtNodeAttributes> = self.task.clone();
        vec![node]
    }

    // TODO: is it really a run phase?
    fn task_phase(&self) -> Option<TP> {
        Some(TP::Run)
    }
}

pub trait Cloneable: InternalDbtNodeAttributes {
    fn execute<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>>;

    fn visit_run<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move {
            let start = SystemTime::now();
            let unique_id = &self.common().unique_id;
            let thread_id = ctx.thread_id;

            let task_status = match self.execute(ctx).await {
                Ok(status) => status,
                Err(e) => {
                    // Insert error stats so it appears in run_results.json
                    ctx.inner.run_stats.insert(
                        unique_id.to_string(),
                        Stat::new(
                            unique_id.to_string(),
                            start,
                            None,
                            NodeStatus::Errored,
                            Some(e.to_string()),
                            thread_id,
                        ),
                    );
                    return Err(e);
                }
            };

            ctx.inner.run_stats.insert(
                unique_id.to_string(),
                Stat::new(
                    unique_id.to_string(),
                    start,
                    None,
                    task_status.clone(),
                    None,
                    thread_id,
                ),
            );

            Ok(task_status)
        })
    }
}

pub fn cloneable_task(nodes: &Nodes, unique_id: &str) -> Option<Arc<dyn Cloneable>> {
    if let Some(model) = nodes.models.get(unique_id) {
        Some(model.clone() as Arc<dyn Cloneable>)
    } else if let Some(snapshot) = nodes.snapshots.get(unique_id) {
        Some(snapshot.clone() as Arc<dyn Cloneable>)
    } else if let Some(seed) = nodes.seeds.get(unique_id) {
        Some(seed.clone() as Arc<dyn Cloneable>)
    } else {
        None
    }
}
