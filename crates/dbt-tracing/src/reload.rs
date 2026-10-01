use super::{
    layer::{ConsumerLayer, MiddlewareLayer},
    layers::data_layer::{TelemetryDataLayer, TelemetryDataLayerConfig},
};
use tracing::Subscriber;
use tracing_subscriber::{
    registry::LookupSpan,
    reload::{Error, Handle, Layer},
};

/// A handle that allows updating the telemetry consumer layers at runtime.
///
/// Use for testing or advanced scenarios only.
pub struct TelemetryReloadHandle<S>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    config: TelemetryDataLayerConfig,
    strip_code_location: bool,
    with_sequential_ids: bool,
    data_layer_reload_handle: Handle<TelemetryDataLayer<S>, S>,
}

impl<S> Clone for TelemetryReloadHandle<S>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    fn clone(&self) -> Self {
        TelemetryReloadHandle {
            config: self.config,
            strip_code_location: self.strip_code_location,
            with_sequential_ids: self.with_sequential_ids,
            data_layer_reload_handle: self.data_layer_reload_handle.clone(),
        }
    }
}

impl<S> TelemetryReloadHandle<S>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    pub(super) fn new(
        config: TelemetryDataLayerConfig,
        strip_code_location: bool,
        with_sequential_ids: bool,
        handle: Handle<TelemetryDataLayer<S>, S>,
    ) -> Self {
        TelemetryReloadHandle {
            config,
            strip_code_location,
            with_sequential_ids,
            data_layer_reload_handle: handle,
        }
    }

    pub fn reload_telemetry(
        &self,
        middlewares: Vec<MiddlewareLayer>,
        consumer_layers: Vec<ConsumerLayer>,
    ) -> Result<(), Error> {
        let mut data_layer = TelemetryDataLayer::new(
            self.config,
            self.strip_code_location,
            middlewares.into_iter(),
            consumer_layers.into_iter(),
        );

        if self.with_sequential_ids {
            data_layer.with_sequential_ids();
        }

        self.data_layer_reload_handle.reload(data_layer)
    }
}

fn create_reloadable_data_layer_inner<S>(
    config: TelemetryDataLayerConfig,
    strip_code_location: bool,
    with_sequential_ids: bool,
    middlewares: Vec<MiddlewareLayer>,
    consumer_layers: Vec<ConsumerLayer>,
) -> (Layer<TelemetryDataLayer<S>, S>, TelemetryReloadHandle<S>)
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    let mut data_layer = TelemetryDataLayer::new(
        config,
        strip_code_location,
        middlewares.into_iter(),
        consumer_layers.into_iter(),
    );

    if with_sequential_ids {
        data_layer.with_sequential_ids();
    }

    let config = data_layer.config();
    let (data_layer, handle) = Layer::new(data_layer);

    (
        data_layer,
        TelemetryReloadHandle::new(config, strip_code_location, with_sequential_ids, handle),
    )
}

/// Data layer that starts with no consumers; install them later via the returned handle.
pub fn create_reloadable_data_layer<S>(
    config: TelemetryDataLayerConfig,
    strip_code_location: bool,
) -> (Layer<TelemetryDataLayer<S>, S>, TelemetryReloadHandle<S>)
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    create_reloadable_data_layer_inner(config, strip_code_location, false, vec![], vec![])
}

pub fn create_data_layer_for_tests<S>(
    config: TelemetryDataLayerConfig,
    middlewares: Vec<MiddlewareLayer>,
    consumer_layers: Vec<ConsumerLayer>,
) -> (Layer<TelemetryDataLayer<S>, S>, TelemetryReloadHandle<S>)
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
{
    create_reloadable_data_layer_inner(
        config,
        // always strip code location in tests, and use sequential IDs to make them predictable
        true,
        true,
        middlewares,
        consumer_layers,
    )
}
