use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use crate::{
    LogRecordInfo, SpanEndInfo, SpanStartInfo, TelemetryOutputFlags, TelemetryRecord,
    data_provider::DataProvider,
    error::{TracingError, TracingResult},
    layer::{ConsumerLayer, TelemetryConsumer},
    serialize::arrow::{TelemetryArrowSchemas, serialize_to_arrow},
    serialize::traits::ArrowRegistryLookup,
    shutdown::{TelemetryShutdown, TelemetryShutdownItem},
};
use parquet::{arrow::ArrowWriter, basic::Compression, file::properties::WriterProperties};

/// Build a parquet writer layer with a background writer. Do not wrap
/// or buffer the writer, as the layer already does its own buffering
/// and operates on a non-blocking worker thread.
pub fn build_parquet_writer_layer<W, R>(
    writer: W,
) -> TracingResult<(ConsumerLayer, TelemetryShutdownItem)>
where
    W: Write + Send + 'static,
    R: ArrowRegistryLookup,
{
    let (parquet_layer, handle) = TelemetryParquetWriterLayer::new::<W, R>(writer)?;

    Ok((Box::new(parquet_layer), Box::new(handle)))
}

/// Buffer size for parquet record batching. This is the buffer in our part of the code
/// used to reduce the number of rust struct -> RecordBatch conversions.
/// The ArrowWriter itself also has an internal buffer. See the memory limit const below.
const PARQUET_WRITER_BUF_SIZE: usize = 1024;

/// Maximum memory usage for the ArrowWriter internal buffer.
const PARQUET_WRITER_MEMORY_LIMIT: usize = 128 * 1024 * 1024; // 128 MB

impl<W> ParquetWriter<W>
where
    W: Write + Send + 'static,
{
    fn new<R>(writer: W) -> TracingResult<Self>
    where
        R: ArrowRegistryLookup,
    {
        let writer_properties = WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .build();
        let arrow_schemas = TelemetryArrowSchemas::new::<R>();

        let parquet_writer = ArrowWriter::try_new(
            writer,
            arrow::datatypes::Schema::new(arrow_schemas.schema_with_timestamps().to_vec()).into(),
            Some(writer_properties),
        )
        .map_err(|e| TracingError::io(format!("Failed to create Parquet writer: {}", e)))?;

        Ok(Self {
            buffer: Vec::with_capacity(PARQUET_WRITER_BUF_SIZE),
            parquet_writer: Some(parquet_writer),
            arrow_schemas,
        })
    }

    fn write_record(&mut self, record: TelemetryRecord) -> TracingResult<()> {
        // Write batch if buffer is full
        if self.buffer.len() >= PARQUET_WRITER_BUF_SIZE {
            self.flush_batch()?;
        }

        // Add to buffer
        self.buffer.push(record);

        Ok(())
    }

    fn flush_batch(&mut self) -> TracingResult<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        // Serialize records to Arrow RecordBatch
        let record_batch = serialize_to_arrow(&self.buffer, &self.arrow_schemas)
            .map_err(|e| TracingError::io(format!("Failed to serialize to Arrow: {}", e)))?;

        // Write the batch
        let Some(ref mut writer) = self.parquet_writer else {
            // Should not be possible, since we ensure that parquet_writer is Some in new()
            // and we only take it in finalize() after flushing
            unreachable!("Parquet writer is not initialized");
        };

        writer
            .write(&record_batch)
            .map_err(|e| TracingError::io(format!("Failed to write Parquet batch: {}", e)))?;

        // Flush if we are over memory limit
        if writer.memory_size() >= PARQUET_WRITER_MEMORY_LIMIT {
            writer
                .flush()
                .map_err(|e| TracingError::io(format!("Failed to flush Parquet writer: {}", e)))?;
        }

        // Clear buffer for reuse (truncate avoids reallocation)
        self.buffer.truncate(0);

        Ok(())
    }

    fn finalize(&mut self) -> TracingResult<()> {
        // Flush any remaining records
        self.flush_batch()?;

        // Close the parquet writer
        if let Some(writer) = self.parquet_writer.take() {
            writer
                .close()
                .map_err(|e| TracingError::io(format!("Failed to close Parquet writer: {}", e)))?;
        }

        Ok(())
    }
}

