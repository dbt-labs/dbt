use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

use crate::emit::{create_info_span, create_root_info_span, emit_info_event};
use crate::init::create_tracing_subcriber_with_layer;
use crate::layer::ConsumerLayer;
use crate::layers::otlp::{OTLPExporterLayer, OtlpResourceConfig};
use crate::test_support::mocks::{
    MockDynLogEvent, MockDynSpanEvent, MockRootSpanEvent, test_data_layer,
};
use crate::{LogRecordInfo, TelemetryOutputFlags};
use opentelemetry::{Key, KeyValue, Value as OtelValue, logs::AnyValue};
use opentelemetry_sdk as sdk;
use opentelemetry_semantic_conventions::resource::{SERVICE_NAME, SERVICE_VERSION};

type SharedSpans = Arc<Mutex<Vec<sdk::trace::SpanData>>>;
type SharedLogs = Arc<Mutex<Vec<sdk::logs::SdkLogRecord>>>;
type SharedResource = Arc<Mutex<Option<sdk::Resource>>>;

const TEST_SERVICE_NAME: &str = "test-service";
const TEST_SERVICE_VERSION: &str = "test-version";

fn test_otlp_resource_config() -> OtlpResourceConfig {
    OtlpResourceConfig::new(TEST_SERVICE_NAME, TEST_SERVICE_VERSION)
}

fn assert_resource_string_attribute(resource: &SharedResource, key: &'static str, expected: &str) {
    let resource = resource.lock().unwrap();
    let resource = resource.as_ref().expect("resource should be set");
    let value = resource
        .get(&Key::new(key))
        .unwrap_or_else(|| panic!("resource should contain {key}"));

    assert!(
        matches!(&value, OtelValue::String(s) if s.as_ref() == expected),
        "expected resource attribute {key}={expected}, got {value:?}"
    );
}

fn prepend_test_marker(record: &LogRecordInfo) -> Cow<'_, LogRecordInfo> {
    let mut record = record.clone();
    record.body = format!("preprocessed: {}", record.body);
    Cow::Owned(record)
}

#[derive(Debug)]
struct TestSpanExporter {
    pub spans: SharedSpans,
    pub resource: SharedResource,
}

impl TestSpanExporter {
    fn new() -> (Self, SharedSpans, SharedResource) {
        let spans = Arc::new(Mutex::new(Vec::new()));
        let resource = Arc::new(Mutex::new(None));
        (
            Self {
                spans: spans.clone(),
                resource: resource.clone(),
            },
            spans,
            resource,
        )
    }
}

impl sdk::trace::SpanExporter for TestSpanExporter {
    async fn export(&self, batch: Vec<sdk::trace::SpanData>) -> sdk::error::OTelSdkResult {
        let mut guard = self.spans.lock().unwrap();
        guard.extend(batch);
        Ok(())
    }

    fn set_resource(&mut self, resource: &sdk::Resource) {
        *self.resource.lock().unwrap() = Some(resource.clone());
    }
}

#[derive(Debug)]
struct TestLogExporter {
    pub logs: SharedLogs,
    pub resource: SharedResource,
}

impl TestLogExporter {
    fn new() -> (Self, SharedLogs, SharedResource) {
        let logs = Arc::new(Mutex::new(Vec::new()));
        let resource = Arc::new(Mutex::new(None));
        (
            Self {
                logs: logs.clone(),
                resource: resource.clone(),
            },
            logs,
            resource,
        )
    }
}

impl sdk::logs::LogExporter for TestLogExporter {
    fn export(
        &self,
        batch: sdk::logs::LogBatch<'_>,
    ) -> impl Future<Output = sdk::error::OTelSdkResult> + Send {
        let logs = self.logs.clone();
        async move {
            let mut guard = logs.lock().unwrap();
            for (rec, _scope) in batch.iter() {
                guard.push(rec.clone());
            }
            Ok(())
        }
    }

    fn set_resource(&mut self, resource: &sdk::Resource) {
        *self.resource.lock().unwrap() = Some(resource.clone());
    }
}

