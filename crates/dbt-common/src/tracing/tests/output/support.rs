//! Harness shared by the output tests: describes a case, runs it through the real layers
//! and compares what each sink wrote with what the case expects.

use std::{
    collections::BTreeSet,
    fmt::{self, Write as _},
    io,
    panic::{self, AssertUnwindSafe, Location},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

use dbt_telemetry::{Invocation, TelemetryEventTypeRegistry};
use dbt_tracing::{
    LogRecordInfo, SpanEndInfo, SpanStartInfo, TelemetryAttributes, TelemetryOutputFlags,
    data_provider::DataProvider,
    layer::{ConsumerLayer, MiddlewareLayer, TelemetryMiddleware},
    test_support::mocks::{MockDynSpanEvent, test_data_layer},
};
use tracing::level_filters::LevelFilter;

use crate::{
    collections::{HashMap, HashSet},
    io_args::{FsCommand, ShowOptions},
    tracing::{
        dbt_init::create_tracing_subcriber_with_layer,
        emit::create_root_info_span,
        layers::{
            file_log_layer::build_file_log_layer_with_background_writer,
            tui_layer::{DelayedMessages, TuiConfig, TuiLayer},
        },
        middlewares::metric_aggregator::TelemetryMetricAggregator,
    },
};

/// Lists every variant of a proto enum.
///
/// The exhaustive `match` stops compiling when the proto gains a variant, which points at
/// the matrix that has to cover it.
macro_rules! all_variants {
    ($enum:ident { $($variant:ident),+ $(,)? }) => {{
        let _exhaustive = |value: $enum| match value {
            $($enum::$variant => ()),+
        };
        [$($enum::$variant),+]
    }};
}
pub(super) use all_variants;

/// Start of the virtual clock: 2026-01-02T03:04:05.678901Z.
fn clock_origin() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_micros(1_767_323_045_678_901)
}

/// Time source of a scenario. Spans and logs are stamped with its current time, and the
/// scenario moves it forward explicitly, so every timestamp and duration is reproducible.
#[derive(Clone)]
pub(super) struct Clock(Arc<Mutex<ClockState>>);

struct ClockState {
    now: SystemTime,
    span_starts: HashMap<u64, SystemTime>,
}

impl Clock {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(ClockState {
            now: clock_origin(),
            span_starts: HashMap::default(),
        })))
    }

    pub(super) fn advance(&self, by: Duration) {
        self.0.lock().expect("clock poisoned").now += by;
    }

    pub(super) fn advance_ms(&self, millis: u64) {
        self.advance(Duration::from_millis(millis));
    }

    fn now(&self) -> SystemTime {
        self.0.lock().expect("clock poisoned").now
    }
}

impl TelemetryMiddleware for Clock {
    fn on_span_start(
        &self,
        mut span: SpanStartInfo,
        _: &mut DataProvider<'_>,
    ) -> Option<SpanStartInfo> {
        let mut state = self.0.lock().expect("clock poisoned");
        let now = state.now;
        state.span_starts.insert(span.span_id, now);
        span.start_time_unix_nano = now;
        Some(span)
    }

    fn on_span_end(&self, mut span: SpanEndInfo, _: &mut DataProvider<'_>) -> Option<SpanEndInfo> {
        let mut state = self.0.lock().expect("clock poisoned");
        let now = state.now;
        span.start_time_unix_nano = state.span_starts.remove(&span.span_id).unwrap_or(now);
        span.end_time_unix_nano = now;
        Some(span)
    }

    fn on_log_record(
        &self,
        mut record: LogRecordInfo,
        _: &mut DataProvider<'_>,
    ) -> Option<LogRecordInfo> {
        record.time_unix_nano = self.now();
        Some(record)
    }
}

/// Event types the cases have emitted, as spans or log records.
#[derive(Clone, Default)]
struct EmittedEvents(Arc<Mutex<BTreeSet<&'static str>>>);

impl EmittedEvents {
    fn record(&self, attributes: &TelemetryAttributes) {
        self.0
            .lock()
            .expect("emitted events poisoned")
            .insert(attributes.event_type());
    }

    fn take(&self) -> BTreeSet<&'static str> {
        std::mem::take(&mut self.0.lock().expect("emitted events poisoned"))
    }
}

impl TelemetryMiddleware for EmittedEvents {
    fn on_span_start(
        &self,
        span: SpanStartInfo,
        _: &mut DataProvider<'_>,
    ) -> Option<SpanStartInfo> {
        self.record(&span.attributes);
        Some(span)
    }

