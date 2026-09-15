use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use crate::collections::HashSet;
use console::Term;
use dbt_telemetry::{
    AssetParsed, CompiledCode, CompiledCodeInline, ConnectionLimitWait, DepsAddPackage,
    DepsAllPackagesInstalled, DepsPackageInstalled, ExecutionPhase, GenericOpExecuted,
    GenericOpItemProcessed, HookProcessed, Invocation, ListItemOutput, LogMessage, NodeEvaluated,
    NodeOutcome, NodeProcessed, NodeSkipReason, NodeType, PhaseExecuted, ProgressMessage,
    QueryExecuted, ShowDataOutput, ShowResult, StateModifiedDiff, TestOutcome, UserLogMessage,
    get_test_outcome,
};
use dbt_tracing::{
    AnyTelemetryEvent, LogRecordInfo, SeverityNumber, SpanEndInfo, SpanStartInfo, SpanStatus,
    StatusCode, TelemetryEventRecType, TelemetryOutputFlags,
};
use dbt_tui_progress::ProgressController;

use dbt_error::ErrorCode;
use tracing::level_filters::LevelFilter;

use crate::{
    constants::DBT_GENERIC_TESTS_DIR_NAME, tracing::event_classifiers::is_exit_with_status_log,
    tracing::formatters::node::format_node_evaluated_start_legacy,
};
use crate::{
    io_args::{FsCommand, LogFormat, ShowOptions},
    tracing::{
        data_provider::DataProvider,
        formatters::{
            asset::format_asset_parsed_start,
            color::BLUE,
            connection_limit_wait::{
                format_connection_limit_wait_end, format_connection_limit_wait_start,
            },
            constants::SELECTED_NODES_TITLE,
            deps::{
                INSTALLING_ACTION, format_package_add_end, format_package_add_start,
                format_package_install_end, format_package_install_start,
                format_package_installed_end, format_package_installed_start,
                get_package_display_name,
            },
            generic::{
                capitalize_first_letter, format_generic_op_end, format_generic_op_item_end,
                format_generic_op_item_start, format_generic_op_start,
            },
            hook::{format_hook_processed_end, format_hook_processed_start},
            invocation::format_invocation_summary,
            layout::{format_delimiter, right_align_action},
            log_message::format_log_message,
            node::{
                format_aggregated_test_group_header, format_aggregated_test_group_member,
                format_compiled_code, format_compiled_inline_code, format_node_evaluated_end,
                format_node_evaluated_start, format_node_processed_end, format_skipped_test_group,
            },
            phase::get_phase_progress_text,
            progress::format_progress_message,
            state_mod_diff::format_state_modified_diff_lines,
            test_result::format_test_failure,
        },
        fs_error_log::get_log_message,
        layer::{ConsumerLayer, TelemetryConsumer},
        private_events::print_event::{StderrMessage, StdoutMessage},
    },
};

/// Build TUI layer that handles all terminal user interface on stdout and stderr, including progress bars
pub struct TuiConfig {
    pub show_options: HashSet<ShowOptions>,
    pub command: FsCommand,
    pub quiet: bool,
}

pub fn build_tui_layer(
    max_log_verbosity: LevelFilter,
    log_format: LogFormat,
    config: TuiConfig,
) -> ConsumerLayer {
    // Enables progress bar for now.
    let is_interactive = log_format == LogFormat::Default;

    Box::new(TuiLayer::new(max_log_verbosity, is_interactive, config))
}

/// Identifies progress bars and spinners in the TUI layer.
///
/// This enum decouples the progress bar identity from the display text,
/// allowing stable IDs for lookups while keeping display text flexible.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
enum ProgressId {
    /// Progress bar for a specific execution phase (Render, Analyze, Run, etc.)
    Phase(ExecutionPhase),
    /// Progress bar/spinner for a generic operation identified by operation_id
    GenericOp(String),
    /// Progress bar for dependencies installation
    DepsInstall,
}

/// A special non-exported event type used grouping all `NodeProcessed` spans under
/// a same root, used to report skipped test nodes on one line on the console in
/// interactive TUI mode.
#[derive(Debug, Clone, Copy)]
pub struct TuiAllProcessingNodesGroup;

impl AnyTelemetryEvent for TuiAllProcessingNodesGroup {
    fn event_type(&self) -> &'static str {
        "v1.internal.events.fusion.node.TuiAllProcessingNodesGroup"
    }

    fn event_display_name(&self) -> String {
        "TuiAllProcessingNodesGroup".to_string()
    }

    fn record_category(&self) -> TelemetryEventRecType {
        TelemetryEventRecType::Span
    }

    fn output_flags(&self) -> TelemetryOutputFlags {
        TelemetryOutputFlags::OUTPUT_CONSOLE
    }

    fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .map(|other_ref| std::ptr::eq(self, other_ref))
            .unwrap_or(false)
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
        Box::new(*self)
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({}))
    }
}

/// A special non-exported event type grouping the `NodeProcessed` spans of one aggregated
/// generic-test query, so the console can report them as a single block. Its close is the
/// signal that every member of the group has finished.
#[derive(Debug, Clone)]
pub struct TuiAggregatedTestGroup {
    /// Generic test macro shared by every member of the group, e.g. `not_null`.
    pub macro_name: String,
    /// unique_id of the node the group's tests are attached to.
    pub attached_node: String,
}

impl AnyTelemetryEvent for TuiAggregatedTestGroup {
    fn event_type(&self) -> &'static str {
        "v1.internal.events.fusion.node.TuiAggregatedTestGroup"
    }

    fn event_display_name(&self) -> String {
        format!(
            "Aggregated test group ({} on {})",
            self.macro_name, self.attached_node
        )
    }

    fn record_category(&self) -> TelemetryEventRecType {
        TelemetryEventRecType::Span
    }

    fn output_flags(&self) -> TelemetryOutputFlags {
        TelemetryOutputFlags::OUTPUT_CONSOLE
    }

    fn event_eq(&self, _: &dyn AnyTelemetryEvent) -> bool {
        false
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({
            "macro_name": self.macro_name,
            "attached_node": self.attached_node,
        }))
    }
}

/// Holds a vector of skipped test node names and flags indicating which types were seen
struct SkippedTestNodes {
    pending_names: Vec<String>,
    seen_test: bool,
    seen_unit_test: bool,
    /// Denominator of the `N of M` counter, copied off the accumulated nodes so the summary
    /// line can pad its bracket to the same width as the indexed lines around it.
    node_count_total: Option<u32>,
}

/// Members of one aggregated generic-test group, buffered on the group's span until it
/// closes. Each entry is the member's event plus the duration its flat line would show.
///
/// The events are buffered rather than formatted lines because the tree connectors depend
/// on how many members there turn out to be, which is only known once the group closes.
struct PendingAggregatedTestGroup {
    members: Vec<(NodeProcessed, std::time::Duration)>,
}

fn emit_pending_skips(tui: &TuiLayer, data_provider: &mut DataProvider<'_>) {
    // This is a non-skipping node, check if there are pending skipped tests to emit
    let mut output_to_emit = None;
    data_provider.with_ancestor_ext_mut::<TuiAllProcessingNodesGroup, SkippedTestNodes>(
        |skipped| {
            if !skipped.pending_names.is_empty() {
                // Format the summary and capture it for emission after lock is released
                output_to_emit = Some(format_skipped_test_group(
                    &skipped.pending_names,
                    skipped.seen_test,
                    skipped.seen_unit_test,
                    skipped.node_count_total,
                    true,
                ));

                // Clear the pending names and flags
                skipped.pending_names.clear();
                skipped.seen_test = false;
                skipped.seen_unit_test = false;
                skipped.node_count_total = None;
            }
        },
    );

    // Emit the output after the span lock has been released to avoid possible deadlocks
    if let Some(output) = output_to_emit {
        tui.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", output).as_bytes())
                .expect("failed to write to stdout");
        });
    }
}

