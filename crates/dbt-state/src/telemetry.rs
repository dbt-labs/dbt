use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

/// Shared monotonic event counter for run cache telemetry.
///
/// Shared between state selector (compilation phase) and task execution
/// (run phase) to ensure globally ordered event_order values. Both phases
/// emit events via SubmitTelemetryBatch RPC, and this counter prevents
/// duplicate/overlapping order values.
#[derive(Clone, Debug, Default)]
pub struct SharedEventOrder(Arc<AtomicI64>);

impl SharedEventOrder {
    pub fn new() -> Self {
        Self(Arc::new(AtomicI64::new(0)))
    }

    /// Atomically increments and returns the next event order value.
    pub fn next(&self) -> i64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}
