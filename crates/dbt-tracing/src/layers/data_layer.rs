use crate::{
    DebugValue, LogRecordInfo, RecordCodeLocation, SeverityNumber, SpanEndInfo, SpanLinkInfo,
    SpanStartInfo, SpanStatus, TelemetryAttributes, TelemetryContext, TelemetryEventRecType,
    constants::{CLOSE_SPAN_FIELD, PROCESS_SPAN_NAME, ROOT_SPAN_NAME},
    data_provider::DataProvider,
    emit::get_file_and_line,
    event_info::{get_log_message, take_event_attributes},
    init::process_span,
    layer::{ConsumerLayer, MiddlewareLayer},
    metrics::init_metrics_storage_on_root_span,
    shared::Recordable,
    span_info::get_span_debug_extra_attrs,
};
use rand::RngCore;

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::SystemTime,
};

use tracing::{
    Level, Subscriber,
    field::{Field, Visit},
    span,
};
use tracing_subscriber::{
    Layer,
    layer::Context,
    registry::{LookupSpan, SpanRef},
};

/// Builds span attributes for spans that were not created through the structured emit/span helpers.
///
/// The data layer passes the tracing metadata it has available here, and the application
/// provides the fallback attributes to use for unstructured spans.
pub type UnstructuredSpanAttributesCallback =
    for<'a> fn(UnstructuredSpanAttributesInput<'a>) -> TelemetryAttributes;

/// Builds log attributes for events that were not created through the structured emit helpers.
pub type UnstructuredLogAttributesCallback =
    fn(UnstructuredLogAttributesInput) -> TelemetryAttributes;

/// Extracts trace correlation data from attributes when a root span defines its own trace context.
pub type RootSpanTraceContextCallback = fn(&TelemetryAttributes) -> Option<RootSpanTraceContext>;

/// Metadata available when constructing fallback span attributes.
pub struct UnstructuredSpanAttributesInput<'a> {
    /// The tracing metadata name for the unstructured span.
    pub name: &'a str,
    /// The tracing severity level for the unstructured span.
    pub level: &'a Level,
    /// Code location calculated from tracing metadata or explicit span fields.
    pub location: RecordCodeLocation,
    /// Extra trace-level span fields, when the caller wants a dev-internal fallback.
    pub debug_extra_attrs: Option<BTreeMap<String, DebugValue>>,
}

/// Metadata available when constructing fallback log attributes.
pub struct UnstructuredLogAttributesInput {
    /// The normalized telemetry severity for the log event.
    pub severity_number: SeverityNumber,
    /// Code location calculated from tracing metadata or explicit event fields.
    pub location: RecordCodeLocation,
}

/// Trace correlation data supplied by structured root span attributes.
pub struct RootSpanTraceContext {
    /// The trace ID that should be used for this root span and its children.
    pub trace_id: u128,
    /// Optional parent span ID for trace correlation.
    pub parent_span_id: Option<u64>,
}

/// Fallback IDs and callbacks used when tracing calls do not carry structured telemetry attributes.
#[derive(Clone, Copy)]
pub struct TelemetryDataLayerConfig {
    /// The trace ID used for spans & events lacking a proper parent span
    /// (essentially the root span and any tracing calls missing the expected
    /// root operation span tree in their context).
    fallback_trace_id: u128,
    /// Optional parent span ID for trace correlation, used as fallback when creating
    /// root spans without an explicit parent.
    fallback_parent_span_id: Option<u64>,
    /// Callback used for fallback span attributes.
    unstructured_span_attributes: UnstructuredSpanAttributesCallback,
    /// Callback used for fallback log attributes.
    unstructured_log_attributes: UnstructuredLogAttributesCallback,
    /// Callback used to detect root spans with structured trace context.
    root_span_trace_context: RootSpanTraceContextCallback,
}

impl TelemetryDataLayerConfig {
    /// Creates a new data layer configuration for fallback telemetry behavior.
    pub fn new(
        fallback_trace_id: u128,
        fallback_parent_span_id: Option<u64>,
        unstructured_span_attributes: UnstructuredSpanAttributesCallback,
        unstructured_log_attributes: UnstructuredLogAttributesCallback,
        root_span_trace_context: RootSpanTraceContextCallback,
    ) -> Self {
        Self {
            fallback_trace_id,
            fallback_parent_span_id,
            unstructured_span_attributes,
            unstructured_log_attributes,
            root_span_trace_context,
        }
    }
}

/// A bitmask used to represent which consumers are not interested in a given
/// telemetry span. Consumers are indexed by their position in the consumer list
/// of a TelemetryDataLayer.
///
/// Keep private to this module to avoid potential modification by consumers
/// or middleware layers via data provider access.
#[derive(Debug, Default, Clone, Copy)]
struct FilterMask(u64);

impl FilterMask {
    /// Returns an empty filter mask (meaning no consumers are disabled)
    pub(super) fn empty() -> Self {
        Self::default()
    }

