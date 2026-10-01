use crate::emit::{create_root_info_span, emit_info_event};
use crate::init::create_tracing_subcriber_with_layer;
use crate::layer::{ConsumerLayer, TelemetryConsumer};
use crate::layers::pretty_writer::TelemetryPrettyWriterLayer;
use crate::test_support::mocks::{
    MockDynLogEvent, MockDynSpanEvent, TestLayer, TestWriter, test_data_layer,
};
use crate::{TelemetryOutputFlags, TelemetryRecordRef};

fn mock_format_telemetry_record(record: TelemetryRecordRef, _is_tty: bool) -> Option<String> {
    match record {
        TelemetryRecordRef::LogRecord(log) => Some(format!(
            "[LOG] msg=\"{}\" span=\"{}\"",
            log.body,
            log.span_name.as_deref().unwrap_or_default()
        )),
        TelemetryRecordRef::SpanStart(span) => Some(format!(
            "[SPAN START] name=\"{}\"",
            span.attributes.event_display_name()
        )),
        TelemetryRecordRef::SpanEnd(span) => Some(format!(
            "[SPAN END] name=\"{}\"",
            span.attributes.event_display_name()
        )),
    }
}

#[test]
fn pretty_layer_applies_filter_and_formatting() {
    let trace_id = rand::random::<u128>();
    let file_writer = TestWriter::non_terminal();
    let tty_writer = TestWriter::terminal();

    let (test_layer, _, _, test_logs) = TestLayer::new();

    let file_layer =
        TelemetryPrettyWriterLayer::new(file_writer.clone(), mock_format_telemetry_record)
            .with_span_filter(|span| span.span_name.contains("keep"));
    let tty_layer =
        TelemetryPrettyWriterLayer::new(tty_writer.clone(), mock_format_telemetry_record)
            .with_span_filter(|span| span.span_name.contains("keep"));

    // Init telemetry using internal API allowing to set thread local subscriber.
    // This avoids collisions with other unit tests
    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None,
            false,
            std::iter::empty(),
            vec![
                Box::new(file_layer) as ConsumerLayer,
                Box::new(tty_layer) as ConsumerLayer,
                Box::new(test_layer) as ConsumerLayer,
            ]
            .into_iter(),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let span = create_root_info_span(MockDynSpanEvent {
            name: "span-keep".into(),
            flags: TelemetryOutputFlags::OUTPUT_LOG_FILE,
            ..Default::default()
        });

        span.in_scope(|| {
            create_root_info_span(MockDynSpanEvent {
                name: "span-drop".into(),
                flags: TelemetryOutputFlags::ALL,
                ..Default::default()
            })
            .in_scope(|| {
                emit_info_event(
                    MockDynLogEvent {
                        code: 1,
                        flags: TelemetryOutputFlags::OUTPUT_LOG_FILE,
                        ..Default::default()
                    },
                    Some("file only"),
                );

                emit_info_event(
                    MockDynLogEvent {
                        code: 2,
                        flags: TelemetryOutputFlags::OUTPUT_CONSOLE,
                        ..Default::default()
                    },
                    Some("console only"),
                );
            });
        });
    });

    // Check that the file writer (non-TTY) received the expected lines

    let file_log = file_writer.get_lines().join("");
    let tty_log = tty_writer.get_lines().join("");

    let captured_logs = test_logs.lock().expect("log capture mutex poisoned");
    assert_eq!(captured_logs.len(), 2);
    assert!(
        captured_logs[0]
            .attributes
            .output_flags()
            .contains(TelemetryOutputFlags::OUTPUT_LOG_FILE),
        "expected first log to target log file"
    );
    assert!(
        !captured_logs[1]
            .attributes
            .output_flags()
            .contains(TelemetryOutputFlags::OUTPUT_LOG_FILE),
        "expected second log to be console-only, flags={:?}",
        captured_logs[1].attributes.output_flags()
    );

    // Note that log reports span-drop as parent - that's the current
    // limitation of our custom filtering - see comments in event_info.rs
    assert_eq!(
        file_log,
        r#"
[SPAN START] name="Mock Dyn Span Event: span-keep"
[LOG] msg="file only" span="Mock Dyn Span Event: span-drop"
[SPAN END] name="Mock Dyn Span Event: span-keep"
"#
        .trim_start(),
        "Unexpected file (non-TTY) log: {file_log}"
    );

    assert_eq!(
        tty_log,
        r#"
[LOG] msg="console only" span="Mock Dyn Span Event: span-drop"
"#
        .trim_start(),
        "Unexpected TTY log: {tty_log}"
    );
}
