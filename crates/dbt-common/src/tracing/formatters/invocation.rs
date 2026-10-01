use std::time::Duration;

use console::Style;

use dbt_telemetry::{Invocation, NodeType};
use dbt_tracing::SpanEndInfo;
use itertools::Itertools;

use crate::tracing::{
    data_provider::DataProvider,
    dbt_metrics::{FusionMetricKey, InvocationMetricKey},
    formatters::{
        color::{BLUE, DIM, GREEN, MAGENTA, RED, WHITE, YELLOW, maybe_apply_color},
        layout::format_delimiter,
    },
};

use super::duration::format_duration_for_summary;

/// Commands that skip the entire Execution Summary banner (no status line, no result breakdown).
const RESULT_LINE_OPT_OUT_COMMANDS: [&str; 2] = ["man", "login"];
/// Commands that should include the extended evaluation/result breakdown.
const SUMMARY_COMMANDS: [&str; 7] = [
    "build", "compile", "run", "sample", "seed", "snapshot", "test",
];

/// Type sfe way to get ordering of all supported node types for summary display.
const fn node_type_to_order(node_type: NodeType) -> u8 {
    match node_type {
        NodeType::Model => 1,
        NodeType::Test => 2,
        NodeType::Snapshot => 3,
        NodeType::Seed => 4,
        NodeType::Source => 5,
        NodeType::Exposure => 6,
        NodeType::Metric => 7,
        NodeType::SemanticModel => 8,
        NodeType::SavedQuery => 9,
        NodeType::Analysis => 10,
        NodeType::Operation => 11,
        NodeType::UnitTest => 12,
        // Grouped with the other validation resources rather than the build ones.
        NodeType::Check => 13,
        NodeType::Function => 14,
        NodeType::Macro => 15,
        NodeType::DocsMacro => 16,
        NodeType::Unspecified => 17,
    }
}

/// Get the plural name of a NodeType
pub fn node_type_plural(node: NodeType) -> &'static str {
    match node {
        NodeType::Unspecified => "unspecified",
        NodeType::Model => "models",
        NodeType::Seed => "seeds",
        NodeType::Snapshot => "snapshots",
        NodeType::Source => "sources",
        NodeType::Test => "tests",
        NodeType::UnitTest => "unit tests",
        NodeType::Macro => "macros",
        NodeType::DocsMacro => "doc macros",
        NodeType::Analysis => "analyses",
        NodeType::Operation => "operations",
        NodeType::Exposure => "exposures",
        NodeType::Metric => "metrics",
        NodeType::SavedQuery => "saved queries",
        NodeType::SemanticModel => "semantic models",
        NodeType::Function => "functions",
        NodeType::Check => "checks",
    }
}

#[derive(Debug)]
struct InvocationOutcomeTotals {
    total: u64,
    success: u64,
    warn: u64,
    error: u64,
    reused: u64,
    skipped: u64,
    canceled: u64,
    no_op: u64,
}

/// Test work the engine avoided on the user's behalf.
#[derive(Debug)]
struct TestOptimizationTotals {
    statically_checked: u64,
    aggregated_tests: u64,
    aggregated_queries: u64,
}

#[derive(Debug)]
struct InvocationMetricsSnapshot {
    warnings: u64,
    errors: u64,
    autofix: u64,
    outcomes: InvocationOutcomeTotals,
    test_optimization: TestOptimizationTotals,
}

#[derive(Debug)]
struct InvocationSummaryInput<'a> {
    command: &'a str,
    target: Option<&'a str>,
    elapsed: Duration,
    metrics: InvocationMetricsSnapshot,
}

#[derive(Debug)]
pub struct FormattedInvocationSummary {
    summary_lines: Option<Vec<String>>,
    autofix_line: Option<String>,
}

impl FormattedInvocationSummary {
    pub fn summary_lines(&self) -> Option<&[String]> {
        self.summary_lines.as_deref()
    }

    pub fn autofix_line(&self) -> Option<&str> {
        self.autofix_line.as_deref()
    }
}

/// Extract structured invocation data from span attributes if available.
fn extract_invocation_command_and_target(attributes: &Invocation) -> (&str, Option<&str>) {
    let command = attributes
        .eval_args
        .as_ref()
        .map(|args| args.command.as_str())
        .unwrap_or("unknown");

    let target = attributes
        .eval_args
        .as_ref()
        .and_then(|args| args.target.as_deref());

    (command, target)
}

