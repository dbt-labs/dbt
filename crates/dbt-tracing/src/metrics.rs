//! Multi-scoped globally accessible metric system based on tracing.
//!
//! This module provides API's for incrementing and reading arbitraty metrics,
//! that are scoped to the current reachable root tracing span. Meaning you can
//! have can have globally accessible metrics that may still be scoped and isolated
//! between parallel logically independent program threads, e.g. a service.
//!
//! Module provides process-wide public getters & writers, while tracing layers &middleware
//! may safely access metrics through [`crate::data_provider::DataProvider`].

use tracing_subscriber::registry::Extensions;

use crate::{
    constants::ROOT_SPAN_NAME,
    span_info::{SpanAccess, with_root_span},
};

mod sccmap {
    /// A scc::HashMap variant that uses a stable hasher in debug builds and a
    /// DoS-resistant hasher in release builds.
    #[allow(clippy::disallowed_types)]
    pub type HashMap<K, V> = scc::HashMap<K, V, dbt_base::MaybeStableHasherBuilder>;

    /// Creates a new scc::HashMap with the stable/DoS-resistant hasher.
    #[inline]
    pub fn new<K, V>() -> HashMap<K, V>
    where
        K: std::hash::Hash + Eq,
    {
        HashMap::with_hasher(dbt_base::MaybeStableHasherBuilder::default())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MetricKey(u64);

impl MetricKey {
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn into_raw(self) -> u64 {
        self.0
    }
}

impl From<u64> for MetricKey {
    fn from(raw: u64) -> Self {
        Self::from_raw(raw)
    }
}

impl From<MetricKey> for u64 {
    fn from(key: MetricKey) -> Self {
        key.into_raw()
    }
}

/// A private struct holding all metric counters.
///
/// Keep it private, this ensures no middleware or consumer can accidentally
/// replace or remove the metrics storage from the span extensions.
#[derive(Debug)]
struct MetricCounters {
    metrics: sccmap::HashMap<MetricKey, u64>,
}

impl MetricCounters {
    fn new() -> Self {
        Self {
            metrics: sccmap::new(),
        }
    }

    fn increment(&self, key: MetricKey, value: u64) {
        self.metrics
            .entry_sync(key)
            .and_modify(|v| *v = v.saturating_add(value))
            .or_insert(value);
    }

    fn get(&self, key: MetricKey) -> u64 {
        self.metrics.read_sync(&key, |_, v| *v).unwrap_or_default()
    }

    fn iter(&self) -> impl Iterator<Item = (MetricKey, u64)> + '_ {
        // Scc maps do not provide Iterator implementations, due
        // to locking requirements => thus we must eagerly collect and
        // allocate a Vec here.
        let mut metrics = Vec::new();
        self.metrics.iter_sync(|k, v| {
            metrics.push((*k, *v));
            true
        });

        metrics.into_iter()
    }
}

/// Initializes the metrics storage in root span extensions.
///
/// This should be called once when a root span is created to initialize
/// the metrics storage. Returns the initialized MetricCounters.
///
/// Panics if the MetricCounters is already initialized.
pub(super) fn init_metrics_storage_on_root_span(root_span: &dyn SpanAccess) {
    root_span.extensions_mut().insert(MetricCounters::new());
}

/// Increments an invocation metric counter
pub fn increment_metric(key: impl Into<MetricKey>, value: u64) {
    // Keep the public API generic while routing through a non-generic helper to
    // avoid extra monomorphized copies at call sites.
    increment_metric_inner(key.into(), value);
}

fn increment_metric_inner(key: MetricKey, value: u64) {
    with_root_span(|root_span| {
        debug_assert_eq!(
            root_span.name(),
            ROOT_SPAN_NAME,
            "Expected root span created via `create_root_info_span` in increment metrics. Got: {}.
            Are you running code not instrumented under an invocation span tree?",
            root_span.name()
        );

        increment_metric_on_span(&root_span as &dyn SpanAccess, key, value);
    });
}

/// Increments a metric counter on span extensions directly. Caller is
/// responsible for ensuring that the extension belongs to the correct (invocation) span.
///
/// Note: This function never takes a mutable lock on extensions to avoid global contention.
/// Metric storage is pre-initialized in data layer when any root span is created.
///
/// It will silently do nothing if the extension is not found.
pub(super) fn increment_metric_on_span(root_span: &dyn SpanAccess, key: MetricKey, value: u64) {
    // By default do not take a mutable lock on extensions to avoid global contention
    if let Some(metrics) = root_span.extensions().get::<MetricCounters>() {
        metrics.increment(key, value);
    };
}

/// Gets a specific invocation totals metrics directly from span extension. Caller is
/// responsible for ensuring that the extension belongs to the correct (invocation) span.
pub(super) fn get_metric_from_span_extension(span_ext: &Extensions<'_>, key: MetricKey) -> u64 {
    span_ext
        .get::<MetricCounters>()
        .map(|counters| counters.get(key))
        .unwrap_or_default()
}

/// Gets a specific invocation totals metrics (stored in the root invocation span).
pub fn get_metric(key: impl Into<MetricKey>) -> u64 {
    // Keep the public API generic while routing through a non-generic helper to
    // avoid extra monomorphized copies at call sites.
    get_metric_inner(key.into())
}

fn get_metric_inner(key: MetricKey) -> u64 {
    with_root_span(|root_span| {
        debug_assert_eq!(
            root_span.name(),
            ROOT_SPAN_NAME,
            "Expected root span created via `create_root_info_span` in get metrics. Got: {}.
            Are you running code not instrumented under an invocation span tree?",
            root_span.name()
        );
        get_metric_from_span_extension(&root_span.extensions(), key)
    })
    .unwrap_or_default()
}

pub(super) fn get_all_metrics_from_span_extension(
    span_ext: &Extensions<'_>,
) -> Vec<(MetricKey, u64)> {
    span_ext
        .get::<MetricCounters>()
        .map(|counters| counters.iter().collect())
        .unwrap_or_default()
}
