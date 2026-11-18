use std::sync::Arc;

use super::support::capture_log_records;
use crate::tracing::dbt_init::create_tracing_subcriber_with_layer;
use crate::tracing::tracing_feature_handles::FsTracingConfigProvider;
use crate::tracing::{
    TracingConfigProvider,
    dbt_emit::{emit_future_change, emit_warn_log_message},
    dbt_metrics::{FusionMetricKey, InvocationMetricKey, NamedProbe},
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

#[test]
fn future_error_keeps_notice_when_promoted_to_error() {
    let config_provider = Arc::new(FsTracingConfigProvider::from_warn_error_options(
        WarnErrorOptions {
            error: vec![WarnErrorOptionValue::all()],
            ..Default::default()
        },
    )) as Arc<dyn TracingConfigProvider>;
    let middlewares: Vec<MiddlewareLayer> = vec![Box::new(
        TelemetryWarnErrorOptionsMiddleware::new(config_provider, false),
    )];

    let records = capture_log_records(middlewares, || {
        emit_future_change(ErrorCode::MalformedBlockName, "Malformed.");
    });

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].severity_number, SeverityNumber::Error);
    assert!(
        records[0]
            .body
            .ends_with("See https://github.com/dbt-labs/dbt/issues/16956"),
        "{}",
        records[0].body
    );
}

#[test]
fn future_incompatibility_is_never_upgraded_but_can_be_silenced() {
    let upgrade_options = [
        WarnErrorOptionValue::all(),
        WarnErrorOptionValue::FusionCode(ErrorCode::FutureIncompatibility as u16),
        WarnErrorOptionValue::LegacyGroup(
            crate::warn_error_options::LegacyWarnErrorGroupValue::Deprecations,
        ),
    ];
    for option in upgrade_options {
        let config_provider = Arc::new(FsTracingConfigProvider::from_warn_error_options(
            WarnErrorOptions {
                error: vec![option.clone()],
                ..Default::default()
            },
        )) as Arc<dyn TracingConfigProvider>;
        let records = capture_log_records(
            vec![Box::new(TelemetryWarnErrorOptionsMiddleware::new(
                config_provider,
                false,
            ))],
            || emit_future_change(NamedProbe::TestOnly, "Changed."),
        );
        assert_eq!(records.len(), 1, "{option:?}");
        assert_eq!(
            records[0].severity_number,
            SeverityNumber::Warn,
            "{option:?}"
        );
    }

    let config_provider = Arc::new(FsTracingConfigProvider::from_warn_error_options(
        WarnErrorOptions {
            silence: vec![WarnErrorOptionValue::FusionCode(
                ErrorCode::FutureIncompatibility as u16,
            )],
            ..Default::default()
        },
    )) as Arc<dyn TracingConfigProvider>;
    let mut hits = 0;
    let records = capture_log_records(
        vec![Box::new(TelemetryWarnErrorOptionsMiddleware::new(
            config_provider,
            false,
        ))],
        || {
            emit_future_change(NamedProbe::TestOnly, "Changed.");
            hits = get_metric(FusionMetricKey::CompatibilityProbe(
                NamedProbe::TestOnly.into(),
            ));
        },
    );
    assert!(records.is_empty());
    // Silenced warnings are still counted.
    assert_eq!(hits, 1);
}
