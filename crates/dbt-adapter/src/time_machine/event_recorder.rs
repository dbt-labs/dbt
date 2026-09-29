//! Event recorder with MPSC channel for non-blocking event emission.
//!
//! The recorder provides a unified interface for emitting events from both
//! synchronous (Adapter) and asynchronous (MetadataAdapter) contexts.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use std::sync::Arc;
use tokio::sync::mpsc;

use super::event::{
    AdapterCallEvent, CacheInvalidationEvent, CatalogSchema, CatalogSchemas, MetadataCallArgs,
    MetadataCallEvent, RecordedCachedTestResult, RecordedEvent, RecordedRunCacheCloneDecision,
    RunCacheCloneEvent, RunRemoteAdhocEvent, SaoEvent, SaoStatus,
};
use super::semantic::SemanticCategory;
use super::serializable_impls::batches_to_ipc_base64;

/// Number of events to keep in buffer
const CHANNEL_BUFFER_SIZE: usize = 10_000;

/// Event recorder that captures adapter operations via MPSC channel.
#[derive(Clone)]
pub struct EventRecorder {
    /// Channel sender for emitting events
    sender: mpsc::Sender<RecordedEvent>,
    /// Per-node sequence counters
    seq_counters: Arc<dashmap::DashMap<String, AtomicU32>>,
    /// Recording start time for relative timestamps
    start_time: Instant,
    /// Global event counter
    event_count: Arc<AtomicU64>,
    /// Closed flag - when true, emits are no-ops
    closed: Arc<AtomicBool>,
}

impl EventRecorder {
    /// The caller must spawn the writer task with the returned receiver.
    pub fn new() -> (Self, mpsc::Receiver<RecordedEvent>) {
        let (sender, receiver) = mpsc::channel(CHANNEL_BUFFER_SIZE);

        let recorder = Self {
            sender,
            seq_counters: Arc::new(dashmap::DashMap::new()),
            start_time: Instant::now(),
            event_count: Arc::new(AtomicU64::new(0)),
            closed: Arc::new(AtomicBool::new(false)),
        };

        (recorder, receiver)
    }

    /// Mark the recorder as closed.
    ///
    /// After calling this, further events will be silently dropped.
    /// The channel closes when this EventRecorder (and all clones) is dropped,
    /// allowing the writer to drain remaining events.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    #[inline]
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Get the next sequence number for a node (thread-safe).
    #[inline]
    pub fn next_seq(&self, node_id: &str) -> u32 {
        // Fast path: check existing entry without allocation
        if let Some(counter) = self.seq_counters.get(node_id) {
            return counter.fetch_add(1, Ordering::Relaxed);
        }
        // Slow path: insert new counter
        self.seq_counters
            .entry(node_id.to_string())
            .or_insert_with(|| AtomicU32::new(0))
            .fetch_add(1, Ordering::Relaxed)
    }

    /// Get elapsed time since recording start in nanoseconds.
    pub fn elapsed_ns(&self) -> u64 {
        self.start_time.elapsed().as_nanos() as u64
    }

    /// Get total number of events emitted.
    pub fn event_count(&self) -> u64 {
        self.event_count.load(Ordering::Relaxed)
    }

    /// Emit an event from a sync context.
    ///
    /// Uses `try_send` to avoid blocking. If the channel is full,
    /// the event is dropped. This is a tradeoff to avoid performance
    /// regressions in synchronous code paths.
    #[inline]
    pub fn emit_sync(&self, event: RecordedEvent) {
        if self.is_closed() {
            return;
        }
        self.event_count.fetch_add(1, Ordering::Relaxed);
        let _ = self.sender.try_send(event);
    }

    /// Emit an event from an async context with backpressure.
    ///
    /// Awaits if the channel is full, applying backpressure to the caller.
    /// This is preferable for memory-constrained scenarios.
    pub async fn emit_async(&self, event: RecordedEvent) {
        if self.is_closed() {
            return;
        }
        self.event_count.fetch_add(1, Ordering::Relaxed);
        let _ = self.sender.send(event).await;
    }

    // -------------------------------------------------------------------------
    // Helper methods for recording specific event types
    // -------------------------------------------------------------------------

    /// Record an adapter call from Adapter::call_method.
    ///
    /// This is the primary entry point for recording Jinja adapter.xxx() calls.
    #[allow(clippy::too_many_arguments)]
    pub fn record_adapter_call(
        &self,
        node_id: impl Into<String>,
        method: impl Into<String>,
        args: serde_json::Value,
        result: serde_json::Value,
        success: bool,
        error: Option<String>,
    ) {
        let node_id = node_id.into();
        let method = method.into();
        let seq = self.next_seq(&node_id);
        let semantic_category = SemanticCategory::from_adapter_method(&method);

        self.emit_sync(RecordedEvent::AdapterCall(AdapterCallEvent {
            node_id,
            seq,
            method,
            semantic_category,
            args,
            result,
            success,
            error,
            timestamp_ns: self.elapsed_ns(),
        }));
    }

