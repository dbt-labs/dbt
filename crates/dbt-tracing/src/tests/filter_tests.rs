use crate::{
    LogRecordInfo, SpanEndInfo, SpanStartInfo, TelemetryOutputFlags,
    data_provider::DataProvider,
    emit::{
        create_debug_span, create_info_span, create_root_info_span, emit_debug_event,
        emit_info_event,
    },
    filter::TelemetryFilterFn,
    init::create_tracing_subcriber_with_layer,
    layer::{ConsumerLayer, TelemetryConsumer},
};

use super::mocks::{MockDynLogEvent, MockDynSpanEvent, TestLayer, test_data_layer};
use tracing::level_filters::LevelFilter;

fn find_span_by_event_name<'a>(records: &'a [SpanStartInfo], name: &str) -> &'a SpanStartInfo {
    records
        .iter()
        .find(|record| {
            record
                .attributes
                .downcast_ref::<MockDynSpanEvent>()
                .map(|event| event.name.as_str() == name)
                .unwrap_or(false)
        })
        .expect("span with expected name should exist")
}

struct TokenFilteringConsumer {
    inner: TestLayer,
    span_token: String,
    log_token: String,
}

impl TokenFilteringConsumer {
    fn new(inner: TestLayer, span_token: impl Into<String>, log_token: impl Into<String>) -> Self {
        Self {
            inner,
            span_token: span_token.into(),
            log_token: log_token.into(),
        }
    }
}

impl TelemetryConsumer for TokenFilteringConsumer {
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool {
        span.attributes
            .downcast_ref::<MockDynSpanEvent>()
            .map(|event| event.name.contains(&self.span_token))
            .unwrap_or(false)
    }

    fn is_log_enabled(&self, log: &LogRecordInfo) -> bool {
        log.body.contains(&self.log_token)
    }

    fn on_span_start(&self, span: &SpanStartInfo, data_provider: &mut DataProvider<'_>) {
        self.inner.on_span_start(span, data_provider);
    }

    fn on_span_end(&self, span: &SpanEndInfo, data_provider: &mut DataProvider<'_>) {
        self.inner.on_span_end(span, data_provider);
    }

    fn on_log_record(&self, record: &LogRecordInfo, data_provider: &mut DataProvider<'_>) {
        self.inner.on_log_record(record, data_provider);
    }
}

#[test]
fn filtered_middle_span_reparents_grandchild() {
    let trace_id = rand::random::<u128>();

    let (baseline_layer, baseline_span_starts, _, _) = TestLayer::new();
    let (filtered_layer, filtered_span_starts, _, _) = TestLayer::new();

    const FILTERED_CHILD_NAME: &str = "filtered-child";

    let filtered_consumer = filtered_layer.with_span_filter(|span| {
        span.attributes
            .downcast_ref::<MockDynSpanEvent>()
            .map(|event| event.name.as_str() != FILTERED_CHILD_NAME)
            .unwrap_or(true)
    });

    let consumers: Vec<ConsumerLayer> = vec![Box::new(baseline_layer), Box::new(filtered_consumer)];

    let mut data_layer = test_data_layer(
        trace_id,
        None,
        false,
        std::iter::empty(),
        consumers.into_iter(),
    );
    data_layer.with_sequential_ids();

    let subscriber = create_tracing_subcriber_with_layer(LevelFilter::TRACE, data_layer, &[])
        .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .entered();

        create_info_span(MockDynSpanEvent {
            name: FILTERED_CHILD_NAME.to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .in_scope(|| {
            create_info_span(MockDynSpanEvent {
                name: "grandchild".to_string(),
                flags: TelemetryOutputFlags::ALL,
                ..Default::default()
            })
            .in_scope(|| {});
        });
    });

    let baseline_spans = {
        let guard = baseline_span_starts
            .lock()
            .expect("baseline span lock poisoned");
        guard.clone()
    };
    let filtered_spans = {
        let guard = filtered_span_starts
            .lock()
            .expect("filtered span lock poisoned");
        guard.clone()
    };

    assert_eq!(
        baseline_spans.len(),
        3,
        "baseline consumer should see all spans"
    );
    assert_eq!(
        filtered_spans.len(),
        2,
        "filtered consumer should skip middle span"
    );

    let _baseline_root = find_span_by_event_name(&baseline_spans, "root");
    let baseline_child = find_span_by_event_name(&baseline_spans, FILTERED_CHILD_NAME);
    let baseline_grandchild = find_span_by_event_name(&baseline_spans, "grandchild");

    assert_eq!(
        baseline_grandchild.parent_span_id,
        Some(baseline_child.span_id)
    );

    let filtered_root = find_span_by_event_name(&filtered_spans, "root");
    let filtered_grandchild = find_span_by_event_name(&filtered_spans, "grandchild");

    assert_eq!(
        filtered_grandchild.parent_span_id,
        Some(filtered_root.span_id)
    );
    assert_ne!(
        filtered_grandchild.parent_span_id,
        Some(baseline_child.span_id)
    );
    assert_eq!(filtered_grandchild.span_id, baseline_grandchild.span_id);
}

