//! The dbt adapter layer.

#![allow(clippy::let_and_return)]

use dbt_common::ErrorCode;

mod macro_exec;
mod value;

pub mod adapter;
pub mod cache;
pub mod catalog_relation;
pub mod column;
/// Connection management: the per-worker-thread connection and its guard.
pub mod connection;
pub mod engine;
pub mod errors;
pub mod format_ident;
pub mod formatter;
pub mod introspective_taint;
pub mod load_catalogs;
pub mod metadata;
pub mod need_quotes;
pub(crate) mod python;
pub mod query_ctx;
pub mod relation;
pub mod render_constraint;
pub mod response;
pub(crate) mod seed;
pub mod snapshots;
/// Tokenizing and fuzzy diffing of SQL strings
pub mod sql;
pub mod sql_types;
pub mod statement;
pub mod stmt_splitter;

/// Cross-Version Record/Replay System
pub mod time_machine;

// Re-export types and modules that were moved to dbt_auth
pub mod auth;
pub mod config {
    pub use dbt_auth::AdapterConfig;
}

/// Parse adapter
pub mod parse;

pub mod mock;

pub mod record_batch;

pub mod cast_util;

/// SqlEngine
pub use engine::AdapterEngine;

/// Functions exposed to jinja
pub mod load_store;

pub use adapter::Adapter;
pub use adapter::AdapterImpl;
pub use adapter::store::{AdapterBuilder, AdapterStore};
pub use column::{Column, ColumnBuilder};
pub use dbt_adapter_core::AdapterType;
pub use errors::AdapterResult;
pub use macro_exec::{
    convert_macro_result_to_record_batch, execute_macro_with_package,
    execute_macro_wrapper_with_package,
};
pub use response::AdapterResponse;

/// Suffix appended to a versioned model's `unique_id` to identify the un-versioned
/// latest-version pointer view's own adapter calls.
///
/// A versioned model with an explicit `latest_version` gets a second relation — the
/// un-versioned pointer view — materialized after the base model's own materialization
/// completes. `materialize_latest_version_pointer` tags that view's adapter calls with
/// `{base_unique_id}{LATEST_VERSION_POINTER_SUFFIX}` so replay can tell them apart from the
/// base model's. Anything that resolves a node id back to recorded events has to know about
/// this suffix; keep the single definition here so the producer and the consumers can't drift.
pub const LATEST_VERSION_POINTER_SUFFIX: &str = "__latest_version_pointer";

/// IMPORTANT: don't change this function to add a new adapter!!! Change the
/// [NON_EXPERIMENTAL_ADAPTERS](dbt_adapter_core::NON_EXPERIMENTAL_ADAPTERS)
/// instead.
pub fn experimental_adapters_allowed() -> bool {
    use dbt_common::tracing::dbt_emit::emit_warn_log_message;

    match dbt_env::env_var_bool("DBT_ALLOW_EXPERIMENTAL_ADAPTERS") {
        Ok(None) => {
            // Allow experimental adapters in debug builds by default,
            // but deny on release builds...
            cfg!(debug_assertions)
        }
        Ok(Some(allow)) => allow, // ...unless explicitly allowed.
        Err(msg) => {
            emit_warn_log_message(ErrorCode::InvalidConfig, msg);
            false // disable when variable value is malformed
        }
    }
}

pub fn enforce_adapter_gating(
    adapter_type: AdapterType,
    allow_experimental_adapters: bool,
) -> AdapterResult<()> {
    use dbt_adapter_core::NON_EXPERIMENTAL_ADAPTERS;
    use dbt_common::{AdapterError, AdapterErrorKind};

    if NON_EXPERIMENTAL_ADAPTERS.contains(&adapter_type) {
        return Ok(());
    }

    if allow_experimental_adapters {
        return Ok(());
    }

    let mut message = format!(
        "The '{}' adapter is not yet supported by dbt. \
Supported adapters: ",
        adapter_type
    );
    let mut supported = NON_EXPERIMENTAL_ADAPTERS.iter();
    message.push_str(supported.next().unwrap().as_ref());
    for adapter in supported {
        message.push_str(", ");
        message.push_str(adapter.as_ref());
    }
    message.push_str(". To use an experimental adapter, set the environment variable DBT_ALLOW_EXPERIMENTAL_ADAPTERS=true. \
Note that experimental adapters may be unstable and are not yet recommended for production use.");

    Err(AdapterError::new(AdapterErrorKind::Configuration, message))
}