fn node_evaluated_progress_status(
    span_status: Option<&SpanStatus>,
    ne: &NodeEvaluated,
) -> Option<&'static str> {
    match ne.node_outcome() {
        NodeOutcome::Success if get_test_outcome(ne.into()) == Some(TestOutcome::Failed) => {
            Some("failed")
        }
        NodeOutcome::Success => Some("succeeded"),
        NodeOutcome::Error => Some("failed"),
        NodeOutcome::Canceled => Some("cancelled"),
        NodeOutcome::Skipped => match ne.node_skip_reason() {
            NodeSkipReason::Cached => Some("reused"),
            NodeSkipReason::NoOp => Some("no-op"),
            NodeSkipReason::Upstream
            | NodeSkipReason::PhaseSkipped
            | NodeSkipReason::PhaseDisabled
            | NodeSkipReason::Unspecified => Some("skipped"),
        },
        NodeOutcome::Unspecified => {
            if let Some(SpanStatus {
                code: StatusCode::Error,
                ..
            }) = span_status
            {
                Some("failed")
            } else {
                Some("succeeded")
            }
        }
    }
}

/// Holds a vector of strings to be printed at the end of the invocation
/// This is used to delay printing of unit test failure tables and errors/warning messages
/// towards the end, right before the invocation summary
struct DelayedMessage {
    message: String,
}

struct DelayedMessages {
    test_failures: Vec<DelayedMessage>,
    errors_and_warnings: Vec<DelayedMessage>,
}

/// Determines whether a progress message should be shown based on its phase and configured ShowOptions.
///
/// The filtering logic maps specific execution phases to their corresponding ShowOptions:
/// - Parse phase -> ProgressParse
/// - Render phase -> ProgressRender
/// - Analyze phase -> ProgressAnalyze
/// - Run phase -> ProgressRun
/// - Hydration phases -> ProgressHydrate
/// - All other phases (including unspecified) -> Progress
///
/// Messages are also shown if ShowOptions::All is enabled.
pub fn should_show_progress_message(
    phase: ExecutionPhase,
    show_options: &HashSet<ShowOptions>,
) -> bool {
    let phase_allows = match phase {
        ExecutionPhase::Parse => show_options.contains(&ShowOptions::ProgressParse),
        ExecutionPhase::Render => show_options.contains(&ShowOptions::ProgressRender),
        ExecutionPhase::Analyze => show_options.contains(&ShowOptions::ProgressAnalyze),
        ExecutionPhase::Run => show_options.contains(&ShowOptions::ProgressRun),
        ExecutionPhase::NodeCacheHydration | ExecutionPhase::DeferHydration => {
            show_options.contains(&ShowOptions::ProgressHydrate)
        }
        ExecutionPhase::SchemaHydration => {
            show_options.contains(&ShowOptions::ProgressHydrate)
                // TODO: The following condition is due to historical logic. Maybe worth reviewing later.
                || show_options.contains(&ShowOptions::Progress)
        }
        // All other phases (including no Phase and Unspecified) match the general Progress option
        _ => show_options.contains(&ShowOptions::Progress),
    };

    phase_allows || show_options.contains(&ShowOptions::All)
}

use crate::collections::SccHashMap;

/// A tracing layer that handles all terminal user interface on stdout and stderr, including progress bars.
///
/// The TuiLayer owns a ProgressController for managing terminal progress bars and spinners.
/// Progress bars are identified by ProgressId, decoupling identity from display text.
pub struct TuiLayer {
    /// TUI has complex filtering logic, so we store max log verbosity here,
    /// instead of applying a blanket filter on the whole layer
    max_log_verbosity: LevelFilter,
    max_term_line_width: Option<usize>,
    show_options: HashSet<ShowOptions>,
    command: FsCommand,
    quiet: bool,
    /// Track if we've emitted the list header yet
    list_header_emitted: AtomicBool,
    /// Whether to group skipped tests under TuiAllProcessingNodesGroup spans
    group_skipped_tests: bool,
    /// Progress bar controller for managing terminal progress indicators.
    /// Wrapped in Arc for the global suspension hook closure (TEMPORARY).
    /// None when not in interactive mode.
    progress: Option<Arc<ProgressController<ProgressId>>>,
    /// Maps operation_id -> is_bar for GenericOp progress bars/spinners.
    /// This mapping is needed because GenericOpItemProcessed only has operation_id,
    /// but we need to know whether to call bar or spinner context methods.
    /// We store the mapping when the parent GenericOpExecuted span starts (or LoadProject
    /// phase for the "load" operation - see TODO in handle_phase_executed_start),
    /// look it up for child items, and remove it when the parent ends.
    generic_op_is_bar: SccHashMap<String, bool>,
    /// Whether running in NEXTEST mode (checked once at init for test purposes)
    #[cfg(debug_assertions)]
    is_nextest: bool,
}

impl TuiLayer {
    pub fn new(max_log_verbosity: LevelFilter, is_interactive: bool, config: TuiConfig) -> Self {
        let TuiConfig {
            show_options,
            command,
            quiet,
        } = config;
        let stdout_term = Term::stdout();
        let is_interactive = is_interactive && stdout_term.is_term();
        let max_term_line_width = stdout_term.size_checked().map(|(_, cols)| cols as usize);

        // Initialize progress controller in interactive mode
        let progress = if is_interactive {
            let mut ctrl = ProgressController::new();
            ctrl.start_ticker();
            Some(Arc::new(ctrl))
        } else {
            None
        };

        let group_skipped_tests = progress.is_some() && max_log_verbosity < LevelFilter::DEBUG;

        #[cfg(debug_assertions)]
        let res = Self {
            max_log_verbosity,
            max_term_line_width,
            show_options,
            command,
            quiet,
            list_header_emitted: AtomicBool::new(false),
            group_skipped_tests,
            progress,
            generic_op_is_bar: SccHashMap::default(),
            is_nextest: std::env::var("NEXTEST").is_ok(),
        };

        #[cfg(not(debug_assertions))]
        let res = Self {
            max_log_verbosity,
            max_term_line_width,
            show_options,
            command,
            quiet,
            list_header_emitted: AtomicBool::new(false),
            group_skipped_tests,
            progress,
            generic_op_is_bar: SccHashMap::default(),
        };

        res
    }

    /// Executes a closure with progress bars suspended for clean output.
    /// If no progress controller is active, just executes the closure directly.
    fn write_suspended<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        match &self.progress {
            Some(p) => p.with_suspended(f),
            None => f(),
        }
    }
}

fn format_unique_id_as_progress_item(unique_id: &str) -> String {
    // Split the unique_id into parts by '.' and take the first and last as the resource type and name
    let parts: Vec<&str> = unique_id.split('.').collect();
    let resource_type = parts.first().unwrap_or(&"unknown");
    let name = parts.last().unwrap_or(&"unknown");
    format!("{resource_type}:{name}")
}

impl TelemetryConsumer for TuiLayer {
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool {
        span.attributes
            .output_flags()
            .contains(TelemetryOutputFlags::OUTPUT_CONSOLE)
            // NodeEvaluated, NodeProcessed, HookProcessed & GenericOp should always be let through
            // because of progress bars relying on them. Their output is controlled
            // in the handler based on the verbosity level.
            && (span.attributes.is::<NodeEvaluated>()
                || span.attributes.is::<NodeProcessed>()
                || span.attributes.is::<ConnectionLimitWait>()
                || span.attributes.is::<HookProcessed>()
                || span.attributes.is::<GenericOpExecuted>()
                || span.attributes.is::<GenericOpItemProcessed>()
                // TOD: This one is also allowed to pass at debug level due to multitude of
                // tests recording prased messages using `progress` cli arg. Special casing
                // should be removed and formatting unified with file layer
                || span.attributes.is::<AssetParsed>()
                || span.severity_number <= self.max_log_verbosity)
    }

    fn is_log_enabled(&self, log_record: &LogRecordInfo) -> bool {
        log_record
            .attributes
            .output_flags()
            .contains(TelemetryOutputFlags::OUTPUT_CONSOLE)
            && log_record.severity_number <= self.max_log_verbosity
            // ExitWithStatus is a pseudo error used only to short-circuit execution, so we
            // filter it from dbt-facing output
            && !is_exit_with_status_log(log_record)
    }

