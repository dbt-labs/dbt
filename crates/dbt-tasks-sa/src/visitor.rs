use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use petgraph::algo::{Cycle, toposort};
use petgraph::{
    Direction,
    graph::{DiGraph, NodeIndex},
    visit::EdgeRef,
};
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::Instrument;

use dbt_common::cancellation::{CancellationToken, CancelledError};
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::stats::{NodeStatus, Stat};
use dbt_common::tracing::dbt_emit::{emit_error_log_from_fs_error, emit_trace_log_message};
use dbt_common::{ErrorCode, FsError, fs_err, status_reporter::report_completed};
use dbt_common::{FsResult, io_args::IoArgs, unexpected_err};
use dbt_schemas::schemas::common::OnError;
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes, NodePathKind};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::TP;
use dbt_tasks_core::task::Task;
use dbt_tasks_core::visitor::SkipReason;
use dbt_telemetry::{ExecutionPhase, NodeOutcome, NodeType};
use dbt_yaml::Span;

/// Pool of reusable logical worker slot IDs (1, 2, 3, …).
///
/// Tasks are assigned a slot before execution and release it after.
/// The slot number appears in stats as `"Thread-{N}"` and in Jinja as `{{ thread_id }}`.
/// Uses a min-heap so `acquire` always returns the smallest available slot.
struct WorkerSlotPool {
    next_id: i32,
    free: std::collections::BinaryHeap<std::cmp::Reverse<i32>>,
}

impl WorkerSlotPool {
    fn new() -> Self {
        let mut free = std::collections::BinaryHeap::new();
        free.push(std::cmp::Reverse(1));
        Self { next_id: 2, free }
    }

    fn acquire(&mut self) -> i32 {
        self.free
            .pop()
            .map(|std::cmp::Reverse(id)| id)
            .unwrap_or_else(|| {
                let id = self.next_id;
                self.next_id += 1;
                id
            })
    }

    fn release(&mut self, id: i32) {
        self.free.push(std::cmp::Reverse(id));
    }
}

type SkippedNodes = Vec<Arc<dyn InternalDbtNodeAttributes>>;

// Handle skipping of the task
struct SkipSet {
    // Map of node index to skip reason. If upstream node failed, the reason is the first failed upstream node's unique_id.
    skip: HashMap<NodeIndex, SkipReason>,
}
impl SkipSet {
    fn new() -> Self {
        Self {
            skip: HashMap::new(),
        }
    }

    fn mark_skip(&mut self, task_node_idx: NodeIndex, reason: SkipReason) {
        let _ = self.skip.entry(task_node_idx).or_insert(reason);
    }

    // Add proper task_node_id to self.skip with reason.
    // return nodes skipped
    fn propagate_failure(
        &mut self,
        task_node_idx: NodeIndex,
        dependents: &[HashSet<NodeIndex>],
        schedule: &DiGraph<Arc<dyn Task>, ()>,
    ) -> SkippedNodes {
        let task = schedule
            .node_weight(task_node_idx)
            .expect("node do not exist");
        let work_node_id = task.work_node_id().to_string();
        let work_nodes = task.dbt_nodes();

        // Initialize tracking collections
        let mut stack = vec![task_node_idx];
        let mut seen_work_node_ids = HashSet::new();
        let mut seen_task_ids = HashSet::new();
        let mut seen_dbt_node_ids = HashSet::new();
        let mut skipped_dbt_nodes = Vec::new();

        // Mark initial node as seen
        seen_work_node_ids.insert(work_node_id.clone());
        seen_task_ids.insert(task_node_idx);
        for node in &work_nodes {
            seen_dbt_node_ids.insert(node.common().unique_id.clone());
        }

        // Process nodes in depth-first order
        while let Some(current_task_idx) = stack.pop() {
            for &dependent_task_idx in &dependents[current_task_idx.index()] {
                let dependent_task = schedule
                    .node_weight(dependent_task_idx)
                    .expect("task should exist.");
                let dep_work_node_id = dependent_task.work_node_id().to_string();

                // Skip barrier nodes
                if dependent_task.task_type() == "barrier" {
                    continue;
                }

                // Add skip for this node
                // Determine if this is a phase skip (same node) or upstream skip (different node)
                let skip_reason = if work_node_id == dep_work_node_id {
                    SkipReason::FailedPhase
                } else {
                    SkipReason::FailedUpstream(work_node_id.clone())
                };
                self.mark_skip(dependent_task_idx, skip_reason);

                // Only process each unique ID once
                if seen_work_node_ids.insert(dep_work_node_id.clone()) {
                    let dependent_nodes = dependent_task.dbt_nodes();

                    for dbt_node in dependent_nodes {
                        let node_ref = dbt_node.as_ref();
                        let dbt_node_common = node_ref.common();
                        let unique_id = dbt_node_common.unique_id.clone();
                        // continue if we already handled it
                        if !seen_dbt_node_ids.insert(unique_id.clone()) {
                            continue;
                        } else {
                            skipped_dbt_nodes.push(dbt_node.clone());
                        }
                    }
                }

                // Continue traversing if we haven't seen this node before
                if seen_task_ids.insert(dependent_task_idx) {
                    stack.push(dependent_task_idx);
                }
            }
        }
        skipped_dbt_nodes
    }

