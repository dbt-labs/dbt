use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering},
    },
};

use dbt_common::{
    FsResult,
    stats::NodeStatus,
    tracing::{
        layers::tui_layer::{TuiAggregatedTestGroup, TuiAllProcessingNodesGroup},
        span_info::{
            read_span_attrs, read_span_start_info, record_span_status_from_attrs, update_span_attrs,
        },
    },
};
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_telemetry::{
    ExecutionPhase, NodeCacheDetail, NodeCacheReason, NodeErrorType, NodeEvaluated, NodeOutcome,
    NodeOutcomeDetail, NodeProcessed, NodeSkipReason, NodeSkipUpstreamDetail, NodeType,
    PhaseExecuted, TestEvaluationDetail, TestOutcome, set_node_warning_outcome_no_warnings,
    set_node_warning_outcome_warned, update_dbt_core_event_code_for_node_processed_end,
};
use dbt_tracing::StaticName as _;
use petgraph::graph::DiGraph;

use crate::{
    span_manager::{
        ParentSpanBuilder, ParentSpanRef, ParentTaskSpanOnClose, ParentTaskSpanOnSkip, SpanLevel,
        SpanManager, SpanTreeRequest,
    },
    task::{AggregatedNodeGroup, TP, Task},
    visitor::SkipReason,
};

/// Hands out dbt core style `[N of M]` execution indices for `NodeProcessed` spans.
///
/// `next_index` is bumped once per node, when that node's span is built. The span manager
/// builds each parent span lazily and exactly once, as the node's first task opens, so the
/// index ends up reflecting the order in which nodes *begin* processing -- the same thing
/// dbt core's dequeue counter measures.
///
/// `total` is accumulated while spans are registered and read back lazily by the builders.
/// Registration completes before any task runs, so builders always observe the final value.
#[derive(Default)]
pub struct NodeIndexAllocator {
    next_index: AtomicU32,
    total: AtomicU32,
}

impl NodeIndexAllocator {
    /// Counts one more node that will be handed an index.
    fn add_node(&self) {
        self.total.fetch_add(1, Ordering::Relaxed);
    }

    /// Claims the next 1-based index, paired with the total it should be shown against.
    fn claim(&self) -> (u32, u32) {
        (
            self.next_index.fetch_add(1, Ordering::AcqRel) + 1,
            self.total.load(Ordering::Acquire),
        )
    }
}

/// Whether a node takes part in the `[N of M]` numbering.
///
/// dbt core numbers every selected node except ephemeral models: those print no progress
/// line and are left out of the `M` denominator (`num_nodes` in `GraphRunnableTask`). Fusion
/// pulls ephemeral upstreams into the selection to build them, and reports them as no-op
/// skips that the TUI suppresses, so they have to be excluded here for the largest index to
/// match the total.
fn takes_execution_index(node: &dyn InternalDbtNodeAttributes, in_selection: bool) -> bool {
    in_selection && node.materialized() != DbtMaterialization::Ephemeral
}

/// Helper functions for generating span keys used in registration and task requests
/// Returns the span key for a PhaseExecuted span
pub fn phase_span_key(phase: ExecutionPhase) -> String {
    format!("{}::{:?}", PhaseExecuted::FULL_NAME, phase)
}

/// Returns the span key for a NodeProcessed span
pub fn node_processed_span_key(unique_id: &str) -> String {
    format!("{}::{}", NodeProcessed::FULL_NAME, unique_id)
}

/// Returns the span key for the TuiAllProcessingNodesGroup span
fn all_processing_nodes_span_key() -> String {
    "__all_processing_nodes__".to_string()
}

/// Returns the span key for an aggregated node group's grouping span
fn aggregated_test_group_span_key(group_unique_id: &str) -> String {
    format!("__aggregated_test_group__::{group_unique_id}")
}

pub fn update_node_outcome_from_skip_reason(ev: &mut NodeEvaluated, skip_reason: &SkipReason) {
    ev.set_node_outcome(NodeOutcome::Skipped);
    match skip_reason {
        SkipReason::FailedPhase => {
            ev.set_node_skip_reason(NodeSkipReason::PhaseSkipped);
        }
        SkipReason::FailedUpstream(upstream_id) => {
            ev.set_node_skip_reason(NodeSkipReason::Upstream);
            ev.node_outcome_detail = Some(NodeOutcomeDetail::NodeSkipUpstreamDetail(
                NodeSkipUpstreamDetail::new(upstream_id.clone()),
            ));
        }
        SkipReason::Reused => {
            ev.set_node_skip_reason(NodeSkipReason::Cached);
            ev.node_outcome_detail = Some(NodeOutcomeDetail::NodeCacheDetail(
                NodeCacheDetail::new(NodeCacheReason::NoChanges, None, None),
            ));
        }
    }
}