    fn on_span_start(&self, span: &SpanStartInfo, data_provider: &mut DataProvider<'_>) {
        // Init delayed messages storage on root span start
        if span.parent_span_id.is_none() {
            // Root span
            data_provider.init_root(DelayedMessages {
                test_failures: Vec::new(),
                errors_and_warnings: Vec::new(),
            });
        }

        // Init skipped test nodes storage
        if span.attributes.is::<TuiAllProcessingNodesGroup>() {
            data_provider.init_cur::<SkippedTestNodes>(SkippedTestNodes {
                pending_names: Vec::new(),
                seen_test: false,
                seen_unit_test: false,
                node_count_total: None,
            });
        }

        // Init member storage for an aggregated test group
        if span.attributes.is::<TuiAggregatedTestGroup>() {
            data_provider.init_cur::<PendingAggregatedTestGroup>(PendingAggregatedTestGroup {
                members: Vec::new(),
            });
        }

        if let Some(asset_parsed) = span.attributes.downcast_ref::<AssetParsed>() {
            self.handle_asset_parsed_start(span, asset_parsed);
            return;
        }

        // Handle NodeEvaluated start
        if let Some(ne) = span.attributes.downcast_ref::<NodeEvaluated>() {
            self.handle_node_evaluated_start(span, ne);
            return;
        }

        if let Some(wait) = span.attributes.downcast_ref::<ConnectionLimitWait>() {
            self.handle_connection_limit_wait_start(span, wait, data_provider);
            return;
        }

        if let Some(pe) = span.attributes.downcast_ref::<PhaseExecuted>() {
            self.handle_phase_executed_start(span, pe);
            return;
        }

        if let Some(ev) = span.attributes.downcast_ref::<DepsAllPackagesInstalled>() {
            self.handle_deps_all_packages_installing_start(ev);
            return;
        }

        // Handle PackageInstalled
        if let Some(pkg) = span.attributes.downcast_ref::<DepsPackageInstalled>() {
            self.handle_dep_installed_start(pkg);
            return;
        }

        // Handle DepsAddPackage
        if let Some(pkg) = span.attributes.downcast_ref::<DepsAddPackage>() {
            self.handle_package_add_start(pkg);
            return;
        }

        if let Some(op) = span.attributes.downcast_ref::<GenericOpExecuted>() {
            self.handle_generic_op_start(span, op);
            return;
        }

        if let Some(item) = span.attributes.downcast_ref::<GenericOpItemProcessed>() {
            self.handle_generic_op_item_start(span, item);
            return;
        }

        if let Some(hook) = span.attributes.downcast_ref::<HookProcessed>() {
            self.handle_hook_processed_start(span, hook);
        }
    }

    fn on_span_end(&self, span: &SpanEndInfo, data_provider: &mut DataProvider<'_>) {
        // Handle QueryExecuted events
        if let Some(query_data) = span.attributes.downcast_ref::<QueryExecuted>() {
            self.handle_query_executed(span, query_data);
            return;
        }

        if let Some(wait) = span.attributes.downcast_ref::<ConnectionLimitWait>() {
            self.handle_connection_limit_wait_end(span, wait, data_provider);
            return;
        }

        // Handle NodeProcessed events for completed nodes
        if let Some(node_processed) = span.attributes.downcast_ref::<NodeProcessed>() {
            self.handle_node_processed(span, node_processed, data_provider);
            return;
        }

        // Handle NodeEvaluated end
        if let Some(ne) = span.attributes.downcast_ref::<NodeEvaluated>() {
            self.handle_node_evaluated_end(span, ne);
            return;
        }

        if let Some(asset_parsed) = span.attributes.downcast_ref::<AssetParsed>() {
            self.handle_asset_parsed_end(span, asset_parsed);
            return;
        }

        if let Some(ne) = span.attributes.downcast_ref::<PhaseExecuted>() {
            self.handle_phase_executed_end(span, ne);
            return;
        }

        if let Some(ev) = span.attributes.downcast_ref::<DepsAllPackagesInstalled>() {
            self.handle_deps_all_packages_installing_end(ev);
            return;
        }

        // Handle PackageInstalled
        if let Some(pkg) = span.attributes.downcast_ref::<DepsPackageInstalled>() {
            self.handle_dep_installed_end(span, pkg);
            return;
        }

        // Handle DepsAddPackage
        if let Some(pkg) = span.attributes.downcast_ref::<DepsAddPackage>() {
            self.handle_package_add_end(span, pkg);
            return;
        }

        if let Some(op) = span.attributes.downcast_ref::<GenericOpExecuted>() {
            self.handle_generic_op_end(span, op);
            return;
        }

        if let Some(item) = span.attributes.downcast_ref::<GenericOpItemProcessed>() {
            self.handle_generic_op_item_end(span, item);
            return;
        }

        if let Some(hook) = span.attributes.downcast_ref::<HookProcessed>() {
            self.handle_hook_processed_end(span, hook);
            return;
        }

        // An aggregated test group has finished, so its buffered members can be emitted
        // as one block
        if let Some(group) = span.attributes.downcast_ref::<TuiAggregatedTestGroup>() {
            if self.show_options.contains(&ShowOptions::Completed) {
                self.handle_aggregated_test_group_end(group, data_provider);
            }
            return;
        }

        // Handle close of TuiAllProcessingNodesGroup in case we have pending skipped tests
        // to emit after all nodes have been processed
        if span.attributes.is::<TuiAllProcessingNodesGroup>()
            && self.show_options.contains(&ShowOptions::Completed)
        {
            emit_pending_skips(self, data_provider);
            return;
        }

        if let Some(invocation) = span.attributes.downcast_ref::<Invocation>() {
            self.handle_invocation_end(span, invocation, data_provider);
        }
    }

    fn on_log_record(&self, log_record: &LogRecordInfo, data_provider: &mut DataProvider<'_>) {
        // Check if this is a LogMessage (error/warning)
        if let Some(log_msg) = get_log_message(&log_record.attributes) {
            self.handle_log_message(log_msg, log_record, data_provider);
            return;
        }

        if let Some(state_mod_diff) = log_record.attributes.downcast_ref::<StateModifiedDiff>() {
            self.handle_state_modified_diff(state_mod_diff);
            return;
        }

        // Handle ProgressMessage events (debug command progress, etc.)
        if let Some(progress_msg) = log_record.attributes.downcast_ref::<ProgressMessage>() {
            self.handle_progress_message(progress_msg, log_record.severity_number);
            return;
        }

        // Handle simple events that print just the body on it's own line: UserLogMessage
        if log_record.attributes.is::<UserLogMessage>() {
            self.handle_user_log_message(log_record);
            return;
        }

        // Handle simple events that print just the body: StdoutMessage
        if log_record.attributes.is::<StdoutMessage>() {
            self.handle_stdout_message(log_record);
            return;
        }

        // Handle ListItemOutput - only show for list commad unconditionally,
        // or if ShowOptions::Nodes is enabled for other commands
        if let Some(list_item) = log_record.attributes.downcast_ref::<ListItemOutput>() {
            self.handle_list_item_output(list_item);
            return;
        }

        // Handle ShowDataOutput - always show unconditionally. Call-sites decide whether to emit or not.
        if let Some(show_data) = log_record.attributes.downcast_ref::<ShowDataOutput>() {
            self.handle_show_data_output(show_data);
            return;
        }

        // Handle ShowResult - always show unconditionally. Call-sites decide whether to emit or not.
        if let Some(show_result) = log_record.attributes.downcast_ref::<ShowResult>() {
            self.handle_show_result(show_result);
            return;
        }

        // Handle CompiledCode from compile command selected nodes.
        if let Some(compiled_code) = log_record.attributes.downcast_ref::<CompiledCode>() {
            self.handle_compiled_code(compiled_code);
            return;
        }

        // Handle CompiledCodeInline - show if Progress or Completed is enabled
        if let Some(compiled_code) = log_record.attributes.downcast_ref::<CompiledCodeInline>() {
            self.handle_compiled_code_inline(compiled_code);
            return;
        }

        // Handle StderrMessage
        if let Some(stderr_message) = log_record.attributes.downcast_ref::<StderrMessage>() {
            self.handle_stderr_message(stderr_message, log_record);
        }
    }
}

