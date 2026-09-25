use crate::{
    AnyTelemetryEvent, LogRecordInfo, RecordCodeLocation, SpanEndInfo, SpanStartInfo,
    TelemetryAttributes, TelemetryContext, TelemetryEventRecType, TelemetryOutputFlags,
    data_provider::DataProvider,
    layer::{ConsumerLayer, MiddlewareLayer, TelemetryConsumer, TelemetryMiddleware},
    layers::data_layer::{
        RootSpanTraceContext, TelemetryDataLayer, TelemetryDataLayerConfig,
        UnstructuredLogAttributesInput, UnstructuredSpanAttributesInput,
    },
    serialize::traits::{
        ArrowAttributesSerialize, ArrowRegistryLookup, TelemetryAttributeDeserializeError,
    },
    shared_writer::SharedWriter,
};
use arrow::datatypes::{DataType, Field, Fields};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use tracing::Subscriber;
use tracing_subscriber::registry::LookupSpan;

pub fn test_data_layer<S>(
    trace_id: u128,
    parent_span_id: Option<u64>,
    strip_code_location: bool,
    middlewares: impl Iterator<Item = MiddlewareLayer>,
    consumers: impl Iterator<Item = ConsumerLayer>,
) -> TelemetryDataLayer<S>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    TelemetryDataLayer::new(
        test_data_layer_config(trace_id, parent_span_id),
        strip_code_location,
        middlewares,
        consumers,
    )
}

pub(crate) fn test_data_layer_config(
    trace_id: u128,
    parent_span_id: Option<u64>,
) -> TelemetryDataLayerConfig {
    TelemetryDataLayerConfig::new(
        trace_id,
        parent_span_id,
        test_unstructured_span_attributes,
        test_unstructured_log_attributes,
        test_root_span_trace_context,
    )
}

fn test_unstructured_span_attributes(
    input: UnstructuredSpanAttributesInput<'_>,
) -> TelemetryAttributes {
    MockUnknown {
        name: input.name.to_string(),
        file: input
            .location
            .file
            .unwrap_or_else(|| "<unknown>".to_string()),
        line: input.location.line.unwrap_or_default(),
        flags: TelemetryOutputFlags::ALL,
    }
    .into()
}

fn test_unstructured_log_attributes(input: UnstructuredLogAttributesInput) -> TelemetryAttributes {
    MockDynLogEvent {
        code: input.severity_number as i32,
        flags: TelemetryOutputFlags::EXPORT_JSONL_AND_OTLP,
        file: input.location.file,
        line: input.location.line,
        ..Default::default()
    }
    .into()
}

fn test_root_span_trace_context(attributes: &TelemetryAttributes) -> Option<RootSpanTraceContext> {
    let root = attributes.downcast_ref::<MockRootSpanEvent>()?;

    Some(RootSpanTraceContext {
        trace_id: root.trace_id?,
        parent_span_id: root.parent_span_id,
    })
}

