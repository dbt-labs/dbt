//! Library-shaped serialization traits.

use arrow_schema::Fields;
use serde::{Deserialize, Serialize};

use crate::attributes::AnyTelemetryEvent;

/// Object-safe serialization boundary for Arrow event attributes.
pub trait ArrowAttributesSerialize: erased_serde::Serialize {}

impl<T> ArrowAttributesSerialize for T where T: Serialize {}

erased_serde::serialize_trait_object!(ArrowAttributesSerialize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelemetryAttributeDeserializeError {
    UnknownEventType { event_type: String },
    Malformed { message: String },
}

impl TelemetryAttributeDeserializeError {
    pub fn unknown_event_type(event_type: impl Into<String>) -> Self {
        Self::UnknownEventType {
            event_type: event_type.into(),
        }
    }

    pub fn malformed(message: impl Into<String>) -> Self {
        Self::Malformed {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TelemetryAttributeDeserializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownEventType { event_type } => {
                write!(f, "unknown event type \"{event_type}\"")
            }
            Self::Malformed { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for TelemetryAttributeDeserializeError {}

/// Registry lookup boundary for typed Arrow event deserialization.
pub trait ArrowRegistryLookup {
    type ArrowAttributes<'a>: Deserialize<'a> + Serialize;

    /// Returns the Arrow struct fields for `Self::ArrowAttributes`.
    ///
    /// `serde_arrow` serializes the dynamic Arrow attribute payload by matching
    /// this schema to the concrete associated `ArrowAttributes` type above. The
    /// fields returned here therefore define the storage contract for that
    /// concrete type, not an independent projection. Implementations must update
    /// this schema whenever the associated attributes type changes, including
    /// field names, field order, data types, nullability, and Arrow extension
    /// metadata.
    ///
    /// The enclosing Arrow record schema embeds these fields as its
    /// `attributes` struct. Schema construction is intentionally left to callers
    /// through `TelemetryArrowSchemas::new::<Registry>()` so long-lived writers
    /// can cache the result instead of relying on a crate-global schema.
    fn arrow_attributes_fields() -> Fields;

    fn deserialize_arrow_attributes(
        &self,
        event_type: &str,
        attributes: &Self::ArrowAttributes<'_>,
    ) -> Result<Box<dyn AnyTelemetryEvent>, TelemetryAttributeDeserializeError>;
}

/// Registry lookup boundary for typed JSON event deserialization.
pub trait JsonRegistryLookup {
    fn deserialize_json_attributes(
        &self,
        event_type: &str,
        attributes: &serde_json::Value,
    ) -> Result<Box<dyn AnyTelemetryEvent>, TelemetryAttributeDeserializeError>;
}
