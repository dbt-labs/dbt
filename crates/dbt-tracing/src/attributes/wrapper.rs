//! Wrapper type for arbitrary telemetry event data

use schemars::{
    JsonSchema,
    r#gen::SchemaGenerator,
    schema::{InstanceType, ObjectValidation, Schema, SchemaObject},
};
use serde::Serialize;
use std::{borrow::Cow, fmt::Debug};

use crate::{AnyTelemetryEvent, SpanStatus, TelemetryEventRecType, TelemetryOutputFlags};

/// Wrapper type that holds a boxed trait object for telemetry event data.
///
/// This wrapper allows using arbitrary trait implementations in telemetry records
/// and implements serde serialization/deserialization with proper flattening.
#[derive(Debug)]
pub struct TelemetryAttributes {
    inner: Box<dyn AnyTelemetryEvent>,
}

impl TelemetryAttributes {
    /// Create a new wrapper from a trait object.
    pub fn new(attr: Box<dyn AnyTelemetryEvent>) -> Self {
        Self { inner: attr }
    }

    /// Get a reference to the inner attribute.
    pub fn inner(&self) -> &dyn AnyTelemetryEvent {
        &*self.inner
    }

    /// Get a mutable reference to the inner attribute.
    pub fn inner_mut(&mut self) -> &mut dyn AnyTelemetryEvent {
        &mut *self.inner
    }

    /// Returns the unique event type identifier.
    ///
    /// Canonical format is the same as protobuf package names:
    /// `v1.public.events.fusion.<category>.<EventName>`
    ///
    /// Internal events should use `v1.internal.events.fusion.<category>.<EventName>`.
    pub fn event_type(&self) -> &'static str {
        self.inner.event_type()
    }

    /// Returns a human-readable name/description for this event.
    ///
    /// This is used for span naming and display purposes and may use
    /// instance data to generate a more descriptive name.
    pub fn event_display_name(&self) -> String {
        self.inner.event_display_name()
    }

    /// Returns the status that should be associated with a span created for this event,
    /// if it can be determined from the event data.
    ///
    /// Events with `record_category() == TelemetryEventRecType::Log` will never
    /// return a status, as logs do not have statuses.
    ///
    /// Events with `record_category() == TelemetryEventRecType::Span` may return
    /// a status if the event data contains sufficient information to determine
    /// the aggregate status of the operation being represented by the span.
    pub fn get_span_status(&self) -> Option<SpanStatus> {
        self.inner.get_span_status()
    }

    /// Returns the category of record (envelope) this event should be recorded in.
    pub fn record_category(&self) -> TelemetryEventRecType {
        self.inner.record_category()
    }

    /// Returns flags that determine where a telemetry event should be exported and/or output.
    pub fn output_flags(&self) -> TelemetryOutputFlags {
        self.inner.output_flags()
    }

    /// Returns true if this event MAY contain sensitive data that should be scrubbed
    /// if the user has opted out of sensitive data collection.
    pub fn has_sensitive_data(&self) -> bool {
        self.inner.has_sensitive_data()
    }

    /// Returns a clone of this event with sensitive data removed. `None` if
    /// the entire event should be dropped.
    pub fn clone_without_sensitive_data(&self) -> Option<Self> {
        let scrubbed = self.inner.clone_without_sensitive_data()?;

        // Ensure the returned type is the same as the original
        debug_assert_eq!(self.inner().as_any().type_id(), scrubbed.as_any().type_id());

        Some(Self::new(scrubbed))
    }

    /// Attempt to downcast the inner attribute to a concrete type.
    pub fn downcast_ref<T: AnyTelemetryEvent + 'static>(&self) -> Option<&T> {
        self.inner.as_any().downcast_ref::<T>()
    }

    /// Attempt to downcast the inner attribute to a concrete type (mutable).
    pub fn downcast_mut<T: AnyTelemetryEvent + 'static>(&mut self) -> Option<&mut T> {
        self.inner.as_any_mut().downcast_mut::<T>()
    }

    /// Returns `true` if the attributes event type is the same as `T`.
    pub fn is<T: AnyTelemetryEvent + 'static>(&self) -> bool {
        self.inner().as_any().is::<T>()
    }
}

impl Clone for TelemetryAttributes {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_box(),
        }
    }
}

impl PartialEq for TelemetryAttributes {
    fn eq(&self, other: &Self) -> bool {
        self.inner.event_eq(&*other.inner)
    }
}

impl<T: AnyTelemetryEvent + 'static> From<T> for TelemetryAttributes {
    fn from(attr: T) -> Self {
        Self {
            inner: Box::new(attr),
        }
    }
}

impl JsonSchema for TelemetryAttributes {
    fn schema_name() -> String {
        "TelemetryAttributes".to_string()
    }

    fn schema_id() -> Cow<'static, str> {
        Cow::Borrowed(concat!(module_path!(), "::TelemetryAttributes"))
    }

    fn json_schema(_gen: &mut SchemaGenerator) -> Schema {
        // We can't generate a full schema for the trait object, so we use a generic object schema
        let mut schema_obj = SchemaObject {
            instance_type: Some(InstanceType::Object.into()),
            ..Default::default()
        };

        // Add properties for event_type and attributes
        let mut properties = schemars::Map::new();
        properties.insert(
            "event_type".to_string(),
            Schema::Object(SchemaObject {
                instance_type: Some(InstanceType::String.into()),
                ..Default::default()
            }),
        );
        properties.insert(
            "attributes".to_string(),
            Schema::Object(SchemaObject {
                instance_type: Some(InstanceType::Object.into()),
                ..Default::default()
            }),
        );

        schema_obj.object = Some(Box::new(ObjectValidation {
            properties,
            required: schemars::Set::from_iter([
                "event_type".to_string(),
                "attributes".to_string(),
            ]),
            ..Default::default()
        }));

        Schema::Object(schema_obj)
    }
}

// Custom serialization that flattens the structure
impl Serialize for TelemetryAttributes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        // Create a map with event_type at the top level and attributes nested
        let mut map = serializer.serialize_map(Some(2))?;

        // Add the event_type field at the top level
        map.serialize_entry("event_type", &self.inner.event_type())?;

        // Serialize the inner attribute data under "attributes"
        // We need to use to_json() since the trait object can't be directly serialized
        let json_value = self.inner.to_json().map_err(serde::ser::Error::custom)?;
        map.serialize_entry("attributes", &json_value)?;

        map.end()
    }
}
