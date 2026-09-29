use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::mpsc;

use crate::renderable::seed::run_seed_render;
use dbt_common::FsResult;
use dbt_common::stats::NodeStatus;
use dbt_schemas::schemas::properties::UnitTestOverrides;
use dbt_schemas::schemas::{DbtSeed, DbtUnitTest, InternalDbtNodeAttributes, Nodes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::render_task_hooks::RenderTaskHooks;
use dbt_tasks_core::task::TP;
use dbt_tasks_core::task::Task;
use dbt_tasks_core::task::TaskResult;
use dbt_telemetry::NodeType;

pub mod aggregated_test;
pub mod common;
pub mod default;
pub mod unit_test;

pub struct RenderTask {
    node: Arc<dyn InternalDbtNodeAttributes>,
    // Channel sender for streaming results to dependent tasks
    result_sender: Option<mpsc::SyncSender<TaskResult>>,
    // Optional unit test overrides for applying macro overrides during rendering to unit test model parents
    local_exec_unit_test_overrides: Option<UnitTestOverrides>,
    // RenderTaskHooks: hooks that run during the RenderTask execution lifecycle, e.g. pre-render, post-render, etc. See `RenderTaskHooks` trait for more details.
    task_hooks: Arc<dyn RenderTaskHooks>,
}

impl RenderTask {
    pub fn new(
        node: Arc<dyn InternalDbtNodeAttributes>,
        result_sender: Option<mpsc::SyncSender<TaskResult>>,
        local_exec_unit_test_overrides: Option<UnitTestOverrides>,
        task_hooks: Arc<dyn RenderTaskHooks>,
    ) -> Self {
        Self {
            node,
            result_sender,
            local_exec_unit_test_overrides,
            task_hooks,
        }
    }
}

impl Task for RenderTask {
    fn run_task<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move {
            // Per-node dev-clone: clone the deferred prod relation into the dev
            // target as a precursor to rendering, so `is_incremental()` in the
            // model body resolves against the cloned table. Mirrors dbt-core's
            // `on_compile` plugin hook; runs concurrently with other nodes' render
            // tasks via the DAG executor. No-op for nodes that aren't dev-clone
            // eligible (non-incremental tables, views, seeds, unit tests, etc.).
            let node_id = self.node.unique_id();
            dbt_tasks_core::run_cache::run_cache_dev_clone::maybe_run_dev_clone_for_node(
                ctx, &node_id,
            )
            .await;

            if self.node.as_any().downcast_ref::<DbtUnitTest>().is_some() {
                unit_test::run_unit_test_render(
                    self.node.clone(),
                    ctx.clone(),
                    self.result_sender.clone(),
                    self.task_hooks.clone(),
                )
                .await
            } else if self.node.as_any().downcast_ref::<DbtSeed>().is_some() {
                run_seed_render(self.node.clone(), ctx.clone(), self.result_sender.clone())
            } else {
                default::run_default_render(
                    self.node.clone(),
                    ctx.clone(),
                    self.result_sender.clone(),
                    self.local_exec_unit_test_overrides.clone(),
                )
                .await
            }
        })
    }

    fn task_type(&self) -> &str {
        "render"
    }

    fn resource_type(&self) -> NodeType {
        self.node.resource_type()
    }

    fn work_node_id(&self) -> &str {
        self.node.common().unique_id.as_str()
    }

    fn dbt_nodes(&self) -> Vec<Arc<dyn InternalDbtNodeAttributes>> {
        let node: Arc<dyn InternalDbtNodeAttributes> = self.node.clone();
        vec![node]
    }

    fn task_phase(&self) -> Option<TP> {
        Some(TP::Render)
    }
}

pub fn is_renderable_task(nodes: &Nodes, unique_id: &str) -> bool {
    nodes.models.contains_key(unique_id)
        || nodes.snapshots.contains_key(unique_id)
        || nodes.tests.contains_key(unique_id)
        || nodes.unit_tests.contains_key(unique_id)
        || nodes.analyses.contains_key(unique_id)
        || nodes.functions.contains_key(unique_id)
}

pub fn renderable_task(
    nodes: &Nodes,
    unique_id: &str,
) -> Option<Arc<dyn InternalDbtNodeAttributes>> {
    if let Some(model) = nodes.models.get(unique_id) {
        Some(model.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else if let Some(snapshot) = nodes.snapshots.get(unique_id) {
        Some(snapshot.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else if let Some(test) = nodes.tests.get(unique_id) {
        Some(test.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else if let Some(seed) = nodes.seeds.get(unique_id) {
        Some(seed.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else if let Some(unit_test) = nodes.unit_tests.get(unique_id) {
        Some(unit_test.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else if let Some(analysis) = nodes.analyses.get(unique_id) {
        Some(analysis.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else if let Some(function) = nodes.functions.get(unique_id) {
        Some(function.clone() as Arc<dyn InternalDbtNodeAttributes>)
    } else {
        None
    }
}
