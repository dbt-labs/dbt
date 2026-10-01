use std::{panic::Location, sync::Arc};

use crate::{
    LogRecordInfo, SeverityNumber, SpanEndInfo, SpanStartInfo, TelemetryAttributes,
    TelemetryOutputFlags,
    data_provider::DataProvider,
    emit::{create_info_span, create_root_info_span, emit_info_event},
    init::create_tracing_subcriber_with_layer,
    layer::ConsumerLayer,
    layer::TelemetryConsumer,
};

use super::mocks::{
    MockDynLogEvent, MockDynSpanEvent, MockRootSpanEvent, MockUnknown, TestLayer,
    TestTelemetryContext, test_data_layer,
};

#[test]
fn test_emit_event_and_apply_context() {
    // Initialize tracing with a custom layer to capture events
    let trace_id = rand::random::<u128>();

    let (test_layer, _, span_ends, log_records) = TestLayer::new();

    // Init telemetry using internal API allowing to set thread local subscriber.
    // This avoids collisions with other unit tests
    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    let mut test_attrs: TelemetryAttributes = MockDynLogEvent {
        code: 42,
        flags: TelemetryOutputFlags::ALL,
        ..Default::default()
    }
    .into();

    let expected_context = TestTelemetryContext {
        workflow_name: "mock-workflow".to_string(),
        attempt: 7,
    };
    let mut mock_log_location = Location::caller();

    tracing::subscriber::with_default(subscriber, || {
        let _rs = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .entered();

        let child_span = create_info_span(MockDynSpanEvent {
            name: "context-provider".to_string(),
            flags: TelemetryOutputFlags::ALL,
            context: Some(expected_context.clone()),
            ..Default::default()
        });
        child_span.in_scope(|| {
            mock_log_location = Location::caller();
            emit_info_event(test_attrs.clone(), Some("Test info event"));
        });
    });

    let log_records = {
        let lr = log_records.lock().expect("Should have no locks");
        lr.clone()
    };
    let span_ends = {
        let se = span_ends.lock().expect("Should have no locks");
        se.clone()
    };

    // Verify captured data
    assert_eq!(span_ends.len(), 2, "Expected 2 span end records");

    let (span_id, span_name) = (span_ends[0].span_id, span_ends[0].span_name.clone());

    assert_eq!(log_records.len(), 1, "Expected 1 log record");
    let log_record = log_records
        .iter()
        .find(|record| {
            record
                .attributes
                .downcast_ref::<MockDynLogEvent>()
                .is_some()
        })
        .expect("expected mock log record");

    assert_eq!(log_record.trace_id, trace_id);
    assert_eq!(log_record.span_id, Some(span_id));
    assert_eq!(log_record.span_name, Some(span_name));
    assert_eq!(log_record.severity_number, SeverityNumber::Info);
    assert_eq!(log_record.severity_text, "INFO".to_string());
    assert_eq!(log_record.body, "Test info event".to_string());

    // The log event applies a test-owned context by downcasting the boxed wrapper.
    if let Some(log) = test_attrs.downcast_mut::<MockDynLogEvent>() {
        log.file = Some(mock_log_location.file().to_string());
        log.line = Some(mock_log_location.line() + 1);
        log.workflow_name = Some(expected_context.workflow_name);
        log.attempt = Some(expected_context.attempt);
    }

    assert_eq!(log_record.attributes, test_attrs);
}

