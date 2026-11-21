//! Module for configuring the jinja environment for the run phase

mod run_config;
mod run_node_context;

#[cfg(test)]
mod lazy_model_tests;

pub use run_config::RunConfig;
pub use run_node_context::{
    WriteConfig, build_run_node_context, build_run_node_context_from_arc, build_run_node_ctx,
    extend_base_context_stateful_fn, reset_result_store,
};