/// TODO: this is a temporary reverse mapping from legacy status to new outcome model.
/// We should revert to using outcome directly in the task result and calculate status
/// from that - since outcome is more expressive, and current implementation is lossy.
fn update_node_outcome_from_legacy_status(event: &mut NodeEvaluated, status: NodeStatus) {
    match status {
        NodeStatus::Succeeded => {
            event.set_node_outcome(NodeOutcome::Success);
            set_node_warning_outcome_no_warnings(event);
        }
        NodeStatus::SucceededWithWarning => {
            event.set_node_outcome(NodeOutcome::Success);
            // TODO: Stamp WithWarnings authoritatively. The middleware stamps it when a warn log fires
            // inside the span, but warnings detected before the span opens (e.g. the pre-
            // registration column_types check) can't be caught by the middleware. Stamping here
            // is idempotent: if the middleware already set it, this is a no-op.
            set_node_warning_outcome_warned(event);
        }
        NodeStatus::TestPassed => {
            // Handled inside the test task
        }
        NodeStatus::StaticallyCheckedDataTest => {
            event.set_node_outcome(NodeOutcome::Success);
            event.node_outcome_detail = Some(NodeOutcomeDetail::NodeTestDetail(
                TestEvaluationDetail::new(TestOutcome::Passed, 0, None, None, Some(true), None),
            ));
        }
        NodeStatus::TestWarned => {
            // Handled inside the test task
        }
        NodeStatus::Errored => {
            if event.node_type() == NodeType::Test || event.node_type() == NodeType::UnitTest {
                // Normally handled inside the test task, but if node outcome is unspecified,
                // set to error. This happens in some scenarios, e.g. statically analyzed error.
                if event.node_outcome() == NodeOutcome::Unspecified {
                    event.set_node_outcome(NodeOutcome::Error);
                    event.set_node_error_type(NodeErrorType::User);
                }
            } else {
                event.set_node_outcome(NodeOutcome::Error);
                event.set_node_error_type(NodeErrorType::User);
            };
        }
        NodeStatus::SkippedUpstreamFailed => {
            event.set_node_outcome(NodeOutcome::Skipped);
            event.set_node_skip_reason(NodeSkipReason::Upstream);
        }
        NodeStatus::ReusedNoChanges(_) => {
            event.set_node_outcome(NodeOutcome::Skipped);
            event.set_node_skip_reason(NodeSkipReason::Cached);

            event.node_outcome_detail = Some(NodeOutcomeDetail::NodeCacheDetail(
                NodeCacheDetail::new(NodeCacheReason::NoChanges, None, None),
            ));
        }
        NodeStatus::ReusedStillFresh(_, freshness_sec, last_updated_secs) => {
            event.set_node_outcome(NodeOutcome::Skipped);
            event.set_node_skip_reason(NodeSkipReason::Cached);

            event.node_outcome_detail =
                Some(NodeOutcomeDetail::NodeCacheDetail(NodeCacheDetail::new(
                    NodeCacheReason::StillFresh,
                    Some(freshness_sec),
                    Some(last_updated_secs),
                )));
        }
        NodeStatus::ReusedStillFreshNoChanges(_) => {
            event.set_node_outcome(NodeOutcome::Skipped);
            event.set_node_skip_reason(NodeSkipReason::Cached);

            event.node_outcome_detail = Some(NodeOutcomeDetail::NodeCacheDetail(
                NodeCacheDetail::new(NodeCacheReason::UpdateCriteriaNotMet, None, None),
            ));
        }
        NodeStatus::ReusedCloned(freshness_sec) => {
            event.set_node_outcome(NodeOutcome::Skipped);
            event.set_node_skip_reason(NodeSkipReason::Cached);

            let node_cache_reason = if freshness_sec.is_some() {
                NodeCacheReason::ClonedExistingStillFresh
            } else {
                NodeCacheReason::ClonedExisting
            };
            event.node_outcome_detail = Some(NodeOutcomeDetail::NodeCacheDetail(
                NodeCacheDetail::new(node_cache_reason, freshness_sec, None),
            ));
        }
        NodeStatus::NoOp => {
            event.set_node_outcome(NodeOutcome::Skipped);
            event.set_node_skip_reason(NodeSkipReason::NoOp);
        }
    };
}