    /// SAFETY (loop termination): when a test's first phase is marked as reused, ALL
    /// subsequent phases of the same test must also be marked. Otherwise the later phases
    /// will be spawned and block forever waiting for results the skipped phase never
    /// produced, causing the main loop's `recv().await` to hang. See the termination
    /// proof comment on the main loop and #8439.
    fn propagate_reuse_to_downstream_tests(
        &mut self,
        task_node_idx: NodeIndex,
        dependents: &[HashSet<NodeIndex>],
        schedule: &DiGraph<Arc<dyn Task>, ()>,
    ) -> SkippedNodes {
        let mut reused_nodes = Vec::new();
        let task_dependents = &dependents[task_node_idx.index()];

        // Reuse propagation is only for tests: models and snapshots rerun or reuse themselves.
        for &dependent_task_idx in task_dependents {
            if let Some(task) = schedule.node_weight(dependent_task_idx) {
                if task.task_type() == "barrier" || task.resource_type() != NodeType::Test {
                    continue;
                }

                // Skip the test task itself
                self.mark_skip(dependent_task_idx, SkipReason::Reused);
                for dbt_node in task.dbt_nodes() {
                    reused_nodes.push(dbt_node);
                }

                // Find all descendant test tasks (from subsequent phases) and ensure they are also marked as reused.
                // This is necessary because task graph doesn't guarantee that all tests tasks directly
                // depend on the upstream model/snapshot task, e.g. at the time of writing, in baseline
                // mode test would get an Analyze task that depends on test Render, but not on the upstream
                // model Run (which causes this propagation logic in the first place).
                let test_work_node_id = task.work_node_id();
                let mut stack = vec![dependent_task_idx];
                let mut seen_task_ids = HashSet::from([dependent_task_idx]);

                while let Some(current_task_idx) = stack.pop() {
                    for &downstream_task_idx in &dependents[current_task_idx.index()] {
                        let Some(downstream_task) = schedule.node_weight(downstream_task_idx)
                        else {
                            continue;
                        };

                        // Not the same test, skip
                        if downstream_task.work_node_id() != test_work_node_id {
                            continue;
                        }

                        // Same test, different phase. Add if not among direct task deps
                        if !task_dependents.contains(&downstream_task_idx) {
                            self.mark_skip(downstream_task_idx, SkipReason::Reused);
                            if seen_task_ids.insert(downstream_task_idx) {
                                stack.push(downstream_task_idx);
                            }
                        }
                    }
                }
            }
        }
        reused_nodes
    }

