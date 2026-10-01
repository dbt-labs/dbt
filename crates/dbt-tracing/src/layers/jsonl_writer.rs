use std::borrow::Cow;

use crate::{
    LogRecordInfo, SpanEndInfo, SpanStartInfo, TelemetryOutputFlags, TelemetryRecordRef,
    background_writer::BackgroundWriter,
    data_provider::DataProvider,
    layer::{ConsumerLayer, LogPreprocessorHook, TelemetryConsumer},
    shared_writer::SharedWriter,
    shutdown::TelemetryShutdownItem,
};
use tracing::level_filters::LevelFilter;

/// Build jsonl layer for arbitrary writer. This will writer directly to
/// the writer. If you want to write to slow IO sink, prefer `build_jsonl_layer_with_background_writer`
pub fn build_jsonl_layer<W: SharedWriter + 'static>(
    writer: W,
    max_log_verbosity: LevelFilter,
    log_preprocessor_hook: Option<LogPreprocessorHook>,
) -> ConsumerLayer {
    Box::new(
        TelemetryJsonlWriterLayer::new(writer, log_preprocessor_hook)
            .with_filter(max_log_verbosity),
    )
}

/// Build jsonl layer with a background writer. This is preferred for writing to
/// slow IO sinks like files.
pub fn build_jsonl_layer_with_background_writer<W: std::io::Write + Send + 'static>(
    writer: W,
    max_log_verbosity: LevelFilter,
    log_preprocessor_hook: Option<LogPreprocessorHook>,
) -> (ConsumerLayer, TelemetryShutdownItem) {
    let (writer, handle) = BackgroundWriter::new(writer);

    (
        build_jsonl_layer(writer, max_log_verbosity, log_preprocessor_hook),
        Box::new(handle),
    )
}

/// A tracing layer that reads telemetry data from extensions and writes it as JSON.
///
/// This layer reads TelemetryRecord data from span extensions and serializes
/// it to JSON using the provided writer.
pub struct TelemetryJsonlWriterLayer {
    writer: Box<dyn SharedWriter>,
    log_preprocessor_hook: Option<LogPreprocessorHook>,
}

impl TelemetryJsonlWriterLayer {
    pub fn new<W: SharedWriter + 'static>(
        writer: W,
        log_preprocessor_hook: Option<LogPreprocessorHook>,
    ) -> Self {
        Self {
            writer: Box::new(writer),
            log_preprocessor_hook,
        }
    }
}

impl TelemetryConsumer for TelemetryJsonlWriterLayer {
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool {
        span.attributes
            .output_flags()
            .contains(TelemetryOutputFlags::EXPORT_JSONL)
    }

    fn is_log_enabled(&self, log_record: &LogRecordInfo) -> bool {
        log_record
            .attributes
            .output_flags()
            .contains(TelemetryOutputFlags::EXPORT_JSONL)
    }

    fn on_span_start(&self, span: &SpanStartInfo, _: &mut DataProvider<'_>) {
        if let Ok(json) = serde_json::to_string(&TelemetryRecordRef::SpanStart(span)) {
            self.writer.writeln(json.as_str());
        }
    }

    fn on_span_end(&self, span: &SpanEndInfo, _: &mut DataProvider<'_>) {
        if let Ok(json) = serde_json::to_string(&TelemetryRecordRef::SpanEnd(span)) {
            self.writer.writeln(json.as_str());
        }
    }

    fn on_log_record(&self, record: &LogRecordInfo, _: &mut DataProvider<'_>) {
        let record = self
            .log_preprocessor_hook
            .map_or(Cow::Borrowed(record), |hook| hook(record));
        if let Ok(json) = serde_json::to_string(&TelemetryRecordRef::LogRecord(record.as_ref())) {
            self.writer.writeln(json.as_str());
        }
    }
}