#[test]
fn test_tracing_with_custom_layer() {
    // Initialize tracing with a custom layer to capture events
    let trace_id = rand::random::<u128>();

    let (test_layer, span_starts, span_ends, log_records) = TestLayer::new();

    // Init telemetry using internal API allowing to set thread local subscriber.
    // This avoids collisions with other unit tests
    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        tracing::info_span!("test_root_span").in_scope(|| {
            tracing::info!("Log message in root span");

            let span = tracing::info_span!("test_child_span");
            let _enter = span.enter();

            tracing::info!("Log message in child span");
            // Span will be created and closed automatically
        })
    });

    // Verify captured data
    let span_starts = {
        let ss = span_starts.lock().expect("Should have no locks");
        ss.clone()
    };
    let span_ends = {
        let se = span_ends.lock().expect("Should have no locks");
        se.clone()
    };
    let log_records = {
        let lr = log_records.lock().expect("Should have no locks");
        lr.clone()
    };

    // Should have 2 user spans
    assert_eq!(span_starts.len(), 2, "Expected 2 span starts");
    assert_eq!(span_ends.len(), 2, "Expected 2 span ends");

    // Should have 2 log records
    assert_eq!(log_records.len(), 2, "Expected 2 log records");

    // Test root span is present
    assert!(span_starts.iter().any(|r| {
        if let SpanStartInfo {
            trace_id: deserialized_trace_id,
            span_name,
            parent_span_id: None,
            attributes,
            ..
        } = r
        {
            let name = attributes
                .downcast_ref::<MockUnknown>()
                .expect("Must be of MockUnknown type")
                .name
                .as_str();
            span_name.starts_with("Mock Unknown Span")
                && name == "test_root_span"
                && *deserialized_trace_id == trace_id
        } else {
            false
        }
    }));
    assert!(span_ends.iter().any(|r| {
        if let SpanEndInfo {
            trace_id: deserialized_trace_id,
            span_name,
            parent_span_id: None,
            attributes,
            ..
        } = r
        {
            let name = attributes
                .downcast_ref::<MockUnknown>()
                .expect("Must be of MockUnknown type")
                .name
                .as_str();
            span_name.starts_with("Mock Unknown Span")
                && name == "test_root_span"
                && *deserialized_trace_id == trace_id
        } else {
            false
        }
    }));

    // Extract root span ID
    let root_span_id = span_starts
        .iter()
        .find_map(|r| {
            let SpanStartInfo {
                span_id,
                attributes,
                ..
            } = r;

            let name = attributes
                .downcast_ref::<MockUnknown>()
                .expect("Must be of MockUnknown type")
                .name
                .as_str();
            if name == "test_root_span" {
                Some(*span_id)
            } else {
                None
            }
        })
        .unwrap();

    // Test child span is present
    assert!(span_starts.iter().any(|r| {
        if let SpanStartInfo {
            trace_id: deserialized_trace_id,
            span_name,
            parent_span_id: Some(parent_id),
            attributes,
            ..
        } = r
        {
            let name = attributes
                .downcast_ref::<MockUnknown>()
                .expect("Must be of MockUnknown type")
                .name
                .as_str();
            span_name.starts_with("Mock Unknown Span")
                && name == "test_child_span"
                && *deserialized_trace_id == trace_id
                && *parent_id == root_span_id
        } else {
            false
        }
    }));
    assert!(span_ends.iter().any(|r| {
        if let SpanEndInfo {
            trace_id: deserialized_trace_id,
            span_name,
            parent_span_id: Some(parent_id),
            attributes,
            ..
        } = r
        {
            let name = attributes
                .downcast_ref::<MockUnknown>()
                .expect("Must be of MockUnknown type")
                .name
                .as_str();
            span_name.starts_with("Mock Unknown Span")
                && name == "test_child_span"
                && *deserialized_trace_id == trace_id
                && *parent_id == root_span_id
        } else {
            false
        }
    }));

    // Test log records are present
    assert!(log_records.iter().any(|r| matches!(
        r,
        LogRecordInfo {
            trace_id: deserialized_trace_id,
            span_name: Some(span_name),
            body,
            span_id: Some(span_id),
            ..
        } if *deserialized_trace_id == trace_id && span_name.starts_with("Mock Unknown Span") && body == "Log message in root span" && *span_id == root_span_id
    )));

    assert!(log_records.iter().any(|r| matches!(
        r,
        LogRecordInfo {
            trace_id: deserialized_trace_id,
            span_name: Some(span_name),
            body,
            span_id: Some(span_id),
            ..
        } if *deserialized_trace_id == trace_id && span_name.starts_with("Mock Unknown Span") && body == "Log message in child span" && *span_id != root_span_id
    )));
}