fn serialize_flags<S>(flags: &TelemetryOutputFlags, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_u32(flags.bits())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestTelemetryContext {
    pub workflow_name: String,
    pub attempt: u32,
}

// Mock dynamic span event with instance-based export flags
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct MockDynSpanEvent {
    pub name: String,
    #[serde(serialize_with = "serialize_flags")]
    pub flags: TelemetryOutputFlags,
    pub has_sensitive: bool,
    pub was_scrubbed: bool,
    pub context: Option<TestTelemetryContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MockArrowAttributes<'a> {
    pub name: Option<Cow<'a, str>>,
    pub code: Option<i32>,
    pub file: Option<Cow<'a, str>>,
    pub line: Option<u32>,
}

pub struct MockTelemetryEventRegistry;

impl ArrowRegistryLookup for MockTelemetryEventRegistry {
    type ArrowAttributes<'a> = MockArrowAttributes<'a>;

    fn arrow_attributes_fields() -> Fields {
        Fields::from(vec![
            Field::new("name", DataType::LargeUtf8, true),
            Field::new("code", DataType::Int32, true),
            Field::new("file", DataType::LargeUtf8, true),
            Field::new("line", DataType::UInt32, true),
        ])
    }

    fn deserialize_arrow_attributes(
        &self,
        event_type: &str,
        attributes: &Self::ArrowAttributes<'_>,
    ) -> Result<Box<dyn AnyTelemetryEvent>, TelemetryAttributeDeserializeError> {
        match event_type {
            MockUnknown::EVENT_TYPE => MockUnknown::from_arrow_record(attributes)
                .map_err(TelemetryAttributeDeserializeError::malformed),
            MockDynLogEvent::EVENT_TYPE => MockDynLogEvent::from_arrow_record(attributes)
                .map_err(TelemetryAttributeDeserializeError::malformed),
            _ => Err(TelemetryAttributeDeserializeError::unknown_event_type(
                event_type,
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct MockUnknown {
    pub name: String,
    pub file: String,
    pub line: u32,
    #[serde(serialize_with = "serialize_flags")]
    pub flags: TelemetryOutputFlags,
}

impl MockUnknown {
    pub const EVENT_TYPE: &'static str = "v1.public.events.fusion.dev.MockUnknown";

    pub fn from_arrow_record(
        attrs: &MockArrowAttributes<'_>,
    ) -> Result<Box<dyn AnyTelemetryEvent>, String> {
        Ok(Box::new(Self {
            name: attrs
                .name
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| format!("Missing `name` for event type \"{}\"", Self::EVENT_TYPE))?,
            file: attrs
                .file
                .as_deref()
                .map(str::to_string)
                .unwrap_or_default(),
            line: attrs.line.unwrap_or_default(),
            flags: TelemetryOutputFlags::EXPORT_PARQUET,
        }))
    }
}

impl AnyTelemetryEvent for MockUnknown {
    fn event_type(&self) -> &'static str {
        Self::EVENT_TYPE
    }

    fn event_display_name(&self) -> String {
        format!(
            "Mock Unknown Span: {} ({}:{})",
            self.name, self.file, self.line
        )
    }

    fn record_category(&self) -> TelemetryEventRecType {
        TelemetryEventRecType::Span
    }

    fn output_flags(&self) -> TelemetryOutputFlags {
        self.flags
    }

    fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|rhs| rhs == self)
    }

    fn code_location(&self) -> Option<RecordCodeLocation> {
        Some(RecordCodeLocation {
            file: Some(self.file.clone()),
            line: Some(self.line),
            ..Default::default()
        })
    }

    fn with_code_location(&mut self, location: RecordCodeLocation) {
        if let Some(file) = location.file {
            self.file = file;
        }

        if let Some(line) = location.line {
            self.line = line;
        }
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        Some(Box::new(self.clone()))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self).map_err(|e| format!("Failed to serialize: {e}"))
    }

    fn to_arrow(&self) -> Option<Box<dyn ArrowAttributesSerialize + '_>> {
        self.flags
            .contains(TelemetryOutputFlags::EXPORT_PARQUET)
            .then(|| {
                Box::new(MockArrowAttributes {
                    name: Some(Cow::Borrowed(self.name.as_str())),
                    file: Some(Cow::Borrowed(self.file.as_str())),
                    line: Some(self.line),
                    ..Default::default()
                }) as Box<dyn ArrowAttributesSerialize + '_>
            })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct MockRootSpanEvent {
    pub name: String,
    #[serde(serialize_with = "serialize_flags")]
    pub flags: TelemetryOutputFlags,
    #[serde(skip)]
    pub trace_id: Option<u128>,
    #[serde(skip)]
    pub parent_span_id: Option<u64>,
}

impl AnyTelemetryEvent for MockRootSpanEvent {
    fn event_type(&self) -> &'static str {
        "v1.public.events.fusion.dev.MockRootSpanEvent"
    }

    fn event_display_name(&self) -> String {
        format!("Mock Root Span Event: {}", self.name)
    }

    fn record_category(&self) -> TelemetryEventRecType {
        TelemetryEventRecType::Span
    }

    fn output_flags(&self) -> TelemetryOutputFlags {
        self.flags
    }

    fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|rhs| rhs == self)
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        Some(Box::new(self.clone()))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self).map_err(|e| format!("Failed to serialize: {e}"))
    }
}

