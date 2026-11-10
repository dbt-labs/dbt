//! Background writer task for persisting recorded events to disk.
//!
//! The writer receives events via MPSC channel and writes them to a
//! compressed NDJSON file with an index for efficient per-node access.

use std::io::Write;
use std::path::PathBuf;
use std::{collections::HashMap, io};

use dbt_common::cancellation::CancellationToken;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::event::{NodeIndex, RecordedEvent, RecordingHeader};

/// Configuration for the event writer.
#[derive(Debug, Clone)]
pub struct WriterConfig {
    /// Output directory for the recording
    pub output_path: PathBuf,
    /// Whether to compress the events file (gzip)
    pub compress: bool,
    /// Batch size for writing (number of events to buffer before flushing)
    pub batch_size: usize,
    /// Flush interval in milliseconds
    pub flush_interval_ms: u64,
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self {
            output_path: PathBuf::from("recording"),
            compress: true,
            batch_size: 100,
            flush_interval_ms: 100,
        }
    }
}

impl WriterConfig {
    /// Create a new writer config with the given output path.
    pub fn new(output_path: impl Into<PathBuf>) -> Self {
        Self {
            output_path: output_path.into(),
            ..Default::default()
        }
    }

    /// Set compression mode.
    pub fn with_compression(mut self, compress: bool) -> Self {
        self.compress = compress;
        self
    }

    /// Set batch size.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }
}

/// Result of a completed recording session.
#[derive(Debug)]
pub struct RecordingResult {
    /// Path to the events file
    pub events_path: PathBuf,
    /// Path to the index file
    pub index_path: PathBuf,
    /// Path to the header file
    pub header_path: PathBuf,
    /// Total number of events written
    pub event_count: u64,
    /// Number of unique nodes recorded
    pub node_count: usize,
}

pub fn spawn_writer(
    receiver: mpsc::Receiver<RecordedEvent>,
    header: RecordingHeader,
    config: WriterConfig,
    token: CancellationToken,
) -> JoinHandle<io::Result<RecordingResult>> {
    tokio::spawn(async move { writer_task(receiver, header, config, token).await })
}

