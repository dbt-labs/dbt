use super::super::{
    dbt_emit::{emit_compatibility_probe, emit_future_change},
    dbt_metrics::{FusionMetricKey, InvocationMetricKey, NamedProbe},
    emit::{create_root_info_span, emit_warn_event},
    layer::{ConsumerLayer, MiddlewareLayer},
    metrics::get_metric,
    middlewares::metric_aggregator::TelemetryMetricAggregator,
};
use crate::ErrorCode;
use crate::tracing::dbt_init::create_tracing_subcriber_with_layer;
use dbt_telemetry::{Invocation, LogMessage};
use dbt_tracing::TelemetryOutputFlags;
use dbt_tracing::test_support::mocks::{MockDynSpanEvent, TestLayer, test_data_layer};

#[test]
fn warning_logs_increment_warning_metric() {
    let trace_id = rand::random::<u128>();

    let (test_layer, ..) = TestLayer::new();

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::once(Box::new(TelemetryMetricAggregator) as MiddlewareLayer),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
    );

    let test_metric_key = FusionMetricKey::InvocationMetric(InvocationMetricKey::TotalWarnings);

    tracing::subscriber::with_default(subscriber, || {
        let root_span_guard = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        })
        .entered();

        assert_eq!(get_metric(test_metric_key), 0);

        emit_warn_event(
            LogMessage::new_from_level(tracing::Level::WARN),
            Some("test warning"),
        );

        assert_eq!(get_metric(test_metric_key), 1);

        drop(root_span_guard);
    });

    assert_eq!(get_metric(test_metric_key), 0);
}

#[test]
fn probe_counts_reach_invocation_metrics() {
    let trace_id = rand::random::<u128>();
    let (test_layer, _span_starts, span_ends, _log_records) = TestLayer::new();
    let consumers: Vec<ConsumerLayer> = vec![Box::new(test_layer)];
    let mut data_layer = test_data_layer(
        trace_id,
        None,
        false,
        std::iter::once(Box::new(TelemetryMetricAggregator) as MiddlewareLayer),
        consumers.into_iter(),
    );
    data_layer.with_sequential_ids();
    let subscriber =
        create_tracing_subcriber_with_layer(tracing::level_filters::LevelFilter::TRACE, data_layer);

    tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(Invocation {
            invocation_id: "test".to_string(),
            ..Default::default()
        })
        .entered();
        emit_compatibility_probe(NamedProbe::TestOnly);
        emit_compatibility_probe(NamedProbe::TestOnly);
        emit_future_change(ErrorCode::MalformedBlockName, "first");
        emit_future_change(ErrorCode::MalformedBlockName, "second");
        emit_future_change(ErrorCode::MalformedBlockName, "third");
        emit_future_change(ErrorCode::CleanTargetOutsideProject, "every");
        emit_future_change(ErrorCode::CleanTargetOutsideProject, "every");
        emit_future_change(NamedProbe::TestOnly, "a");
        emit_future_change(NamedProbe::TestOnlyOther, "b");
        emit_future_change(NamedProbe::TestOnly, "a again");
    });

    let span_ends = span_ends.lock().expect("span ends mutex poisoned").clone();
    let metrics = span_ends
        .iter()
        .find_map(|span| span.attributes.downcast_ref::<Invocation>())
        .and_then(|invocation| invocation.metrics.clone())
        .expect("invocation metrics");
    // Silent probes and surfaced future incompatibilities share one counter.
    assert_eq!(metrics.probe_counts.get("test_only").copied(), Some(4));
    assert_eq!(
        metrics.probe_counts.get("test_only_other").copied(),
        Some(1)
    );
    assert!(
        !metrics
            .probe_counts
            .contains_key("CompatibilityProbe::test_only")
    );
    assert_eq!(
        metrics.probe_counts.get("MalformedBlockName").copied(),
        Some(3)
    );
    assert_eq!(
        metrics
            .probe_counts
            .get("CleanTargetOutsideProject")
            .copied(),
        Some(2)
    );
    assert!(metrics.node_type_counts.is_empty());
}