impl AnyTelemetryEvent for MockDynSpanEvent {
    fn event_type(&self) -> &'static str {
        "v1.public.events.fusion.dev.MockDynSpanEvent"
    }

    fn event_display_name(&self) -> String {
        format!("Mock Dyn Span Event: {}", self.name)
    }

    fn record_category(&self) -> TelemetryEventRecType {
        TelemetryEventRecType::Span
    }

    fn output_flags(&self) -> TelemetryOutputFlags {
        self.flags
    }

    fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|rhs| rhs == self)
    }

    fn context(&self) -> Option<TelemetryContext> {
        self.context.clone().map(TelemetryContext::new)
    }

    fn has_sensitive_data(&self) -> bool {
        self.has_sensitive
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        Some(Box::new(Self {
            name: self.name.clone(),
            flags: self.flags,
            has_sensitive: self.has_sensitive,
            was_scrubbed: true,
            context: self.context.clone(),
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self).map_err(|e| format!("Failed to serialize: {e}"))
    }
}

// Mock dynamic log event with instance-based export flags
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct MockDynLogEvent {
    pub code: i32,
    #[serde(serialize_with = "serialize_flags")]
    pub flags: TelemetryOutputFlags,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub has_sensitive: bool,
    pub was_scrubbed: bool,
    pub workflow_name: Option<String>,
    pub attempt: Option<u32>,
}

impl MockDynLogEvent {
    pub const EVENT_TYPE: &'static str = "v1.public.events.fusion.dev.MockDynLogEvent";

    pub fn from_arrow_record(
        attrs: &MockArrowAttributes<'_>,
    ) -> Result<Box<dyn AnyTelemetryEvent>, String> {
        Ok(Box::new(Self {
            code: attrs
                .code
                .ok_or_else(|| format!("Missing `code` for event type \"{}\"", Self::EVENT_TYPE))?,
            flags: TelemetryOutputFlags::EXPORT_PARQUET,
            file: attrs.file.as_deref().map(str::to_string),
            line: attrs.line,
            ..Default::default()
        }))
    }
}

impl AnyTelemetryEvent for MockDynLogEvent {
    fn event_type(&self) -> &'static str {
        Self::EVENT_TYPE
    }

    fn event_display_name(&self) -> String {
        format!("Mock Dyn Log Event: {}", self.code)
    }

    fn record_category(&self) -> TelemetryEventRecType {
        TelemetryEventRecType::Log
    }

    fn output_flags(&self) -> TelemetryOutputFlags {
        self.flags
    }

    fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|rhs| rhs == self)
    }

    fn code_location(&self) -> Option<RecordCodeLocation> {
        Some(RecordCodeLocation {
            file: self.file.clone(),
            line: self.line,
            ..Default::default()
        })
    }

    fn with_code_location(&mut self, location: RecordCodeLocation) {
        if self.file.is_none() {
            self.file = location.file;
        }

        if self.line.is_none() {
            self.line = location.line;
        }
    }

    fn with_context(&mut self, context: &TelemetryContext) {
        let Some(context) = context.downcast_ref::<TestTelemetryContext>() else {
            return;
        };

        if self.workflow_name.is_none() {
            self.workflow_name = Some(context.workflow_name.clone());
        }

        if self.attempt.is_none() {
            self.attempt = Some(context.attempt);
        }
    }

    fn has_sensitive_data(&self) -> bool {
        self.has_sensitive
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        Some(Box::new(Self {
            code: self.code,
            flags: self.flags,
            file: self.file.clone(),
            line: self.line,
            has_sensitive: self.has_sensitive,
            was_scrubbed: true,
            workflow_name: self.workflow_name.clone(),
            attempt: self.attempt,
        }))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self).map_err(|e| format!("Failed to serialize: {e}"))
    }

    fn to_arrow(&self) -> Option<Box<dyn ArrowAttributesSerialize + '_>> {
        self.flags
            .contains(TelemetryOutputFlags::EXPORT_PARQUET)
            .then(|| {
                Box::new(MockArrowAttributes {
                    code: Some(self.code),
                    file: self.file.as_deref().map(Cow::Borrowed),
                    line: self.line,
                    ..Default::default()
                }) as Box<dyn ArrowAttributesSerialize + '_>
            })
    }
}

