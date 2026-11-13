use crate::{CodeLocationWithFile, fs_err};
use dbt_error::ErrorCode;
use dbt_tracing::{SeverityNumber, TelemetryOutputFlags};
use std::panic::Location;

use super::support::capture_log_records;
use crate::tracing::dbt_init::create_tracing_subcriber_with_layer;
use crate::tracing::{
    dbt_emit::{
        emit_compatibility_probe, emit_error_log_from_fs_error, emit_error_log_message,
        emit_future_change, emit_future_change_from_fs_error, emit_warn_log_message,
    },
    dbt_metrics::{FusionMetricKey, NamedProbe},
    fs_error_log::get_log_message,
    future_incompatibility_log::FutureIncompatibilityLog,
    layer::{ConsumerLayer, MiddlewareLayer},
    metrics::get_metric,
    middlewares::markdown_log_filter::TelemetryMarkdownLogFilter,
};
use dbt_tracing::emit::create_root_info_span;

use dbt_tracing::test_support::mocks::{MockDynSpanEvent, TestLayer, test_data_layer};

#[test]
fn test_convenience_log_message_functions() {
    // Initialize tracing with a custom layer to capture events
    let trace_id = rand::random::<u128>();

    let (test_layer, _, span_ends, log_records) = TestLayer::new();

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
    );

    let mut error_location = Location::caller();
    let mut warn_location = Location::caller();

    tracing::subscriber::with_default(subscriber, || {
        let _rs = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .entered();

        error_location = Location::caller();
        emit_error_log_message(ErrorCode::Generic, "Test error log message");

        warn_location = Location::caller();
        emit_warn_log_message(ErrorCode::AccessDenied, "Test warn log message");
    });

    let log_records = {
        let lr = log_records.lock().expect("Should have no locks");
        lr.clone()
    };
    let span_ends = {
        let se = span_ends.lock().expect("Should have no locks");
        se.clone()
    };

    // Verify we captured 2 events
    assert_eq!(log_records.len(), 2, "Expected 2 log records");
    assert_eq!(span_ends.len(), 1, "Expected 1 span end");

    // Verify error log message
    let error_event = log_records
        .iter()
        .find(|r| r.body == "Test error log message")
        .expect("Should find error log message");
    assert_eq!(error_event.severity_number, SeverityNumber::Error);
    assert_eq!(error_event.severity_text, "ERROR");
    if let Some(lm) = get_log_message(&error_event.attributes) {
        assert_eq!(
            lm.code,
            Some(ErrorCode::Generic as u32),
            "Expected code ErrorCode::Generic"
        );
        assert_eq!(
            lm.original_severity_number,
            SeverityNumber::Error as i32,
            "Expected original severity to be Error"
        );
        assert_eq!(
            lm.original_severity_text, "ERROR",
            "Expected original severity text to be ERROR"
        );
        assert_eq!(lm.file, Some(error_location.file().to_string()));
        assert_eq!(lm.line, Some(error_location.line() + 1));
    } else {
        panic!("Expected LogMessage attributes");
    }

    // Verify warn log message
    let warn_event = log_records
        .iter()
        .find(|r| r.body == "Test warn log message")
        .expect("Should find warn log message");
    assert_eq!(warn_event.severity_number, SeverityNumber::Warn);
    assert_eq!(warn_event.severity_text, "WARN");
    if let Some(lm) = get_log_message(&warn_event.attributes) {
        assert_eq!(
            lm.code,
            Some(ErrorCode::AccessDenied as u32),
            "Expected code ErrorCode::AccessDenied"
        );
        assert_eq!(
            lm.original_severity_number,
            SeverityNumber::Warn as i32,
            "Expected original severity to be Warn"
        );
        assert_eq!(
            lm.original_severity_text, "WARN",
            "Expected original severity text to be WARN"
        );
        assert_eq!(lm.file, Some(warn_location.file().to_string()));
        assert_eq!(lm.line, Some(warn_location.line() + 1));
    } else {
        panic!("Expected LogMessage attributes");
    }
}