/// A tracing layer that batches telemetry data and writes it as Parquet files.
///
/// This layer collects telemetry records in batches and writes them to Parquet
/// format using Arrow serialization in a separate worker thread. It writes records whose
/// attributes are enabled for Parquet export.
pub struct TelemetryParquetWriterLayer {
    sender: mpsc::Sender<ParquetMessage>,
    /// Flag used to avoid repeated error messages in case of early shutdown
    /// due to panics in writer thread (e.g. disk full)
    shutdown_flag: Arc<AtomicBool>,
}

/// Messages sent to the parquet writer thread
enum ParquetMessage {
    Write(Box<TelemetryRecord>),
    Shutdown,
}

/// Internal parquet writer that handles batching and file operations
struct ParquetWriter<W>
where
    W: Write + Send + 'static,
{
    buffer: Vec<TelemetryRecord>,
    parquet_writer: Option<ArrowWriter<W>>,
    arrow_schemas: TelemetryArrowSchemas,
}

impl TelemetryParquetWriterLayer {
    pub fn new<W, R>(writer: W) -> TracingResult<(Self, TelemetryParquetWriterHandle)>
    where
        W: Write + Send + 'static,
        R: ArrowRegistryLookup,
    {
        let (sender, receiver) = mpsc::channel::<ParquetMessage>();
        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let shutdown_flag_clone = shutdown_flag.clone();
        let shutdown_err = Arc::new(Mutex::new(None));
        let shutdown_err_clone = shutdown_err.clone();

        let mut parquet_writer = ParquetWriter::new::<R>(writer)?;

        let writer_thread = thread::spawn(move || {
            while let Ok(message) = receiver.recv() {
                match message {
                    ParquetMessage::Write(record) => {
                        if let Err(e) = parquet_writer.write_record(*record) {
                            // Save the error for later reporting
                            let mut err_lock = shutdown_err_clone.lock().expect("Mutex poisoned");
                            *err_lock = Some(io::Error::other(e.to_string()));

                            // Avoid further attempts to write, assume fatal
                            break;
                        }
                    }
                    ParquetMessage::Shutdown => {
                        // Process any remaining messages in the channel
                        while let Ok(ParquetMessage::Write(record)) = receiver.try_recv() {
                            if let Err(e) = parquet_writer.write_record(*record) {
                                // Save the error for later reporting
                                let mut err_lock =
                                    shutdown_err_clone.lock().expect("Mutex poisoned");
                                *err_lock = Some(io::Error::other(e.to_string()));

                                // Avoid further attempts to write, but do not break out of outer loop yet
                                // we may still be able to finalize
                                break;
                            }
                        }

                        // Finalize and close the parquet writer
                        if let Err(e) = parquet_writer.finalize() {
                            // Save the error for later reporting
                            let mut err_lock = shutdown_err_clone.lock().expect("Mutex poisoned");
                            *err_lock = Some(io::Error::other(e.to_string()));
                        }

                        break;
                    }
                }
            }

            // Mark shutdown complete for whatever reason we exited the loop
            shutdown_flag_clone.store(true, Ordering::Release);
        });

        let layer = Self {
            sender: sender.clone(),
            shutdown_flag: shutdown_flag.clone(),
        };

        let handle = TelemetryParquetWriterHandle {
            sender,
            writer_thread: Some(writer_thread),
            shutdown_flag,
            shutdown_err,
        };

        Ok((layer, handle))
    }

    /// Send a telemetry record to be written
    pub fn write_record(&self, record: TelemetryRecord) -> TracingResult<()> {
        if self.shutdown_flag.load(Ordering::Acquire) {
            // Writer thread has shut down
            return Err(TracingError::io(
                "Attempt to write to telemetry parquet writer after shutdown",
            ));
        }

        self.sender
            .send(ParquetMessage::Write(Box::new(record)))
            .map_err(|_| {
                // Channel is disconnected, mark as shut down
                self.shutdown_flag.store(true, Ordering::Release);
                TracingError::channel_closed(
                    "Telemetry parquet writer thread has terminated unexpectedly",
                )
            })
    }
}

