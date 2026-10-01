use crate::{
    LogRecordInfo, TelemetryOutputFlags,
    data_provider::DataProvider,
    emit::{create_info_span, create_root_info_span, emit_info_event},
    init::create_tracing_subcriber_with_layer,
    layer::{ConsumerLayer, MiddlewareLayer, TelemetryMiddleware},
    metrics::{MetricKey, get_metric},
    tests::mocks::{MockDynLogEvent, MockDynSpanEvent, MockMiddleware, TestLayer, test_data_layer},
};

use std::thread;
use std::{
    sync::{Arc, Barrier, Condvar, Mutex},
    time::Duration,
};
use tracing::{Dispatch, level_filters::LevelFilter};

const TEST_TOTAL_WARNINGS_METRIC: MetricKey = MetricKey::from_raw(1);

#[test]
fn middleware_modifies_drops_and_updates_metrics() {
    let trace_id = rand::random::<u128>();
    let (test_layer, span_starts, span_ends, log_records) = TestLayer::new();

    let middleware = MockMiddleware::new()
        .with_span_start(|mut span, metrics| {
            if span.span_name.ends_with("drop-me") {
                return None;
            }

            if span.span_name.ends_with("child") {
                metrics.increment_metric(TEST_TOTAL_WARNINGS_METRIC, 1);

                span.attributes = MockDynSpanEvent {
                    name: "mutated-child".to_string(),
                    flags: TelemetryOutputFlags::ALL,
                    has_sensitive: false,
                    was_scrubbed: true,
                    ..Default::default()
                }
                .into();
                span.span_name = "Mock Dyn Span Event: mutated-child".to_string();
            }

            Some(span)
        })
        .with_log_record(|record, _| {
            if record
                .attributes
                .downcast_ref::<MockDynLogEvent>()
                .is_some_and(|event| matches!(event.code, 2 | 3))
            {
                None
            } else {
                Some(record)
            }
        });

    let middlewares: Vec<MiddlewareLayer> = vec![Box::new(middleware)];
    let consumers: Vec<ConsumerLayer> = vec![Box::new(test_layer)];

    let mut data_layer = test_data_layer(
        trace_id,
        None, // parent_span_id not needed in tests
        false,
        middlewares.into_iter(),
        consumers.into_iter(),
    );
    data_layer.with_sequential_ids();

    let subscriber = create_tracing_subcriber_with_layer(LevelFilter::TRACE, data_layer, &[])
        .expect("test tracing filter directives must be valid");

    let recorded_metric = tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .entered();

        create_info_span(MockDynSpanEvent {
            name: "child".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .in_scope(|| {
            emit_info_event(
                MockDynLogEvent {
                    code: 1,
                    flags: TelemetryOutputFlags::ALL,
                    ..Default::default()
                },
                Some("keep me"),
            );
            emit_info_event(
                MockDynLogEvent {
                    code: 2,
                    flags: TelemetryOutputFlags::ALL,
                    ..Default::default()
                },
                Some("drop me"),
            );
        });

        create_info_span(MockDynSpanEvent {
            name: "drop-me".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .in_scope(|| {
            emit_info_event(
                MockDynLogEvent {
                    code: 3,
                    flags: TelemetryOutputFlags::ALL,
                    ..Default::default()
                },
                Some("should vanish"),
            );
        });

        get_metric(TEST_TOTAL_WARNINGS_METRIC)
    });

    assert_eq!(recorded_metric, 1, "middleware should increment metric");

    let captured_span_starts = {
        let guard = span_starts.lock().expect("span starts mutex poisoned");
        guard.clone()
    };
    let captured_span_ends = {
        let guard = span_ends.lock().expect("span ends mutex poisoned");
        guard.clone()
    };
    let captured_log_records = {
        let guard = log_records.lock().expect("log records mutex poisoned");
        guard.clone()
    };

    let log_codes: Vec<Option<i32>> = captured_log_records
        .iter()
        .map(|record| {
            record
                .attributes
                .downcast_ref::<MockDynLogEvent>()
                .map(|event| event.code)
        })
        .collect();

    assert_eq!(
        captured_span_starts.len(),
        2,
        "dropped span should not be recorded"
    );
    assert_eq!(
        captured_span_ends.len(),
        2,
        "dropped span should not emit end record"
    );
    assert_eq!(
        log_codes,
        vec![Some(1)],
        "only log with code 1 should remain"
    );
    assert_eq!(
        captured_log_records.len(),
        1,
        "dropped log should be filtered out"
    );
    assert_eq!(captured_log_records[0].body, "keep me");

    let mutated_span = captured_span_starts
        .iter()
        .find(|span| span.span_name == "Mock Dyn Span Event: mutated-child")
        .expect("mutated span should be present");
    let mutated_end = captured_span_ends
        .iter()
        .find(|span| span.span_name == "Mock Dyn Span Event: mutated-child")
        .expect("mutated span end should be present");

    let attrs = mutated_span
        .attributes
        .downcast_ref::<MockDynSpanEvent>()
        .expect("attributes should be the mutated span event");
    assert_eq!(attrs.name, "mutated-child");
    assert!(attrs.was_scrubbed, "middleware should update attributes");

    assert_eq!(mutated_span.span_id, mutated_end.span_id);
}

#[derive(Default)]
struct CooperativeMiddlewareShared {
    state: Mutex<(u8, u8)>,
    condvar: Condvar,
}

impl CooperativeMiddlewareShared {
    fn record_entry(&self) {
        let mut guard = self.state.lock().expect("middleware state poisoned");
        let (ref mut active, ref mut max_active) = *guard;
        *active += 1;
        *max_active = (*max_active).max(*active);
        self.condvar.notify_all();
    }

    fn wait_for_other(&self) {
        let guard = self.state.lock().expect("middleware state poisoned");
        let (mut guard, _) = self
            .condvar
            // Wait up to 100ms for another thread to enter, should be plenty of time
            .wait_timeout(guard, Duration::from_millis(100))
            .expect("middleware state poisoned while waiting for other thread");

        guard.0 -= 1;
    }

    fn max_active(&self) -> u8 {
        self.state.lock().expect("middleware state poisoned").1
    }
}

#[derive(Clone, Default)]
struct CooperativeMiddleware {
    shared: Arc<CooperativeMiddlewareShared>,
}

impl CooperativeMiddleware {
    fn new() -> (Self, Arc<CooperativeMiddlewareShared>) {
        let shared = Arc::new(CooperativeMiddlewareShared::default());
        (
            Self {
                shared: shared.clone(),
            },
            shared,
        )
    }
}

impl TelemetryMiddleware for CooperativeMiddleware {
    fn on_log_record(
        &self,
        record: LogRecordInfo,
        _data_provider: &mut DataProvider<'_>,
    ) -> Option<LogRecordInfo> {
        self.shared.record_entry();
        self.shared.wait_for_other();
        Some(record)
    }
}

#[test]
fn middleware_invocations_do_not_block_across_threads() {
    let trace_id = rand::random::<u128>();
    let (test_layer, _, _, _) = TestLayer::new();

    let (cooperative_middleware, shared_state) = CooperativeMiddleware::new();
    let middlewares: Vec<MiddlewareLayer> = vec![Box::new(cooperative_middleware)];
    let consumers: Vec<ConsumerLayer> = vec![Box::new(test_layer)];

    let mut data_layer = test_data_layer(
        trace_id,
        None, // parent_span_id not needed in tests
        false,
        middlewares.into_iter(),
        consumers.into_iter(),
    );
    data_layer.with_sequential_ids();

    let subscriber = create_tracing_subcriber_with_layer(LevelFilter::TRACE, data_layer, &[])
        .expect("test tracing filter directives must be valid");
    let disptcher = Dispatch::new(subscriber);

    tracing::dispatcher::with_default(&disptcher, || {
        let root_span = create_root_info_span(MockDynSpanEvent {
            name: "shared-root".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        });
        let _root_guard = root_span.enter();

        const NUM_THREADS: usize = 2;
        let start_barrier = Barrier::new(NUM_THREADS);
        thread::scope(|s| {
            for _idx in 0..NUM_THREADS {
                s.spawn(|| {
                    tracing::dispatcher::with_default(&disptcher, || {
                        let _thread_guard = root_span.enter();
                        start_barrier.wait();

                        emit_info_event(
                            MockDynLogEvent {
                                code: 42i32,
                                flags: TelemetryOutputFlags::ALL,
                                ..Default::default()
                            },
                            Some("middleware concurrency check"),
                        );
                    });
                });
            }
        });
    });

    assert!(
        shared_state.max_active() == 2,
        "expected middleware invocations to overlap across threads"
    );
}
