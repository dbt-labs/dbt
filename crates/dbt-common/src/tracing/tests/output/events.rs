//! Event builders and emitters shared by the output tests.

use dbt_telemetry::{
    ExecutionPhase, Invocation, InvocationEvalArgs, NodeEvaluated, NodeMaterialization,
    NodeOutcome, NodeProcessed, NodeType, TestEvaluationDetail, TestOutcome,
    node_processed::NodeOutcomeDetail,
};

use super::support::Clock;
use crate::tracing::{
    emit::create_info_span,
    layers::tui_layer::{TuiAggregatedTestGroup, TuiAllProcessingNodesGroup},
};

/// Identifier the output tests give the invocation.
pub(super) const INVOCATION_ID: &str = "0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69";

/// An invocation of `command` against target `dev`.
pub(super) fn invocation(command: &str) -> Invocation {
    Invocation {
        invocation_id: INVOCATION_ID.to_string(),
        raw_command: format!("dbt {command}"),
        eval_args: Some(InvocationEvalArgs {
            command: command.to_string(),
            target: Some("dev".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A successful node of `node_type`, as the scheduler reports it in the run phase:
/// the 3rd of 12 selected nodes, 250ms of work.
pub(super) fn node(node_type: NodeType) -> NodeProcessed {
    let (name, schema, materialization, relative_path, defined_at) = match node_type {
        NodeType::Model => (
            "customers",
            "analytics",
            Some(NodeMaterialization::Table),
            "models/customers.sql",
            None,
        ),
        NodeType::Seed => (
            "raw_orders",
            "analytics",
            Some(NodeMaterialization::Seed),
            "seeds/raw_orders.csv",
            None,
        ),
        NodeType::Snapshot => (
            "orders_snapshot",
            "snapshots",
            Some(NodeMaterialization::Snapshot),
            "snapshots/orders_snapshot.sql",
            None,
        ),
        NodeType::Source => (
            "orders",
            "raw_data",
            None,
            "models/sources.yml",
            Some((3, 9)),
        ),
        NodeType::Test => (
            "not_null_customers_customer_id",
            "analytics_dbt_test__audit",
            Some(NodeMaterialization::Test),
            "models/schema.yml",
            Some((12, 15)),
        ),
        NodeType::UnitTest => (
            "test_customer_totals",
            "analytics",
            Some(NodeMaterialization::Unit),
            "models/schema.yml",
            Some((40, 7)),
        ),
        NodeType::Analysis => (
            "order_trends",
            "analytics",
            Some(NodeMaterialization::Analysis),
            "analyses/order_trends.sql",
            None,
        ),
        NodeType::Function => (
            "to_cents",
            "analytics",
            Some(NodeMaterialization::Function),
            "functions/to_cents.sql",
            None,
        ),
        NodeType::Check => (
            "orders_are_recent",
            "analytics",
            Some(NodeMaterialization::Analysis),
            "checks/orders_are_recent.sql",
            None,
        ),
        NodeType::Plugin => (
            "orders_widget",
            "analytics",
            None,
            "widgets/orders_widget.yml",
            None,
        ),
        NodeType::Unspecified
        | NodeType::Macro
        | NodeType::DocsMacro
        | NodeType::Operation
        | NodeType::Exposure
        | NodeType::Metric
        | NodeType::SavedQuery
        | NodeType::SemanticModel => ("thing", "analytics", None, "models/things.yml", None),
    };

    NodeProcessed {
        unique_id: format!("{}.jaffle_shop.{name}", node_type.as_static_ref()),
        name: name.to_string(),
        database: Some("warehouse".to_string()),
        schema: Some(schema.to_string()),
        identifier: (!matches!(node_type, NodeType::Check | NodeType::Plugin))
            .then(|| name.to_string()),
        source_name: (node_type == NodeType::Source).then(|| "jaffle_raw".to_string()),
        plugin_type: (node_type == NodeType::Plugin).then(|| "widget".to_string()),
        materialization: materialization.map(|m| m as i32),
        node_type: node_type as i32,
        node_outcome: NodeOutcome::Success as i32,
        last_phase: ExecutionPhase::Run as i32,
        relative_path: relative_path.to_string(),
        defined_at_line: defined_at.map(|(line, _)| line),
        defined_at_col: defined_at.map(|(_, col)| col),
        node_checksum: "checksum".to_string(),
        duration_ms: Some(250),
        in_selection: true,
        node_index: Some(3),
        node_count_total: Some(12),
        ..Default::default()
    }
}

/// A data test that executed and reached `outcome` with `failing_rows`.
pub(super) fn data_test(outcome: TestOutcome, failing_rows: i32) -> NodeProcessed {
    with_test_detail(node(NodeType::Test), outcome, failing_rows)
}

/// `node` carrying a test verdict.
pub(super) fn with_test_detail(
    mut node: NodeProcessed,
    outcome: TestOutcome,
    failing_rows: i32,
) -> NodeProcessed {
    node.node_outcome_detail = Some(NodeOutcomeDetail::NodeTestDetail(
        TestEvaluationDetail::new(outcome, failing_rows, None, None, None, None),
    ));
    node
}

/// Mutable access to the test verdict of a node built by [`with_test_detail`].
pub(super) fn test_detail(node: &mut NodeProcessed) -> &mut TestEvaluationDetail {
    match &mut node.node_outcome_detail {
        Some(NodeOutcomeDetail::NodeTestDetail(detail)) => detail,
        _ => panic!("node has no test detail"),
    }
}

/// The `NodeEvaluated` span of `node` in `phase`.
pub(super) fn evaluated(node: &NodeProcessed, phase: ExecutionPhase) -> NodeEvaluated {
    NodeEvaluated {
        unique_id: node.unique_id.clone(),
        name: node.name.clone(),
        database: node.database.clone(),
        schema: node.schema.clone(),
        identifier: node.identifier.clone(),
        materialization: node.materialization,
        custom_materialization: node.custom_materialization.clone(),
        node_type: node.node_type,
        node_outcome: node.node_outcome,
        phase: phase as i32,
        relative_path: node.relative_path.clone(),
        defined_at_line: node.defined_at_line,
        defined_at_col: node.defined_at_col,
        node_checksum: node.node_checksum.clone(),
        node_skip_reason: node.node_skip_reason,
        ..Default::default()
    }
}

/// Runs `nodes` under the span the scheduler groups every processed node under.
pub(super) fn node_group(nodes: impl FnOnce()) {
    create_info_span(TuiAllProcessingNodesGroup).in_scope(nodes);
}

/// Runs `members` under the span grouping the tests of one aggregated query.
pub(super) fn aggregated_test_group(macro_name: &str, attached_node: &str, members: impl FnOnce()) {
    create_info_span(TuiAggregatedTestGroup {
        macro_name: macro_name.to_string(),
        attached_node: attached_node.to_string(),
    })
    .in_scope(members);
}

/// Emits `node` as a finished `NodeProcessed` span, open for its `duration_ms`.
pub(super) fn processed(clock: &Clock, node: NodeProcessed) {
    let duration_ms = node.duration_ms.unwrap_or_default();
    let span = create_info_span(node);
    clock.advance_ms(duration_ms);
    drop(span);
}
