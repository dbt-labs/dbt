//! Telemetry record definitions for dbt Fusion.

use std::time::SystemTime;

use crate::{
    SeverityNumber, SpanStatus, TelemetryAttributes,
    serialize::envelope::{
        serialize_optional_span_id, serialize_span_id, serialize_timestamp, serialize_trace_id,
    },
};
use dbt_yaml::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use strum::EnumDiscriminants;
use uuid::Uuid;

/// Represents a linked span
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SpanLinkInfo {
    /// Unique identifier for a trace. All spans from the same trace share
    /// the same `trace_id`. 16-byte identifier stored as 32-character hex string (invalid if all zeroes).
    #[serde(serialize_with = "serialize_trace_id")]
    #[schemars(with = "String")]
    pub trace_id: u128,

    /// Unique identifier for a span within a trace, assigned when the span
    /// is created. 8-byte identifier stored as 16-character hex string (invalid if all zeroes).
    #[serde(serialize_with = "serialize_span_id")]
    #[schemars(with = "String")]
    pub span_id: u64,

    /// Arbitrary JSON payload associated with this link.
    pub attributes: std::collections::BTreeMap<String, JsonValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SpanStartInfo {
    /// Unique identifier for a trace. All spans from the same trace share
    /// the same `trace_id`. 16-byte identifier stored as 32-character hex string (invalid if all zeroes).
    #[serde(serialize_with = "serialize_trace_id")]
    #[schemars(with = "String")]
    pub trace_id: u128,

    /// Unique identifier for a span within a trace, assigned when the span
    /// is created. 8-byte identifier stored as 16-character hex string (invalid if all zeroes).
    #[serde(serialize_with = "serialize_span_id")]
    #[schemars(with = "String")]
    pub span_id: u64,

    /// A human-readable description of the span's operation.
    pub span_name: String,

    /// The `span_id` of this span's parent span. Empty for root spans.
    #[serde(serialize_with = "serialize_optional_span_id")]
    #[schemars(with = "Option<String>")]
    pub parent_span_id: Option<u64>,

    /// Links to other spans in the same or different traces.
    pub links: Option<Vec<SpanLinkInfo>>,

    /// Start time of the span as UNIX timestamp in nanoseconds.
    #[serde(serialize_with = "serialize_timestamp")]
    #[schemars(with = "String")]
    pub start_time_unix_nano: SystemTime,

    /// Severity level as a number (OpenTelemetry standard values).
    #[schemars(with = "i32")]
    pub severity_number: SeverityNumber,

    /// Severity level as text: "DEBUG", "INFO", "WARNING", "ERROR", "TRACE".
    pub severity_text: String,

    /// Structured attributes for this span.
    /// Serialized to json as: `{ trace_id: "...", ..., "event_type": "discriminator", "attributes": { ... } }`
    #[serde(flatten)]
    pub attributes: TelemetryAttributes,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SpanEndInfo {
    /// Unique identifier for a trace. All spans from the same trace share
    /// the same `trace_id`. 16-byte identifier stored as 32-character hex string (invalid if all zeroes).
    #[serde(serialize_with = "serialize_trace_id")]
    #[schemars(with = "String")]
    pub trace_id: u128,

    /// Unique identifier for a span within a trace, assigned when the span
    /// is created. 8-byte identifier stored as 16-character hex string (invalid if all zeroes).
    #[serde(serialize_with = "serialize_span_id")]
    #[schemars(with = "String")]
    pub span_id: u64,

    /// A human-readable description of the span's operation.
    pub span_name: String,

    /// The `span_id` of this span's parent span. Empty for root spans.
    #[serde(serialize_with = "serialize_optional_span_id")]
    #[schemars(with = "Option<String>")]
    pub parent_span_id: Option<u64>,

    /// Links to other spans in the same or different traces.
    pub links: Option<Vec<SpanLinkInfo>>,

    /// Start time of the span as UNIX timestamp in nanoseconds.
    #[serde(serialize_with = "serialize_timestamp")]
    #[schemars(with = "String")]
    pub start_time_unix_nano: SystemTime,

    /// End time of the span as UNIX timestamp in nanoseconds.
    #[serde(serialize_with = "serialize_timestamp")]
    #[schemars(with = "String")]
    pub end_time_unix_nano: SystemTime,

    /// Severity level as a number (OpenTelemetry standard values).
    #[schemars(with = "i32")]
    pub severity_number: SeverityNumber,

    /// Severity level as text: "DEBUG", "INFO", "WARNING", "ERROR", "TRACE".
    pub severity_text: String,

    /// Final status for this span. When not set, assumes unset status (code = 0).
    pub status: Option<SpanStatus>,

    /// Structured attributes for this span.
    /// Serialized to json as: `{ trace_id: "...", ..., "event_type": "discriminator", "attributes": { ... } }`
    #[serde(flatten)]
    pub attributes: TelemetryAttributes,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct LogRecordInfo {
    /// Unique identifier for a trace. All logs from the same trace share
    /// the same `trace_id`. 16-byte identifier stored as 32-character hex string (invalid if all zeroes).
    #[serde(serialize_with = "serialize_trace_id")]
    #[schemars(with = "String")]
    pub trace_id: u128,

    /// Unique identifier for the span active when the log is created.
    /// 8-byte identifier stored as 16-character hex string (invalid if all zeroes).
    #[serde(serialize_with = "serialize_optional_span_id")]
    #[schemars(with = "Option<String>")]
    pub span_id: Option<u64>,

    /// A human-readable description of the span's in which the log was created.
    pub span_name: Option<String>,

    /// Globally unique identifier for the log event.
    #[schemars(with = "String")]
    pub event_id: Uuid,

    /// Time when the event occurred as UNIX timestamp in nanoseconds.
    /// Value of 0 indicates unknown or missing timestamp.
    #[serde(serialize_with = "serialize_timestamp")]
    #[schemars(with = "String")]
    pub time_unix_nano: SystemTime,

    /// Severity level as a number (OpenTelemetry standard values).
    #[schemars(with = "i32")]
    pub severity_number: SeverityNumber,

    /// Severity level as text: "DEBUG", "INFO", "WARNING", "ERROR", "TRACE".
    pub severity_text: String,

    /// Human-readable message describing the event.
    pub body: String,

    /// Structured attributes for this log.
    /// Serialized to json as: `{ trace_id: "...", ..., "event_type": "discriminator", "attributes": { ... } }`
    #[serde(flatten)]
    pub attributes: TelemetryAttributes,
}

/// Represents a telemetry record which loosely follows OpenTelemetry
/// log and trace signal logical models & semantics (but not OTLP schema!)
/// and combines them under a single enum type.
///
/// This is a discriminated union on `record_type` field, which is not part of the OTLP schema.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema, EnumDiscriminants)]
#[serde(tag = "record_type")]
// The following derives a variant discriminator enum for the telemetry records,
// used for type-safe (de)serialization and matching.
#[strum_discriminants(derive(Serialize, Deserialize), name(TelemetryRecordType))]
pub enum TelemetryRecord {
    /// # Span Start
    /// Represents the start of a span in a trace.
    ///
    /// This is a partial-span record emitted as soon as the span is created.
    /// The corresponding `SpanEnd` event is guaranteed to have the same
    /// values for all same-named fields except attributes and
    SpanStart(SpanStartInfo),

    /// # Span
    /// Represents a completed span in a trace.
    ///
    /// This is a full-span record emitted when the span is completed.
    SpanEnd(SpanEndInfo),

    /// # Log Record
    ///
    /// Represents a log record, which is a structured point in time event that can be emitted
    /// during the execution of a span.
    LogRecord(LogRecordInfo),
}

impl TelemetryRecord {
    pub fn attributes(&self) -> &TelemetryAttributes {
        match self {
            TelemetryRecord::SpanStart(info) => &info.attributes,
            TelemetryRecord::SpanEnd(info) => &info.attributes,
            TelemetryRecord::LogRecord(info) => &info.attributes,
        }
    }

    pub fn as_ref(&self) -> TelemetryRecordRef<'_> {
        match self {
            TelemetryRecord::SpanStart(info) => TelemetryRecordRef::SpanStart(info),
            TelemetryRecord::SpanEnd(info) => TelemetryRecordRef::SpanEnd(info),
            TelemetryRecord::LogRecord(info) => TelemetryRecordRef::LogRecord(info),
        }
    }
}

// Provides a default discriminant so downstream record types that embed
// `TelemetryRecordType` can derive `Default`. The value is not used in practice,
// as `record_type` is always set explicitly during conversion.
#[allow(clippy::derivable_impls)]
impl Default for TelemetryRecordType {
    fn default() -> Self {
        TelemetryRecordType::LogRecord
    }
}

/// A reference to a telemetry record, used in tracing to avoiding cloning. Make sure
/// it matches the `TelemetryRecord` enum.
#[derive(Serialize)]
#[serde(tag = "record_type")]
pub enum TelemetryRecordRef<'a> {
    /// # Span Start
    /// Represents the start of a span in a trace.
    ///
    /// This is a partial-span record emitted as soon as the span is created.
    /// The corresponding `SpanEnd` event is guaranteed to have the same
    /// values for all same-named fields except attributes and
    SpanStart(&'a SpanStartInfo),

    /// # Span
    /// Represents a span in a trace.
    ///
    /// This is a full-span record emitted when the span is completed.
    SpanEnd(&'a SpanEndInfo),

    /// # Log Record
    ///
    /// Represents a log record, which is a structured point in time event that can be emitted
    /// during the execution of a span.
    LogRecord(&'a LogRecordInfo),
}
