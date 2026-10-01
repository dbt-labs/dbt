//! Core traits for the extensible telemetry event types and associated system.

use serde::Serialize;
use serde_json::Value as JsonValue;
use std::{any::Any, fmt::Debug};

use crate::{
    SpanStatus, TelemetryOutputFlags, attributes::TelemetryContext, schemas::RecordCodeLocation,
    serialize::traits::ArrowAttributesSerialize,
};

/// Category of record (envelope) this event should be recorded in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryEventRecType {
    Span,
    Log,
}

/// Core trait that all telemetry events must implement.
///
/// This trait enables runtime dispatch based on event type URIs,
/// allowing downstream users to define their own events with arbitrary data.
///
/// This trait is dyn-compatible (object-safe) to allow for trait objects.
pub trait AnyTelemetryEvent: Debug + Send + Sync + Any {
    /// Returns the unique event type identifier.
    ///
    /// By convention this is fully-qualified protobuf message name, e.g.
    /// `v1.public.events.fusion.<category>.<EventName>` if event is intended to be serialized.
    ///
    /// Internal events should use dummy fqn's starting with `v1.internal.events.fusion.`.
    fn event_type(&self) -> &'static str;

    /// Returns a human-readable name/description for this event.
    ///
    /// This is used for span naming and display purposes and may use
    /// instance data to generate a more descriptive name.
    fn event_display_name(&self) -> String;

    /// Returns the status that should be associated with a span created for this event,
    /// if it can be determined from the event data.
    ///
    /// Events with `record_category() == TelemetryEventRecType::Log` should never
    /// return a status, as logs do not have statuses.
    ///
    /// Events with `record_category() == TelemetryEventRecType::Span` may return
    /// a status if the event data contains sufficient information to determine
    /// the aggregate status of the operation being represented by the span.
    ///
    /// Default implementation returns `None`.
    fn get_span_status(&self) -> Option<SpanStatus> {
        None
    }

    /// Returns the category of record (envelope) this event should be recorded in.
    fn record_category(&self) -> TelemetryEventRecType;

    /// Returns flags that determine where a telemetry event should be exported and/or output.
    fn output_flags(&self) -> TelemetryOutputFlags;

    /// Equality check for trait objects. Used to implement PartialEq for boxed trait objects.
    fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool;

    /// Returns code location associated with this event, if any.
    ///
    /// Default implementation returns `None`.
    fn code_location(&self) -> Option<RecordCodeLocation> {
        None
    }

    /// Called with code location based on emission call-site.
    /// If event stores code location - it can use the provided value to
    /// populate its location fields even if they were not set at construction time.
    ///
    /// Default implementation does nothing - types with location fields should override
    /// and implement the logic to fill in fields if they are empty.
    fn with_code_location(&mut self, _location: RecordCodeLocation) {}

    /// Returns a context that this event defines (if any) to be propagated
    /// to child spans and logs.
    ///
    /// Default implementation returns `None`.
    fn context(&self) -> Option<TelemetryContext> {
        None
    }

    /// Inject a provided context into this event (if supported).
    ///
    /// Default implementation does nothing.
    fn with_context(&mut self, _context: &TelemetryContext) {}

    /// Returns true if this event MAY contain sensitive data that should be scrubbed
    /// if the user has opted out of sensitive data collection.
    fn has_sensitive_data(&self) -> bool;

    /// Returns a clone of this event with sensitive data removed. `None` if
    /// the entire event should be dropped.
    ///
    /// Default implementation panics if `has_sensitive_data` is true.
    /// Types with sensitive data MUST override.
    ///
    /// SAFETY: This method MUST return a new instance of the same concrete type,
    /// just type-erased as a trait object. The returned value must be downcastable
    /// to the original type. This is used to implement sensitive data scrubbing logic.
    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        if self.has_sensitive_data() {
            panic!(
                "clone_without_sensitive_data called on event type {} that doesn't support it",
                self.event_type()
            );
        }