/// Callback for task span on_close: updates NodeEvaluated with outcome from result.
pub fn task_span_on_close(task_span: &tracing::Span, result: &FsResult<NodeStatus>) {
    record_span_status_from_attrs(task_span, |attrs| {
        if let Some(ev) = attrs.downcast_mut::<NodeEvaluated>() {
            match result {
                Ok(ns) => update_node_outcome_from_legacy_status(ev, ns.clone()),
                Err(_) => {
                    ev.set_node_outcome(NodeOutcome::Error);
                    ev.set_node_error_type(NodeErrorType::Internal);
                }
            };
        }
    })
}

/// Callback for task span on_skip: updates NodeEvaluated with outcome from skip reason.
pub fn task_span_on_skip(task_span: &tracing::Span, skip_reason: &SkipReason) {
    record_span_status_from_attrs(task_span, |attrs| {
        if let Some(ev) = attrs.downcast_mut::<NodeEvaluated>() {
            update_node_outcome_from_skip_reason(ev, skip_reason);
        }
    })
}

pub fn create_task_span_for_node(
    node: &dyn InternalDbtNodeAttributes,
    phase: ExecutionPhase,
    span_manager: &SpanManager<FsResult<NodeStatus>, SkipReason>,
    in_dir: &Path,
    out_dir: &Path,
) -> FsResult<tracing::Span> {
    let mut builder = SpanTreeRequest::builder(
        SpanLevel::Debug,
        node.get_node_evaluated_event(phase, in_dir, out_dir).into(),
        Some(Box::new(task_span_on_close)),
        Some(Box::new(task_span_on_skip)),
    );
    builder.with_parent(SpanLevel::Info, phase_span_key(phase));
    let key = node_processed_span_key(&node.unique_id());
    builder.add_related_span(SpanLevel::Info, &key);
    let request = builder.build();
    span_manager.get_task_span(request)
}

/// Information about node counts across phases, used for span creation.
#[derive(Debug, Clone)]
struct NodeCounts {
    /// Number of nodes in each phase
    node_counts_by_phase: HashMap<TP, u64>,
    /// Number of tasks for each unique node ID
    node_unique_id_to_task_count: HashMap<String, u8>,
}

/// Calculate node counts from the task schedule.
///
/// This determines how many nodes are in each phase and how many tasks
/// each unique node has, which is used for span lifecycle management.
fn calculate_node_counts(schedule: &DiGraph<Arc<dyn Task>, ()>) -> NodeCounts {
    let mut node_counts_by_phase: HashMap<TP, u64> = HashMap::new();
    let mut node_unique_id_to_task_count: HashMap<String, u8> = HashMap::new();

    for task in schedule.node_weights() {
        if let Some(tp) = task.task_phase() {
            let count = task.dbt_nodes().len() as u64;
            *node_counts_by_phase.entry(tp).or_insert(0) += count;
            for unique_id in task
                .dbt_nodes()
                .iter()
                .map(|node| node.common().unique_id.as_str())
            {
                *node_unique_id_to_task_count
                    .entry(unique_id.to_string())
                    .or_insert(0) += 1
            }
        }
    }

    NodeCounts {
        node_counts_by_phase,
        node_unique_id_to_task_count,
    }
}

/// Collects the aggregated node groups in the schedule together with the unique ids of the
/// nodes each one covers.
///
/// Keyed by group unique id so a group reported by several tasks is registered exactly
/// once - `register_parent_span_builder` rejects duplicate keys, and a partial
/// registration surfaces much later as a missing parent span builder.
fn collect_aggregated_node_groups(
    schedule: &DiGraph<Arc<dyn Task>, ()>,
) -> BTreeMap<String, (AggregatedNodeGroup, BTreeSet<String>)> {
    let mut groups: BTreeMap<String, (AggregatedNodeGroup, BTreeSet<String>)> = BTreeMap::new();

    for task in schedule.node_weights() {
        let Some(group) = task.aggregated_node_group() else {
            continue;
        };

        let (_, member_ids) = groups
            .entry(group.unique_id.clone())
            .or_insert_with(|| (group, BTreeSet::new()));

        member_ids.extend(
            task.dbt_nodes()
                .iter()
                .map(|node| node.common().unique_id.clone()),
        );
    }

    groups
}

