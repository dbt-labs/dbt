use std::fmt;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use dbt_common::stats::NodeStatus;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_schemas::schemas::telemetry::{ExecutionPhase, NodeType};

use crate::context::TaskRunnerCtx;
use crate::span_manager::{SpanLevel, SpanTreeRequest};
use crate::task_spans::{
    node_processed_span_key, phase_span_key, task_span_on_close, task_span_on_skip,
    update_node_outcome_from_skip_reason,
};
use crate::visitor::SkipReason;

use dbt_common::collections::DashMap;
use dbt_scheduler::instructions::{LpInstruction, SqlInstruction};
use minijinja::Value as MinijinjaValue;

// Unified result type for all task outputs (render and/or analyze phase)
#[derive(Debug, Clone)]
pub struct TaskResult {
    pub sql_instruction: SqlInstruction,
    pub config_map: Arc<DashMap<String, MinijinjaValue>>,
    /// Present only when the analyze phase ran (Local/Sidecar/Service execution).
    pub lp_instruction: Option<LpInstruction>,
}

/// Stands for Task Phase
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum TP {
    Render,
    Analyze,
    Run,
    Compare,
    Show,
}

impl fmt::Display for TP {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                TP::Render => "render",
                TP::Analyze => "analyze",
                TP::Run => "run",
                TP::Compare => "compare",
                TP::Show => "show",
            }
        )
    }
}

impl From<TP> for ExecutionPhase {
    fn from(tp: TP) -> Self {
        match tp {
            TP::Render => ExecutionPhase::Render,
            TP::Analyze => ExecutionPhase::Analyze,
            TP::Run => ExecutionPhase::Run,
            TP::Show => ExecutionPhase::Run,
            TP::Compare => ExecutionPhase::Compare,
        }
    }
}

/// Awaitable scheduling dispatch for a single unit of work.
///
/// Each variant handles its own thread pool dispatch (spawn_blocking, async).
/// Use this to build purely functional pipelines where each step's output feeds the next:
///
/// ```ignore
/// let relations = TaskOp::blocking(|| discover(ctx)).run().await??;
/// TaskOp::r#async(fetch(relations, ctx)).run().await??;
/// let result = TaskOp::blocking(|| render(ctx, relations)).run().await??;
/// ```
pub enum TaskOp<T: Send + 'static> {
    /// CPU-bound work, dispatched to `dbt_runtime::spawn_blocking`. A
    /// connection the closure borrows stays in that worker's thread-local slot
    /// for whatever runs there next.
    Blocking(Box<dyn FnOnce() -> T + Send>),
}

impl<T: Send + 'static> TaskOp<T> {
    /// Marker: wraps an async future for pipeline visibility.
    /// Just awaits the future — no thread pool dispatch.
    pub async fn r#async<F: Future<Output = T>>(fut: F) -> T {
        fut.await
    }
    /// Marker: runs a sync closure inline on the current async task.
    /// No thread pool dispatch — use for lightweight sync work that
    /// doesn't justify a `spawn_blocking` thread.
    pub fn inline_blocking(f: impl FnOnce() -> T) -> T {
        f()
    }
    /// Execute this operation on the appropriate thread pool.
    pub async fn run(self) -> FsResult<T> {
        match self {
            TaskOp::Blocking(f) => dbt_runtime::spawn_blocking(f)
                .await
                .map_err(|e| fs_err!(ErrorCode::Generic, "spawn_blocking join error: {}", e)),
        }
    }
}

/// Identity of a set of dbt nodes that share a single query.
#[derive(Debug)]
pub struct AggregatedNodeGroup {
    /// The group's synthetic unique_id, used as the group span key.
    pub unique_id: String,
    /// Generic test macro shared by every member of the group, e.g. `not_null`.
    pub macro_name: String,
    /// unique_id of the node the group's tests are attached to.
    pub attached_node: String,
}

pub trait Task: Send + Sync {
    fn run_task<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>>;

    fn resource_type(&self) -> NodeType;

    fn task_type(&self) -> &str;