#[derive(Clone, Default)]
pub struct TestWriter {
    is_terminal: bool,
    lines: Arc<Mutex<Vec<String>>>,
}

impl TestWriter {
    pub fn non_terminal() -> Self {
        Default::default()
    }

    pub fn terminal() -> Self {
        Self {
            is_terminal: true,
            lines: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn get_lines(&self) -> Vec<String> {
        self.lines.lock().expect("writer mutex poisoned").clone()
    }
}

impl SharedWriter for TestWriter {
    fn write(&self, data: &str) {
        self.lines
            .lock()
            .expect("writer mutex poisoned")
            .push(data.to_string());
    }

    fn writeln(&self, data: &str) {
        self.lines
            .lock()
            .expect("writer mutex poisoned")
            .push(format!("{data}\n"));
    }

    fn is_terminal(&self) -> bool {
        self.is_terminal
    }
}

type SpanStartConsumerHandler = dyn for<'a> Fn(&SpanStartInfo, &mut DataProvider<'a>) + Send + Sync;
type SpanEndConsumerHandler = dyn for<'a> Fn(&SpanEndInfo, &mut DataProvider<'a>) + Send + Sync;
type LogRecordConsumerHandler = dyn for<'a> Fn(&LogRecordInfo, &mut DataProvider<'a>) + Send + Sync;

// Shared capture layer used by multiple tests to collect structured telemetry
pub struct TestLayer {
    span_starts: Arc<Mutex<Vec<SpanStartInfo>>>,
    span_ends: Arc<Mutex<Vec<SpanEndInfo>>>,
    log_records: Arc<Mutex<Vec<LogRecordInfo>>>,
    on_span_start: Box<SpanStartConsumerHandler>,
    on_span_end: Box<SpanEndConsumerHandler>,
    on_log_record: Box<LogRecordConsumerHandler>,
}

impl TestLayer {
    #[allow(clippy::type_complexity)]
    pub fn new() -> (
        Self,
        Arc<Mutex<Vec<SpanStartInfo>>>,
        Arc<Mutex<Vec<SpanEndInfo>>>,
        Arc<Mutex<Vec<LogRecordInfo>>>,
    ) {
        let span_starts = Arc::new(Mutex::new(Vec::new()));
        let span_ends = Arc::new(Mutex::new(Vec::new()));
        let log_records = Arc::new(Mutex::new(Vec::new()));

        let layer = Self {
            span_starts: span_starts.clone(),
            span_ends: span_ends.clone(),
            log_records: log_records.clone(),
            on_span_start: Box::new(|_, _| {}),
            on_span_end: Box::new(|_, _| {}),
            on_log_record: Box::new(|_, _| {}),
        };

        (layer, span_starts, span_ends, log_records)
    }

    #[allow(dead_code)]
    pub fn with_span_start<F>(mut self, f: F) -> Self
    where
        F: for<'a> Fn(&SpanStartInfo, &mut DataProvider<'a>) + Send + Sync + 'static,
    {
        self.on_span_start = Box::new(f);
        self
    }

    #[allow(dead_code)]
    pub fn with_span_end<F>(mut self, f: F) -> Self
    where
        F: for<'a> Fn(&SpanEndInfo, &mut DataProvider<'a>) + Send + Sync + 'static,
    {
        self.on_span_end = Box::new(f);
        self
    }

    #[allow(dead_code)]
    pub fn with_log_record<F>(mut self, f: F) -> Self
    where
        F: for<'a> Fn(&LogRecordInfo, &mut DataProvider<'a>) + Send + Sync + 'static,
    {
        self.on_log_record = Box::new(f);
        self
    }
}

