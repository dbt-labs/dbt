use crate::TelemetryAttributes;
use std::cell::RefCell;
use tracing::Event;

// Unfortunately `tracing` library lacks a number of capabilities:
//
// 1) It provides efficieint out of the box way to store arbitrary data associated with
// spans (`extensions`), but lacks a thread safe storage for events aka log data. And we
// need both for our telemetry data pipeline.
// 2) Tracing doesn't allow passing arbitrary structured data to span/log facades,
// unless it is one of primitive types.
// 3) Tracing per-layer filtering API doesn't provide access to span/log data.
// 4) Tracing allows runtime reloading of layers, but doesn't work if they are filtered:
//    https://github.com/tokio-rs/tracing/issues/1629 - we need this for out tests.
//
// We work around (2) via thread-local variable defined below.
//
// NOTE: this assumes that consuming layer always read structured data from the same thread
// as the data layer that wrote it, so make sure no downstream layer uses async/spawn
// until it read the data into locals.
thread_local! {
    /// Thread-local storage for structured telemetry attributes, aka even data.
    /// Used to efficiently pass structured data to data layer without serialization
    /// through tracing fields. Solves (2) from the list above. Used for spans and logs.
    static CURRENT_EVENT_ATTRIBUTES: RefCell<Option<TelemetryAttributes>> = const { RefCell::new(None) };
}

/// Pre-saves structured event attributes to be immediately consumed by tracing span/log call.
///
/// If you want to emit a log or create a new span, prefer - `dbt_tracing::emit::...` macros to avoid mistakes.
///
/// The only use case where this API should be used outside of `dbt_common::tracing` is
/// in conjunction with `#[tracing::instrument]`:
/// ```no_run
/// use dbt_tracing::event_info::store_event_attributes;
///
/// #[tracing::instrument(
///    skip_all,
///    fields(
///        _e = ?store_event_attributes(/* your AnyTelemetryEvent value here */),
///    )
///)]
/// fn your_function(...) { ... }
/// ```
///
/// Note that `_e` field name is irrelevant, and only necessary to inject
/// the call to `store_event_attributes()` into instrumented function. We
/// actually use it's side effect of storing the attributes in thread-local storage.
///
/// ALso note that `?` is necessary due to `tracing` macro limitations.
pub fn store_event_attributes(attrs: impl Into<TelemetryAttributes>) {
    CURRENT_EVENT_ATTRIBUTES.with(|cell| {
        *cell.borrow_mut() = Some(attrs.into());
    });
}

/// A private API for Data Layer to access pre-populated structured event attributes.
pub(super) fn take_event_attributes() -> Option<TelemetryAttributes> {
    CURRENT_EVENT_ATTRIBUTES.with(|cell| cell.take())
}

pub(super) fn get_log_message(event: &Event<'_>) -> String {
    struct MessageVisitor<'a>(&'a mut String);

    impl<'a> tracing::field::Visit for MessageVisitor<'a> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0.push_str(&format!("{value:?}"));
            }
        }

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            if field.name() == "message" {
                self.0.push_str(value);
            }
        }
    }

    let mut message = String::new();
    let mut visitor = MessageVisitor(&mut message);
    event.record(&mut visitor);

    message
}
