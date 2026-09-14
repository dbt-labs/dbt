use crate::proto::v1::public::events::fusion::{
    node::{
        NodeCacheDetail, NodeEvaluated, NodeEvaluationDetail, NodeMaterialization, NodeOutcome,
        NodeProcessed, NodeSkipReason, NodeSkipUpstreamDetail, NodeType, NodeWarningOutcome,
        SourceFreshnessDetail, SourceFreshnessOutcome, TestEvaluationDetail, TestOutcome,
        node_evaluated, node_processed,
    },
    phase::ExecutionPhase,
};

// Display trait is intentionally not implemented to avoid inefficient usage.
// Prefer `node_type.as_ref()` or if you need String use `node_type.as_ref().to_string()`.

impl SourceFreshnessOutcome {
    /// Returns a human-readable status string for the freshness outcome.
    pub const fn as_static_ref(&self) -> &'static str {
        match self {
            Self::OutcomePassed => "pass",
            Self::OutcomeWarned => "warn",
            Self::OutcomeFailed => "error",
        }
    }
}

impl NodeType {
    pub const fn as_static_ref(&self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Model => "model",
            Self::Seed => "seed",
            Self::Snapshot => "snapshot",
            Self::Source => "source",
            Self::Test => "test",
            Self::UnitTest => "unit_test",
            Self::Macro => "macro",
            Self::DocsMacro => "docs_macro",
            Self::Analysis => "analysis",
            Self::Operation => "operation",
            Self::Exposure => "exposure",
            Self::Metric => "metric",
            Self::SavedQuery => "saved_query",
            Self::SemanticModel => "semantic_model",
            Self::Function => "function",
            Self::Check => "check",
        }
    }
}

impl NodeType {
    pub const fn pretty(&self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Model => "model",
            Self::Seed => "seed",
            Self::Snapshot => "snapshot",
            Self::Source => "source",
            Self::Test => "test",
            Self::UnitTest => "unit test",
            Self::Macro => "macro",
            Self::DocsMacro => "docs macro",
            Self::Analysis => "analysis",
            Self::Operation => "operation",
            Self::Exposure => "exposure",
            Self::Metric => "metric",
            Self::SavedQuery => "saved query",
            Self::SemanticModel => "semantic model",
            Self::Function => "function",
            Self::Check => "check",
        }
    }
}

impl NodeMaterialization {
    pub const fn as_static_ref(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Snapshot => "snapshot",
            Self::Seed => "seed",
            Self::View => "view",
            Self::Table => "table",
            Self::Incremental => "incremental",
            Self::MaterializedView => "materialized_view",
            Self::External => "external",
            Self::Test => "test",
            Self::Ephemeral => "ephemeral",
            Self::Unit => "unit_test",
            Self::Analysis => "analysis",
            Self::StreamingTable => "streaming_table",
            Self::DynamicTable => "dynamic_table",
            Self::InteractiveTable => "interactive_table",
            Self::Function => "function",
            Self::MetricView => "metric_view",
            Self::Custom => "custom",
        }
    }
}

fn dbt_core_event_code_for_node_evaluation(phase: ExecutionPhase) -> Option<&'static str> {
    // Matches json_compat_layer behavior: emit Q030/Q031 for all nodes in Render/Run phases
    match phase {
        // Q030: NodeCompiling — emitted for all nodes in Render phase
        ExecutionPhase::Render => Some("Q030"),
        // Q031: NodeExecuting — emitted for all nodes in Run phase
        ExecutionPhase::Run => Some("Q031"),
        // No codes for other phases
        _ => None,
    }
}