    // Handle different task outcomes consistently.
    // Returns (failed_nodes, reused_nodes).
    fn handle_task_result(
        &mut self,
        result: FsResult<NodeStatus>,
        task_idx: NodeIndex,
        dependents: &[HashSet<NodeIndex>],
        schedule: &DiGraph<Arc<dyn Task>, ()>,
        fail_fast_flag: bool,
        reuse_downstream_tests: bool,
    ) -> (SkippedNodes, SkippedNodes) {
        // `on_error: continue` — skip failure propagation so downstreams
        // keep running. Handles both `Ok(Errored)` (normal run failure) and
        // `Err(_)` (e.g. microbatch failures before normalization).
        let is_failure = matches!(&result, Err(_) | Ok(NodeStatus::Errored));
        if is_failure && !fail_fast_flag && model_should_continue_on_error(task_idx, schedule) {
            return (Vec::new(), Vec::new());
        }
        match result {
            Ok(node_status) => match node_status {
                NodeStatus::Errored => (
                    self.propagate_failure(task_idx, dependents, schedule),
                    Vec::new(),
                ),
                NodeStatus::ReusedNoChanges(_)
                | NodeStatus::ReusedStillFresh(_, _, _)
                | NodeStatus::ReusedStillFreshNoChanges(_)
                | NodeStatus::ReusedCloned(_) => (
                    Vec::new(),
                    if reuse_downstream_tests {
                        // TODO: Unfortunately, we have diverging logic for data tests vs all other node types,
                        // when it comes to upstream being reused. All other node tasks will run and just
                        // report themselves as reused, but for tests we have historically propagated skips
                        // in the visitor. This should be unified eventually.
                        self.propagate_reuse_to_downstream_tests(task_idx, dependents, schedule)
                    } else {
                        Vec::new()
                    },
                ),
                NodeStatus::Succeeded
                | NodeStatus::SucceededWithWarning
                | NodeStatus::TestPassed
                | NodeStatus::StaticallyCheckedDataTest
                | NodeStatus::TestWarned
                | NodeStatus::NoOp => (Vec::new(), Vec::new()),
                NodeStatus::SkippedUpstreamFailed => {
                    unreachable!("this should be handled somewhere else")
                }
            },
            Err(_) => (
                self.propagate_failure(task_idx, dependents, schedule),
                Vec::new(),
            ),
        }
    }
}

// Returns true when the failing task is a model with `on_error: continue`.
//
// Phase gating: honored for Render and Run failures, but **not** Analyze.
// Analyze produces the type/binding facts that `--static-analysis strict`
// downstreams consume, so upstream Analyze failures must still propagate.
//
// TODO: The correct rule is per-downstream — propagate Render/Analyze
// failure only to downstreams whose `static_analysis` is `strict`. The
// current phase-level guard is a pragmatic approximation.
//
// `--fail-fast` overrides this: callers gate with `!fail_fast_flag`.
fn model_should_continue_on_error(
    task_idx: NodeIndex,
    schedule: &DiGraph<Arc<dyn Task>, ()>,
) -> bool {
    let Some(task) = schedule.node_weight(task_idx) else {
        return false;
    };
    if !matches!(task.resource_type(), NodeType::Model) {
        return false;
    }
    if matches!(task.task_phase(), Some(TP::Analyze)) {
        return false;
    }
    task.dbt_nodes().iter().any(|node| {
        node.as_any()
            .downcast_ref::<DbtModel>()
            .is_some_and(|m| matches!(m.deprecated_config.on_error, Some(OnError::Continue)))
    })
}

fn record_skipped_stats(
    ctx: &TaskRunnerCtx,
    skipped_nodes: &[Arc<dyn InternalDbtNodeAttributes>],
    skip_reason: &SkipReason,
) {
    let now = chrono::Utc::now();
    for node in skipped_nodes {
        let unique_id = node.common().unique_id.clone();
        let (status, message) = match skip_reason {
            SkipReason::Reused => (
                NodeStatus::ReusedNoChanges("Model reused".to_string()),
                Some("Skipped due to model reuse".to_string()),
            ),
            SkipReason::FailedUpstream(_) | SkipReason::FailedPhase => (
                NodeStatus::SkippedUpstreamFailed,
                Some("Skipped due to upstream failure".to_string()),
            ),
        };
        // Insert stats for skipped nodes so they appear in run_results.json
        ctx.inner.run_stats.insert(
            unique_id.clone(),
            Stat::new(
                unique_id,
                now.into(),
                None,
                status,
                message,
                0, // thread_id - use 0 as skipped nodes weren't actually executed
            ),
        );
    }
}

