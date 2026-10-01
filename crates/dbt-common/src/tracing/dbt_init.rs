//! Dbt-specific tracing initialization built on top of the generic subscriber setup.

use std::sync::Arc;

use dbt_error::{ErrorCode, FsError, FsResult, fs_err};

use super::{
    config::FsTraceConfig,
    dbt_data_layer::{dbt_data_layer_config, dbt_process_span_attributes},
    dbt_emit::print_err_from_fs_error,
    tracing_feature_handles::TracingConfigProvider,
};
use dbt_tracing::{
    TelemetryAttributes,
    init::{BaseSubscriber, TelemetryHandle},
    layer::{ConsumerLayer, MiddlewareLayer},
    layers::data_layer::TelemetryDataLayer,
    reload::{TelemetryReloadHandle, create_reloadable_data_layer},
    shutdown::TelemetryShutdownItem,
};
#[cfg(test)]
use tracing::Subscriber;
use tracing::{level_filters::LevelFilter, span};
use tracing_subscriber::Layer;

const DBT_TRACING_FILTER_DIRECTIVES: &[&str] = &[
    "hyper=off",
    "h2=off",
    "reqwest=off",
    "ureq=off",
    "opentelemetry=off",
];

/// Maps dbt output verbosity to the base subscriber cap.
///
/// dbt keeps the subscriber open at DEBUG unless TRACE is requested. This lets
/// DEBUG spans/events enter the telemetry pipeline even when a user-facing sink
/// is configured for INFO or lower; individual consumer layers still apply the
/// actual stdout, file, telemetry, or other sink-specific filters. TRACE remains
/// opt-in because native trace spans can be high-volume developer diagnostics.
fn dbt_max_log_verbosity(max_log_verbosity: LevelFilter) -> LevelFilter {
    if matches!(max_log_verbosity, LevelFilter::TRACE) {
        LevelFilter::TRACE
    } else {
        LevelFilter::DEBUG
    }
}

/// Creates a tracing subscriber with dbt's default base filter behavior.
#[cfg(test)]
pub(crate) fn create_tracing_subcriber_with_layer<
    D: Layer<BaseSubscriber> + Send + Sync + 'static,
>(
    max_log_verbosity: LevelFilter,
    data_layer: D,
) -> impl Subscriber + Send + Sync + 'static {
    dbt_tracing::init::create_tracing_subcriber_with_layer(
        dbt_max_log_verbosity(max_log_verbosity),
        data_layer,
        DBT_TRACING_FILTER_DIRECTIVES,
    )
    .expect("dbt tracing filter directives must be valid")
}

/// Initializes tracing with dbt's default base filter behavior.
pub fn init_tracing_with_data_layer<D: Layer<BaseSubscriber> + Send + Sync + 'static>(
    max_log_verbosity: LevelFilter,
    process_attributes: TelemetryAttributes,
    data_layer: D,
) -> dbt_tracing::error::TracingResult<span::Span> {
    dbt_tracing::init::init_tracing(
        dbt_max_log_verbosity(max_log_verbosity),
        process_attributes,
        data_layer,
        DBT_TRACING_FILTER_DIRECTIVES,
    )
}

/// Initializes tracing from an explicitly assembled dbt middleware and consumer stack.
pub fn init_tracing_with_layers(
    package: &'static str,
    fallback_trace_id: u128,
    fallback_parent_span_id: Option<u64>,
    max_log_verbosity: LevelFilter,
    middlewares: Vec<MiddlewareLayer>,
    consumer_layers: Vec<ConsumerLayer>,
    shutdown_items: Vec<TelemetryShutdownItem>,
) -> FsResult<TelemetryHandle> {
    // Strip code location in non-debug builds
    let strip_code_location = !cfg!(debug_assertions);

    let data_layer = TelemetryDataLayer::new(
        dbt_data_layer_config(fallback_trace_id, fallback_parent_span_id),
        strip_code_location,
        middlewares.into_iter(),
        consumer_layers.into_iter(),
    );

    let process_span = init_tracing_with_data_layer(
        max_log_verbosity,
        dbt_process_span_attributes(package),
        data_layer,
    )
    .map_err(FsError::from)?;

    Ok(TelemetryHandle::new(shutdown_items, process_span))
}

/// Process-wide tracing state for a host that runs many invocations.
///
/// Must stay alive for the process: the process span it owns has to outlive any thread
/// emitting telemetry.
pub struct ProcessTracing {
    _process_span: span::Span,
    reload_handle: TelemetryReloadHandle<BaseSubscriber>,
}