#[test]
fn test_emit_error_log_from_fs_error_md_reports_warning() {
    let trace_id = rand::random::<u128>();
    let (test_layer, _, _, log_records) = TestLayer::new();
    let middlewares: Vec<MiddlewareLayer> =
        vec![Box::new(TelemetryMarkdownLogFilter) as MiddlewareLayer];

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            middlewares.into_iter(),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
    );

    tracing::subscriber::with_default(subscriber, || {
        let _rs = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .entered();

        let err = fs_err!(ErrorCode::MacroSyntaxInvalid, "md parse error")
            .with_location(CodeLocationWithFile::new(1, 1, 0, "models/README.md"));
        emit_error_log_from_fs_error(err);
    });

    let log_records = {
        let lr = log_records.lock().expect("Should have no locks");
        lr.clone()
    };

    assert_eq!(log_records.len(), 1);
    let record = &log_records[0];
    assert_eq!(record.severity_number, SeverityNumber::Warn);
    assert_eq!(record.severity_text, "WARN");
    assert!(
        record.body.contains("md parse error"),
        "Expected body to contain error message"
    );
    let log_attrs = get_log_message(&record.attributes).unwrap();
    assert_eq!(log_attrs.relative_path.as_deref(), Some("models/README.md"));
}

#[test]
fn test_emit_error_log_from_fs_error_sql_reports_error() {
    let trace_id = rand::random::<u128>();
    let (test_layer, _, _, log_records) = TestLayer::new();
    let middlewares: Vec<MiddlewareLayer> =
        vec![Box::new(TelemetryMarkdownLogFilter) as MiddlewareLayer];

    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            middlewares.into_iter(),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
    );

    tracing::subscriber::with_default(subscriber, || {
        let _rs = create_root_info_span(MockDynSpanEvent {
            name: "root".to_string(),
            flags: TelemetryOutputFlags::ALL,
            ..Default::default()
        })
        .entered();

        let err = fs_err!(ErrorCode::MacroSyntaxInvalid, "sql parse error")
            .with_location(CodeLocationWithFile::new(1, 1, 0, "models/view.sql"));
        emit_error_log_from_fs_error(err);
    });

    let log_records = {
        let lr = log_records.lock().expect("Should have no locks");
        lr.clone()
    };

    assert_eq!(log_records.len(), 1);
    let record = &log_records[0];
    assert_eq!(record.severity_number, SeverityNumber::Error);
    assert_eq!(record.severity_text, "ERROR");
    assert!(
        record.body.contains("sql parse error"),
        "Expected body to contain error message"
    );
    let log_attrs = get_log_message(&record.attributes).unwrap();
    assert_eq!(log_attrs.relative_path.as_deref(), Some("models/view.sql"));
}

#[test]
fn emit_future_change_appends_error_notice_and_link() {
    let records = capture_log_records(vec![], || {
        emit_future_change(
            ErrorCode::CleanTargetOutsideProject,
            "dbt will not clean 'x'.",
        );
    });

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].severity_number, SeverityNumber::Warn);
    assert_eq!(
        records[0].body,
        "dbt will not clean 'x'. This will become an error in a future release. \
         See https://github.com/dbt-labs/dbt/issues/16929"
    );
}

#[test]
fn emit_future_change_from_fs_error_puts_notice_before_the_location() {
    let location = CodeLocationWithFile::new(17, 9, 0, "models/docs.md");
    let records = capture_log_records(vec![], || {
        emit_future_change_from_fs_error(*dbt_error::fs_err!(
            code => ErrorCode::MalformedBlockName,
            loc => location,
            "Malformed."
        ));
    });

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].severity_number, SeverityNumber::Warn);
    assert_eq!(
        records[0].body,
        "Malformed. This will become an error in a future release. \
         See https://github.com/dbt-labs/dbt/issues/16956\n  --> models/docs.md:17:9"
    );
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "register NoNodesSelected in ErrorCode::future_error")]
fn emit_future_change_rejects_stable_code() {
    capture_log_records(vec![], || {
        emit_future_change(ErrorCode::NoNodesSelected, "Nothing to do.");
    });
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "emit DepsDuplicatePackage with emit_future_change")]
fn plain_warning_rejects_future_error_code() {
    capture_log_records(vec![], || {
        emit_warn_log_message(ErrorCode::DepsDuplicatePackage, "Duplicate.");
    });
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "emit DepsDuplicatePackage with emit_future_change")]
fn plain_error_rejects_future_error_code() {
    capture_log_records(vec![], || {
        emit_error_log_message(ErrorCode::DepsDuplicatePackage, "Duplicate.");
    });
}

