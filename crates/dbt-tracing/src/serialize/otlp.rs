use opentelemetry::{
    Key, KeyValue, Value as OtelValue,
    context::Context as OtelContext,
    logs::{AnyValue, LogRecord, Logger, Severity as OtelSeverity},
    trace::{
        Link, SamplingResult, Span as _, SpanContext, SpanId, SpanKind, Status as OtelStatus,
        TraceContextExt, TraceFlags, TraceId, TraceState, Tracer,
    },
};
use opentelemetry_sdk::{logs::SdkLogger, trace::SdkTracer};
use opentelemetry_semantic_conventions::attribute::{CODE_FILE_PATH, CODE_LINE_NUMBER};
use std::collections::HashMap;

use crate::{
    LogRecordInfo, SeverityNumber, SpanEndInfo, SpanLinkInfo, SpanStatus, StatusCode,
    TelemetryAttributes,
};

/// Converts a [`SpanLinkInfo`] to an OpenTelemetry [`Link`].
fn span_link_to_otel(link: &SpanLinkInfo) -> Link {
    let span_context = SpanContext::new(
        TraceId::from(link.trace_id),
        SpanId::from(link.span_id),
        TraceFlags::SAMPLED,
        false,
        TraceState::NONE,
    );

    let attributes: Vec<KeyValue> = link
        .attributes
        .iter()
        .map(|(k, v)| KeyValue::new(Key::from(k.clone()), serde_json_value_to_otel(v)))
        .collect();

    // Link::new takes span_context, attributes, and dropped_attributes_count (0 in our case)
    Link::new(span_context, attributes, 0)
}

/// Exports a [`SpanEndInfo`] telemetry record using the provided OpenTelemetry tracer.
///
/// This function assumes the caller has already checked [`TelemetryOutputFlags`] and only
/// invokes it for records that should be sent to OTLP.
pub fn export_span(tracer: &SdkTracer, span_record: &SpanEndInfo) {
    let otel_trace_id = span_record.trace_id.into();
    let otel_span_id = span_record.span_id.into();

    // OTEL sdk doesn't allow "just" specifying the parent span id, so we
    // use this faked remote context to achieve that...
    let otel_parent_cx = span_record
        .parent_span_id
        .map(|parent_span_id| {
            OtelContext::new().with_remote_span_context(SpanContext::new(
                otel_trace_id,
                parent_span_id.into(),
                TraceFlags::SAMPLED,
                false,
                TraceState::NONE,
            ))
        })
        .unwrap_or_default();

    let span_attrs = telemetry_attributes_to_key_values(&span_record.attributes);

    // Convert span links to OpenTelemetry format
    let otel_links: Vec<Link> = span_record
        .links
        .as_ref()
        .map(|links| links.iter().map(span_link_to_otel).collect())
        .unwrap_or_default();

    // Create OpenTelemetry span
    let mut otel_span = tracer
        .span_builder(span_record.span_name.clone())
        // This forces all spans to be exported
        .with_sampling_result(SamplingResult {
            attributes: Default::default(),
            decision: opentelemetry::trace::SamplingDecision::RecordAndSample,
            trace_state: Default::default(),
        })
        .with_kind(SpanKind::Internal)
        .with_trace_id(otel_trace_id)
        .with_span_id(otel_span_id)
        .with_start_time(span_record.start_time_unix_nano)
        .with_attributes(span_attrs)
        .with_links(otel_links)
        .start_with_context(tracer, &otel_parent_cx);

    // Set span status as OK
    let status = span_status_to_otel(span_record.status.as_ref());
    if let Some(status) = status {
        otel_span.set_status(status);
    }

    otel_span.end_with_timestamp(span_record.end_time_unix_nano);
}