    /// Returns a full filter mask (meaning all consumers are disabled)
    pub(super) fn disabled() -> Self {
        Self(u64::MAX)
    }

    /// Returns true if this is a filter mask that disables all consumers
    pub(super) fn is_disabled(&self) -> bool {
        self.0 == u64::MAX
    }

    pub(super) fn set_filtered(&mut self, index: usize) {
        // Shift would panic in debug if index >= 64, but we add a nice message via debug_assert
        debug_assert!(
            index < 64,
            "Exceeding mask length. Index must be less than 64"
        );
        self.0 |= 1 << index;
    }

    pub(super) fn is_filtered(&self, index: usize) -> bool {
        // Shift would panic in debug if index >= 64, but we add a nice message via debug_assert
        debug_assert!(
            index < 64,
            "Exceeding mask length. Index must be less than 64"
        );
        self.0 & (1 << index) != 0
    }
}

// Private newtypes to protect internal data layer state from being accessed
// by middleware or consumer layers. These types are stored in span extensions
// and are only accessible by the data layer itself.

/// Read-only wrapper for SpanStartInfo stored in span extensions.
///
/// This struct provides immutable access to SpanStartInfo while preventing
/// accidental modification by middleware or consumer layers. The wrapper can
/// only be constructed within the data layer, but can be accessed immutably
/// from anywhere within the tracing module to enable read-only APIs.
///
/// # Safety guarantees
///
/// - Construction is restricted to the data layer (private field)
/// - Deref provides only immutable references to SpanStartInfo
/// - Cannot be mutated via span extensions (extensions_mut requires
///   constructing a new value to replace, and our Deref prevents mutable access)
pub(crate) struct DLSpanStartInfo(SpanStartInfo);

impl DLSpanStartInfo {
    /// Creates a new wrapper for SpanStartInfo.
    /// Only accessible within the data layer module.
    fn new(info: SpanStartInfo) -> Self {
        Self(info)
    }
}

impl std::ops::Deref for DLSpanStartInfo {
    type Target = SpanStartInfo;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Private wrapper for TelemetryContext stored in span extensions.
struct DLTelemetryContext(TelemetryContext);

/// Close state of a span that can be force closed, shared with all of its descendants.
struct CloseScope {
    /// Set once the owning span is force closed. Never reset.
    closed: AtomicBool,
    /// Scope of the closest closable ancestor, if any.
    parent: Option<Arc<CloseScope>>,
}

impl CloseScope {
    /// Returns true if the owning span or any closable ancestor was force closed.
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) || self.parent.as_ref().is_some_and(|p| p.is_closed())
    }
}

/// Private wrapper for the close scope stored in span extensions. A closable span
/// owns its scope, every other span holds the scope of its closest closable ancestor.
struct DLCloseScope(Arc<CloseScope>);

/// Detects a force close request recorded via [`crate::emit::force_close_span`].
/// Any value recorded on the marker field is a request.
#[derive(Default)]
struct CloseRequestVisitor(bool);

impl Visit for CloseRequestVisitor {
    fn record_debug(&mut self, field: &Field, _value: &dyn std::fmt::Debug) {
        if field.name() == CLOSE_SPAN_FIELD {
            self.0 = true;
        }
    }
}

/// A tracing layer that creates structured telemetry data and stores it in span extensions.
///
/// This layer captures span events and converts them to structured telemetry
/// records that include the trace ID for correlation across systems.
pub struct TelemetryDataLayer<S>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    /// Fallback IDs and callback configuration for telemetry data construction.
    config: TelemetryDataLayerConfig,
    /// Whether to strip code location from span & log attributes.
    strip_code_location: bool,
    /// The telemetry middlewares to apply before notifying consumers.
    middlewares: Vec<MiddlewareLayer>,
    /// The telemetry consumers to notify of span & event events.
    consumers: Vec<ConsumerLayer>,
    /// If set, uses sequential span & event IDs for easier testing and debugging.
    /// Normally this is None and we use a thread-local RNG to generate
    /// unique span IDs, uuid::Uuid::new_v7() for event IDs.
    next_id: Option<AtomicU64>,
    /// Phantom data to associate with the subscriber type.
    __phantom: std::marker::PhantomData<S>,
}