fn dbt_core_event_code_for_node_processed_end(
    node_type: NodeType,
    node_outcome: NodeOutcome,
    node_skip_reason: Option<NodeSkipReason>,
    last_phase: ExecutionPhase,
) -> Option<&'static str> {
    // Handle explicit skip outcomes first where Core has distinct codes
    if node_outcome == NodeOutcome::Skipped {
        if matches!(node_skip_reason, Some(NodeSkipReason::NoOp)) {
            // Q019: LogNodeNoOpResult — clear match for NO-OP skips (e.g., ephemeral)
            return Some("Q019");
        }
        // Q034: SkippingDetails
        return Some("Q034");
    }

    // Q018: LogFreshnessResult — the freshness phase reports a freshness result for every
    // node kind it measures, so the phase wins over the node type. Without this a model
    // carrying a freshness SLA would be reported as a model build result (Q012).
    if last_phase == ExecutionPhase::FreshnessAnalysis {
        return Some("Q018");
    }

    // NodeProcessed spans entire node execution, so we use node type to determine the code
    match node_type {
        // Q007: LogTestResult — info level, specific to tests
        NodeType::Test | NodeType::UnitTest => Some("Q007"),

        // Q012: LogModelResult — info level, preferred over generic Q025
        NodeType::Model => Some("Q012"),

        // Q016: LogSeedResult — info level, preferred over generic Q025
        NodeType::Seed => Some("Q016"),

        // Q015: LogSnapshotResult — info level, preferred over generic Q025
        NodeType::Snapshot => Some("Q015"),

        // Q047: LogFunctionResult — info level, specific to functions
        NodeType::Function => Some("Q047"),

        // Q018: LogFreshnessResult — info level, for source freshness
        NodeType::Source => Some("Q018"),

        // Q008: LogNodeResult — info level, generic fallback for other node types
        _ => Some("Q008"),
    }
}

pub fn update_dbt_core_event_code_for_node_processed_end(event: &mut NodeProcessed) {
    if let Some(code) = dbt_core_event_code_for_node_processed_end(
        event.node_type(),
        event.node_outcome(),
        // Only pass `Some()` if it is actually set
        event.node_skip_reason.map(|_| event.node_skip_reason()),
        event.last_phase(),
    ) {
        event.dbt_core_event_code = code.to_string();
    }
}

pub enum NodeEvent<'a> {
    Evaluated(&'a NodeEvaluated),
    Processed(&'a NodeProcessed),
}

impl<'a> From<&'a NodeEvaluated> for NodeEvent<'a> {
    fn from(ne: &'a NodeEvaluated) -> Self {
        NodeEvent::Evaluated(ne)
    }
}

impl<'a> From<&'a NodeProcessed> for NodeEvent<'a> {
    fn from(np: &'a NodeProcessed) -> Self {
        NodeEvent::Processed(np)
    }
}

pub enum AnyNodeOutcomeDetail<'a> {
    NodeCacheDetail(&'a NodeCacheDetail),
    NodeTestDetail(&'a TestEvaluationDetail),
    NodeFreshnessOutcome(&'a SourceFreshnessDetail),
    NodeSkipUpstreamDetail(&'a NodeSkipUpstreamDetail),
    NodeEvaluationDetail(&'a NodeEvaluationDetail),
}

impl<'a> From<&'a node_processed::NodeOutcomeDetail> for AnyNodeOutcomeDetail<'a> {
    fn from(detail: &'a node_processed::NodeOutcomeDetail) -> Self {
        match detail {
            node_processed::NodeOutcomeDetail::NodeCacheDetail(cache_detail) => {
                AnyNodeOutcomeDetail::NodeCacheDetail(cache_detail)
            }
            node_processed::NodeOutcomeDetail::NodeTestDetail(test_detail) => {
                AnyNodeOutcomeDetail::NodeTestDetail(test_detail)
            }
            node_processed::NodeOutcomeDetail::NodeFreshnessOutcome(freshness_detail) => {
                AnyNodeOutcomeDetail::NodeFreshnessOutcome(freshness_detail)
            }
            node_processed::NodeOutcomeDetail::NodeSkipUpstreamDetail(skip_detail) => {
                AnyNodeOutcomeDetail::NodeSkipUpstreamDetail(skip_detail)
            }
            node_processed::NodeOutcomeDetail::NodeEvaluationDetail(eval_detail) => {
                AnyNodeOutcomeDetail::NodeEvaluationDetail(eval_detail)
            }
        }
    }
}

impl<'a> From<&'a node_evaluated::NodeOutcomeDetail> for AnyNodeOutcomeDetail<'a> {
    fn from(detail: &'a node_evaluated::NodeOutcomeDetail) -> Self {
        match detail {
            node_evaluated::NodeOutcomeDetail::NodeCacheDetail(cache_detail) => {
                AnyNodeOutcomeDetail::NodeCacheDetail(cache_detail)
            }
            node_evaluated::NodeOutcomeDetail::NodeTestDetail(test_detail) => {
                AnyNodeOutcomeDetail::NodeTestDetail(test_detail)
            }
            node_evaluated::NodeOutcomeDetail::NodeFreshnessOutcome(freshness_detail) => {
                AnyNodeOutcomeDetail::NodeFreshnessOutcome(freshness_detail)
            }
            node_evaluated::NodeOutcomeDetail::NodeSkipUpstreamDetail(skip_detail) => {
                AnyNodeOutcomeDetail::NodeSkipUpstreamDetail(skip_detail)
            }
            node_evaluated::NodeOutcomeDetail::NodeEvaluationDetail(eval_detail) => {
                AnyNodeOutcomeDetail::NodeEvaluationDetail(eval_detail)
            }
        }
    }
}

pub fn get_node_outcome_detail<'a>(node: NodeEvent<'a>) -> Option<AnyNodeOutcomeDetail<'a>> {
    match node {
        NodeEvent::Evaluated(ne) => ne.node_outcome_detail.as_ref().map(|dtl| dtl.into()),
        NodeEvent::Processed(np) => np.node_outcome_detail.as_ref().map(|dtl| dtl.into()),
    }
}