        let cloned = self.clone_box();
        // Validate that the cloned object is of the same concrete type.
        debug_assert_eq!(self.as_any().type_id(), cloned.as_any().type_id());

        Some(cloned)
    }

    /// Helper for downcasting to concrete types.
    fn as_any(&self) -> &dyn Any;

    /// Helper for downcasting to concrete types (mutable).
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Clone the event as a boxed trait object.
    ///
    /// IMPORTANT: This method MUST return a boxed clone of the same concrete type,
    /// just type-erased as a trait object. The returned value must be downcastable
    /// to the original type. This is used to implement Clone for concrete types.
    fn clone_box(&self) -> Box<dyn AnyTelemetryEvent>;

    /// Serialize the event data to JSON Value.
    fn to_json(&self) -> Result<JsonValue, String>;

    /// Convert this event to Arrow attributes if supported.
    ///
    /// Default behavior:
    /// - If `EXPORT_PARQUET` flag is NOT set in `output_flags()`, returns `None` (not exported to Arrow/Parquet).
    /// - If `EXPORT_PARQUET` flag IS set but the event does not override this method, this panics.
    ///   Types that are exported to Parquet MUST override and provide a concrete implementation.
    fn to_arrow(&self) -> Option<Box<dyn ArrowAttributesSerialize + '_>> {
        #[cfg(debug_assertions)]
        if self
            .output_flags()
            .contains(TelemetryOutputFlags::EXPORT_PARQUET)
        {
            panic!(
                "Missing Arrow serializer for event type \"{}\" (EXPORT_PARQUET set)",
                self.event_type()
            );
        }

        None
    }
}

/// A convenience trait for Arrow serialization of telemetry events. This is used
/// to ensure that both serialization and deserialization are implemented for types
/// and then used for blanket implementations as `to_arrow` in `AnyTelemetryEvent`
/// and the source of deserialization in the registry.
pub trait ArrowSerializableTelemetryEvent {
    /// The concrete Arrow-serializable record type produced/consumed by this event.
    type ArrowRecord<'a>: ArrowAttributesSerialize
    where
        Self: 'a;

    /// Serialize the event data to Arrow compatible record (used in arrow serialization)
    fn to_arrow_record(&self) -> Self::ArrowRecord<'_>;

