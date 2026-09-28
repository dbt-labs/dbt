use std::sync::{Arc, Mutex, mpsc};

use crate::{
    SpanEndInfo, TelemetryOutputFlags,
    constants::CLOSE_SPAN_FIELD,
    emit::{create_info_span, create_root_info_span, emit_info_event, force_close_span},
    event_info::store_event_attributes,
    init::create_tracing_subcriber_with_layer,
    layer::ConsumerLayer,
    reload::create_data_layer_for_tests,
    span_info::record_span_status,
};

use super::mocks::{
    MockDynLogEvent, MockDynSpanEvent, MockRootSpanEvent, TestLayer, test_data_layer,
    test_data_layer_config,
};

/// Code of log events emitted where they must be suppressed
const SUPPRESSED_CODE: i32 = 101;
/// Code of log events emitted where they must be delivered
const DELIVERED_CODE: i32 = 102;

fn root_span(name: &str) -> tracing::Span {
    create_root_info_span(MockRootSpanEvent {
        name: name.to_string(),
        flags: TelemetryOutputFlags::ALL,
        trace_id: Some(rand::random()),
        parent_span_id: None,
    })
}

fn child_span(name: &str) -> tracing::Span {
    create_info_span(MockDynSpanEvent {
        name: name.to_string(),
        flags: TelemetryOutputFlags::ALL,
        ..Default::default()
    })
}

/// Creates a force closable child span, the way a dedicated span creation helper would:
/// structured attributes plus the close marker field.
fn closable_child_span(name: &str) -> tracing::Span {
    store_event_attributes(MockDynSpanEvent {
        name: name.to_string(),
        flags: TelemetryOutputFlags::ALL,
        ..Default::default()
    });
    tracing::info_span!(
        "closable_child",
        { CLOSE_SPAN_FIELD } = tracing::field::Empty
    )
}

fn log(code: i32) {
    emit_info_event(
        MockDynLogEvent {
            code,
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        },
        Some("log"),
    );
}

fn root_end_names(ends: &Mutex<Vec<SpanEndInfo>>) -> Vec<String> {
    ends.lock()
        .unwrap()
        .iter()
        .filter_map(|span| {
            span.attributes
                .downcast_ref::<MockRootSpanEvent>()
                .map(|event| event.name.clone())
        })
        .collect()
}

fn end_names(ends: &Mutex<Vec<SpanEndInfo>>) -> Vec<String> {
    ends.lock()
        .unwrap()
        .iter()
        .filter_map(|span| {
            span.attributes
                .downcast_ref::<MockRootSpanEvent>()
                .map(|event| event.name.clone())
                .or_else(|| {
                    span.attributes
                        .downcast_ref::<MockDynSpanEvent>()
                        .map(|event| event.name.clone())
                })
        })
        .collect()
}

#[test]
fn force_close_of_nested_span_leaves_parent_open() {
    let (consumer, starts, ends, logs) = TestLayer::new();
    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            rand::random(),
            None,
            false,
            std::iter::empty(),
            std::iter::once(Box::new(consumer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let root = root_span("root");
        let (inner, retained) = root.in_scope(|| {
            let inner = closable_child_span("inner");
            let retained = inner.in_scope(|| child_span("retained"));
            (inner, retained)
        });

        force_close_span(&inner);
        assert_eq!(end_names(&ends), vec!["inner"]);

        // Everything in the closed subtree is suppressed, including the late native closes
        retained.in_scope(|| log(SUPPRESSED_CODE));
        inner.in_scope(|| drop(child_span("late")));
        drop(retained);
        drop(inner);
        assert_eq!(end_names(&ends), vec!["inner"]);

        // The parent and the rest of its subtree are unaffected
        root.in_scope(|| {
            log(DELIVERED_CODE);
            drop(child_span("sibling"));
        });
        let inner_2 = root.in_scope(|| closable_child_span("inner-2"));

        // Closing the parent covers nested closable spans that are still open: their own
        // force close and native close are no-ops
        force_close_span(&root);
        force_close_span(&inner_2);
        drop(inner_2);
        drop(root);

        assert_eq!(end_names(&ends), vec!["inner", "sibling", "root"]);
        assert_eq!(starts.lock().unwrap().len(), 5);
        let logs = logs.lock().unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(
            logs[0]
                .attributes
                .downcast_ref::<MockDynLogEvent>()
                .map(|event| event.code),
            Some(DELIVERED_CODE)
        );
    });
}

