use crate::{
    TelemetryOutputFlags,
    emit::{create_root_info_span, emit_debug_event, emit_info_event},
    error::TracingError,
    init::create_tracing_subcriber_with_layer,
    layer::ConsumerLayer,
};

use super::mocks::{MockDynLogEvent, MockDynSpanEvent, TestLayer, test_data_layer};
use tracing::level_filters::LevelFilter;

fn captured_log_bodies(
    max_log_verbosity: LevelFilter,
    filter_directives: &[&str],
    emit: impl FnOnce(),
) -> Vec<String> {
    let trace_id = rand::random::<u128>();
    let (test_layer, _, _, log_records) = TestLayer::new();

    let subscriber = create_tracing_subcriber_with_layer(
        max_log_verbosity,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        filter_directives,
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .entered();

        emit();
    });

    log_records
        .lock()
        .expect("log records mutex should not be poisoned")
        .iter()
        .map(|record| record.body.clone())
        .collect()
}

#[test]
fn generic_subscriber_uses_exact_info_max_level() {
    let bodies = captured_log_bodies(LevelFilter::INFO, &[], || {
        emit_info_event(
            MockDynLogEvent {
                flags: TelemetryOutputFlags::ALL,
                ..Default::default()
            },
            Some("info record"),
        );
        emit_debug_event(
            MockDynLogEvent {
                flags: TelemetryOutputFlags::ALL,
                ..Default::default()
            },
            Some("debug record"),
        );
    });

    assert_eq!(bodies, vec!["info record"]);
}

#[test]
fn generic_subscriber_applies_supplied_directives() {
    let bodies = captured_log_bodies(LevelFilter::DEBUG, &["blocked_target=off"], || {
        tracing::debug!(target: "allowed_target", "allowed debug");
        tracing::debug!(target: "blocked_target", "blocked debug");
    });

    assert_eq!(bodies, vec!["allowed debug"]);
}

#[test]
fn generic_subscriber_rejects_invalid_directives() {
    let trace_id = rand::random::<u128>();
    let (test_layer, _, _, _) = TestLayer::new();

    let result = create_tracing_subcriber_with_layer(
        LevelFilter::DEBUG,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        &["blocked_target=definitely_not_a_level"],
    );
    let Err(error) = result else {
        panic!("invalid tracing filter directive should return an error");
    };

    assert!(matches!(error, TracingError::InvalidFilterDirective(_)));
}
