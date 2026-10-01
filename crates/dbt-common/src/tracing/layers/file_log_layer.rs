use dbt_error::ErrorCode;
use dbt_telemetry::{
    AssetParsed, CompiledCode, CompiledCodeInline, ConnectionLimitWait, DepsAddPackage,
    DepsAllPackagesInstalled, DepsPackageInstalled, GenericOpExecuted, GenericOpItemProcessed,
    Invocation, ListItemOutput, LogMessage, NodeEvaluated, NodeOutcome, NodeProcessed, NodeType,
    PhaseExecuted, ProgressMessage, QueryExecuted, ShowDataOutput, ShowResult, StateModifiedDiff,
    UserLogMessage,
};
use std::{
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
        asset::format_asset_parsed_end,
        connection_limit_wait::{
            format_connection_limit_wait_end, format_connection_limit_wait_start,
        },
        constants::SELECTED_NODES_TITLE,
        deps::{
            format_package_add_end, format_package_add_start, format_package_install_end,
            format_package_install_start, format_package_installed_end,
            format_package_installed_start,
        },
        duration::{format_timestamp_time_only, format_timestamp_utc_zulu},
        generic::{
            format_generic_op_end, format_generic_op_item_end, format_generic_op_item_start,
            format_generic_op_start,
        },
        invocation::format_invocation_summary,
        layout::format_delimiter,
        log_message::format_log_message,
        meta::format_severity_fixed_width,
        node::{
            format_compiled_code, format_compiled_inline_code, format_node_evaluated_end,
            format_node_evaluated_start, format_node_processed_end, format_node_processed_start,
        },
        phase::{format_phase_executed_end, format_phase_executed_start},
        progress::format_progress_message,
        state_mod_diff::format_state_modified_diff_lines,
        test_result::format_test_failure,
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

    /// Write log lines with timestamp and severity level prefix.
    /// Each string in the slice represents one distinct log line.
    fn write_log_lines(
        &self,
        timestamp: SystemTime,
        severity: SeverityNumber,
        lines: &[impl AsRef<str>],
    ) {
        let timestamp_str = format_timestamp_time_only(timestamp);
        let level_str = format_severity_fixed_width(severity);

        let mut line_iter = lines.iter();

        // First line with timestamp and level
        if let Some(first_line) = line_iter.next() {
            let formatted_line =
                format!("{} {}: {}", timestamp_str, level_str, first_line.as_ref());
            self.writer.writeln(&formatted_line);
        }

        // Subsequent lines are printed as is, without timestamp/level prefix
        for line in line_iter {
            self.writer.writeln(line.as_ref());
        }
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

        // Handle NodeProcessed start
        if let Some(node_processed) = span.attributes.downcast_ref::<NodeProcessed>() {
            self.handle_node_processed_start(span, node_processed);
            return;
        }

        if let Some(wait) = span.attributes.downcast_ref::<ConnectionLimitWait>() {
            self.handle_connection_limit_wait_start(span, wait);
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

        if let Some(wait) = span.attributes.downcast_ref::<ConnectionLimitWait>() {
            self.handle_connection_limit_wait_end(span, wait);
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
        let timestamp = format_timestamp_utc_zulu(span.start_time_unix_nano);
        let invocation_id = &invocation.invocation_id;
        let header = format!(
            "{} {} | {} {}",
            HEADER_SEPARATOR, timestamp, invocation_id, HEADER_SEPARATOR
        );
        self.writer.writeln(&header);
    }

    fn handle_query_executed(&self, span: &SpanEndInfo, query_data: &QueryExecuted) {
        let node_id = query_data.unique_id.as_deref().unwrap_or("unknown");
        let formatted_query = format!("Query executed on node {}:\n{}", node_id, query_data.sql);
        self.write_log_lines(
            span.end_time_unix_nano,
            span.severity_number,
            &[formatted_query],
        );
    }

    fn handle_connection_limit_wait_start(&self, span: &SpanStartInfo, wait: &ConnectionLimitWait) {
        let formatted = format_connection_limit_wait_start(wait);
        self.write_log_lines(
            span.start_time_unix_nano,
            span.severity_number,
            &[formatted],
        );
    }

    fn handle_connection_limit_wait_end(&self, span: &SpanEndInfo, wait: &ConnectionLimitWait) {
        let duration = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();
        let formatted = format_connection_limit_wait_end(wait, duration);
        self.write_log_lines(span.end_time_unix_nano, span.severity_number, &[formatted]);
    }

    fn handle_phase_executed_start(&self, span: &SpanStartInfo, phase: &PhaseExecuted) {
        let formatted = format_phase_executed_start(phase);
        self.write_log_lines(
            span.start_time_unix_nano,
            span.severity_number,
            &[formatted],
        );
    }

    fn handle_phase_executed_end(&self, span: &SpanEndInfo, phase: &PhaseExecuted) {
        let duration = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();
        let formatted = format_phase_executed_end(phase, duration);
        self.write_log_lines(span.end_time_unix_nano, span.severity_number, &[formatted]);
    }

    fn handle_node_evaluated_start(&self, span: &SpanStartInfo, ne: &NodeEvaluated) {
        let formatted = format_node_evaluated_start(ne, false);
        self.write_log_lines(
            span.start_time_unix_nano,
            span.severity_number,
            &[formatted],
        );
    }

    fn handle_node_evaluated(&self, span: &SpanEndInfo, ne: &NodeEvaluated) {
        let duration = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();
        let formatted = format_node_evaluated_end(ne, duration, false);
        self.write_log_lines(span.end_time_unix_nano, span.severity_number, &[formatted]);
    }

    fn handle_node_processed_start(&self, span: &SpanStartInfo, node: &NodeProcessed) {
        let formatted = format_node_processed_start(node, false);
        self.write_log_lines(
            span.start_time_unix_nano,
            span.severity_number,
            &[formatted],
        );
    }

    fn handle_node_processed(&self, span: &SpanEndInfo, node: &NodeProcessed) {
        // Calculate duration: use accumulated duration_ms from NodeProcessed, which reflects actual
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

        // Format and emit the node processed end line
        let formatted = format_node_processed_end(node, duration, false);
        self.write_log_lines(span.end_time_unix_nano, span.severity_number, &[formatted]);

        // Print unit test summary messages
        if (node.node_type() == NodeType::Test || node.node_type() == NodeType::UnitTest)
            && node.node_outcome() == NodeOutcome::Success
            && let Some(test_failure_message) = format_test_failure(node, false)
        {
            self.write_log_lines(
                span.end_time_unix_nano,
                span.severity_number,
                &[test_failure_message],
            );
        }
    }

    fn handle_invocation_end(
        &self,
        span: &SpanEndInfo,
        invocation: &Invocation,
        data_provider: &mut DataProvider<'_>,
    ) {
        let formatted = format_invocation_summary(span, invocation, data_provider, false, None);

        // Per pre-migration logic, autofix line were always printed ignoring show options
        if let Some(line) = formatted.autofix_line() {
            self.write_log_lines(span.end_time_unix_nano, span.severity_number, &[line]);
        }

        if let Some(summary_lines) = formatted.summary_lines() {
            self.write_log_lines(span.end_time_unix_nano, span.severity_number, summary_lines);
        }
    }

    fn handle_log_message(&self, log_msg: &LogMessage, log_record: &LogRecordInfo) {
        // Format the message without level prefix (we add it via write_log_lines)
        let formatted_message = format_log_message(
            log_msg
                .code
                .and_then(|c| u16::try_from(c).ok())
                .and_then(|c| ErrorCode::try_from(c).ok()),
            // Unfortunately, we do not currently enforce log body to not contain ANSI codes,
            // so we need to make sure to strip them
            console::strip_ansi_codes(log_record.body.as_str()),
            log_record.severity_number,
            false,
            false, // Don't include level prefix, we add it via write_log_lines
        );

        self.write_log_lines(
            log_record.time_unix_nano,
            log_record.severity_number,
            &[formatted_message],
        );
    }

    fn handle_user_log_message(&self, log_record: &LogRecordInfo) {
        // Write user log messages to file log
        self.write_log_lines(
            log_record.time_unix_nano,
            log_record.severity_number,
            std::slice::from_ref(&log_record.body),
        );
    }

    fn handle_list_item_output(&self, log_record: &LogRecordInfo, list_item: &ListItemOutput) {
        // Emit header once before first list item
        if !self.list_header_emitted.swap(true, Ordering::Relaxed) {
            let header = format_delimiter(SELECTED_NODES_TITLE, None, false);
            self.writer.writeln(&header);
        }

        // Write list item content to file log
        self.write_log_lines(
            log_record.time_unix_nano,
            log_record.severity_number,
            std::slice::from_ref(&list_item.content),
        );
    }

    fn handle_show_data_output(&self, log_record: &LogRecordInfo, show_data: &ShowDataOutput) {
        // Write preview to file log
        self.write_log_lines(
            log_record.time_unix_nano,
            log_record.severity_number,
            std::slice::from_ref(&show_data.content),
        );
    }

    fn handle_show_result(&self, log_record: &LogRecordInfo, show_result: &ShowResult) {
        // Write title and content to file log (without color codes for file output)
        let lines = vec![
            show_result.title.clone(),
            console::strip_ansi_codes(show_result.content.as_str()).to_string(),
        ];
        self.write_log_lines(
            log_record.time_unix_nano,
            log_record.severity_number,
            &lines,
        );
    }

    fn handle_compiled_code_inline(
        &self,
        log_record: &LogRecordInfo,
        compiled_code: &CompiledCodeInline,
    ) {
        // Write formatted compiled code to file log using the shared formatter
        let formatted = format_compiled_inline_code(compiled_code, false);
        self.write_log_lines(
            log_record.time_unix_nano,
            log_record.severity_number,
            &[formatted],
        );
    }

    fn handle_compiled_code(&self, log_record: &LogRecordInfo, compiled_code: &CompiledCode) {
        let formatted = format_compiled_code(compiled_code, false);
        self.write_log_lines(
            log_record.time_unix_nano,
            log_record.severity_number,
            &[formatted],
        );
    }

    fn handle_progress_message(&self, log_record: &LogRecordInfo, progress_msg: &ProgressMessage) {
        let formatted =
            format_progress_message(progress_msg, log_record.severity_number, false, false);
        self.write_log_lines(
            log_record.time_unix_nano,
            log_record.severity_number,
            &[formatted],
        );
    }

    fn handle_asset_parsed_end(&self, span: &SpanEndInfo, asset: &AssetParsed) {
        let duration = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();
        let formatted = format_asset_parsed_end(asset, duration, false);
        self.write_log_lines(span.end_time_unix_nano, span.severity_number, &[formatted]);
    }

    fn handle_state_modified_diff(
        &self,
        log_record: &LogRecordInfo,
        state_mod_diff: &StateModifiedDiff,
    ) {
        let lines = format_state_modified_diff_lines(state_mod_diff);
        self.write_log_lines(
            log_record.time_unix_nano,
            log_record.severity_number,
            &lines,
        );
    }

    fn handle_deps_all_packages_installing_start(
        &self,
        span: &SpanStartInfo,
        ev: &DepsAllPackagesInstalled,
    ) {
        // Format with shared formatter
        let formatted_message = format_package_install_start(ev, false);

        self.write_log_lines(
            span.start_time_unix_nano,
            span.severity_number,
            &[formatted_message],
        );
    }

    fn handle_deps_all_packages_installing_end(
        &self,
        span: &SpanEndInfo,
        ev: &DepsAllPackagesInstalled,
    ) {
        // Format with shared formatter
        let formatted_message = format_package_install_end(ev, false);

        self.write_log_lines(
            span.end_time_unix_nano,
            span.severity_number,
            &[formatted_message],
        );
    }

    fn handle_dep_installed_start(&self, span: &SpanStartInfo, pkg: &DepsPackageInstalled) {
        // Format with shared formatter
        let formatted_message = format_package_installed_start(pkg, false);

        self.write_log_lines(
            span.start_time_unix_nano,
            span.severity_number,
            &[formatted_message],
        );
    }

    fn handle_dep_installed_end(&self, span: &SpanEndInfo, pkg: &DepsPackageInstalled) {
        // Format with shared formatter
        let formatted_message = format_package_installed_end(
            pkg,
            span.status.as_ref().map_or(StatusCode::Unset, |s| s.code),
            false,
        );

        self.write_log_lines(
            span.end_time_unix_nano,
            span.severity_number,
            &[formatted_message],
        );
    }

    fn handle_package_add_start(&self, span: &SpanStartInfo, pkg: &DepsAddPackage) {
        // Format with shared formatter
        let formatted_message = format_package_add_start(pkg, false);

        self.write_log_lines(
            span.start_time_unix_nano,
            span.severity_number,
            &[formatted_message],
        );
    }

    fn handle_package_add_end(&self, span: &SpanEndInfo, pkg: &DepsAddPackage) {
        // Format with shared formatter
        let formatted_message = format_package_add_end(
            pkg,
            span.status.as_ref().map_or(StatusCode::Unset, |s| s.code),
            false,
        );

        self.write_log_lines(
            span.end_time_unix_nano,
            span.severity_number,
            &[formatted_message],
        );
    }

    fn handle_generic_op_start(&self, span: &SpanStartInfo, op: &GenericOpExecuted) {
        let formatted = format_generic_op_start(op, false);
        self.write_log_lines(
            span.start_time_unix_nano,
            span.severity_number,
            &[formatted],
        );
    }

    fn handle_generic_op_end(&self, span: &SpanEndInfo, op: &GenericOpExecuted) {
        let duration = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();
        let formatted = format_generic_op_end(op, duration, span.status.as_ref(), false);
        self.write_log_lines(span.end_time_unix_nano, span.severity_number, &[formatted]);
    }

    fn handle_generic_op_item_start(&self, span: &SpanStartInfo, item: &GenericOpItemProcessed) {
        let formatted = format_generic_op_item_start(item);
        self.write_log_lines(
            span.start_time_unix_nano,
            span.severity_number,
            &[formatted],
        );
    }

    fn handle_generic_op_item_end(&self, span: &SpanEndInfo, item: &GenericOpItemProcessed) {
        let duration = span
            .end_time_unix_nano
            .duration_since(span.start_time_unix_nano)
            .unwrap_or_default();
        let formatted = format_generic_op_item_end(item, duration, span.status.as_ref(), false);
        self.write_log_lines(span.end_time_unix_nano, span.severity_number, &[formatted]);
    }
}