impl<S> TelemetryDataLayer<S>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    pub fn new(
        config: TelemetryDataLayerConfig,
        strip_code_location: bool,
        middlewares: impl Iterator<Item = MiddlewareLayer>,
        consumers: impl Iterator<Item = ConsumerLayer>,
    ) -> Self {
        Self {
            config,
            strip_code_location,
            middlewares: middlewares.collect(),
            consumers: consumers.collect(),
            next_id: None,
            __phantom: std::marker::PhantomData,
        }
    }

    /// Returns a copy of the data layer fallback configuration.
    pub fn config(&self) -> TelemetryDataLayerConfig {
        self.config
    }

    /// For testing and debugging purposes, enables sequential span IDs
    pub fn with_sequential_ids(&mut self) {
        self.next_id = Some(AtomicU64::new(1));
    }

    /// Returns a globally unique span ID for the next span. We can't use the span ID from
    /// `tracing` directly because it is not guaranteed to be unique across even within a single
    /// process, especially in a multi-threaded environment.
    ///
    /// This uses a thread-local random number generator which is thread-safe by design.
    /// The probability of collision for a 64-bit random number is negligible in practice.
    fn next_span_id(&self) -> u64 {
        self.next_id
            .as_ref()
            .map(|next_span_id| next_span_id.fetch_add(1, Ordering::AcqRel))
            .unwrap_or_else(|| rand::rng().next_u64())
    }

    /// Returns a globally unique event ID for the next event.
    fn next_event_id(&self) -> uuid::Uuid {
        self.next_id
            .as_ref()
            .map(|next_event_id| {
                let id = next_event_id.fetch_add(1, Ordering::AcqRel);
                // Convert the 64-bit ID to a UUID by padding with zeros
                let mut bytes = [0u8; 16];
                bytes[8..].copy_from_slice(&id.to_be_bytes());
                uuid::Uuid::from_bytes(bytes)
            })
            .unwrap_or_else(uuid::Uuid::now_v7)
    }

    fn get_location(
        &self,
        metadata: &tracing::Metadata<'_>,
        values: Option<Recordable<'_>>,
    ) -> RecordCodeLocation {
        if self.strip_code_location {
            RecordCodeLocation::default()
        } else {
            // Try extracting using our custom location
            if let Some((file, line)) = values.and_then(|values| get_file_and_line(values)) {
                return RecordCodeLocation {
                    file: Some(file),
                    line: Some(line),
                    ..Default::default()
                };
            }
            // Extract code location from metadata
            RecordCodeLocation::from(metadata)
        }
    }

    /// Delivers the span end record. `force_closed` is set when called for a force close
    /// rather than for the native close of the span.
    fn end_span(&self, span: &SpanRef<'_, S>, force_closed: bool) {
        let metadata = span.metadata();

        // Extract the span end info from span extensions in block to limit borrow scope
        let (mut record, span_filter_mask) = {
            // Acquire a read-only reference to span extensions
            let span_ext = span.extensions();

            // Nothing is delivered in a force closed subtree, except for the end of the force
            // closed span itself, which is delivered by the force close. Its native close is a no-op.
            if let Some(DLCloseScope(scope)) = span_ext.get::<DLCloseScope>() {
                let suppressed = if force_closed {
                    scope.parent.as_ref().is_some_and(|p| p.is_closed())
                } else {
                    scope.is_closed()
                };
                if suppressed {
                    return;
                }
            }

            // Get the shared info from the stored SpanStart record
            let (
                trace_id,
                span_id,
                parent_span_id,
                links,
                start_time_unix_nano,
                severity_number,
                severity_text,
                start_attributes,
            ) = if let Some(DLSpanStartInfo(SpanStartInfo {
                trace_id,
                span_id,
                parent_span_id,
                links,
                start_time_unix_nano,
                severity_number,
                severity_text,
                attributes,
                ..
            })) = span_ext.get::<DLSpanStartInfo>()
            {
                (
                    *trace_id,
                    *span_id,
                    *parent_span_id,
                    links.clone(),
                    *start_time_unix_nano,
                    *severity_number,
                    severity_text.clone(),
                    attributes.clone(),
                )
            } else {
                let severity_number: SeverityNumber = metadata.level().into();
                let location = self.get_location(metadata, None);

                (
                    self.config.fallback_trace_id,
                    self.next_span_id(),
                    None,
                    None, // No links in fallback case
                    SystemTime::now(),
                    severity_number,
                    severity_number.as_str().to_string(),
                    // Fallback. Should not happen: spans should have a start record by close time.
                    (self.config.unstructured_span_attributes)(UnstructuredSpanAttributesInput {
                        name: metadata.name(),
                        level: metadata.level(),
                        location,
                        debug_extra_attrs: None,
                    }),
                ) // Fallback. Should not happen
            };

            // Pull the current span context (if any) to inject into closing attributes
            let current_ctx = span_ext.get::<DLTelemetryContext>().map(|c| c.0.clone());

            let mut attributes = span_ext.get::<TelemetryAttributes>().cloned().unwrap_or({
                // If no attributes were recorded, use the start attributes
                start_attributes
            });

            // Pull record status from span extensions (if any) or try to infer from attributes
            let status = span_ext
                .get::<SpanStatus>()
                .cloned()
                .or_else(|| attributes.get_span_status());

            // check that attributes are of expected record type
            debug_assert_eq!(attributes.record_category(), TelemetryEventRecType::Span);

            // Apply current span context (if any) before finalizing
            if let Some(ctx) = &current_ctx {
                attributes.inner_mut().with_context(ctx);
            }

            // For each span we have a mask of which consumers have filtered out this span.
            // It is determined at span creation time and stored in span extensions.
            let span_filter_mask = span_ext
                .get::<FilterMask>()
                .copied()
                .unwrap_or_else(FilterMask::empty);

            let record = SpanEndInfo {
                trace_id,
                span_id,
                span_name: attributes.event_display_name(),
                parent_span_id,
                links,
                start_time_unix_nano,
                end_time_unix_nano: SystemTime::now(),
                severity_number,
                severity_text,
                status,
                attributes,
            };

            (record, span_filter_mask)
        };

        if span_filter_mask.is_disabled() {
            // Span was filtered out at creation by middleware or all consumers
            // are not interested in this span, so nothing to do here
            return;
        }

        let root_span = span
            .scope()
            .from_root()
            .next()
            .expect("Root span must exist");

        // If tracing is set up correctly, the only time when root span doesn't have our special root
        // span name marker is when it is the process span created during tracing initialization.
        debug_assert!(
            root_span.name() == ROOT_SPAN_NAME
                || span.name() == PROCESS_SPAN_NAME
                || cfg!(any(test, feature = "test-utils")),
            "Expected root span created via `create_root_info_span`. Got: {}.
            Are you running code not instrumented under a root operation span tree?",
            root_span.name()
        );

        let mut data_provider = DataProvider::new(&root_span, span);

        if !self.middlewares.is_empty() {
            for middleware in &self.middlewares {
                match middleware.on_span_end(record, &mut data_provider) {
                    Some(next_record) => {
                        record = next_record;
                    }
                    None => {
                        // Span was filtered out by middleware on close, early return
                        return;
                    }
                }
            }
        }

        // Notify consumers if the span was not filtered out by middleware
        let curr_parent = span.parent();
        let parent_span_filter_mask = curr_parent
            .as_ref()
            .and_then(|parent_span| parent_span.extensions().get::<FilterMask>().copied())
            .unwrap_or_else(FilterMask::empty);

        for (index, consumer) in self.consumers.iter().enumerate() {
            debug_assert!(
                index < 64,
                "Consumer index must be less than 64. Invariant is preserved by construction."
            );

            // Check if span is enabled for this consumer
            if span_filter_mask.is_filtered(index) {
                continue;
            }

            let Some(curr_parent) = curr_parent.as_ref() else {
                // No parent span, so no filtering to do. Call consumer and continue
                consumer.on_span_end(&record, &mut data_provider);
                continue;
            };

            // Check parent span is enabled for this consumer. This is the fast path
            // for the common case where parent span is not filtered out and we
            // can pass the record as is
            if !parent_span_filter_mask.is_filtered(index) {
                // Parent span is not filtered out, we can pass the record as is
                // No need to search for unfiltered parent
                consumer.on_span_end(&record, &mut data_provider);
                continue;
            }

            // Slow path: parent span was filtered out for this consumer.
            // Find the closest unfiltered parent span ID for this consumer (if any)
            // and then create a new record with the updated parent span ID
            let active_parent_span_id = lookup_filtered_parent_span_id(index, curr_parent);

            // Now create a new record with the updated parent span ID
            let modified_record = SpanEndInfo {
                parent_span_id: active_parent_span_id,
                ..record.clone()
            };

            consumer.on_span_end(&modified_record, &mut data_provider);
        }
    }
}

