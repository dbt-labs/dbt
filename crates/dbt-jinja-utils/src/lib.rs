#![deny(missing_docs)]
//! This crate provides utilities for working with Jinja templates in the context of dbt.

/// Module for rendering event listener functionality
pub mod listener;

/// Module for formatting Jinja macro arguments
pub mod jinja_arg_format;

/// Module for serialization/deserialization functionality
pub mod serde;

/// Module containing utility functions and helpers
pub mod utils;

/// The `info_schema()` Jinja helper for project quality checks
pub mod info_schema;

/// Module for functions implementations for the dbt jinja context
mod functions;
pub use functions::Var;
pub use functions::env_var;
pub use functions::register_base_functions;
pub use functions::silence_base_context;

/// Module for the Jinja Environment
pub mod jinja_environment;

/// Module for building a Minijinja Environment
mod environment_builder;
pub use environment_builder::{JinjaEnvBuilder, MacroUnitsWrapper};

/// Implements dbt's flags object for Minijinja
pub mod flags;

/// Module for the different phases of the dbt jinja environment
pub mod phases;
pub use phases::parse::init::{DefaultJinjaFactory, JinjaFactory};

/// Module for the Invocation Args
pub mod invocation_args;

/// Module for the invocation-scoped Jinja `graph` mapping
pub mod invocation_graph;

/// Module for the Refs and Sources
pub mod node_resolver;

/// Module for the typechecking
pub mod typecheck;

/// TypecheckingEventListener implementation for YAML Jinja type checking
pub mod typecheck_listener;

/// Module for rendering-based mangled ref/source checking
pub mod mangled_ref;

/// Mock Jinja object
#[cfg(any(test, feature = "testing"))]
pub mod mock_object;

/// Parse-time listener that warns on malformed snapshot/docs block names
pub mod malformed_block_name;