#[test]
fn level_filter_respects_span_and_log_levels() {
    let trace_id = rand::random::<u128>();

    let (baseline_layer, baseline_span_starts, _, baseline_log_records) = TestLayer::new();
    let (filtered_layer, filtered_span_starts, _, filtered_log_records) = TestLayer::new();

    let filtered_consumer = filtered_layer.with_filter(LevelFilter::INFO);

    let consumers: Vec<ConsumerLayer> = vec![Box::new(baseline_layer), Box::new(filtered_consumer)];

    let mut data_layer = test_data_layer(
        trace_id,
        None,
        false,
        std::iter::empty(),
        consumers.into_iter(),
    );
    data_layer.with_sequential_ids();

    let subscriber = create_tracing_subcriber_with_layer(LevelFilter::TRACE, data_layer, &[])
        .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(MockDynSpanEvent {
            name: "info-root".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .entered();

        create_debug_span(MockDynSpanEvent {
            name: "debug-child".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .in_scope(|| {});

        emit_info_event(
            MockDynLogEvent {
                code: 1,
                flags: TelemetryOutputFlags::ALL,
                ..Default::default()
            },
            Some("info-log"),
        );

        emit_debug_event(
            MockDynLogEvent {
                code: 2,
                flags: TelemetryOutputFlags::ALL,
                ..Default::default()
            },
            Some("debug-log"),
        );
    });

    let baseline_spans = {
        let guard = baseline_span_starts
            .lock()
            .expect("baseline span lock poisoned");
        guard.clone()
    };
    let filtered_spans = {
        let guard = filtered_span_starts
            .lock()
            .expect("filtered span lock poisoned");
        guard.clone()
    };
    let baseline_logs = {
        let guard = baseline_log_records
            .lock()
            .expect("baseline log lock poisoned");
        guard.clone()
    };
    let filtered_logs = {
        let guard = filtered_log_records
            .lock()
            .expect("filtered log lock poisoned");
        guard.clone()
    };

    assert_eq!(baseline_spans.len(), 2, "baseline must capture both spans");
    assert_eq!(filtered_spans.len(), 1, "debug span should be filtered out");
    assert!(filtered_spans.iter().all(|span| {
        span.attributes
            .downcast_ref::<MockDynSpanEvent>()
            .map(|event| event.name == "info-root")
            .unwrap_or(false)
    }));

    assert_eq!(baseline_logs.len(), 2, "baseline must capture both logs");
    assert_eq!(filtered_logs.len(), 1, "debug log should be filtered out");
    assert_eq!(filtered_logs[0].body, "info-log");
}

#[test]
fn filter_combines_with_consumer_predicates() {
    const FILTER_TOKEN: &str = "filter-ok";
    const CONSUMER_TOKEN: &str = "consumer-ok";

    let trace_id = rand::random::<u128>();

    let (capturing_layer, span_starts, span_ends, log_records) = TestLayer::new();

    let consumer = TokenFilteringConsumer::new(
        capturing_layer,
        CONSUMER_TOKEN.to_string(),
        CONSUMER_TOKEN.to_string(),
    );

    let span_filter_token = FILTER_TOKEN.to_string();
    let log_filter_token = FILTER_TOKEN.to_string();
    let telemetry_filter = TelemetryFilterFn::new(
        move |span| {
            span.attributes
                .downcast_ref::<MockDynSpanEvent>()
                .map(|event| event.name.contains(&span_filter_token))
                .unwrap_or(false)
        },
        move |log| log.body.contains(&log_filter_token),
    );

    let filtered_consumer = consumer.with_filter(telemetry_filter);
    let consumers: Vec<ConsumerLayer> = vec![Box::new(filtered_consumer)];

    let mut data_layer = test_data_layer(
        trace_id,
        None,
        false,
        std::iter::empty(),
        consumers.into_iter(),
    );
    data_layer.with_sequential_ids();

    let subscriber = create_tracing_subcriber_with_layer(LevelFilter::TRACE, data_layer, &[])
        .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(MockDynSpanEvent {
            name: format!("root {FILTER_TOKEN} {CONSUMER_TOKEN}"),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .entered();

        create_info_span(MockDynSpanEvent {
            name: format!("allowed-span {FILTER_TOKEN} {CONSUMER_TOKEN}"),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .in_scope(|| {
            let allowed_body = format!("allowed-log {FILTER_TOKEN} {CONSUMER_TOKEN}");
            emit_info_event(
                MockDynLogEvent {
                    code: 1,
                    flags: TelemetryOutputFlags::ALL,
                    ..Default::default()
                },
                Some(&allowed_body),
            );
        });

        create_info_span(MockDynSpanEvent {
            name: format!("consumer-filtered-span {FILTER_TOKEN}"),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .in_scope(|| {
            let filter_only_body = format!("filter-only-log {FILTER_TOKEN}");
            emit_info_event(
                MockDynLogEvent {
                    code: 2,
                    flags: TelemetryOutputFlags::ALL,
                    ..Default::default()
                },
                Some(&filter_only_body),
            );
        });

        create_info_span(MockDynSpanEvent {
            name: format!("filter-filtered-span {CONSUMER_TOKEN}"),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .in_scope(|| {
            let consumer_only_body = format!("consumer-only-log {CONSUMER_TOKEN}");
            emit_info_event(
                MockDynLogEvent {
                    code: 3,
                    flags: TelemetryOutputFlags::ALL,
                    ..Default::default()
                },
                Some(&consumer_only_body),
            );
        });
    });

    let spans = {
        let guard = span_starts.lock().expect("span starts lock poisoned");
        guard.clone()
    };
    let span_end_records = {
        let guard = span_ends.lock().expect("span ends lock poisoned");
        guard.clone()
    };
    let logs = {
        let guard = log_records.lock().expect("log records lock poisoned");
        guard.clone()
    };

    assert_eq!(spans.len(), 2, "only root and allowed span should remain");
    assert!(spans.iter().all(|span| {
        span.attributes
            .downcast_ref::<MockDynSpanEvent>()
            .is_some_and(|event| {
                event.name.contains(FILTER_TOKEN) && event.name.contains(CONSUMER_TOKEN)
            })
    }));

    assert_eq!(
        span_end_records.len(),
        2,
        "only allowed spans should produce end events"
    );
    assert_eq!(
        logs.len(),
        1,
        "only logs allowed by both filter and consumer should pass"
    );
    assert!(logs[0].body.contains(FILTER_TOKEN) && logs[0].body.contains(CONSUMER_TOKEN));
}