impl TelemetryConsumer for TelemetryParquetWriterLayer {
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool {
        span.attributes
            .output_flags()
            .contains(TelemetryOutputFlags::EXPORT_PARQUET)
    }

    fn is_log_enabled(&self, log_record: &LogRecordInfo) -> bool {
        log_record
            .attributes
            .output_flags()
            .contains(TelemetryOutputFlags::EXPORT_PARQUET)
    }

    fn on_span_start(&self, span: &SpanStartInfo, _: &mut DataProvider<'_>) {
        let telemetry_record = TelemetryRecord::SpanStart(span.clone());

        // Errors are stored internally and reported during shutdown.
        // If the writer has already failed, this will return an error
        // but we can safely ignore it as the failure will be reported on shutdown.
        self.write_record(telemetry_record).ok();
    }

    fn on_span_end(&self, span: &SpanEndInfo, _: &mut DataProvider<'_>) {
        let telemetry_record = TelemetryRecord::SpanEnd(span.clone());

        // Errors are stored internally and reported during shutdown.
        // If the writer has already failed, this will return an error
        // but we can safely ignore it as the failure will be reported on shutdown.
        self.write_record(telemetry_record).ok();
    }

    fn on_log_record(&self, record: &LogRecordInfo, _: &mut DataProvider<'_>) {
        let telemetry_record = TelemetryRecord::LogRecord(record.clone());

        // Errors are stored internally and reported during shutdown.
        // If the writer has already failed, this will return an error
        // but we can safely ignore it as the failure will be reported on shutdown.
        self.write_record(telemetry_record).ok();
    }
}

/// Handle for shutdown handling
pub struct TelemetryParquetWriterHandle {
    sender: mpsc::Sender<ParquetMessage>,
    writer_thread: Option<JoinHandle<()>>,
    shutdown_flag: Arc<AtomicBool>,
    shutdown_err: Arc<Mutex<Option<io::Error>>>,
}

impl TelemetryShutdown for TelemetryParquetWriterHandle {
    fn shutdown(&mut self) -> TracingResult<()> {
        if !self.shutdown_flag.swap(true, Ordering::AcqRel) {
            // Send shutdown message. Ignore error if the channel is already closed.
            self.sender.send(ParquetMessage::Shutdown).ok();
        }

        // Wait for the writer thread to finish
        if let Some(handle) = self.writer_thread.take() {
            handle.join().map_err(|e| {
                TracingError::thread_join(format!(
                    "Failed to close telemetry parquet writer: {e:?}"
                ))
            })?;
        }

        // Check if there was an error during writing
        let err_lock = self.shutdown_err.lock().expect("Mutex poisoned");

        if let Some(e) = err_lock.as_ref() {
            return Err(TracingError::io(format!(
                "Telemetry parquet writer encountered an error: {}. Some telemetry data may have been lost.",
                e
            )));
        }

        Ok(())
    }
}