/// Exports a [`LogRecordInfo`] telemetry record using the provided OpenTelemetry logger.
///
/// This function assumes the caller has already checked [`TelemetryOutputFlags`] and only
/// invokes it for records that should be sent to OTLP.
pub fn export_log(logger: &SdkLogger, log_record: &LogRecordInfo) {
    // Create a new log record and populate it with data from LogRecordInfo
    let mut otel_log_record = logger.create_log_record();

    // Set the log basic attributes
    otel_log_record.set_severity_number(level_to_otel_severity(&log_record.severity_number));

    otel_log_record.set_severity_text(log_record.severity_number.as_str());

    // Message
    otel_log_record.set_body(AnyValue::from(log_record.body.clone()));

    // Set timestamp ourselves, since sdk only sets the observed timestamp.
    otel_log_record.set_timestamp(log_record.time_unix_nano);
    otel_log_record.set_observed_timestamp(log_record.time_unix_nano);

    let log_attrs = telemetry_attributes_to_any_values(&log_record.attributes);
    otel_log_record.set_event_name(log_record.attributes.event_type());
    otel_log_record.add_attributes(log_attrs);

    otel_log_record.set_trace_context(
        log_record.trace_id.into(),
        log_record
            .span_id
            .map(Into::into)
            .unwrap_or(SpanId::INVALID),
        Some(TraceFlags::SAMPLED),
    );
    logger.emit(otel_log_record);
}

fn span_status_to_otel(status: Option<&SpanStatus>) -> Option<OtelStatus> {
    status.and_then(|span_status| match span_status.code {
        StatusCode::Ok => Some(OtelStatus::Ok),
        StatusCode::Error => Some(OtelStatus::Error {
            description: span_status.message.clone().unwrap_or_default().into(),
        }),
        StatusCode::Unset => None,
    })
}

/// Convert our proto defined severity level to OpenTelemetry severity number.
/// Panics if `SeverityNumber::Unspecified` is provided.
const fn level_to_otel_severity(severity_number: &SeverityNumber) -> OtelSeverity {
    match severity_number {
        SeverityNumber::Unspecified => panic!("Do not use unspecified severity level!"),
        SeverityNumber::Trace => OtelSeverity::Trace,
        SeverityNumber::Debug => OtelSeverity::Debug,
        SeverityNumber::Info => OtelSeverity::Info,
        SeverityNumber::Warn => OtelSeverity::Warn,
        SeverityNumber::Error => OtelSeverity::Error,
    }
}

fn telemetry_attributes_to_key_values(attributes: &TelemetryAttributes) -> Vec<KeyValue> {
    telemetry_attributes_to_values(attributes, |k, v| {
        KeyValue::new(k.clone(), serde_json_value_to_otel(v))
    })
}

fn telemetry_attributes_to_any_values(attributes: &TelemetryAttributes) -> Vec<(Key, AnyValue)> {
    telemetry_attributes_to_values(attributes, |k, v| {
        (Key::from(k.clone()), serde_json_value_to_otel_any_value(v))
    })
}

fn telemetry_attributes_to_values<T, F>(attributes: &TelemetryAttributes, mapper: F) -> Vec<T>
where
    F: Fn(&String, &serde_json::Value) -> T,
{
    let mut otel_attrs = attributes
        .inner()
        .to_json()
        .ok()
        .and_then(|val| {
            val.as_object().map(|mapping| {
                mapping
                    .iter()
                    .filter_map(|(k, v)| {
                        if k.as_str() == "file" || k.as_str() == "line" {
                            return None;
                        }
                        Some(mapper(k, v))
                    })
                    .collect::<Vec<_>>()
            })
        })
        .unwrap_or_default();

    if let Some(location) = attributes.inner().code_location() {
        if let Some(file) = location.file.clone() {
            otel_attrs.push(mapper(
                &CODE_FILE_PATH.to_string(),
                &serde_json::Value::String(file),
            ));
        }

        if let Some(line) = location.line {
            otel_attrs.push(mapper(
                &CODE_LINE_NUMBER.to_string(),
                &serde_json::Value::Number(serde_json::Number::from(line)),
            ));
        }
    }

    otel_attrs
}

fn serde_json_value_to_otel(value: &serde_json::Value) -> OtelValue {
    match value {
        serde_json::Value::Bool(b) => OtelValue::from(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                OtelValue::from(i)
            } else if let Some(u) = n.as_u64() {
                if u > i64::MAX as u64 {
                    // If the number is too large for i64, we convert it to a string
                    OtelValue::from(u.to_string())
                } else {
                    // Otherwise, we can safely convert it to i64
                    OtelValue::from(u as i64)
                }
            } else if let Some(f) = n.as_f64() {
                OtelValue::from(f)
            } else {
                // Should not be reached
                OtelValue::from(n.to_string())
            }
        }
        serde_json::Value::String(s) => OtelValue::from(s.clone()),
        _ => value.to_string().into(),
    }
}

