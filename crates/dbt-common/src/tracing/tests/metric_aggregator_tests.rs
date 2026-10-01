use super::super::{
    dbt_metrics::{FusionMetricKey, InvocationMetricKey},
    emit::{create_root_info_span, emit_warn_event},
    layer::{ConsumerLayer, MiddlewareLayer},
    metrics::get_metric,
    middlewares::metric_aggregator::TelemetryMetricAggregator,
};
use crate::tracing::dbt_init::create_tracing_subcriber_with_layer;
use dbt_telemetry::LogMessage;
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
