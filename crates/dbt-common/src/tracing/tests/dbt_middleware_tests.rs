use std::sync::Arc;

use crate::tracing::dbt_init::create_tracing_subcriber_with_layer;
use crate::tracing::tracing_feature_handles::FsTracingConfigProvider;
use crate::tracing::{
    TracingConfigProvider,
    dbt_emit::emit_warn_log_message,
    dbt_metrics::{FusionMetricKey, InvocationMetricKey},
    layer::{ConsumerLayer, MiddlewareLayer},
    metrics::get_metric,
    middlewares::{
        metric_aggregator::TelemetryMetricAggregator,
        warn_error_options::TelemetryWarnErrorOptionsMiddleware,
    },
};
use dbt_tracing::emit::create_root_info_span;
use dbt_tracing::test_support::mocks::{MockDynSpanEvent, TestLayer, test_data_layer};

use crate::ErrorCode;
use crate::warn_error_options::{SupportedLegacyWarnError, WarnErrorOptionValue, WarnErrorOptions};
use dbt_tracing::{SeverityNumber, TelemetryOutputFlags};
use tracing::level_filters::LevelFilter;

#[test]
fn warn_error_options_middleware_updates_runtime_decisions() {
    let trace_id = rand::random::<u128>();
    let (test_layer, _span_starts, _span_ends, log_records) = TestLayer::new();
    let config_provider =
        Arc::new(FsTracingConfigProvider::default()) as Arc<dyn TracingConfigProvider>;
    let warn_error_options_middleware =
        TelemetryWarnErrorOptionsMiddleware::new(Arc::clone(&config_provider), false);

    let middlewares: Vec<MiddlewareLayer> = vec![
        Box::new(warn_error_options_middleware),
        Box::new(TelemetryMetricAggregator),
    ];
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

    let (error_count, warning_count) = tracing::subscriber::with_default(subscriber, || {
        let _root_guard = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .entered();

        emit_warn_log_message(ErrorCode::NoNodesSelected, "warn");

        config_provider.set_warn_error_options(WarnErrorOptions {
            error: vec![WarnErrorOptionValue::FusionCode(
                ErrorCode::NoNodesSelected as u16,
            )],
            ..Default::default()
        });
        emit_warn_log_message(ErrorCode::NoNodesSelected, "error");

        config_provider.set_warn_error_options(WarnErrorOptions {
            silence: vec![WarnErrorOptionValue::SupportedLegacy(
                SupportedLegacyWarnError::NothingToDo,
            )],
            ..Default::default()
        });
        emit_warn_log_message(ErrorCode::NoNodesSelected, "silence");

        (
            get_metric(FusionMetricKey::InvocationMetric(
                InvocationMetricKey::TotalErrors,
            )),
            get_metric(FusionMetricKey::InvocationMetric(
                InvocationMetricKey::TotalWarnings,
            )),
        )
    });

    let captured_log_records = log_records
        .lock()
        .expect("log records mutex poisoned")
        .clone();

    assert_eq!(captured_log_records.len(), 2);
    assert_eq!(
        captured_log_records[0].severity_number,
        SeverityNumber::Warn
    );
    assert_eq!(
        captured_log_records[1].severity_number,
        SeverityNumber::Error
    );
    assert_eq!(warning_count, 1);
    assert_eq!(error_count, 1);
}

/// Upgrades are withheld only for fusion-only codes, and only when built with `skip_fusion_only_upgrades = true`.
#[test]
fn skip_fusion_only_upgrades_withholds_upgrade_only_for_fusion_only_codes() {
    let trace_id = rand::random::<u128>();
    let (test_layer, _span_starts, _span_ends, log_records) = TestLayer::new();
    let config_provider = Arc::new(FsTracingConfigProvider::from_warn_error_options(
        WarnErrorOptions {
            error: vec![WarnErrorOptionValue::all()],
            ..Default::default()
        },
    )) as Arc<dyn TracingConfigProvider>;
    let warn_error_options_middleware =
        TelemetryWarnErrorOptionsMiddleware::new(Arc::clone(&config_provider), true);

    let middlewares: Vec<MiddlewareLayer> = vec![Box::new(warn_error_options_middleware)];
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

        emit_warn_log_message(ErrorCode::UnusedConfigKey, "fusion-only");
        emit_warn_log_message(ErrorCode::DeprecatedModel, "has dbt-core counterpart");

        // Silencing must still take effect for a fusion-only code.
        config_provider.set_warn_error_options(WarnErrorOptions {
            error: vec![WarnErrorOptionValue::all()],
            silence: vec![WarnErrorOptionValue::FusionCode(
                ErrorCode::UnusedConfigKey as u16,
            )],
            ..Default::default()
        });
        emit_warn_log_message(ErrorCode::UnusedConfigKey, "silenced");
    });

    let captured_log_records = log_records
        .lock()
        .expect("log records mutex poisoned")
        .clone();

    assert_eq!(captured_log_records.len(), 2);
    assert_eq!(
        captured_log_records[0].severity_number,
        SeverityNumber::Warn,
        "fusion-only warning must not be upgraded while replaying"
    );
    assert_eq!(
        captured_log_records[1].severity_number,
        SeverityNumber::Error,
        "warning with a dbt-core counterpart must still be upgraded"
    );
}