#[test]
fn future_incompatibility_is_a_plain_code_warning_with_notice() {
    let records = capture_log_records(vec![], || {
        emit_future_change(NamedProbe::TestOnly, "Something changed.");
    });

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].severity_number, SeverityNumber::Warn);
    assert_eq!(
        records[0].body,
        "Something changed. This behavior may change in a future release. \
         See https://github.com/dbt-labs/dbt/issues/1"
    );
    let log_message = get_log_message(&records[0].attributes).expect("log message");
    assert_eq!(
        log_message.code,
        Some(ErrorCode::FutureIncompatibility as u32)
    );
    assert_eq!(
        log_message.code_name.as_deref(),
        Some("FutureIncompatibility")
    );
    let record = records[0]
        .attributes
        .downcast_ref::<FutureIncompatibilityLog>()
        .expect("FutureIncompatibilityLog");
    assert_eq!(record.probe(), NamedProbe::TestOnly.into());
    // Serializes exactly like the LogMessage it wraps.
    assert_eq!(
        dbt_tracing::AnyTelemetryEvent::to_json(record),
        dbt_tracing::AnyTelemetryEvent::to_json(log_message)
    );
}

#[test]
fn future_incompatibility_counts_its_probe() {
    let mut hits = 0;
    capture_log_records(vec![], || {
        emit_future_change(NamedProbe::TestOnly, "Changed.");
        emit_future_change(NamedProbe::TestOnly, "Changed again.");
        emit_compatibility_probe(NamedProbe::TestOnly);
        hits = get_metric(FusionMetricKey::CompatibilityProbe(
            NamedProbe::TestOnly.into(),
        ));
    });

    assert_eq!(hits, 3);
}

#[test]
fn future_error_counts_its_code_as_a_probe() {
    let mut hits = 0;
    capture_log_records(vec![], || {
        emit_future_change(ErrorCode::MalformedBlockName, "Malformed.");
        emit_future_change_from_fs_error(*dbt_error::fs_err!(
            ErrorCode::MalformedBlockName,
            "Malformed again."
        ));
        hits = get_metric(FusionMetricKey::CompatibilityProbe(
            ErrorCode::MalformedBlockName.into(),
        ));
    });

    assert_eq!(hits, 2);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "emit FutureIncompatibility with emit_future_change")]
fn future_incompatibility_requires_its_helper() {
    capture_log_records(vec![], || {
        emit_warn_log_message(ErrorCode::FutureIncompatibility, "No kind.");
    });
}

#[test]
fn compatibility_probe_increments_its_metric_and_emits_nothing() {
    let mut hits = 0;
    let records = capture_log_records(vec![], || {
        emit_compatibility_probe(NamedProbe::TestOnly);
        emit_compatibility_probe(NamedProbe::TestOnly);
        hits = get_metric(FusionMetricKey::CompatibilityProbe(
            NamedProbe::TestOnly.into(),
        ));
    });

    assert!(records.is_empty());
    assert_eq!(hits, 2);
}

#[test]
fn compatibility_probe_without_root_span_is_dropped() {
    let (test_layer, _span_starts, _span_ends, log_records) = TestLayer::new();
    let consumers: Vec<ConsumerLayer> = vec![Box::new(test_layer)];
    let data_layer = test_data_layer(
        rand::random::<u128>(),
        None,
        false,
        std::iter::empty(),
        consumers.into_iter(),
    );
    let subscriber =
        create_tracing_subcriber_with_layer(tracing::level_filters::LevelFilter::TRACE, data_layer);

    tracing::subscriber::with_default(subscriber, || {
        emit_compatibility_probe(NamedProbe::TestOnly);
    });

    assert!(
        log_records
            .lock()
            .expect("log records mutex poisoned")
            .is_empty()
    );
}