/// The main writer task implementation.
///
/// This is separated from `spawn_writer` to allow testing without spawning.
pub async fn writer_task(
    mut receiver: mpsc::Receiver<RecordedEvent>,
    header: RecordingHeader,
    config: WriterConfig,
    token: CancellationToken,
) -> io::Result<RecordingResult> {
    std::fs::create_dir_all(&config.output_path)?;

    let header_path = config.output_path.join("header.json");
    let header_json = serde_json::to_string_pretty(&header)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(&header_path, header_json)?;

    // Create events file (with or without compression)
    let events_path = if config.compress {
        config.output_path.join("events.ndjson.gz")
    } else {
        config.output_path.join("events.ndjson")
    };

    let file = std::fs::File::create(&events_path)?;
    let mut writer: Box<dyn Write + Send> = if config.compress {
        Box::new(flate2::write::GzEncoder::new(
            file,
            flate2::Compression::fast(),
        ))
    } else {
        Box::new(io::BufWriter::new(file))
    };

    // Track index information (event counts and timestamp ranges per node)
    let mut index: HashMap<String, NodeIndex> = HashMap::new();
    let mut total_event_count: u64 = 0;

    // Batch buffer
    let mut batch: Vec<RecordedEvent> = Vec::with_capacity(config.batch_size);

    // Timeout for batch flushing
    let flush_duration = std::time::Duration::from_millis(config.flush_interval_ms);

    loop {
        // Cancellation: exit immediately, batch and channel contents are lost.
        if token.is_cancelled() {
            break;
        }

        // Try to receive with timeout
        let recv_result = tokio::time::timeout(flush_duration, receiver.recv()).await;

        match recv_result {
            Ok(Some(event)) => {
                batch.push(event);

                // Drain more if available (up to batch size)
                while batch.len() < config.batch_size {
                    match receiver.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
            }
            Ok(None) => {
                // Channel closed - flush remaining and exit
                flush_batch(&mut batch, &mut writer, &mut index, &mut total_event_count)?;
                break;
            }
            Err(_) => {
                // Timeout - flush what we have
            }
        }

        // Flush batch if we have events
        if !batch.is_empty() {
            flush_batch(&mut batch, &mut writer, &mut index, &mut total_event_count)?;
        }
    }

    // Finish compression if enabled
    drop(writer);

    // Write index file
    let index_path = config.output_path.join("index.json");
    let index_json = serde_json::to_string_pretty(&index)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(&index_path, index_json)?;

    Ok(RecordingResult {
        events_path,
        index_path,
        header_path,
        event_count: total_event_count,
        node_count: index.len(),
    })
}

/// Flush a batch of events to the writer, serializing each as a newline-delimited
/// JSON (NDJSON) line so the recording can be streamed/appended incrementally
/// and replayed without loading the whole file into memory.
fn flush_batch(
    batch: &mut Vec<RecordedEvent>,
    writer: &mut Box<dyn Write + Send>,
    index: &mut HashMap<String, NodeIndex>,
    total_event_count: &mut u64,
) -> io::Result<()> {
    for event in batch.drain(..) {
        let node_id = event.node_id().to_string();
        let timestamp_ns = event.timestamp_ns();

        let mut line = serde_json::to_vec(&event)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        line.push(b'\n');

        let entry = index.entry(node_id).or_insert(NodeIndex {
            event_count: 0,
            first_timestamp_ns: timestamp_ns,
            last_timestamp_ns: timestamp_ns,
        });

        writer.write_all(&line)?;

        entry.event_count += 1;
        entry.last_timestamp_ns = timestamp_ns;
        *total_event_count += 1;
    }

    writer.flush()?;
    Ok(())
}

/// Synchronous writer for testing and simple single-threaded tools.
///
/// Unlike the async writer (`spawn_writer`), this writes directly to disk on each
/// `write_event()` call. Use this only for unit tests where deterministic behavior is needed
/// without a Tokio runtime, or when high throughput isn't required.
///
/// For production recording, use `spawn_writer` instead.
#[allow(dead_code)]
pub(crate) struct SyncWriter {
    writer: Box<dyn Write + Send>,
    index: HashMap<String, NodeIndex>,
    event_count: u64,
    output_path: PathBuf,
    compress: bool,
}

#[allow(dead_code)]
impl SyncWriter {
    /// Create a new synchronous writer.
    pub(crate) fn new(
        output_path: impl Into<PathBuf>,
        header: &RecordingHeader,
    ) -> io::Result<Self> {
        let output_path = output_path.into();
        std::fs::create_dir_all(&output_path)?;

        let header_path = output_path.join("header.json");
        let header_json = serde_json::to_string_pretty(header)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        std::fs::write(header_path, header_json)?;

        let events_path = output_path.join("events.ndjson.gz");
        let file = std::fs::File::create(events_path)?;
        let writer: Box<dyn Write + Send> = Box::new(flate2::write::GzEncoder::new(
            file,
            flate2::Compression::fast(),
        ));

        Ok(Self {
            writer,
            index: HashMap::new(),
            event_count: 0,
            output_path,
            compress: true,
        })
    }

    /// Write a single event.
    pub(crate) fn write_event(&mut self, event: &RecordedEvent) -> io::Result<()> {
        let node_id = event.node_id().to_string();
        let timestamp_ns = event.timestamp_ns();

        let mut line =
            serde_json::to_vec(event).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        line.push(b'\n');

        let entry = self.index.entry(node_id).or_insert(NodeIndex {
            event_count: 0,
            first_timestamp_ns: timestamp_ns,
            last_timestamp_ns: timestamp_ns,
        });

        self.writer.write_all(&line)?;

        entry.event_count += 1;
        entry.last_timestamp_ns = timestamp_ns;
        self.event_count += 1;

        Ok(())
    }

    pub(crate) fn finish(mut self) -> io::Result<RecordingResult> {
        self.writer.flush()?;
        drop(self.writer);

        // Write index
        let index_path = self.output_path.join("index.json");
        let index_json = serde_json::to_string_pretty(&self.index)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        std::fs::write(&index_path, index_json)?;

        let events_path = if self.compress {
            self.output_path.join("events.ndjson.gz")
        } else {
            self.output_path.join("events.ndjson")
        };

        Ok(RecordingResult {
            events_path,
            index_path,
            header_path: self.output_path.join("header.json"),
            event_count: self.event_count,
            node_count: self.index.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time_machine::event::AdapterCallEvent;
    use crate::time_machine::semantic::SemanticCategory;

    fn create_test_event(node_id: &str, seq: u32) -> RecordedEvent {
        RecordedEvent::AdapterCall(AdapterCallEvent {
            node_id: node_id.to_string(),
            seq,
            method: "execute".to_string(),
            semantic_category: SemanticCategory::Write,
            args: serde_json::json!(["SELECT 1"]),
            result: serde_json::json!(null),
            success: true,
            error: None,
            timestamp_ns: 0,
        })
    }

    #[dbt_runtime::test]
    async fn test_writer_task() {
        let temp_dir =
            std::env::temp_dir().join(format!("time_machine_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);

        let (tx, rx) = mpsc::channel(100);
        let header = RecordingHeader::new("test", "test-invocation");
        let config = WriterConfig::new(&temp_dir).with_compression(false);

        // Send some events
        for i in 0..5 {
            tx.send(create_test_event("node_a", i)).await.unwrap();
        }
        for i in 0..3 {
            tx.send(create_test_event("node_b", i)).await.unwrap();
        }

        // Close the channel
        drop(tx);

        // Run the writer
        let result = writer_task(rx, header, config, CancellationToken::never_cancels())
            .await
            .unwrap();

        assert_eq!(result.event_count, 8);
        assert_eq!(result.node_count, 2);
        assert!(result.events_path.exists());
        assert!(result.index_path.exists());
        assert!(result.header_path.exists());

        // Verify index content
        let index_content = std::fs::read_to_string(&result.index_path).unwrap();
        let index: HashMap<String, NodeIndex> = serde_json::from_str(&index_content).unwrap();
        assert_eq!(index.get("node_a").unwrap().event_count, 5);
        assert_eq!(index.get("node_b").unwrap().event_count, 3);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_sync_writer() {
        let temp_dir =
            std::env::temp_dir().join(format!("time_machine_sync_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);

        let header = RecordingHeader::new("test", "test-invocation");
        let mut writer = SyncWriter::new(&temp_dir, &header).unwrap();

        for i in 0..5 {
            writer.write_event(&create_test_event("node_a", i)).unwrap();
        }

        let result = writer.finish().unwrap();

        assert_eq!(result.event_count, 5);
        assert_eq!(result.node_count, 1);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