/// Ensure shutdown is called on drop
impl Drop for TelemetryParquetWriterHandle {
    fn drop(&mut self) {
        // Discard any error, as we can't return it from drop
        self.shutdown().ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::mocks::{
        MockDynLogEvent, MockTelemetryEventRegistry, MockUnknown, test_data_layer,
    };
    use crate::{
        LogRecordInfo, SeverityNumber, TelemetryOutputFlags, TelemetryRecord,
        serialize::arrow::{TelemetryArrowSchemas, deserialize_from_arrow},
    };
    use arrow_schema::Schema;
    use std::io::{self, Cursor, Write};
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    /// Mock writer that uses an in-memory buffer
    struct MockWriter {
        buffer: Arc<Mutex<Cursor<Vec<u8>>>>,
        fail_after_bytes: Option<usize>,
        fail_on_close: bool,
    }

    impl MockWriter {
        fn new() -> (Self, Arc<Mutex<Cursor<Vec<u8>>>>) {
            let buffer = Arc::new(Mutex::new(Cursor::new(Vec::new())));
            (
                Self {
                    buffer: buffer.clone(),
                    fail_after_bytes: None,
                    fail_on_close: false,
                },
                buffer,
            )
        }

        fn with_fail_after_bytes(mut self, bytes: usize) -> Self {
            self.fail_after_bytes = Some(bytes);
            self
        }

        #[allow(dead_code)]
        fn with_fail_on_close(mut self) -> Self {
            self.fail_on_close = true;
            self
        }
    }

    impl Write for MockWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut cursor = self.buffer.lock().unwrap();

            // Check if we should fail
            if let Some(fail_after) = self.fail_after_bytes {
                let current_pos = cursor.position() as usize;
                if current_pos + buf.len() > fail_after {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "Mock write error",
                    ));
                }
            }

            cursor.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            if self.fail_on_close {
                Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "Mock flush error",
                ))
            } else {
                Ok(())
            }
        }
    }

    fn get_test_log_record(i: u64) -> TelemetryRecord {
        TelemetryRecord::LogRecord(LogRecordInfo {
            trace_id: 12345,
            span_id: Some(i),
            event_id: uuid::Uuid::new_v4(),
            span_name: Some(format!("test_span_{i}")),
            time_unix_nano: SystemTime::now(),
            body: format!("Test message {i}"),
            severity_number: SeverityNumber::Info,
            severity_text: "INFO".to_string(),
            attributes: MockDynLogEvent {
                code: i as i32,
                flags: TelemetryOutputFlags::EXPORT_PARQUET,
                file: None,
                line: None,
                ..Default::default()
            }
            .into(),
        })
    }

    fn deserialize_parquet(buffer: &[u8]) -> Vec<TelemetryRecord> {
        use bytes::Bytes;
        use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};

        let bytes = Bytes::from_owner(buffer.to_vec());
        let schemas = TelemetryArrowSchemas::new::<MockTelemetryEventRegistry>();
        let schema_ref = Arc::new(Schema::new(schemas.schema_with_timestamps().to_vec()));

        let arrow_reader = ParquetRecordBatchReaderBuilder::try_new_with_options(
            bytes,
            ArrowReaderOptions::new().with_schema(schema_ref),
        )
        .unwrap()
        .build()
        .unwrap();

        let registry = MockTelemetryEventRegistry;

        let mut records = Vec::new();
        for batch in arrow_reader {
            let (mut batch_records, errors) =
                deserialize_from_arrow(&batch.unwrap(), &registry).unwrap();
            assert!(
                errors.is_empty(),
                "unexpected deserialize errors: {errors:?}"
            );
            records.append(&mut batch_records);
        }

        records
    }

    #[test]
    fn test_normal_write_and_shutdown_idempotency() {
        let (mock, buffer) = MockWriter::new();
        let (layer, mut handle) =
            TelemetryParquetWriterLayer::new::<_, MockTelemetryEventRegistry>(mock).unwrap();

        // Create some test records
        let record1 = get_test_log_record(1);
        let record2 = get_test_log_record(2);

        // Write records
        assert!(layer.write_record(record1.clone()).is_ok());
        assert!(layer.write_record(record2.clone()).is_ok());

        // Multiple shutdowns should be safe
        assert!(handle.shutdown().is_ok());
        assert!(handle.shutdown().is_ok());

        // Verify data was written (parquet format will have headers/footers)
        let buffer_contents = buffer.lock().unwrap();
        let records = deserialize_parquet(buffer_contents.get_ref());
        assert_eq!(records.len(), 2);
        assert_eq!(records[0], record1);
        assert_eq!(records[1], record2);
    }

    #[test]
    fn test_write_failure_stops_writer() {
        // Create a writer that fails after 1 byte
        let (mock, buffer) = MockWriter::new();
        let mock = mock.with_fail_after_bytes(1);
        let (layer, mut handle) =
            TelemetryParquetWriterLayer::new::<_, MockTelemetryEventRegistry>(mock).unwrap();

        // Write itself should succeed (the error will occur in the writer thread)
        assert!(layer.write_record(get_test_log_record(1)).is_ok());

        // Shutdown - should return error due to write failure
        let Err(error) = handle.shutdown() else {
            panic!("Expected shutdown to return error due to write failure");
        };

        assert!(matches!(&error, TracingError::Io(_)));
        assert!(
            // Due to internal parquet buffering, our mock writer will only
            // be really hit on finalize, NOT on the initial write
            error.to_string().contains("Failed to close Parquet writer"),
            "{}",
            error.to_string()
        );

        // Verify that no complete parquet file was written (the buffer should be empty or have incomplete data)
        let buffer_contents = buffer.lock().unwrap();
        let buf = buffer_contents.get_ref();

        // If any data was written, it would be incomplete and not parseable as valid parquet
        if !buf.is_empty() {
            // Attempting to deserialize should fail since the parquet file is incomplete
            let result = std::panic::catch_unwind(|| deserialize_parquet(buf));
            assert!(
                result.is_err(),
                "Should not be able to deserialize incomplete parquet data"
            );
        }
    }

    #[test]
    fn test_write_after_shutdown() {
        let (mock, buffer) = MockWriter::new();
        let (layer, mut handle) =
            TelemetryParquetWriterLayer::new::<_, MockTelemetryEventRegistry>(mock).unwrap();

        let record1 = get_test_log_record(1);
        let record2 = get_test_log_record(2);

        // Write first record before shutdown
        assert!(layer.write_record(record1.clone()).is_ok());

        handle.shutdown().unwrap();

        // Writes after shutdown should return error
        assert!(layer.write_record(record2).is_err());

        // Verify first record was written
        let buffer_contents = buffer.lock().unwrap();
        let records = deserialize_parquet(buffer_contents.get_ref());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0], record1);
    }

    #[test]
    fn test_layer_with_tracing_registry() {
        let (mock, buffer) = MockWriter::new();
        let (parquet_layer, mut handle) =
            TelemetryParquetWriterLayer::new::<_, MockTelemetryEventRegistry>(mock).unwrap();

        let trace_id = uuid::Uuid::new_v4().as_u128();
        let subscriber = crate::init::create_tracing_subcriber_with_layer(
            tracing::level_filters::LevelFilter::TRACE,
            test_data_layer(
                trace_id,
                None,
                false,
                std::iter::empty(),
                std::iter::once(Box::new(parquet_layer) as ConsumerLayer),
            ),
            &[],
        )
        .expect("test tracing filter directives must be valid");

        tracing::subscriber::with_default(subscriber, || {
            // Create nested spans
            let root_span = tracing::info_span!("root_span");
            let _root_guard = root_span.enter();

            {
                let child_span = tracing::info_span!("child_span");
                let _child_guard = child_span.enter();
                // Child span closes here
            }
            // Root span closes here
        });

        // Shutdown
        handle.shutdown().unwrap();

        // Verify the span records were written to parquet
        let buffer_contents = buffer.lock().unwrap();
        let records = deserialize_parquet(buffer_contents.get_ref());

        assert_eq!(
            records.len(),
            4,
            "Should have 2 span start + 2 span end records"
        );

        // Count span starts and ends
        let span_starts = records
            .iter()
            .filter(|r| matches!(r, TelemetryRecord::SpanStart(_)))
            .count();
        let span_ends = records
            .iter()
            .filter(|r| matches!(r, TelemetryRecord::SpanEnd(_)))
            .count();

        assert_eq!(span_starts, 2, "Should have 2 span start records");
        assert_eq!(span_ends, 2, "Should have 2 span end records");

        // Check records for correct span names and parent-child relationship
        for record in &records {
            let (trace_id_val, span_name, parent_span_id, attributes) = match record {
                TelemetryRecord::SpanStart(info) => (
                    &info.trace_id,
                    &info.span_name,
                    &info.parent_span_id,
                    &info.attributes,
                ),
                TelemetryRecord::SpanEnd(info) => (
                    &info.trace_id,
                    &info.span_name,
                    &info.parent_span_id,
                    &info.attributes,
                ),
                _ => panic!("Unexpected record: {record:?}"),
            };

            let name = attributes
                .downcast_ref::<MockUnknown>()
                .expect("Must be of MockUnknown type")
                .name
                .as_str();
            assert_eq!(trace_id_val, &trace_id);
            assert!(span_name.starts_with("Mock Unknown Span"));

            if name == "child_span" {
                // Child span should have root span as parent
                assert!(parent_span_id.is_some());
            } else if name == "root_span" {
                // Root span should have no parent
                assert!(parent_span_id.is_none());
            } else {
                panic!("Unexpected span name: {name}");
            }
        }
    }
}
