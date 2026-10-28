use std::{fmt, time::Duration};

use console::Style;

use dbt_telemetry::{Invocation, NodeType};
use dbt_tracing::SpanEndInfo;
use itertools::Itertools;

use crate::tracing::{
    data_provider::DataProvider,
    dbt_metrics::{FusionMetricKey, InvocationMetricKey},
    formatters::{
        color::{BLUE, DIM, GREEN, MAGENTA, MaybeColored, RED, WHITE, YELLOW},
        layout::Delimiter,
    },
};

use super::duration::SummaryDuration;

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
        NodeType::Plugin => u8::MAX,
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
        NodeType::Plugin => "plugin nodes",
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

/// Whether `command` shows the extended evaluation/result breakdown.
fn is_summary_command(command: &str) -> bool {
    SUMMARY_COMMANDS
        .iter()
        .any(|summary_command| summary_command.eq_ignore_ascii_case(command))
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

/// Invocation summary output (both colored and non-colored variants).
///
/// Metrics are read once on construction; the summary is only rendered when displayed.
pub struct InvocationSummary<'a> {
    /// `None` if the command is opted out of summary display
    input: Option<InvocationSummaryInput<'a>>,
    /// Number of hooks run, shown before the node counts
    hook_count: u64,
    /// Processed nodes per type, in display order, without zeros
    node_counts: Vec<(NodeType, u64)>,
    colorize: bool,
    max_line_width: Option<usize>,
}

impl<'a> InvocationSummary<'a> {
    pub fn new(
        span: &SpanEndInfo,
        invocation: &'a Invocation,
        data_provider: &DataProvider<'_>,
        colorize: bool,
        max_line_width: Option<usize>,
    ) -> Self {
        let (command, target) = extract_invocation_command_and_target(invocation);

        // Exit early if the command is opted out of summary display
        if RESULT_LINE_OPT_OUT_COMMANDS
            .iter()
            .any(|cmd| cmd.eq_ignore_ascii_case(command))
        {
            return Self {
                input: None,
                hook_count: 0,
                node_counts: Vec::new(),
                colorize,
                max_line_width,
            };
        }

        let metrics: InvocationMetricsSnapshot = collect_invocation_metrics(data_provider);
        let elapsed = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();

        let input = InvocationSummaryInput {
            command,
            target,
            elapsed,
            metrics,
        };

        let (hook_count, node_counts) = if is_summary_command(command) {
            let node_counts = data_provider
                .get_all_metrics()
                .iter()
                .filter_map(|(key, count)| match FusionMetricKey::try_from(*key).ok() {
                    Some(FusionMetricKey::NodeCounts(node_type)) if *count > 0 => {
                        Some((node_type, *count))
                    }
                    _ => None,
                })
                .sorted_by_key(|(node_type, _)| node_type_to_order(*node_type))
                .collect();
            (
                data_provider.get_metric(FusionMetricKey::HookCounts),
                node_counts,
            )
        } else {
            (0, Vec::new())
        };

        Self {
            input: Some(input),
            hook_count,
            node_counts,
            colorize,
            max_line_width,
        }
    }

    /// The summary lines, or `None` if the command shows no summary.
    pub fn summary_lines(&self) -> Option<SummaryLines<'_>> {
        self.input.as_ref().map(|input| SummaryLines {
            summary: self,
            input,
        })
    }

    /// The suggestion to run autofix, if any compatibility issue suggested one.
    pub fn autofix_line(&self) -> Option<AutofixLine> {
        self.input
            .as_ref()
            .filter(|input| input.metrics.autofix > 0)
            .map(|_| AutofixLine {
                colorize: self.colorize,
            })
    }
}

/// The lines of an [`InvocationSummary`], separated by newlines.
pub struct SummaryLines<'s> {
    summary: &'s InvocationSummary<'s>,
    input: &'s InvocationSummaryInput<'s>,
}

