//! Module for the compile phase

mod compile_config;
mod compile_node_context;

pub use compile_node_context::DependencyValidationConfig;
pub use compile_node_context::REPL_MODEL_NAME;
pub use compile_node_context::{build_compile_node_context, build_compile_node_context_inner};