impl<S> Layer<S> for TelemetryDataLayer<S>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let span = ctx
            .span(id)
            .expect("Span must exist for id in the current context");
        let metadata = span.metadata();

        let global_span_id = self.next_span_id();

        // Start by extracting event attributes if any. To avoid leakage, we extract internal metadata
        // such as location, name etc. only in debug builds

        // Calculate code location once
        let location = self.get_location(metadata, Some(attrs.values().into()));

        // Extract attributes in the following priority:
        // - Pre-populated attributes (the "normal" way for all non-trace level spans)
        // - Fallback to default attributes based on metadata (shouldn't happen for properly instrumented spans)
        let mut attributes = if let Some(mut attrs) = take_event_attributes() {
            attrs.inner_mut().with_code_location(location);
            attrs
        } else {
            // Fallback to configured default attributes based on metadata
            // (shouldn't happen for properly instrumented spans).
            (self.config.unstructured_span_attributes)(UnstructuredSpanAttributesInput {
                name: metadata.name(),
                level: metadata.level(),
                location,
                debug_extra_attrs: (metadata.level() == &Level::TRACE)
                    .then(|| get_span_debug_extra_attrs(attrs.values().into())),
            })
        };

        // check that attributes are of expected record type
        debug_assert_eq!(attributes.record_category(), TelemetryEventRecType::Span);

        // Pull the trace ID, parent span ID & parent ctx from the parent span (if any)
        let (
            trace_id,
            global_parent_span_id,
            parent_ctx,
            parent_span_filter_mask,
            parent_close_scope,
        ) = span
            .parent()
            .and_then(|parent_span| {
                let parent_span_ext = parent_span.extensions();
                let parent_span_filter_mask = parent_span_ext
                    .get::<FilterMask>()
                    .cloned()
                    .unwrap_or_else(FilterMask::empty);

                let parent_ctx = parent_span_ext.get::<DLTelemetryContext>();
                let parent_close_scope = parent_span_ext
                    .get::<DLCloseScope>()
                    .map(|s| Arc::clone(&s.0));
                parent_span_ext
                    .get::<DLSpanStartInfo>()
                    .map(|parent_span_record| {
                        (
                            parent_span_record.0.trace_id,
                            Some(parent_span_record.0.span_id),
                            parent_ctx.map(|pcx| pcx.0.clone()),
                            parent_span_filter_mask,
                            parent_close_scope,
                        )
                    })
            })
            .unwrap_or_else(|| {
                // If no parent span is found, we have a couple possible scenarios:
                // 1. This is the root span of the trace, in which case we use the fallback trace ID,
                //    and optionally the fallback parent span ID if one was configured
                // 2. This is a configured root telemetry span. The application callback recognizes
                //    structured root span attributes and supplies the trace context to use
                // 3. This is a tracing call missing the expected root operation span tree in its
                //    context, in which case we fall back to the configured trace and parent span IDs
                if let Some(root_span_context) = (self.config.root_span_trace_context)(&attributes)
                {
                    (
                        root_span_context.trace_id,
                        root_span_context.parent_span_id,
                        None,
                        FilterMask::empty(),
                        None,
                    )
                } else {
                    (
                        self.config.fallback_trace_id,
                        self.config.fallback_parent_span_id,
                        None,
                        FilterMask::empty(),
                        None,
                    )
                }
            });

        // A span under a force closed ancestor is kept for native tracing, but never
        // reaches middlewares or consumers.
        let parent_closed = parent_close_scope.as_ref().is_some_and(|s| s.is_closed());

        // A span that declares the close field owns a new close scope, others inherit the parent's
        let close_scope = if metadata.fields().field(CLOSE_SPAN_FIELD).is_some() {
            Some(DLCloseScope(Arc::new(CloseScope {
                closed: AtomicBool::new(false),
                parent: parent_close_scope,
            })))
        } else {
            parent_close_scope.map(DLCloseScope)
        };

        // First inject parent span context into the new span attributes (if any)
        if let Some(ctx) = &parent_ctx {
            attributes.inner_mut().with_context(ctx);
        }

        // Determine the context for this span: either this span provides it, or we inherit from parent
        let this_ctx = attributes.inner().context().or(parent_ctx);

        let start_time = SystemTime::now();
        let severity_number = metadata.level().into();

        let mut record = SpanStartInfo {
            trace_id,
            span_id: global_span_id,
            span_name: attributes.event_display_name(),
            parent_span_id: global_parent_span_id,
            links: None, // Links are always empty at creation time
            start_time_unix_nano: start_time,
            severity_number,
            severity_text: severity_number.as_str().to_string(),
            attributes: attributes.clone(),
        };

        // If this is the root span, initialize root-level metrics storage.
        // We have to do it early to ensure it is available to middlewares
        if span.parent().is_none() {
            init_metrics_storage_on_root_span(&span);
        }

        // Get root_span for data provider
        let root_span = span
            .scope()
            .from_root()
            .next()
            .expect("Root span must exist");

        // If tracing is set up correctly, the only time when root span doesn't have our special root
        // span name marker is when it is the process span created during tracing initialization.
        debug_assert!(
            root_span.name() == ROOT_SPAN_NAME
                || span.name() == PROCESS_SPAN_NAME
                || cfg!(any(test, feature = "test-utils")),
            "Expected root span created via `create_root_info_span`. Got: {}.
            Are you running code not instrumented under a root operation span tree?",
            root_span.name()
        );

        // For each span we save which consumers have filtered out this span
        let mut span_filter_mask = if parent_closed {
            FilterMask::disabled()
        } else {
            FilterMask::empty()
        };

        if !span_filter_mask.is_disabled() && !self.middlewares.is_empty() {
            // In case middleware filters out the span, we need to rebuild
            // the original record to store in span extensions. By storing
            // something even for filtered out spans, we maintain invariants
            // for user code API's used to modify span attributes post-creation
            let rebuild_span_start_record = || SpanStartInfo {
                trace_id,
                span_id: global_span_id,
                span_name: attributes.event_display_name(),
                parent_span_id: global_parent_span_id,
                links: None, // Links are always empty at creation time
                start_time_unix_nano: start_time,
                severity_number,
                severity_text: severity_number.as_str().to_string(),
                attributes: attributes.clone(),
            };

            // This block scope ensures that we don't hold mutable extensions beyond middleware calls.
            // This is important because current span may be the root span itself, and we later take
            // another mutable reference to store the data there.
            let mut data_provider = DataProvider::new(&root_span, &span);

            for middleware in &self.middlewares {
                // Extract links before moving record into middleware
                match middleware.on_span_start(record, &mut data_provider) {
                    Some(next_record) => {
                        record = next_record;
                    }
                    None => {
                        span_filter_mask = FilterMask::disabled();
                        record = rebuild_span_start_record();
                        break;
                    }
                }
            }

            // Update attributes with the latest from the record in case middleware modified them.
            // But only if the span was not filtered out by middleware.
            if !span_filter_mask.is_disabled() {
                attributes = record.attributes.clone();
            }
        }

        // Notify consumers if the span was not filtered out by middleware
        // This block also creates scope to limit read-only borrow of span extensions
        // as we need mutable borrow later
        if !span_filter_mask.is_disabled() {
            let mut data_provider = DataProvider::new(&root_span, &span);

            for (index, consumer) in self.consumers.iter().enumerate() {
                debug_assert!(
                    index < 64,
                    "Consumer index must be less than 64. Invariant is preserved by construction."
                );

                // Check if span is enabled for this consumer
                if !consumer.is_span_enabled(&record) {
                    // Mark this consumer as filtered out for this span
                    span_filter_mask.set_filtered(index);
                    continue;
                }

                // Check parent span is enabled for this consumer. This is the fast path
                // for the common case where parent span is not filtered out and we
                // can pass the record as is
                if !parent_span_filter_mask.is_filtered(index) {
                    // Parent span is not filtered out, we can pass the record as is
                    // No need to search for unfiltered parent
                    consumer.on_span_start(&record, &mut data_provider);
                    continue;
                }

                // Slow path: parent span was filtered out for this consumer.
                // Find the closest unfiltered parent span ID for this consumer (if any)
                // and then create a new record with the updated parent span ID
                let active_parent_span_id = lookup_filtered_parent_span_id(
                    index,
                    &span.parent().expect(
                        "Parent span must exist or otherwise we would have taken the other branch",
                    ),
                );

                // Now create a new record with the updated parent span ID
                let modified_record = SpanStartInfo {
                    parent_span_id: active_parent_span_id,
                    ..record.clone()
                };

                consumer.on_span_start(&modified_record, &mut data_provider);
            }
        }

        // Get a mutable reference to span extensions to store our data
        let mut ext_mut = span.extensions_mut();

        // First, private data that should only be accessible to data layer itself.
        // We use private newtype wrappers to avoid accidental modification by middleware
        // or consumer layers via data provider access.
        // Store the filter mask for this span
        ext_mut.insert(span_filter_mask);

        // Store the close scope this span belongs to (if any)
        if let Some(close_scope) = close_scope {
            ext_mut.insert(close_scope);
        }

        // Store an immutable start record in span extensions. Used later to build the SpanEnd record
        ext_mut.insert(DLSpanStartInfo::new(record));

        // Store computed context for this span (if any)
        if let Some(ctx) = this_ctx {
            ext_mut.insert(DLTelemetryContext(ctx));
        }

        // Finally store "mutable", user modifiable attributes.
        // This allows both the app code as well as middleware to
        // modify span attributes post-creation before they are finalized at span end.
        ext_mut.insert(attributes);
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, ctx: Context<'_, S>) {
        let mut close_request = CloseRequestVisitor::default();
        values.record(&mut close_request);
        if !close_request.0 {
            return;
        }

        let Some(span) = ctx.span(id) else {
            return;
        };

        // Only spans that declare the close field receive a close request, and each of them owns
        // its close scope. Close only once. The caller holds a span handle, so the native close
        // can't run concurrently with this one.
        let first_close = span
            .extensions()
            .get::<DLCloseScope>()
            .is_some_and(|s| !s.0.closed.swap(true, Ordering::AcqRel));
        if !first_close {
            return;
        }

        // Deliver the span end now. The eventual native close will be a no-op
        self.end_span(&span, true);
    }

    fn on_follows_from(&self, span: &span::Id, follows: &span::Id, ctx: Context<'_, S>) {
        // Get the span that declares it follows another span
        let Some(current_span) = ctx.span(span) else {
            return;
        };

        // Get the span that is being followed
        let Some(followed_span) = ctx.span(follows) else {
            // The followed span might not be tracked (e.g., it was filtered out or never created)
            return;
        };

        // Extract trace_id and span_id from the followed span
        let (followed_trace_id, followed_span_id) = {
            let followed_ext = followed_span.extensions();
            let Some(DLSpanStartInfo(followed_start_info)) = followed_ext.get::<DLSpanStartInfo>()
            else {
                // The followed span doesn't have start info (shouldn't happen in normal operation)
                return;
            };
            (followed_start_info.trace_id, followed_start_info.span_id)
        };

        // Create a SpanLinkInfo for this follows_from relationship
        let link = SpanLinkInfo {
            trace_id: followed_trace_id,
            span_id: followed_span_id,
            attributes: BTreeMap::new(),
        };

        // Update the current span's DLSpanStartInfo to include this link
        let mut current_ext = current_span.extensions_mut();
        if let Some(DLSpanStartInfo(start_info)) = current_ext.get_mut::<DLSpanStartInfo>() {
            // Initialize links vector if it doesn't exist, then append the new link
            match &mut start_info.links {
                Some(links) => links.push(link),
                None => start_info.links = Some(vec![link]),
            }
        }
        // If there's no DLSpanStartInfo, the span wasn't properly initialized, so we skip
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let span = ctx
            .span(&id)
            .expect("Span must exist for id in the current context");
        self.end_span(&span, false);
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        // Extract information about the current span
        let (
            trace_id,
            span_id,
            span_name,
            parent_ctx,
            parent_span_filter_mask,
            parent_closed,
            parent_span,
        ) = ctx
            .event_span(event)
            .or_else(|| process_span(&ctx))
            // Get the parent span to extract span information
            .and_then(|parent_span| {
                let ctx_data = {
                    let parent_span_ext = parent_span.extensions();
                    let parent_span_filter_mask = parent_span_ext
                        .get::<FilterMask>()
                        .cloned()
                        .unwrap_or_else(FilterMask::empty);
                    let parent_closed = parent_span_ext
                        .get::<DLCloseScope>()
                        .is_some_and(|s| s.0.is_closed());

                    let parent_ctx = parent_span_ext.get::<DLTelemetryContext>();
                    parent_span_ext
                        .get::<DLSpanStartInfo>()
                        .map(|parent_span_start_info| {
                            (
                                parent_span_start_info.0.trace_id,
                                Some(parent_span_start_info.0.span_id),
                                Some(parent_span_start_info.0.span_name.clone()),
                                parent_ctx.map(|pcx| pcx.0.clone()),
                                parent_span_filter_mask,
                                parent_closed,
                            )
                        })
                };

                ctx_data.map(|cd| (cd.0, cd.1, cd.2, cd.3, cd.4, cd.5, Some(parent_span)))
            })
            .unwrap_or_else(||
                // If no parent is found this is definitely a buggy tracing call (before our init & outside of any span)
                (self.config.fallback_trace_id, None, None, None, FilterMask::empty(), false, None));

        // Nothing is delivered under a force closed span. Still consume the attributes stored
        // for this event, so they can't leak into the next event emitted on this thread.
        if parent_closed {
            take_event_attributes();
            return;
        }

        // Get event metadata
        let metadata = event.metadata();

        // Calculate code location
        let location = self.get_location(metadata, Some(event.into()));

        // TODO: calculate modified severity based on user config when such feature is implemented
        let severity_number: SeverityNumber = metadata.level().into();

        // Extract message from event
        let message = get_log_message(event);

        // Extract attributes in the following priority:
        // - Pre-populated attributes (the "normal" way)
        // - Attributes from the event itself (if any, otherwise use default log attributes)
        let mut attributes = if let Some(mut attrs) = take_event_attributes() {
            attrs.inner_mut().with_code_location(location);
            attrs
        } else {
            // Fallback to configured default log attributes.
            (self.config.unstructured_log_attributes)(UnstructuredLogAttributesInput {
                severity_number,
                location,
            })
        };

        // check that attributes are of expected record type
        debug_assert_eq!(attributes.record_category(), TelemetryEventRecType::Log);

        // Inject parent span context into the log attributes (if any)
        if let Some(ctx_val) = parent_ctx {
            attributes.inner_mut().with_context(&ctx_val);
        }

        let time_unix_nano = SystemTime::now();

        let mut log_record = LogRecordInfo {
            time_unix_nano,
            trace_id,
            span_id,
            span_name,
            event_id: self.next_event_id(),
            severity_number,
            severity_text: severity_number.as_str().to_string(),
            body: message,
            attributes,
        };

        let root_span = parent_span
            .as_ref()
            .map(|ps| ps.scope().from_root().next().expect("Root span must exist"));

        // Unlike spans, where we expect that they are set-up corerctly and rooted under our special root span,
        // there are multiple valid use-cases for events being logged outside of any span context:
        // 1. Third-party libraries may log events on worker threads
        // 2. Worker threads that emit traces not tied to the main root operation
        // 3. Apps using this infrastructure that do not have a concept of root operations
        // 4. Errors during initialization before the root operation span is created
        //
        // In order to accommodate these use-cases, we allow events as long as at least process
        // span is present as a fallback. The lack of process span would indicate that a worker
        // thread is logging events after tracing has been shutdown, which is likely a bug and
        // will cause unexpected behavior in consumers.
        //
        // The main downside of a lax check here, is that middlewares that rely on data_provider
        // scoped to root operation context will instead get a process-level data provider, which will
        // lead to incorrect behavior. E.g. error metric counts and related exit code calculation will
        // ignore such events. These scenarios should be covered by tests to avoid regressions.
        debug_assert!(
            root_span.is_some() || cfg!(any(test, feature = "test-utils")),
            "Event logged outside of span context. Expected root span created via `create_root_info_span`
            or a process span. Are you logging events after tracing has been shutdown?",
        );

        let mut data_provider = match (root_span.as_ref(), parent_span.as_ref()) {
            (Some(root_span), Some(parent_span)) => DataProvider::new(root_span, parent_span),
            _ => DataProvider::none(),
        };

        if !self.middlewares.is_empty() {
            for middleware in &self.middlewares {
                match middleware.on_log_record(log_record, &mut data_provider) {
                    Some(next_record) => {
                        log_record = next_record;
                    }
                    None => {
                        // Event was filtered out by middleware, early return
                        return;
                    }
                }
            }
        }

        // Notify consumers if the event was not filtered out by middleware
        for (index, consumer) in self.consumers.iter().enumerate() {
            debug_assert!(
                index < 64,
                "Consumer index must be less than 64. Invariant is preserved by construction."
            );

            if !consumer.is_log_enabled(&log_record) {
                continue;
            }

            // Check parent span is enabled for this consumer. This is the fast path
            // for the common case where parent span is not filtered out and we
            // can pass the record as is
            if !parent_span_filter_mask.is_filtered(index) {
                // Parent span is not filtered out, we can pass the record as is
                // No need to search for unfiltered parent
                consumer.on_log_record(&log_record, &mut data_provider);
                continue;
            }

            // Looking up filtered parent span ID requires a parent span, so check
            // we have one
            let Some(curr_parent) = parent_span.as_ref() else {
                // No parent span, so no filtering to do & can't get a real data provider
                consumer.on_log_record(&log_record, &mut data_provider);
                continue;
            };

            // Slow path: parent span was filtered out for this consumer.
            // Find the closest unfiltered parent span ID for this consumer (if any)
            // and then create a new record with the updated parent span ID
            let active_parent_span_id = lookup_filtered_parent_span_id(index, curr_parent);

            // Now create a new record with the updated parent span ID
            let modified_record = LogRecordInfo {
                span_id: active_parent_span_id,
                ..log_record.clone()
            };

            consumer.on_log_record(&modified_record, &mut data_provider);
        }
    }
}

