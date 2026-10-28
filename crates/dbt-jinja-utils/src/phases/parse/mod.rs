//! This module contains everything related to configuring the Jinja environment for the parse phase.

mod adapter_scope;
mod resolve_context;
mod resolve_model_context;

pub mod init;
pub mod sql_resource;

pub use crate::utils::render_extract_ref_or_source_expr;
pub use adapter_scope::{ParseAdapterScope, ParseAdapterScopes};
pub use resolve_context::{
    build_docs_jinja_environment, build_docs_resolve_context, build_resolve_context,
};
pub use resolve_model_context::build_resolve_model_context;