fn task_graph_cycle_error(
    cycle: Cycle<NodeIndex>,
    schedule: &DiGraph<Arc<dyn Task>, ()>,
) -> Box<FsError> {
    let cycle_node = schedule
        .node_weight(cycle.node_id())
        .and_then(|task| task.dbt_nodes().first().cloned());
    let cycle_node_unique_id = cycle_node
        .as_ref()
        .map(|node| node.common().unique_id.as_str())
        .unwrap_or("unknown");

    Box::new(FsError::new(
        ErrorCode::Unexpected,
        format!(
            "Internal error: task graph has cycles. Node participating in the cycle: {cycle_node_unique_id}.\nPlease report a bug at: https://github.com/dbt-labs/dbt-fusion/issues"
        ),
    ))
}

fn show_skip_summary(io: &IoArgs, skipped_nodes: &[Arc<dyn InternalDbtNodeAttributes>]) {
    if skipped_nodes.is_empty() {
        return;
    }

    for node in skipped_nodes {
        let unique_id = &node.common().unique_id;

        // Show completed message for non-test nodes
        if !unique_id.starts_with("test.") && !unique_id.starts_with("unit_test.") {
            report_completed(
                &NodeStatus::SkippedUpstreamFailed,
                None,
                &node
                    .get_node_path(
                        NodePathKind::Definition,
                        io.in_dir.as_path(),
                        io.out_dir.as_path(),
                    )
                    .display()
                    .to_string(),
                false,
                io.status_reporter.as_ref(),
            );
        }
    }
}

#[allow(clippy::cognitive_complexity)]
pub async fn visit_sequential(
    io: &IoArgs,
    schedule: &DiGraph<Arc<dyn Task>, ()>,
    ctx: &mut TaskRunnerCtx,
    token: &CancellationToken,
) -> FsResult<()> {
    visit(io, schedule, ctx, VisitStrategy::Sequential, token).await
}

#[allow(clippy::cognitive_complexity)]
pub async fn visit_parallel(
    io: &IoArgs,
    schedule: &DiGraph<Arc<dyn Task>, ()>,
    ctx: &mut TaskRunnerCtx,
    token: &CancellationToken,
) -> FsResult<()> {
    visit(io, schedule, ctx, VisitStrategy::Parallel, token).await
}

fn get_indegree_and_dependents(
    schedule: &DiGraph<Arc<dyn Task>, ()>,
) -> (Vec<i32>, Vec<HashSet<NodeIndex>>) {
    let mut indegree = vec![0; schedule.node_count()];
    let mut dependents = vec![HashSet::<NodeIndex>::new(); schedule.node_count()];

    for edge in schedule.edge_references() {
        indegree[edge.target().index()] += 1;
        dependents[edge.source().index()].insert(edge.target());
    }

    (indegree, dependents)
}

#[derive(Clone, Copy)]
enum VisitStrategy {
    Sequential,
    Parallel,
}

impl VisitStrategy {
    fn new_pending_nodes(self, schedule: &DiGraph<Arc<dyn Task>, ()>) -> FsResult<Vec<NodeIndex>> {
        // Invariant check - task graph must be a DAG.
        // We use `toposort` to check for cycles as it uses iterative algorithm,
        // while pergraph::algo::is_cyclic_directed uses recursive which may cause stack overflow on large graphs.
        let sorted_nodes =
            toposort(schedule, None).map_err(|cycle| task_graph_cycle_error(cycle, schedule))?;

        let mut pending = vec![];
        if matches!(self, VisitStrategy::Parallel) {
            // Initialize worklist with nodes that have an indegree of 0
            pending.extend(schedule.externals(Direction::Incoming));
        } else {
            pending.extend(sorted_nodes.iter().rev().copied());
        }
        Ok(pending)
    }

    /// If not parallel, returns empty.
    fn get_incomplete_tasks(
        self,
        schedule: &DiGraph<Arc<dyn Task>, ()>,
        indegree: &[i32],
    ) -> Vec<(String, String)> {
        if !matches!(self, VisitStrategy::Parallel) {
            return vec![];
        }

        let mut incomplete_tasks = Vec::new();
        for (i, &degree) in indegree.iter().enumerate() {
            if degree > 0
                && let Some(task) = schedule.node_weight(NodeIndex::new(i))
            {
                incomplete_tasks.push((
                    task.work_node_id().to_string(),
                    task.task_type().to_string(),
                ));
            }
        }
        incomplete_tasks
    }