    fn on_log_record(
        &self,
        record: LogRecordInfo,
        _: &mut DataProvider<'_>,
    ) -> Option<LogRecordInfo> {
        self.record(&record.attributes);
        Some(record)
    }
}

/// Notes when a root span other than an invocation ends while the terminal still holds
/// output back for the invocation summary. That output is never printed, so the case
/// would silently miss it.
#[derive(Clone, Default)]
struct DroppedOutput(Arc<AtomicBool>);

impl TelemetryMiddleware for DroppedOutput {
    fn on_span_end(
        &self,
        span: SpanEndInfo,
        data_provider: &mut DataProvider<'_>,
    ) -> Option<SpanEndInfo> {
        if span.parent_span_id.is_none() && !span.attributes.is::<Invocation>() {
            data_provider.with_root::<DelayedMessages>(|delayed| {
                if !delayed.is_empty() {
                    self.0.store(true, Ordering::Relaxed);
                }
            });
        }
        Some(span)
    }
}

/// Root span of a case that no layer renders.
fn quiet_root() -> TelemetryAttributes {
    MockDynSpanEvent {
        name: "root".to_string(),
        flags: TelemetryOutputFlags::OUTPUT_ALL,
        ..Default::default()
    }
    .into()
}

/// Runs `scenario` inside a `root` span with `consumer` as the only consumer.
///
/// Returns whether the consumer was left holding output for an invocation that never ended.
fn run(
    consumer: ConsumerLayer,
    root: TelemetryAttributes,
    emitted: &EmittedEvents,
    scenario: &dyn Fn(&Clock),
) -> DroppedOutput {
    let clock = Clock::new();
    let dropped = DroppedOutput::default();
    let middlewares: [MiddlewareLayer; 4] = [
        Box::new(emitted.clone()),
        Box::new(dropped.clone()),
        Box::new(clock.clone()),
        Box::new(TelemetryMetricAggregator),
    ];
    let subscriber = create_tracing_subcriber_with_layer(
        LevelFilter::TRACE,
        test_data_layer(
            rand::random::<u128>(),
            None,
            false,
            middlewares.into_iter(),
            std::iter::once(consumer),
        ),
    );

    tracing::subscriber::with_default(subscriber, || {
        let root = create_root_info_span(root);
        root.in_scope(|| scenario(&clock));
        drop(root);
    });
    dropped
}

/// In-memory `io::Write` sink that can be read back after the writer is shut down.
#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl SharedBuffer {
    fn contents(&self) -> String {
        String::from_utf8(self.0.lock().expect("buffer poisoned").clone())
            .expect("output is not utf-8")
    }
}

