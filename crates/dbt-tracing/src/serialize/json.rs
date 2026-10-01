//! Registry-backed JSON deserialization for telemetry records.

use std::{collections::BTreeMap, time::SystemTime};

use serde::Deserialize;
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::{
    IndexedTelemetryDeserializeError, LogRecordInfo, PartialLogRecordInfo, PartialSpanEndInfo,
    PartialSpanStartInfo, PartialTelemetryRecord, SeverityNumber, SpanEndInfo, SpanLinkInfo,
    SpanStartInfo, SpanStatus, StatusCode, TelemetryAttributes, TelemetryDeserializeError,
    TelemetryEventRecType, TelemetryRecord,
    serialize::{
        deserialize::TelemetryDeserializeError as DeserializeError,
        envelope::{
            deserialize_optional_span_id, deserialize_span_id, deserialize_timestamp,
            deserialize_trace_id,
        },
        traits::{JsonRegistryLookup, TelemetryAttributeDeserializeError},
    },
};

pub fn deserialize_from_json_str<R>(
    input: &str,
    registry: &R,
) -> Result<TelemetryRecord, TelemetryDeserializeError>
where
    R: JsonRegistryLookup,
{
    let record: TelemetryRecordJson =
        serde_json::from_str(input).map_err(DeserializeError::invalid_envelope)?;
    record.into_telemetry_record(registry)
}

pub fn deserialize_from_json_value<R>(
    value: JsonValue,
    registry: &R,
) -> Result<TelemetryRecord, TelemetryDeserializeError>
where
    R: JsonRegistryLookup,
{
    let record: TelemetryRecordJson =
        serde_json::from_value(value).map_err(DeserializeError::invalid_envelope)?;
    record.into_telemetry_record(registry)
}

pub fn deserialize_json_lines<R>(
    input: &str,
    registry: &R,
) -> (Vec<TelemetryRecord>, Vec<IndexedTelemetryDeserializeError>)
where
    R: JsonRegistryLookup,
{
    let mut records = Vec::new();
    let mut errors = Vec::new();

    for (idx, line) in input.lines().enumerate() {
        match deserialize_from_json_str(line, registry) {
            Ok(record) => records.push(record),
            Err(err) => errors.push(IndexedTelemetryDeserializeError::new(idx + 1, err)),
        }
    }

    (records, errors)
}

#[derive(Deserialize)]
#[serde(tag = "record_type")]
enum TelemetryRecordJson {
    SpanStart(SpanStartJson),
    SpanEnd(SpanEndJson),
    LogRecord(LogRecordJson),
}

impl TelemetryRecordJson {
    fn into_telemetry_record<R>(
        self,
        registry: &R,
    ) -> Result<TelemetryRecord, TelemetryDeserializeError>
    where
        R: JsonRegistryLookup,
    {
        match self {
            Self::SpanStart(record) => record.into_telemetry_record(registry),
            Self::SpanEnd(record) => record.into_telemetry_record(registry),
            Self::LogRecord(record) => record.into_telemetry_record(registry),
        }
    }
}

#[derive(Deserialize)]
struct SpanStartJson {
    #[serde(deserialize_with = "deserialize_trace_id")]
    trace_id: u128,
    #[serde(deserialize_with = "deserialize_span_id")]
    span_id: u64,
    span_name: String,
    #[serde(default, deserialize_with = "deserialize_optional_span_id")]
    parent_span_id: Option<u64>,
    links: Option<Vec<SpanLinkJson>>,
    #[serde(deserialize_with = "deserialize_timestamp")]
    start_time_unix_nano: SystemTime,
    severity_number: SeverityNumber,
    severity_text: String,
    event_type: String,
    attributes: JsonValue,
}

impl SpanStartJson {
    fn into_telemetry_record<R>(
        self,
        registry: &R,
    ) -> Result<TelemetryRecord, TelemetryDeserializeError>
    where
        R: JsonRegistryLookup,
    {
        let SpanStartJson {
            trace_id,
            span_id,
            span_name,
            parent_span_id,
            links,
            start_time_unix_nano,
            severity_number,
            severity_text,
            event_type,
            attributes,
        } = self;

        let deserialized_attributes = match deserialize_attributes(
            registry,
            event_type.as_ref(),
            &attributes,
            TelemetryEventRecType::Span,
        ) {
            Ok(attributes) => attributes,
            Err(err) => {
                let record = PartialTelemetryRecord::SpanStart(PartialSpanStartInfo {
                    trace_id,
                    span_id,
                    span_name,
                    parent_span_id,
                    links: links.map(span_links_from_json),
                    start_time_unix_nano,
                    severity_number,
                    severity_text,
                    event_type,
                    attributes,
                });
                return Err(TelemetryDeserializeError::from_attribute_error(record, err));
            }
        };

        Ok(TelemetryRecord::SpanStart(SpanStartInfo {
            trace_id,
            span_id,
            span_name,
            parent_span_id,
            links: links.map(span_links_from_json),
            start_time_unix_nano,
            severity_number,
            severity_text,
            attributes: deserialized_attributes,
        }))
    }
}