fn collect_outcome_totals(data_provider: &DataProvider<'_>) -> InvocationOutcomeTotals {
    let success = data_provider.get_metric(FusionMetricKey::InvocationMetric(
        InvocationMetricKey::NodeTotalsSuccess,
    ));
    let warn = data_provider.get_metric(FusionMetricKey::InvocationMetric(
        InvocationMetricKey::NodeTotalsWarning,
    ));
    let error = data_provider.get_metric(FusionMetricKey::InvocationMetric(
        InvocationMetricKey::NodeTotalsError,
    ));
    let reused = data_provider.get_metric(FusionMetricKey::InvocationMetric(
        InvocationMetricKey::NodeTotalsReused,
    ));
    let skipped = data_provider.get_metric(FusionMetricKey::InvocationMetric(
        InvocationMetricKey::NodeTotalsSkipped,
    ));
    let canceled = data_provider.get_metric(FusionMetricKey::InvocationMetric(
        InvocationMetricKey::NodeTotalsCanceled,
    ));
    let no_op = data_provider.get_metric(FusionMetricKey::InvocationMetric(
        InvocationMetricKey::NodeTotalsNoOp,
    ));

    InvocationOutcomeTotals {
        total: success + warn + error + reused + skipped + canceled + no_op,
        success,
        warn,
        error,
        reused,
        skipped,
        canceled,
        no_op,
    }
}

fn collect_test_optimization_totals(data_provider: &DataProvider<'_>) -> TestOptimizationTotals {
    TestOptimizationTotals {
        statically_checked: data_provider.get_metric(FusionMetricKey::InvocationMetric(
            InvocationMetricKey::StaticallyCheckedTests,
        )),
        aggregated_tests: data_provider.get_metric(FusionMetricKey::InvocationMetric(
            InvocationMetricKey::AggregatedTests,
        )),
        aggregated_queries: data_provider.get_metric(FusionMetricKey::InvocationMetric(
            InvocationMetricKey::AggregatedTestQueries,
        )),
    }
}

/// Collects invocation-level metrics exposed through the data provider for the Invocation span.
fn collect_invocation_metrics(data_provider: &DataProvider<'_>) -> InvocationMetricsSnapshot {
    let warnings = data_provider.get_metric(FusionMetricKey::InvocationMetric(
        InvocationMetricKey::TotalWarnings,
    ));
    let errors = data_provider.get_metric(FusionMetricKey::InvocationMetric(
        InvocationMetricKey::TotalErrors,
    ));
    let autofix = data_provider.get_metric(FusionMetricKey::InvocationMetric(
        InvocationMetricKey::AutoFixSuggestions,
    ));

    let outcomes = collect_outcome_totals(data_provider);
    let test_optimization = collect_test_optimization_totals(data_provider);

    InvocationMetricsSnapshot {
        warnings,
        errors,
        autofix,
        outcomes,
        test_optimization,
    }
}

/// Formats invocation summary output (both colored and non-colored variants).
pub fn format_invocation_summary(
    span: &SpanEndInfo,
    invocation: &Invocation,
    data_provider: &DataProvider<'_>,
    colorize: bool,
    max_line_width: Option<usize>,
) -> FormattedInvocationSummary {
    let (command, target) = extract_invocation_command_and_target(invocation);

    // Exit early if the command is opted out of summary display
    if RESULT_LINE_OPT_OUT_COMMANDS
        .iter()
        .any(|cmd| cmd.eq_ignore_ascii_case(command))
    {
        return FormattedInvocationSummary {
            summary_lines: None,
            autofix_line: None,
        };
    }

    let metrics: InvocationMetricsSnapshot = collect_invocation_metrics(data_provider);
    let elapsed = span
        .end_time_unix_nano
        .duration_since(span.start_time_unix_nano)
        .unwrap_or_default();

    let summary = InvocationSummaryInput {
        command,
        target,
        elapsed,
        metrics,
    };

    let is_summary_command = SUMMARY_COMMANDS
        .iter()
        .any(|command| command.eq_ignore_ascii_case(summary.command));

    let mut lines = Vec::new();

    // Start with a blank line for spacing
    lines.push(String::new());

    // Supplementary sections come before the headline block, so the verdict stays last.
    if is_summary_command
        && let Some(section) = format_test_optimization_section(
            &summary.metrics.test_optimization,
            max_line_width,
            colorize,
        )
    {
        lines.extend(section);
        lines.push(String::new());
    }

    // Insert a centered execution summary delimiter line
    let header = format_delimiter(" Execution Summary ", max_line_width, colorize);
    lines.push(header);

    lines.push(format_status_line(&summary, colorize));

    if is_summary_command {
        if let Some(evaluated) = format_evaluated_line(data_provider, colorize) {
            lines.push(evaluated);
        }
        if let Some(result) = format_result_line(&summary.metrics.outcomes, colorize) {
            lines.push(result);
        }
    }

    let autofix_line = if summary.metrics.autofix > 0 {
        Some(format_autofix_line(colorize))
    } else {
        None
    };

    FormattedInvocationSummary {
        summary_lines: Some(lines),
        autofix_line,
    }
}