impl io::Write for SharedBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("buffer poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// How the terminal (TUI) is configured for a case.
#[derive(Clone, Copy)]
pub(super) struct Terminal {
    /// `--log-format default` on a terminal: progress bars, with lines grouped under them
    pub(super) interactive: bool,
    pub(super) level: LevelFilter,
    pub(super) command: FsCommand,
    pub(super) show: &'static [ShowOptions],
    pub(super) quiet: bool,
    /// Whether output is styled with ANSI colors
    pub(super) colorize: bool,
}

impl Terminal {
    /// What `dbt build` shows on a terminal by default.
    pub(super) const INTERACTIVE: Self = Self {
        interactive: true,
        level: LevelFilter::INFO,
        command: FsCommand::Build,
        show: &[ShowOptions::Progress, ShowOptions::Completed],
        quiet: false,
        colorize: true,
    };

    /// What `dbt build --log-format text` prints on a terminal.
    pub(super) const TEXT: Self = Self {
        interactive: false,
        ..Self::INTERACTIVE
    };

    /// `dbt build --log-format text --debug --show all` on a terminal.
    pub(super) const DEBUG: Self = Self {
        interactive: false,
        level: LevelFilter::DEBUG,
        show: &[ShowOptions::All],
        ..Self::INTERACTIVE
    };

    pub(super) const fn command(self, command: FsCommand) -> Self {
        Self { command, ..self }
    }

    pub(super) const fn show(self, show: &'static [ShowOptions]) -> Self {
        Self { show, ..self }
    }
}

/// A sink a case is rendered into, with its configuration.
#[derive(Clone, Copy)]
pub(super) enum Sink {
    /// `logs/dbt.log` at the given verbosity
    DbtLog(LevelFilter),
    /// The terminal, labeled
    Terminal(&'static str, Terminal),
}

/// `logs/dbt.log` at its default verbosity.
pub(super) const DBT_LOG: Sink = Sink::DbtLog(LevelFilter::INFO);
/// `logs/dbt.log` with `--log-level-file debug`.
pub(super) const DBT_LOG_DEBUG: Sink = Sink::DbtLog(LevelFilter::DEBUG);
/// `logs/dbt.log` with `--log-level-file trace`.
pub(super) const DBT_LOG_TRACE: Sink = Sink::DbtLog(LevelFilter::TRACE);
/// The interactive terminal `dbt build` shows by default.
pub(super) const TERMINAL: Sink = Sink::Terminal("terminal", Terminal::INTERACTIVE);
/// `--quiet` on a terminal: no decoration, only results and problems.
pub(super) const TERMINAL_QUIET: Sink = Sink::Terminal(
    "terminal --quiet",
    Terminal {
        show: &[],
        quiet: true,
        ..Terminal::INTERACTIVE
    },
);
/// `NO_COLOR` on a terminal.
pub(super) const TERMINAL_NO_COLOR: Sink = Sink::Terminal(
    "terminal NO_COLOR",
    Terminal {
        colorize: false,
        ..Terminal::INTERACTIVE
    },
);
/// `--log-format text` on a terminal.
pub(super) const TEXT: Sink = Sink::Terminal("text", Terminal::TEXT);
/// `--log-format text --debug --show all` on a terminal.
pub(super) const TEXT_DEBUG: Sink = Sink::Terminal("text --debug --show all", Terminal::DEBUG);

/// What a sink wrote for a case.
struct Rendered {
    stdout: String,
    stderr: String,
    /// Output held back for the end of the invocation, which the root never printed
    dropped_output: bool,
}

impl Sink {
    fn label(&self) -> String {
        match self {
            Self::DbtLog(level) if *level == LevelFilter::INFO => "dbt.log".to_string(),
            Self::DbtLog(level) => format!("dbt.log ({level})"),
            Self::Terminal(label, _) => (*label).to_string(),
        }
    }

    fn render(
        &self,
        root: TelemetryAttributes,
        emitted: &EmittedEvents,
        scenario: &dyn Fn(&Clock),
    ) -> Rendered {
        match self {
            Self::DbtLog(level) => {
                let buffer = SharedBuffer::default();
                let (layer, mut shutdown) =
                    build_file_log_layer_with_background_writer(buffer.clone(), *level);
                let dropped = run(layer, root, emitted, scenario);
                shutdown.shutdown().expect("file log writer failed");
                Rendered {
                    stdout: buffer.contents(),
                    stderr: String::new(),
                    dropped_output: dropped.0.load(Ordering::Relaxed),
                }
            }
            Self::Terminal(_, setup) => {
                let (layer, captured) = TuiLayer::new_captured(
                    setup.level,
                    setup.interactive,
                    setup.colorize,
                    TuiConfig {
                        show_options: setup.show.iter().copied().collect::<HashSet<_>>(),
                        command: setup.command,
                        quiet: setup.quiet,
                    },
                );
                let dropped = run(Box::new(layer), root, emitted, scenario);

                let read = |buffer: &Mutex<Vec<u8>>| {
                    String::from_utf8(buffer.lock().expect("buffer poisoned").clone())
                        .expect("output is not utf-8")
                };
                Rendered {
                    stdout: read(&captured.stdout),
                    stderr: read(&captured.stderr),
                    dropped_output: dropped.0.load(Ordering::Relaxed),
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stream {
    Stdout,
    Stderr,
}

struct Expectation {
    sink: Sink,
    stream: Stream,
    output: String,
}

/// A scenario and the exact output it produces in each sink it is rendered into.
///
/// Every sink with an expectation renders the scenario from scratch. A terminal sink
/// checks both streams: one without an expectation must stay empty.
/// [`Case::expect_no_output`] expects a sink to write nothing at all.
pub(super) struct Case {
    name: String,
    defined_at: &'static Location<'static>,
    root: TelemetryAttributes,
    scenario: Box<dyn Fn(&Clock)>,
    expectations: Vec<Expectation>,
}

impl Case {
    #[track_caller]
    pub(super) fn new(name: impl fmt::Display) -> Self {
        Self {
            name: name.to_string(),
            defined_at: Location::caller(),
            root: quiet_root(),
            scenario: Box::new(|_| {}),
            expectations: Vec::new(),
        }
    }

    /// Runs the scenario under `root` instead of a root span no sink renders.
    pub(super) fn root(mut self, root: impl Into<TelemetryAttributes>) -> Self {
        self.root = root.into();
        self
    }

    /// What the case emits. It runs once per sink, inside the root span.
    pub(super) fn run(mut self, scenario: impl Fn(&Clock) + 'static) -> Self {
        self.scenario = Box::new(scenario);
        self
    }

    /// Expects `sink` to write exactly `output`, to stdout for a terminal.
    ///
    /// `output` is in the readable form produced by [`visible`].
    #[track_caller]
    pub(super) fn expect(self, sink: Sink, output: &str) -> Self {
        self.expect_output(sink, Stream::Stdout, output)
    }

    /// Expects the terminal `sink` to write exactly `output` to stderr.
    #[track_caller]
    pub(super) fn expect_stderr(self, sink: Sink, output: &str) -> Self {
        assert!(
            matches!(sink, Sink::Terminal(..)),
            "case {:?}: only the terminal writes to stderr",
            self.name
        );
        self.expect_output(sink, Stream::Stderr, output)
    }

    /// Expects `sink` to write nothing at all, to any stream.
    #[track_caller]
    pub(super) fn expect_no_output(self, sink: Sink) -> Self {
        let case = self.expect_stream(sink, Stream::Stdout, "");
        match sink {
            Sink::DbtLog(_) => case,
            Sink::Terminal(..) => case.expect_stream(sink, Stream::Stderr, ""),
        }
    }

    /// Expects `sink` to write exactly `output`, or nothing at all for `None`. For
    /// matrices where only some rows print.
    #[track_caller]
    pub(super) fn expect_or_no_output(self, sink: Sink, output: Option<&str>) -> Self {
        match output {
            Some(output) => self.expect(sink, output),
            None => self.expect_no_output(sink),
        }
    }

    #[track_caller]
    fn expect_output(self, sink: Sink, stream: Stream, output: &str) -> Self {
        assert!(
            !output.is_empty(),
            "case {:?}: expect no output from {} with `expect_no_output`",
            self.name,
            sink.label()
        );
        self.expect_stream(sink, stream, output)
    }

    #[track_caller]
    fn expect_stream(mut self, sink: Sink, stream: Stream, output: &str) -> Self {
        assert!(
            !self
                .expectations
                .iter()
                .any(|e| e.stream == stream && e.sink.label() == sink.label()),
            "case {:?} expects {} twice",
            self.name,
            sink.label()
        );
        self.expectations.push(Expectation {
            sink,
            stream,
            output: output.to_string(),
        });
        self
    }

    /// Renders the case into each sink it has expectations for and describes every
    /// stream whose output differs.
    fn check(&self, emitted: &EmittedEvents) -> Vec<String> {
        let mut sinks: Vec<Sink> = Vec::new();
        for expectation in &self.expectations {
            if !sinks.iter().any(|s| s.label() == expectation.sink.label()) {
                sinks.push(expectation.sink);
            }
        }

        let mut mismatches = Vec::new();
        for sink in sinks {
            let label = sink.label();
            let rendered = sink.render(self.root.clone(), emitted, &*self.scenario);
            if rendered.dropped_output {
                mismatches.push(format!(
                    "[{label}]\nheld back output that is only printed when an invocation ends; \
                     run the case under `.root(invocation(..))`"
                ));
            }
            for (stream, actual) in [
                (Stream::Stdout, rendered.stdout),
                (Stream::Stderr, rendered.stderr),
            ] {
                let expected = self
                    .expectations
                    .iter()
                    .find(|e| e.stream == stream && e.sink.label() == label)
                    .map_or("", |e| e.output.as_str());
                let actual = visible(&actual);
                if actual != expected {
                    let target = match stream {
                        Stream::Stdout => label.clone(),
                        Stream::Stderr => format!("{label}, stderr"),
                    };
                    mismatches.push(format!("[{target}]\n{}", diff(expected, &actual)));
                }
            }
        }
        mismatches
    }
}

fn diff(expected: &str, actual: &str) -> String {
    similar::TextDiff::from_lines(expected, actual)
        .unified_diff()
        .context_radius(1_000)
        .header("expected", "actual")
        .to_string()
}

/// Runs every case, then checks that the cases emit every public event.
///
/// All cases run even after a failure, so one run reports every difference and every
/// event no case emits.
pub(super) fn check_cases(cases: Vec<Case>) {
    let mut seen = HashSet::default();
    let duplicates: Vec<_> = cases
        .iter()
        .filter(|case| !seen.insert((case.defined_at.file(), case.name.as_str())))
        .map(|case| format!("{}: {:?}", case.defined_at, case.name))
        .collect();
    assert!(
        duplicates.is_empty(),
        "case names must be unique within a file:\n{}",
        duplicates.join("\n")
    );

    let emitted = EmittedEvents::default();
    let mut emitted_by_cases = BTreeSet::new();
    let mut failures = Vec::new();
    for case in &cases {
        assert!(
            !case.expectations.is_empty(),
            "{}: case {:?} expects nothing",
            case.defined_at,
            case.name
        );

        let result = panic::catch_unwind(AssertUnwindSafe(|| case.check(&emitted)));
        emitted_by_cases.append(&mut emitted.take());
        let mismatches = match result {
            Ok(mismatches) => mismatches,
            Err(_) => vec!["panicked, see the panic message above".to_string()],
        };
        if !mismatches.is_empty() {
            failures.push(format!(
                "{}: case {:?}\n{}",
                case.defined_at,
                case.name,
                mismatches.join("\n")
            ));
        }
    }

    let mut problems = Vec::new();
    if !failures.is_empty() {
        problems.push(format!(
            "{} of {} output cases differ:\n\n{}\n\
             `-` lines are what a case expects, `+` lines what the sink writes now. If the new \
             output is intended, update the expected text of each listed case at its file:line, \
             in the readable form described in `tracing/tests/output/mod.rs`. Otherwise, the \
             change broke the output: fix the formatting code instead.",
            failures.len(),
            cases.len(),
            failures.join("\n")
        ));
    }

    let missing: BTreeSet<_> = TelemetryEventTypeRegistry::public()
        .iter()
        .filter(|event_type| !emitted_by_cases.contains(event_type))
        .collect();
    if !missing.is_empty() {
        problems.push(format!(
            "no output case emits these public events: {missing:?}\n\
             Add cases pinning their output, or to `unrendered_tests.rs` if no sink renders them."
        ));
    }

    assert!(problems.is_empty(), "{}", problems.join("\n\n"));
}

/// Renders raw output in the readable form expectations are written in.
///
/// - ANSI SGR sequences become tags: `\x1b[32m\x1b[1mPassed\x1b[0m` is `<green+bold>Passed</>`.
/// - A line ending in whitespace gets a trailing `⏎`, so editors cannot strip the padding.
/// - Output that does not end with a newline gets a final `\ no newline at end` line.
fn visible(output: &str) -> String {
    if output.is_empty() {
        return String::new();
    }

    let tagged = tag_ansi(output);
    let mut rendered = String::with_capacity(tagged.len());
    let (body, has_final_newline) = match tagged.strip_suffix('\n') {
        Some(body) => (body, true),
        None => (tagged.as_str(), false),
    };

    for line in body.split('\n') {
        rendered.push_str(line);
        if line.ends_with([' ', '\t']) {
            rendered.push('⏎');
        }
        rendered.push('\n');
    }
    if !has_final_newline {
        rendered.push_str("\\ no newline at end\n");
    }

    rendered
}

/// Replaces ANSI SGR sequences with readable tags, merging adjacent ones.
fn tag_ansi(output: &str) -> String {
    let mut tagged = String::with_capacity(output.len());
    let mut pending: Vec<&'static str> = Vec::new();
    let flush = |pending: &mut Vec<&'static str>, tagged: &mut String| {
        if !pending.is_empty() {
            write!(tagged, "<{}>", pending.join("+")).expect("writing to a String");
            pending.clear();
        }
    };

    let mut rest = output;
    while let Some(start) = rest.find("\x1b[") {
        let (text, sequence) = rest.split_at(start);
        if !text.is_empty() {
            flush(&mut pending, &mut tagged);
            tagged.push_str(text);
        }

        let end = sequence
            .find('m')
            .expect("only SGR escape sequences are expected");
        for code in sequence[2..end].split(';') {
            match code {
                "0" | "" => {
                    flush(&mut pending, &mut tagged);
                    tagged.push_str("</>");
                }
                code => pending.push(sgr_name(code)),
            }
        }
        rest = &sequence[end + 1..];
    }
    flush(&mut pending, &mut tagged);
    tagged.push_str(rest);

    tagged
}

fn sgr_name(code: &str) -> &'static str {
    match code {
        "1" => "bold",
        "2" => "dim",
        "3" => "italic",
        "4" => "underline",
        "30" => "black",
        "31" => "red",
        "32" => "green",
        "33" => "yellow",
        "34" => "blue",
        "35" => "magenta",
        "36" => "cyan",
        "37" => "white",
        other => panic!("unexpected SGR code {other}"),
    }
}
