use crate::{
    LogRecordInfo, SpanEndInfo, SpanStartInfo, data_provider::DataProvider,
    layer::TelemetryConsumer,
};

pub trait TelemetryFilter {
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool;

    fn is_log_enabled(&self, span: &LogRecordInfo) -> bool;
}

pub struct FilteredTelemetryConsumer<C, F>
where
    C: TelemetryConsumer,
    F: TelemetryFilter,
{
    inner: C,
    filter: F,
}

impl<C, F> FilteredTelemetryConsumer<C, F>
where
    C: TelemetryConsumer,
    F: TelemetryFilter,
{
    pub fn new(consumer: C, filter: F) -> Self {
        Self {
            inner: consumer,
            filter,
        }
    }
}

impl<C, F> TelemetryConsumer for FilteredTelemetryConsumer<C, F>
where
    C: TelemetryConsumer,
    F: TelemetryFilter,
{
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool {
        self.filter.is_span_enabled(span) && self.inner.is_span_enabled(span)
    }

    fn is_log_enabled(&self, span: &LogRecordInfo) -> bool {
        self.filter.is_log_enabled(span) && self.inner.is_log_enabled(span)
    }

    fn on_span_start(&self, span: &SpanStartInfo, data_provider: &mut DataProvider<'_>) {
        self.inner.on_span_start(span, data_provider);
    }

    fn on_span_end(&self, span: &SpanEndInfo, data_provider: &mut DataProvider<'_>) {
        self.inner.on_span_end(span, data_provider);
    }

    fn on_log_record(&self, event: &LogRecordInfo, data_provider: &mut DataProvider<'_>) {
        self.inner.on_log_record(event, data_provider);
    }
}

/// A convenience [`TelemetryFilter`] that delegates filtering decisions to user
/// supplied closure(s).
///
/// The generic parameter S is a closure (Fn) that receives a SpanStartInfo and its
/// Metadata and returns true if the span should be enabled. Likewise, L is a
/// closure that receives a LogRecordInfo and its Metadata to decide whether the
/// log event should be enabled.
///
/// We provide convenience functions `enable_all_spans`, `disable_all_spans`,
/// `enable_all_logs`, and `disable_all_logs` that can be used when you only want
/// to filter one of the two types of telemetry records.
///
/// This is useful when constructing lightweight, ad‑hoc filters without having
/// to implement a dedicated type.
///
/// Note that for filtering based on level you can use [`tracing::level_filters::LevelFilter`]
/// directly as it implements the filter trait.
pub struct TelemetryFilterFn<S, L>
where
    S: Fn(&SpanStartInfo) -> bool,
    L: Fn(&LogRecordInfo) -> bool,
{
    is_span_enabled: S,
    is_log_enabled: L,
}

impl<S, L> TelemetryFilterFn<S, L>
where
    S: Fn(&SpanStartInfo) -> bool,
    L: Fn(&LogRecordInfo) -> bool,
{
    /// Creates a [`TelemetryFilterFn`] with optional span and log filters.
    pub fn new(is_span_enabled: S, is_log_enabled: L) -> Self {
        Self {
            is_span_enabled,
            is_log_enabled,
        }
    }
}

// Convenience functions for common cases
pub fn disable_all_spans(_span: &SpanStartInfo) -> bool {
    false
}

pub fn enable_all_spans(_span: &SpanStartInfo) -> bool {
    true
}

pub fn disable_all_logs(_log: &LogRecordInfo) -> bool {
    false
}

pub fn enable_all_logs(_log: &LogRecordInfo) -> bool {
    true
}

impl<S, L> TelemetryFilter for TelemetryFilterFn<S, L>
where
    S: Fn(&SpanStartInfo) -> bool,
    L: Fn(&LogRecordInfo) -> bool,
{
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool {
        (self.is_span_enabled)(span)
    }

    fn is_log_enabled(&self, span: &LogRecordInfo) -> bool {
        (self.is_log_enabled)(span)
    }
}

impl TelemetryFilter for tracing::level_filters::LevelFilter {
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool {
        span.severity_number
            .try_into()
            .is_ok_and(|level: tracing::Level| level <= *self)
    }

    fn is_log_enabled(&self, log_record: &LogRecordInfo) -> bool {
        log_record
            .severity_number
            .try_into()
            .is_ok_and(|level: tracing::Level| level <= *self)
    }
}
