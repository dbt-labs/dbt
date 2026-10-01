use std::{any::Any, collections::BTreeMap, time::SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::{
    AnyTelemetryEvent, LogRecordInfo, PartialTelemetryRecord, SeverityNumber, SpanEndInfo,
    SpanLinkInfo, SpanStartInfo, SpanStatus, StatusCode, TelemetryAttributes,
    TelemetryDeserializeError, TelemetryEventRecType, TelemetryOutputFlags, TelemetryRecord,
    TelemetryRecordRef,
    serialize::{
        json::{deserialize_from_json_str, deserialize_from_json_value, deserialize_json_lines},
        traits::{JsonRegistryLookup, TelemetryAttributeDeserializeError},
    },
};

const MOCK_SPAN_EVENT_TYPE: &str = "v1.public.events.fusion.test.MockJsonSpanEvent";
const MOCK_LOG_EVENT_TYPE: &str = "v1.public.events.fusion.test.MockJsonLogEvent";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MockJsonSpanEvent {
    name: String,
    count: u64,
}

impl AnyTelemetryEvent for MockJsonSpanEvent {
    fn event_type(&self) -> &'static str {
        MOCK_SPAN_EVENT_TYPE
    }

    fn event_display_name(&self) -> String {
        self.name.clone()
    }

    fn record_category(&self) -> TelemetryEventRecType {
        TelemetryEventRecType::Span
    }

    fn output_flags(&self) -> TelemetryOutputFlags {
        TelemetryOutputFlags::EXPORT_JSONL
    }

    fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| self == other)
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self).map_err(|err| format!("Failed to serialize: {err}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MockJsonLogEvent {
    message_key: String,
    code: u32,
}

impl AnyTelemetryEvent for MockJsonLogEvent {
    fn event_type(&self) -> &'static str {
        MOCK_LOG_EVENT_TYPE
    }

    fn event_display_name(&self) -> String {
        self.message_key.clone()
    }

    fn record_category(&self) -> TelemetryEventRecType {
        TelemetryEventRecType::Log
    }

    fn output_flags(&self) -> TelemetryOutputFlags {
        TelemetryOutputFlags::EXPORT_JSONL
    }

    fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|other| self == other)
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self).map_err(|err| format!("Failed to serialize: {err}"))
    }
}

struct MockJsonRegistry;

impl JsonRegistryLookup for MockJsonRegistry {
    fn deserialize_json_attributes(
        &self,
        event_type: &str,
        attributes: &serde_json::Value,
    ) -> Result<Box<dyn AnyTelemetryEvent>, TelemetryAttributeDeserializeError> {
        match event_type {
            MOCK_SPAN_EVENT_TYPE => MockJsonSpanEvent::deserialize(attributes)
                .map(|event| Box::new(event) as Box<dyn AnyTelemetryEvent>)
                .map_err(|err| {
                    TelemetryAttributeDeserializeError::malformed(format!(
                        "failed to deserialize span event: {err}"
                    ))
                }),
            MOCK_LOG_EVENT_TYPE => MockJsonLogEvent::deserialize(attributes)
                .map(|event| Box::new(event) as Box<dyn AnyTelemetryEvent>)
                .map_err(|err| {
                    TelemetryAttributeDeserializeError::malformed(format!(
                        "failed to deserialize log event: {err}"
                    ))
                }),
            _ => Err(TelemetryAttributeDeserializeError::unknown_event_type(
                event_type,
            )),
        }
    }
}

#[test]
fn telemetry_record_ref_json_roundtrips_for_span_start_span_end_and_log_record() {
    let span_start = span_start_record();
    let span_end = span_end_record();
    let log_record = log_record();

    assert_eq!(
        deserialize_record_ref(TelemetryRecordRef::SpanStart(&span_start)),
        TelemetryRecord::SpanStart(span_start)
    );
    assert_eq!(
        deserialize_record_ref(TelemetryRecordRef::SpanEnd(&span_end)),
        TelemetryRecord::SpanEnd(span_end)
    );
    assert_eq!(
        deserialize_record_ref(TelemetryRecordRef::LogRecord(&log_record)),
        TelemetryRecord::LogRecord(log_record)
    );
}

#[test]
fn unknown_event_type_fails() {
    let mut value = span_start_json_value();
    let unknown_event_type = "v1.public.events.fusion.test.Unknown";
    value["event_type"] = json!(unknown_event_type);

    let err = deserialize_from_json_value(value, &MockJsonRegistry).unwrap_err();

    let TelemetryDeserializeError::UnknownEventType { record } = err else {
        panic!("expected unknown event type error");
    };
    let PartialTelemetryRecord::SpanStart(record) = record.as_ref() else {
        panic!("expected span start partial record");
    };
    assert_eq!(record.event_type, unknown_event_type);
    assert_eq!(record.attributes["name"], json!("compile"));
}

#[test]
fn missing_attributes_fails() {
    let mut value = span_start_json_value();
    value.as_object_mut().unwrap().remove("attributes");

    let err = deserialize_from_json_value(value, &MockJsonRegistry).unwrap_err();

    assert!(matches!(
        err,
        TelemetryDeserializeError::InvalidEnvelope { ref message }
            if message.contains("missing field `attributes`")
    ));
}