impl TuiLayer {
    fn handle_invocation_end(
        &self,
        span: &SpanEndInfo,
        invocation: &Invocation,
        data_provider: &mut DataProvider<'_>,
    ) {
        // Print any delayed messages first
        data_provider.with_root::<DelayedMessages>(|delayed_messages| {
            let mut stdout = io::stdout().lock();
            let mut stderr = io::stderr().lock();

            // Print test failures with header if any exist (historically on stdout)
            if !delayed_messages.test_failures.is_empty() {
                stdout
                    .write_all(
                        format!(
                            "\n{}\n",
                            format_delimiter(" Test Failures ", self.max_term_line_width, true)
                        )
                        .as_bytes(),
                    )
                    .expect("failed to write to stdout");
                for msg in &delayed_messages.test_failures {
                    stdout
                        .write_all(msg.message.as_bytes())
                        .expect("failed to write to stdout");
                }
            }

            // Print errors and warnings with header if any exist (on stderr)
            if !delayed_messages.errors_and_warnings.is_empty() {
                stderr
                    .write_all(
                        format!(
                            "\n{}\n",
                            format_delimiter(
                                " Errors and Warnings ",
                                self.max_term_line_width,
                                true
                            )
                        )
                        .as_bytes(),
                    )
                    .expect("failed to write to stderr");

                for msg in &delayed_messages.errors_and_warnings {
                    stderr
                        .write_all(msg.message.as_bytes())
                        .expect("failed to write to stderr");
                }
            }

            // Flush streams if we had any messages
            if !delayed_messages.test_failures.is_empty()
                || !delayed_messages.errors_and_warnings.is_empty()
            {
                stderr.flush().expect("failed to write to stderr");
                stdout.flush().expect("failed to write to stdout");
            }
        });

        // Then print the invocation summary
        self.handle_invocation_summary(span, invocation, data_provider);
    }

    fn handle_invocation_summary(
        &self,
        span: &SpanEndInfo,
        invocation: &Invocation,
        data_provider: &DataProvider<'_>,
    ) {
        let formatted = format_invocation_summary(
            span,
            invocation,
            data_provider,
            true,
            self.max_term_line_width,
        );

        let mut stdout = io::stdout().lock();

        // Per pre-migration logic, autofix line were always printed ignoring show options
        if let Some(line) = formatted.autofix_line() {
            stdout
                .write_fmt(format_args!("{}\n", line))
                .expect("failed to write to stdout");
        }

        if !self.show_options.contains(&ShowOptions::Completed)
            && !self.show_options.contains(&ShowOptions::All)
        {
            return;
        }

        if let Some(summary_lines) = formatted.summary_lines() {
            for line in summary_lines {
                stdout
                    .write_fmt(format_args!("{}\n", line))
                    .expect("failed to write to stdout");
            }
        }
    }

    fn handle_phase_executed_start(&self, _span: &SpanStartInfo, phase: &PhaseExecuted) {
        let Some(ref progress) = self.progress else {
            return;
        };

        let total = phase.node_count_total.unwrap_or_default();
        let phase_enum = phase.phase();
        let Some(progress_text) = get_phase_progress_text(phase_enum) else {
            // Do not show progress for phases without defined progress text
            return;
        };

        match phase_enum {
            ExecutionPhase::Render | ExecutionPhase::Analyze | ExecutionPhase::Run => {
                progress.start_bar(ProgressId::Phase(phase_enum), total, progress_text);
            }
            ExecutionPhase::LoadProject => {
                // TODO: Remove this when progress contextual items in loader use dedicated events.
                // Currently, loader items use GenericOpItemProcessed with operation_id="load",
                // so we must register the spinner under that ID for items to find it.
                let load_op_id = "load".to_string();

                // Track that this is a spinner (not a bar) for context item lookups
                #[cfg(debug_assertions)]
                self.generic_op_is_bar
                    .insert_sync(load_op_id.clone(), false)
                    .expect(
                        "A non unique id used for two distinct & concurrent generic operation spans!",
                    );
                #[cfg(not(debug_assertions))]
                self.generic_op_is_bar
                    .upsert_sync(load_op_id.clone(), false);

                progress.start_spinner(ProgressId::GenericOp(load_op_id), progress_text);
            }
            ExecutionPhase::Clean
            | ExecutionPhase::Parse
            | ExecutionPhase::Schedule
            | ExecutionPhase::TaskGraphBuild
            | ExecutionPhase::Debug
            | ExecutionPhase::DeferHydration
            | ExecutionPhase::FreshnessAnalysis
            | ExecutionPhase::OnRunStart
            | ExecutionPhase::OnRunEnd => {
                progress.start_spinner(ProgressId::Phase(phase_enum), progress_text);
            }
            ExecutionPhase::Unspecified
            | ExecutionPhase::Compare
            | ExecutionPhase::InitAdapter
            | ExecutionPhase::NodeCacheHydration
            | ExecutionPhase::SchemaHydration
            | ExecutionPhase::Lineage => {
                // Do not show progress for these phases
            }
        }
    }

    fn handle_phase_executed_end(&self, _span: &SpanEndInfo, phase: &PhaseExecuted) {
        let Some(ref progress) = self.progress else {
            return;
        };

        let phase_enum = phase.phase();
        let Some(_progress_text) = get_phase_progress_text(phase_enum) else {
            // Do not show progress for phases without defined progress text
            return;
        };

        match phase_enum {
            // Close contextual progress bar for render, analyze, run phases
            ExecutionPhase::Render | ExecutionPhase::Analyze | ExecutionPhase::Run => {
                progress.remove_bar(&ProgressId::Phase(phase_enum));
            }
            // Use spinner for phases without progress total
            ExecutionPhase::LoadProject => {
                // TODO: Remove this when progress contextual items in loader use dedicated events.
                // Must match the ID used in handle_phase_executed_start for LoadProject.
                let load_op_id = "load".to_string();

                // Remove from tracking map and get whether it existed
                let was_present = self.generic_op_is_bar.remove_sync(&load_op_id);

                if was_present.is_some() {
                    progress.remove_spinner(&ProgressId::GenericOp(load_op_id));
                } else {
                    #[cfg(debug_assertions)]
                    panic!("A non existing id was used to end a generic operation span!");
                }
            }
            ExecutionPhase::Clean
            | ExecutionPhase::Parse
            | ExecutionPhase::Schedule
            | ExecutionPhase::TaskGraphBuild
            | ExecutionPhase::Debug
            | ExecutionPhase::DeferHydration
            | ExecutionPhase::FreshnessAnalysis
            | ExecutionPhase::OnRunStart
            | ExecutionPhase::OnRunEnd => {
                progress.remove_spinner(&ProgressId::Phase(phase_enum));
            }
            ExecutionPhase::Unspecified
            | ExecutionPhase::Compare
            | ExecutionPhase::InitAdapter
            | ExecutionPhase::NodeCacheHydration
            | ExecutionPhase::SchemaHydration
            | ExecutionPhase::Lineage => {
                // Do not show progress for these phases
            }
        }
    }

    fn set_node_context_idle_state(&self, data_provider: &DataProvider<'_>, idle: bool) {
        let Some(ref progress) = self.progress else {
            return;
        };

        let mut progress_item = None;
        data_provider.with_ancestor_attrs::<NodeEvaluated>(|node| {
            let phase = node.phase();
            if matches!(
                phase,
                ExecutionPhase::Render | ExecutionPhase::Analyze | ExecutionPhase::Run
            ) {
                progress_item = Some((
                    ProgressId::Phase(phase),
                    format_unique_id_as_progress_item(node.unique_id.as_str()),
                ));
            }
        });

        if let Some((progress_id, item)) = progress_item {
            if idle {
                progress.set_bar_context_idle(&progress_id, &item);
            } else {
                progress.set_bar_context_active(&progress_id, &item);
            }
        }
    }

