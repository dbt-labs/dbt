use crate::{
    TelemetryOutputFlags,
    data_provider::DataProvider,
    emit::{create_info_span, create_root_info_span},
    init::create_tracing_subcriber_with_layer,
    layer::ConsumerLayer,
    metrics::{MetricKey, get_metric, increment_metric},
};
use tracing_subscriber::{Registry, registry::LookupSpan};

use super::mocks::{MockDynSpanEvent, TestLayer, test_data_layer};

const TOTAL_ERRORS_KEY: MetricKey = MetricKey::from_raw(1);
const TOTAL_WARNINGS_KEY: MetricKey = MetricKey::from_raw(2);
const AUTOFIX_SUGGESTIONS_KEY: MetricKey = MetricKey::from_raw(3);

#[test]
fn metrics_are_scoped_to_root_span() {
    let trace_id = rand::random::<u128>();

    let (test_layer, ..) = TestLayer::new();

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
        let first_root = create_root_info_span(MockDynSpanEvent {
            name: "first_root".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        });
        {
            let _root_scope = first_root.enter();

            let first_child = create_info_span(MockDynSpanEvent {
                name: "first_child".to_string(),
                flags: TelemetryOutputFlags::empty(),
                ..Default::default()
            });
            {
                let _child_scope = first_child.enter();
                increment_metric(TOTAL_ERRORS_KEY, 3);

                let first_child_id = first_child.id().expect("child span must have id");
                tracing::dispatcher::get_default(|dispatch| {
                    let registry = dispatch
                        .downcast_ref::<Registry>()
                        .expect("active subscriber must be backed by a registry");
                    let span_ref = registry
                        .span(&first_child_id)
                        .expect("child span must exist in registry");
                    let root_span = span_ref.scope().from_root().next().unwrap();

                    assert_eq!(first_root.id().expect("must exist"), root_span.id());

                    DataProvider::new(&root_span, &span_ref)
                        .increment_metric(TOTAL_WARNINGS_KEY, 2);
                });

                assert_eq!(get_metric(TOTAL_ERRORS_KEY), 3);
                assert_eq!(get_metric(TOTAL_WARNINGS_KEY), 2);
                assert_eq!(get_metric(AUTOFIX_SUGGESTIONS_KEY), 0);

                tracing::dispatcher::get_default(|dispatch| {
                    let registry = dispatch
                        .downcast_ref::<Registry>()
                        .expect("active subscriber must be backed by a registry");
                    let span_ref = registry
                        .span(&first_child_id)
                        .expect("child span must exist in registry");
                    let root_span = span_ref.scope().from_root().next().unwrap();
                    assert_eq!(
                        DataProvider::new(&root_span, &span_ref).get_metric(TOTAL_WARNINGS_KEY),
                        2
                    );
                });
            }
        }
        drop(first_root);

        assert_eq!(get_metric(TOTAL_ERRORS_KEY), 0);
        assert_eq!(get_metric(TOTAL_WARNINGS_KEY), 0);
        assert_eq!(get_metric(AUTOFIX_SUGGESTIONS_KEY), 0);

        let second_root = create_root_info_span(MockDynSpanEvent {
            name: "second_root".to_string(),
            flags: TelemetryOutputFlags::empty(),
            ..Default::default()
        });
        {
            let _root_scope = second_root.enter();

            let second_child = create_info_span(MockDynSpanEvent {
                name: "second_child".to_string(),
                flags: TelemetryOutputFlags::empty(),
                ..Default::default()
            });
            {
                let _child_scope = second_child.enter();
                increment_metric(TOTAL_ERRORS_KEY, 7);

                assert_eq!(get_metric(TOTAL_ERRORS_KEY), 7);
                assert_eq!(get_metric(AUTOFIX_SUGGESTIONS_KEY), 0);
            }
        }
        drop(second_root);

        assert_eq!(get_metric(TOTAL_ERRORS_KEY), 0);
        assert_eq!(get_metric(TOTAL_WARNINGS_KEY), 0);
        assert_eq!(get_metric(AUTOFIX_SUGGESTIONS_KEY), 0);
    });

    assert_eq!(get_metric(TOTAL_ERRORS_KEY), 0);
    assert_eq!(get_metric(TOTAL_WARNINGS_KEY), 0);
    assert_eq!(get_metric(AUTOFIX_SUGGESTIONS_KEY), 0);
}
