use std::fmt::{self, Write as _};

use console::Style;
use dbt_telemetry::{
    AnyNodeOutcomeDetail, CompiledCode, CompiledCodeInline, ExecutionPhase, MicrobatchBatchSize,
    MicrobatchEvaluated, NodeCacheDetail, NodeCacheReason, NodeEvaluated, NodeEvent,
    NodeMaterialization, NodeOutcome, NodeProcessed, NodeSkipReason, NodeType,
    SourceFreshnessOutcome, TestOutcome, get_cache_detail, get_freshness_detail,
    get_node_outcome_detail, get_test_outcome, has_node_warning, is_batched_test,
    is_statically_checked_test,
};

use crate::io_args::FsCommand;

use super::{
    color::{BLUE, CYAN, GREEN, MaybeColored, PLAIN, RED, YELLOW},
    constants::{
        ACTION_WIDTH, DURATION_WIDTH, MAX_QUALIFIER_DISPLAY_LEN, MEMBER_TREE_INDENT,
        MIN_NODE_TYPE_WIDTH, UNIT_TEST_SCHEMA_SUFFIX,
    },
    duration::FixedWidthDuration,
    layout::RightAligned,
    phase::{get_phase_action, get_phase_progress_text},
};

/// Title used for compiled inline node output (matching dbt-core)
pub const COMPILED_INLINE_NODE_TITLE: &str = "Compiled inline node is:";

/// Get the display alias for a node based on its type.
///
/// For tests and unit tests, we use `name` which contains the original (untruncated)
/// test name for human-readable display. For other nodes, we use `identifier` (the
/// database-safe alias) with `name` as a fallback.
///
/// This matches dbt-core behavior where truncated test names (with MD5 hashes) are
/// used as database identifiers, but the full readable name is shown in CLI output.
pub fn get_node_display_alias<'a>(
    node_type: NodeType,
    identifier: Option<&'a str>,
    name: &'a str,
) -> &'a str {
    match node_type {
        // Tests and unit tests always use `name` for display (contains original untruncated name).
        // Checks and plugin nodes likewise: they have no relation, so `identifier` is empty and
        // would display as nothing at all.
        NodeType::Test | NodeType::UnitTest | NodeType::Check | NodeType::Plugin => name,
        // Other nodes prefer `identifier` (database alias) with `name` as fallback
        _ => identifier.unwrap_or(name),
    }
}

/// Extract num_failures from test details if available
pub fn get_num_failures(node: NodeEvent) -> Option<i32> {
    get_node_outcome_detail(node).and_then(|detail| {
        if let AnyNodeOutcomeDetail::NodeTestDetail(test_detail) = detail {
            Some(test_detail.failing_rows)
        } else {
            None
        }
    })
}

/// A qualifier as shown before an alias: truncated if longer than
/// MAX_QUALIFIER_DISPLAY_LEN, followed by a dot unless empty.
///
/// The qualifier is the concatenation of its two parts, so a suffix can be appended
/// without building a `String`.
struct DisplayedQualifier<'a>(&'a str, &'a str);

impl fmt::Display for DisplayedQualifier<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let DisplayedQualifier(head, tail) = *self;
        let len = head.len() + tail.len();
        if len > MAX_QUALIFIER_DISPLAY_LEN {
            let keep = MAX_QUALIFIER_DISPLAY_LEN.saturating_sub(4);
            if keep <= head.len() {
                f.write_str(&head[..keep])?;
            } else {
                f.write_str(head)?;
                f.write_str(&tail[..keep - head.len()])?;
            }
            f.write_str("...")
        } else if len == 0 {
            Ok(())
        } else {
            f.write_str(head)?;
            f.write_str(tail)?;
            f.write_str(".")
        }
    }
}

/// Displays a qualifier (schema or node name) and alias with truncation for long names.
/// If the qualifier is longer than MAX characters, truncate to "long_name....::alias"
pub struct QualifierAlias<'a, A = &'a str> {
    pub qualifier: &'a str,
    pub alias: A,
    pub colorize: bool,
}

impl<A: fmt::Display> fmt::Display for QualifierAlias<'_, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_qualifier_alias(
            f,
            DisplayedQualifier(self.qualifier, ""),
            &self.alias,
            self.colorize,
        )
    }
}

fn write_qualifier_alias(
    f: &mut fmt::Formatter<'_>,
    qualifier: DisplayedQualifier<'_>,
    alias: &dyn fmt::Display,
    colorize: bool,
) -> fmt::Result {
    write!(
        f,
        "{}{}",
        MaybeColored::new(&CYAN, qualifier, colorize),
        MaybeColored::new(&BLUE, alias, colorize)
    )
}

/// Displays node type with minimum width for alignment.
/// Minimum width is 5 characters (length of "model") but allows longer strings
pub struct NodeTypeFixedWidth<'a> {
    pub node_type: &'a str,
    pub colorize: bool,
}

impl fmt::Display for NodeTypeFixedWidth<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            MaybeColored::new(&PLAIN, self.node_type, self.colorize)
        )?;

        // Pad if shorter than minimum width, otherwise write as-is
        for _ in self.node_type.len()..MIN_NODE_TYPE_WIDTH {
            f.write_char(' ')?;
        }
        Ok(())
    }
}

/// Displays materialization and adapter without fixed width (for end of line)
pub struct MaterializationSuffix<'a, D = &'a str> {
    pub materialization: Option<&'a str>,
    pub adapter: Option<&'a str>,
    pub desc: Option<D>,
}

impl<D: fmt::Display> fmt::Display for MaterializationSuffix<'_, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let truncated_mat = match self.materialization {
            Some("materialized_view") => Some("mat_view"),
            Some("streaming_table") => Some("streaming"),
            // Hide materialization label for tests and unit tests
            Some("test") | Some("unit_test") | Some("unit") | None => None,
            Some(other) => Some(other),
        };
        let has_label = truncated_mat.is_some() || self.adapter.is_some();
        if !has_label && self.desc.is_none() {
            return Ok(());
        }

        f.write_str(" (")?;
        match (truncated_mat, self.adapter) {
            (Some(mat), Some(adapter)) => write!(f, "{mat}, {adapter}")?,
            (Some(mat), None) => f.write_str(mat)?,
            (None, Some(adapter)) => f.write_str(adapter)?,
            (None, None) => {}
        }
        if let Some(desc) = &self.desc {
            if has_label {
                f.write_str(" - ")?;
            }
            write!(f, "{desc}")?;
        }
        f.write_str(")")
    }
}

/// Displays where a test is defined as `path`, `path:line` or `path:line:col`.
struct TestSourceLocation<'a>(&'a NodeProcessed);

impl fmt::Display for TestSourceLocation<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self.0;
        f.write_str(&node.relative_path)?;
        if let Some(line) = node.defined_at_line {
            write!(f, ":{line}")?;
            if let Some(col) = node.defined_at_col {
                write!(f, ":{col}")?;
            }
        }
        Ok(())
    }
}

/// Description shown after a node's materialization.
enum NodeDescription<'a> {
    /// State-aware orchestration rebuilt the node
    NewChanges,
    /// The node was reused
    Cache(&'a NodeCacheDetail),
    StaticallyChecked,
    /// Where a test that did not pass is defined
    Location {
        node: &'a NodeProcessed,
        batched: bool,
    },
    Batched,
}

impl<'a> NodeDescription<'a> {
    fn new(node: &'a NodeProcessed) -> Option<Self> {
        let node_type = node.node_type();
        let node_outcome = node.node_outcome();

        // SAO enabled and not cached (reused) => "New changes detected"
        if (node_type != NodeType::Test && node_type != NodeType::UnitTest)
            && node_outcome == NodeOutcome::Success
        {
            return node.sao_enabled.and_then(|s| s.then_some(Self::NewChanges));
        }

        if let Some(cache_detail) = get_cache_detail(node.into()) {
            return Some(Self::Cache(cache_detail));
        }

        if is_statically_checked_test(node.into()) {
            return Some(Self::StaticallyChecked);
        }

        let batched = is_batched_test(node.into());

        if matches!(node_type, NodeType::Test | NodeType::UnitTest)
            && get_test_outcome(node.into()) != Some(TestOutcome::Passed)
        {
            return Some(Self::Location { node, batched });
        }

        batched.then_some(Self::Batched)
    }
}