    /// If not parallel, will return [None] if there are nodes waiting.
    fn try_pop_pending_node(
        self,
        waiting: &HashMap<NodeIndex, tracing::Span>,
        pending: &mut Vec<NodeIndex>,
    ) -> Option<NodeIndex> {
        if !matches!(self, VisitStrategy::Parallel) && !waiting.is_empty() {
            return None;
        }
        pending.pop()
    }

    /// Decrement the indegree of each dependent of `node_index`, pushing any that
    /// reach zero onto `pending`. Only relevant in parallel mode, where indegree
    /// gates when a node becomes ready.
    fn release_dependents(
        self,
        node_index: NodeIndex,
        dependents: &[HashSet<NodeIndex>],
        indegree: &mut [i32],
        pending: &mut Vec<NodeIndex>,
    ) {
        if matches!(self, VisitStrategy::Parallel) {
            for &dependent in &dependents[node_index.index()] {
                indegree[dependent.index()] -= 1;
                if indegree[dependent.index()] == 0 {
                    pending.push(dependent);
                }
            }
        }
    }

    fn spawn_task(
        self,
        mut ctx: TaskRunnerCtx,
        task_span: &tracing::Span,
        node: Arc<dyn Task>,
    ) -> JoinHandle<Result<NodeStatus, Box<FsError>>> {
        tokio::spawn(
            async move {
                // Note: this send may fail if the main loop gets terminated
                // by should_cancel_compilation, in which case we simply
                // ignore the error
                node.run_task(&mut ctx).await
            }
            // Instrument the task with the span and assign parent
            .instrument(task_span.clone()),
        )
    }
}