fn update_node_processed_from_node_evaluated(sum_ev: &mut NodeProcessed, phase_ev: &NodeEvaluated) {
    if phase_ev.phase() == ExecutionPhase::NodeCacheHydration {
        // Special case - save error outcome, but otherwise save as no-op
        // This will make frontier nodes a no-op, since they don't have any other phases
        if phase_ev.node_outcome() == NodeOutcome::Error {
            sum_ev.set_node_outcome(NodeOutcome::Error);
            sum_ev.node_error_type = phase_ev.node_error_type;
        } else {
            sum_ev.set_node_outcome(NodeOutcome::Skipped);
            sum_ev.set_node_skip_reason(NodeSkipReason::NoOp);
        }
        sum_ev.last_phase = phase_ev.phase;
        return;
    }

    // Determine if the new incoming closed NodeEvaluated event should update
    // the previous outcome recorded for this node in NodeProcessed.
    let update_previous = match phase_ev.node_outcome() {
        NodeOutcome::Skipped => match phase_ev.node_skip_reason() {
            NodeSkipReason::Upstream => true,
            NodeSkipReason::Cached => true,
            NodeSkipReason::NoOp => true,
            // If we skipped the phase, or skipped because the phase is disabled,
            // we keep whatever previous outcome was recorded for the node
            NodeSkipReason::PhaseSkipped
            | NodeSkipReason::PhaseDisabled
            | NodeSkipReason::Unspecified => false,
        },
        NodeOutcome::Success => true,
        NodeOutcome::Error => true,
        NodeOutcome::Canceled => true,
        NodeOutcome::Unspecified => false, // should never happen
    };

    if !update_previous {
        return;
    }

    sum_ev.sao_enabled = phase_ev.sao_enabled;
    sum_ev.node_outcome = phase_ev.node_outcome;
    sum_ev.node_skip_reason = phase_ev.node_skip_reason;
    sum_ev.node_error_type = phase_ev.node_error_type;
    sum_ev.node_cancel_reason = phase_ev.node_cancel_reason;
    sum_ev.node_outcome_detail = phase_ev
        .node_outcome_detail
        .as_ref()
        .map(|d| d.clone().into());
    sum_ev.last_phase = phase_ev.phase;
    sum_ev.rows_affected = phase_ev.rows_affected;
}

fn accumulate_node_processed_phase_duration(
    sum_ev: &mut NodeProcessed,
    wall_duration_ms: Option<u64>,
    phase_ev: Option<&NodeEvaluated>,
) {
    let idle_time_ms = phase_ev
        .and_then(|ev| {
            if ev.node_outcome() == NodeOutcome::Success
                && ev.node_skip_reason() == NodeSkipReason::Cached
            {
                wall_duration_ms
            } else {
                ev.idle_time_ms
            }
        })
        .unwrap_or_default();

    if let Some(duration) = wall_duration_ms {
        let net_duration = duration.saturating_sub(idle_time_ms);
        sum_ev.duration_ms = Some(
            sum_ev
                .duration_ms
                .unwrap_or_default()
                .saturating_add(net_duration),
        );
    }

    if idle_time_ms > 0 {
        sum_ev.idle_time_ms = Some(
            sum_ev
                .idle_time_ms
                .unwrap_or_default()
                .saturating_add(idle_time_ms),
        );
    }
}

/// Factory for phase parent on_task_close callback.
fn create_phase_on_task_close(
    phase_finished_counter: Arc<AtomicU64>,
    node_count_in_phase_total: u64,
) -> ParentTaskSpanOnClose<FsResult<NodeStatus>> {
    Box::new(
        move |phase_span, _task_span, result: &FsResult<NodeStatus>| {
            update_span_attrs(phase_span, |attrs: &mut PhaseExecuted| {
                let outcome = result
                    .as_ref()
                    .map(|r| r.clone().into())
                    .unwrap_or(NodeOutcome::Error);
                match outcome {
                    NodeOutcome::Error => {
                        if let Some(c) = attrs.node_count_error.as_mut() {
                            *c += 1;
                        };
                    }
                    // Should not happen, since we have a separate skip handler,
                    // but just in case
                    NodeOutcome::Skipped => {
                        if let Some(c) = attrs.node_count_skipped.as_mut() {
                            *c += 1;
                        };
                    }
                    _ => {}
                }
            });

            // Increment finished counter
            let cur_finished = phase_finished_counter.fetch_add(1, Ordering::AcqRel) + 1;

            // Close when all tasks are done
            cur_finished == node_count_in_phase_total
        },
    )
}