    fn handle_connection_limit_wait_start(
        &self,
        span: &SpanStartInfo,
        wait: &ConnectionLimitWait,
        data_provider: &DataProvider<'_>,
    ) {
        self.set_node_context_idle_state(data_provider, true);

        // In interactive mode, waiting is represented by the progress context
        // item state instead of a separate log line.
        if self.progress.is_some() || span.severity_number > self.max_log_verbosity {
            return;
        }

        let formatted = format_connection_limit_wait_start(wait);
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_connection_limit_wait_end(
        &self,
        span: &SpanEndInfo,
        wait: &ConnectionLimitWait,
        data_provider: &DataProvider<'_>,
    ) {
        self.set_node_context_idle_state(data_provider, false);

        // In interactive mode, waiting is represented by the progress context
        // item state instead of a separate log line.
        if self.progress.is_some() || span.severity_number > self.max_log_verbosity {
            return;
        }

        let duration = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();
        let formatted = format_connection_limit_wait_end(wait, duration);
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_node_evaluated_start(&self, span: &SpanStartInfo, ne: &NodeEvaluated) {
        // Do not emit anything for skipped node phases even in debug
        if ne.node_outcome() == NodeOutcome::Skipped {
            return;
        }

        let phase = ne.phase();

        // Handle progress in interactive mode
        if let Some(ref progress) = self.progress {
            if matches!(
                phase,
                ExecutionPhase::Render | ExecutionPhase::Analyze | ExecutionPhase::Run
            ) {
                let formatted_item = format_unique_id_as_progress_item(ne.unique_id.as_str());
                progress.add_bar_context(&ProgressId::Phase(phase), &formatted_item);
            }
        } else {
            let node_type = ne.node_type();
            let is_yaml_defined_generic_test = node_type == NodeType::Test
                && (ne.relative_path.ends_with(".yml") || ne.relative_path.ends_with(".yaml"));

            // For text mode, use legacy format & logic to maintain test expectations
            if should_show_progress_message(phase, &self.show_options)
                // Only for render, run, and compare.
                // For render, keep legacy filtering: hide seed/unit test and generic YAML tests.
                // Singular SQL tests should still emit render lines.
                // TODO: This legacy path should be phase-based only and not command-dependent.
                && ((phase == ExecutionPhase::Render
                    && !(node_type == NodeType::Seed
                        || is_yaml_defined_generic_test))
                    || phase == ExecutionPhase::Run
                    || phase == ExecutionPhase::Compare)
            {
                let formatted = format_node_evaluated_start_legacy(ne, self.command);
                self.write_suspended(|| {
                    io::stdout()
                        .lock()
                        .write_all(format!("{}\n", formatted).as_bytes())
                        .expect("failed to write to stdout");
                });

                // Avoid double-printing in text mode if this is also debug level
                return;
            }
        }

        // Run/Compare NodeEvaluated spans were upgraded from Debug to Info level so they
        // appear in dbt.log and structured telemetry (JSONL, Parquet, OTLP). However, in
        // the TUI they should only render when debug verbosity is explicitly requested,
        // preserving their previous behavior as Debug-level spans. In interactive mode,
        // progress bars already provide per-node feedback for these phases.
        if matches!(phase, ExecutionPhase::Run | ExecutionPhase::Compare)
            && self.max_log_verbosity < LevelFilter::DEBUG
        {
            return;
        }

        // Print line in debug mode using new format (also used by file log & json)
        if span.severity_number <= self.max_log_verbosity {
            let formatted = format_node_evaluated_start(ne, true);
            self.write_suspended(|| {
                io::stdout()
                    .lock()
                    .write_all(format!("{}\n", formatted).as_bytes())
                    .expect("failed to write to stdout");
            });
        }
    }

    fn handle_node_evaluated_end(&self, span: &SpanEndInfo, ne: &NodeEvaluated) {
        let phase = ne.phase();

        // Handle progress in interactive mode
        if let Some(ref progress) = self.progress {
            if matches!(
                phase,
                ExecutionPhase::Render | ExecutionPhase::Analyze | ExecutionPhase::Run
            ) {
                let status = node_evaluated_progress_status(span.status.as_ref(), ne);
                let formatted_item = format_unique_id_as_progress_item(ne.unique_id.as_str());
                progress.finish_bar_context(&ProgressId::Phase(phase), &formatted_item, status);
            }
        }

        // Run/Compare NodeEvaluated spans were upgraded from Debug to Info level so they
        // appear in dbt.log and structured telemetry (JSONL, Parquet, OTLP). However, in
        // the TUI they should only render when debug verbosity is explicitly requested,
        // preserving their previous behavior as Debug-level spans. In interactive mode,
        // progress bars already provide per-node feedback for these phases.
        if matches!(phase, ExecutionPhase::Run | ExecutionPhase::Compare)
            && self.max_log_verbosity < LevelFilter::DEBUG
        {
            return;
        }

        // Do not emit anything for skipped node phases even in debug
        if ne.node_outcome() == NodeOutcome::Skipped {
            return;
        }

        // Print line only in debug mode.
        // `should_show_progress_message(phase, &self.show_options)` is not consulted
        // due to historical behavior where per-phase processing conclusion had no output.
        if span.severity_number > self.max_log_verbosity {
            return;
        }

        let duration = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();
        let formatted = format_node_evaluated_end(ne, duration, true);
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_query_executed(&self, _span: &SpanEndInfo, query_data: &QueryExecuted) {
        let node_id = query_data.unique_id.as_deref().unwrap_or("unknown");
        let formatted_query = format!("Query executed on node {}:\n{}", node_id, query_data.sql);
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted_query).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_node_processed(
        &self,
        span: &SpanEndInfo,
        node: &NodeProcessed,
        data_provider: &mut DataProvider<'_>,
    ) {
        // Skip nodes with unspecified outcome
        if node.node_outcome() == NodeOutcome::Unspecified {
            return;
        }

        // Skip NoOp nodes (similar to the macro logic), which includes frontier nodes
        if matches!(node.node_skip_reason(), NodeSkipReason::NoOp) {
            return;
        }

        // Do not emit for non-selected nodes (e.g. when model is analyzed in test command)
        if !node.in_selection {
            return;
        }

        // Do not report successful nodes in compile, to reduce verbosity. Unless in debug mode.
        if node.node_outcome() == NodeOutcome::Success
            && self.command == FsCommand::Compile
            && self.max_log_verbosity < LevelFilter::DEBUG
        {
            return;
        }

        // Determine if the current node is a skipped test
        let is_current_node_skipped_test = (node.node_type() == NodeType::Test
            || node.node_type() == NodeType::UnitTest)
            && matches!(node.node_skip_reason(), NodeSkipReason::Upstream);

        // Check if we need to emit pending skipped tests summary before processing current node
        if !is_current_node_skipped_test
            && self.group_skipped_tests
            && self.show_options.contains(&ShowOptions::Completed)
        {
            emit_pending_skips(self, data_provider);
        }

        // Capture and delay unit test summary messages regardless of show options
        if (node.node_type() == NodeType::Test || node.node_type() == NodeType::UnitTest)
            && node.node_outcome() == NodeOutcome::Success
        {
            // This is a failed test, capture its summary diff table to be printed on stdout later
            if let Some(test_failure_message) = format_test_failure(node, true) {
                data_provider.with_root_mut::<DelayedMessages>(|delayed_messages| {
                    delayed_messages.test_failures.push(DelayedMessage {
                        message: format!("{}\n", test_failure_message),
                    });
                });
            }
        }

        // In interactive non-debug mode, accumulate skipped test nodes instead of printing them individually
        if is_current_node_skipped_test && self.group_skipped_tests {
            // Find an ancestor TuiAllProcessingNodesGroup span and add this node to it
            data_provider.with_ancestor_ext_mut::<TuiAllProcessingNodesGroup, SkippedTestNodes>(
                |skipped| {
                    skipped.pending_names.push(node.name.clone());
                    skipped.node_count_total = node.node_count_total;
                    if node.node_type() == NodeType::Test {
                        skipped.seen_test = true;
                    } else if node.node_type() == NodeType::UnitTest {
                        skipped.seen_unit_test = true;
                    }
                },
            );
            // Skip normal printing - our tests will catch if we break something
            return;
        }

        // Only show if ShowOptions::Completed is enabled
        if !self.show_options.contains(&ShowOptions::Completed) {
            return;
        }

        // Use the accumulated duration_ms from NodeProcessed, which reflects actual
        // processing time across all phases (excluding time waiting for upstream nodes).
        // Fall back to span duration if duration_ms is not available.
        let duration = node
            .duration_ms
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| {
                span.end_time_unix_nano
                    .duration_since(span.start_time_unix_nano)
                    .unwrap_or_default()
            });

        // Members of an aggregated generic-test query are buffered on their group span and
        // printed as one block when it closes. Whether this node is a member is answered
        // structurally, by the presence of that ancestor span.
        //
        // The outcome guard keeps members that finished without a reported result (the
        // aggregated run task's error path) on their own flat line, as today: the group
        // header can only render a test verdict, not a node error.
        let mut buffered = false;
        if get_test_outcome(node.into()).is_some() {
            data_provider
                .with_ancestor_ext_mut::<TuiAggregatedTestGroup, PendingAggregatedTestGroup>(
                    |group| {
                        group.members.push((node.clone(), duration));
                        buffered = true;
                    },
                );
        }
        if buffered {
            return;
        }

        // Format the output line using the formatter with color enabled
        let output = format_node_processed_end(node, duration, true);

        // Print to stdout with progress bars suspended
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", output).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    /// Emits an aggregated test group's buffered members as one block, headed by the
    /// group's worst verdict and the query's duration.
    fn handle_aggregated_test_group_end(
        &self,
        group: &TuiAggregatedTestGroup,
        data_provider: &mut DataProvider<'_>,
    ) {
        // The closing span is included in the ancestor search, so this reads the members
        // off the group span itself
        let mut members = Vec::new();
        data_provider.with_ancestor_ext_mut::<TuiAggregatedTestGroup, PendingAggregatedTestGroup>(
            |pending| {
                members = std::mem::take(&mut pending.members);
            },
        );

        if members.is_empty() {
            return;
        }

        // The header's duration is the worst member's accumulated processing time, not the
        // group span's wall time, which also covers waiting on upstream nodes.
        let worst_outcome = members
            .iter()
            .map(|(node, _)| get_test_outcome(node.into()).unwrap_or(TestOutcome::Passed))
            .max()
            .unwrap_or(TestOutcome::Passed);
        let max_duration = members
            .iter()
            .map(|(_, duration)| *duration)
            .max()
            .unwrap_or_default();

        // Written as one atomic stdout write, so concurrently completing nodes cannot
        // interleave lines into the block.
        let mut block = format!(
            "{}\n",
            format_aggregated_test_group_header(
                &group.macro_name,
                &group.attached_node,
                members.len(),
                worst_outcome,
                max_duration,
                // Every member shares the one invocation-wide denominator.
                members.first().and_then(|(node, _)| node.node_count_total),
                true,
            )
        );
        for (index, (node, _)) in members.iter().enumerate() {
            block.push_str(&format_aggregated_test_group_member(
                node,
                index + 1 == members.len(),
                true,
            ));
            block.push('\n');
        }

        // Emit after the span lock has been released to avoid possible deadlocks
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(block.as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_log_message(
        &self,
        log_msg: &LogMessage,
        log_record: &LogRecordInfo,
        data_provider: &mut DataProvider<'_>,
    ) {
        // Format the message
        let formatted_message = format_log_message(
            log_msg
                .code
                .and_then(|c| u16::try_from(c).ok())
                .and_then(|c| ErrorCode::try_from(c).ok()),
            &log_record.body,
            log_record.severity_number,
            true,
            true,
        );

        // Delay errors and warnings to be printed at the end
        if log_record.severity_number > SeverityNumber::Info {
            data_provider.with_root_mut::<DelayedMessages>(|delayed_messages| {
                delayed_messages.errors_and_warnings.push(DelayedMessage {
                    message: format!("{}\n", formatted_message),
                });
            });
        } else {
            // Print info and below messages immediately
            self.write_suspended(|| {
                io::stdout()
                    .lock()
                    .write_all(format!("{}\n", formatted_message).as_bytes())
                    .expect("failed to write to stdout");
            });
        }
    }

    fn handle_state_modified_diff(&self, state_mod_diff: &StateModifiedDiff) {
        let formatted = format_state_modified_diff_lines(state_mod_diff).join("\n");
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{formatted}\n").as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_user_log_message(&self, log_record: &LogRecordInfo) {
        // Print user log messages immediately to stdout
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", log_record.body).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_stdout_message(&self, log_record: &LogRecordInfo) {
        // Print immediately to stdout
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(log_record.body.as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_list_item_output(&self, list_item: &ListItemOutput) {
        if self.show_options.contains(&ShowOptions::Nodes) || self.command == FsCommand::List {
            self.write_suspended(|| {
                let mut stdout = io::stdout().lock();

                // Only emit the decorative header when show_options is non-empty (i.e. not quiet).
                // In quiet mode show_options is cleared, so the header is suppressed while list
                // item content (the result payload) continues to be printed — matching dbt-core's
                // PrintEvent behaviour where only decorative chrome is stripped.
                if !self.show_options.is_empty()
                    && !self.list_header_emitted.swap(true, Ordering::Relaxed)
                {
                    let header =
                        format_delimiter(SELECTED_NODES_TITLE, self.max_term_line_width, true);
                    stdout
                        .write_all(format!("{}\n", header).as_bytes())
                        .expect("failed to write header to stdout");
                }

                // Print list item content (always — result payload survives quiet)
                stdout
                    .write_all(format!("{}\n", list_item.content).as_bytes())
                    .expect("failed to write to stdout");
            });
        }
    }

    fn handle_show_data_output(&self, show_data: &ShowDataOutput) {
        self.write_suspended(|| {
            let mut stdout = io::stdout().lock();

            stdout
                .write_all(format!("{}\n", show_data.content).as_bytes())
                .expect("failed to write show data to stdout");
        });
    }

    fn handle_show_result(&self, show_result: &ShowResult) {
        self.write_suspended(|| {
            let mut stdout = io::stdout().lock();

            if self.show_options.is_empty() {
                // Quiet mode: suppress decorative title, emit raw content only.
                // Mirrors dbt-core's ShowNode quiet=True behaviour where the
                // "Previewing node 'X':" header is dropped but the data is kept.
                stdout
                    .write_all(format!("{}\n", show_result.content).as_bytes())
                    .expect("failed to write show result to stdout");
            } else {
                let colored_title = BLUE.apply_to(&show_result.title);
                stdout
                    .write_all(format!("\n{}\n{}\n", colored_title, show_result.content).as_bytes())
                    .expect("failed to write show result to stdout");
            }
        });
    }

    fn handle_compiled_code_inline(&self, compiled_code: &CompiledCodeInline) {
        if self.quiet {
            self.write_suspended(|| {
                io::stdout()
                    .lock()
                    .write_all(format!("{}\n", compiled_code.sql).as_bytes())
                    .expect("failed to write compiled code to stdout");
            });
            return;
        }

        // Only show if any Progress*, Completed or All option is enabled
        let should_show = self.show_options.contains(&ShowOptions::Progress)
            || self.show_options.contains(&ShowOptions::ProgressRender)
            || self.show_options.contains(&ShowOptions::Completed)
            || self.show_options.contains(&ShowOptions::All);

        if !should_show {
            return;
        }

        let formatted = format_compiled_inline_code(compiled_code, true);
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted).as_bytes())
                .expect("failed to write compiled code to stdout");
        });
    }

    fn handle_compiled_code(&self, compiled_code: &CompiledCode) {
        if self.command != FsCommand::Compile {
            return;
        }

        if !should_show_progress_message(ExecutionPhase::Render, &self.show_options) {
            return;
        }

        let formatted = format_compiled_code(compiled_code, true);
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted).as_bytes())
                .expect("failed to write compiled code to stdout");
        });
    }

    fn handle_stderr_message(&self, stderr_message: &StderrMessage, log_record: &LogRecordInfo) {
        // Format the message
        let formatted_message = format_log_message(
            stderr_message.error_code(),
            &log_record.body,
            log_record.severity_number,
            true,
            true,
        );

        // Print user log messages immediately to stdout
        self.write_suspended(|| {
            io::stderr()
                .lock()
                .write_all(format!("{formatted_message}\n").as_bytes())
                .expect("failed to write to stderr");
        });
    }

    fn handle_progress_message(
        &self,
        progress_msg: &ProgressMessage,
        severity_number: SeverityNumber,
    ) {
        if !should_show_progress_message(progress_msg.phase(), &self.show_options) {
            return;
        }

        let formatted = format_progress_message(progress_msg, severity_number, true, true);
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_asset_parsed_start(&self, _span: &SpanStartInfo, asset: &AssetParsed) {
        // Legacy filter exclusion for generic tests, which are generated rather than
        // authored and so are noise in both the progress indicator and the log line.
        if asset.display_path.contains(DBT_GENERIC_TESTS_DIR_NAME) {
            return;
        }

        // Show the file on the phase's progress indicator. Keyed on the asset's own
        // phase so this is a no-op for phases that register no spinner.
        if let Some(ref progress) = self.progress {
            progress.add_spinner_context(&ProgressId::Phase(asset.phase()), &asset.display_path);
        }

        // TODO: This is temporary legacy rendering for parse progress and should
        // be replaced with the new end-span rendering (matching file log) once fully migrated.
        // We ignore this span severity level and only check show options for progress.

        if !should_show_progress_message(asset.phase(), &self.show_options) {
            return;
        }

        let formatted = format_asset_parsed_start(asset, true);
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_asset_parsed_end(&self, _span: &SpanEndInfo, asset: &AssetParsed) {
        let Some(ref progress) = self.progress else {
            return;
        };

        // Must mirror the exclusion in `handle_asset_parsed_start`, or the excluded
        // assets would decrement a counter they never incremented.
        if asset.display_path.contains(DBT_GENERIC_TESTS_DIR_NAME) {
            return;
        }

        progress.finish_spinner_context(
            &ProgressId::Phase(asset.phase()),
            &asset.display_path,
            Some("parsed"),
        );
    }

    fn handle_deps_all_packages_installing_start(&self, ev: &DepsAllPackagesInstalled) {
        // Do not show anything if ShowOptions::Progress is not enabled
        if !self.show_options.contains(&ShowOptions::Progress)
            && !self.show_options.contains(&ShowOptions::All)
        {
            return;
        }

        if let Some(ref progress) = self.progress {
            // In interactive mode, start a progress bar
            progress.start_bar(
                ProgressId::DepsInstall,
                ev.package_count,
                INSTALLING_ACTION.clone(),
            );

            // In interactive non-debug mode, skip the "Installing packages" message
            // since the progress bar will show context items for each package
            if self.max_log_verbosity < LevelFilter::DEBUG {
                return;
            }
        }

        // In non-interactive mode (or debug mode) - print static message when starting
        let formatted_message = format_package_install_start(ev, true);

        // Print immediately to stdout
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted_message).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_deps_all_packages_installing_end(&self, ev: &DepsAllPackagesInstalled) {
        // Do not show anything if ShowOptions::Progress is not enabled
        if !self.show_options.contains(&ShowOptions::Progress)
            && !self.show_options.contains(&ShowOptions::All)
        {
            return;
        }

        if let Some(ref progress) = self.progress {
            // In interactive mode, stop the progress bar
            progress.remove_bar(&ProgressId::DepsInstall);
        }

        // Regardless of the mode - print static message when finished
        let formatted_message = format_package_install_end(ev, true);

        // Print immediately to stdout
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted_message).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_dep_installed_start(&self, pkg: &DepsPackageInstalled) {
        // Only show if ShowOptions::Progress is enabled
        if !self.show_options.contains(&ShowOptions::Progress)
            && !self.show_options.contains(&ShowOptions::All)
        {
            return;
        }

        // In interactive add the package as a context item to the progress bar
        if let Some(ref progress) = self.progress {
            if let Some(display_name) = get_package_display_name(pkg) {
                progress.add_bar_context(&ProgressId::DepsInstall, display_name);
            }

            // In non-debug mode, skip the "Installing package" message
            if self.max_log_verbosity < LevelFilter::DEBUG {
                return;
            }
        }

        #[cfg(debug_assertions)]
        {
            // In debug builds, skip printing package installed messages in NEXTEST mode
            // This was historically done to avoid unstable order of these in test output
            if self.is_nextest {
                return;
            }
        }

        // In non-interactive mode (or debug mode) - print static message
        let formatted_message = format_package_installed_start(pkg, true);

        // Print immediately to stdout
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted_message).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_dep_installed_end(&self, span: &SpanEndInfo, pkg: &DepsPackageInstalled) {
        // Only process if ShowOptions::Progress is enabled
        if !self.show_options.contains(&ShowOptions::Progress)
            && !self.show_options.contains(&ShowOptions::All)
        {
            return;
        }

        // In interactive mode, update the progress bar
        if let Some(ref progress) = self.progress {
            if let Some(display_name) = get_package_display_name(pkg) {
                let status = if let Some(SpanStatus {
                    code: StatusCode::Error,
                    ..
                }) = &span.status
                {
                    Some("failed")
                } else {
                    Some("succeeded")
                };
                progress.finish_bar_context(&ProgressId::DepsInstall, display_name, status);
            } else {
                // Just increment the progress bar counter if we can't get a display name
                progress.inc_bar(&ProgressId::DepsInstall, 1);
            }
        }

        #[cfg(debug_assertions)]
        {
            // In debug builds, skip printing package installed messages in NEXTEST mode
            // This was historically done to avoid unstable order of these in test output
            if self.is_nextest {
                return;
            }
        }

        // Format with shared formatter (colorize = true for TUI)
        let formatted_message = format_package_installed_end(
            pkg,
            span.status.as_ref().map_or(StatusCode::Unset, |s| s.code),
            true,
        );

        // Print immediately to stdout
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted_message).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_hook_processed_start(&self, span: &SpanStartInfo, hook: &HookProcessed) {
        // Handle progress in interactive mode - show hooks in progress
        if let Some(ref progress) = self.progress
            && let Some(hook_name) = hook.name.as_deref()
        {
            let progress_id = ProgressId::Phase(hook.phase());
            progress.add_bar_context(&progress_id, hook_name);
        }

        // Only show if ShowOptions::All is enabled or DEBUG verbosity
        if !self.show_options.contains(&ShowOptions::All)
            && self.max_log_verbosity < LevelFilter::DEBUG
        {
            return;
        }

        // Do not show if max verbosity is lower than span verbosity
        if span.severity_number > self.max_log_verbosity {
            return;
        }

        let formatted_message = format_hook_processed_start(hook, true);

        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{formatted_message}\n").as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_hook_processed_end(&self, span: &SpanEndInfo, hook: &HookProcessed) {
        // Handle progress in interactive mode
        if let Some(ref progress) = self.progress
            && let Some(hook_name) = hook.name.as_deref()
        {
            let progress_id = ProgressId::Phase(hook.phase());
            let status = match span.status {
                Some(SpanStatus {
                    code: StatusCode::Error,
                    ..
                }) => Some("failed"),
                Some(SpanStatus {
                    code: StatusCode::Ok,
                    ..
                }) => Some("succeeded"),
                _ => None,
            };
            progress.finish_bar_context(&progress_id, hook_name, status);
        }

        // Only show if ShowOptions::Completed or All is enabled
        if !self.show_options.contains(&ShowOptions::Completed)
            && !self.show_options.contains(&ShowOptions::All)
        {
            return;
        }

        // Do not show if max verbosity is lower than span verbosity
        if span.severity_number > self.max_log_verbosity {
            return;
        }

        let duration = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();

        let formatted_message = format_hook_processed_end(hook, duration, true);

        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted_message).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_package_add_start(&self, pkg: &DepsAddPackage) {
        // Only show if ShowOptions::Progress is enabled
        if !self.show_options.contains(&ShowOptions::Progress)
            && !self.show_options.contains(&ShowOptions::All)
        {
            return;
        }

        // Format with shared formatter (colorize = true for TUI)
        let formatted_message = format_package_add_start(pkg, true);

        // Print immediately to stdout
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted_message).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_package_add_end(&self, span: &SpanEndInfo, pkg: &DepsAddPackage) {
        // Only process if ShowOptions::Progress is enabled
        if !self.show_options.contains(&ShowOptions::Progress)
            && !self.show_options.contains(&ShowOptions::All)
        {
            return;
        }

        // Format with shared formatter (colorize = true for TUI)
        let formatted_message = format_package_add_end(
            pkg,
            span.status.as_ref().map_or(StatusCode::Unset, |s| s.code),
            true,
        );

        // Print immediately to stdout
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted_message).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_generic_op_start(&self, span: &SpanStartInfo, op: &GenericOpExecuted) {
        // Handle progress bars in interactive mode
        if let Some(ref progress) = self.progress {
            // Create action text for the progress bar or spinner
            let progress_text =
                right_align_action(capitalize_first_letter(op.display_action.as_str()).into());
            let is_bar = op.item_count_total.is_some();

            // Track whether this op uses a bar or spinner for later lookups.
            // In debug builds panic if id is not unique, but in prod just overwrite.
            #[cfg(debug_assertions)]
            self.generic_op_is_bar
                .insert_sync(op.operation_id.clone(), is_bar)
                .expect(
                    "A non unique id used for two distinct & concurrent generic operation spans!",
                );
            #[cfg(not(debug_assertions))]
            self.generic_op_is_bar
                .upsert_sync(op.operation_id.clone(), is_bar);

            let progress_id = ProgressId::GenericOp(op.operation_id.clone());
            match op.item_count_total {
                Some(total) => {
                    // Start a progress bar if we have a total count
                    progress.start_bar(progress_id, total, progress_text.to_string());
                }
                None => {
                    // Start a spinner if we don't have a total count
                    progress.start_spinner(progress_id, progress_text.to_string());
                }
            };

            // Return, as we do not want to show both static message and progress bar in interactive mode
            return;
        }

        // Only show if ShowOptions::Progress or All is enabled and non-interactive mode
        if !self.show_options.contains(&ShowOptions::Progress)
            && !self.show_options.contains(&ShowOptions::All)
        {
            return;
        }

        // Do not show if max verbosity is lower than span verbosity
        if span.severity_number > self.max_log_verbosity {
            return;
        }

        let formatted_message = format_generic_op_start(op, true);

        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted_message).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_generic_op_end(&self, span: &SpanEndInfo, op: &GenericOpExecuted) {
        // Handle progress bars in interactive mode
        if let Some(ref progress) = self.progress {
            // Get whether this op uses a bar or spinner and remove from tracking
            let is_bar_entry = self.generic_op_is_bar.remove_sync(&op.operation_id);

            let progress_id = ProgressId::GenericOp(op.operation_id.clone());
            if let Some((_, is_bar)) = is_bar_entry {
                if is_bar {
                    progress.remove_bar(&progress_id);
                } else {
                    progress.remove_spinner(&progress_id);
                }
            } else {
                #[cfg(debug_assertions)]
                panic!("A non existing id was used to end a generic operation span!");
            }

            // Progress is transient, so we fall through to allow end line be printed in interactive mode
        }

        // Only show conclusion line if ShowOptions::Completed or All is enabled
        if !self.show_options.contains(&ShowOptions::Completed)
            && !self.show_options.contains(&ShowOptions::All)
        {
            return;
        }

        // Do not show if max verbosity is lower than span verbosity
        if span.severity_number > self.max_log_verbosity {
            return;
        }

        // Compute duration from span start/end times
        let duration = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();

        // Format with shared formatter (colorize = true for TUI)
        let formatted_message = format_generic_op_end(op, duration, span.status.as_ref(), true);

        // Print conclusion line
        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted_message).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_generic_op_item_start(&self, span: &SpanStartInfo, item: &GenericOpItemProcessed) {
        // Handle progress in interactive mode
        if let Some(ref progress) = self.progress {
            let progress_id = ProgressId::GenericOp(item.operation_id.clone());
            self.generic_op_is_bar
                .read_sync(&item.operation_id, |_, is_bar| {
                    if *is_bar {
                        progress.add_bar_context(&progress_id, &item.target);
                    } else {
                        progress.add_spinner_context(&progress_id, &item.target);
                    }
                });
        }

        // Only show if ShowOptions::All is enabled or DEBUG verbosity
        if !self.show_options.contains(&ShowOptions::All)
            && self.max_log_verbosity < LevelFilter::DEBUG
        {
            return;
        }

        // Do not show if max verbosity is lower than span verbosity
        if span.severity_number > self.max_log_verbosity {
            return;
        }

        let formatted_message = format_generic_op_item_start(item);

        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted_message).as_bytes())
                .expect("failed to write to stdout");
        });
    }

    fn handle_generic_op_item_end(&self, span: &SpanEndInfo, item: &GenericOpItemProcessed) {
        // Handle progress in interactive mode
        if let Some(ref progress) = self.progress {
            let progress_id = ProgressId::GenericOp(item.operation_id.clone());
            let status = match span.status {
                Some(SpanStatus {
                    code: StatusCode::Error,
                    ..
                }) => Some("failed"),
                Some(SpanStatus {
                    code: StatusCode::Ok,
                    ..
                }) => Some("succeeded"),
                _ => None,
            };

            self.generic_op_is_bar
                .read_sync(&item.operation_id, |_, is_bar| {
                    if *is_bar {
                        progress.finish_bar_context(&progress_id, &item.target, status);
                    } else {
                        progress.finish_spinner_context(&progress_id, &item.target, status);
                    }
                });
        }

        // Lint/format run one of these spans per source file, including every
        // macro -- an order of magnitude more items than a typical node-based
        // command schedules, so `ShowOptions::Completed` alone (already on by
        // default) would print one line per file. Require `All` for these
        // commands specifically; other commands keep the normal `Completed`
        // gate.
        let item_visible = if matches!(
            self.command,
            FsCommand::Extension("lint")
                | FsCommand::Extension("format")
                | FsCommand::Extension("format-legacy")
        ) {
            self.show_options.contains(&ShowOptions::All)
        } else {
            self.show_options.contains(&ShowOptions::Completed)
                || self.show_options.contains(&ShowOptions::All)
        };
        if !item_visible {
            return;
        }

        // Do not show if max verbosity is lower than span verbosity
        if span.severity_number > self.max_log_verbosity {
            return;
        }

        let duration = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();

        let formatted_message =
            format_generic_op_item_end(item, duration, span.status.as_ref(), true);

        self.write_suspended(|| {
            io::stdout()
                .lock()
                .write_all(format!("{}\n", formatted_message).as_bytes())
                .expect("failed to write to stdout");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_telemetry::{NodeOutcomeDetail, TestEvaluationDetail};

    fn unit_test_evaluated_with_outcome(test_outcome: TestOutcome) -> NodeEvaluated {
        let mut node = NodeEvaluated::start(
            "unit_test.jaffle_shop.stg_locations.test_opened_at".to_string(),
            "test_opened_at".to_string(),
            None,
            Some("dbt_test__audit".to_string()),
            None,
            None,
            None,
            NodeType::UnitTest,
            ExecutionPhase::Run,
            "models/staging/stg_locations.yml".to_string(),
            Some(10),
            Some(5),
            "checksum".to_string(),
        );
        node.set_node_outcome(NodeOutcome::Success);
        node.node_outcome_detail = Some(NodeOutcomeDetail::NodeTestDetail(
            TestEvaluationDetail::new(test_outcome, 1, None, None, None, None),
        ));
        node
    }

    #[test]
    fn failed_unit_test_counts_as_failed_for_progress() {
        let node = unit_test_evaluated_with_outcome(TestOutcome::Failed);

        assert_eq!(node_evaluated_progress_status(None, &node), Some("failed"));
    }

    #[test]
    fn passed_unit_test_counts_as_succeeded_for_progress() {
        let node = unit_test_evaluated_with_outcome(TestOutcome::Passed);

        assert_eq!(
            node_evaluated_progress_status(None, &node),
            Some("succeeded")
        );
    }

    #[test]
    fn test_outcome_ordering_ranks_severity() {
        // An aggregated group header takes its verdict from `max`, so a new variant added
        // out of severity order would silently outrank `Failed`.
        assert!(TestOutcome::Passed < TestOutcome::Warned);
        assert!(TestOutcome::Warned < TestOutcome::Failed);
    }
}
