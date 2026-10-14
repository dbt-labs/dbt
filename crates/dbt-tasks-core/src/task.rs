use std::fmt;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dbt_common::stats::NodeStatus;
use dbt_common::{ErrorCode, FsResult, create_debug_span_with_parent, fs_err};
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_schemas::schemas::telemetry::{ExecutionPhase, NodeType};
use dbt_telemetry::ThreadPoolWait;
use tokio::task::JoinSet;
use tracing::Instrument;

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

/// Queued blocking work waits this long before it reports a `ThreadPoolWait`.
const POOL_WAIT_THRESHOLD: Duration = Duration::from_millis(50);

/// Run blocking task work on the runtime pool, mapping join failures to task errors.
///
/// Also manages the `ThreadPoolWait` span that accounts for time spent
/// waiting on runtime pool contention, used by downstream consumers for node
/// runtime calculations and display.
pub async fn run_blocking_task_operation<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> FsResult<T> {
    run_blocking_task_operation_with_wait_parents(f, None).await
}

/// Like [`run_blocking_task_operation`], but accounts the pool wait to each of
/// `wait_parents` when one operation is shared by several node spans.
///
/// `None` accounts the wait to the current span.
pub async fn run_blocking_task_operation_with_wait_parents<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
    wait_parents: Option<Vec<tracing::span::Id>>,
) -> FsResult<T> {
    let wait_parents: Vec<Option<tracing::span::Id>> = match wait_parents {
        Some(parents) => {
            debug_assert!(
                !parents.is_empty(),
                "explicit wait parents must not be empty"
            );
            parents.into_iter().map(Some).collect()
        }
        None => vec![tracing::Span::current().id()],
    };
    spawn_blocking_task_operation(
        move || {
            wait_parents
                .into_iter()
                .map(|parent| create_debug_span_with_parent(parent, ThreadPoolWait::default()))
                .collect::<Vec<_>>()
        },
        f,
    )
    .await
}

async fn spawn_blocking_task_operation<T: Send + 'static, G: Send + 'static>(
    wait_guard: impl FnOnce() -> G,
    f: impl FnOnce() -> T + Send + 'static,
) -> FsResult<T> {
    dbt_runtime::spawn_blocking_with_wait_guard(POOL_WAIT_THRESHOLD, wait_guard, f)
        .await
        .map_err(|e| fs_err!(ErrorCode::Generic, "spawn_blocking join error: {}", e))
}