/// Factory for phase parent on_task_skip callback.
fn create_phase_on_task_skip(
    phase_finished_counter: Arc<AtomicU64>,
    node_count_in_phase_total: u64,
) -> ParentTaskSpanOnSkip<SkipReason> {
    Box::new(move |phase_span, _task_span, _skip_reason| {
        update_span_attrs(phase_span, |attrs: &mut PhaseExecuted| {
            if let Some(c) = attrs.node_count_skipped.as_mut() {
                *c += 1;
            };
        });

        // Increment finished counter
        let cur_finished = phase_finished_counter.fetch_add(1, Ordering::AcqRel) + 1;

        // Close when all tasks are done
        cur_finished == node_count_in_phase_total
    })
}

/// Factory for node processed on_task_close callback.
fn create_node_processed_on_task_close(
    node_processed_finished_counter: Arc<AtomicU8>,
    task_count_total: u8,
) -> ParentTaskSpanOnClose<FsResult<NodeStatus>> {
    Box::new(move |this_span, task_span, _| {
        // Calculate duration of this NodeEvaluated span
        let phase_duration_ms = read_span_start_info(task_span, |start_info| {
            let now = std::time::SystemTime::now();
            now.duration_since(start_info.start_time_unix_nano)
                .ok()
                .map(|d| d.as_millis() as u64)
        })
        .flatten();

        update_span_attrs(this_span, |ev: &mut NodeProcessed| {
            // Update NodeProcessed from NodeEvaluated attributes
            let has_node_attrs = read_span_attrs::<NodeEvaluated, _>(task_span, |attrs| {
                accumulate_node_processed_phase_duration(ev, phase_duration_ms, Some(attrs));
                update_node_processed_from_node_evaluated(ev, attrs);
            })
            .is_some();

            if !has_node_attrs {
                accumulate_node_processed_phase_duration(ev, phase_duration_ms, None);
            }
        });

        // Increment finished counter
        let cur_finished = node_processed_finished_counter.fetch_add(1, Ordering::AcqRel) + 1;

        // Close when all tasks are done
        cur_finished == task_count_total
    })
}

/// Factory for node processed on_task_skip callback.
fn create_node_processed_on_task_skip(
    node_processed_finished_counter: Arc<AtomicU8>,
    task_count_total: u8,
) -> ParentTaskSpanOnSkip<SkipReason> {
    Box::new(move |this_span, task_span, _| {
        // Calculate duration of this NodeEvaluated span
        let phase_duration_ms = read_span_start_info(task_span, |start_info| {
            let now = std::time::SystemTime::now();
            now.duration_since(start_info.start_time_unix_nano)
                .ok()
                .map(|d| d.as_millis() as u64)
        })
        .flatten();

        update_span_attrs(this_span, |ev: &mut NodeProcessed| {
            // Update NodeProcessed from NodeEvaluated attributes
            let has_node_attrs = read_span_attrs::<NodeEvaluated, _>(task_span, |attrs| {
                accumulate_node_processed_phase_duration(ev, phase_duration_ms, Some(attrs));
                update_node_processed_from_node_evaluated(ev, attrs);
            })
            .is_some();

            if !has_node_attrs {
                accumulate_node_processed_phase_duration(ev, phase_duration_ms, None);
            }
        });

        // Increment finished counter
        let cur_finished = node_processed_finished_counter.fetch_add(1, Ordering::AcqRel) + 1;

        // Close when all tasks are done
        cur_finished == task_count_total
    })
}

/// Creates a builder_fn closure for the Phase parent span.
fn create_phase_parent_builder_fn(
    phase: ExecutionPhase,
    node_count_in_phase_total: u64,
) -> impl FnOnce() -> ParentSpanBuilder<FsResult<NodeStatus>, SkipReason> {
    move || {
        let phase_finished_counter = Arc::new(AtomicU64::new(0));
        let phase_finished_counter_clone = phase_finished_counter.clone();

        ParentSpanBuilder::new(
            PhaseExecuted::start_with_node_count(phase, node_count_in_phase_total).into(),
            // No special handling on close. PhaseExecuted automatically infers span status from
            // node counts.
            None,
            // Track when child tasks finish for counting
            Some(create_phase_on_task_close(
                phase_finished_counter_clone,
                node_count_in_phase_total,
            )),
            Some(create_phase_on_task_skip(
                phase_finished_counter,
                node_count_in_phase_total,
            )),
        )
    }
}

