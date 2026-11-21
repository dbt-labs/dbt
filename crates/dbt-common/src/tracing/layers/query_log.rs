use std::io::Write;

use dbt_telemetry::QueryExecuted;
use dbt_tracing::{SpanStartInfo, TelemetryRecordRef};

use super::super::formatters::query_log::QueryLogEntry;
use dbt_tracing::{
    background_writer::BackgroundWriter,
    filter::{TelemetryFilterFn, disable_all_logs},
    layer::{ConsumerLayer, TelemetryConsumer},
    layers::pretty_writer::TelemetryPrettyWriterLayer,
    shared_writer::SharedWriter,
    shutdown::TelemetryShutdownItem,
};

/// Writes the query log entry of a finished query
fn write_query_log_record(record: TelemetryRecordRef, _is_tty: bool, writer: &dyn SharedWriter) {
    if let TelemetryRecordRef::SpanEnd(span_end) = record
        && let Some(query_data) = span_end.attributes.downcast_ref::<QueryExecuted>()
    {
        writer.writeln(&QueryLogEntry {
            query: query_data,
            start_time: span_end.start_time_unix_nano,
            end_time: span_end.end_time_unix_nano,
        });
    }
}

/// Build a query log writing layer with a background writer. Do not wrap
/// or buffer the writer, as the layer already does its own buffering
/// and operates on a non-blocking worker thread.
pub fn build_query_log_layer_with_background_writer<W: Write + Send + 'static>(
    writer: W,
) -> (ConsumerLayer, TelemetryShutdownItem) {
    let (writer, handle) = BackgroundWriter::new(writer);

    let layer = TelemetryPrettyWriterLayer::new(writer, write_query_log_record).with_filter(
        TelemetryFilterFn::new(
            |span: &SpanStartInfo| span.attributes.is::<QueryExecuted>(),
            disable_all_logs,
        ),
    );

    (Box::new(layer), Box::new(handle))
}
