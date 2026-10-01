use crate::tracing::dbt_init::create_tracing_subcriber_with_layer;
use crate::{
    io_args::EvalArgs,
    tracing::{
        emit::{create_root_info_span, emit_info_event},
        invocation::create_invocation_attributes,
        layers::file_log_layer::build_file_log_layer_with_background_writer,
    },
};
use dbt_telemetry::LogMessage;
use dbt_tracing::SeverityNumber;
use dbt_tracing::test_support::mocks::test_data_layer;
use rand::random;
use std::fs;
use tracing::level_filters::LevelFilter;

#[test]
fn file_log_layer_creates_invocation_and_log_stub() {
    let invocation_id = uuid::Uuid::now_v7();
    let trace_id = invocation_id.as_u128();
    let temp_file_path =
        std::env::temp_dir().join(format!("file-log-layer-{}.log", random::<u64>()));

    let (file_log_layer, mut shutdown_handle) = build_file_log_layer_with_background_writer(
        fs::File::create(&temp_file_path).expect("Failed to create temporary log file"),
        LevelFilter::DEBUG,
    );

    // Init telemetry using internal API allowing to set thread local subscriber.
    // This avoids collisions with other unit tests
    let subscriber = create_tracing_subcriber_with_layer(
        LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            std::iter::once(file_log_layer),
        ),
    );

    let mut eval_args = EvalArgs::default();
    eval_args.io.invocation_id = invocation_id;

    tracing::subscriber::with_default(subscriber, || {
        let invocation_span =
            create_root_info_span(create_invocation_attributes("dbt-test", &eval_args));

        invocation_span.in_scope(|| {
            let log_message = LogMessage {
                code: None,
                dbt_core_event_code: None,
                original_severity_number: SeverityNumber::Info as i32,
                original_severity_text: "INFO".to_string(),
                ..Default::default()
            };

            emit_info_event(log_message, Some("file log layer stub log"));
        });
    });

    // Shutdown telemetry to ensure all data is flushed to the file
    shutdown_handle
        .shutdown()
        .expect("Failed to shutdown telemetry");

    let _ = fs::remove_file(&temp_file_path);
}
