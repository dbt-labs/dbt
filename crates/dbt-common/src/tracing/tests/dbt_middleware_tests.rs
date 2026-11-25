use std::sync::Arc;

use super::support::{capture_log_records, capture_records};
use crate::tracing::dbt_init::create_tracing_subcriber_with_layer;
use crate::tracing::tracing_feature_handles::FsTracingConfigProvider;
use crate::tracing::{
    TracingConfigProvider,
    dbt_emit::{
        emit_error_log_from_fs_error, emit_future_change, emit_info_log_message,
        emit_info_progress_message, emit_warn_log_message,
    },
    dbt_metrics::{FusionMetricKey, InvocationMetricKey, NamedProbe},
    fs_error_log::FsErrorLog,
    layer::{ConsumerLayer, MiddlewareLayer},
    metrics::get_metric,
    middlewares::{
        metric_aggregator::TelemetryMetricAggregator, secret_scrubber::TelemetrySecretScrubber,
        warn_error_options::TelemetryWarnErrorOptionsMiddleware,
    },
};
use dbt_error::FsError;
use dbt_telemetry::{GenericOpExecuted, ProgressMessage};
use dbt_tracing::emit::{create_info_span, create_root_info_span};
use dbt_tracing::span_info::record_span_status;
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

fn scrubbed_bodies(secrets: Vec<String>, message: &str) -> Vec<String> {
    let middlewares: Vec<MiddlewareLayer> =
        vec![Box::new(TelemetrySecretScrubber::with_secrets(secrets))];
    capture_log_records(middlewares, || emit_info_log_message(message))
        .into_iter()
        .map(|r| r.body)
        .collect()
}

#[test]
fn secret_scrubber_masks_secret_values_in_log_body() {
    assert_eq!(
        scrubbed_bodies(vec!["s3cr3t".into()], "a s3cr3t b"),
        vec!["a ***** b"]
    );
}

#[test]
fn secret_scrubber_leaves_body_without_match_or_secrets() {
    assert_eq!(scrubbed_bodies(vec!["s3cr3t".into()], "a b"), vec!["a b"]);
    assert_eq!(scrubbed_bodies(vec![], "a s3cr3t b"), vec!["a s3cr3t b"]);
}

fn secret_scrubber(secret: &str) -> Vec<MiddlewareLayer> {
    vec![Box::new(TelemetrySecretScrubber::with_secrets(vec![
        secret.to_string(),
    ]))]
}

#[test]
fn secret_scrubber_masks_span_name_status_and_attributes() {
    let records = capture_records(secret_scrubber("s3cr3t"), || {
        let span = create_info_span(GenericOpExecuted {
            operation_id: "clone s3cr3t".to_string(),
            display_action: "Cloning https://s3cr3t@host/repo".to_string(),
            item_count_total: None,
        });
        record_span_status(&span, Some("could not clone https://s3cr3t@host/repo"));
    });

    let start = records
        .span_starts
        .iter()
        .find(|span| span.attributes.is::<GenericOpExecuted>())
        .expect("the span start reaches the consumer");
    assert_eq!(
        start.span_name,
        "GenericOp: Cloning https://*****@host/repo"
    );

    let end = records
        .span_ends
        .iter()
        .find(|span| span.attributes.is::<GenericOpExecuted>())
        .expect("the span end reaches the consumer");
    assert_eq!(end.span_name, "GenericOp: Cloning https://*****@host/repo");
    assert_eq!(
        end.status.as_ref().and_then(|s| s.message.as_deref()),
        Some("could not clone https://*****@host/repo")
    );
    // Every string of the event is masked, and the event keeps its type.
    for attributes in [&start.attributes, &end.attributes] {
        let event = attributes
            .downcast_ref::<GenericOpExecuted>()
            .expect("the event keeps its type");
        assert_eq!(event.operation_id, "clone *****");
        assert_eq!(event.display_action, "Cloning https://*****@host/repo");
    }
}

#[test]
fn secret_scrubber_masks_log_attributes() {
    let records = capture_records(secret_scrubber("s3cr3t"), || {
        emit_info_progress_message(ProgressMessage::new_from_action_and_target(
            "Installing".to_string(),
            "https://host/pkg.tar.gz?token=s3cr3t".to_string(),
        ));
    });

    let progress = records
        .log_records
        .iter()
        .find_map(|record| record.attributes.downcast_ref::<ProgressMessage>())
        .expect("the progress message reaches the consumer");
    assert_eq!(progress.target, "https://host/pkg.tar.gz?token=*****");
}

/// In-process consumers such as the LSP read the error an error log retains, not its body.
#[test]
fn secret_scrubber_masks_the_error_retained_by_an_error_log() {
    let records = capture_records(secret_scrubber("s3cr3t"), || {
        emit_error_log_from_fs_error(FsError::new(
            ErrorCode::PackageDownloadFailed,
            "Failed to get https://s3cr3t@host/pkg.tar.gz",
        ));
    });

    let record = records
        .log_records
        .iter()
        .find(|record| record.attributes.is::<FsErrorLog>())
        .expect("the error log reaches the consumer");
    assert!(!record.body.contains("s3cr3t"), "body: {}", record.body);
    let error = record
        .attributes
        .downcast_ref::<FsErrorLog>()
        .expect("the event keeps its type")
        .get_fs_error();
    assert_eq!(error.code, ErrorCode::PackageDownloadFailed);
    assert_eq!(
        error.to_string(),
        "Failed to get https://*****@host/pkg.tar.gz"
    );
}

#[test]
fn secret_scrubber_leaves_spans_without_a_secret_unchanged() {
    let records = capture_records(secret_scrubber("s3cr3t"), || {
        let span = create_info_span(GenericOpExecuted {
            operation_id: "clone".to_string(),
            display_action: "Cloning https://host/repo".to_string(),
            item_count_total: Some(3),
        });
        record_span_status(&span, Some("could not clone"));
    });

    let end = records
        .span_ends
        .iter()
        .find(|span| span.attributes.is::<GenericOpExecuted>())
        .expect("the span end reaches the consumer");
    assert_eq!(
        end.span_name,
        "GenericOp: Cloning https://host/repo (3 items)"
    );
    assert_eq!(
        end.status.as_ref().and_then(|s| s.message.as_deref()),
        Some("could not clone")
    );
    assert_eq!(
        end.attributes.downcast_ref::<GenericOpExecuted>(),
        Some(&GenericOpExecuted {
            operation_id: "clone".to_string(),
            display_action: "Cloning https://host/repo".to_string(),
            item_count_total: Some(3),
        })
    );
}