/// Runs blocking operations of one evaluation concurrently on the runtime pool.
///
/// Each operation is the span its work runs in and the work itself. Returns
/// the results in completion order, or the first error once every operation
/// has finished. After the first error, operations that have not started yet
/// still run their work, but get `canceled` set so they can skip it and
/// report that outcome. Operations already running finish normally.
///
/// Each operation accounts its own pool wait to its span, like
/// [`run_blocking_task_operation`]. Those waits may overlap. The span current at
/// the call gets a separate `ThreadPoolWait` for the time when submitted
/// operations are waiting and none is running, so the evaluation's own waits
/// never overlap and cover exactly the time it makes no progress.
///
/// If this call is dropped before it returns, operations still queued get
/// `canceled` set and are no longer accounted to the evaluation.
pub async fn run_concurrent_task_operations<T, W>(
    operations: impl IntoIterator<Item = (tracing::Span, W)>,
) -> FsResult<Vec<T>>
where
    T: Send + 'static,
    W: FnOnce(bool) -> FsResult<T> + Send + 'static,
{
    let evaluation = Arc::new(EvaluationWait::new(tracing::Span::current()));
    let canceled = Arc::new(AtomicBool::new(false));
    let _scope = OperationsScope {
        evaluation: Arc::clone(&evaluation),
        canceled: Arc::clone(&canceled),
    };
    let mut operations = operations
        .into_iter()
        .map(|(span, work)| {
            let wait_parent = span.id();
            let waiting = Arc::clone(&evaluation);
            let running = Arc::clone(&evaluation);
            let canceled = Arc::clone(&canceled);
            spawn_blocking_task_operation(
                move || {
                    (
                        create_debug_span_with_parent(wait_parent, ThreadPoolWait::default()),
                        OperationStateGuard::enter(waiting, OperationState::Waiting),
                    )
                },
                move || {
                    let _running = OperationStateGuard::enter(running, OperationState::Running);
                    let result = work(canceled.load(Ordering::Relaxed));
                    // Set before this worker frees up, so no queued operation
                    // starts unaware of the failure.
                    if result.is_err() {
                        canceled.store(true, Ordering::Relaxed);
                    }
                    result
                },
            )
            .instrument(span)
        })
        .collect::<JoinSet<_>>();

    let mut results = Vec::with_capacity(operations.len());
    let mut first_error = None;
    while let Some(result) = operations.join_next().await {
        match result.map_err(Into::into).flatten().flatten() {
            Ok(value) => results.push(value),
            Err(error) => {
                canceled.store(true, Ordering::Relaxed);
                first_error.get_or_insert(error);
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(results),
    }
}

/// State of an operation of [`run_concurrent_task_operations`] once submitted.
#[derive(Clone, Copy)]
enum OperationState {
    /// Queued past the wait threshold.
    Waiting,
    Running,
}

/// The evaluation wait of [`run_concurrent_task_operations`], shared by its operations.
struct EvaluationWait(Mutex<EvaluationWaitState>);

struct EvaluationWaitState {
    /// Evaluation span the wait is accounted to, `None` once it no longer accepts waits.
    parent: Option<tracing::Span>,
    waiting: usize,
    running: usize,
    wait: Option<tracing::Span>,
}

impl EvaluationWait {
    fn new(parent: tracing::Span) -> Self {
        Self(Mutex::new(EvaluationWaitState {
            parent: Some(parent),
            waiting: 0,
            running: 0,
            wait: None,
        }))
    }

    fn increment(&self, state: OperationState) {
        self.update(state, true);
    }

    fn decrement(&self, state: OperationState) {
        self.update(state, false);
    }

    /// Counts one operation entering or leaving `state`, then opens the wait
    /// while operations wait and none runs, and closes it otherwise.
    fn update(&self, state: OperationState, increment: bool) {
        let mut underflow = false;
        let closed_wait = {
            let mut guard = self.0.lock().expect("evaluation wait lock poisoned");
            let wait_state = &mut *guard;
            let count = match state {
                OperationState::Waiting => &mut wait_state.waiting,
                OperationState::Running => &mut wait_state.running,
            };
            if increment {
                *count += 1;
            } else {
                underflow = *count == 0;
                *count = count.saturating_sub(1);
            }
            let idle = wait_state.running == 0 && wait_state.waiting > 0;
            match &wait_state.parent {
                Some(parent) if idle => {
                    if wait_state.wait.is_none() {
                        wait_state.wait = Some(create_debug_span_with_parent(
                            parent.id(),
                            ThreadPoolWait::default(),
                        ));
                    }
                    None
                }
                _ => wait_state.wait.take(),
            }
        };
        // Closing the span runs telemetry callbacks; keep them outside the lock.
        drop(closed_wait);
        // Asserted outside the lock, so a failure cannot poison it.
        debug_assert!(!underflow, "operation state count underflow");
    }

    /// Ends the wait and stops accounting further waits to the evaluation.
    fn close(&self) {
        let (wait, parent) = {
            let mut wait_state = self.0.lock().expect("evaluation wait lock poisoned");
            (wait_state.wait.take(), wait_state.parent.take())
        };
        drop(wait);
        drop(parent);
    }
}

/// Cancels operations not yet started and closes the evaluation wait when
/// [`run_concurrent_task_operations`] returns or is dropped.
struct OperationsScope {
    evaluation: Arc<EvaluationWait>,
    canceled: Arc<AtomicBool>,
}

impl Drop for OperationsScope {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Relaxed);
        self.evaluation.close();
    }
}

/// Counts one operation in `state` while alive.
struct OperationStateGuard {
    evaluation: Arc<EvaluationWait>,
    state: OperationState,
}

impl OperationStateGuard {
    fn enter(evaluation: Arc<EvaluationWait>, state: OperationState) -> Self {
        evaluation.increment(state);
        Self { evaluation, state }
    }
}

impl Drop for OperationStateGuard {
    fn drop(&mut self) {
        self.evaluation.decrement(self.state);
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