    /// Returns the identifier that groups together tasks operating on the same logical work node.
    fn work_node_id(&self) -> &str;

    /// Returns the collection of underlying dbt nodes that this task operates on.
    fn dbt_nodes(&self) -> Vec<Arc<dyn InternalDbtNodeAttributes>>;

    /// Returns the task phase if applicable, otherwise None.
    /// As of today only the barrier task does not belong to a phase.
    fn task_phase(&self) -> Option<TP>;

    /// Returns the identity of the aggregated query group whose nodes this task runs, if
    /// any. `None` for every task whose nodes each get their own query.
    ///
    /// Nodes reported here are grouped under a shared span so consumers can treat the
    /// group's lifetime as one unit. Only the task that actually issues the shared query
    /// should report a group.
    fn aggregated_node_group(&self) -> Option<AggregatedNodeGroup> {
        None
    }

    /// Returns telemetry (sub)tree request that task visitor should create for this task
    /// via span manager.
    ///
    /// Uses span keys that reference pre-registered builders in the span manager.
    /// The span manager must be pre-populated with builders for all referenced keys
    /// before this method is called.
    ///
    /// By default:
    /// - Tasks that operate on a single dbt node will run under a `NodeEvaluated` span with:
    ///   - Parent: `PhaseExecuted` span for the task's phase
    ///   - Related: `NodeProcessed` span for the node
    ///   - see `task_spans` module for details.
    /// - Tasks that operate on multiple dbt nodes will request
    ///   visitor to use current visitor span for the task itself.
    ///   Such tasks MUST create similar task-like spans for each underlying node themselves.
    ///
    /// If `skip_reason` is provided, the default implementation applies the same shared skip
    /// mapping used on span skip-close callbacks so `NodeEvaluated` starts with a skipped
    /// outcome already set.
    ///
    /// # Panics
    /// - If task reports zero nodes but doesn't override this method.
    /// - If task reports has no phase but doesn't override this method.
    fn telemetry_request(
        &self,
        in_dir: &Path,
        out_dir: &Path,
        skip_reason: Option<&SkipReason>,
    ) -> SpanTreeRequest<FsResult<NodeStatus>, SkipReason> {
        let mut nodes = self.dbt_nodes().into_iter();

        let Some(node) = nodes.next() else {
            unreachable!("Task should always operate on at least a single node");
        };

        if nodes.next().is_some() {
            // task for multiple nodes use current span as task span.
            // Task inner code should create actual spans for themselves.
            return SpanTreeRequest::use_current();
        }

        let Some(task_phase) = self.task_phase() else {
            unreachable!(
                "Task that doesn't belong to a phase should override telemetry_tree method"
            );
        };

        let phase = task_phase.into();
        let mut attrs = node.get_node_evaluated_event(phase, in_dir, out_dir);
        if let Some(skip_reason) = skip_reason {
            update_node_outcome_from_skip_reason(&mut attrs, skip_reason);
        }

        let task_span_level = match phase {
            ExecutionPhase::Run | ExecutionPhase::Compare => SpanLevel::Info,
            _ => SpanLevel::Debug,
        };
        let mut builder = SpanTreeRequest::builder(
            task_span_level,
            attrs.into(),
            Some(Box::new(task_span_on_close)),
            Some(Box::new(task_span_on_skip)),
        );

        // Reference Phase span as direct parent (must be pre-registered)
        builder.with_parent(SpanLevel::Info, phase_span_key(phase));

        // Reference NodeProcessed span as direct related span (must be pre-registered)
        // Span manager will automatically create its parent (TuiAllProcessingNodesGroup)
        let node_key = node_processed_span_key(&node.unique_id());
        builder.add_related_span(SpanLevel::Info, &node_key);

        builder.build()
    }
}

pub struct TasksForNode {
    pub renderable: Option<Arc<dyn Task>>,
    pub analyzeable: Option<Arc<dyn Task>>,
    pub runnable: Option<Arc<dyn Task>>,
    pub showable: Option<Arc<dyn Task>>,
}