fn serde_json_value_to_otel_any_value(value: &serde_json::Value) -> AnyValue {
    match value {
        serde_json::Value::Bool(b) => AnyValue::from(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                AnyValue::from(i)
            } else if let Some(u) = n.as_u64() {
                if u > i64::MAX as u64 {
                    // If the number is too large for i64, we convert it to string
                    AnyValue::from(u.to_string())
                } else {
                    // Otherwise, we can safely convert it to i64
                    AnyValue::from(u as i64)
                }
            } else if let Some(f) = n.as_f64() {
                AnyValue::from(f)
            } else {
                // Should not be reached
                AnyValue::from(n.to_string())
            }
        }
        serde_json::Value::String(s) => AnyValue::from(s.clone()),
        serde_json::Value::Array(arr) => AnyValue::ListAny(Box::new(
            arr.iter()
                .map(serde_json_value_to_otel_any_value)
                .collect::<Vec<_>>(),
        )),
        serde_json::Value::Object(obj) => AnyValue::Map(Box::new(
            obj.iter()
                .map(|(k, v)| (Key::from(k.clone()), serde_json_value_to_otel_any_value(v)))
                .collect::<HashMap<_, _>>(),
        )),
        _ => AnyValue::from(value.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    use opentelemetry::Value as OtelValue;
    use opentelemetry::logs::LoggerProvider;
    use opentelemetry::logs::Severity as OtelSeverity;
    use opentelemetry::trace::{Status as OtelStatus, TracerProvider};
    use opentelemetry_sdk::{
        logs::{LogExporter, SdkLoggerProvider},
        trace::{SdkTracerProvider, SpanExporter},
    };
    use uuid::Uuid;

    use super::*;
    use crate::{
        AnyTelemetryEvent, LogRecordInfo, RecordCodeLocation, SeverityNumber, SpanStatus,
        TelemetryAttributes, TelemetryEventRecType, TelemetryOutputFlags,
    };

    #[derive(Debug, Clone)]
    struct DummySpanEvent;

    impl AnyTelemetryEvent for DummySpanEvent {
        fn event_type(&self) -> &'static str {
            "v1.internal.events.fusion.test.DummySpan"
        }

        fn event_display_name(&self) -> String {
            "dummy span".to_string()
        }

        fn record_category(&self) -> TelemetryEventRecType {
            TelemetryEventRecType::Span
        }

        fn output_flags(&self) -> TelemetryOutputFlags {
            TelemetryOutputFlags::EXPORT_OTLP
        }

        fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
            other.as_any().downcast_ref::<Self>().is_some()
        }

        fn has_sensitive_data(&self) -> bool {
            false
        }

        fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
            Box::new(self.clone())
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }

        fn code_location(&self) -> Option<RecordCodeLocation> {
            Some(RecordCodeLocation {
                file: Some("test_file".to_string()),
                line: Some(42),
                ..Default::default()
            })
        }

        fn to_json(&self) -> Result<serde_json::Value, String> {
            let mut map = serde_json::Map::new();
            map.insert("dummy".to_string(), serde_json::Value::from("value"));
            // Immitate that file/line are also serialized
            map.insert("file".to_string(), serde_json::Value::from("other_value"));
            map.insert("line".to_string(), serde_json::Value::from(100));
            Ok(serde_json::Value::Object(map))
        }
    }

    #[derive(Debug, Clone)]
    struct DummyLogEvent;

    impl AnyTelemetryEvent for DummyLogEvent {
        fn event_type(&self) -> &'static str {
            "v1.internal.events.fusion.test.DummyLog"
        }

        fn event_display_name(&self) -> String {
            "dummy log".to_string()
        }

        fn record_category(&self) -> TelemetryEventRecType {
            TelemetryEventRecType::Log
        }

        fn output_flags(&self) -> TelemetryOutputFlags {
            TelemetryOutputFlags::EXPORT_OTLP
        }

        fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
            other.as_any().downcast_ref::<Self>().is_some()
        }

        fn has_sensitive_data(&self) -> bool {
            false
        }

        fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
            Box::new(self.clone())
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }

        fn to_json(&self) -> Result<serde_json::Value, String> {
            let mut map = serde_json::Map::new();
            map.insert("code".to_string(), serde_json::Value::from(1));
            map.insert("message".to_string(), serde_json::Value::from("test"));
            Ok(serde_json::Value::Object(map))
        }
    }

    #[derive(Debug)]
    struct TestSpanExporter {
        spans: Arc<Mutex<Vec<opentelemetry_sdk::trace::SpanData>>>,
    }

    impl TestSpanExporter {
        fn new() -> (Self, Arc<Mutex<Vec<opentelemetry_sdk::trace::SpanData>>>) {
            let spans = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    spans: Arc::clone(&spans),
                },
                spans,
            )
        }
    }

    impl SpanExporter for TestSpanExporter {
        fn export(
            &self,
            batch: Vec<opentelemetry_sdk::trace::SpanData>,
        ) -> impl Future<Output = opentelemetry_sdk::error::OTelSdkResult> + Send {
            let spans = Arc::clone(&self.spans);
            async move {
                let mut guard = spans.lock().unwrap();
                guard.extend(batch);
                Ok(())
            }
        }
    }

    #[derive(Debug)]
    struct TestLogExporter {
        logs: Arc<Mutex<Vec<opentelemetry_sdk::logs::SdkLogRecord>>>,
    }

    impl TestLogExporter {
        fn new() -> (Self, Arc<Mutex<Vec<opentelemetry_sdk::logs::SdkLogRecord>>>) {
            let logs = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    logs: Arc::clone(&logs),
                },
                logs,
            )
        }
    }

    impl LogExporter for TestLogExporter {
        fn export(
            &self,
            batch: opentelemetry_sdk::logs::LogBatch<'_>,
        ) -> impl Future<Output = opentelemetry_sdk::error::OTelSdkResult> + Send {
            let logs = Arc::clone(&self.logs);
            async move {
                let mut guard = logs.lock().unwrap();
                for (record, _scope) in batch.iter() {
                    guard.push(record.clone());
                }
                Ok(())
            }
        }
    }

    #[test]
    fn export_span_emits_expected_data() {
        let attributes = TelemetryAttributes::new(Box::new(DummySpanEvent));
        let span_info = SpanEndInfo {
            trace_id: 1,
            span_id: 2,
            parent_span_id: Some(3),
            links: None,
            span_name: "dummy".to_string(),
            start_time_unix_nano: SystemTime::UNIX_EPOCH,
            end_time_unix_nano: SystemTime::UNIX_EPOCH,
            attributes,
            status: Some(SpanStatus::failed("oops")),
            severity_number: SeverityNumber::Warn,
            severity_text: "WARN".to_string(),
        };

        let (span_exporter, spans) = TestSpanExporter::new();
        let tracer_provider = SdkTracerProvider::builder()
            .with_simple_exporter(span_exporter)
            .build();
        let tracer = tracer_provider.tracer("test");

        export_span(&tracer, &span_info);

        tracer_provider.force_flush().unwrap();
        tracer_provider.shutdown().unwrap();

        let spans = spans.lock().unwrap();
        assert_eq!(spans.len(), 1);
        let span = &spans[0];

        assert_eq!(span.span_context.trace_id(), span_info.trace_id.into());
        assert_eq!(span.span_context.span_id(), span_info.span_id.into());
        assert_eq!(
            Some(span.parent_span_id),
            span_info.parent_span_id.map(Into::into)
        );
        assert_eq!(span.name, "dummy");
        assert_eq!(
            span.status,
            OtelStatus::Error {
                description: "oops".into()
            }
        );

        let has_dummy_attr = span.attributes.iter().any(|kv| {
            kv.key.as_str() == "dummy"
                && matches!(kv.value, OtelValue::String(ref s) if s.as_ref() == "value")
        });
        assert!(has_dummy_attr);

        // check file & line attributes from code_location, and they are coming
        // from code_location, not from to_json()
        let has_file_attr = span.attributes.iter().any(|kv| {
            kv.key.as_str() == CODE_FILE_PATH
                && matches!(kv.value, OtelValue::String(ref s) if s.as_ref() == "test_file")
        });
        assert!(has_file_attr);

        let has_line_attr = span.attributes.iter().any(|kv| {
            kv.key.as_str() == CODE_LINE_NUMBER && matches!(kv.value, OtelValue::I64(i) if i == 42)
        });
        assert!(has_line_attr);
    }

    #[test]
    fn export_log_emits_expected_data() {
        let attributes = TelemetryAttributes::new(Box::new(DummyLogEvent));
        let log_info = LogRecordInfo {
            trace_id: 11,
            span_id: Some(22),
            span_name: Some("dummy span".to_string()),
            event_id: Uuid::new_v4(),
            time_unix_nano: SystemTime::UNIX_EPOCH,
            severity_number: SeverityNumber::Info,
            severity_text: "INFO".to_string(),
            body: "hello".to_string(),
            attributes,
        };

        let (log_exporter, logs) = TestLogExporter::new();
        let logger_provider = SdkLoggerProvider::builder()
            .with_simple_exporter(log_exporter)
            .build();
        let logger = logger_provider.logger("test");

        export_log(&logger, &log_info);

        logger_provider.force_flush().unwrap();
        logger_provider.shutdown().unwrap();

        let logs_guard = logs.lock().unwrap();
        assert_eq!(logs_guard.len(), 1);
        let record = &logs_guard[0];

        assert_eq!(
            record.event_name(),
            Some("v1.internal.events.fusion.test.DummyLog")
        );
        assert_eq!(record.severity_number(), Some(OtelSeverity::Info));
        assert_eq!(record.severity_text(), Some("INFO"));

        let has_code = record.attributes_iter().any(|(key, value)| {
            key.as_str() == "code" && matches!(value, AnyValue::Int(value) if *value == 1)
        });
        assert!(has_code);

        let trace_context = record.trace_context().expect("trace context");
        assert_eq!(trace_context.trace_id, log_info.trace_id.into());
        assert_eq!(trace_context.span_id, log_info.span_id.unwrap().into());
    }

    #[test]
    fn export_span_with_links_emits_expected_data() {
        let attributes = TelemetryAttributes::new(Box::new(DummySpanEvent));

        // Create span links
        let mut link_attrs = std::collections::BTreeMap::new();
        link_attrs.insert(
            "link_key".to_string(),
            serde_json::from_str("\"link_value\"").unwrap(),
        );

        let links = vec![
            SpanLinkInfo {
                trace_id: 100,
                span_id: 200,
                attributes: link_attrs.clone(),
            },
            SpanLinkInfo {
                trace_id: 101,
                span_id: 201,
                attributes: std::collections::BTreeMap::new(),
            },
        ];

        let span_info = SpanEndInfo {
            trace_id: 1,
            span_id: 2,
            parent_span_id: Some(3),
            links: Some(links),
            span_name: "dummy_with_links".to_string(),
            start_time_unix_nano: SystemTime::UNIX_EPOCH,
            end_time_unix_nano: SystemTime::UNIX_EPOCH,
            attributes,
            status: Some(SpanStatus::succeeded()),
            severity_number: SeverityNumber::Info,
            severity_text: "INFO".to_string(),
        };

        let (span_exporter, spans) = TestSpanExporter::new();
        let tracer_provider = SdkTracerProvider::builder()
            .with_simple_exporter(span_exporter)
            .build();
        let tracer = tracer_provider.tracer("test");

        export_span(&tracer, &span_info);

        tracer_provider.force_flush().unwrap();
        tracer_provider.shutdown().unwrap();

        let spans = spans.lock().unwrap();
        assert_eq!(spans.len(), 1);
        let span = &spans[0];

        assert_eq!(span.span_context.trace_id(), span_info.trace_id.into());
        assert_eq!(span.span_context.span_id(), span_info.span_id.into());
        assert_eq!(span.name, "dummy_with_links");
        assert_eq!(span.status, OtelStatus::Ok);

        // Verify links
        assert_eq!(span.links.len(), 2);

        // Check first link (Link has span_context and attributes as fields, not methods)
        let link1 = &span.links[0];
        assert_eq!(link1.span_context.trace_id(), TraceId::from(100u128));
        assert_eq!(link1.span_context.span_id(), SpanId::from(200u64));

        // Verify the link has 1 attribute
        assert!(
            !link1.attributes.is_empty(),
            "Expected at least 1 attribute, found: {}",
            link1.attributes.len()
        );

        // Check that link_key exists with link_value
        let has_link_attr = link1.attributes.iter().any(|kv| {
            kv.key.as_str() == "link_key"
                && matches!(kv.value, OtelValue::String(ref s) if s.as_ref() == "link_value")
        });
        assert!(
            has_link_attr,
            "link_key attribute not found or has wrong value. Attributes: {:?}",
            link1.attributes
        );

        // Check second link (no attributes)
        let link2 = &span.links[1];
        assert_eq!(link2.span_context.trace_id(), TraceId::from(101u128));
        assert_eq!(link2.span_context.span_id(), SpanId::from(201u64));
        assert_eq!(link2.attributes.len(), 0);
    }
}
