use std::sync::{Arc, Mutex};

use dbt_telemetry::Invocation;
use dbt_tracing::{
    TelemetryOutputFlags,
    emit::create_info_span,
    layer::{ConsumerLayer, MiddlewareLayer},
    test_support::mocks::{MockDynSpanEvent, TestLayer, test_data_layer},
};
use tracing::level_filters::LevelFilter;

use crate::{
    ErrorCode,
    io_args::EvalArgs,
    tracing::{
        create_root_info_span, dbt_emit::emit_warn_log_message,
        dbt_init::create_tracing_subcriber_with_layer, force_close_span,
        invocation::create_invocation_attributes,
        middlewares::metric_aggregator::TelemetryMetricAggregator,
    },
};

/// Imitates the end of a dbt invocation while an abandoned worker still holds a descendant of
/// the invocation span: the invocation end, with final metrics computed by the real metric
/// aggregator, must be delivered by the force close, before the host tears down its sinks.
#[test]
fn force_closed_invocation_delivers_final_metrics_with_retained_child() {
    let (consumer, _, ends, _) = TestLayer::new();
    // Records the order of the invocation end delivery and the (imitated) sink teardown
    let order = Arc::new(Mutex::new(Vec::new()));
    let root_order = Arc::clone(&order);
    let consumer = consumer.with_span_end(move |span, _| {
        if span.attributes.downcast_ref::<Invocation>().is_some() {
            root_order.lock().unwrap().push("root");
        }
    });
    let subscriber = create_tracing_subcriber_with_layer(
        LevelFilter::TRACE,
        test_data_layer(
            rand::random(),
            None,
            false,
            std::iter::once(Box::new(TelemetryMetricAggregator) as MiddlewareLayer),
            std::iter::once(Box::new(consumer) as ConsumerLayer),
        ),
    );

    tracing::subscriber::with_default(subscriber, || {
        let eval_args = EvalArgs::default();
        let invocation =
            create_root_info_span(create_invocation_attributes("dbt-test", &eval_args));
        // The warning must show up in the final invocation metrics. The child span stands in for
        // a span held by an abandoned worker: while it is alive, the invocation can't close natively.
        let retained = invocation.in_scope(|| {
            emit_warn_log_message(ErrorCode::NoNodesSelected, "counted warning");
            create_info_span(MockDynSpanEvent {
                name: "retained".to_string(),
                flags: TelemetryOutputFlags::ALL,
                ..Default::default()
            })
        });

        force_close_span(&invocation);
        // What the host does next: shut down its sinks. The invocation end must already be delivered.
        order.lock().unwrap().push("sink");
        // The late native close of the invocation must not deliver its end a second time
        drop(invocation);
        drop(retained);

        let invocation_ends = ends
            .lock()
            .unwrap()
            .iter()
            .filter_map(|span| span.attributes.downcast_ref::<Invocation>())
            .cloned()
            .collect::<Vec<_>>();
        // Exactly one invocation end, carrying the metrics aggregated at the force close
        assert_eq!(invocation_ends.len(), 1);
        assert_eq!(
            invocation_ends[0]
                .metrics
                .as_ref()
                .and_then(|metrics| metrics.total_warnings),
            Some(1)
        );
        assert_eq!(*order.lock().unwrap(), vec!["root", "sink"]);
    });
}