impl fmt::Display for SummaryLines<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let input = self.input;
        let colorize = self.summary.colorize;
        let max_line_width = self.summary.max_line_width;
        let is_summary_command = is_summary_command(input.command);

        // Start with a blank line for spacing: every line below starts with a newline.

        // Supplementary sections come before the headline block, so the verdict stays last.
        if is_summary_command {
            let totals = &input.metrics.test_optimization;
            let statically_checked = totals.statically_checked > 0;
            // Both counters are incremented together, so either alone being zero means
            // they are inconsistent and reporting a saving would be misleading.
            let batched = totals.aggregated_tests > 0 && totals.aggregated_queries > 0;
            if statically_checked || batched {
                write!(
                    f,
                    "\n{}",
                    Delimiter::new(" Test Optimization ", max_line_width, colorize)
                )?;
                // Test optimization body lines are intentionally unstyled in both colour modes.
                if statically_checked {
                    write!(
                        f,
                        "\n{} statically checked (0 queries)",
                        CountText::new(totals.statically_checked, "test", "tests"),
                    )?;
                }
                if batched {
                    write!(
                        f,
                        "\n{} batched (reduced to {})",
                        CountText::new(totals.aggregated_tests, "test", "tests"),
                        CountText::new(totals.aggregated_queries, "query", "queries"),
                    )?;
                }
                f.write_str("\n")?;
            }
        }

        // A centered execution summary delimiter line
        write_delimiter(f, " Execution Summary ", max_line_width, colorize)?;
        write_status_line(f, input, colorize)?;

        if is_summary_command {
            if self.summary.hook_count > 0 || !self.summary.node_counts.is_empty() {
                f.write_str("\n")?;
                self.write_processed_line(f)?;
            }
            if input.metrics.outcomes.total > 0 {
                f.write_str("\n")?;
                write_result_line(f, &input.metrics.outcomes, colorize)?;
            }
        }

        // Close the block with a matching delimiter so one invocation's end is
        // distinguishable from the next one's start in scrollback.
        write_delimiter(f, " End of Execution ", max_line_width, colorize)?;
        Ok(())
    }
}

impl SummaryLines<'_> {
    fn write_processed_line(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Processed: ")?;
        let mut separator = "";

        // First, add hooks if any
        let hook_count = self.summary.hook_count;
        if hook_count > 0 {
            let word = if hook_count > 1 { "hooks" } else { "hook" };
            write!(f, "{} {}", hook_count, word)?;
            separator = " | ";
        }

        // Then add nodes by type
        for &(node_type, count) in &self.summary.node_counts {
            let word = if count > 1 {
                node_type_plural(node_type)
            } else {
                node_type.pretty()
            };
            write!(f, "{separator}{} {}", count, word)?;
            separator = " | ";
        }
        Ok(())
    }
}

fn write_delimiter(
    f: &mut fmt::Formatter<'_>,
    label: &str,
    max_line_width: Option<usize>,
    colorize: bool,
) -> fmt::Result {
    write!(f, "\n{}\n", Delimiter::new(label, max_line_width, colorize))
}

fn write_status_line(
    f: &mut fmt::Formatter<'_>,
    summary: &InvocationSummaryInput<'_>,
    colorize: bool,
) -> fmt::Result {
    write!(
        f,
        "Finished '{}' ",
        MaybeColored::new(&WHITE, summary.command, colorize)
    )?;
    write_status_text(
        f,
        summary.metrics.errors,
        summary.metrics.warnings,
        colorize,
    )?;
    if let Some(target) = summary.target {
        write!(
            f,
            " for target '{}'",
            MaybeColored::new(&WHITE, target, colorize)
        )?;
    }
    write!(
        f,
        " {}",
        MaybeColored::new(&DIM, BracketedDuration(summary.elapsed), colorize)
    )
}

/// `[{duration}]` of the whole invocation.
struct BracketedDuration(Duration);

impl fmt::Display for BracketedDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", SummaryDuration(self.0))
    }
}

fn write_status_text(
    f: &mut fmt::Formatter<'_>,
    errors: u64,
    warnings: u64,
    colorize: bool,
) -> fmt::Result {
    let colored_count = |value, single, plural, style| {
        MaybeColored::new(style, CountText::new(value, single, plural), colorize)
    };
    match (errors, warnings) {
        (0, 0) => write!(f, "{}", MaybeColored::new(&GREEN, "successfully", colorize)),
        (0, warn) => write!(
            f,
            "with {}",
            colored_count(warn, "warning", "warnings", &YELLOW)
        ),
        (err, 0) => write!(f, "with {}", colored_count(err, "error", "errors", &RED)),
        (err, warn) => write!(
            f,
            "with {} and {}",
            colored_count(warn, "warning", "warnings", &RED),
            colored_count(err, "error", "errors", &RED),
        ),
    }
}

