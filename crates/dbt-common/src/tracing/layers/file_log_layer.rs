use dbt_error::ErrorCode;
use dbt_telemetry::{
    AssetParsed, CompiledCode, CompiledCodeInline, DepsAddPackage, DepsAllPackagesInstalled,
    DepsPackageInstalled, GenericOpExecuted, GenericOpItemProcessed, Invocation, ListItemOutput,
    LogMessage, MicrobatchEvaluated, NodeEvaluated, NodeOutcome, NodeProcessed, NodeType,
    PhaseExecuted, ProgressMessage, QueryExecuted, ShowDataOutput, ShowResult, StateModifiedDiff,
    ThreadPoolWait, UserLogMessage,
};
use std::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
    time::SystemTime,
};
use tracing::level_filters::LevelFilter;

use dbt_tracing::{
    LogRecordInfo, SeverityNumber, SpanEndInfo, SpanStartInfo, StatusCode, TelemetryOutputFlags,
    background_writer::BackgroundWriter,
    data_provider::DataProvider,
    layer::{ConsumerLayer, TelemetryConsumer},
    shared_writer::SharedWriter,
    shutdown::TelemetryShutdownItem,
};

use super::super::{
    event_classifiers::is_exit_with_status_log,
    formatters::{
        asset::AssetParsedEnd,
        constants::SELECTED_NODES_TITLE,
        deps::{
            PackageAddEnd, PackageAddStart, PackageInstallEnd, PackageInstallStart,
            PackageInstalledEnd, PackageInstalledStart,
        },
        duration::{TimeOnlyTimestamp, UtcZuluTimestamp},
        generic::{GenericOpEnd, GenericOpItemEnd, GenericOpItemStart, GenericOpStart},
        invocation::InvocationSummary,
        layout::Delimiter,
        log_message::LogMessageLine,
        meta::severity_fixed_width,
        node::{
            CompiledCodeLine, CompiledInlineCode, MicrobatchEvaluatedEnd, MicrobatchEvaluatedStart,
            NodeEvaluatedEnd, NodeEvaluatedStart, NodeProcessedEnd, NodeProcessedStart,
        },
        phase::{PhaseExecutedEnd, PhaseExecutedStart},
        progress::ProgressMessageLine,
        state_mod_diff::StateModifiedDiffLines,
        test_result::TestFailure,
        thread_pool_wait::{THREAD_POOL_WAIT_START, ThreadPoolWaitEnd},
    },
    fs_error_log::get_log_message,
};

const HEADER_SEPARATOR: &str = "====================";

/// Build file log layer with a background writer. This is preferred for writing to
/// slow IO sinks like files.
pub fn build_file_log_layer_with_background_writer<W: std::io::Write + Send + 'static>(
    writer: W,
    max_log_verbosity: LevelFilter,
) -> (ConsumerLayer, TelemetryShutdownItem) {
    let (writer, handle) = BackgroundWriter::new(writer);

    (
        Box::new(FileLogLayer::new(writer).with_filter(max_log_verbosity)),
        Box::new(handle),
    )
}

pub struct FileLogLayer {
    writer: Box<dyn SharedWriter>,
    /// Track if we've emitted the list header yet
    list_header_emitted: AtomicBool,
}

impl FileLogLayer {
    pub fn new<W: SharedWriter + 'static>(writer: W) -> Self {
        Self {
            writer: Box::new(writer),
            list_header_emitted: AtomicBool::new(false),
        }
    }

    /// Write a log line with timestamp and severity level prefix.
    fn write_log_line(
        &self,
        timestamp: SystemTime,
        severity: SeverityNumber,
        line: &dyn fmt::Display,
    ) {
        self.writer.writeln(&format_args!(
            "{} {}: {}",
            TimeOnlyTimestamp(timestamp),
            severity_fixed_width(severity),
            line
        ));
    }
}

impl TelemetryConsumer for FileLogLayer {
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool {
        span.attributes
            .output_flags()
            .contains(TelemetryOutputFlags::OUTPUT_LOG_FILE)
    }