impl fmt::Display for NodeDescription<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewChanges => f.write_str("New changes detected"),
            Self::Cache(cache_detail) => match cache_detail.node_cache_reason() {
                NodeCacheReason::NoChanges => f.write_str("No new changes on any upstreams"),
                NodeCacheReason::StillFresh => write!(
                    f,
                    "New changes detected within lag tolerance of {}. Last updated {} ago",
                    humantime::format_duration(std::time::Duration::from_secs(
                        cache_detail.build_after_seconds()
                    )),
                    humantime::format_duration(std::time::Duration::from_secs(
                        cache_detail.last_updated_seconds()
                    )),
                ),
                NodeCacheReason::UpdateCriteriaNotMet => {
                    f.write_str("No new changes on all upstreams")
                }
                NodeCacheReason::ClonedExisting => f.write_str("Cloned from other environment"),
                NodeCacheReason::ClonedExistingStillFresh => {
                    f.write_str("Cloned from other environment within tolerance")
                }
            },
            Self::StaticallyChecked => f.write_str("Statically checked"),
            Self::Location { node, batched } => {
                if *batched {
                    f.write_str("Batched - ")?;
                }
                write!(f, "{}", TestSourceLocation(node))
            }
            Self::Batched => f.write_str("Batched"),
        }
    }
}

/// Resolve a node outcome to its status text and colour.
fn node_outcome_status_parts(
    node_outcome: NodeOutcome,
    skip_reason: Option<NodeSkipReason>,
    test_outcome: Option<TestOutcome>,
    freshness_outcome: Option<SourceFreshnessOutcome>,
    has_warn: bool,
) -> (&'static str, &'static Style) {
    match (node_outcome, skip_reason, test_outcome, freshness_outcome) {
        // Freshness outcomes (mutually exclusive with test_outcome)
        (NodeOutcome::Success, _, _, Some(f_outcome)) => match f_outcome {
            SourceFreshnessOutcome::OutcomePassed => ("pass", &GREEN),
            SourceFreshnessOutcome::OutcomeWarned => ("warn", &YELLOW),
            SourceFreshnessOutcome::OutcomeFailed => ("error", &RED),
        },
        // Test outcomes
        (NodeOutcome::Success, _, Some(t_outcome), None) => match t_outcome {
            TestOutcome::Passed => ("pass", &GREEN),
            TestOutcome::Warned => ("warn", &YELLOW),
            TestOutcome::Failed => ("fail", &RED),
        },
        // Non test/freshness nodes that succeeded with warnings
        (NodeOutcome::Success, _, None, None) if has_warn => ("warn", &YELLOW),
        // Non test/freshness nodes. Success means "success"
        (NodeOutcome::Success, _, None, None) => ("success", &GREEN),
        (NodeOutcome::Error, _, _, _) => ("error", &RED),
        (NodeOutcome::Skipped, s_reason, _, _) => match s_reason {
            Some(NodeSkipReason::Upstream) => ("skipped", &YELLOW),
            Some(NodeSkipReason::Cached) => ("reused", &GREEN),
            Some(NodeSkipReason::NoOp) => ("no-op", &YELLOW),
            // Other skip reasons are just "skipped"
            Some(NodeSkipReason::PhaseSkipped)
            | Some(NodeSkipReason::PhaseDisabled)
            | Some(NodeSkipReason::Unspecified)
            | None => ("skipped", &YELLOW),
        },
        (NodeOutcome::Canceled, _, _, _) => ("cancelled", &YELLOW),
        (NodeOutcome::Unspecified, _, _, _) => ("no-op", &YELLOW),
    }
}

/// Resolve a node outcome to its unpadded action label and status colour.
/// Callers that pad the label themselves need the plain text, hence the split.
fn node_action_parts(
    node_outcome: NodeOutcome,
    skip_reason: Option<NodeSkipReason>,
    test_outcome: Option<TestOutcome>,
    freshness_outcome: Option<SourceFreshnessOutcome>,
    has_warn: bool,
) -> (&'static str, &'static Style) {
    match (node_outcome, skip_reason, test_outcome, freshness_outcome) {
        // Freshness outcomes (mutually exclusive with test_outcome)
        (NodeOutcome::Success, _, _, Some(f_outcome)) => match f_outcome {
            SourceFreshnessOutcome::OutcomePassed => ("Passed", &GREEN),
            SourceFreshnessOutcome::OutcomeWarned => ("Warned", &YELLOW),
            SourceFreshnessOutcome::OutcomeFailed => ("Stale", &RED),
        },
        // Test outcomes
        (NodeOutcome::Success, _, Some(t_outcome), None) => match t_outcome {
            TestOutcome::Passed => ("Passed", &GREEN),
            TestOutcome::Warned => ("Warned", &YELLOW),
            TestOutcome::Failed => ("Failed", &RED),
        },
        // Non test/freshness nodes that succeeded with warnings
        (NodeOutcome::Success, _, None, None) if has_warn => ("Warned", &YELLOW),
        // Non test/freshness nodes. Success means "Succeeded"
        (NodeOutcome::Success, _, None, None) => ("Succeeded", &GREEN),
        (NodeOutcome::Error, _, _, _) => ("Failed", &RED),
        (NodeOutcome::Skipped, s_reason, _, _) => match s_reason {
            Some(NodeSkipReason::Upstream) => ("Skipped", &YELLOW),
            Some(NodeSkipReason::Cached) => ("Reused", &GREEN),
            Some(NodeSkipReason::NoOp) => ("Skipped", &YELLOW),
            // Other skip reasons are just "skipped"
            Some(NodeSkipReason::PhaseSkipped)
            | Some(NodeSkipReason::PhaseDisabled)
            | Some(NodeSkipReason::Unspecified)
            | None => ("Skipped", &YELLOW),
        },
        (NodeOutcome::Canceled, _, _, _) => ("Cancelled", &YELLOW),
        (NodeOutcome::Unspecified, _, _, _) => ("Finished", &PLAIN),
    }
}

/// Displays the node outcome as a status word, optionally colorized.
/// This closely follows dbt-core's formatting for consistency.
/// Note: test_outcome and freshness_outcome are mutually exclusive (oneof in proto).
#[derive(Clone, Copy)]
pub struct NodeOutcomeStatus {
    pub node_outcome: NodeOutcome,
    pub skip_reason: Option<NodeSkipReason>,
    pub test_outcome: Option<TestOutcome>,
    pub freshness_outcome: Option<SourceFreshnessOutcome>,
    pub has_warn: bool,
    pub colorize: bool,
}

impl fmt::Display for NodeOutcomeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (status, color) = node_outcome_status_parts(
            self.node_outcome,
            self.skip_reason,
            self.test_outcome,
            self.freshness_outcome,
            self.has_warn,
        );
        write!(f, "{}", MaybeColored::new(color, status, self.colorize))
    }
}

/// Displays the action of a node result, right aligned so it shares a column with every
/// other progress message, optionally colorized.
/// Note: test_outcome and freshness_outcome are mutually exclusive (oneof in proto).
#[derive(Clone, Copy)]
pub struct NodeAction {
    pub node_outcome: NodeOutcome,
    pub skip_reason: Option<NodeSkipReason>,
    pub test_outcome: Option<TestOutcome>,
    pub freshness_outcome: Option<SourceFreshnessOutcome>,
    pub has_warn: bool,
    pub colorize: bool,
}

impl fmt::Display for NodeAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (action, color) = node_action_parts(
            self.node_outcome,
            self.skip_reason,
            self.test_outcome,
            self.freshness_outcome,
            self.has_warn,
        );
        debug_assert!(
            action.len() <= ACTION_WIDTH,
            "Action text too long for padding"
        );
        write!(
            f,
            "{}",
            MaybeColored::new(color, RightAligned(action), self.colorize)
        )
    }
}

/// The trailing progress field: dbt core's `N of M` execution counter and the
/// elapsed time, e.g. `5 of 56 in 14.15s` (brackets are added by [`WithProgressField`]).
///
/// This sits at the end of the line, so nothing downstream depends on its width and the
/// parts are rendered at their natural size rather than padded.
///
/// `elapsed` is `None` only for work that never ran, which the caller decides from the
/// outcome. It deliberately is not inferred from a zero duration here: `duration_ms` has
/// millisecond resolution, so a node that really did run can measure zero, and reading
/// that as "never ran" would make the line change shape between runs.
#[derive(Clone, Copy)]
struct ProgressField {
    // Grouped lines stand in for several nodes at once, and nodes outside the selection
    // carry no counter, so both render the duration alone.
    counter: Option<(u32, u32)>,
    elapsed: Option<std::time::Duration>,
}