    /// Deserialize from Arrow compatible record into a boxed trait object.
    /// This is a non-dispatchable method called on concrete types.
    fn from_arrow_record(record: &Self::ArrowRecord<'_>) -> Result<Self, String>
    where
        Self: Sized;
}

/// A convenience trait bundling the bounds for statically-typed telemetry events;
/// the statically-typed counterpart to `AnyTelemetryEvent`. Reduces boilerplate.
/// Implement this trait for all schema-defined (e.g. proto-generated) events,
/// which should be all non-internal events.
///
/// Any type that implements this trait automatically implements AnyTelemetryEvent.
pub trait StaticTelemetryEvent:
    Debug
    + Clone
    + Send
    + Sync
    + Any
    + Serialize
    + PartialEq
    + crate::StaticName
    + ArrowSerializableTelemetryEvent
    + 'static
{
    /// Category of record (envelope) this event should be recorded in.
    const RECORD_CATEGORY: TelemetryEventRecType;

    /// Flags that determine where a telemetry event should be exported and/or output.
    const OUTPUT_FLAGS: TelemetryOutputFlags;

    /// Returns a human-readable name/description for this event.
    ///
    /// This is used for span naming and display purposes and may use
    /// instance data to generate a more descriptive name.
    fn event_display_name(&self) -> String;

    /// Returns the status that should be associated with a span created for this event,
    /// if it can be determined from the event data.
    ///
    /// Events with `record_category() == TelemetryEventRecType::Log` should never
    /// return a status, as logs do not have statuses.
    ///
    /// Events with `record_category() == TelemetryEventRecType::Span` may return
    /// a status if the event data contains sufficient information to determine
    /// the aggregate status of the operation being represented by the span.
    ///
    /// Default implementation returns `None`.
    fn get_span_status(&self) -> Option<SpanStatus> {
        None
    }

    /// Returns code location associated with this event, if any.
    ///
    /// Default implementation returns `None`.
    fn code_location(&self) -> Option<RecordCodeLocation> {
        None
    }

    /// Called with code location based on emission call-site.
    /// If event stores code location - it can use the provided value to
    /// populate its location fields even if they were not set at construction time.
    ///
    /// Default implementation does nothing - types with location fields should override
    /// and implement the logic to fill in fields if they are empty.
    fn with_code_location(&mut self, _location: RecordCodeLocation) {}

    /// Returns a context that this event defines (if any) to be propagated
    /// to child spans and logs.
    ///
    /// Default implementation returns `None`.
    fn context(&self) -> Option<TelemetryContext> {
        None
    }

    /// Inject a provided context into this event (if supported).
    ///
    /// Default implementation does nothing.
    fn with_context(&mut self, _context: &TelemetryContext) {}

    /// Returns true if this event MAY contain sensitive data that should be scrubbed
    /// if the user has opted out of sensitive data collection.
    fn has_sensitive_data(&self) -> bool;

    /// Returns a clone of this event with sensitive data removed. `None` if
    /// the entire event should be dropped.
    ///
    /// Default implementation panics if `has_sensitive_data` is true.
    /// Types with sensitive data MUST override.
    ///
    /// SAFETY: This method MUST return a new instance of the same concrete type,
    /// just type-erased as a trait object. The returned value must be downcastable
    /// to the original type. This is used to implement sensitive data scrubbing logic.
    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        AnyTelemetryEvent::clone_without_sensitive_data(self)
    }
}

// Blanket implementation of AnyTelemetryEvent for types that implement StaticTelemetryEvent
impl<T: StaticTelemetryEvent> AnyTelemetryEvent for T {
    #[inline]
    fn event_type(&self) -> &'static str {
        T::FULL_NAME
    }

    fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
        self.event_type() == other.event_type()
            && other
                .as_any()
                .downcast_ref::<Self>()
                .is_some_and(|other| self == other)
    }

    fn event_display_name(&self) -> String {
        StaticTelemetryEvent::event_display_name(self)
    }

    fn get_span_status(&self) -> Option<SpanStatus> {
        StaticTelemetryEvent::get_span_status(self)
    }

    #[inline]
    fn record_category(&self) -> TelemetryEventRecType {
        T::RECORD_CATEGORY
    }

    #[inline]
    fn output_flags(&self) -> TelemetryOutputFlags {
        T::OUTPUT_FLAGS
    }

    fn code_location(&self) -> Option<RecordCodeLocation> {
        StaticTelemetryEvent::code_location(self)
    }

    fn with_code_location(&mut self, location: RecordCodeLocation) {
        StaticTelemetryEvent::with_code_location(self, location)
    }

    #[inline]
    fn context(&self) -> Option<TelemetryContext> {
        StaticTelemetryEvent::context(self)
    }

    #[inline]
    fn with_context(&mut self, context: &TelemetryContext) {
        StaticTelemetryEvent::with_context(self, context)
    }

    fn has_sensitive_data(&self) -> bool {
        StaticTelemetryEvent::has_sensitive_data(self)
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        StaticTelemetryEvent::clone_without_sensitive_data(self)
    }

    /// Helper for downcasting to concrete types.
    #[inline]
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// Helper for downcasting to concrete types (mutable).
    #[inline]
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    /// Clone the event as a boxed trait object.
    fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<JsonValue, String> {
        serde_json::to_value(self).map_err(|e| format!("Failed to serialize: {e}"))
    }

    fn to_arrow(&self) -> Option<Box<dyn ArrowAttributesSerialize + '_>> {
        Some(Box::new(self.to_arrow_record()))
    }
}
