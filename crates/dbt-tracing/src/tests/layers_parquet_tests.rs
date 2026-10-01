use crate::emit::{create_info_span_with_parent, emit_info_event};
use crate::event_info::store_event_attributes;
use crate::init::create_tracing_subcriber_with_layer;
use crate::layers::data_layer::get_span_start_info_from_span;
use crate::layers::parquet_writer::build_parquet_writer_layer;
use crate::span_info;
use crate::test_support::mocks::{
    MockDynLogEvent, MockDynSpanEvent, MockTelemetryEventRegistry, MockUnknown, test_data_layer,
};
use crate::{
    LogRecordInfo, RecordCodeLocation, SeverityNumber, SpanEndInfo, TelemetryAttributes,
    TelemetryOutputFlags, TelemetryRecord, serialize::arrow::deserialize_from_arrow,
};
use std::{fs, panic::Location, time::SystemTime};

#[test]
#[allow(clippy::cognitive_complexity)]
fn test_tracing_parquet_filtering() {
    let trace_id = rand::random::<u128>();

    // Create a temporary file for the parquet output
    let temp_dir = tempfile::tempdir().expect("Failed to create temporary test directory");
    let temp_file_path = temp_dir.path().join("test_telemetry_filtering.parquet");

    let (parquet_layer, mut shutdown_handle) =
        build_parquet_writer_layer::<_, MockTelemetryEventRegistry>(
            fs::File::create(&temp_file_path).expect("Failed to create temporary OTM file"),
        )
        .expect("Failed to create parquet layer");

    // Init telemetry using internal API allowing to set thread local subscriber.
    // This avoids collisions with other unit tests
    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            std::iter::once(parquet_layer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    // Pre-create attrs to compare them later
    let mut test_log_attrs: TelemetryAttributes = MockDynLogEvent {
        code: 42,
        flags: TelemetryOutputFlags::EXPORT_PARQUET,
        file: None,
        line: None,
        ..Default::default()
    }
    .into();

    let mut dev_span_attrs: TelemetryAttributes = MockUnknown {
        name: "dev_test".to_string(),
        file: String::new(),
        line: 0,
        flags: TelemetryOutputFlags::EXPORT_PARQUET,
    }
    .into();

    let before_start = SystemTime::now();

    // We do not need location here, but this is easier than unwrapping later
    let mut test_span_location = Location::caller();
    let mut test_log_location = Location::caller();
    // Same for expected span id
    let mut expected_span_id = 0;

    tracing::subscriber::with_default(subscriber, || {
        test_span_location = Location::caller();
        let dev_span = tracing::trace_span!(
            "dev_internal_span",
            _e = ?store_event_attributes(dev_span_attrs.clone())
        );

        let _sp = dev_span.enter();

        span_info::with_span(&dev_span, |span_ref| {
            expected_span_id = get_span_start_info_from_span(&span_ref).unwrap().span_id;
        });

        // Emit a log with Log attributes (should be included) & save the location (almost, one line off)
        test_log_location = Location::caller();
        emit_info_event(test_log_attrs.clone(), Some("Valid log message"));

        // Emit mock span without EXPORT_PARQUET flag (should be filtered out)
        let mock_span_attrs: TelemetryAttributes = MockDynSpanEvent {
            name: "filtered_span".to_string(),
            flags: TelemetryOutputFlags::EXPORT_JSONL_AND_OTLP,
            ..Default::default()
        }
        .into();
        create_info_span_with_parent(dev_span.id(), mock_span_attrs);

        // Emit mock log without EXPORT_PARQUET flag (should be filtered out)
        let mock_log_attrs: TelemetryAttributes = MockDynLogEvent {
            code: 999,
            flags: TelemetryOutputFlags::EXPORT_JSONL_AND_OTLP,
            ..Default::default()
        }
        .into();
        emit_info_event(mock_log_attrs, Some("This log should be filtered out"));
    });

    // Shutdown telemetry to ensure all data is flushed to the file
    shutdown_handle
        .shutdown()
        .expect("Failed to shutdown telemetry");

    // Verify the parquet file was created
    assert!(temp_file_path.exists(), "Parquet file should exist");

    // Read back and deserialize the parquet file
    let file = fs::File::open(&temp_file_path).unwrap();
    let reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file)
        .unwrap()
        .build()
        .unwrap();

    let mut all_records = Vec::new();
    let registry = MockTelemetryEventRegistry;
    for batch_result in reader {
        let batch = batch_result.unwrap();
        let (mut records, errors) = deserialize_from_arrow(&batch, &registry).unwrap();
        assert!(
            errors.is_empty(),
            "unexpected deserialize errors: {errors:?}"
        );
        all_records.append(&mut records);
    }

    // Verify filtering worked correctly - should have 3 records (1 SpanStart, 1 SpanEnd with Process attrs, 1 LogRecord with Log attrs)
    assert_eq!(all_records.len(), 3, "Expected 3 records after filtering");

    // Verify we have the correct records
    let span_start_record = all_records
        .iter()
        .find(|r| matches!(r, TelemetryRecord::SpanStart(_)))
        .expect("Expected a SpanStart record");
    let span_end_record = all_records
        .iter()
        .find(|r| matches!(r, TelemetryRecord::SpanEnd(_)))
        .expect("Expected a SpanEnd record");
    let log_record_record = all_records
        .iter()
        .find(|r| matches!(r, TelemetryRecord::LogRecord(_)))
        .expect("Expected a LogRecord");

    // Verify the SpanStart record is the valid one
    if let TelemetryRecord::SpanStart(span_start_info) = span_start_record {
        assert_eq!(span_start_info.trace_id, trace_id);
        assert_eq!(span_start_info.span_id, expected_span_id);
        assert!(
            span_start_info
                .span_name
                .starts_with("Mock Unknown Span: dev_test")
        );
        assert!(span_start_info.parent_span_id.is_none());
        assert!(span_start_info.links.is_none());
        assert_eq!(span_start_info.severity_number, SeverityNumber::Trace);
        assert_eq!(span_start_info.severity_text, "TRACE");
        assert!(span_start_info.start_time_unix_nano > before_start);
    } else {
        panic!("Expected a SpanStart record");
    }

    // Verify the SpanEnd record is the valid one
    if let TelemetryRecord::SpanEnd(SpanEndInfo {
        trace_id: recorded_trace_id,
        span_id,
        span_name,
        parent_span_id,
        links,
        start_time_unix_nano,
        end_time_unix_nano,
        severity_number,
        severity_text,
        status,
        attributes,
    }) = span_end_record
    {
        assert_eq!(*recorded_trace_id, trace_id);
        assert_eq!(*span_id, expected_span_id);
        assert!(span_name.starts_with("Mock Unknown Span: dev_test"));
        assert!(parent_span_id.is_none());
        assert!(links.is_none());
        assert_eq!(*severity_number, SeverityNumber::Trace);
        assert_eq!(severity_text, "TRACE");
        assert!(*start_time_unix_nano > before_start);
        assert!(*end_time_unix_nano > before_start);
        assert_eq!(*status, None);

        // Now, the actual attributes that we should get back must include the location
        let expected_location = RecordCodeLocation {
            file: Some(test_span_location.file().to_string()),
            line: Some(test_span_location.line() + 1),
            module_path: Some(std::module_path!().to_string()),
            target: Some(std::module_path!().to_string()),
        };

        dev_span_attrs
            .inner_mut()
            .with_code_location(expected_location);

        assert_eq!(*attributes, dev_span_attrs);
    } else {
        panic!("Expected a SpanEnd record");
    };

    // Verify the LogRecord is the valid one (Log attributes)
    if let TelemetryRecord::LogRecord(LogRecordInfo {
        trace_id: recorded_trace_id,
        span_id,
        event_id: _,
        span_name,
        time_unix_nano,
        body,
        severity_number,
        severity_text,
        attributes,
    }) = log_record_record
    {
        assert_eq!(*recorded_trace_id, trace_id);
        assert_eq!(*span_id, Some(expected_span_id));
        assert!(
            span_name
                .clone()
                .expect("Span must be set")
                .starts_with("Mock Unknown Span: dev_test")
        );
        assert!(*time_unix_nano > before_start);
        assert_eq!(body, "Valid log message");
        assert_eq!(*severity_number, SeverityNumber::Info);
        assert_eq!(*severity_text, "INFO");

        // Now, the actual attributes that we should get back must include the location
        let expected_location = RecordCodeLocation {
            file: Some(test_log_location.file().to_string()),
            line: Some(test_log_location.line() + 1),
            module_path: Some(std::module_path!().to_string()),
            target: Some(std::module_path!().to_string()),
        };

        test_log_attrs
            .inner_mut()
            .with_code_location(expected_location);
        assert_eq!(*attributes, test_log_attrs);
    } else {
        panic!("Expected a LogRecord");
    }

    // Clean up
    let _ = fs::remove_file(&temp_file_path);
}