    /// Record a metadata adapter call.
    ///
    /// This is for async MetadataAdapter methods.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_metadata_call(
        &self,
        caller_id: impl Into<String>,
        method: impl Into<String>,
        args: MetadataCallArgs,
        result: serde_json::Value,
        success: bool,
        error: Option<String>,
        duration_ms: u64,
    ) {
        let caller_id = caller_id.into();
        let method = method.into();
        let seq = self.next_seq(&caller_id);
        let semantic_category = SemanticCategory::from_metadata_method(&method);

        self.emit_async(RecordedEvent::MetadataCall(MetadataCallEvent {
            caller_id,
            seq,
            method,
            semantic_category,
            args,
            result,
            success,
            error,
            duration_ms,
            timestamp_ns: self.elapsed_ns(),
        }))
        .await;
    }

    /// Record an SAO  skip event.
    ///
    /// This is called when a node is skipped due to a cache hit.
    pub fn record_sao_skip(
        &self,
        node_id: impl Into<String>,
        status: SaoStatus,
        message: impl Into<String>,
        stored_hash: impl Into<String>,
    ) {
        self.record_sao_skip_with_test_result(node_id, status, message, stored_hash, None);
    }

    /// Record a cache skip with the cached data-test result needed for replay.
    pub fn record_sao_skip_with_test_result(
        &self,
        node_id: impl Into<String>,
        status: SaoStatus,
        message: impl Into<String>,
        stored_hash: impl Into<String>,
        cached_test_result: Option<RecordedCachedTestResult>,
    ) {
        self.emit_sync(RecordedEvent::Sao(SaoEvent {
            node_id: node_id.into(),
            status,
            message: message.into(),
            stored_hash: stored_hash.into(),
            cached_test_result,
            timestamp_ns: self.elapsed_ns(),
        }));
    }

    /// Record a direct engine query from `run_remote_adhoc()`.
    ///
    /// This captures queries that bypass the Adapter layer, such as
    /// `dbt show --inline` queries executed via ADBC connections.
    pub fn record_run_remote_adhoc(
        &self,
        sql: impl Into<String>,
        batches: &[arrow::array::RecordBatch],
        schema: &arrow_schema::SchemaRef,
        success: bool,
        error: Option<String>,
    ) {
        let caller_id = "run_remote_adhoc".to_string();
        let seq = self.next_seq(&caller_id);
        let result_ipc_base64 = batches_to_ipc_base64(batches, schema).unwrap_or_default();

        self.emit_sync(RecordedEvent::RunRemoteAdhoc(RunRemoteAdhocEvent {
            caller_id,
            seq,
            sql: sql.into(),
            result_ipc_base64,
            success,
            error,
            timestamp_ns: self.elapsed_ns(),
        }));
    }

    /// Record cache invalidation decisions for missing warehouse relations.
    ///
    /// This captures which nodes were invalidated so replay can reproduce the
    /// same invalidation without querying the warehouse.
    pub fn record_cache_invalidation(&self, invalidated_nodes: Vec<String>) {
        self.emit_sync(RecordedEvent::CacheInvalidation(CacheInvalidationEvent {
            invalidated_nodes,
            timestamp_ns: self.elapsed_ns(),
        }));
    }

    /// Record a dbt State service "ready to clone" decision.
    ///
    /// This is called when a node's execution is satisfied by cloning an
    /// existing relation, so replay can route the node back through the
    /// same clone path instead of falling through to a normal Execute.
    pub fn record_run_cache_clone(
        &self,
        node_id: impl Into<String>,
        clone: RecordedRunCacheCloneDecision,
    ) {
        self.record_clone(node_id, clone, false);
    }

    /// Record a clone that runs before node rendering.
    pub fn record_dev_clone(
        &self,
        node_id: impl Into<String>,
        clone: RecordedRunCacheCloneDecision,
    ) {
        self.record_clone(node_id, clone, true);
    }

    fn record_clone(
        &self,
        node_id: impl Into<String>,
        clone: RecordedRunCacheCloneDecision,
        dev_clone: bool,
    ) {
        self.emit_sync(RecordedEvent::RunCacheClone(RunCacheCloneEvent {
            node_id: node_id.into(),
            dev_clone,
            clone,
            timestamp_ns: self.elapsed_ns(),
        }));
    }
}

impl Default for EventRecorder {
    fn default() -> Self {
        Self::new().0
    }
}

impl std::fmt::Debug for EventRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventRecorder")
            .field("event_count", &self.event_count())
            .field("is_closed", &self.is_closed())
            .finish()
    }
}

/// Create MetadataCallArgs for list_relations_schemas.
pub fn args_list_relations_schemas(
    unique_id: Option<String>,
    phase: Option<String>,
    relations: impl IntoIterator<Item = impl AsRef<str>>,
) -> MetadataCallArgs {
    MetadataCallArgs::ListRelationsSchemas {
        unique_id,
        phase,
        relations: relations
            .into_iter()
            .map(|r| r.as_ref().to_string())
            .collect(),
    }
}