#[allow(clippy::cognitive_complexity)]
async fn visit(
    io: &IoArgs,
    schedule: &DiGraph<Arc<dyn Task>, ()>,
    ctx: &mut TaskRunnerCtx,
    strategy: VisitStrategy,
    token: &CancellationToken,
) -> FsResult<()> {
    let mut slot_pool = WorkerSlotPool::new();
    // Work items yet to be started:
    let mut pending = strategy.new_pending_nodes(schedule)?;
    // Work items that are in progress:
    let mut waiting = HashMap::<NodeIndex, tracing::Span>::new();
    // Work items that need to be skipped
    let mut skip_set = SkipSet::new();

    // Used in span creation
    let span_manager = ctx.inner.span_manager();

    // Channel for completion events
    let (sender, mut receiver) = mpsc::unbounded_channel();

    let (mut indegree, dependents) = get_indegree_and_dependents(schedule);

    let handle_cancellation = |ctx: &mut TaskRunnerCtx,
                               e: CancelledError,
                               waiting: HashMap<NodeIndex, _>|
     -> FsResult<()> {
        // Any tasks that are running and waiting need to be reported as failed.
        for (node_idx, task_span) in waiting {
            let maybe_node = schedule.node_weight(node_idx);
            if let Some(node) = maybe_node {
                report_node_evaluation(ctx, node.as_ref(), Some(&NodeStatus::Errored));
            }
            // Record span result
            span_manager.handle_task_finished(task_span, &Ok(NodeStatus::Errored));
        }
        return Err(e.into());
    };

    // Termination proof for the loop below
    // ======================================
    // Measure: M = N - processed, where N = node count and "processed" means a node
    // was either skipped (in the inner while-loop) or its completion was received from
    // the channel.
    //
    // 1. Each node enters `pending` at most once: after initialization `indegree[v]`
    //    is only decremented (lines that do `-= 1`), never incremented, so the
    //    `== 0` guard that triggers `pending.push` fires at most once per node.
    //
    // 2. The inner while-loop terminates: it pops from `pending`, and by (1) at most
    //    N items ever enter `pending`.
    //
    // 3. Every spawned task sends exactly one completion message: the wrapper task
    //    catches panics via JoinError and always calls `sender.send(...)`.
    //    Therefore `receiver.recv().await` returns whenever `waiting` is non-empty.
    //
    // 4. On every non-breaking iteration at least one node is processed:
    //    - If `pending` was non-empty: the inner loop processes ≥1 node (skip or
    //      spawn). If any were spawned, `waiting` is non-empty and we receive one
    //      completion (+1 processed). If all were skipped and `waiting` is empty,
    //      the break condition fires.
    //    - If `pending` was empty and `waiting` non-empty: we receive one completion.
    //    - If both empty: we break.
    //
    // Since M is a non-negative integer that strictly decreases on every non-breaking
    // iteration, the loop terminates in at most N iterations.
    //
    // CRITICAL ASSUMPTION: every spawned task terminates in finite time. If skip_set
    // skips a preceding phase (e.g. test/render) but fails to also skip a dependent
    // phase (e.g. test/analyze), the dependent may block forever waiting for results
    // that were never produced — violating assumption (3) and causing the loop to
    // hang. This is why `propagate_reuse_to_downstream_tests` must mark ALL phases
    // of a reused test, not just the direct dependent. See #8439.
    loop {
        if let Err(e) = token.check_cancellation() {
            return handle_cancellation(ctx, e, waiting);
        }

        while let Some(node_index) = strategy.try_pop_pending_node(&waiting, &mut pending) {
            if let Some(node) = schedule.node_weight(node_index) {
                let skip_reason = skip_set.skip.get(&node_index);
                let task_span = span_manager.get_task_span(node.telemetry_request(
                    &io.in_dir,
                    &io.out_dir,
                    skip_reason,
                ))?;

                if let Some(skip_reason) = skip_reason {
                    // If the node is skipped, we need to decrement the indegree of its dependents
                    strategy.release_dependents(
                        node_index,
                        &dependents,
                        &mut indegree,
                        &mut pending,
                    );

                    // If node is skipped, record skipped result and close it immediately
                    span_manager.handle_task_skipped(task_span, skip_reason);
                    continue;
                }
                let sender = sender.clone();
                let node_clone = node.clone();
                let mut ctx_clone = ctx.clone();
                let thread_id = slot_pool.acquire();
                ctx_clone.thread_id = thread_id;

                let work_node_id_for_send = node_clone.work_node_id().to_string();

                // Spawn the actual task and instrument it with the span. We then await the
                // JoinHandle in a second task so that panics inside run_task are caught via
                // JoinError rather than propagating out of the async block and silently
                // dropping the sender. If the sender were dropped without a send, the main
                // loop's `receiver.recv().await` would block forever because the original
                // `sender` kept the channel open.
                let task_handle = strategy.spawn_task(ctx_clone, &task_span, node_clone);

                // Spawn under current visitor span, not task span
                dbt_common::tracing::spawn_traced(async move {
                    // Await the task handle; convert a panic (JoinError) into an FsError so
                    // the main loop always receives exactly one message per spawned task.
                    let result = match task_handle.await {
                        Ok(r) => r,
                        Err(join_err) => unexpected_err!(
                            "Task '{}' panicked during parallel execution: {}",
                            work_node_id_for_send,
                            join_err
                        ),
                    };
                    // Note: this send may fail if the main loop gets terminated
                    // by should_cancel_compilation, in which case we simply
                    // ignore the error
                    let _ = sender.send((result, node_index, thread_id));
                });
                waiting.insert(node_index, task_span);
            } else {
                return unexpected_err!("Node {:?} not found in schedule", node_index);
            };
        }

        // Process the next completed task
        if !waiting.is_empty() {
            if let Err(e) = token.check_cancellation() {
                return handle_cancellation(ctx, e, waiting);
            }

            let (result, node_idx, thread_id) = match receiver.recv().await {
                Some((res, node_idx, thread_id)) => (res, node_idx, thread_id),
                None => return unexpected_err!("Receiver channel closed"),
            };

            slot_pool.release(thread_id);

            let maybe_node = schedule.node_weight(node_idx);
            if let Some(node) = maybe_node {
                report_node_evaluation(ctx, node.as_ref(), result.as_ref().ok());
            }

            let task_span = waiting
                .remove(&node_idx)
                .expect("Completed node was not in waiting set");
            if let Err(ref e) = result {
                // Ensure log message inherits NodeEvaluated context (e.g., unique_id).
                task_span.in_scope(|| {
                    emit_error_log_from_fs_error(e.clone_without_backtrace());
                });
            }

            // Record span result
            span_manager.handle_task_finished(task_span, &result);

            // Invalidate upstream model caches when a test fails
            if matches!(result.as_ref(), Ok(NodeStatus::Errored)) {
                if let Some(node) = maybe_node.as_ref() {
                    ctx.on_test_failure(node).await;
                }
            }

            let is_error = matches!(result, Err(_) | Ok(NodeStatus::Errored));
            let (failed_nodes, reused_nodes) = skip_set.handle_task_result(
                result,
                node_idx,
                &dependents,
                schedule,
                ctx.inner.arg.fail_fast_flag,
                should_reuse_downstream_tests(ctx),
            );
            if let Some(node) = maybe_node {
                report_skipped_node_evaluations(ctx, node.as_ref(), &failed_nodes);
            }
            record_skipped_stats(ctx, &failed_nodes, &SkipReason::FailedPhase);
            record_skipped_stats(ctx, &reused_nodes, &SkipReason::Reused);
            show_skip_summary(io, &failed_nodes);
            if is_error {
                ctx.inner.arg.fail_fast.trigger();
            }

            // Decrement the indegree of its dependents
            strategy.release_dependents(node_idx, &dependents, &mut indegree, &mut pending);
        }

        // All done if nothing is in the worklist and nothing is waiting
        if pending.is_empty() && waiting.is_empty() {
            break;
        }
    }

    // Check if all tasks have been completed
    let incomplete_tasks = strategy.get_incomplete_tasks(schedule, &indegree);
    if !incomplete_tasks.is_empty() {
        let count = incomplete_tasks.len();

        // Report all unprocessed tasks in trace logs
        emit_trace_log_message(|| {
            format!(
                "Unprocessed task unique_ids: {}",
                incomplete_tasks
                    .iter()
                    .map(|(id, typ)| format!("{} (type: {})", id, typ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });

        // Hard fail if there are unprocessed tasks
        return Err(fs_err!(
            ErrorCode::Unexpected,
            "Internal error: {count} tasks were not processed.\nPlease report a bug at: https://github.com/dbt-labs/dbt-fusion/issues"
        ));
    }

    Ok(())
}

fn should_reuse_downstream_tests(ctx: &TaskRunnerCtx) -> bool {
    let state_data_tests_enabled = ctx
        .inner
        .run_cache_ctx
        .run_cache_service_config
        .as_ref()
        .is_some_and(|config| config.enable_data_tests);

    // When dbt State data-test reuse is enabled, let each test consult its own cached result.
    !(ctx.inner.run_cache_ctx.run_cache_service_requested && state_data_tests_enabled)
}

fn get_execution_phase_from_task(node: &dyn Task) -> ExecutionPhase {
    match node.task_phase() {
        Some(TP::Render) => ExecutionPhase::Render,
        Some(TP::Analyze) => ExecutionPhase::Analyze,
        Some(TP::Compare) => ExecutionPhase::Compare,
        Some(TP::Run) => ExecutionPhase::Run,
        Some(TP::Show) => ExecutionPhase::Analyze,
        _ => ExecutionPhase::Unspecified,
    }
}

fn report_node_evaluation(
    ctx: &mut TaskRunnerCtx,
    node: &dyn Task,
    node_status: Option<&NodeStatus>,
) {
    if let Some(reporter) = &ctx.inner.arg.io.status_reporter {
        // For successful status, emit a node evaluation event
        let node_outcome = match node_status {
            Some(NodeStatus::Succeeded | NodeStatus::SucceededWithWarning) => NodeOutcome::Success,
            _ => NodeOutcome::Error,
        };
        for dbt_node in node.dbt_nodes().iter() {
            reporter.collect_node_evaluation(
                &dbt_node.common().unique_id,
                get_execution_phase_from_task(node),
                node_outcome,
                None,
                dbt_node.static_analysis().into_inner(),
                (None, Span::default()),
            );
        }
    }
}

fn report_skipped_node_evaluations(
    ctx: &mut TaskRunnerCtx,
    node: &dyn Task,
    skipped_nodes: &[Arc<dyn InternalDbtNodeAttributes>],
) {
    if let Some(reporter) = &ctx.inner.arg.io.status_reporter {
        let upstream_target = node.dbt_nodes().into_iter().next().map(|upstream_node| {
            let common = upstream_node.common();
            (common.package_name.clone(), common.name.clone(), true)
        });
        for skipped_node in skipped_nodes.iter() {
            reporter.collect_node_evaluation(
                &skipped_node.common().unique_id,
                get_execution_phase_from_task(node),
                NodeOutcome::Skipped,
                upstream_target.clone(),
                StaticAnalysisKind::Off,
                (None, Span::default()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::nodes::{DbtModel, DbtTest};
    use std::future::Future;
    use std::pin::Pin;

    struct TestTask {
        resource_type: NodeType,
        task_type: &'static str,
        work_node_id: String,
        nodes: Vec<Arc<dyn InternalDbtNodeAttributes>>,
    }

    impl Task for TestTask {
        fn run_task<'a>(
            &'a self,
            _ctx: &'a mut TaskRunnerCtx,
        ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
            Box::pin(async { Ok(NodeStatus::Succeeded) })
        }

        fn resource_type(&self) -> NodeType {
            self.resource_type
        }

        fn task_type(&self) -> &str {
            self.task_type
        }

        fn work_node_id(&self) -> &str {
            &self.work_node_id
        }

        fn dbt_nodes(&self) -> Vec<Arc<dyn InternalDbtNodeAttributes>> {
            self.nodes.clone()
        }

        fn task_phase(&self) -> Option<TP> {
            Some(TP::Run)
        }
    }

    fn model_node() -> Arc<dyn InternalDbtNodeAttributes> {
        let mut model = DbtModel::default();
        model.__common_attr__.unique_id = "model.test.orders".to_string();
        Arc::new(model)
    }

    fn test_node() -> Arc<dyn InternalDbtNodeAttributes> {
        let mut test = DbtTest::default();
        test.__common_attr__.unique_id = "test.test.accepted_values_orders".to_string();
        Arc::new(test)
    }

    fn model_with_downstream_test_schedule() -> (
        DiGraph<Arc<dyn Task>, ()>,
        Vec<HashSet<NodeIndex>>,
        NodeIndex,
        NodeIndex,
    ) {
        let mut schedule = DiGraph::new();
        let model_task: Arc<dyn Task> = Arc::new(TestTask {
            resource_type: NodeType::Model,
            task_type: "run",
            work_node_id: "model.test.orders".to_string(),
            nodes: vec![model_node()],
        });
        let test_task: Arc<dyn Task> = Arc::new(TestTask {
            resource_type: NodeType::Test,
            task_type: "run",
            work_node_id: "test.test.accepted_values_orders".to_string(),
            nodes: vec![test_node()],
        });
        let model_idx = schedule.add_node(model_task);
        let test_idx = schedule.add_node(test_task);
        schedule.add_edge(model_idx, test_idx, ());

        let mut dependents = vec![HashSet::new(); schedule.node_count()];
        for edge in schedule.edge_references() {
            dependents[edge.source().index()].insert(edge.target());
        }

        (schedule, dependents, model_idx, test_idx)
    }

    #[test]
    fn reused_model_does_not_preempt_downstream_tests_when_state_data_tests_run() {
        let (schedule, dependents, model_idx, test_idx) = model_with_downstream_test_schedule();
        let mut skip_set = SkipSet::new();

        let (_failed_nodes, reused_nodes) = skip_set.handle_task_result(
            Ok(NodeStatus::ReusedNoChanges(
                "No new changes on any upstreams".to_string(),
            )),
            model_idx,
            &dependents,
            &schedule,
            false,
            false,
        );

        assert!(reused_nodes.is_empty());
        assert!(!skip_set.skip.contains_key(&test_idx));
    }

    #[test]
    fn reused_model_still_preempts_downstream_tests_in_legacy_mode() {
        let (schedule, dependents, model_idx, test_idx) = model_with_downstream_test_schedule();
        let mut skip_set = SkipSet::new();

        let (_failed_nodes, reused_nodes) = skip_set.handle_task_result(
            Ok(NodeStatus::ReusedNoChanges(
                "No new changes on any upstreams".to_string(),
            )),
            model_idx,
            &dependents,
            &schedule,
            false,
            true,
        );

        assert_eq!(reused_nodes.len(), 1);
        assert!(matches!(
            skip_set.skip.get(&test_idx),
            Some(SkipReason::Reused)
        ));
    }
}
