//! Traits and types for custom telemetry consumers and middleware.
//!
//! Unfortunately `tracing` library lacks a number of capabilities:
//!
//! 1) It provides efficient out of the box way to store arbitrary data associated with
//!    spans (`extensions`), but lacks a thread safe storage for events aka log data. And we
//!    need both for our telemetry data pipeline.
//! 2) Tracing doesn't allow passing arbitrary structured data to span/log facades,
//!    unless it is one of primitive types.
//! 3) Tracing per-layer filtering API doesn't provide access to span/log data.
//! 4) Tracing allows runtime reloading of layers, but doesn't work if they are filtered:
//!    https://github.com/tokio-rs/tracing/issues/1629 - we need this for out tests.
//!
//! We solve (1), (3), and (4) by implementing our own layer-like, filter
//! and middle-ware traits and types that are called from our single tracing layer
//! - `DataLayer`. They have less general purpose API than `tracing-subscriber` layers,
//!   but provide all the capabilities we need, and in a more performant way to boot.

use std::borrow::Cow;

use crate::{
    LogRecordInfo, SpanEndInfo, SpanStartInfo,
    data_provider::DataProvider,
    filter::{
        FilteredTelemetryConsumer, TelemetryFilter, TelemetryFilterFn, enable_all_logs,
        enable_all_spans,
    },
};

/// A consumer of telemetry data.
///
/// Implementing types are expected to ingest prepared telemetry data
/// and export/output it in some way.
///
/// Consumer have access to a `DataProvider` that allows safe access
/// to metics and thread-safe efficient per-invocation storage.
///
/// Note that consumers are not expected to modify metrics even though
/// API allows it. Metrics should be modified by middleware or in app code.
///
/// Callbacks must not emit tracing events or create spans. Recursive dispatch
/// through the data layer is unsupported and can deadlock tracing teardown.
pub trait TelemetryConsumer {
    /// Should return true if the consumer is interested in the span.
    #[allow(unused_variables)]
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool {
        true
    }

    /// Should return true if the consumer is interested in the log record.
    #[allow(unused_variables)]
    fn is_log_enabled(&self, log_record: &LogRecordInfo) -> bool {
        true
    }

    /// Callback invoked when a span starts.
    #[allow(unused_variables)]
    fn on_span_start(&self, span: &SpanStartInfo, data_provider: &mut DataProvider<'_>) {}

    /// Callback invoked when a span ends.
    #[allow(unused_variables)]
    fn on_span_end(&self, span: &SpanEndInfo, data_provider: &mut DataProvider<'_>) {}

    /// Callback invoked when a log record is created.
    #[allow(unused_variables)]
    fn on_log_record(&self, log_record: &LogRecordInfo, data_provider: &mut DataProvider<'_>) {}

    // Non dispatchable

    /// Create a new consumer that filters spans using the provided closure
    fn with_span_filter<S>(self, f: S) -> impl TelemetryConsumer
    where
        Self: Sized,
        S: Fn(&SpanStartInfo) -> bool + 'static,
    {
        FilteredTelemetryConsumer::new(self, TelemetryFilterFn::new(f, enable_all_logs))
    }

    /// Create a new consumer that filters logs using the provided closure
    fn with_log_filter<L>(self, f: L) -> impl TelemetryConsumer
    where
        Self: Sized,
        L: Fn(&LogRecordInfo) -> bool + 'static,
    {
        FilteredTelemetryConsumer::new(self, TelemetryFilterFn::new(enable_all_spans, f))
    }

    /// Create a new consumer that filters records using the provided filter
    fn with_filter<F>(self, filter: F) -> FilteredTelemetryConsumer<Self, F>
    where
        Self: Sized,
        F: TelemetryFilter,
    {
        FilteredTelemetryConsumer::new(self, filter)
    }
}

pub type ConsumerLayer = Box<dyn TelemetryConsumer + Send + Sync + 'static>;

/// Optional hook for consumers that need to rewrite log records before export.
pub type LogPreprocessorHook = for<'a> fn(&'a LogRecordInfo) -> Cow<'a, LogRecordInfo>;

/// A middleware that can modify telemetry data as it passes through the system.
///
/// All middleware's operate before any consumers see the data and have a global
/// effect on all consumers. So be mindful that changes you make will be
/// visible to all consumers.
///
/// Callbacks must not emit tracing events or create spans. Recursive dispatch
/// through the data layer is unsupported and can deadlock tracing teardown.
pub trait TelemetryMiddleware: Send + Sync {
    /// Callback invoked when a span starts. Return None to drop the span for all consumers.
    ///
    /// Note that if you return None, the span end callback will not be called,
    /// since this span will be marked as disabled for all consumers.
    #[allow(unused_variables)]
    fn on_span_start(
        &self,
        span: SpanStartInfo,
        data_provider: &mut DataProvider<'_>,
    ) -> Option<SpanStartInfo> {
        Some(span)
    }

    /// Callback invoked when a span ends. Return None to prevent the span end
    /// from being reported to consumers.
    #[allow(unused_variables)]
    fn on_span_end(
        &self,
        span: SpanEndInfo,
        data_provider: &mut DataProvider<'_>,
    ) -> Option<SpanEndInfo> {
        Some(span)
    }

    /// Callback invoked when a log record is created. Return None to drop the log record
    /// for all consumers.
    #[allow(unused_variables)]
    fn on_log_record(
        &self,
        record: LogRecordInfo,
        data_provider: &mut DataProvider<'_>,
    ) -> Option<LogRecordInfo> {
        Some(record)
    }
}

pub type MiddlewareLayer = Box<dyn TelemetryMiddleware + 'static>;
