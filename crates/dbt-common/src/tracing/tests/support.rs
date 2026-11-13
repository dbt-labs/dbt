//! Helpers shared by the tracing tests.

use dbt_tracing::emit::create_root_info_span;
use dbt_tracing::test_support::mocks::{MockDynSpanEvent, TestLayer, test_data_layer};
use dbt_tracing::{LogRecordInfo, TelemetryOutputFlags};
use tracing::level_filters::LevelFilter;

use crate::tracing::dbt_init::create_tracing_subcriber_with_layer;
use crate::tracing::layer::{ConsumerLayer, MiddlewareLayer};

/// Runs `emit` under the given middlewares and returns the records that reach a consumer.
pub(super) fn capture_log_records(
    middlewares: Vec<MiddlewareLayer>,
    emit: impl FnOnce(),
) -> Vec<LogRecordInfo> {
    let trace_id = rand::random::<u128>();
    let (test_layer, _span_starts, _span_ends, log_records) = TestLayer::new();
    let consumers: Vec<ConsumerLayer> = vec![Box::new(test_layer)];
    let mut data_layer = test_data_layer(
        trace_id,
        None,
        false,
        middlewares.into_iter(),
        consumers.into_iter(),
    );
    data_layer.with_sequential_ids();
    let subscriber = create_tracing_subcriber_with_layer(LevelFilter::TRACE, data_layer);

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
        .expect("log records mutex poisoned")
        .clone()
}
