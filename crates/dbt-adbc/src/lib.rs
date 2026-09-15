#![cfg_attr(docsrs, feature(doc_auto_cfg, doc_cfg))]
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/apache/arrow/refs/heads/main/docs/source/_static/favicon.ico",
    html_favicon_url = "https://raw.githubusercontent.com/apache/arrow/refs/heads/main/docs/source/_static/favicon.ico"
)]
#![doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"))]
#![allow(clippy::cognitive_complexity)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::let_and_return)]
#![allow(clippy::needless_bool)]
#![allow(clippy::should_implement_trait)]

use std::ffi::c_char;

pub(crate) mod env_var;

pub mod driver;
pub use driver::Backend;
pub use driver::Driver;
pub use driver::LoadStrategy;

pub mod database;
pub use database::Database;

pub mod connection;
pub use connection::Connection;

pub mod statement;
pub use statement::Statement;

pub mod query_ctx;
pub use query_ctx::QueryCtx;

pub mod semaphore;

pub(crate) mod builder;
pub(crate) mod checksums;
pub(crate) mod driver_channel;
pub mod driver_manager;
pub mod duration;
pub mod install;

// Constants for different backends
pub mod athena;
pub mod bigquery;
pub mod clickhouse;
pub mod databricks;
pub mod lake_compute;
pub mod redshift;
pub mod salesforce;
pub mod snowflake;
pub mod spark;

// REPL for ADBC drivers
#[cfg(feature = "repl")]
pub mod repl;

/// Interpret the SQLSTATE [1] 5-char ASCII string as a Rust string.
///
/// [1] https://en.wikipedia.org/wiki/SQLSTATE
pub fn str_from_sqlstate(sqlstate: &[c_char; 5]) -> &str {
    // This is safe because the range of the byte values is validated by str::from_utf8 below.
    // It would be unnecessary if Rust ADBC used u8 for [`Error::sqlstate`] [1] instead of i8.
    //
    // [1] https://github.com/apache/arrow-adbc/pull/1725#discussion_r1567531539
    let unsigned: &[u8; 5] = unsafe { std::mem::transmute(sqlstate) };
    let res = std::str::from_utf8(unsigned);
    debug_assert!(res.is_ok(), "SQLSTATE is not valid ASCII: {sqlstate:?}");
    res.unwrap_or("")
}

pub const SNOWFLAKE_DRIVER_VERSION: &str = "0.21.0.dev+dbt0.21.18";
/// Legacy driver built from `dbt-labs/arrow-adbc` repository
pub const BIGQUERY_DRIVER_VERSION: &str = "0.21.0.dev+dbt0.21.18";
/// Built from `dbt-labs/bigquery-adbc repository
pub const BIGQUERY_FOUNDRY_DRIVER_VERSION: &str = "0.21.0.dev+dbt0.1.4";
pub const POSTGRES_DRIVER_VERSION: &str = "0.21.0+dbt0.21.0";
pub const DATABRICKS_DRIVER_VERSION: &str = "0.21.0.dev+dbt0.21.13";
pub const REDSHIFT_DRIVER_VERSION: &str = "0.21.0.dev+dbt0.18.8";
pub const DUCKDB_DRIVER_VERSION: &str = "1.5.4";
pub const DUCKDB_EXTENDED_DRIVER_VERSION: &str = "0.21.0.dev+dbt0.0.31";
pub const LAKE_COMPUTE_DRIVER_VERSION: &str = "0.4.0+dbt0.1.10.g53736d6";
pub const CLICKHOUSE_DRIVER_VERSION: &str = "0.1.1";
pub const SALESFORCE_DRIVER_VERSION: &str = "0.21.0.dev+dbt0.22.1";
pub const SPARK_DRIVER_VERSION: &str = "0.21.0.dev+dbt0.1.2";
pub const MSSQLSERVER_DRIVER_VERSION: &str = "1.3.1";
pub const EXASOL_DRIVER_VERSION: &str = "0.9.0";

pub use install::pre_install_all_drivers;
pub use install::pre_install_driver;
