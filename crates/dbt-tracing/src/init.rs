use std::sync::OnceLock;

use tracing::{Subscriber, level_filters::LevelFilter, span};

use tracing_subscriber::{
    EnvFilter, Layer, Registry,
    filter::Directive,
    layer::{Context, Layered, SubscriberExt},
    registry::{LookupSpan, SpanRef},
};

use crate::{
    TelemetryAttributes,
    constants::PROCESS_SPAN_NAME,
    error::{TracingError, TracingResult},
    event_info::store_event_attributes,
    shutdown::TelemetryShutdownItem,
};

// We use a global to store a special "process" span Id, that
// is created during initialization and used as a fallback span
// if any logs or spans are emitted outside of the context of our infrastructure.
//
// This may happen for two reasons:
// - some library used in the code flow before the root operation span is created that
// is not filtered by our `tracing` filters.
// - Intentionally emitted logs outside of the root operation span
//
// Normally any binary using our infra should go through initialisation
// that will assign this span. However, in some scenarios, such as unit
// tests - it may stay uninitialized
static PROCESS_SPAN: OnceLock<span::Id> = OnceLock::new();

/// The process span for the current process. Only available after
/// tracing has been initialized and before tracing handle is dropped.
///
/// See `PROCESS_SPAN` for more details.
pub(super) fn process_span<'a, S>(ctx: &'a Context<'a, S>) -> Option<SpanRef<'a, S>>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    let process_span_id = PROCESS_SPAN.get()?;

    ctx.span(process_span_id)
}

pub type BaseSubscriber = Layered<EnvFilter, Registry>;

fn parse_filter_directives(filter_directives: &[&str]) -> TracingResult<Vec<Directive>> {
    filter_directives
        .iter()
        .map(|directive| {
            directive.parse().map_err(|error| {
                TracingError::invalid_filter_directive(format!(
                    "Invalid tracing filter directive `{directive}`: {error}"
                ))
            })
        })
        .collect()
}

/// The handle returned by the telemetry initialization function.
///
/// Make sure to call `shutdown` on it when you are done with telemetry,
/// to ensure that all telemetry resources are released properly.
pub struct TelemetryHandle {
    items: Vec<TelemetryShutdownItem>,
    // We have Option here to allow first dropping the handle
    // during shutdown, and then closing all layers
    process_span_handle: Option<span::Span>,
}

// This impl block is intended to stay with the future generic tracing library.
impl TelemetryHandle {
    pub fn new(items: Vec<TelemetryShutdownItem>, process_span_handle: span::Span) -> Self {
        TelemetryHandle {
            items,
            process_span_handle: Some(process_span_handle),
        }
    }

    /// Gracefully shuts down telemetry
    pub fn shutdown_once(mut self) -> Result<(), Vec<TracingError>> {
        // First, drop the process span handle to ensure that
        // the process span is closed properly.
        if let Some(handle) = self.process_span_handle.take() {
            drop(handle);
        }

        // Then, do shutdown of all items.
        let errors = self
            .items
            .iter_mut()
            .filter_map(|item| item.shutdown().err())
            .collect::<Vec<_>>();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Initializes tracing with the provided data layer, which is ultimately
/// composed of middleware and consumer layers.
///
/// This function will set up a global tracing subscriber and will fail on re-entry.
///
/// The caller provides process span attributes so this generic initializer does not
/// construct application-specific telemetry data.
///
/// If you need to change or add layers after initialization, use `super::reload::create_realodable_data_layer`,
/// to get a reloadable data layer.
///
/// IMPORTANT: there are a number of extra constraints on consumer layers beyond what
/// the `TelemetryConsumer` trait itself implies:
/// - Never rely on or read span/event attributes provided by `tracing` directly! All of the
///   necessary data for your consumer must come from the structured records (`SpanStartInfo`,
///   `SpanEndInfo`, `LogRecordInfo`). If you lack something, extend the schema of
///   an existing event or add new one and pass new fields at call-sites accordingly.
/// - Apply filtering via `with_filter`, `with_span_filter` and/or `with_log_filter`
///   methods defined for all consumer layers - this will facilitate modularity
///
/// # Returns
///
/// On success, returns the "process" span, used as a parent span fallback of last resort
/// in data layer for events (but not for spans!).
pub fn init_tracing<D: Layer<BaseSubscriber> + Send + Sync + 'static>(
    max_log_verbosity: LevelFilter,
    process_attributes: TelemetryAttributes,
    data_layer: D,
    filter_directives: &[&str],
) -> TracingResult<span::Span> {
    // Check if tracing is already initialized
    if PROCESS_SPAN.get().is_some() {
        return Err(TracingError::AlreadyInitialized);
    }

    let subscriber =
        create_tracing_subcriber_with_layer(max_log_verbosity, data_layer, filter_directives)?;

    tracing::subscriber::set_global_default(subscriber)
        .map_err(|_| TracingError::SetGlobalSubscriber)?;

    // Create the process span and store it in the global PROCESS_SPAN
    store_event_attributes(process_attributes);
    let process_span = tracing::info_span!(PROCESS_SPAN_NAME);

    PROCESS_SPAN
        .set(process_span.id().expect("Process span must have an ID"))
        .expect("Process span must be set only once");

    Ok(process_span)
}

/// Creates a tracing subscriber implementing our telemetry data pipeline.
///
/// See module README for details on the pipeline.
pub fn create_tracing_subcriber_with_layer<D: Layer<BaseSubscriber> + Send + Sync + 'static>(
    max_log_verbosity: LevelFilter,
    data_layer: D,
    filter_directives: &[&str],
) -> TracingResult<impl Subscriber + Send + Sync + 'static> {
    // Set-up global filters first.
    //
    // This max level is the broadest subscriber-level cap for the telemetry
    // pipeline. Consumer layers may still apply narrower filters for their own
    // sinks, allowing different output on stdout, files, telemetry exports, and
    // other consumers.
    //
    // In debug builds we also allow RUST_LOG to control the global level filter.
    // Otherwise, generic subscriber setup uses exactly the caller-provided max
    // level and directive list.

    #[cfg(debug_assertions)]
    let base_telemetry_filter = EnvFilter::builder()
        .with_default_directive(max_log_verbosity.into())
        .from_env_lossy();

    // For prod builds it is almost the same except RUST_LOG is not used
    #[cfg(not(debug_assertions))]
    let base_telemetry_filter = EnvFilter::builder().parse_lossy(max_log_verbosity.to_string());

    let base_telemetry_filter = parse_filter_directives(filter_directives)?
        .into_iter()
        .fold(base_telemetry_filter, |filter, directive| {
            filter.add_directive(directive)
        });

    // Compose the registry with global filter and data layer
    Ok(Registry::default()
        .with(base_telemetry_filter)
        .with(data_layer))
}
