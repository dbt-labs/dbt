//! A module for emitting structured events.
//!
//! This module provides API's used for all span/event creation based on
//! our tracing infrastructre. They wrap `tracing::event!`/`tracing::span!` macros
//! and add functionality to capture file/line information of the callsite
//! (rather than the macro invocation site) and also efficiently pass telemetry attributes
//! into the tracing pipeline via thread-local storage.

use std::panic::Location;

use crate::{
    TelemetryAttributes, TelemetryEventRecType,
    constants::{CLOSE_SPAN_FIELD, ROOT_SPAN_NAME},
    event_info::store_event_attributes,
    shared::Recordable,
};

use tracing;

// Tracing's library built-in file/line detection is not based on panic module, and
// thus will always report the actual location of the macro call where it was invoke.
// We on the other hand, would like to use function, rather than macros to emit events
// and create spans to aide lsp/IDE's (and thus simplify debugging, refactoring etc.)
// To do that, we use functuns with `#[track_caller]` attribute that allow capturing
// file/line position of the callsite and then inject them as custom fields into
// tracing. Our data layer extracts these and prefers them over native location info
// privided by tracing, while still being compatible with direct tracing calls.
const FILE_FIELD: &str = "__file";
const LINE_FIELD: &str = "__line";

/// Helper that extracts file & line from fields if available
pub(super) fn get_file_and_line(values: Recordable<'_>) -> Option<(String, u32)> {
    struct SpanEventAttributesVisitor {
        file: Option<String>,
        line: Option<u32>,
    }

    impl tracing::field::Visit for SpanEventAttributesVisitor {
        fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            if field.name() == FILE_FIELD {
                self.file = Some(value.to_string());
            }
        }

        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            if field.name() == LINE_FIELD {
                self.line = Some(value as u32);
            }
        }
    }

    let mut visitor = SpanEventAttributesVisitor {
        file: None,
        line: None,
    };

    values.record(&mut visitor);

    visitor.file.map(|f| (f, visitor.line.unwrap_or(0)))
}

// The following repetetive functions have to be separate, as tracing requires
// a constant level for its macros and thus we cannot pass level as a parameter.
// They are also intentionally spelled out rather than using a macro, to ease
// debugging and IDE support.

/// Emit an error level event with the provided attributes and optional message.
#[track_caller]
pub fn emit_error_event(attrs: impl Into<TelemetryAttributes>, message: Option<&str>) {
    let attrs: TelemetryAttributes = attrs.into();

    debug_assert_eq!(
        attrs.record_category(),
        TelemetryEventRecType::Log,
        "Do not emit events of span type as logs!"
    );

    // Get the real code location
    let loc = Location::caller();

    // Save attributes to thread-local storage for the data layer to pick up
    store_event_attributes(attrs);

    // Emit event into tracing pipeline
    tracing::event!(
        tracing::Level::ERROR,
        message,
        { FILE_FIELD } = loc.file(),
        { LINE_FIELD } = loc.line()
    );
}

/// Emit a warning level event with the provided attributes and optional message.
#[track_caller]
pub fn emit_warn_event(attrs: impl Into<TelemetryAttributes>, message: Option<&str>) {
    let attrs: TelemetryAttributes = attrs.into();

    debug_assert_eq!(
        attrs.record_category(),
        TelemetryEventRecType::Log,
        "Do not emit events of span type as logs!"
    );

    // Get the real code location
    let loc = Location::caller();

    // Save attributes to thread-local storage for the data layer to pick up
    store_event_attributes(attrs);

    // Emit event into tracing pipeline
    tracing::event!(
        tracing::Level::WARN,
        message,
        { FILE_FIELD } = loc.file(),
        { LINE_FIELD } = loc.line()
    );
}