impl TelemetryConsumer for TestLayer {
    fn on_span_start(&self, span: &SpanStartInfo, data_provider: &mut DataProvider<'_>) {
        (self.on_span_start)(span, data_provider);
        self.span_starts.lock().unwrap().push(span.clone());
    }

    fn on_span_end(&self, span: &SpanEndInfo, data_provider: &mut DataProvider<'_>) {
        (self.on_span_end)(span, data_provider);
        self.span_ends.lock().unwrap().push(span.clone());
    }

    fn on_log_record(&self, record: &LogRecordInfo, data_provider: &mut DataProvider<'_>) {
        (self.on_log_record)(record, data_provider);
        self.log_records.lock().unwrap().push(record.clone());
    }
}

type SpanStartMiddlewareHandler =
    dyn for<'a> Fn(SpanStartInfo, &mut DataProvider<'a>) -> Option<SpanStartInfo> + Send + Sync;
type SpanEndMiddlewareHandler =
    dyn for<'a> Fn(SpanEndInfo, &mut DataProvider<'a>) -> Option<SpanEndInfo> + Send + Sync;
type LogRecordMiddlewareHandler =
    dyn for<'a> Fn(LogRecordInfo, &mut DataProvider<'a>) -> Option<LogRecordInfo> + Send + Sync;

/// A configurable middleware used to test how telemetry data passes through middleware hooks.
pub struct MockMiddleware {
    span_start: Box<SpanStartMiddlewareHandler>,
    span_end: Box<SpanEndMiddlewareHandler>,
    log_record: Box<LogRecordMiddlewareHandler>,
}

impl Default for MockMiddleware {
    fn default() -> Self {
        Self {
            span_start: Box::new(|span, _| Some(span)),
            span_end: Box::new(|span, _| Some(span)),
            log_record: Box::new(|record, _| Some(record)),
        }
    }
}

impl MockMiddleware {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_span_start<F>(mut self, f: F) -> Self
    where
        F: for<'a> Fn(SpanStartInfo, &mut DataProvider<'a>) -> Option<SpanStartInfo>
            + Send
            + Sync
            + 'static,
    {
        self.span_start = Box::new(f);
        self
    }

    #[allow(dead_code)]
    pub fn with_span_end<F>(mut self, f: F) -> Self
    where
        F: for<'a> Fn(SpanEndInfo, &mut DataProvider<'a>) -> Option<SpanEndInfo>
            + Send
            + Sync
            + 'static,
    {
        self.span_end = Box::new(f);
        self
    }

    pub fn with_log_record<F>(mut self, f: F) -> Self
    where
        F: for<'a> Fn(LogRecordInfo, &mut DataProvider<'a>) -> Option<LogRecordInfo>
            + Send
            + Sync
            + 'static,
    {
        self.log_record = Box::new(f);
        self
    }
}

impl TelemetryMiddleware for MockMiddleware {
    fn on_span_start(
        &self,
        span: SpanStartInfo,
        data_provider: &mut DataProvider<'_>,
    ) -> Option<SpanStartInfo> {
        (self.span_start)(span, data_provider)
    }

    fn on_span_end(
        &self,
        span: SpanEndInfo,
        data_provider: &mut DataProvider<'_>,
    ) -> Option<SpanEndInfo> {
        (self.span_end)(span, data_provider)
    }

    fn on_log_record(
        &self,
        record: LogRecordInfo,
        data_provider: &mut DataProvider<'_>,
    ) -> Option<LogRecordInfo> {
        (self.log_record)(record, data_provider)
    }
}
