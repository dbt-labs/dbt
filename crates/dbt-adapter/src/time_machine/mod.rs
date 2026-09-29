//! Time Machine: Cross-Version Record/Replay System
//!
//! This module provides infrastructure for recording and replaying adapter-level
//! behavior, enabling backward compatibility testing across Fusion versions.
//!
//! # Architecture
//!
//! The system captures events at two levels:
//!
//! 1. `Adapter` (sync): Jinja `adapter.xxx()` calls via `Object::call_method`
//! 2. `MetadataAdapter` (async): Schema discovery, freshness, and relation listing
//!
//! Events are emitted via an MPSC channel to a background writer task, keeping overhead
//! on the hot path minimal.
//!
//! # Recording Format
//!
//! Recordings are stored as:
//! ```text
//! {invocation_id}/             # UUID v7 (time-ordered) e.g., 019384a5-b6c7-7def-8901-234567890abc
//!   header.json          # Metadata: engine version, adapter type
//!   events.ndjson.gz     # All events in arrival order (compressed)
//!   index.json           # Informational summary to help process events
//! ```
//!
//! # Global Recording Session
//!
//! The recorder uses a global singleton pattern to ensure it stays alive for the
//! entire Fusion run. Initialize it once at startup and access it from anywhere:
//!
//! ```ignore
//! use dbt_adapter::time_machine::{init_recording, global_recorder, shutdown_recording};
//! use dbt_common::cancellation::CancellationToken;
//!
//! // Initialize at start of run (returns a handle)
//! let handle = init_recording("./recordings/run-123", "snowflake", invocation_id, token)?;
//!
//! // Adapter: pass TimeMachine explicitly
//! let bridge = Adapter::with_time_machine(
//!     adapter_impl,
//!     db,
//!     cache,
//!     TimeMachine::recorder(global_recorder().unwrap().clone()),
//! );
//!
//! // MetadataAdapter: uses global_recorder() internally via with_metadata_recording()
//! // No explicit setup needed - it finds the global automatically
//!
//! // Shutdown at end of run
//! let result = handle.shutdown().await?;
//! println!("Recorded {} events", result.event_count);
//! ```
//!
//! # Semantic Categories
//!
//! Events are classified by their side effects for dependency analysis:
//!
//! - `MetadataRead`: query database state without mutation (SELECT/SHOW)
//! - `Write`: mutate database state (DDL/DML)
//! - `Cache`: internal bookkeeping (no DB I/O)
//! - `Pure`: local computation (no I/O at all)

use std::{io, sync::Arc};

use dbt_common::cancellation::CancellationToken;
use parking_lot::Mutex;
use tokio::task::JoinHandle;

pub mod engine;
pub mod event;
pub mod event_recorder;
pub mod event_replay;
pub mod metadata;
pub mod semantic;
pub mod serde;
pub mod serializable;
pub(crate) mod serializable_impls;
pub(crate) mod validation;
pub mod writer;

// Re-export commonly used types
pub use engine::{EventReplayer, ReplayCallError, ReplayerStats, TimeMachine};
pub use event::{
    AdapterCallEvent, CacheInvalidationEvent, CatalogSchema, CatalogSchemas, MetadataCallArgs,
    MetadataCallEvent, NodeIndex, RecordedCachedTestResult, RecordedEvent,
    RecordedRunCacheCloneDecision, RecordingHeader, RunCacheCloneEvent, RunRemoteAdhocEvent,
    SaoEvent, SaoStatus,
};
pub use event_recorder::EventRecorder;
pub use event_replay::{
    Recording, ReplayDifference, ReplayError, ReplayMode, ReplayResult, validate_replay,
};
pub use serializable_impls::{batches_to_ipc_base64, ipc_base64_to_batches};

// Re-export time machine ordering type and provide conversion
use dbt_common::io_args::TimeMachineReplayOrdering;

impl From<TimeMachineReplayOrdering> for ReplayMode {
    fn from(ordering: TimeMachineReplayOrdering) -> Self {
        match ordering {
            TimeMachineReplayOrdering::Strict => ReplayMode::Strict,
            TimeMachineReplayOrdering::Semantic => ReplayMode::Semantic,
        }
    }
}
pub use metadata::{
    MetadataResultDeserialize, MetadataResultSerialize, args_create_schemas_if_not_exists,
    args_fetch_view_definitions, args_freshness, args_freshness_all_in_schema,
    args_freshness_with_overrides, args_list_relations_in_parallel, args_list_relations_schemas,
    args_list_relations_schemas_by_patterns, args_list_udfs, args_relations_exist,
    with_time_machine_metadata_wrapper,
};
pub use semantic::SemanticCategory;
pub use serde::{
    ReplayContext, json_to_value_with_context, serialize_args, serialize_value, values_match,
};
pub use serializable::{
    DeserializeFn, JsonExtractor, TimeMachineSerializable, TypeEntry, deserialize_object, registry,
    serialize_object,
};
pub use writer::{RecordingResult, WriterConfig, spawn_writer};