/// Extract test outcome from test details if available
pub fn get_test_outcome(node: NodeEvent) -> Option<TestOutcome> {
    get_node_outcome_detail(node).and_then(|detail| {
        if let AnyNodeOutcomeDetail::NodeTestDetail(test_detail) = detail {
            Some(test_detail.test_outcome())
        } else {
            None
        }
    })
}

/// Returns true if a test passed by static checking instead of execution.
pub fn is_statically_checked_test(node: NodeEvent<'_>) -> bool {
    let Some(AnyNodeOutcomeDetail::NodeTestDetail(test_detail)) = get_node_outcome_detail(node)
    else {
        return false;
    };
    test_detail.statically_checked.unwrap_or(false)
}

/// Returns true if a test ran as part of a batched generic-test query.
pub fn is_batched_test(node: NodeEvent<'_>) -> bool {
    get_test_batch_unique_id(node).is_some()
}

/// Returns the unique_id of the batched test node whose query produced this result, if any.
pub fn get_test_batch_unique_id(node: NodeEvent<'_>) -> Option<&str> {
    let Some(AnyNodeOutcomeDetail::NodeTestDetail(test_detail)) = get_node_outcome_detail(node)
    else {
        return None;
    };
    test_detail.batch_unique_id.as_deref()
}

/// Extract cache detail from node details if available
pub fn get_cache_detail(node: NodeEvent<'_>) -> Option<&'_ NodeCacheDetail> {
    get_node_outcome_detail(node).and_then(|detail| {
        if let AnyNodeOutcomeDetail::NodeCacheDetail(cache_detail) = detail {
            Some(cache_detail)
        } else {
            None
        }
    })
}

/// Extract freshness detail from node details if available
pub fn get_freshness_detail(node: NodeEvent<'_>) -> Option<&'_ SourceFreshnessDetail> {
    get_node_outcome_detail(node).and_then(|detail| {
        if let AnyNodeOutcomeDetail::NodeFreshnessOutcome(freshness_detail) = detail {
            Some(freshness_detail)
        } else {
            None
        }
    })
}

/// Returns true if the node completed with warnings.
pub fn has_node_warning(node: NodeEvent<'_>) -> bool {
    let Some(AnyNodeOutcomeDetail::NodeEvaluationDetail(d)) = get_node_outcome_detail(node) else {
        return false;
    };
    d.node_warning_outcome() == NodeWarningOutcome::WithWarnings
}

/// Marks the `NodeEvaluated` span as having produced warnings during successful execution.
///
/// This is called automatically by `TelemetryNodeWarnOutcome` middleware whenever a
/// `Warn`-severity log record passes through the pipeline.
pub fn set_node_warning_outcome_warned(ev: &mut NodeEvaluated) {
    ev.node_outcome_detail = Some(node_evaluated::NodeOutcomeDetail::NodeEvaluationDetail(
        NodeEvaluationDetail {
            node_warning_outcome: NodeWarningOutcome::WithWarnings as i32,
        },
    ));
}

/// Marks the `NodeEvaluated` span as having completed with no warnings.
///
/// Should be called for every successful non-test node that did not emit any warn logs,
/// so consumers can distinguish a clean success from old records that predate this field.
pub fn set_node_warning_outcome_no_warnings(ev: &mut NodeEvaluated) {
    ev.node_outcome_detail = Some(node_evaluated::NodeOutcomeDetail::NodeEvaluationDetail(
        NodeEvaluationDetail {
            node_warning_outcome: NodeWarningOutcome::NoWarnings as i32,
        },
    ));
}

impl From<node_evaluated::NodeOutcomeDetail> for node_processed::NodeOutcomeDetail {
    fn from(detail: node_evaluated::NodeOutcomeDetail) -> Self {
        match detail {
            node_evaluated::NodeOutcomeDetail::NodeCacheDetail(d) => {
                node_processed::NodeOutcomeDetail::NodeCacheDetail(d)
            }
            node_evaluated::NodeOutcomeDetail::NodeTestDetail(d) => {
                node_processed::NodeOutcomeDetail::NodeTestDetail(d)
            }
            node_evaluated::NodeOutcomeDetail::NodeFreshnessOutcome(d) => {
                node_processed::NodeOutcomeDetail::NodeFreshnessOutcome(d)
            }
            node_evaluated::NodeOutcomeDetail::NodeSkipUpstreamDetail(d) => {
                node_processed::NodeOutcomeDetail::NodeSkipUpstreamDetail(d)
            }
            node_evaluated::NodeOutcomeDetail::NodeEvaluationDetail(d) => {
                node_processed::NodeOutcomeDetail::NodeEvaluationDetail(d)
            }
        }
    }
}

impl NodeEvaluated {
    /// Creates a new `NodeEvaluated` event indicating start of a node processing.
    ///
    /// This is thin wrapper around `new` that avoids the need to pass
    /// `None` for fields that are only known at the end of processing.
    ///
    /// # Arguments
    /// * `unique_id` - unique_id is the globally unique identifier for this node.
    /// * `name` - Node name.
    /// * `database` - Database where this node will be created if applicable.
    /// * `schema` - Schema where this node will be created if applicable.
    /// * `identifier` - Name of the relation (table, view, etc.) that will be created for this node if applicable.
    /// * `materialization` - How this node is materialized in the data warehouse.
    /// * `custom_materialization` - If materialization == NODE_MATERIALIZATION_CUSTOM, this field contains the custom materialization name.
    /// * `node_type` - Type of node being evaluated. Known as `resource_type` in dbt core.
    /// * `phase` - Execution phase during which this node was evaluated.
    /// * `relative_path` - The file path to the node, relative to the project root.
    /// * `defined_at_line` - The line number in the file where the test is defined, if applicable.
    /// * `defined_at_column` - The column number in the file where the test is defined, if applicable.
    /// * `node_checksum` - The checksum of the node's contents
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        unique_id: String,
        name: String,
        database: Option<String>,
        schema: Option<String>,
        identifier: Option<String>,
        materialization: Option<NodeMaterialization>,
        custom_materialization: Option<String>,
        node_type: NodeType,
        phase: ExecutionPhase,
        relative_path: String,
        defined_at_line: Option<u32>,
        defined_at_column: Option<u32>,
        node_checksum: String,
    ) -> Self {
        Self::new(
            unique_id,
            name,
            database,
            schema,
            identifier,
            materialization,
            custom_materialization,
            node_type,
            NodeOutcome::Unspecified,
            phase,
            relative_path,
            defined_at_line,
            defined_at_column,
            node_checksum,
            Some(false), // whether sao_enabled or not is only known at task runtime
            None,        // node_error_type
            None,        // node_cancel_reason
            None,        // node_skip_reason
            dbt_core_event_code_for_node_evaluation(phase).map(str::to_string),
            None, // rows_affected
            None, // idle_time_ms
            None, // state_decision_id
            None, // node_outcome_detail
        )
    }
}

impl NodeProcessed {
    /// Creates a new `NodeProcessed` event indicating start of node processing across all phases.
    ///
    /// This is thin wrapper around `new` that avoids the need to pass
    /// `None` for fields that are only known at the end of processing,
    /// and auto assigns the appropriate dbt core event code (Q024) for start of node processing.
    ///
    /// # Arguments
    /// * `unique_id` - unique_id is the globally unique identifier for this node.
    /// * `name` - Node name.
    /// * `database` - Database where this node will be created if applicable.
    /// * `schema` - Schema where this node will be created if applicable.
    /// * `identifier` - Name of the relation (table, view, etc.) that will be created for this node if applicable.
    /// * `materialization` - How this node is materialized in the data warehouse.
    /// * `custom_materialization` - If materialization == NODE_MATERIALIZATION_CUSTOM, this field contains the custom materialization name.
    /// * `node_type` - Type of node being evaluated. Known as `resource_type` in dbt core.
    /// * `last_phase` - Last execution phase that is expected to be run for this node.
    /// * `relative_path` - The file path to the node, relative to the project root.
    /// * `defined_at_line` - The line number in the file where the test is defined, if applicable.
    /// * `defined_at_column` - The column number in the file where the test is defined, if applicable.
    /// * `node_checksum` - The checksum of the node's contents
    /// * `in_selection` - Whether the node is in the selection set for execution
    /// * `group` - Optional group identifier for model notifications
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        unique_id: String,
        name: String,
        database: Option<String>,
        schema: Option<String>,
        identifier: Option<String>,
        materialization: Option<NodeMaterialization>,
        custom_materialization: Option<String>,
        node_type: NodeType,
        last_phase: Option<ExecutionPhase>,
        relative_path: String,
        defined_at_line: Option<u32>,
        defined_at_column: Option<u32>,
        node_checksum: String,
        in_selection: bool,
        group: Option<String>,
    ) -> Self {
        Self::new(
            unique_id,
            name,
            database,
            schema,
            identifier,
            None, // source_name
            materialization,
            custom_materialization,
            node_type,
            NodeOutcome::Unspecified,
            last_phase.unwrap_or_default(),
            relative_path,
            defined_at_line,
            defined_at_column,
            node_checksum,
            Some(false), // whether sao_enabled or not is only known at task runtime
            None,        // node_error_type
            None,        // node_cancel_reason
            None,        // node_skip_reason
            "Q024".to_string(), // dbt_core_event_code
            None,        // duration_ms will be calculated during task execution
            in_selection,
            None, // rows_affected
            group,
            None, // idle_time_ms
            None, // node_index assigned by the span builder at start of processing
            None, // node_count_total assigned by the span builder at start of processing
            None, // node_outcome_detail
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_table_as_static_ref() {
        assert_eq!(
            NodeMaterialization::InteractiveTable.as_static_ref(),
            "interactive_table"
        );
    }

    fn processed(node_type: NodeType, phase: ExecutionPhase) -> NodeProcessed {
        let mut event = NodeProcessed::start(
            "model.project.stg_orders".to_string(),
            "stg_orders".to_string(),
            None,
            Some("analytics".to_string()),
            Some("stg_orders".to_string()),
            None,
            None,
            node_type,
            Some(phase),
            "models/staging/stg_orders.sql".to_string(),
            None,
            None,
            "checksum".to_string(),
            true,
            None,
        );
        event.set_node_outcome(NodeOutcome::Success);
        event
    }

    /// The freshness phase reports a freshness result for every node kind it measures, so a
    /// model measured there is Q018, not the model-build Q012.
    #[test]
    fn model_in_freshness_phase_gets_q018() {
        let mut event = processed(NodeType::Model, ExecutionPhase::FreshnessAnalysis);
        update_dbt_core_event_code_for_node_processed_end(&mut event);
        assert_eq!(event.dbt_core_event_code, "Q018");
    }

    #[test]
    fn source_in_freshness_phase_gets_q018() {
        let mut event = processed(NodeType::Source, ExecutionPhase::FreshnessAnalysis);
        update_dbt_core_event_code_for_node_processed_end(&mut event);
        assert_eq!(event.dbt_core_event_code, "Q018");
    }

    /// The phase is what makes it a freshness result — a model built by `run` is unaffected.
    #[test]
    fn model_outside_freshness_phase_keeps_q012() {
        let mut event = processed(NodeType::Model, ExecutionPhase::Run);
        update_dbt_core_event_code_for_node_processed_end(&mut event);
        assert_eq!(event.dbt_core_event_code, "Q012");
    }

    /// Skip outcomes are resolved before the phase check.
    #[test]
    fn noop_skip_in_freshness_phase_still_gets_q019() {
        let mut event = processed(NodeType::Model, ExecutionPhase::FreshnessAnalysis);
        event.set_node_outcome(NodeOutcome::Skipped);
        event.set_node_skip_reason(NodeSkipReason::NoOp);
        update_dbt_core_event_code_for_node_processed_end(&mut event);
        assert_eq!(event.dbt_core_event_code, "Q019");
    }
}