impl ProcessTracing {
    /// Installs `config`'s consumer layers for one invocation, giving it its own log path,
    /// verbosity and warn-error options.
    ///
    /// Returns a guard to hold for the invocation, and the feature stack's config provider.
    pub fn begin_invocation(
        &self,
        config: FsTraceConfig,
        tracing_config: Arc<dyn TracingConfigProvider>,
    ) -> FsResult<InvocationTracingGuard> {
        let (middlewares, consumer_layers, shutdown_items) =
            config.build_layers(tracing_config)?.into_parts();

        self.reload_handle
            .reload_telemetry(middlewares, consumer_layers)
            .map_err(|e| {
                fs_err!(
                    ErrorCode::Generic,
                    "Failed to install invocation telemetry layers: {}",
                    e
                )
            })?;

        Ok(InvocationTracingGuard {
            shutdown_items,
            reload_handle: self.reload_handle.clone(),
            finished: false,
        })
    }
}

/// Initializes tracing with no consumer layers; call [`ProcessTracing::begin_invocation`]
/// per invocation to install them.
///
/// `max_log_verbosity` is applied once, so it must suit every later invocation — anything it
/// excludes reaches none of their layers. [`dbt_max_log_verbosity`] collapses everything
/// below `TRACE` to `DEBUG`, making the choice effectively binary.
pub fn init_tracing_cli_reloadable(
    package: &'static str,
    max_log_verbosity: LevelFilter,
) -> FsResult<ProcessTracing> {
    // Strip code location in non-debug builds
    let strip_code_location = !cfg!(debug_assertions);

    // No invocation exists yet to borrow an ID from. Root spans carry their own trace ID, so
    // this only labels stray events emitted outside any invocation.
    let fallback_trace_id = uuid::Uuid::now_v7().as_u128();

    let (data_layer, reload_handle) = create_reloadable_data_layer(
        dbt_data_layer_config(fallback_trace_id, None),
        strip_code_location,
    );

    let process_span = init_tracing_with_data_layer(
        max_log_verbosity,
        dbt_process_span_attributes(package),
        data_layer,
    )
    .map_err(FsError::from)?;

    Ok(ProcessTracing {
        _process_span: process_span,
        reload_handle,
    })
}

/// Holds this invocation's telemetry layers. Call [`Self::finish`] to tear them down and see
/// the failures; `Drop` does the same work and prints them, so none are lost silently.
pub struct InvocationTracingGuard {
    shutdown_items: Vec<TelemetryShutdownItem>,
    reload_handle: TelemetryReloadHandle<BaseSubscriber>,
    finished: bool,
}

impl InvocationTracingGuard {
    /// Flushes this invocation's layers and detaches them, returning every failure.
    ///
    /// A failure means log lines or telemetry rows were lost, so callers should surface it.
    pub fn finish(mut self) -> Result<(), Vec<FsError>> {
        let errors = self.teardown();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn teardown(&mut self) -> Vec<FsError> {
        // Set before the work, not after: a panicking shutdown would otherwise be retried
        // during unwinding, and a second panic there aborts the process.
        if self.finished {
            return Vec::new();
        }
        self.finished = true;

        let mut errors: Vec<FsError> = self
            .shutdown_items
            .iter_mut()
            .filter_map(|item| item.shutdown().err().map(FsError::from))
            .collect();

        if let Err(e) = self.reload_handle.reload_telemetry(vec![], vec![]) {
            errors.push(*fs_err!(
                ErrorCode::Generic,
                "Failed to detach invocation telemetry layers: {}",
                e
            ));
        }

        errors
    }
}

impl Drop for InvocationTracingGuard {
    fn drop(&mut self) {
        // Shutting the items down here is load-bearing, not a redundant backstop: the file
        // and parquet writers do flush via their own handles' `Drop`, but OTLP does not —
        // `OTLPExporterLayer` co-owns the providers (its `tracer` and `logger` each hold a
        // strong ref too), so releasing `shutdown_items` never drops the last `Arc` and
        // their shutdown-on-drop never fires.
        //
        // Printed rather than discarded, as the standalone binary does (main_impl.rs);
        // `finished` keeps this quiet on the normal `finish` path.
        //
        // Covers unwinding panics, but not a host that exits via a panic hook or
        // `hard_exit` — neither runs destructors, so neither reaches this or `finish`.
        //
        // print_err_from_fs_error, not eprintln!: tracing and the process span are
        // still alive here, so the failure goes through the same telemetry path as
        // everything else instead of bypassing it.
        for error in self.teardown() {
            print_err_from_fs_error(&error);
        }
    }
}