/// Create MetadataCallArgs for list_relations_in_parallel.
pub fn args_list_relations_in_parallel(
    db_schemas: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
) -> MetadataCallArgs {
    MetadataCallArgs::ListRelationsInParallel {
        db_schemas: db_schemas
            .into_iter()
            .map(|(c, s)| CatalogSchema {
                catalog: c.into(),
                schema: s.into(),
            })
            .collect(),
    }
}

/// Create MetadataCallArgs for list_user_defined_functions.
pub fn args_list_udfs(
    catalog_schemas: impl IntoIterator<
        Item = (
            impl Into<String>,
            impl IntoIterator<Item = impl Into<String>>,
        ),
    >,
) -> MetadataCallArgs {
    MetadataCallArgs::ListUserDefinedFunctions {
        catalog_schemas: catalog_schemas
            .into_iter()
            .map(|(c, schemas)| CatalogSchemas {
                catalog: c.into(),
                schemas: schemas.into_iter().map(|s| s.into()).collect(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[dbt_runtime::test]
    async fn test_recorder_basic() {
        let (recorder, mut receiver) = EventRecorder::new();

        // Emit a sync event
        recorder.record_adapter_call(
            "model.test.orders",
            "execute",
            serde_json::json!(["SELECT 1"]),
            serde_json::json!({"rows": 1}),
            true,
            None,
        );

        // Should receive the event
        let event = receiver.recv().await.unwrap();
        assert_eq!(event.node_id(), "model.test.orders");
        assert_eq!(event.seq(), 0);
    }

    #[dbt_runtime::test]
    async fn test_record_sao_skip() {
        let (recorder, mut receiver) = EventRecorder::new();

        // Record an SAO skip event
        recorder.record_sao_skip(
            "model.test.orders",
            SaoStatus::ReusedNoChanges,
            "No new changes on any upstreams",
            "abc123",
        );

        // Should receive the event
        let event = receiver.recv().await.unwrap();
        assert_eq!(event.node_id(), "model.test.orders");
        assert_eq!(event.seq(), 0); // SAO events always have seq 0

        if let RecordedEvent::Sao(sao) = event {
            assert!(matches!(sao.status, SaoStatus::ReusedNoChanges));
            assert_eq!(sao.message, "No new changes on any upstreams");
            assert_eq!(sao.stored_hash, "abc123");
        } else {
            panic!("Expected Sao event");
        }
    }

    #[dbt_runtime::test]
    async fn test_record_sao_skip_with_freshness() {
        let (recorder, mut receiver) = EventRecorder::new();

        // Record an SAO skip event with freshness info
        recorder.record_sao_skip(
            "model.test.orders",
            SaoStatus::ReusedStillFresh {
                freshness_seconds: 3600,
                last_updated_seconds: 1800,
            },
            "Still within freshness period",
            "def456",
        );

        let event = receiver.recv().await.unwrap();
        if let RecordedEvent::Sao(sao) = event {
            if let SaoStatus::ReusedStillFresh {
                freshness_seconds,
                last_updated_seconds,
            } = sao.status
            {
                assert_eq!(freshness_seconds, 3600);
                assert_eq!(last_updated_seconds, 1800);
            } else {
                panic!("Expected ReusedStillFresh status");
            }
        } else {
            panic!("Expected Sao event");
        }
    }

    #[dbt_runtime::test]
    async fn test_sequence_numbers() {
        let (recorder, mut receiver) = EventRecorder::new();

        // Emit multiple events for same node
        for i in 0..3 {
            recorder.record_adapter_call(
                "model.test.orders",
                "execute",
                serde_json::json!([format!("query {}", i)]),
                serde_json::json!(null),
                true,
                None,
            );
        }

        // Verify sequence numbers
        for expected_seq in 0..3 {
            let event = receiver.recv().await.unwrap();
            assert_eq!(event.seq(), expected_seq);
        }
    }

    #[dbt_runtime::test]
    async fn test_multiple_nodes() {
        let (recorder, mut receiver) = EventRecorder::new();

        // Events for different nodes
        recorder.record_adapter_call(
            "node_a",
            "execute",
            serde_json::json!([]),
            serde_json::json!(null),
            true,
            None,
        );
        recorder.record_adapter_call(
            "node_b",
            "execute",
            serde_json::json!([]),
            serde_json::json!(null),
            true,
            None,
        );
        recorder.record_adapter_call(
            "node_a",
            "execute",
            serde_json::json!([]),
            serde_json::json!(null),
            true,
            None,
        );

        // node_a should have seq 0, 1
        // node_b should have seq 0
        let e1 = receiver.recv().await.unwrap();
        let e2 = receiver.recv().await.unwrap();
        let e3 = receiver.recv().await.unwrap();

        assert_eq!((e1.node_id(), e1.seq()), ("node_a", 0));
        assert_eq!((e2.node_id(), e2.seq()), ("node_b", 0));
        assert_eq!((e3.node_id(), e3.seq()), ("node_a", 1));
    }
}