impl ProgressField {
    /// Returns `None` when there is nothing worth showing, so the caller can drop the
    /// brackets entirely instead of emitting an empty pair.
    fn new(
        index: Option<u32>,
        total: Option<u32>,
        elapsed: Option<std::time::Duration>,
    ) -> Option<Self> {
        let counter = index.zip(total);
        (counter.is_some() || elapsed.is_some()).then_some(Self { counter, elapsed })
    }
}

impl fmt::Display for ProgressField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((index, total)) = self.counter {
            write!(f, "{index} of {total}")?;
            if self.elapsed.is_some() {
                f.write_str(" in ")?;
            }
        }
        // Trimmed: the fixed width existed to align a mid-line column, which this no longer is.
        // A measured zero is still a measurement, so it renders as a duration rather than the
        // `-------` placeholder that stands for "nothing to time".
        match self.elapsed {
            Some(duration) if duration.is_zero() => f.write_str("0.00s"),
            Some(duration) => write!(
                SkipLeadingSpaces { f, skipping: true },
                "{}",
                FixedWidthDuration(duration)
            ),
            None => Ok(()),
        }
    }
}

/// Forwards to `f`, dropping the spaces written before anything else.
struct SkipLeadingSpaces<'a, 'b> {
    f: &'a mut fmt::Formatter<'b>,
    skipping: bool,
}

impl fmt::Write for SkipLeadingSpaces<'_, '_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let s = if self.skipping {
            let trimmed = s.trim_start_matches(' ');
            self.skipping = trimmed.is_empty();
            trimmed
        } else {
            s
        };
        self.f.write_str(s)
    }
}

/// A rendered line followed by its bracketed progress field, or the line alone when
/// there is no progress to report.
struct WithProgressField<L> {
    line: L,
    progress: Option<ProgressField>,
}

impl<L: fmt::Display> fmt::Display for WithProgressField<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.progress {
            Some(progress) => write!(f, "{} [{}]", self.line, progress),
            None => write!(f, "{}", self.line),
        }
    }
}

/// Qualifier shown before a processed node's alias: schema for all nodes except sources,
/// which use their source name, and plugin nodes, which write no relation, so their name
/// stands alone.
fn node_processed_qualifier(node: &NodeProcessed, node_type: NodeType) -> &str {
    if node_type == NodeType::Plugin {
        return "";
    }
    if node_type == NodeType::Source
        && let Some(source_name) = node.source_name.as_deref()
    {
        // For sources we show source_name.identifier to match dbt-core output.
        return source_name;
    }
    node.schema.as_deref().unwrap_or_default()
}

/// Writes a processed node's qualifier and alias. Unit tests show the test schema suffix
/// after the qualifier.
fn write_processed_qualifier_alias(
    f: &mut fmt::Formatter<'_>,
    qualifier: &str,
    node_type: NodeType,
    alias: &str,
    colorize: bool,
) -> fmt::Result {
    let suffix = if node_type == NodeType::UnitTest {
        UNIT_TEST_SCHEMA_SUFFIX
    } else {
        ""
    };
    write_qualifier_alias(f, DisplayedQualifier(qualifier, suffix), &alias, colorize)
}

/// Displays a NodeProcessed event for the start of processing (no duration)
///
/// Writes the pattern:
/// `Started {node_type} {schema}.{alias}`
pub struct NodeProcessedStart<'a> {
    pub node: &'a NodeProcessed,
    pub colorize: bool,
}

impl fmt::Display for NodeProcessedStart<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self.node;
        let node_type = node.node_type();
        let alias = get_node_display_alias(node_type, node.identifier.as_deref(), &node.name);

        write!(f, "Started {} ", node_type_label(node, true))?;
        write_processed_qualifier_alias(
            f,
            node_processed_qualifier(node, node_type),
            node_type,
            alias,
            self.colorize,
        )
    }
}

/// The node type a node's line shows: a plugin node's own kind, else its node type.
fn node_type_label(node: &NodeProcessed, pretty: bool) -> &str {
    match node.plugin_type.as_deref() {
        Some(kind) if node.node_type() == NodeType::Plugin => kind,
        _ if pretty => node.node_type().pretty(),
        _ => node.node_type().as_static_ref(),
    }
}

/// Displays a complete NodeProcessed event as a single output line
///
/// Writes the pattern:
/// `{action} {node_type} {schema}.{alias}{materialization_suffix} [{progress}]`
pub struct NodeProcessedEnd<'a> {
    pub node: &'a NodeProcessed,
    pub duration: std::time::Duration,
    pub colorize: bool,
}

impl fmt::Display for NodeProcessedEnd<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NodeProcessedEnd {
            node,
            duration,
            colorize,
        } = *self;
        let node_outcome = node.node_outcome();
        let node_type = node.node_type();

        // Special case for freshness phase - dispatch by phase, not node type
        if node.last_phase() == ExecutionPhase::FreshnessAnalysis {
            return write!(
                f,
                "{}",
                FreshnessResult {
                    node,
                    duration,
                    colorize,
                }
            );
        }

        // Skips, cache reuse, and cached tests that preserved a verdict never executed, so
        // there is no elapsed time to report. Every other node reports one, including the ones
        // whose work rounds down to zero milliseconds.
        let elapsed = if node_outcome == NodeOutcome::Skipped
            || (node_outcome == NodeOutcome::Success
                && node.node_skip_reason() == NodeSkipReason::Cached)
            || (node_outcome == NodeOutcome::Success && is_statically_checked_test(node.into()))
        {
            None
        } else {
            Some(duration)
        };

        let mut qualifier = node_processed_qualifier(node, node_type);
        let alias = get_node_display_alias(node_type, node.identifier.as_deref(), &node.name);

        // For data tests, only show the schema qualifier when store_failures is enabled.
        // Without store_failures, dbt-core shows just the test name (no schema prefix).
        if node_type == NodeType::Test {
            let store_failures = get_node_outcome_detail(node.into())
                .and_then(|detail| {
                    if let AnyNodeOutcomeDetail::NodeTestDetail(test_detail) = detail {
                        test_detail.store_failures
                    } else {
                        None
                    }
                })
                .unwrap_or(false);
            if !store_failures {
                qualifier = "";
            }
        }

        // Get materialization string - use custom_materialization if materialization is Custom.
        // Checks and plugin nodes materialize nothing, so they get no suffix — reporting one would
        // claim a relation that is never written.
        let materialization = if matches!(node_type, NodeType::Check | NodeType::Plugin) {
            None
        } else if node.materialization.is_some() {
            let mat = node.materialization();
            Some(if mat == NodeMaterialization::Custom {
                node.custom_materialization.as_deref().unwrap_or_default()
            } else {
                mat.as_static_ref()
            })
        } else {
            None
        };

        let action = NodeAction {
            node_outcome,
            skip_reason: node.node_skip_reason.map(|_| node.node_skip_reason()),
            test_outcome: get_test_outcome(node.into()),
            freshness_outcome: None, // not applicable for non-freshness nodes
            has_warn: has_node_warning(node.into()),
            colorize,
        };
        let node_type_formatted = NodeTypeFixedWidth {
            node_type: node_type_label(node, false),
            colorize,
        };

        write!(f, "{action} {node_type_formatted} ")?;
        write_processed_qualifier_alias(f, qualifier, node_type, alias, colorize)?;
        write!(
            f,
            "{}",
            MaterializationSuffix {
                materialization,
                adapter: node.adapter.as_deref(),
                // Determine description based on outcome
                desc: NodeDescription::new(node),
            }
        )?;
        if let Some(progress) = ProgressField::new(node.node_index, node.node_count_total, elapsed)
        {
            write!(f, " [{progress}]")?;
        }
        Ok(())
    }
}

/// Displays a NodeEvaluated event for the start of evaluation (no duration)
///
/// Writes the pattern:
/// `Started {phase_action} {node_type} {schema}.{alias}`
pub struct NodeEvaluatedStart<'a> {
    pub node: &'a NodeEvaluated,
    pub colorize: bool,
}

impl fmt::Display for NodeEvaluatedStart<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let node = self.node;
        let node_type = node.node_type();

        write!(
            f,
            "Started {} {} {}",
            get_phase_action(node.phase()),
            node_type.pretty(),
            QualifierAlias {
                qualifier: node.schema.as_deref().unwrap_or_default(),
                alias: get_node_display_alias(node_type, node.identifier.as_deref(), &node.name),
                colorize: self.colorize,
            }
        )
    }
}

/// Displays a NodeEvaluated event for the start of evaluation (no duration)
/// using legacy non-interactive format
///
/// Writes the pattern:
/// `{padded_action} {path_to_node}`
pub struct NodeEvaluatedStartLegacy<'a> {
    pub node: &'a NodeEvaluated,
    pub command: FsCommand,
}