/// Emit an info level event with the provided attributes and optional message.
#[track_caller]
pub fn emit_info_event(attrs: impl Into<TelemetryAttributes>, message: Option<&str>) {
    let attrs: TelemetryAttributes = attrs.into();

    debug_assert_eq!(
        attrs.record_category(),
        TelemetryEventRecType::Log,
        "Do not emit events of span type as logs!"
    );

    // Get the real code location
    let loc = Location::caller();

    // Save attributes to thread-local storage for the data layer to pick up
    store_event_attributes(attrs);

    // Emit event into tracing pipeline
    tracing::event!(
        tracing::Level::INFO,
        message,
        { FILE_FIELD } = loc.file(),
        { LINE_FIELD } = loc.line()
    );
}

/// Emit a debug level event with the provided attributes and optional message.
#[track_caller]
pub fn emit_debug_event(attrs: impl Into<TelemetryAttributes>, message: Option<&str>) {
    let attrs: TelemetryAttributes = attrs.into();

    debug_assert_eq!(
        attrs.record_category(),
        TelemetryEventRecType::Log,
        "Do not emit events of span type as logs!"
    );

    // Get the real code location
    let loc = Location::caller();

    // Save attributes to thread-local storage for the data layer to pick up
    store_event_attributes(attrs);

    // Emit event into tracing pipeline
    tracing::event!(
        tracing::Level::DEBUG,
        message,
        { FILE_FIELD } = loc.file(),
        { LINE_FIELD } = loc.line()
    );
}

/// Emit a trace level event with the provided attributes and message.
///
/// NOTE: Trace level events are intended for fusion developer debugging and
/// turned off by default in production optional builds.
#[track_caller]
pub fn emit_trace_event(attrs_and_msg: impl FnOnce() -> (TelemetryAttributes, Option<String>)) {
    if tracing::event_enabled!(tracing::Level::TRACE) {
        let (attrs, message) = attrs_and_msg();

        debug_assert_eq!(
            attrs.record_category(),
            TelemetryEventRecType::Log,
            "Do not emit events of span type as logs!"
        );

        // Get the real code location
        let loc = Location::caller();

        // Save attributes to thread-local storage for the data layer to pick up
        store_event_attributes(attrs);

        // Emit event into tracing pipeline
        tracing::event!(
            tracing::Level::TRACE,
            message,
            { FILE_FIELD } = loc.file(),
            { LINE_FIELD } = loc.line()
        );
    }
}

/// Returns true if trace-level telemetry is enabled.
#[inline(always)]
pub fn is_trace_enabled() -> bool {
    tracing::enabled!(tracing::Level::TRACE)
}

/// Create a root info-level span with no parent.
///
/// This function creates a new tracing span at the info level that explicitly
/// has no parent span (root of a trace tree). It tracks the caller's location
/// and injects file/line information into the span for better debugging.
///
/// The returned span is force closable, see [`force_close_span`].
///
/// # Arguments
/// * `attrs` - Telemetry attributes for the span. In production this is expected
///   to be an `Invocation` type
#[track_caller]
pub fn create_root_info_span(attrs: impl Into<TelemetryAttributes>) -> tracing::Span {
    let attrs: TelemetryAttributes = attrs.into();

    // Get the real code location
    let loc = Location::caller();

    // Save attributes to thread-local storage for the data layer to pick up
    store_event_attributes(attrs);

    // Create the span
    tracing::info_span!(
        parent: None,
        // In our structured tracing we do not care about the span name,
        // everything comes from the attributes. However, we give the name here
        // for debug assertions in some API's that assume the correct root span is used.
        ROOT_SPAN_NAME,
        { FILE_FIELD } = loc.file(),
        { LINE_FIELD } = loc.line(),
        // Makes the span force closable, see `force_close_span`
        { CLOSE_SPAN_FIELD } = tracing::field::Empty
    )
}

/// Ends the structured telemetry of a force closable span now, even if other
/// handles to the span or its descendants are still alive.
///
/// A span is force closable if it was created with that capability. As of now,
/// only [`create_root_info_span`] creates such spans. For any other span this
/// does nothing.
///
/// The span end record is delivered at most once: repeated calls and the
/// eventual native close of the span are no-ops. Every later record in the span
/// subtree, including events, new child spans and the ends of children that
/// were already open, is suppressed. The native span itself stays alive until
/// all handles are dropped.
///
/// A record that is already being delivered on another thread when the span is
/// closed may still reach consumers after the span end.
pub fn force_close_span(span: &tracing::Span) {
    // Recording any value on the marker field requests the close from the data layer
    span.record(CLOSE_SPAN_FIELD, true);
}