#[test]
fn test_tracing_log_record_poisoning() {
    use std::thread;

    struct SharedLayer;

    impl TelemetryConsumer for SharedLayer {
        fn on_log_record(&self, record: &LogRecordInfo, _: &mut DataProvider<'_>) {
            assert_eq!(
                record.body,
                format!("event from thread {:?}", thread::current().id()),
            );
        }
    }

    // Initialize tracing with a custom layer to capture events
    let trace_id = rand::random::<u128>();

    // Init telemetry using internal API allowing to set thread local subscriber.
    // This avoids collisions with other unit tests
    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            std::iter::once(Box::new(SharedLayer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    let subscriber = Arc::new(subscriber);

    tracing::subscriber::with_default(subscriber.clone(), || {
        let shared_span = tracing::info_span!("test_root_span");
        let shared_span_clone = shared_span.clone();

        // Thread 1
        let subscriber1 = subscriber.clone();
        let t1 = thread::spawn(move || {
            tracing::subscriber::with_default(subscriber1, || {
                let _g = shared_span.entered();
                let msg = format!("event from thread {:?}", thread::current().id());
                emit_info_event(
                    MockDynLogEvent {
                        flags: TelemetryOutputFlags::ALL,
                        ..Default::default()
                    },
                    Some(&msg),
                );
            })
        });

        // Thread 2
        let subscriber2 = subscriber.clone();
        let t2 = thread::spawn(move || {
            tracing::subscriber::with_default(subscriber2, || {
                let _g = shared_span_clone.entered();
                let msg = format!("event from thread {:?}", thread::current().id());
                emit_info_event(
                    MockDynLogEvent {
                        flags: TelemetryOutputFlags::ALL,
                        ..Default::default()
                    },
                    Some(&msg),
                );
            })
        });

        t1.join().unwrap();
        t2.join().unwrap();
    });
}

#[test]
fn test_parent_span_id_captured_on_root_span() {
    // Test that when a parent_span_id is provided by root span attributes,
    // it is correctly captured on the root span.
    let fallback_trace_id: u128 = 0x11112222333344445555666677778888;
    let fallback_parent_span_id: u64 = 0x1111222233334444;
    let root_trace_id: u128 = 0x9999aaaabbbbccccddddeeeeffff0000;
    let expected_parent_span_id: u64 = 0xdeadbeefcafebabe;

    let (test_layer, span_starts, span_ends, _) = TestLayer::new();

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            fallback_trace_id,
            Some(fallback_parent_span_id),
            false,
            std::iter::empty(),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let root_span = create_root_info_span(MockRootSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::ALL,
            trace_id: Some(root_trace_id),
            parent_span_id: Some(expected_parent_span_id),
        });
        root_span.in_scope(|| {
            // Create a child span to verify parent-child relationships still work
            let _child = create_info_span(MockDynSpanEvent {
                name: "child".to_string(),
                flags: TelemetryOutputFlags::ALL,
                ..Default::default()
            });
        });
    });

    let span_starts = span_starts.lock().expect("Should have no locks").clone();
    let span_ends = span_ends.lock().expect("Should have no locks").clone();

    // Should have 2 spans: root and child
    assert_eq!(span_starts.len(), 2, "Expected 2 span starts");
    assert_eq!(span_ends.len(), 2, "Expected 2 span ends");

    // Find the root span (the one with mock root span attributes)
    let root_span_start = span_starts
        .iter()
        .find(|s| s.attributes.downcast_ref::<MockRootSpanEvent>().is_some())
        .expect("Should find root span start");

    let root_span_end = span_ends
        .iter()
        .find(|s| s.attributes.downcast_ref::<MockRootSpanEvent>().is_some())
        .expect("Should find root span end");

    // Verify the root span uses trace context extracted from attributes, not fallback IDs.
    assert_eq!(root_span_start.trace_id, root_trace_id);
    assert_eq!(root_span_end.trace_id, root_trace_id);
    assert_ne!(root_span_start.trace_id, fallback_trace_id);
    assert_eq!(
        root_span_start.parent_span_id,
        Some(expected_parent_span_id),
        "Root span start should have the configured parent_span_id"
    );
    assert_eq!(
        root_span_end.parent_span_id,
        Some(expected_parent_span_id),
        "Root span end should have the configured parent_span_id"
    );

    // Find the child span
    let child_span_start = span_starts
        .iter()
        .find(|s| {
            s.attributes
                .downcast_ref::<MockDynSpanEvent>()
                .map(|e| e.name == "child")
                .unwrap_or(false)
        })
        .expect("Should find child span start");

    let child_span_end = span_ends
        .iter()
        .find(|s| {
            s.attributes
                .downcast_ref::<MockDynSpanEvent>()
                .map(|e| e.name == "child")
                .unwrap_or(false)
        })
        .expect("Should find child span end");

    // Child span should inherit the root trace ID and use the root span as its parent.
    assert_eq!(child_span_start.trace_id, root_trace_id);
    assert_eq!(child_span_end.trace_id, root_trace_id);
    assert_eq!(
        child_span_start.parent_span_id,
        Some(root_span_start.span_id),
        "Child span should have root span as parent"
    );
}