    fn is_log_enabled(&self, log_record: &LogRecordInfo) -> bool {
        log_record
        .attributes
        .output_flags()
        .contains(TelemetryOutputFlags::OUTPUT_LOG_FILE)
            // ExitWithStatus is a pseudo error used only to short-circuit execution, so we
            // filter it from dbt-facing output
            && !is_exit_with_status_log(log_record)
    }

    fn on_span_start(&self, span: &SpanStartInfo, _: &mut DataProvider<'_>) {
        // Write header line when invocation starts
        if let Some(invocation) = span.attributes.downcast_ref::<Invocation>() {
            self.handle_invocation_start(span, invocation);
            return;
        }

        // Handle PhaseExecuted start
        if let Some(phase) = span.attributes.downcast_ref::<PhaseExecuted>() {
            self.handle_phase_executed_start(span, phase);
            return;
        }

        // Handle NodeEvaluated start
        if let Some(ne) = span.attributes.downcast_ref::<NodeEvaluated>() {
            self.handle_node_evaluated_start(span, ne);
            return;
        }

        // Handle MicrobatchEvaluated start
        if let Some(batch) = span.attributes.downcast_ref::<MicrobatchEvaluated>() {
            self.handle_microbatch_evaluated_start(span, batch);
            return;
        }

        // Handle NodeProcessed start
        if let Some(node_processed) = span.attributes.downcast_ref::<NodeProcessed>() {
            self.handle_node_processed_start(span, node_processed);
            return;
        }

        if span.attributes.is::<ThreadPoolWait>() {
            self.handle_thread_pool_wait_start(span);
            return;
        }

        // Handle DepsAllPackagesInstalled start
        if let Some(ev) = span.attributes.downcast_ref::<DepsAllPackagesInstalled>() {
            self.handle_deps_all_packages_installing_start(span, ev);
            return;
        }

        // Handle DepsPackageInstalled start
        if let Some(pkg) = span.attributes.downcast_ref::<DepsPackageInstalled>() {
            self.handle_dep_installed_start(span, pkg);
            return;
        }

        // Handle DepsAddPackage start
        if let Some(pkg) = span.attributes.downcast_ref::<DepsAddPackage>() {
            self.handle_package_add_start(span, pkg);
            return;
        }

        // Handle GenericOpExecuted start
        if let Some(op) = span.attributes.downcast_ref::<GenericOpExecuted>() {
            self.handle_generic_op_start(span, op);
            return;
        }

        // Handle GenericOpItemProcessed start
        if let Some(item) = span.attributes.downcast_ref::<GenericOpItemProcessed>() {
            self.handle_generic_op_item_start(span, item);
        }
    }