impl fmt::Display for NodeEvaluatedStartLegacy<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NodeEvaluatedStartLegacy { node, command } = *self;
        let phase = node.phase();

        if phase == ExecutionPhase::Run && command == FsCommand::Show {
            // Show command generated very specific messages in run phase text output.
            return write!(
                f,
                "Previewing {} ({})",
                node.node_type().as_static_ref(),
                node.unique_id
            );
        }

        let phase_action = if phase == ExecutionPhase::Compare {
            RightAligned("Comparing")
        } else if phase == ExecutionPhase::Run && command == FsCommand::Clone {
            RightAligned("Cloning")
        } else {
            let Some(phase_action) = get_phase_progress_text(phase) else {
                unreachable!("Phase action text should be available for NodeEvaluated start");
            };
            phase_action
        };
        write!(f, "{} {}", phase_action, node.relative_path)?;

        // YAML-defined nodes should keep the entry name for clarity.
        // Singular SQL tests and legacy SQL snapshots should keep the old plain path output.
        let is_yaml_defined_node =
            node.relative_path.ends_with(".yml") || node.relative_path.ends_with(".yaml");

        if matches!(
            node.node_type(),
            NodeType::Test | NodeType::UnitTest | NodeType::Snapshot
        ) && is_yaml_defined_node
        {
            if let Some(line) = node.defined_at_line {
                write!(f, ":{line}")?;
                if let Some(col) = node.defined_at_col {
                    write!(f, ":{col}")?;
                }
            }
            write!(f, " ({})", node.name)?;
        }
        Ok(())
    }
}

/// Displays a NodeEvaluated event for the end of evaluation (with duration and outcome)
///
/// Writes the pattern:
/// `Finished {phase_action} [{duration}] {node_type} {schema}.{alias} [{outcome}]`
pub struct NodeEvaluatedEnd<'a> {
    pub node: &'a NodeEvaluated,
    pub duration: std::time::Duration,
    pub colorize: bool,
}

impl fmt::Display for NodeEvaluatedEnd<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let NodeEvaluatedEnd {
            node,
            duration,
            colorize,
        } = *self;
        let node_type = node.node_type();
        let node_outcome = node.node_outcome();

        let active_duration = duration.saturating_sub(std::time::Duration::from_millis(
            node.idle_time_ms.unwrap_or_default(),
        ));
        let active_duration = if node_outcome == NodeOutcome::Success
            && (node.node_skip_reason() == NodeSkipReason::Cached
                || is_statically_checked_test(node.into()))
        {
            std::time::Duration::ZERO
        } else {
            active_duration
        };

        write!(
            f,
            "Finished {} [{}] {} {} [{}]",
            get_phase_action(node.phase()),
            FixedWidthDuration(active_duration),
            node_type.pretty(),
            QualifierAlias {
                qualifier: node.schema.as_deref().unwrap_or_default(),
                alias: get_node_display_alias(node_type, node.identifier.as_deref(), &node.name),
                colorize,
            },
            NodeOutcomeStatus {
                node_outcome,
                skip_reason: node.node_skip_reason.map(|_| node.node_skip_reason()),
                test_outcome: get_test_outcome(node.into()),
                freshness_outcome: None, // NodeEvaluated doesn't have freshness details
                has_warn: has_node_warning(node.into()),
                colorize,
            }
        )
    }
}

/// Displays the start of a microbatch window as of its batch size, e.g. `2026-01-02`
/// for a daily batch.
struct MicrobatchWindow<'a>(&'a MicrobatchEvaluated);

impl fmt::Display for MicrobatchWindow<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let batch = self.0;
        let format = match batch.batch_size() {
            MicrobatchBatchSize::Hour => "%Y-%m-%dT%H",
            MicrobatchBatchSize::Day => "%Y-%m-%d",
            MicrobatchBatchSize::Month => "%Y-%m",
            MicrobatchBatchSize::Year => "%Y",
            MicrobatchBatchSize::Unspecified => return f.write_str(&batch.batch_id),
        };
        match chrono::DateTime::parse_from_rfc3339(&batch.event_time_start) {
            // The window is shown in its own offset; formatting the naive local time
            // skips the offset string `DateTime::format` allocates.
            Ok(start) => start.naive_local().format(format).write_to(f),
            Err(_) => f.write_str(&batch.batch_id),
        }
    }
}

/// Describes a microbatch like dbt-core does.
///
/// Writes the pattern:
/// `batch {window} of {schema}.{alias}`
pub struct MicrobatchDescription<'a> {
    pub batch: &'a MicrobatchEvaluated,
    pub colorize: bool,
}

impl fmt::Display for MicrobatchDescription<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "batch {} of {}",
            MicrobatchWindow(self.batch),
            QualifierAlias {
                qualifier: &self.batch.schema,
                alias: self.batch.identifier.as_str(),
                colorize: self.colorize,
            }
        )
    }
}

/// Time a microbatch spent executing: its span's wall time minus its idle time.
pub fn microbatch_active_duration(
    batch: &MicrobatchEvaluated,
    duration: std::time::Duration,
) -> std::time::Duration {
    duration.saturating_sub(std::time::Duration::from_millis(batch.idle_time_ms))
}

/// Displays a MicrobatchEvaluated event for the start of evaluation (no duration)
///
/// Writes the pattern:
/// `Started batch {window} of {schema}.{alias} [{index} of {total}]`
pub struct MicrobatchEvaluatedStart<'a> {
    pub batch: &'a MicrobatchEvaluated,
    pub colorize: bool,
}

impl fmt::Display for MicrobatchEvaluatedStart<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let batch = self.batch;
        write!(
            f,
            "{}",
            WithProgressField {
                line: format_args!(
                    "{} {}",
                    RightAligned("Started"),
                    MicrobatchDescription {
                        batch,
                        colorize: self.colorize,
                    }
                ),
                progress: ProgressField::new(
                    Some(batch.batch_index),
                    Some(batch.total_batches),
                    None
                ),
            }
        )
    }
}

/// Displays a MicrobatchEvaluated event for the end of evaluation (with duration and outcome)
///
/// Writes the pattern:
/// `{action} batch {window} of {schema}.{alias} [{index} of {total} in {duration}]`
pub struct MicrobatchEvaluatedEnd<'a> {
    pub batch: &'a MicrobatchEvaluated,
    pub duration: std::time::Duration,
    pub colorize: bool,
}

impl fmt::Display for MicrobatchEvaluatedEnd<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let MicrobatchEvaluatedEnd {
            batch,
            duration,
            colorize,
        } = *self;
        write!(
            f,
            "{}",
            WithProgressField {
                line: format_args!(
                    "{} {}",
                    NodeAction {
                        node_outcome: batch.node_outcome(),
                        skip_reason: None,
                        test_outcome: None,
                        freshness_outcome: None,
                        has_warn: false,
                        colorize,
                    },
                    MicrobatchDescription { batch, colorize }
                ),
                progress: ProgressField::new(
                    Some(batch.batch_index),
                    Some(batch.total_batches),
                    Some(microbatch_active_duration(batch, duration)),
                ),
            }
        )
    }
}

/// Names of skipped tests, quoted: all of them up to three, otherwise the first two and
/// a count of the others.
struct SkippedTestNames<'a> {
    names: &'a [String],
    colorize: bool,
}

impl fmt::Display for SkippedTestNames<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shown = if self.names.len() > 3 {
            &self.names[..2]
        } else {
            self.names
        };
        for (index, name) in shown.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(f, "'{}'", MaybeColored::new(&YELLOW, name, self.colorize))?;
        }
        if self.names.len() > 3 {
            write!(f, " and {} others", self.names.len() - 2)?;
        }
        Ok(())
    }
}

/// Displays a skipped test group summary line
///
/// Writes the pattern:
/// `{action} {resource_type} {names}`
pub struct SkippedTestGroup<'a> {
    pub node_names: &'a [String],
    pub seen_test: bool,
    pub seen_unit_test: bool,
    pub node_count_total: Option<u32>,
    pub colorize: bool,
}