/// Create an info-level span with the current active span as parent.
///
/// This function creates a new tracing span at the info level. If there is an
/// active span, it will automatically become the parent. It tracks the caller's
/// location and injects file/line information into the span for better debugging.
///
/// # Arguments
/// * `attrs` - Telemetry attributes for the span
#[track_caller]
pub fn create_info_span(attrs: impl Into<TelemetryAttributes>) -> tracing::Span {
    let attrs: TelemetryAttributes = attrs.into();

    // Get the real code location
    let loc = Location::caller();

    // Save attributes to thread-local storage for the data layer to pick up
    store_event_attributes(attrs);

    // Create the span
    tracing::info_span!(
        // In our structured tracing we do not care about the span name,
        // everything comes from the attributes.
        "",
        { FILE_FIELD } = loc.file(),
        { LINE_FIELD } = loc.line()
    )
}

/// Create an info-level span with an explicit parent span.
///
/// This function creates a new tracing span at the info level with an explicitly
/// specified parent span (or None for no parent). It tracks the caller's location
/// and injects file/line information into the span for better debugging.
///
/// # Arguments
/// * `parent` - Optional parent span ID (obtain via `span.id()`)
/// * `attrs` - Telemetry attributes for the span
#[track_caller]
pub fn create_info_span_with_parent(
    parent: Option<tracing::span::Id>,
    attrs: impl Into<TelemetryAttributes>,
) -> tracing::Span {
    let attrs: TelemetryAttributes = attrs.into();

    // Get the real code location
    let loc = Location::caller();

    // Save attributes to thread-local storage for the data layer to pick up
    store_event_attributes(attrs);

    // Create the span
    tracing::info_span!(
        parent: parent,
        "",
        { FILE_FIELD } = loc.file(),
        { LINE_FIELD } = loc.line()
    )
}

/// Create a debug-level span with the current active span as parent.
///
/// This function creates a new tracing span at the debug level. If there is an
/// active span, it will automatically become the parent. It tracks the caller's
/// location and injects file/line information into the span for better debugging.
///
/// # Arguments
/// * `attrs` - Telemetry attributes for the span
#[track_caller]
pub fn create_debug_span(attrs: impl Into<TelemetryAttributes>) -> tracing::Span {
    let attrs: TelemetryAttributes = attrs.into();

    // Get the real code location
    let loc = Location::caller();

    // Save attributes to thread-local storage for the data layer to pick up
    store_event_attributes(attrs);

    // Create the span
    tracing::debug_span!(
        // In our structured tracing we do not care about the span name,
        // everything comes from the attributes.
        "",
        { FILE_FIELD } = loc.file(),
        { LINE_FIELD } = loc.line()
    )
}

/// Create a debug-level span with an explicit parent span.
///
/// This function creates a new tracing span at the debug level with an explicitly
/// specified parent span (or None for no parent). It tracks the caller's location
/// and injects file/line information into the span for better debugging.
///
/// # Arguments
/// * `parent` - Optional parent span ID (obtain via `span.id()`)
/// * `attrs` - Telemetry attributes for the span
#[track_caller]
pub fn create_debug_span_with_parent(
    parent: Option<tracing::span::Id>,
    attrs: impl Into<TelemetryAttributes>,
) -> tracing::Span {
    let attrs: TelemetryAttributes = attrs.into();

    // Get the real code location
    let loc = Location::caller();

    // Save attributes to thread-local storage for the data layer to pick up
    store_event_attributes(attrs);

    // Create the span
    tracing::debug_span!(
        parent: parent,
        "",
        { FILE_FIELD } = loc.file(),
        { LINE_FIELD } = loc.line()
    )
}