#[test]
fn test_otlp_layer_exports_only_marked_records() {
    let trace_id = rand::random::<u128>();

    // Create test exporters and share state
    let (trace_exporter, spans, trace_resource) = TestSpanExporter::new();
    let (log_exporter, logs, log_resource) = TestLogExporter::new();

    // Build OTLP layer with test exporters
    let otlp_layer = OTLPExporterLayer::new_for_tests(
        trace_exporter,
        log_exporter,
        test_otlp_resource_config()
            .with_resource_attributes([KeyValue::new("test.attribute", "present")]),
        None,
    );
    // Keep both provider handles alive across with_default and shut them down
    // explicitly after it returns. If the OTLP layer owns the last provider
    // handle, DefaultGuard teardown can drop the layer while tracing-core is
    // updating thread-local subscribers; provider Drop emits OpenTelemetry SDK
    // tracing events and can deadlock that teardown path.
    let trace_provider = otlp_layer.tracer_provider();
    let log_provider = otlp_layer.logger_provider();

    // Init telemetry using internal API allowing to set thread local subscriber.
    // This avoids collisions with other unit tests
    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            std::iter::once(Box::new(otlp_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    // Emit events under the thread-local subscriber
    tracing::subscriber::with_default(subscriber, || {
        let exportable_span = create_root_info_span(MockDynSpanEvent {
            name: "exportable".to_string(),
            flags: TelemetryOutputFlags::EXPORT_OTLP,
            ..Default::default()
        });

        exportable_span.in_scope(|| {
            emit_info_event(
                MockDynLogEvent {
                    code: 1,
                    flags: TelemetryOutputFlags::EXPORT_OTLP,
                    ..Default::default()
                },
                Some("included log"),
            );
            emit_info_event(
                MockDynLogEvent {
                    code: 2,
                    flags: TelemetryOutputFlags::EXPORT_JSONL, // Not OTLP-exportable
                    ..Default::default()
                },
                Some("excluded log"),
            );
        });

        // This span should not be exported to OTLP
        let _non_exportable_span = create_root_info_span(MockDynSpanEvent {
            name: "non_exportable".to_string(),
            flags: TelemetryOutputFlags::EXPORT_JSONL, // Not OTLP-exportable
            ..Default::default()
        });
    });

    // Shutdown telemetry to ensure all data is flushed to the file
    trace_provider
        .shutdown()
        .expect("Failed to shutdown telemetry");
    log_provider
        .shutdown()
        .expect("Failed to shutdown telemetry");

    // Validate we exported exactly 1 span and 1 log
    let exported_spans = spans.lock().unwrap().clone();
    let exported_logs = logs.lock().unwrap().clone();

    assert_eq!(exported_spans.len(), 1, "expected one OTLP-exported span");
    assert_eq!(exported_logs.len(), 1, "expected one OTLP-exported log");

    for resource in [&trace_resource, &log_resource] {
        assert_resource_string_attribute(resource, SERVICE_NAME, TEST_SERVICE_NAME);
        assert_resource_string_attribute(resource, SERVICE_VERSION, TEST_SERVICE_VERSION);
        assert_resource_string_attribute(resource, "test.attribute", "present");
    }

    // Validate span attributes include name=exportable
    let span = &exported_spans[0];
    assert_eq!(span.instrumentation_scope.name(), TEST_SERVICE_NAME);
    let has_name_attr = span.attributes.iter().any(|kv| {
        kv.key.as_str() == "name"
            && matches!(&kv.value, OtelValue::String(s) if s.as_ref() == "exportable")
    });
    assert!(
        has_name_attr,
        "exported span should contain attribute name=exportable"
    );

    // Validate log: event name and attributes include code=1
    let log = &exported_logs[0];
    assert_eq!(
        log.event_name(),
        Some("v1.public.events.fusion.dev.MockDynLogEvent"),
        "expected event name on log record"
    );
    let has_code_1 = log
        .attributes_iter()
        .any(|(k, v)| k.as_str() == "code" && matches!(v, AnyValue::Int(i) if *i == 1));
    assert!(has_code_1, "expected log attributes to contain code=1");
}

#[test]
fn test_otlp_configured_log_preprocessor_hook() {
    let trace_id = rand::random::<u128>();
    let (trace_exporter, _spans, _trace_resource) = TestSpanExporter::new();
    let (log_exporter, logs, _log_resource) = TestLogExporter::new();

    let otlp_layer = OTLPExporterLayer::new_for_tests(
        trace_exporter,
        log_exporter,
        test_otlp_resource_config(),
        Some(prepend_test_marker),
    );
    let trace_provider = otlp_layer.tracer_provider();
    let log_provider = otlp_layer.logger_provider();

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            std::iter::once(Box::new(otlp_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        emit_info_event(
            MockDynLogEvent {
                flags: TelemetryOutputFlags::EXPORT_OTLP,
                ..Default::default()
            },
            Some("red"),
        );
    });

    trace_provider
        .shutdown()
        .expect("Failed to shutdown telemetry");
    log_provider
        .shutdown()
        .expect("Failed to shutdown telemetry");

    let exported_logs = logs.lock().unwrap().clone();
    assert_eq!(exported_logs.len(), 1, "expected one OTLP-exported log");
    assert!(
        matches!(exported_logs[0].body(), Some(AnyValue::String(body)) if body.as_ref() == "preprocessed: red"),
        "expected OTLP log body to be preprocessed"
    );
}

#[test]
fn test_otlp_export_with_links() {
    // Test that links are exported to OTLP
    let trace_id = rand::random::<u128>();

    let (trace_exporter, spans, _trace_resource) = TestSpanExporter::new();
    let (log_exporter, _logs, _log_resource) = TestLogExporter::new();

    let otlp_layer = OTLPExporterLayer::new_for_tests(
        trace_exporter,
        log_exporter,
        test_otlp_resource_config(),
        None,
    );
    // Keep both provider handles alive across with_default and shut them down
    // explicitly after it returns. See test_otlp_layer_exports_only_marked_records.
    let trace_provider = otlp_layer.tracer_provider();
    let log_provider = otlp_layer.logger_provider();

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            std::iter::once(Box::new(otlp_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let span1 = create_root_info_span(MockDynSpanEvent {
            name: "span_1".to_string(),
            flags: TelemetryOutputFlags::EXPORT_OTLP,
            ..Default::default()
        });

        let span2 = create_root_info_span(MockDynSpanEvent {
            name: "span_2".to_string(),
            flags: TelemetryOutputFlags::EXPORT_OTLP,
            ..Default::default()
        });

        // span2 follows from span1
        span2.follows_from(&span1);
    });

    trace_provider
        .shutdown()
        .expect("Failed to shutdown telemetry");
    log_provider
        .shutdown()
        .expect("Failed to shutdown telemetry");

    let exported_spans = spans.lock().unwrap().clone();

    assert_eq!(exported_spans.len(), 2, "Should have exported 2 spans");

    // Find span2 (the one with links) - MockDynSpanEvent adds a prefix to the name
    let span2_data = exported_spans
        .iter()
        .find(|s| s.name.contains("span_2"))
        .unwrap_or_else(|| {
            panic!(
                "span2 not found in exported spans. Available spans: {:?}",
                exported_spans.iter().map(|s| &s.name).collect::<Vec<_>>()
            )
        });

    assert_eq!(
        span2_data.links.len(),
        1,
        "span2 should have exactly 1 link"
    );

    // Verify the link points to span1
    let span1_data = exported_spans
        .iter()
        .find(|s| s.name.contains("span_1"))
        .expect("span1 not found in exported spans");

    let link = &span2_data.links[0];
    assert_eq!(
        link.span_context.span_id(),
        span1_data.span_context.span_id(),
        "link should point to span1"
    );
    assert_eq!(
        link.span_context.trace_id(),
        span1_data.span_context.trace_id(),
        "link should have the same trace_id as span1"
    );
}

#[test]
fn test_otlp_export_includes_parent_span_id_on_root_span() {
    // Test that when a parent_span_id is provided, the root span is exported
    // to OTLP with that parent span ID set.
    let trace_id = rand::random::<u128>();
    let expected_parent_span_id: u64 = 0xdeadbeefcafebabe;

    let (trace_exporter, spans, _trace_resource) = TestSpanExporter::new();
    let (log_exporter, _logs, _log_resource) = TestLogExporter::new();

    let otlp_layer = OTLPExporterLayer::new_for_tests(
        trace_exporter,
        log_exporter,
        test_otlp_resource_config(),
        None,
    );
    // Keep both provider handles alive across with_default and shut them down
    // explicitly after it returns. See test_otlp_layer_exports_only_marked_records.
    let trace_provider = otlp_layer.tracer_provider();
    let log_provider = otlp_layer.logger_provider();

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            Some(expected_parent_span_id),
            false,
            std::iter::empty(),
            std::iter::once(Box::new(otlp_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let root_span = create_root_info_span(MockRootSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::EXPORT_OTLP,
            trace_id: Some(trace_id),
            parent_span_id: Some(expected_parent_span_id),
        });
        root_span.in_scope(|| {
            // Create a child span to verify parent-child relationships still work
            let _child = create_info_span(MockDynSpanEvent {
                name: "child".to_string(),
                flags: TelemetryOutputFlags::EXPORT_OTLP,
                ..Default::default()
            });
        });
    });

    trace_provider
        .shutdown()
        .expect("Failed to shutdown telemetry");
    log_provider
        .shutdown()
        .expect("Failed to shutdown telemetry");

    let exported_spans = spans.lock().unwrap().clone();

    // Should have 2 spans: root and child
    assert_eq!(exported_spans.len(), 2, "Should have exported 2 spans");

    // Find the root span by the mock root span name attribute.
    let root_span = exported_spans
        .iter()
        .find(|s| {
            s.attributes.iter().any(|kv| {
                kv.key.as_str() == "name"
                    && matches!(&kv.value, OtelValue::String(s) if s.as_ref() == "root")
            })
        })
        .expect("Should find root span");

    // Verify the root span has the expected parent_span_id
    let parent_span_id = root_span.parent_span_id;

    // Convert the expected parent span ID to the format used by OTLP
    let expected_span_id_bytes = expected_parent_span_id.to_be_bytes();
    let expected_otel_span_id = opentelemetry::trace::SpanId::from_bytes(expected_span_id_bytes);

    assert_eq!(
        parent_span_id, expected_otel_span_id,
        "Root span should have the provided parent_span_id"
    );

    // Find the child span and verify it has the root span as parent
    let child_span = exported_spans
        .iter()
        .find(|s| {
            s.attributes.iter().any(|kv| {
                kv.key.as_str() == "name"
                    && matches!(&kv.value, OtelValue::String(s) if s.as_ref() == "child")
            })
        })
        .expect("Should find child span");

    assert_eq!(
        child_span.parent_span_id,
        root_span.span_context.span_id(),
        "Child span should have root span as parent"
    );
}
