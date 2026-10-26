//! Formatter for test results.
//!
//! This module provides formatters for test failure messages with optional
//! color formatting.

use std::fmt;

use crate::pretty_string::RED;
use dbt_telemetry::node_processed::NodeOutcomeDetail;
use dbt_telemetry::{NodeProcessed, TestEvaluationDetail, TestOutcome};

use super::color::MaybeColored;

/// Displays a test failure message with "Test failed" prefix and optional colorization.
///
/// When available, includes the pretty-printed diff table or a `select *` query pointing
/// at the stored failures relation; otherwise falls back to a single line citing the
/// failing-row count so that failed tests are never silently dropped from the output.
pub struct TestFailure<'a> {
    node: &'a NodeProcessed,
    detail: &'a TestEvaluationDetail,
    colorize: bool,
}

impl<'a> TestFailure<'a> {
    /// `None` if the test outcome of `node` is not `Failed`.
    pub fn new(node: &'a NodeProcessed, colorize: bool) -> Option<Self> {
        match &node.node_outcome_detail {
            Some(NodeOutcomeDetail::NodeTestDetail(detail))
                if detail.test_outcome() == TestOutcome::Failed =>
            {
                Some(Self {
                    node,
                    detail,
                    colorize,
                })
            }
            _ => None,
        }
    }
}

impl fmt::Display for TestFailure<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self.node;
        let test_name = &node.name;
        let prefix = MaybeColored::new(&RED, "Test failed", self.colorize);
        let failing_rows = self.detail.failing_rows;

        if let Some(diff_table) = &self.detail.diff_table {
            write!(f, "{prefix}: {test_name}\n{diff_table}")
        } else if self.detail.store_failures.unwrap_or(false)
            && let Some(db) = &node.database
            && let Some(schema) = &node.schema
            && let Some(ident) = &node.identifier
        {
            write!(
                f,
                "{prefix} ({failing_rows} failed row(s)): {test_name}\nSee test failures:\nselect * from {db}.{schema}.{ident}"
            )
        } else {
            write!(f, "{prefix} ({failing_rows} failed row(s)): {test_name}")
        }
    }
}