/// Global recording session
struct RecordingSession {
    recorder: Arc<EventRecorder>,
    writer_handle: Mutex<Option<JoinHandle<io::Result<RecordingResult>>>>,
}

/// Global recording session — resettable so test harnesses can run multiple
/// invocations within a single process, each with its own recording.
static GLOBAL_SESSION: Mutex<Option<RecordingSession>> = Mutex::new(None);

/// Global replayer — resettable for multi-invocation test support.
static GLOBAL_REPLAYER: Mutex<Option<Arc<EventReplayer>>> = Mutex::new(None);

/// Call at the start of a Fusion run. If recording is already initialized (e.g., by
/// another adapter), this is a no-op — the existing session is used and the provided
/// config is ignored.
pub fn get_or_init_recording(
    output_path: impl Into<std::path::PathBuf>,
    adapter_type: impl Into<String>,
    invocation_id: impl Into<String>,
    invocation_command: Option<String>,
    token: CancellationToken,
) -> RecordingHandle {
    let mut guard = GLOBAL_SESSION.lock();
    if guard.is_none() {
        let output_path = output_path.into();
        let adapter_type = adapter_type.into();
        let invocation_id = invocation_id.into();

        let (recorder, receiver) = EventRecorder::new();
        let mut header = RecordingHeader::new(adapter_type, invocation_id);
        if let Some(cmd) = invocation_command {
            header = header.with_invocation_command(cmd);
        }
        let config = WriterConfig::new(output_path);
        let writer_handle = spawn_writer(receiver, header, config, token);

        *guard = Some(RecordingSession {
            recorder: Arc::new(recorder),
            writer_handle: Mutex::new(Some(writer_handle)),
        });
    }

    RecordingHandle { _private: () }
}

/// The primary way to access the recorder from anywhere in the codebase.
///
/// ```ignore
/// if let Some(recorder) = global_recorder() {
///     recorder.record_adapter_call(
///         node_id,
///         "execute",
///         serde_json::json!(["SELECT 1"]),
///         serde_json::json!(null),
///         true,
///         None,
///     );
/// }
/// ```
pub fn global_recorder() -> Option<Arc<EventRecorder>> {
    GLOBAL_SESSION
        .lock()
        .as_ref()
        .map(|s| Arc::clone(&s.recorder))
}

pub fn is_recording() -> bool {
    GLOBAL_SESSION.lock().is_some()
}

/// Lets `Adapter` (which holds `TimeMachine` directly) and `MetadataAdapter` (which
/// accesses via `global_replayer()`) share the same replayer instance.
///
/// ```ignore
/// let replayer = get_or_init_replayer(|| {
///     Arc::new(EventReplayer::load(path)?.with_replay_mode(mode))
/// })?;
/// let bridge = Adapter::with_time_machine(..., TimeMachine::replayer(replayer));
/// ```
pub fn get_or_init_replayer<F>(f: F) -> Result<Arc<EventReplayer>, ReplayError>
where
    F: FnOnce() -> Result<Arc<EventReplayer>, ReplayError>,
{
    // Don't allow both recording and replay at the same time
    if is_recording() {
        return Err(ReplayError::InvalidFormat(
            "Cannot initialize replay while recording is active".to_string(),
        ));
    }

    let mut guard = GLOBAL_REPLAYER.lock();
    if let Some(existing) = guard.as_ref() {
        return Ok(Arc::clone(existing));
    }

    // Create the replayer via closure
    let replayer = f()?;
    *guard = Some(Arc::clone(&replayer));
    Ok(replayer)
}

/// Get the global replayer, if initialized.
///
/// This is the primary way to access the replayer from anywhere in the codebase.
/// Returns `None` if replay has not been initialized.
pub fn global_replayer() -> Option<Arc<EventReplayer>> {
    GLOBAL_REPLAYER.lock().as_ref().map(Arc::clone)
}

pub fn is_replaying() -> bool {
    GLOBAL_REPLAYER.lock().is_some()
}

/// Handle for the global replay session.
pub struct ReplayHandle {
    _private: (),
}

impl ReplayHandle {
    /// Get the replayer statistics.
    pub fn stats(&self) -> Option<ReplayerStats> {
        GLOBAL_REPLAYER.lock().as_ref().map(|r| r.stats())
    }

