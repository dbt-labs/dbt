use crate::{CodeLocationWithFile, fs_err};
use dbt_error::ErrorCode;
use dbt_tracing::{SeverityNumber, TelemetryOutputFlags};
use std::panic::Location;

use crate::tracing::dbt_init::create_tracing_subcriber_with_layer;
use crate::tracing::{
    dbt_emit::{emit_error_log_from_fs_error, emit_error_log_message, emit_warn_log_message},
    fs_error_log::get_log_message,
    layer::{ConsumerLayer, MiddlewareLayer},
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