    fn on_span_end(&self, span: &SpanEndInfo, data_provider: &mut DataProvider<'_>) {
        // Query log (it has a separate layer, a dedicated sql file, but also goes to dbt.log as of today)
        if let Some(query_data) = span.attributes.downcast_ref::<QueryExecuted>() {
            self.handle_query_executed(span, query_data);
            return;
        }

        if span.attributes.is::<ThreadPoolWait>() {
            self.handle_thread_pool_wait_end(span);
            return;
        }

        // Handle PhaseExecuted end
        if let Some(phase) = span.attributes.downcast_ref::<PhaseExecuted>() {
            self.handle_phase_executed_end(span, phase);
            return;
        }

        if let Some(asset_parsed) = span.attributes.downcast_ref::<AssetParsed>() {
            self.handle_asset_parsed_end(span, asset_parsed);
            return;
        }

        // Handle NodeProcessed events for completed nodes
        if let Some(node_processed) = span.attributes.downcast_ref::<NodeProcessed>() {
            self.handle_node_processed(span, node_processed);
            return;
        }

        // Handle NodeEvaluated events
        if let Some(ne) = span.attributes.downcast_ref::<NodeEvaluated>() {
            self.handle_node_evaluated(span, ne);
            return;
        }

        // Handle MicrobatchEvaluated events
        if let Some(batch) = span.attributes.downcast_ref::<MicrobatchEvaluated>() {
            self.handle_microbatch_evaluated(span, batch);
            return;
        }

        // Invocation end
        if let Some(invocation) = span.attributes.downcast_ref::<Invocation>() {
            self.handle_invocation_end(span, invocation, data_provider);
            return;
        }

        // Handle DepsAllPackagesInstalled end
        if let Some(ev) = span.attributes.downcast_ref::<DepsAllPackagesInstalled>() {
            self.handle_deps_all_packages_installing_end(span, ev);
            return;
        }

        // Handle DepsPackageInstalled end
        if let Some(pkg) = span.attributes.downcast_ref::<DepsPackageInstalled>() {
            self.handle_dep_installed_end(span, pkg);
            return;
        }

        // Handle DepsAddPackage end
        if let Some(pkg) = span.attributes.downcast_ref::<DepsAddPackage>() {
            self.handle_package_add_end(span, pkg);
            return;
        }

        // Handle GenericOpExecuted end
        if let Some(op) = span.attributes.downcast_ref::<GenericOpExecuted>() {
            self.handle_generic_op_end(span, op);
            return;
        }

        // Handle GenericOpItemProcessed end
        if let Some(item) = span.attributes.downcast_ref::<GenericOpItemProcessed>() {
            self.handle_generic_op_item_end(span, item);
        }
    }

    fn on_log_record(&self, log_record: &LogRecordInfo, _data_provider: &mut DataProvider<'_>) {
        // Check if this is a LogMessage (error/warning)
        if let Some(log_msg) = get_log_message(&log_record.attributes) {
            self.handle_log_message(log_msg, log_record);
            return;
        }

        if let Some(state_mod_diff) = log_record.attributes.downcast_ref::<StateModifiedDiff>() {
            self.handle_state_modified_diff(log_record, state_mod_diff);
            return;
        }

        // Handle ProgressMessage events (debug command progress, etc.)
        if let Some(progress_msg) = log_record.attributes.downcast_ref::<ProgressMessage>() {
            self.handle_progress_message(log_record, progress_msg);
            return;
        }

        // Handle UserLogMessage events (from Jinja print() and log() functions)
        if log_record.attributes.is::<UserLogMessage>() {
            self.handle_user_log_message(log_record);
            return;
        }

        // Handle ListItemOutput events (from dbt list command)
        if let Some(list_item) = log_record.attributes.downcast_ref::<ListItemOutput>() {
            self.handle_list_item_output(log_record, list_item);
            return;
        }

        // Handle ShowDataOutput events (from dbt show command or run with --show data)
        if let Some(show_data) = log_record.attributes.downcast_ref::<ShowDataOutput>() {
            self.handle_show_data_output(log_record, show_data);
            return;
        }

        // Handle ShowResult events (verbose/diagnostic output controlled by --show flag)
        if let Some(show_result) = log_record.attributes.downcast_ref::<ShowResult>() {
            self.handle_show_result(log_record, show_result);
            return;
        }

        // Handle CompiledCode events (from rendered SQL in project nodes)
        if let Some(compiled_code) = log_record.attributes.downcast_ref::<CompiledCode>() {
            self.handle_compiled_code(log_record, compiled_code);
            return;
        }

        // Handle CompiledCodeInline events (from compile command with inline query)
        if let Some(compiled_code) = log_record.attributes.downcast_ref::<CompiledCodeInline>() {
            self.handle_compiled_code_inline(log_record, compiled_code);
        }
    }
}

impl FileLogLayer {
    fn handle_invocation_start(&self, span: &SpanStartInfo, invocation: &Invocation) {
        self.writer.writeln(&format_args!(
            "{} {} | {} {}",
            HEADER_SEPARATOR,
            UtcZuluTimestamp(span.start_time_unix_nano),
            invocation.invocation_id,
            HEADER_SEPARATOR
        ));
    }