impl fmt::Display for SkippedTestGroup<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let colorize = self.colorize;
        // Determine resource type based on which types were seen
        let resource_type = match (self.seen_test, self.seen_unit_test) {
            (true, true) => "test,unit_test",
            (true, false) => "test",
            (false, true) => "unit_test",
            (false, false) => "unknown",
        };

        write!(
            f,
            "{}",
            WithProgressField {
                line: format_args!(
                    "{} {} {}",
                    NodeAction {
                        node_outcome: NodeOutcome::Skipped,
                        skip_reason: Some(NodeSkipReason::Upstream),
                        test_outcome: None,
                        freshness_outcome: None,
                        has_warn: false, // skipped nodes never have warn detail
                        colorize,
                    },
                    NodeTypeFixedWidth {
                        node_type: resource_type,
                        colorize,
                    },
                    SkippedTestNames {
                        names: self.node_names,
                        colorize,
                    }
                ),
                // One line standing in for several skipped nodes, so there is no single index
                // to show, and no execution to time either: this resolves to no trailing field
                // at all.
                progress: ProgressField::new(None, self.node_count_total, None),
            }
        )
    }
}

/// `{macro_name} on {model_name}`, the label of an aggregated test group.
struct AggregatedTestGroupLabel<'a> {
    macro_name: &'a str,
    model_name: &'a str,
}

impl fmt::Display for AggregatedTestGroupLabel<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} on {}", self.macro_name, self.model_name)
    }
}

/// `{member_count} batched`, the description of an aggregated test group.
struct BatchedCount(usize);

impl fmt::Display for BatchedCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} batched", self.0)
    }
}

/// Displays the header line of an aggregated generic-test group
///
/// Writes the pattern:
/// `{worst_action} test {macro_name} on {model_name} ({member_count} batched) [{duration}]`
pub struct AggregatedTestGroupHeader<'a> {
    pub macro_name: &'a str,
    /// unique_id of the node the group's tests are attached to
    pub attached_node: &'a str,
    pub member_count: usize,
    pub worst_outcome: TestOutcome,
    pub duration: std::time::Duration,
    pub node_count_total: Option<u32>,
    pub adapter: Option<&'a str>,
    pub colorize: bool,
}

impl fmt::Display for AggregatedTestGroupHeader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let colorize = self.colorize;
        // unique_id is `<type>.<package>.<name>[.v<version>]`; keep the version suffix.
        let model_name = self
            .attached_node
            .splitn(3, '.')
            .nth(2)
            .unwrap_or(self.attached_node);
        debug_assert_eq!(
            FixedWidthDuration(self.duration).to_string().len(),
            DURATION_WIDTH,
            "duration must render at its declared fixed width"
        );

        write!(
            f,
            "{}",
            WithProgressField {
                line: format_args!(
                    "{} {} {}{}",
                    NodeAction {
                        node_outcome: NodeOutcome::Success,
                        skip_reason: None, // an aggregated group always executed
                        test_outcome: Some(self.worst_outcome),
                        freshness_outcome: None,
                        has_warn: false, // the group's verdict comes from its members
                        colorize,
                    },
                    NodeTypeFixedWidth {
                        node_type: NodeType::Test.as_static_ref(),
                        colorize,
                    },
                    QualifierAlias {
                        qualifier: "",
                        alias: AggregatedTestGroupLabel {
                            macro_name: self.macro_name,
                            model_name,
                        },
                        colorize,
                    },
                    MaterializationSuffix {
                        materialization: None,
                        adapter: self.adapter,
                        desc: Some(BatchedCount(self.member_count)),
                    }
                ),
                // The header covers every member of the group, so it has no single index of
                // its own; it still reports the query's elapsed time.
                progress: ProgressField::new(None, self.node_count_total, Some(self.duration)),
            }
        )
    }
}

/// Indentation and connector starting a tree branch under a result line.
struct TreeBranch {
    is_last: bool,
}

impl fmt::Display for TreeBranch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for _ in 0..MEMBER_TREE_INDENT {
            f.write_char(' ')?;
        }
        f.write_str(if self.is_last { "└─" } else { "├─" })
    }
}

/// Displays one member line of an aggregated generic-test group as a tree branch under
/// the group header. `is_last` picks the terminating connector.
///
/// Writes the pattern:
/// `{indent}{connector} {action} {node_type} {name}{source_location_suffix}`
pub struct AggregatedTestGroupMember<'a> {
    pub node: &'a NodeProcessed,
    pub is_last: bool,
    pub colorize: bool,
}

impl fmt::Display for AggregatedTestGroupMember<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let AggregatedTestGroupMember {
            node,
            is_last,
            colorize,
        } = *self;
        // Members carry no duration bracket; the header states the query's cost once.
        let desc = (get_test_outcome(node.into()) != Some(TestOutcome::Passed))
            .then_some(TestSourceLocation(node));

        let (action_label, action_style) = node_action_parts(
            node.node_outcome(),
            node.node_skip_reason.map(|_| node.node_skip_reason()),
            get_test_outcome(node.into()),
            None, // freshness_outcome
            has_node_warning(node.into()),
        );

        // The action follows the connector unpadded, so a label longer than a test verdict
        // shifts the rest of its own line right. Deliberate: padding reopens a wide gap.
        write!(
            f,
            "{} {} {} {}{}",
            TreeBranch { is_last },
            MaybeColored::new(action_style, action_label, colorize),
            NodeTypeFixedWidth {
                node_type: node.node_type().as_static_ref(),
                colorize,
            },
            QualifierAlias {
                qualifier: "",
                alias: node.name.as_str(),
                colorize,
            },
            MaterializationSuffix {
                materialization: None,
                adapter: node.adapter.as_deref(),
                desc,
            }
        )
    }
}

/// Displays one microbatch of a model as a tree branch under the model's result line.
/// `is_last` picks the terminating connector.
///
/// Writes the pattern:
/// `{indent}{connector} {action} batch {window} [{index} of {total} in {duration}]`
pub struct MicrobatchMember<'a> {
    pub batch: &'a MicrobatchEvaluated,
    pub duration: std::time::Duration,
    pub is_last: bool,
    pub colorize: bool,
}

impl fmt::Display for MicrobatchMember<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let batch = self.batch;
        let (action_label, action_style) =
            node_action_parts(batch.node_outcome(), None, None, None, false);
        write!(
            f,
            "{}",
            WithProgressField {
                line: format_args!(
                    "{} {} batch {}",
                    TreeBranch {
                        is_last: self.is_last
                    },
                    MaybeColored::new(action_style, action_label, self.colorize),
                    MicrobatchWindow(batch)
                ),
                progress: ProgressField::new(
                    Some(batch.batch_index),
                    Some(batch.total_batches),
                    Some(microbatch_active_duration(batch, self.duration)),
                ),
            }
        )
    }
}

/// Displays compiled inline code output
///
/// Writes the title and SQL:
/// `{title}\n{sql}`
pub struct CompiledInlineCode<'a> {
    pub compiled_code: &'a CompiledCodeInline,
    pub colorize: bool,
}

impl fmt::Display for CompiledInlineCode<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}\n{}",
            MaybeColored::new(&BLUE, COMPILED_INLINE_NODE_TITLE, self.colorize),
            self.compiled_code.sql
        )
    }
}

/// Displays compiled project node output.
///
/// Writes one-line path-oriented message:
/// `Compiled SQL for node {unique_id} at {relative_path}`
pub struct CompiledCodeLine<'a> {
    pub compiled_code: &'a CompiledCode,
    pub colorize: bool,
}

impl fmt::Display for CompiledCodeLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Compiled SQL for node {} at {}",
            MaybeColored::new(&BLUE, &self.compiled_code.unique_id, self.colorize),
            self.compiled_code.relative_path
        )
    }
}

/// ` (last updated {age} ago)`, where the age is unknown without a measurement.
struct LastUpdated(Option<i64>);

impl fmt::Display for LastUpdated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(age) => write!(
                f,
                " (last updated {} ago)",
                humantime::format_duration(std::time::Duration::from_secs(age as u64))
            ),
            None => f.write_str(" (last updated unknown ago)"),
        }
    }
}

/// Displays a freshness result, for a source or for a model carrying a freshness SLA.
///
/// Writes the pattern:
/// `{action} {node_type} {qualifier}.{identifier} (last updated {age} ago) [{progress}]`
/// where the qualifier is the source name for sources and the schema for models.
pub struct FreshnessResult<'a> {
    pub node: &'a NodeProcessed,
    pub duration: std::time::Duration,
    pub colorize: bool,
}

