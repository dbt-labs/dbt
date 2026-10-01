//! Shared deserialization error and partial-record types.

use std::time::SystemTime;

use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::serialize::traits::TelemetryAttributeDeserializeError;
use crate::{SeverityNumber, SpanLinkInfo, SpanStatus};

#[derive(Debug, Clone, PartialEq)]
pub struct PartialSpanStartInfo {
    pub trace_id: u128,
    pub span_id: u64,
    pub span_name: String,
    pub parent_span_id: Option<u64>,
    pub links: Option<Vec<SpanLinkInfo>>,
    pub start_time_unix_nano: SystemTime,
    pub severity_number: SeverityNumber,
    pub severity_text: String,
    pub event_type: String,
    pub attributes: JsonValue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartialSpanEndInfo {
    pub trace_id: u128,
    pub span_id: u64,
    pub span_name: String,
    pub parent_span_id: Option<u64>,
    pub links: Option<Vec<SpanLinkInfo>>,
    pub start_time_unix_nano: SystemTime,
    pub end_time_unix_nano: SystemTime,
    pub severity_number: SeverityNumber,
    pub severity_text: String,
    pub status: Option<SpanStatus>,
    pub event_type: String,
    pub attributes: JsonValue,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartialLogRecordInfo {
    pub trace_id: u128,
    pub span_id: Option<u64>,
    pub span_name: Option<String>,
    pub event_id: Uuid,
    pub time_unix_nano: SystemTime,
    pub severity_number: SeverityNumber,
    pub severity_text: String,
    pub body: String,
    pub event_type: String,
    pub attributes: JsonValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PartialTelemetryRecord {
    SpanStart(PartialSpanStartInfo),
    SpanEnd(PartialSpanEndInfo),
    LogRecord(PartialLogRecordInfo),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TelemetryDeserializeError {
    /// The record envelope could not be decoded or validated.
    ///
    /// Examples include malformed JSON, missing required fields, invalid trace
    /// or span IDs, invalid span link payloads, invalid event IDs, and invalid
    /// enum values such as severity numbers.
    InvalidEnvelope { message: String },
    /// The envelope was valid, but the registry did not know the event type.
    UnknownEventType { record: Box<PartialTelemetryRecord> },
    /// The envelope was valid and the event type was known, but the event
    /// attributes could not be deserialized into that known event type.
    MalformedEvent {
        record: Box<PartialTelemetryRecord>,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexedTelemetryDeserializeError {
    /// One-based line number for JSONL inputs or row number for Arrow batches.
    index: usize,
    error: TelemetryDeserializeError,
}

impl IndexedTelemetryDeserializeError {
    pub(crate) fn new(index: usize, error: TelemetryDeserializeError) -> Self {
        Self { index, error }
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn error(&self) -> &TelemetryDeserializeError {
        &self.error
    }
}

impl TelemetryDeserializeError {
    pub(crate) fn invalid_envelope(message: impl ToString) -> Self {
        Self::InvalidEnvelope {
            message: message.to_string(),
        }
    }

    pub(crate) fn from_attribute_error(
        record: PartialTelemetryRecord,
        err: TelemetryAttributeDeserializeError,
    ) -> Self {
        match err {
            TelemetryAttributeDeserializeError::UnknownEventType { .. } => Self::UnknownEventType {
                record: Box::new(record),
            },
            TelemetryAttributeDeserializeError::Malformed { message } => Self::MalformedEvent {
                record: Box::new(record),
                message,
            },
        }
    }
}

fn partial_record_event_type(record: &PartialTelemetryRecord) -> &str {
    match record {
        PartialTelemetryRecord::SpanStart(record) => &record.event_type,
        PartialTelemetryRecord::SpanEnd(record) => &record.event_type,
        PartialTelemetryRecord::LogRecord(record) => &record.event_type,
    }
}

impl std::fmt::Display for TelemetryDeserializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEnvelope { message } => write!(f, "invalid telemetry envelope: {message}"),
            Self::UnknownEventType { record } => {
                write!(
                    f,
                    "unknown telemetry event type \"{}\"",
                    partial_record_event_type(record)
                )
            }
            Self::MalformedEvent { record, message } => write!(
                f,
                "malformed telemetry event \"{}\": {message}",
                partial_record_event_type(record)
            ),
        }
    }
}

impl std::error::Error for TelemetryDeserializeError {}

impl std::fmt::Display for IndexedTelemetryDeserializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "telemetry deserialization error at row {}: {}",
            self.index, self.error
        )
    }
}

impl std::error::Error for IndexedTelemetryDeserializeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}