#[test]
fn known_malformed_event_fails_with_partial_record() {
    let mut value = span_start_json_value();
    value["attributes"] = json!({
        "name": "compile",
    });

    let err = deserialize_from_json_value(value, &MockJsonRegistry).unwrap_err();

    let TelemetryDeserializeError::MalformedEvent { record, message } = err else {
        panic!("expected malformed event error");
    };
    let PartialTelemetryRecord::SpanStart(record) = record.as_ref() else {
        panic!("expected span start partial record");
    };
    assert_eq!(record.event_type, MOCK_SPAN_EVENT_TYPE);
    assert_eq!(record.attributes["name"], json!("compile"));
    assert!(message.contains("failed to deserialize span event"));
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "wrong record category")]
fn span_record_with_log_attributes_fails_category_validation() {
    let mut value = span_start_json_value();
    value["event_type"] = json!(MOCK_LOG_EVENT_TYPE);
    value["attributes"] = json!({
        "message_key": "E001",
        "code": 500,
    });

    let _ = deserialize_from_json_value(value, &MockJsonRegistry);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "wrong record category")]
fn log_record_with_span_attributes_fails_category_validation() {
    let mut value = log_record_json_value();
    value["event_type"] = json!(MOCK_SPAN_EVENT_TYPE);
    value["attributes"] = json!({
        "name": "compile",
        "count": 2,
    });

    let _ = deserialize_from_json_value(value, &MockJsonRegistry);
}

#[test]
fn invalid_trace_id_or_span_id_fails() {
    let mut invalid_trace_id = span_start_json_value();
    invalid_trace_id["trace_id"] = json!("not-hex");
    let err = deserialize_from_json_value(invalid_trace_id, &MockJsonRegistry).unwrap_err();
    assert!(matches!(
        err,
        TelemetryDeserializeError::InvalidEnvelope { ref message }
            if message.contains("invalid trace_id")
    ));

    let mut invalid_span_id = span_start_json_value();
    invalid_span_id["span_id"] = json!("not-hex");
    let err = deserialize_from_json_value(invalid_span_id, &MockJsonRegistry).unwrap_err();
    assert!(matches!(
        err,
        TelemetryDeserializeError::InvalidEnvelope { ref message }
            if message.contains("invalid span_id")
    ));
}

#[test]
fn deserialize_json_lines_collects_invalid_line_errors() {
    let valid =
        serde_json::to_string(&TelemetryRecordRef::SpanStart(&span_start_record())).unwrap();
    let mut invalid = span_start_json_value();
    invalid["span_id"] = json!("not-hex");

    let (records, errors) = deserialize_json_lines(
        &format!("{valid}\n{}", serde_json::to_string(&invalid).unwrap()),
        &MockJsonRegistry,
    );

    assert_eq!(
        records,
        vec![TelemetryRecord::SpanStart(span_start_record())]
    );
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].index(), 2);
    assert!(matches!(
        errors[0].error(),
        TelemetryDeserializeError::InvalidEnvelope { message }
            if message.contains("invalid span_id")
    ));
}

fn deserialize_record_ref(record: TelemetryRecordRef<'_>) -> TelemetryRecord {
    let json = serde_json::to_string(&record).unwrap();
    deserialize_from_json_str(&json, &MockJsonRegistry).unwrap()
}

fn span_start_json_value() -> JsonValue {
    serde_json::to_value(TelemetryRecordRef::SpanStart(&span_start_record())).unwrap()
}

fn log_record_json_value() -> JsonValue {
    serde_json::to_value(TelemetryRecordRef::LogRecord(&log_record())).unwrap()
}

fn span_start_record() -> SpanStartInfo {
    SpanStartInfo {
        trace_id: 42,
        span_id: 7,
        span_name: "mock span start".to_string(),
        parent_span_id: Some(3),
        links: Some(span_links()),
        start_time_unix_nano: nanos(12345),
        severity_number: SeverityNumber::Info,
        severity_text: "INFO".to_string(),
        attributes: span_attributes("compile", 2),
    }
}

fn span_end_record() -> SpanEndInfo {
    SpanEndInfo {
        trace_id: 42,
        span_id: 7,
        span_name: "mock span end".to_string(),
        parent_span_id: Some(3),
        links: Some(span_links()),
        start_time_unix_nano: nanos(12345),
        end_time_unix_nano: nanos(67890),
        severity_number: SeverityNumber::Warn,
        severity_text: "WARN".to_string(),
        status: Some(SpanStatus {
            message: Some("boom".to_string()),
            code: StatusCode::Error,
        }),
        attributes: span_attributes("compile", 2),
    }
}

fn log_record() -> LogRecordInfo {
    LogRecordInfo {
        trace_id: 42,
        span_id: Some(7),
        span_name: Some("mock span".to_string()),
        event_id: Uuid::parse_str("67e55044-10b1-426f-9247-bb680e5fe0c8").unwrap(),
        time_unix_nano: nanos(13579),
        severity_number: SeverityNumber::Error,
        severity_text: "ERROR".to_string(),
        body: "mock body".to_string(),
        attributes: TelemetryAttributes::new(Box::new(MockJsonLogEvent {
            message_key: "E001".to_string(),
            code: 500,
        })),
    }
}

fn span_links() -> Vec<SpanLinkInfo> {
    Vec::from([SpanLinkInfo {
        trace_id: 99,
        span_id: 5,
        attributes: BTreeMap::from([("linked".to_string(), json!(true))]),
    }])
}

fn span_attributes(name: &str, count: u64) -> TelemetryAttributes {
    TelemetryAttributes::new(Box::new(MockJsonSpanEvent {
        name: name.to_string(),
        count,
    }))
}

fn nanos(nanos: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(nanos)
}