fn write_result_line(
    f: &mut fmt::Formatter<'_>,
    outcomes: &InvocationOutcomeTotals,
    colorize: bool,
) -> fmt::Result {
    write!(
        f,
        "Summary: {}",
        MaybeColored::new(&WHITE, Metric(outcomes.total, "total"), colorize)
    )?;

    for (value, label, style) in [
        (outcomes.success, "success", &GREEN),
        (outcomes.reused, "reused", &MAGENTA),
        (outcomes.warn, "warn", &YELLOW),
        (outcomes.error, "error", &RED),
        (outcomes.skipped, "skipped", &DIM),
        (outcomes.canceled, "canceled", &DIM),
        (outcomes.no_op, "no-op", &DIM),
    ] {
        if value > 0 {
            let style: &Style = style;
            write!(
                f,
                " | {}",
                MaybeColored::new(style, Metric(value, label), colorize)
            )?;
        }
    }
    Ok(())
}

/// `{value} {label}`
struct Metric(u64, &'static str);

impl fmt::Display for Metric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.0, self.1)
    }
}

/// `{value} {label}`, with the label singular for a value of one.
struct CountText {
    value: u64,
    label: &'static str,
}

impl CountText {
    fn new(value: u64, label_single: &'static str, label_plural: &'static str) -> Self {
        let label = if value == 1 {
            label_single
        } else {
            label_plural
        };
        Self { value, label }
    }
}

impl fmt::Display for CountText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.value, self.label)
    }
}

/// Suggests running autofix for dbt v2 compatibility issues.
pub struct AutofixLine {
    colorize: bool,
}

impl fmt::Display for AutofixLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let colorize = self.colorize;
        let suggestion_label = MaybeColored::new(&BLUE, "suggestion:", colorize);
        let command = MaybeColored::new(&YELLOW, "dbt deps", colorize);
        let autofix_url =
            MaybeColored::new(&BLUE, "https://github.com/dbt-labs/dbt-autofix", colorize);
        let upgrade_guide_url = MaybeColored::new(
            &BLUE,
            "https://docs.getdbt.com/docs/dbt-versions/dbt-upgrade/upgrading-to-v2",
            colorize,
        );

        write!(
            f,
            "{suggestion_label} Run '{command}' to see the latest dbt v2 compatible packages. For compatibility errors, try the autofix script: {autofix_url}. Upgrading to dbt v2 requires updates to your project -- work with your team and follow our upgrade guide: {upgrade_guide_url}"
        )
    }
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
        let input = InvocationSummaryInput {
            command: "test",
            target: None,
            elapsed: Duration::ZERO,
            metrics: InvocationMetricsSnapshot {
                warnings: 0,
                errors: 0,
                autofix: 0,
                outcomes: InvocationOutcomeTotals {
                    total: 0,
                    success: 0,
                    warn: 0,
                    error: 0,
                    reused: 0,
                    skipped: 0,
                    canceled: 0,
                    no_op: 0,
                },
                test_optimization: TestOptimizationTotals {
                    statically_checked,
                    aggregated_tests,
                    aggregated_queries,
                },
            },
        };
        let summary = InvocationSummary {
            input: Some(input),
            hook_count: 0,
            node_counts: Vec::new(),
            colorize: false,
            max_line_width: WIDTH,
        };
        let lines = summary
            .summary_lines()
            .expect("test shows a summary")
            .to_string()
            .split('\n')
            .map(str::to_string)
            .collect::<Vec<_>>();

        // The section sits between the leading blank line and its own trailing one.
        let start = lines
            .iter()
            .position(|line| line.contains(" Test Optimization "))?;
        let end = start + lines[start..].iter().position(String::is_empty)?;
        Some(lines[start..end].to_vec())
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
    fn summary_delimiters_match_execution_summary_shape() {
        let execution_width = Delimiter::new(" Execution Summary ", WIDTH, false)
            .to_string()
            .len();
        let test_optimization = section(1, 0, 0).expect("section should be present");

        assert_delimiter_matches_width(
            &test_optimization[0],
            "==================== Test Optimization =====================",
            execution_width,
        );
        assert_delimiter_matches_width(
            &Delimiter::new(" End of Execution ", WIDTH, false).to_string(),
            "===================== End of Execution =====================",
            execution_width,
        );
    }

    fn assert_delimiter_matches_width(actual: &str, expected: &str, width: usize) {
        assert_eq!(actual, expected);
        assert_eq!(actual.len(), width);
    }
}
