use crate::constants::DBT_BRAND_NAME;
use crate::tracing::FsTraceConfigBuilder;
use dbt_tracing::{
    LogRecordInfo, SeverityNumber, SpanStartInfo, TelemetryOutputFlags,
    test_support::mocks::MockDynLogEvent,
};
use std::time::SystemTime;
use tracing::level_filters::LevelFilter;

/// Builds a log record marked for OTLP export at the given severity.
fn otlp_log(severity_number: SeverityNumber) -> LogRecordInfo {
    LogRecordInfo {
        trace_id: 1,
        span_id: None,
        span_name: None,
        event_id: uuid::Uuid::now_v7(),
        time_unix_nano: SystemTime::now(),
        severity_number,
        severity_text: format!("{severity_number:?}").to_uppercase(),
        body: "otlp level filter probe".to_string(),
        attributes: MockDynLogEvent {
            flags: TelemetryOutputFlags::EXPORT_OTLP,
            ..Default::default()
        }
        .into(),
    }
}

/// Builds a span start record marked for OTLP export at the given severity.
fn otlp_span(severity_number: SeverityNumber) -> SpanStartInfo {
    SpanStartInfo {
        trace_id: 1,
        span_id: 2,
        span_name: "otlp-level-filter-probe".to_string(),
        parent_span_id: None,
        links: None,
        start_time_unix_nano: SystemTime::now(),
        severity_number,
        severity_text: format!("{severity_number:?}").to_uppercase(),
        attributes: MockDynLogEvent {
            flags: TelemetryOutputFlags::EXPORT_OTLP,
            ..Default::default()
        }
        .into(),
    }
}

/// Regression test for dbt-labs/dbt#17066: the OTLP export consumer must
/// honour the configured `--log-level` (`max_log_verbosity`) filter, like the
/// console and file sinks do. Before the fix, `build_otlp_layer` checked only
/// the `EXPORT_OTLP` output flag, so a run with `--log-level warn
/// --export-to-otlp` still exported DEBUG/INFO records and spans to the OTLP
/// endpoint.
#[test]
fn otlp_export_respects_max_log_verbosity() {
    let log_path =
        std::env::temp_dir().join(format!("otlp-level-filter-{}", rand::random::<u64>()));

    let config = FsTraceConfigBuilder::new("dbt-test", DBT_BRAND_NAME)
        .with_log_path(Some(&log_path))
        .with_max_log_verbosity(LevelFilter::WARN)
        .with_max_file_log_verbosity(LevelFilter::OFF)
        .with_export_to_otlp(true)
        .build();
    let config_provider = config.create_config_provider();
    let (_middlewares, consumer_layers, _shutdown_items) = config
        .build_layers(config_provider)
        .expect("build_layers must succeed")
        .into_parts();

    // The OTLP consumer is appended last when `export_to_otlp` is enabled.
    let otlp_layer = consumer_layers
        .last()
        .expect("export_to_otlp must add the OTLP consumer layer");

    // Records at or above the configured level are exported.
    assert!(
        otlp_layer.is_log_enabled(&otlp_log(SeverityNumber::Error)),
        "ERROR log records must be exported at --log-level warn"
    );
    assert!(
        otlp_layer.is_log_enabled(&otlp_log(SeverityNumber::Warn)),
        "WARN log records must be exported at --log-level warn"
    );
    assert!(
        otlp_layer.is_span_enabled(&otlp_span(SeverityNumber::Warn)),
        "WARN spans must be exported at --log-level warn"
    );

    // Records below the configured level are not exported.
    assert!(
        !otlp_layer.is_log_enabled(&otlp_log(SeverityNumber::Info)),
        "INFO log records must not be exported at --log-level warn"
    );
    assert!(
        !otlp_layer.is_log_enabled(&otlp_log(SeverityNumber::Debug)),
        "DEBUG log records must not be exported at --log-level warn"
    );
    assert!(
        !otlp_layer.is_span_enabled(&otlp_span(SeverityNumber::Info)),
        "INFO spans must not be exported at --log-level warn"
    );
    assert!(
        !otlp_layer.is_span_enabled(&otlp_span(SeverityNumber::Debug)),
        "DEBUG spans must not be exported at --log-level warn"
    );

    // The `EXPORT_OTLP` output flag remains required at every level.
    let mut console_only = otlp_log(SeverityNumber::Error);
    console_only.attributes = MockDynLogEvent {
        flags: TelemetryOutputFlags::EXPORT_JSONL,
        ..Default::default()
    }
    .into();
    assert!(
        !otlp_layer.is_log_enabled(&console_only),
        "records without the EXPORT_OTLP flag must not be exported"
    );
}