/// Creates a builder_fn closure for the NodeProcessed related span.
fn create_node_processed_builder_fn(
    node: Arc<dyn InternalDbtNodeAttributes>,
    in_dir: PathBuf,
    out_dir: PathBuf,
    initial_phase: ExecutionPhase,
    task_count_total: u8,
    in_selection: bool,
    index_allocator: Arc<NodeIndexAllocator>,
    takes_index: bool,
) -> impl FnOnce() -> ParentSpanBuilder<FsResult<NodeStatus>, SkipReason> {
    let node_processed_finished_counter = Arc::new(AtomicU8::new(0));
    let node_processed_finished_counter_clone = node_processed_finished_counter.clone();

    move || {
        let mut event = node.get_node_processed_event(
            Some(initial_phase),
            in_dir.as_path(),
            out_dir.as_path(),
            in_selection,
        );

        // Claimed here, rather than at registration, so the number reflects the order nodes
        // start processing instead of the order they happen to sit in the schedule.
        if takes_index {
            let (index, total) = index_allocator.claim();
            event.node_index = Some(index);
            event.node_count_total = Some(total);
        }

        ParentSpanBuilder::new(
            event.into(),
            // Update dbt core code, status should auto-infer from attributes
            Some(Box::new(|this_span| {
                update_span_attrs(this_span, |ev: &mut NodeProcessed| {
                    update_dbt_core_event_code_for_node_processed_end(ev);
                })
            })),
            Some(create_node_processed_on_task_close(
                node_processed_finished_counter,
                task_count_total,
            )),
            Some(create_node_processed_on_task_skip(
                node_processed_finished_counter_clone,
                task_count_total,
            )),
        )
    }
}

/// Creates a builder_fn closure for an aggregated node group's grouping span.
///
/// `member_count` must be the number of `NodeProcessed` spans registered under this group,
/// so the span closes exactly when the last of them does.
fn create_aggregated_test_group_builder_fn(
    group: AggregatedNodeGroup,
    member_count: u64,
) -> impl FnOnce() -> ParentSpanBuilder<FsResult<NodeStatus>, SkipReason> {
    move || {
        let group_finished_counter = Arc::new(AtomicU64::new(0));
        let group_finished_counter_clone = group_finished_counter.clone();

        ParentSpanBuilder::new(
            TuiAggregatedTestGroup {
                macro_name: group.macro_name,
                attached_node: group.attached_node,
            }
            .into(),
            // No callbacks, pure TUI grouping span
            None,
            // Track when member nodes finish for counting
            Some(Box::new(move |_, _, _| {
                let cur_finished = group_finished_counter.fetch_add(1, Ordering::AcqRel) + 1;

                cur_finished == member_count
            })),
            Some(Box::new(move |_, _, _| {
                let cur_finished = group_finished_counter_clone.fetch_add(1, Ordering::AcqRel) + 1;

                cur_finished == member_count
            })),
        )
    }
}

/// Creates a builder_fn closure for the all processing nodes group parent span.
///
/// The close callbacks fire once per closing *direct child*, so `child_count_total` counts
/// spans registered directly under this one - not nodes. Aggregated group members close
/// under their own group span and contribute one increment per group, not per member.
fn create_node_group_builder_fn(
    child_count_total: u64,
) -> impl FnOnce() -> ParentSpanBuilder<FsResult<NodeStatus>, SkipReason> {
    move || {
        let node_group_finished_counter = Arc::new(AtomicU64::new(0));
        let node_group_finished_counter_clone = node_group_finished_counter.clone();

        ParentSpanBuilder::new(
            TuiAllProcessingNodesGroup.into(),
            // No callbacks, pure TUI grouping span
            None,
            // Track when child tasks finish for counting
            Some(Box::new(move |_, _, _| {
                // Increment finished counter
                let cur_finished = node_group_finished_counter.fetch_add(1, Ordering::AcqRel) + 1;

                // Close when all children are done
                cur_finished == child_count_total
            })),
            Some(Box::new(move |_, _, _| {
                // Increment finished counter
                let cur_finished =
                    node_group_finished_counter_clone.fetch_add(1, Ordering::AcqRel) + 1;

                // Close when all children are done
                cur_finished == child_count_total
            })),
        )
    }
}