#[test]
fn force_close_ends_root_once_and_suppresses_descendants_retained_by_another_thread() {
    let (consumer, starts, ends, logs) = TestLayer::new();
    let subscriber = Arc::new(
        create_tracing_subcriber_with_layer(
            tracing::level_filters::LevelFilter::TRACE,
            test_data_layer(
                rand::random(),
                None,
                false,
                std::iter::empty(),
                std::iter::once(Box::new(consumer) as ConsumerLayer),
            ),
            &[],
        )
        .expect("test tracing filter directives must be valid"),
    );

    // Imitates an abandoned worker: it keeps descendants of the root alive, and keeps
    // emitting under them after the owner of the root has force closed it.
    let root = tracing::subscriber::with_default(Arc::clone(&subscriber), || root_span("root"));
    let worker_root = root.clone();
    let worker_subscriber = Arc::clone(&subscriber);
    let (retained_tx, retained_rx) = mpsc::channel::<()>();
    let (closed_tx, closed_rx) = mpsc::channel::<()>();
    let worker = std::thread::spawn(move || {
        tracing::subscriber::with_default(worker_subscriber, || {
            let (retained, retained_grandchild) = worker_root.in_scope(|| {
                let retained = child_span("retained");
                let grandchild = retained.in_scope(|| child_span("grandchild"));
                (retained, grandchild)
            });
            // Only the descendants keep the root alive from here on
            drop(worker_root);
            retained_tx.send(()).unwrap();
            closed_rx.recv().unwrap();

            // Late work under the retained descendants
            retained.in_scope(|| {
                log(SUPPRESSED_CODE);
                let late = child_span("late");
                late.in_scope(|| log(SUPPRESSED_CODE));
                record_span_status(&late, Some("ignored"));
            });
            drop(retained_grandchild);
            drop(retained);

            // A suppressed event must still consume the attributes it stored in thread-local
            // storage. Otherwise the next unstructured event on this thread would pick them up.
            let next = root_span("next");
            next.in_scope(|| tracing::info!("unstructured"));
        });
    });

    retained_rx.recv().unwrap();
    assert_eq!(starts.lock().unwrap().len(), 3);
    tracing::subscriber::with_default(Arc::clone(&subscriber), || {
        force_close_span(&root);
        assert_eq!(root_end_names(&ends), vec!["root"]);

        // Repeated close is a no-op
        force_close_span(&root);
        root.in_scope(|| log(SUPPRESSED_CODE));
        drop(root);
    });
    assert_eq!(ends.lock().unwrap().len(), 1);

    closed_tx.send(()).unwrap();
    worker.join().unwrap();

    // Only the "next" root was delivered after the close
    assert_eq!(starts.lock().unwrap().len(), 4);
    assert_eq!(root_end_names(&ends), vec!["root", "next"]);
    assert_eq!(ends.lock().unwrap().len(), 2);
    let logs = logs.lock().unwrap();
    assert_eq!(logs.len(), 1);
    // The mock fallback for unstructured logs is a `MockDynLogEvent` too, with its own code
    let code = logs[0]
        .attributes
        .downcast_ref::<MockDynLogEvent>()
        .map(|event| event.code);
    assert_ne!(
        code,
        Some(SUPPRESSED_CODE),
        "the unstructured event picked up the attributes of a suppressed event"
    );
}

#[test]
fn force_close_does_not_leak_late_work_into_reloaded_consumers() {
    let (consumer_a, _, ends_a, logs_a) = TestLayer::new();
    let (data_layer, reload_handle) = create_data_layer_for_tests(
        test_data_layer_config(rand::random(), None),
        vec![],
        vec![Box::new(consumer_a) as ConsumerLayer],
    );
    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        data_layer,
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let root_a = root_span("root-a");
        let retained = root_a.in_scope(|| child_span("retained-a"));
        force_close_span(&root_a);
        drop(root_a);
        assert_eq!(root_end_names(&ends_a), vec!["root-a"]);

        let (consumer_b, starts_b, ends_b, logs_b) = TestLayer::new();
        reload_handle
            .reload_telemetry(vec![], vec![Box::new(consumer_b) as ConsumerLayer])
            .expect("install consumer B");

        retained.in_scope(|| {
            log(SUPPRESSED_CODE);
            drop(child_span("late-a"));
        });
        drop(retained);
        assert!(logs_a.lock().unwrap().is_empty());
        assert_eq!(ends_a.lock().unwrap().len(), 1);
        assert!(starts_b.lock().unwrap().is_empty());
        assert!(ends_b.lock().unwrap().is_empty());
        assert!(logs_b.lock().unwrap().is_empty());

        let root_b = root_span("root-b");
        root_b.in_scope(|| log(DELIVERED_CODE));
        drop(root_b);
        assert_eq!(root_end_names(&ends_b), vec!["root-b"]);
        assert_eq!(logs_b.lock().unwrap().len(), 1);
    });
}

#[test]
fn force_close_leaves_other_roots_and_natural_close_unchanged() {
    let (consumer, _, ends, logs) = TestLayer::new();
    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            rand::random(),
            None,
            false,
            std::iter::empty(),
            std::iter::once(Box::new(consumer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let root_a = root_span("root-a");
        let retained = root_a.in_scope(|| child_span("retained"));
        let root_b = root_span("root-b");

        force_close_span(&root_a);

        // Only spans created via `create_root_info_span` can be force closed
        let child_b = root_b.in_scope(|| child_span("child-b"));
        force_close_span(&child_b);
        child_b.in_scope(|| log(DELIVERED_CODE));
        drop(child_b);
        drop(root_b);

        drop(retained);
        drop(root_a);

        assert_eq!(root_end_names(&ends), vec!["root-a", "root-b"]);
        assert_eq!(ends.lock().unwrap().len(), 3);
        assert_eq!(logs.lock().unwrap().len(), 1);
    });
}