fn lookup_filtered_parent_span_id<S>(index: usize, curr: &SpanRef<'_, S>) -> Option<u64>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    let Some(mut parent) = curr.parent() else {
        // No parent span, so no active parent span ID
        return None;
    };

    loop {
        // Create a block scope to limit the borrow of parent extensions
        {
            let parent_ext = parent.extensions();
            let parent_span_filter_mask = parent_ext
                .get::<FilterMask>()
                .copied()
                .unwrap_or_else(FilterMask::empty);

            // Check if this parent span was filtered out for this consumer
            if !parent_span_filter_mask.is_filtered(index) {
                // Found an unfiltered parent span for this consumer. Extract its span ID
                if let Some(parent_span_record) = parent_ext.get::<DLSpanStartInfo>() {
                    return Some(parent_span_record.0.span_id);
                } else {
                    unreachable!("Parent span must have a SpanStartInfo record in its extensions");
                }
            }
        }

        let Some(grand_parent) = parent.parent() else {
            // No parent span, so no active parent span ID
            return None;
        };

        parent = grand_parent;
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub fn get_span_start_info_from_span(
    span: &SpanRef<'_, impl Subscriber + for<'lookup> LookupSpan<'lookup>>,
) -> Option<SpanStartInfo> {
    let span_ext = span.extensions();
    span_ext
        .get::<DLSpanStartInfo>()
        .map(|start_info| (**start_info).clone())
}

#[cfg(test)]
mod tests {
    use super::FilterMask;

    #[test]
    fn empty_mask_reports_empty() {
        let mask = FilterMask::empty();

        for i in 0..64 {
            assert!(!mask.is_filtered(i));
        }
    }

    #[test]
    fn full_mask_reports_full() {
        let mask = FilterMask::disabled();

        assert!(mask.is_disabled());
        for i in 0..64 {
            assert!(mask.is_filtered(i));
        }
    }

    #[test]
    fn set_filtered_marks_bits() {
        let mut mask = FilterMask::empty();

        assert!(!mask.is_filtered(1));
        assert!(!mask.is_filtered(63));

        mask.set_filtered(1);
        mask.set_filtered(63);

        assert!(mask.is_filtered(1));
        assert!(mask.is_filtered(63));
        assert!(!mask.is_filtered(0));
    }
}