    fn handle_query_executed(&self, span: &SpanEndInfo, query_data: &QueryExecuted) {
        let node_id = query_data.unique_id.as_deref().unwrap_or("unknown");
        self.write_log_line(
            span.end_time_unix_nano,
            span.severity_number,
            &format_args!("Query executed on node {}:\n{}", node_id, query_data.sql),
        );
    }

    fn handle_thread_pool_wait_start(&self, span: &SpanStartInfo) {
        self.write_log_line(
            span.start_time_unix_nano,
            span.severity_number,
            &THREAD_POOL_WAIT_START,
        );
    }

    fn handle_thread_pool_wait_end(&self, span: &SpanEndInfo) {
        self.write_log_line(
            span.end_time_unix_nano,
            span.severity_number,
            &ThreadPoolWaitEnd(span_duration(span)),
        );
    }

    fn handle_phase_executed_start(&self, span: &SpanStartInfo, phase: &PhaseExecuted) {
        self.write_log_line(
            span.start_time_unix_nano,
            span.severity_number,
            &PhaseExecutedStart(phase),
        );
    }

    fn handle_phase_executed_end(&self, span: &SpanEndInfo, phase: &PhaseExecuted) {
        self.write_log_line(
            span.end_time_unix_nano,
            span.severity_number,
            &PhaseExecutedEnd {
                phase,
                duration: span_duration(span),
            },
        );
    }

    fn handle_node_evaluated_start(&self, span: &SpanStartInfo, ne: &NodeEvaluated) {
        self.write_log_line(
            span.start_time_unix_nano,
            span.severity_number,
            &NodeEvaluatedStart {
                node: ne,
                colorize: false,
            },
        );
    }

    fn handle_node_evaluated(&self, span: &SpanEndInfo, ne: &NodeEvaluated) {
        self.write_log_line(
            span.end_time_unix_nano,
            span.severity_number,
            &NodeEvaluatedEnd {
                node: ne,
                duration: span_duration(span),
                colorize: false,
            },
        );
    }

    fn handle_microbatch_evaluated_start(&self, span: &SpanStartInfo, batch: &MicrobatchEvaluated) {
        self.write_log_line(
            span.start_time_unix_nano,
            span.severity_number,
            &MicrobatchEvaluatedStart {
                batch,
                colorize: false,
            },
        );
    }

    fn handle_microbatch_evaluated(&self, span: &SpanEndInfo, batch: &MicrobatchEvaluated) {
        let severity = if batch.node_outcome() == NodeOutcome::Error {
            SeverityNumber::Error
        } else {
            span.severity_number
        };
        self.write_log_line(
            span.end_time_unix_nano,
            severity,
            &MicrobatchEvaluatedEnd {
                batch,
                duration: span_duration(span),
                colorize: false,
            },
        );
    }

    fn handle_node_processed_start(&self, span: &SpanStartInfo, node: &NodeProcessed) {
        self.write_log_line(
            span.start_time_unix_nano,
            span.severity_number,
            &NodeProcessedStart {
                node,
                colorize: false,
            },
        );
    }

    fn handle_node_processed(&self, span: &SpanEndInfo, node: &NodeProcessed) {
        // Calculate duration: use accumulated duration_ms from NodeProcessed, which reflects actual
        // processing time across all phases (excluding time waiting for upstream nodes).
        // Fall back to span duration if duration_ms is not available.
        let duration = node
            .duration_ms
            .map(std::time::Duration::from_millis)
            .unwrap_or_else(|| span_duration(span));

        // Emit the node processed end line
        self.write_log_line(
            span.end_time_unix_nano,
            span.severity_number,
            &NodeProcessedEnd {
                node,
                duration,
                colorize: false,
            },
        );

        // Print unit test summary messages
        if (node.node_type() == NodeType::Test || node.node_type() == NodeType::UnitTest)
            && node.node_outcome() == NodeOutcome::Success
            && let Some(test_failure) = TestFailure::new(node, false)
        {
            self.write_log_line(span.end_time_unix_nano, span.severity_number, &test_failure);
        }
    }