impl fmt::Display for FreshnessResult<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let FreshnessResult {
            node,
            duration,
            colorize,
        } = *self;
        // Without freshness details the check exited early due to an error
        let freshness_detail = get_freshness_detail(node.into());

        // dbt-core logs `source_name.identifier` for sources. A model has no source name, so
        // qualify it with its schema instead — the same qualifier its build line uses.
        let qualifier = node
            .source_name
            .as_deref()
            .or(node.schema.as_deref())
            .unwrap_or("");
        let identifier = node.identifier.as_deref().unwrap_or(&node.name);

        write!(
            f,
            "{} {} {}",
            NodeAction {
                node_outcome: node.node_outcome(),
                skip_reason: node.node_skip_reason.map(|_| node.node_skip_reason()),
                test_outcome: None,
                freshness_outcome: freshness_detail.map(|detail| detail.node_freshness_outcome()),
                has_warn: false, // freshness nodes use freshness_outcome instead
                colorize,
            },
            NodeTypeFixedWidth {
                node_type: node.node_type().as_static_ref(),
                colorize,
            },
            QualifierAlias {
                qualifier,
                alias: identifier,
                colorize,
            }
        )?;
        if let Some(detail) = freshness_detail {
            write!(f, "{}", LastUpdated(detail.age_seconds))?;
        }
        if let Some(progress) =
            ProgressField::new(node.node_index, node.node_count_total, Some(duration))
        {
            write!(f, " [{progress}]")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_telemetry::{
        NodeOutcomeDetail, SourceFreshnessDetail, TestEvaluationDetail,
        node_processed::NodeOutcomeDetail as ProcessedDetail,
    };

    /// Stand-in for the batch's synthetic test node unique_id that members are stamped with.
    const BATCH_UNIQUE_ID: &str = "test.project.aggregated_accepted_values_orders";

    fn freshness_processed(node_type: NodeType, source_name: Option<&str>) -> NodeProcessed {
        let mut node = NodeProcessed::start(
            "model.project.stg_orders".to_string(),
            "stg_orders".to_string(),
            None,
            Some("analytics".to_string()),
            Some("stg_orders".to_string()),
            None,
            None,
            node_type,
            Some(ExecutionPhase::FreshnessAnalysis),
            "models/staging/stg_orders.sql".to_string(),
            None,
            None,
            "checksum".to_string(),
            true,
            None,
        );
        node.source_name = source_name.map(str::to_string);
        node.set_node_outcome(NodeOutcome::Success);
        node.node_outcome_detail = Some(ProcessedDetail::NodeFreshnessOutcome(
            SourceFreshnessDetail {
                node_freshness_outcome: SourceFreshnessOutcome::OutcomePassed as i32,
                age_seconds: Some(60),
            },
        ));
        node
    }

    /// A plugin node's lines name its own kind and show no schema or materialization.
    #[test]
    fn plugin_node_lines_use_its_own_kind() {
        let mut node = NodeProcessed::start(
            "widget.project.orders_widget".to_string(),
            "orders_widget".to_string(),
            None,
            Some("analytics".to_string()),
            None,
            Some(NodeMaterialization::Analysis),
            None,
            NodeType::Plugin,
            Some(ExecutionPhase::Run),
            "models/widgets.yml".to_string(),
            None,
            None,
            "checksum".to_string(),
            true,
            None,
        );
        node.plugin_type = Some("widget".to_string());
        assert_eq!(
            NodeProcessedStart {
                node: &node,
                colorize: false,
            }
            .to_string(),
            "Started widget orders_widget"
        );

        node.set_node_outcome(NodeOutcome::Success);
        let line = NodeProcessedEnd {
            node: &node,
            duration: std::time::Duration::from_secs(1),
            colorize: false,
        }
        .to_string();
        assert!(line.contains("widget orders_widget"), "got {line}");
        assert!(!line.contains("plugin"), "got {line}");
        assert!(!line.contains("analytics"), "got {line}");
        assert!(!line.contains("(analysis)"), "got {line}");
    }

    /// A model has no source name, so the schema qualifies it — the same qualifier its
    /// build line uses. Without the fallback this rendered as a bare identifier.
    #[test]
    fn model_freshness_result_is_qualified_by_schema() {
        let line = FreshnessResult {
            node: &freshness_processed(NodeType::Model, None),
            duration: std::time::Duration::from_secs(1),
            colorize: false,
        }
        .to_string();
        assert!(
            line.contains("analytics.stg_orders"),
            "model freshness line is not schema-qualified: {line}"
        );
        assert!(line.contains("(last updated 1m ago)"), "got {line}");
    }

    /// Sources keep dbt-core's `source_name.identifier`, not the schema.
    #[test]
    fn source_freshness_result_is_qualified_by_source_name() {
        let line = FreshnessResult {
            node: &freshness_processed(NodeType::Source, Some("raw")),
            duration: std::time::Duration::from_secs(1),
            colorize: false,
        }
        .to_string();
        assert!(
            line.contains("raw.stg_orders"),
            "source freshness line lost its source name: {line}"
        );
        assert!(
            !line.contains("analytics."),
            "source freshness line should not use the schema: {line}"
        );
    }

    fn cached_warned_test_processed() -> NodeProcessed {
        let mut node = NodeProcessed::start(
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
        );
        node.set_node_outcome(NodeOutcome::Success);
        node.set_node_skip_reason(NodeSkipReason::Cached);
        node.node_outcome_detail = Some(ProcessedDetail::NodeTestDetail(
            TestEvaluationDetail::new(TestOutcome::Warned, 2, None, None, None, None),
        ));
        node
    }

    fn test_processed(
        outcome: TestOutcome,
        failures: i32,
        statically_checked: Option<bool>,
        batch_unique_id: Option<&str>,
    ) -> NodeProcessed {
        let mut node = NodeProcessed::start(
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
        );
        node.set_node_outcome(NodeOutcome::Success);
        node.node_outcome_detail =
            Some(ProcessedDetail::NodeTestDetail(TestEvaluationDetail::new(
                outcome,
                failures,
                None,
                None,
                statically_checked,
                batch_unique_id.map(str::to_string),
            )));
        node
    }

    fn cached_warned_test_evaluated() -> NodeEvaluated {
        let mut node = NodeEvaluated::start(
            "test.project.accepted_values_orders_is_today_order__True".to_string(),
            "accepted_values_orders_is_today_order__True".to_string(),
            None,
            Some("dbt_test__audit".to_string()),
            None,
            None,
            None,
            NodeType::Test,
            ExecutionPhase::Run,
            "models/marts/orders.yml".to_string(),
            Some(37),
            Some(13),
            "checksum".to_string(),
        );
        node.set_node_outcome(NodeOutcome::Success);
        node.set_node_skip_reason(NodeSkipReason::Cached);
        node.node_outcome_detail = Some(NodeOutcomeDetail::NodeTestDetail(
            TestEvaluationDetail::new(TestOutcome::Warned, 2, None, None, None, None),
        ));
        node
    }

    #[test]
    fn cached_warned_test_processed_formats_zero_duration_and_warning() {
        let output = NodeProcessedEnd {
            node: &cached_warned_test_processed(),
            duration: std::time::Duration::from_millis(250),
            colorize: false,
        }
        .to_string();

        assert!(output.contains("Warned"));
        // Cache reuse has nothing to time, so no trailing field is emitted.
        assert!(!output.contains('['), "{output}");
        assert!(output.contains("accepted_values_orders_is_today_order__True"));
    }

    #[test]
    fn statically_checked_test_processed_formats_no_query_duration_and_description() {
        let output = NodeProcessedEnd {
            node: &test_processed(TestOutcome::Passed, 0, Some(true), None),
            duration: std::time::Duration::from_millis(250),
            colorize: false,
        }
        .to_string();

        assert!(output.contains("Passed"));
        assert!(!output.contains('['), "{output}");
        assert!(output.contains("Statically checked"));
    }

    #[test]
    fn non_statically_checked_passed_test_processed_keeps_duration_and_description() {
        let output = NodeProcessedEnd {
            node: &test_processed(TestOutcome::Passed, 0, None, None),
            duration: std::time::Duration::from_millis(250),
            colorize: false,
        }
        .to_string();

        assert!(output.contains("Passed"));
        assert!(output.contains("[0.25s]"), "{output}");
        assert!(!output.contains("Statically checked"));
        assert!(!output.contains("Batched"));
    }

    /// `duration_ms` counts whole milliseconds, so a node that really did run can measure
    /// zero. Treating that as "nothing to time" dropped the elapsed part, and a node
    /// sitting on the 0/1ms boundary rendered a different line from one run to the next.
    #[test]
    fn executed_node_measuring_zero_still_reports_elapsed() {
        let mut node = test_processed(TestOutcome::Passed, 0, None, None);
        node.node_index = Some(7);
        node.node_count_total = Some(10);

        let output = NodeProcessedEnd {
            node: &node,
            duration: std::time::Duration::ZERO,
            colorize: false,
        }
        .to_string();

        assert!(output.ends_with("[7 of 10 in 0.00s]"), "{output}");
    }

    /// The counterpart: work that never ran reports the counter alone, however long the
    /// span it hung off happened to be.
    #[test]
    fn cache_reused_node_reports_the_counter_alone() {
        let mut node = cached_warned_test_processed();
        node.node_index = Some(7);
        node.node_count_total = Some(10);

        let output = NodeProcessedEnd {
            node: &node,
            duration: std::time::Duration::from_millis(250),
            colorize: false,
        }
        .to_string();

        assert!(output.ends_with("[7 of 10]"), "{output}");
    }

    // `NodeProcessedEnd` feeds `logs/dbt.log` and `--log-format json`, which stay
    // one flat line per member test. Only stdout regroups; do not align these with it.
    #[test]
    fn aggregated_passed_test_processed_shows_flat_marker_for_log_sinks() {
        let output = NodeProcessedEnd {
            node: &test_processed(TestOutcome::Passed, 0, None, Some(BATCH_UNIQUE_ID)),
            duration: std::time::Duration::from_millis(250),
            colorize: false,
        }
        .to_string();

        assert!(output.contains("Passed"));
        assert!(output.contains("(Batched)"));
        // An aggregated group does issue a query, so the duration must survive.
        assert!(output.contains("[0.25s]"), "{output}");
    }

    #[test]
    fn aggregated_failed_test_processed_shows_flat_marker_and_location_for_log_sinks() {
        let output = NodeProcessedEnd {
            node: &test_processed(TestOutcome::Failed, 3, None, Some(BATCH_UNIQUE_ID)),
            duration: std::time::Duration::from_millis(250),
            colorize: false,
        }
        .to_string();

        assert!(output.contains("Failed"));
        assert!(output.contains("(Batched - models/marts/orders.yml:37:13)"));
    }

    #[test]
    fn non_aggregated_failed_test_processed_keeps_bare_location() {
        let output = NodeProcessedEnd {
            node: &test_processed(TestOutcome::Failed, 3, None, None),
            duration: std::time::Duration::from_millis(250),
            colorize: false,
        }
        .to_string();

        assert!(output.contains("Failed"));
        assert!(output.contains("(models/marts/orders.yml:37:13)"));
        assert!(!output.contains("Batched"));
    }

    fn model_processed(adapter: Option<&str>) -> NodeProcessed {
        let mut node = NodeProcessed::start(
            "model.project.customers".to_string(),
            "customers".to_string(),
            None,
            Some("analytics".to_string()),
            Some("customers".to_string()),
            Some(NodeMaterialization::Table),
            None,
            NodeType::Model,
            Some(ExecutionPhase::Run),
            "models/customers.sql".to_string(),
            None,
            None,
            "checksum".to_string(),
            true,
            None,
        );
        node.adapter = adapter.map(str::to_string);
        node.set_node_outcome(NodeOutcome::Success);
        node
    }

    #[test]
    fn adapter_renders_alongside_materialization() {
        let output = NodeProcessedEnd {
            node: &model_processed(Some("snowflake")),
            duration: std::time::Duration::from_millis(250),
            colorize: false,
        }
        .to_string();

        assert!(
            output.contains("analytics.customers (table, snowflake)"),
            "{output}"
        );
    }

    #[test]
    fn missing_adapter_keeps_materialization_alone() {
        let output = NodeProcessedEnd {
            node: &model_processed(None),
            duration: std::time::Duration::from_millis(250),
            colorize: false,
        }
        .to_string();

        assert!(output.contains("analytics.customers (table) "), "{output}");
    }

    #[test]
    fn adapter_renders_for_tests_without_materialization() {
        let mut node = test_processed(TestOutcome::Passed, 0, None, None);
        node.adapter = Some("snowflake".to_string());

        let output = NodeProcessedEnd {
            node: &node,
            duration: std::time::Duration::from_millis(250),
            colorize: false,
        }
        .to_string();

        assert!(
            output.contains("accepted_values_orders_is_today_order__True (snowflake) "),
            "{output}"
        );
    }

    #[test]
    fn materialization_suffix_places_adapter_before_description() {
        assert_eq!(
            MaterializationSuffix {
                materialization: Some("table"),
                adapter: Some("snowflake"),
                desc: Some("cached")
            }
            .to_string(),
            " (table, snowflake - cached)"
        );
        assert_eq!(
            MaterializationSuffix {
                materialization: None,
                adapter: Some("snowflake"),
                desc: Some("cached")
            }
            .to_string(),
            " (snowflake - cached)"
        );
        assert_eq!(
            MaterializationSuffix {
                materialization: Some("table"),
                adapter: None,
                desc: Some("cached")
            }
            .to_string(),
            " (table - cached)"
        );
        assert_eq!(
            MaterializationSuffix {
                materialization: None,
                adapter: None,
                desc: None::<&str>
            }
            .to_string(),
            ""
        );
    }

    fn indexed_test_processed(index: Option<u32>, total: Option<u32>) -> NodeProcessed {
        let mut node = test_processed(TestOutcome::Passed, 0, None, None);
        node.node_index = index;
        node.node_count_total = total;
        node
    }

    /// Display column of `pat` within `line`, counting characters rather than bytes.
    fn column_of(line: &str, pat: &str) -> usize {
        let byte = line.find(pat).unwrap_or_else(|| panic!("{pat} in {line}"));
        line[..byte].chars().count()
    }

    #[test]
    fn execution_index_renders_as_a_trailing_field() {
        let output = NodeProcessedEnd {
            node: &indexed_test_processed(Some(1), Some(9)),
            duration: std::time::Duration::from_millis(340),
            colorize: false,
        }
        .to_string();

        assert!(output.ends_with("[1 of 9 in 0.34s]"), "{output}");
    }

    #[test]
    fn execution_index_does_not_disturb_the_left_columns() {
        // The field trails the line, so the index's width cannot push the columns before it
        // around, and the counter needs no padding of its own.
        let narrow = NodeProcessedEnd {
            node: &indexed_test_processed(Some(1), Some(53)),
            duration: std::time::Duration::from_millis(340),
            colorize: false,
        }
        .to_string();
        let wide = NodeProcessedEnd {
            node: &indexed_test_processed(Some(53), Some(53)),
            duration: std::time::Duration::from_millis(340),
            colorize: false,
        }
        .to_string();

        assert!(narrow.ends_with("[1 of 53 in 0.34s]"), "{narrow}");
        assert!(wide.ends_with("[53 of 53 in 0.34s]"), "{wide}");
        assert_eq!(column_of(&narrow, "test "), column_of(&wide, "test "));
    }

    #[test]
    fn action_is_right_aligned_to_a_fixed_column() {
        // The action shares its column with every other progress message ("Loading",
        // "Parsing", ...), so it is right aligned and the node type column stays put
        // whatever the length of the action label.
        let passed = NodeProcessedEnd {
            node: &indexed_test_processed(Some(1), Some(9)),
            duration: std::time::Duration::from_millis(340),
            colorize: false,
        }
        .to_string();
        let mut skipped_node = indexed_test_processed(Some(2), Some(9));
        skipped_node.set_node_outcome(NodeOutcome::Skipped);
        skipped_node.set_node_skip_reason(NodeSkipReason::Upstream);
        let skipped = NodeProcessedEnd {
            node: &skipped_node,
            duration: std::time::Duration::from_millis(340),
            colorize: false,
        }
        .to_string();

        assert!(passed.starts_with("    Passed "), "{passed}");
        assert!(skipped.starts_with("   Skipped "), "{skipped}");
        assert_eq!(column_of(&passed, "test "), column_of(&skipped, "test "));
    }

    #[test]
    fn execution_index_drops_the_elapsed_time_when_there_was_none() {
        // Skips and cache reuse have nothing to time, so the counter stands alone rather
        // than being paired with a placeholder duration. The span they hang off still has
        // a length, so the decision comes from the outcome, never from the measurement.
        let mut node = indexed_test_processed(Some(4), Some(53));
        node.set_node_outcome(NodeOutcome::Skipped);
        node.set_node_skip_reason(NodeSkipReason::Upstream);

        let output = NodeProcessedEnd {
            node: &node,
            duration: std::time::Duration::from_millis(340),
            colorize: false,
        }
        .to_string();

        assert!(output.ends_with("[4 of 53]"), "{output}");
        assert!(!output.contains(" in "), "{output}");
    }

    #[test]
    fn missing_execution_index_falls_back_to_the_elapsed_time_alone() {
        let output = NodeProcessedEnd {
            node: &indexed_test_processed(None, None),
            duration: std::time::Duration::from_millis(340),
            colorize: false,
        }
        .to_string();

        assert!(output.ends_with("[0.34s]"), "{output}");
        assert!(!output.contains(" of "), "{output}");
    }

    #[test]
    fn no_index_and_no_elapsed_time_drops_the_field_entirely() {
        let mut node = indexed_test_processed(None, None);
        node.set_node_outcome(NodeOutcome::Skipped);
        node.set_node_skip_reason(NodeSkipReason::Upstream);

        let output = NodeProcessedEnd {
            node: &node,
            duration: std::time::Duration::from_millis(340),
            colorize: false,
        }
        .to_string();

        assert!(!output.contains('['), "{output}");
    }

    #[test]
    fn grouped_lines_report_elapsed_time_without_an_index() {
        // A grouped line covers several nodes, so it has no single index, but it still
        // shares the left columns with the indexed lines around it.
        let grouped = AggregatedTestGroupHeader {
            macro_name: "unique",
            attached_node: "model.my_pkg.unproven_unique",
            member_count: 2,
            worst_outcome: TestOutcome::Passed,
            duration: std::time::Duration::from_millis(340),
            node_count_total: Some(53),
            adapter: None,
            colorize: false,
        }
        .to_string();
        let indexed = NodeProcessedEnd {
            node: &indexed_test_processed(Some(7), Some(53)),
            duration: std::time::Duration::from_millis(340),
            colorize: false,
        }
        .to_string();

        assert!(grouped.ends_with("[0.34s]"), "{grouped}");
        assert!(!grouped.contains(" of "), "{grouped}");
        assert_eq!(column_of(&grouped, "test "), column_of(&indexed, "test "));
    }

    #[test]
    fn aggregated_group_header_formats_worst_outcome_and_label() {
        let output = AggregatedTestGroupHeader {
            macro_name: "unique",
            attached_node: "model.my_pkg.unproven_unique",
            member_count: 2,
            worst_outcome: TestOutcome::Failed,
            duration: std::time::Duration::from_millis(250),
            node_count_total: None,
            adapter: None,
            colorize: false,
        }
        .to_string();

        assert!(output.contains("Failed"));
        assert!(output.contains("unique on unproven_unique"));
        assert!(output.contains("(2 batched)"));
    }

    #[test]
    fn aggregated_group_header_uses_unqualified_model_name() {
        let output = AggregatedTestGroupHeader {
            macro_name: "unique",
            attached_node: "model.my_pkg.unproven_unique",
            member_count: 2,
            worst_outcome: TestOutcome::Passed,
            duration: std::time::Duration::from_millis(250),
            node_count_total: None,
            adapter: None,
            colorize: false,
        }
        .to_string();

        assert!(output.contains("unique on unproven_unique"));
        assert!(!output.contains("model.my_pkg."));
    }

    #[test]
    fn aggregated_group_header_keeps_version_suffix() {
        let output = AggregatedTestGroupHeader {
            macro_name: "unique",
            attached_node: "model.simpler.model_with_version.v1",
            member_count: 2,
            worst_outcome: TestOutcome::Passed,
            duration: std::time::Duration::from_millis(250),
            node_count_total: None,
            adapter: None,
            colorize: false,
        }
        .to_string();

        assert!(output.contains("unique on model_with_version.v1"));
        assert!(!output.contains("unique on v1"));
    }

    #[test]
    fn aggregated_group_header_warned_outcome() {
        let output = AggregatedTestGroupHeader {
            macro_name: "unique",
            attached_node: "model.my_pkg.unproven_unique",
            member_count: 2,
            worst_outcome: TestOutcome::Warned,
            duration: std::time::Duration::from_millis(250),
            node_count_total: None,
            adapter: None,
            colorize: false,
        }
        .to_string();

        assert!(output.contains("Warned"));
    }

    #[test]
    fn aggregated_group_header_shows_adapter() {
        let output = AggregatedTestGroupHeader {
            macro_name: "unique",
            attached_node: "model.my_pkg.unproven_unique",
            member_count: 2,
            worst_outcome: TestOutcome::Passed,
            duration: std::time::Duration::from_millis(250),
            node_count_total: None,
            adapter: Some("snowflake"),
            colorize: false,
        }
        .to_string();

        assert!(output.contains("(snowflake - 2 batched)"), "{output}");
    }

    #[test]
    fn aggregated_group_member_shows_adapter() {
        let mut node = test_processed(TestOutcome::Passed, 0, None, Some(BATCH_UNIQUE_ID));
        node.adapter = Some("snowflake".to_string());

        let output = AggregatedTestGroupMember {
            node: &node,
            is_last: false,
            colorize: false,
        }
        .to_string();

        assert!(output.ends_with("(snowflake)"), "{output}");
    }

    #[test]
    fn aggregated_group_member_passed_has_no_bracket_and_no_marker() {
        let output = AggregatedTestGroupMember {
            node: &test_processed(TestOutcome::Passed, 0, None, Some(BATCH_UNIQUE_ID)),
            is_last: false,
            colorize: false,
        }
        .to_string();

        assert_eq!(
            output,
            "    ├─ Passed test  accepted_values_orders_is_today_order__True"
        );
    }

    #[test]
    fn aggregated_group_member_failed_keeps_source_location() {
        let output = AggregatedTestGroupMember {
            node: &test_processed(TestOutcome::Failed, 3, None, Some(BATCH_UNIQUE_ID)),
            is_last: true,
            colorize: false,
        }
        .to_string();

        assert_eq!(
            output,
            "    └─ Failed test  accepted_values_orders_is_today_order__True \
             (models/marts/orders.yml:37:13)"
        );
    }

    #[test]
    fn aggregated_group_member_branches_from_a_fixed_column() {
        let passed = test_processed(TestOutcome::Passed, 0, None, Some(BATCH_UNIQUE_ID));
        // A cancelled member renders the longest reachable action label ("Cancelled"),
        // the case that legitimately shifts its own node type column.
        let mut cancelled = test_processed(TestOutcome::Passed, 0, None, Some(BATCH_UNIQUE_ID));
        cancelled.set_node_outcome(NodeOutcome::Canceled);
        assert!(
            AggregatedTestGroupMember {
                node: &cancelled,
                is_last: false,
                colorize: false
            }
            .to_string()
            .contains("Cancelled")
        );

        // The connector column is the invariant: members branch from the column where the
        // header's verdict starts, whatever their own action label is.
        let header = AggregatedTestGroupHeader {
            macro_name: "unique",
            attached_node: "model.my_pkg.unproven_unique",
            member_count: 2,
            worst_outcome: TestOutcome::Failed,
            duration: std::time::Duration::from_millis(250),
            node_count_total: None,
            adapter: None,
            colorize: false,
        }
        .to_string();
        for node in [&passed, &cancelled] {
            for (is_last, connector) in [(false, "├─"), (true, "└─")] {
                let member = AggregatedTestGroupMember {
                    node,
                    is_last,
                    colorize: false,
                }
                .to_string();
                assert_eq!(column_of(&header, "Failed"), column_of(&member, connector));
            }
        }

        // Same-length labels share a node type column; a longer one pushes only its own
        // line rightwards, by exactly the extra characters. Padding it back would
        // reintroduce the wide gap this layout exists to remove.
        let passed_member = AggregatedTestGroupMember {
            node: &passed,
            is_last: false,
            colorize: false,
        }
        .to_string();
        let cancelled_member = AggregatedTestGroupMember {
            node: &cancelled,
            is_last: false,
            colorize: false,
        }
        .to_string();
        assert_eq!(
            column_of(&cancelled_member, "test ") - column_of(&passed_member, "test "),
            "Cancelled".len() - "Passed".len(),
        );
    }

    #[test]
    fn aggregated_group_member_last_differs_only_by_connector() {
        let node = test_processed(TestOutcome::Passed, 0, None, Some(BATCH_UNIQUE_ID));
        let non_last = AggregatedTestGroupMember {
            node: &node,
            is_last: false,
            colorize: false,
        }
        .to_string();
        let last = AggregatedTestGroupMember {
            node: &node,
            is_last: true,
            colorize: false,
        }
        .to_string();

        assert_ne!(non_last, last);
        assert_eq!(non_last.replacen("├─", "└─", 1), last);
    }

    #[test]
    fn cached_warned_test_evaluated_formats_zero_duration_and_warning() {
        let output = NodeEvaluatedEnd {
            node: &cached_warned_test_evaluated(),
            duration: std::time::Duration::from_millis(250),
            colorize: false,
        }
        .to_string();

        assert!(output.contains("Finished running"));
        assert!(output.contains("[-------]"));
        assert!(output.contains("[warn]"));
    }
}
