use dbt_tracing::{
    TelemetryOutputFlags,
    emit::create_debug_span,
    layer::ConsumerLayer,
    test_support::mocks::{MockDynSpanEvent, TestLayer, test_data_layer},
};
use std::process::Command;
use tracing::level_filters::LevelFilter;

use crate::tracing::dbt_init::create_tracing_subcriber_with_layer;

const CHILD_PROCESS_MARKER: &str = "DBT_TEST_RUST_LOG_FILTER_CHILD";

#[test]
fn dbt_filter_directives_preserve_structural_spans_under_rust_log() {
    if std::env::var_os(CHILD_PROCESS_MARKER).is_none() {
        let status = Command::new(std::env::current_exe().expect("test executable must exist"))
            .arg("--exact")
            .arg("tracing::tests::dbt_init_tests::dbt_filter_directives_preserve_structural_spans_under_rust_log")
            .arg("--nocapture")
            .env(
                "RUST_LOG",
                "warn,dbt_tracing::emit=off,unrelated_target=off",
            )
            .env(CHILD_PROCESS_MARKER, "1")
            .status()
            .expect("child test process must start");

        assert!(status.success());
        return;
    }

    let trace_id = rand::random::<u128>();
    let (test_layer, _, _, _) = TestLayer::new();
    let subscriber = create_tracing_subcriber_with_layer(
        LevelFilter::DEBUG,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
    );

    tracing::subscriber::with_default(subscriber, || {
        let structural_span = create_debug_span(MockDynSpanEvent {
            name: "structural".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        });
        assert!(structural_span.id().is_some());

        let unrelated_span = tracing::debug_span!(target: "unrelated_target", "unrelated");
        assert!(unrelated_span.id().is_none());
    });
}