fn format_status_line(summary: &InvocationSummaryInput<'_>, colorize: bool) -> String {
    let duration = maybe_apply_color(
        &DIM,
        format!("[{}]", format_duration_for_summary(summary.elapsed)).as_str(),
        colorize,
    );
    let status = format_status_text(summary.metrics.errors, summary.metrics.warnings, colorize);

    let command = maybe_apply_color(&WHITE, summary.command, colorize);
    let maybe_target = summary
        .target
        .map(|target| {
            format!(
                " for target '{}'",
                maybe_apply_color(&WHITE, target, colorize)
            )
        })
        .unwrap_or_default();

    format!("Finished '{command}' {status}{maybe_target} {duration}")
}

fn format_status_text(errors: u64, warnings: u64, colorize: bool) -> String {
    match (errors, warnings) {
        (0, 0) => maybe_apply_color(&GREEN, "successfully", colorize),
        (0, warn) => format!(
            "with {}",
            colored_count(warn, "warning", "warnings", colorize, &YELLOW)
        ),
        (err, 0) => format!(
            "with {}",
            colored_count(err, "error", "errors", colorize, &RED)
        ),
        (err, warn) => format!(
            "with {} and {}",
            colored_count(warn, "warning", "warnings", colorize, &RED),
            colored_count(err, "error", "errors", colorize, &RED),
        ),
    }
}

fn format_evaluated_line(data_provider: &DataProvider<'_>, _colorize: bool) -> Option<String> {
    let mut parts = Vec::new();

    // First, add hooks if any
    let hook_count = data_provider.get_metric(FusionMetricKey::HookCounts);
    if hook_count > 0 {
        let word = if hook_count > 1 { "hooks" } else { "hook" };
        parts.push(format!("{} {}", hook_count, word));
    }

    // Then add nodes by type
    for (node_type, count) in data_provider
        .get_all_metrics()
        .iter()
        .filter_map(|(key, count)| match FusionMetricKey::try_from(*key).ok() {
            Some(FusionMetricKey::NodeCounts(node_type)) => Some((node_type, count)),
            _ => None,
        })
        .sorted_by(|a, b| {
            let order_a = node_type_to_order(a.0);
            let order_b = node_type_to_order(b.0);
            order_a.cmp(&order_b)
        })
    {
        if *count == 0 {
            continue;
        }

        let word = if *count > 1 {
            node_type_plural(node_type)
        } else {
            node_type.pretty()
        };

        parts.push(format!("{} {}", count, word));
    }

    if parts.is_empty() {
        return None;
    }

    Some(format!("Processed: {}", parts.join(" | ")))
}

/// Reports test work the engine avoided: statically proven tests and aggregated
/// queries. Returns `None` when neither optimization did anything, so the whole
/// section is absent by default.
fn format_test_optimization_section(
    totals: &TestOptimizationTotals,
    max_line_width: Option<usize>,
    colorize: bool,
) -> Option<Vec<String>> {
    // Body lines are intentionally unstyled in both colour modes.
    let mut body = Vec::new();

    if totals.statically_checked > 0 {
        body.push(format!(
            "{} statically checked (0 queries)",
            count_text(totals.statically_checked, "test", "tests"),
        ));
    }

    // Both counters are incremented together, so either alone being zero means
    // they are inconsistent and reporting a saving would be misleading.
    if totals.aggregated_tests > 0 && totals.aggregated_queries > 0 {
        body.push(format!(
            "{} batched (reduced to {})",
            count_text(totals.aggregated_tests, "test", "tests"),
            count_text(totals.aggregated_queries, "query", "queries"),
        ));
    }

    if body.is_empty() {
        return None;
    }

    let mut lines = vec![format_delimiter(
        " Test Optimization ",
        max_line_width,
        colorize,
    )];
    lines.extend(body);

    Some(lines)
}

