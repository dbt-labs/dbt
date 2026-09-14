mod config;
pub mod dbt_convert;
mod dbt_data_layer;
pub mod dbt_emit;
pub mod dbt_init;
pub mod dbt_metrics;
pub mod event_classifiers;
pub mod formatters;
pub mod fs_error_log;
pub mod invocation;
pub mod layers;
pub mod middlewares;
mod private_events;
pub mod tracing_feature_handles;

pub use config::{
    FsTraceConfig, FsTraceConfigBuilder, build_file_log_consumer, build_jsonl_file_consumer,
    build_shared_middleware_layers,
};
pub use dbt_data_layer::{dbt_data_layer_config, dbt_process_span_attributes};
pub use dbt_init::init_tracing_with_data_layer;
pub use dbt_tracing::async_tracing::{spawn_traced, spawn_traced_block_in_place};
pub use dbt_tracing::emit::{
    create_debug_span, create_debug_span_with_parent, create_info_span,
    create_info_span_with_parent, create_root_info_span,
};
pub use dbt_tracing::init::TelemetryHandle;
pub use dbt_tracing::{data_provider, emit, error, event_info, layer, metrics, reload, span_info};
pub use tracing_feature_handles::{TracingConfigProvider, noop_tracing_config_provider};

#[cfg(test)]
mod tests;