    /// Reset replay state for all callers.
    pub fn reset(&self) {
        if let Some(replayer) = GLOBAL_REPLAYER.lock().as_ref() {
            replayer.reset();
        }
    }
}

/// Handle for the global recording session.
///
/// This handle controls the lifetime of the recording session. When dropped
/// without calling `shutdown()`, it will log a warning but recording will
/// continue until the process exits. Call `shutdown()` for proper cleanup.
pub struct RecordingHandle {
    _private: (),
}

impl RecordingHandle {
    /// Shutdown the recording session and wait for all events to be written.
    ///
    /// After shutdown, the global recording session is cleared and events
    /// will be silently dropped.
    pub async fn shutdown(self) -> Result<RecordingResult, RecordingError> {
        // Extract needed data synchronously (don't hold MutexGuard across await).
        // Must `take()` (not `as_ref()`): the writer's mpsc channel only closes
        // once the sole `Arc<EventRecorder>` is dropped, so leaving it in
        // `GLOBAL_SESSION` means the writer's `receiver.recv()` never sees
        // `None` and the `handle.await` below never resolves.
        let handle = {
            let mut guard = GLOBAL_SESSION.lock();
            let session = guard.take().ok_or(RecordingError::NotInitialized)?;
            session.recorder.close();
            session
                .writer_handle
                .lock()
                .take()
                .ok_or(RecordingError::AlreadyShutdown)?
        };

        // Wait for the writer to drain remaining events and complete
        handle
            .await
            .map_err(|e| RecordingError::WriterPanic(e.to_string()))?
            .map_err(RecordingError::IoError)
    }

    /// Get statistics about the current recording session.
    pub fn stats(&self) -> Option<RecordingStats> {
        GLOBAL_SESSION.lock().as_ref().map(|s| RecordingStats {
            event_count: s.recorder.event_count(),
        })
    }
}

/// Statistics about an active recording session.
#[derive(Debug, Clone)]
pub struct RecordingStats {
    /// Total number of events emitted so far.
    pub event_count: u64,
}

/// Errors that can occur during recording operations.
#[derive(Debug)]
pub enum RecordingError {
    /// Recording was not initialized.
    NotInitialized,
    /// Recording was already shut down.
    AlreadyShutdown,
    /// Writer task panicked.
    WriterPanic(String),
    /// I/O error during writing.
    IoError(io::Error),
}

impl std::fmt::Display for RecordingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotInitialized => write!(f, "Recording is not initialized"),
            Self::AlreadyShutdown => write!(f, "Recording was already shut down"),
            Self::WriterPanic(e) => write!(f, "Writer task panicked: {}", e),
            Self::IoError(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl std::error::Error for RecordingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::IoError(e) => Some(e),
            _ => None,
        }
    }
}

/// Reset global time machine state so a new recording or replay session can begin.
///
/// This is intended for test harnesses that run multiple dbt invocations
/// within a single process, each needing its own isolated recording/replay.
///
/// For recording mode this closes the recorder and flushes events to disk
/// before the next invocation starts.
pub async fn reset_time_machine_globals() -> Result<(), RecordingError> {
    // Take the recording session out of the global (sync, no await while locked)
    let taken_session = GLOBAL_SESSION.lock().take();

    // If a recording was active, close it and flush to disk
    if let Some(session) = taken_session {
        session.recorder.close();
        let writer_handle = session.writer_handle.lock().take();
        // Drop the session (and its Arc<EventRecorder> / sender) so the
        // channel closes and the writer task can finish draining.
        drop(session);
        if let Some(handle) = writer_handle {
            handle
                .await
                .map_err(|e| RecordingError::WriterPanic(e.to_string()))?
                .map_err(RecordingError::IoError)?;
        }
    }

    // Reset replayer
    *GLOBAL_REPLAYER.lock() = None;

    Ok(())
}

#[allow(dead_code)] // Used in tests
/// NOT FOR PROD USE: Create a recording session with default configuration.
///
/// This is a convenience function for testing or to establish multiple
/// independent recording sessions. For production use, prefer `init_recording()`
/// which sets up a global singleton.
pub(crate) fn start_recording(
    output_path: impl Into<std::path::PathBuf>,
    adapter_type: impl Into<String>,
    invocation_id: impl Into<String>,
) -> (EventRecorder, JoinHandle<io::Result<RecordingResult>>) {
    let (recorder, receiver) = EventRecorder::new();
    let header = RecordingHeader::new(adapter_type, invocation_id);
    let config = WriterConfig::new(output_path);
    let writer_handle = spawn_writer(receiver, header, config, CancellationToken::never_cancels());

    (recorder, writer_handle)
}