/// Pre-populate the span manager with all expected parent span builders.
///
/// These are lazily instantiated when visitor visits tasks, which reference
/// them by span key. See `Task::telemetry_tree` method for details.
///
/// This must be called before any task spans are created. It registers:
/// - Phase spans (one per phase with nodes in that phase)
/// - TuiAggregatedTestGroup spans (one per aggregated node group)
/// - NodeProcessed spans (one per unique node)
/// - TuiAllProcessingNodesGroup span (single span for all nodes)
///
/// # Returns
/// - `Ok(())`: All builders registered successfully
/// - `Err`: A duplicate span key was encountered
pub fn populate_span_manager(
    span_manager: &SpanManager<FsResult<NodeStatus>, SkipReason>,
    schedule: &DiGraph<Arc<dyn Task>, ()>,
    in_dir: &Path,
    out_dir: &Path,
    selected_nodes: &BTreeSet<String>,
) -> FsResult<()> {
    let node_counts = calculate_node_counts(schedule);

    // Register phase parent spans (no parent - root level)
    for (phase, &count) in &node_counts.node_counts_by_phase {
        let execution_phase: ExecutionPhase = (*phase).into();
        span_manager.register_parent_span_builder(
            phase_span_key(execution_phase),
            None, // Phase spans have no managed parent (will be placed under visitor span)
            Box::new(create_phase_parent_builder_fn(execution_phase, count)),
        )?;
    }

    let aggregated_node_groups = collect_aggregated_node_groups(schedule);
    let node_id_to_group: HashMap<String, String> = aggregated_node_groups
        .iter()
        .flat_map(|(group_unique_id, (_, member_ids))| {
            member_ids
                .iter()
                .map(|member_id| (member_id.clone(), group_unique_id.clone()))
        })
        .collect();

    // Counted as spans are registered, so the total the TuiAllProcessingNodesGroup span
    // closes on can never disagree with the tree that actually gets built.
    let mut all_processing_nodes_child_count = 0u64;

    // Shared by every NodeProcessed builder: the loop below tallies the denominator while
    // registering, and each builder claims its own index once execution reaches that node.
    let index_allocator = Arc::new(NodeIndexAllocator::default());

    // Register TuiAggregatedTestGroup spans (one per aggregated node group)
    for (group_unique_id, (group, member_ids)) in aggregated_node_groups {
        all_processing_nodes_child_count += 1;

        span_manager.register_parent_span_builder(
            aggregated_test_group_span_key(&group_unique_id),
            // TuiAggregatedTestGroup has TuiAllProcessingNodesGroup as parent
            Some(ParentSpanRef::new(
                SpanLevel::Info,
                all_processing_nodes_span_key().into(),
            )),
            Box::new(create_aggregated_test_group_builder_fn(
                group,
                member_ids.len() as u64,
            )),
        )?;
    }

    // Register NodeProcessed spans (one per unique node)
    let mut seen_nodes = HashSet::new();
    for task in schedule.node_weights() {
        for node in task.dbt_nodes() {
            let unique_id = node.common().unique_id.clone();
            if seen_nodes.insert(unique_id.clone()) {
                let task_count_total = *node_counts
                    .node_unique_id_to_task_count
                    .get(&unique_id)
                    .unwrap_or(&1);

                let in_selection = selected_nodes.contains(&unique_id);

                let takes_index = takes_execution_index(node.as_ref(), in_selection);
                if takes_index {
                    index_allocator.add_node();
                }

                // Members of an aggregated group hang off their group span; every other
                // node hangs off TuiAllProcessingNodesGroup directly.
                let parent_span_key = match node_id_to_group.get(&unique_id) {
                    Some(group_unique_id) => aggregated_test_group_span_key(group_unique_id),
                    None => {
                        all_processing_nodes_child_count += 1;
                        all_processing_nodes_span_key()
                    }
                };

                span_manager.register_parent_span_builder(
                    node_processed_span_key(&unique_id),
                    Some(ParentSpanRef::new(SpanLevel::Info, parent_span_key.into())),
                    Box::new(create_node_processed_builder_fn(
                        node.clone(),
                        in_dir.to_path_buf(),
                        out_dir.to_path_buf(),
                        ExecutionPhase::Unspecified,
                        task_count_total,
                        in_selection,
                        index_allocator.clone(),
                        takes_index,
                    )),
                )?;
            }
        }
    }

    // Register the single TuiAllProcessingNodesGroup span
    span_manager.register_parent_span_builder(
        all_processing_nodes_span_key(),
        None, // TuiAllProcessingNodesGroup has no managed parent (will be placed under visitor span)
        Box::new(create_node_group_builder_fn(
            all_processing_nodes_child_count,
        )),
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_telemetry::{
        TestEvaluationDetail, TestOutcome, node_processed::NodeOutcomeDetail as ProcessedDetail,
    };

    fn node_processed() -> NodeProcessed {
        NodeProcessed::start(
            "test.project.accepted_values_orders_is_today_order__True".to_string(),
            "accepted_values_orders_is_today_order__True".to_string(),
            None,
            Some("dbt_test__audit".to_string()),
            None,
            None,
            None,
            NodeType::Test,
            Some(ExecutionPhase::Run),
            "models/marts/orders.yml".to_string(),
            Some(37),
            Some(13),
            "checksum".to_string(),
            true,
            None,
        )
    }

    fn node_evaluated(phase: ExecutionPhase) -> NodeEvaluated {
        NodeEvaluated::start(
            "test.project.accepted_values_orders_is_today_order__True".to_string(),
            "accepted_values_orders_is_today_order__True".to_string(),
            None,
            Some("dbt_test__audit".to_string()),
            None,
            None,
            None,
            NodeType::Test,
            phase,
            "models/marts/orders.yml".to_string(),
            Some(37),
            Some(13),
            "checksum".to_string(),
        )
    }

    #[test]
    fn statically_checked_data_test_sets_detail_and_processed_copies_it() {
        let mut run = node_evaluated(ExecutionPhase::Run);

        update_node_outcome_from_legacy_status(&mut run, NodeStatus::StaticallyCheckedDataTest);

        assert_eq!(run.node_outcome(), NodeOutcome::Success);
        match run.node_outcome_detail.as_ref() {
            Some(NodeOutcomeDetail::NodeTestDetail(detail)) => {
                assert_eq!(detail.test_outcome(), TestOutcome::Passed);
                assert_eq!(detail.failing_rows, 0);
                assert_eq!(detail.statically_checked, Some(true));
            }
            other => panic!("expected statically checked test detail, got {other:?}"),
        }

        let mut processed = node_processed();
        update_node_processed_from_node_evaluated(&mut processed, &run);

        assert_eq!(processed.node_outcome(), NodeOutcome::Success);
        match processed.node_outcome_detail {
            Some(ProcessedDetail::NodeTestDetail(detail)) => {
                assert_eq!(detail.test_outcome(), TestOutcome::Passed);
                assert_eq!(detail.failing_rows, 0);
                assert_eq!(detail.statically_checked, Some(true));
            }
            other => panic!("expected processed statically checked test detail, got {other:?}"),
        }
    }

    #[test]
    fn cached_warning_test_run_outcome_overrides_prior_no_op_phase() {
        let mut processed = node_processed();

        let mut cache_hydration = node_evaluated(ExecutionPhase::NodeCacheHydration);
        cache_hydration.set_node_outcome(NodeOutcome::Success);
        update_node_processed_from_node_evaluated(&mut processed, &cache_hydration);
        assert_eq!(processed.node_outcome(), NodeOutcome::Skipped);
        assert_eq!(processed.node_skip_reason(), NodeSkipReason::NoOp);

        let mut run = node_evaluated(ExecutionPhase::Run);
        run.set_node_outcome(NodeOutcome::Success);
        run.set_node_skip_reason(NodeSkipReason::Cached);
        run.node_outcome_detail = Some(NodeOutcomeDetail::NodeTestDetail(
            TestEvaluationDetail::new(TestOutcome::Warned, 2, None, None, None, None),
        ));

        update_node_processed_from_node_evaluated(&mut processed, &run);

        assert_eq!(processed.node_outcome(), NodeOutcome::Success);
        assert_eq!(processed.node_skip_reason(), NodeSkipReason::Cached);
        match processed.node_outcome_detail {
            Some(ProcessedDetail::NodeTestDetail(detail)) => {
                assert_eq!(detail.test_outcome(), TestOutcome::Warned);
                assert_eq!(detail.failing_rows, 2);
            }
            other => panic!("expected warned test detail, got {other:?}"),
        }
    }

    #[test]
    fn cached_warning_test_duration_is_idle_time() {
        let mut processed = node_processed();
        let mut run = node_evaluated(ExecutionPhase::Run);
        run.set_node_outcome(NodeOutcome::Success);
        run.set_node_skip_reason(NodeSkipReason::Cached);
        run.node_outcome_detail = Some(NodeOutcomeDetail::NodeTestDetail(
            TestEvaluationDetail::new(TestOutcome::Warned, 2, None, None, None, None),
        ));

        accumulate_node_processed_phase_duration(&mut processed, Some(250), Some(&run));
        update_node_processed_from_node_evaluated(&mut processed, &run);

        assert_eq!(processed.duration_ms, Some(0));
        assert_eq!(processed.idle_time_ms, Some(250));
        assert_eq!(processed.node_outcome(), NodeOutcome::Success);
        assert_eq!(processed.node_skip_reason(), NodeSkipReason::Cached);
    }
}