    fn handle_invocation_end(
        &self,
        span: &SpanEndInfo,
        invocation: &Invocation,
        data_provider: &mut DataProvider<'_>,
    ) {
        let summary = InvocationSummary::new(span, invocation, data_provider, false, None);

        // Per pre-migration logic, autofix line were always printed ignoring show options
        if let Some(line) = summary.autofix_line() {
            self.write_log_line(span.end_time_unix_nano, span.severity_number, &line);
        }

        if let Some(summary_lines) = summary.summary_lines() {
            self.write_log_line(
                span.end_time_unix_nano,
                span.severity_number,
                &summary_lines,
            );
        }
    }

    fn handle_log_message(&self, log_msg: &LogMessage, log_record: &LogRecordInfo) {
        // Unfortunately, we do not currently enforce log body to not contain ANSI codes,
        // so we need to make sure to strip them
        let body = console::strip_ansi_codes(log_record.body.as_str());

        self.write_log_line(
            log_record.time_unix_nano,
            log_record.severity_number,
            &LogMessageLine {
                error_code: log_msg
                    .code
                    .and_then(|c| u16::try_from(c).ok())
                    .and_then(|c| ErrorCode::try_from(c).ok()),
                message: &body,
                severity: log_record.severity_number,
                colorize: false,
                include_level_prefix: false, // the level prefix is added by write_log_line
            },
        );
    }

    fn handle_user_log_message(&self, log_record: &LogRecordInfo) {
        // Write user log messages to file log
        self.write_log_line(
            log_record.time_unix_nano,
            log_record.severity_number,
            &log_record.body,
        );
    }

    fn handle_list_item_output(&self, log_record: &LogRecordInfo, list_item: &ListItemOutput) {
        // Emit header once before first list item
        if !self.list_header_emitted.swap(true, Ordering::Relaxed) {
            self.writer
                .writeln(&Delimiter::new(SELECTED_NODES_TITLE, None, false));
        }

        // Write list item content to file log
        self.write_log_line(
            log_record.time_unix_nano,
            log_record.severity_number,
            &list_item.content,
        );
    }

    fn handle_show_data_output(&self, log_record: &LogRecordInfo, show_data: &ShowDataOutput) {
        // Write preview to file log
        self.write_log_line(
            log_record.time_unix_nano,
            log_record.severity_number,
            &show_data.content,
        );
    }

    fn handle_show_result(&self, log_record: &LogRecordInfo, show_result: &ShowResult) {
        // Write title and content to file log (without color codes for file output)
        self.write_log_line(
            log_record.time_unix_nano,
            log_record.severity_number,
            &show_result.title,
        );
        self.writer
            .writeln(&console::strip_ansi_codes(show_result.content.as_str()));
    }

    fn handle_compiled_code_inline(
        &self,
        log_record: &LogRecordInfo,
        compiled_code: &CompiledCodeInline,
    ) {
        self.write_log_line(
            log_record.time_unix_nano,
            log_record.severity_number,
            &CompiledInlineCode {
                compiled_code,
                colorize: false,
            },
        );
    }

    fn handle_compiled_code(&self, log_record: &LogRecordInfo, compiled_code: &CompiledCode) {
        self.write_log_line(
            log_record.time_unix_nano,
            log_record.severity_number,
            &CompiledCodeLine {
                compiled_code,
                colorize: false,
            },
        );
    }

    fn handle_progress_message(&self, log_record: &LogRecordInfo, progress_msg: &ProgressMessage) {
        self.write_log_line(
            log_record.time_unix_nano,
            log_record.severity_number,
            &ProgressMessageLine {
                progress: progress_msg,
                severity: log_record.severity_number,
                pad_action: false,
                colorize: false,
            },
        );
    }