fn format_result_line(outcomes: &InvocationOutcomeTotals, colorize: bool) -> Option<String> {
    if outcomes.total == 0 {
        return None;
    }

    let mut segments = Vec::new();
    segments.push(colored_metric(outcomes.total, "total", colorize, &WHITE));

    if outcomes.success > 0 {
        segments.push(colored_metric(
            outcomes.success,
            "success",
            colorize,
            &GREEN,
        ));
    }

    if outcomes.reused > 0 {
        segments.push(colored_metric(
            outcomes.reused,
            "reused",
            colorize,
            &MAGENTA,
        ));
    }
    if outcomes.warn > 0 {
        segments.push(colored_metric(outcomes.warn, "warn", colorize, &YELLOW));
    }
    if outcomes.error > 0 {
        segments.push(colored_metric(outcomes.error, "error", colorize, &RED));
    }
    if outcomes.skipped > 0 {
        segments.push(colored_metric(outcomes.skipped, "skipped", colorize, &DIM));
    }
    if outcomes.canceled > 0 {
        segments.push(colored_metric(
            outcomes.canceled,
            "canceled",
            colorize,
            &DIM,
        ));
    }
    if outcomes.no_op > 0 {
        segments.push(colored_metric(outcomes.no_op, "no-op", colorize, &DIM));
    }

    Some(format!("Summary: {}", segments.join(" | ")))
}

fn colored_metric(value: u64, label: &str, colorize: bool, style: &Style) -> String {
    let text = format!("{} {}", value, label);
    maybe_apply_color(style, &text, colorize)
}

fn count_text(value: u64, label_single: &str, label_plural: &str) -> String {
    let word = if value == 1 {
        label_single
    } else {
        label_plural
    };

    format!("{} {}", value, word)
}

fn colored_count(
    value: u64,
    label_single: &str,
    label_plural: &str,
    colorize: bool,
    style: &Style,
) -> String {
    maybe_apply_color(
        style,
        &count_text(value, label_single, label_plural),
        colorize,
    )
}

fn format_autofix_line(colorize: bool) -> String {
    let suggestion_label = maybe_apply_color(&BLUE, "suggestion:", colorize);
    let command = maybe_apply_color(&YELLOW, "dbt deps", colorize);
    let url = maybe_apply_color(&BLUE, "https://github.com/dbt-labs/dbt-autofix", colorize);

    format!(
        "{suggestion_label} Run '{}' to see the latest fusion compatible packages. For compatibility errors, try the autofix script: {url}",
        command
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: Option<usize> = Some(60);

    fn section(
        statically_checked: u64,
        aggregated_tests: u64,
        aggregated_queries: u64,
    ) -> Option<Vec<String>> {
        format_test_optimization_section(
            &TestOptimizationTotals {
                statically_checked,
                aggregated_tests,
                aggregated_queries,
            },
            WIDTH,
            false,
        )
    }

    #[test]
    fn test_optimization_section_absent_when_nothing_optimized() {
        assert_eq!(section(0, 0, 0), None);
    }

    #[test]
    fn test_optimization_section_static_only() {
        let lines = section(3, 0, 0).expect("section should be present");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], "3 tests statically checked (0 queries)");
        assert!(!lines.iter().any(|line| line.contains("batched")));
    }

    #[test]
    fn test_optimization_section_aggregated_only() {
        let lines = section(0, 8, 4).expect("section should be present");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], "8 tests batched (reduced to 4 queries)");
        assert!(!lines.iter().any(|line| line.contains("statically")));
    }

    #[test]
    fn test_optimization_section_both() {
        let lines = section(4, 4, 2).expect("section should be present");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1], "4 tests statically checked (0 queries)");
        assert_eq!(lines[2], "4 tests batched (reduced to 2 queries)");
    }

    #[test]
    fn test_optimization_section_singular_counts() {
        let lines = section(1, 2, 1).expect("section should be present");
        assert_eq!(lines[1], "1 test statically checked (0 queries)");
        assert_eq!(lines[2], "2 tests batched (reduced to 1 query)");
    }

    #[test]
    fn test_optimization_section_omits_aggregation_line_without_queries() {
        let lines = section(1, 8, 0).expect("section should be present");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], "1 test statically checked (0 queries)");
    }

    #[test]
    fn test_optimization_banner_matches_execution_summary_shape() {
        let lines = section(1, 0, 0).expect("section should be present");
        assert_eq!(
            lines[0],
            "==================== Test Optimization ====================="
        );
        assert_eq!(
            lines[0].len(),
            format_delimiter(" Execution Summary ", WIDTH, false).len()
        );
    }
}
