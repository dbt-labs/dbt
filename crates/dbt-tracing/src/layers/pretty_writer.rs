use crate::{
    LogRecordInfo, SpanEndInfo, SpanStartInfo, TelemetryOutputFlags, TelemetryRecordRef,
    data_provider::DataProvider, layer::TelemetryConsumer, shared_writer::SharedWriter,
};

pub type TelemetryRecordPrettyFormatter =
    Box<dyn Fn(TelemetryRecordRef, bool) -> Option<String> + Send + Sync>;

/// A tracing layer that renders telemetry events in a human-readable format.
///
/// The layer respects [`TelemetryOutputFlags`] to decide whether a record should be written and
/// relies on [`TelemetryRecordPrettyFormatter`] for event-specific formatting.
/// It is intended for simple console or log-file style sinks.
pub struct TelemetryPrettyWriterLayer {
    writer: Box<dyn SharedWriter>,
    formatter: TelemetryRecordPrettyFormatter,
    is_tty: bool,
    filter_flag: TelemetryOutputFlags,
}

impl TelemetryPrettyWriterLayer {
    pub fn new<W, F>(writer: W, formatter: F) -> Self
    where
        W: SharedWriter + 'static,
        F: Fn(TelemetryRecordRef, bool) -> Option<String> + Send + Sync + 'static,
    {
        let is_tty = writer.is_terminal();

        Self {
            writer: Box::new(writer),
            formatter: Box::new(formatter),
            is_tty,
            filter_flag: if is_tty {
                TelemetryOutputFlags::OUTPUT_CONSOLE
            } else {
                TelemetryOutputFlags::OUTPUT_LOG_FILE
            },
        }
    }
}

impl TelemetryConsumer for TelemetryPrettyWriterLayer {
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool {
        span.attributes.output_flags().contains(self.filter_flag)
    }

    fn is_log_enabled(&self, log_record: &LogRecordInfo) -> bool {
        log_record
            .attributes
            .output_flags()
            .contains(self.filter_flag)
    }

    fn on_span_start(&self, span: &SpanStartInfo, _: &mut DataProvider<'_>) {
        if let Some(line) = (self.formatter)(TelemetryRecordRef::SpanStart(span), self.is_tty) {
            self.writer.writeln(&line);
        }
    }

    fn on_span_end(&self, span: &SpanEndInfo, _: &mut DataProvider<'_>) {
        if let Some(line) = (self.formatter)(TelemetryRecordRef::SpanEnd(span), self.is_tty) {
            self.writer.writeln(&line);
        }
    }

    fn on_log_record(&self, record: &LogRecordInfo, _: &mut DataProvider<'_>) {
        if let Some(line) = (self.formatter)(TelemetryRecordRef::LogRecord(record), self.is_tty) {
            self.writer.writeln(&line);
        }
    }
}