    fn handle_asset_parsed_end(&self, span: &SpanEndInfo, asset: &AssetParsed) {
        self.write_log_line(
            span.end_time_unix_nano,
            span.severity_number,
            &AssetParsedEnd {
                asset,
                duration: span_duration(span),
                colorize: false,
            },
        );
    }

    fn handle_state_modified_diff(
        &self,
        log_record: &LogRecordInfo,
        state_mod_diff: &StateModifiedDiff,
    ) {
        self.write_log_line(
            log_record.time_unix_nano,
            log_record.severity_number,
            &StateModifiedDiffLines(state_mod_diff),
        );
    }

    fn handle_deps_all_packages_installing_start(
        &self,
        span: &SpanStartInfo,
        ev: &DepsAllPackagesInstalled,
    ) {
        self.write_log_line(
            span.start_time_unix_nano,
            span.severity_number,
            &PackageInstallStart {
                event: ev,
                colorize: false,
            },
        );
    }

    fn handle_deps_all_packages_installing_end(
        &self,
        span: &SpanEndInfo,
        ev: &DepsAllPackagesInstalled,
    ) {
        self.write_log_line(
            span.end_time_unix_nano,
            span.severity_number,
            &PackageInstallEnd {
                event: ev,
                colorize: false,
            },
        );
    }

    fn handle_dep_installed_start(&self, span: &SpanStartInfo, pkg: &DepsPackageInstalled) {
        self.write_log_line(
            span.start_time_unix_nano,
            span.severity_number,
            &PackageInstalledStart {
                pkg,
                colorize: false,
            },
        );
    }

    fn handle_dep_installed_end(&self, span: &SpanEndInfo, pkg: &DepsPackageInstalled) {
        self.write_log_line(
            span.end_time_unix_nano,
            span.severity_number,
            &PackageInstalledEnd {
                pkg,
                status: span.status.as_ref().map_or(StatusCode::Unset, |s| s.code),
                colorize: false,
            },
        );
    }

    fn handle_package_add_start(&self, span: &SpanStartInfo, pkg: &DepsAddPackage) {
        self.write_log_line(
            span.start_time_unix_nano,
            span.severity_number,
            &PackageAddStart {
                pkg,
                colorize: false,
            },
        );
    }

    fn handle_package_add_end(&self, span: &SpanEndInfo, pkg: &DepsAddPackage) {
        self.write_log_line(
            span.end_time_unix_nano,
            span.severity_number,
            &PackageAddEnd {
                pkg,
                status: span.status.as_ref().map_or(StatusCode::Unset, |s| s.code),
                colorize: false,
            },
        );
    }

    fn handle_generic_op_start(&self, span: &SpanStartInfo, op: &GenericOpExecuted) {
        self.write_log_line(
            span.start_time_unix_nano,
            span.severity_number,
            &GenericOpStart {
                op,
                colorize: false,
            },
        );
    }

    fn handle_generic_op_end(&self, span: &SpanEndInfo, op: &GenericOpExecuted) {
        self.write_log_line(
            span.end_time_unix_nano,
            span.severity_number,
            &GenericOpEnd {
                op,
                duration: span_duration(span),
                status: span.status.as_ref(),
                colorize: false,
            },
        );
    }

    fn handle_generic_op_item_start(&self, span: &SpanStartInfo, item: &GenericOpItemProcessed) {
        self.write_log_line(
            span.start_time_unix_nano,
            span.severity_number,
            &GenericOpItemStart(item),
        );
    }

    fn handle_generic_op_item_end(&self, span: &SpanEndInfo, item: &GenericOpItemProcessed) {
        self.write_log_line(
            span.end_time_unix_nano,
            span.severity_number,
            &GenericOpItemEnd {
                item,
                duration: span_duration(span),
                status: span.status.as_ref(),
                colorize: false,
            },
        );
    }
}

/// Wall time of a span.
fn span_duration(span: &SpanEndInfo) -> std::time::Duration {
    span.end_time_unix_nano
        .duration_since(span.start_time_unix_nano)
        .unwrap_or_default()
}
