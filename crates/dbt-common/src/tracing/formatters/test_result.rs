//! Formatter for test results.
//!
//! This module provides formatters for test failure messages with optional
//! color formatting.

use crate::pretty_string::RED;
use dbt_telemetry::node_processed::NodeOutcomeDetail;
use dbt_telemetry::{NodeProcessed, TestOutcome};

use super::color::maybe_apply_color;

/// Format a test failure message with "Test failed: " prefix and optional colorization.
///
/// # Arguments
/// * `node` - The `NodeProcessed` span data for the failed test node
/// * `colorize` - If true, applies red color to the "Test failed:" prefix
///
/// # Returns
/// A formatted failure message. When available, includes the pretty-printed
/// diff table or a `select *` query pointing at the stored failures relation;
/// otherwise falls back to a single line citing the failing-row count so that
/// failed tests are never silently dropped from the output.
/// `None` only if the test outcome is not `Failed`.
pub fn format_test_failure(node: &NodeProcessed, colorize: bool) -> Option<String> {
    let test_detail = if let Some(NodeOutcomeDetail::NodeTestDetail(test_detail)) =
        &node.node_outcome_detail
        && test_detail.test_outcome() == TestOutcome::Failed
    {
        test_detail
    } else {
        return None;
    };

    let test_name = &node.name;
    let prefix = maybe_apply_color(&RED, "Test failed", colorize);
    let failing_rows = test_detail.failing_rows;

    if let Some(diff_table) = &test_detail.diff_table {
        Some(format!("{prefix}: {test_name}\n{diff_table}"))
    } else if test_detail.store_failures.unwrap_or(false)
        && let Some(db) = &node.database
        && let Some(schema) = &node.schema
        && let Some(ident) = &node.identifier
    {
        let sql = format!("select * from {db}.{schema}.{ident}");
        Some(format!(
            "{prefix} ({failing_rows} failed row(s)): {test_name}\nSee test failures:\n{sql}"
        ))
    } else {
        Some(format!(
            "{prefix} ({failing_rows} failed row(s)): {test_name}"
        ))
    }
}