#[derive(Deserialize)]
struct SpanEndJson {
    #[serde(deserialize_with = "deserialize_trace_id")]
    trace_id: u128,
    #[serde(deserialize_with = "deserialize_span_id")]
    span_id: u64,
    span_name: String,
    #[serde(default, deserialize_with = "deserialize_optional_span_id")]
    parent_span_id: Option<u64>,
    links: Option<Vec<SpanLinkJson>>,
    #[serde(deserialize_with = "deserialize_timestamp")]
    start_time_unix_nano: SystemTime,
    #[serde(deserialize_with = "deserialize_timestamp")]
    end_time_unix_nano: SystemTime,
    severity_number: SeverityNumber,
    severity_text: String,
    status: Option<SpanStatusJson>,
    event_type: String,
    attributes: JsonValue,
}

impl SpanEndJson {
    fn into_telemetry_record<R>(
        self,
        registry: &R,
    ) -> Result<TelemetryRecord, TelemetryDeserializeError>
    where
        R: JsonRegistryLookup,
    {
        let SpanEndJson {
            trace_id,
            span_id,
            span_name,
            parent_span_id,
            links,
            start_time_unix_nano,
            end_time_unix_nano,
            severity_number,
            severity_text,
            status,
            event_type,
            attributes,
        } = self;

        let deserialized_attributes = match deserialize_attributes(
            registry,
            event_type.as_ref(),
            &attributes,
            TelemetryEventRecType::Span,
        ) {
            Ok(attributes) => attributes,
            Err(err) => {
                let record = PartialTelemetryRecord::SpanEnd(PartialSpanEndInfo {
                    trace_id,
                    span_id,
                    span_name,
                    parent_span_id,
                    links: links.map(span_links_from_json),
                    start_time_unix_nano,
                    end_time_unix_nano,
                    severity_number,
                    severity_text,
                    status: status.map(Into::into),
                    event_type,
                    attributes,
                });
                return Err(TelemetryDeserializeError::from_attribute_error(record, err));
            }
        };

        Ok(TelemetryRecord::SpanEnd(SpanEndInfo {
            trace_id,
            span_id,
            span_name,
            parent_span_id,
            links: links.map(span_links_from_json),
            start_time_unix_nano,
            end_time_unix_nano,
            severity_number,
            severity_text,
            status: status.map(Into::into),
            attributes: deserialized_attributes,
        }))
    }
}

#[derive(Deserialize)]
struct LogRecordJson {
    #[serde(deserialize_with = "deserialize_trace_id")]
    trace_id: u128,
    #[serde(default, deserialize_with = "deserialize_optional_span_id")]
    span_id: Option<u64>,
    span_name: Option<String>,
    event_id: Uuid,
    #[serde(deserialize_with = "deserialize_timestamp")]
    time_unix_nano: SystemTime,
    severity_number: SeverityNumber,
    severity_text: String,
    body: String,
    event_type: String,
    attributes: JsonValue,
}

impl LogRecordJson {
    fn into_telemetry_record<R>(
        self,
        registry: &R,
    ) -> Result<TelemetryRecord, TelemetryDeserializeError>
    where
        R: JsonRegistryLookup,
    {
        let LogRecordJson {
            trace_id,
            span_id,
            span_name,
            event_id,
            time_unix_nano,
            severity_number,
            severity_text,
            body,
            event_type,
            attributes,
        } = self;

        let deserialized_attributes = match deserialize_attributes(
            registry,
            event_type.as_ref(),
            &attributes,
            TelemetryEventRecType::Log,
        ) {
            Ok(attributes) => attributes,
            Err(err) => {
                let record = PartialTelemetryRecord::LogRecord(PartialLogRecordInfo {
                    trace_id,
                    span_id,
                    span_name,
                    event_id,
                    time_unix_nano,
                    severity_number,
                    severity_text,
                    body,
                    event_type,
                    attributes,
                });
                return Err(TelemetryDeserializeError::from_attribute_error(record, err));
            }
        };

        Ok(TelemetryRecord::LogRecord(LogRecordInfo {
            trace_id,
            span_id,
            span_name,
            event_id,
            time_unix_nano,
            severity_number,
            severity_text,
            body,
            attributes: deserialized_attributes,
        }))
    }
}

#[derive(Deserialize)]
struct SpanStatusJson {
    message: Option<String>,
    code: StatusCode,
}

impl From<SpanStatusJson> for SpanStatus {
    fn from(status: SpanStatusJson) -> Self {
        Self {
            message: status.message,
            code: status.code,
        }
    }
}

#[derive(Deserialize)]
struct SpanLinkJson {
    #[serde(deserialize_with = "deserialize_trace_id")]
    trace_id: u128,
    #[serde(deserialize_with = "deserialize_span_id")]
    span_id: u64,
    attributes: BTreeMap<String, JsonValue>,
}

fn span_links_from_json(links: Vec<SpanLinkJson>) -> Vec<SpanLinkInfo> {
    links
        .into_iter()
        .map(|link| SpanLinkInfo {
            trace_id: link.trace_id,
            span_id: link.span_id,
            attributes: link.attributes,
        })
        .collect()
}

fn deserialize_attributes<R>(
    registry: &R,
    event_type: &str,
    attributes: &JsonValue,
    expected_category: TelemetryEventRecType,
) -> Result<TelemetryAttributes, TelemetryAttributeDeserializeError>
where
    R: JsonRegistryLookup,
{
    let event = registry.deserialize_json_attributes(event_type, attributes)?;
    debug_assert_eq!(
        event.record_category(),
        expected_category,
        "event type \"{event_type}\" deserialized to the wrong record category"
    );

    Ok(TelemetryAttributes::new(event))
}
