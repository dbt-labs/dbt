//! The dbt Adapter Engine.

use adbc_core::options::OptionValue;

pub mod behavior;

pub mod concat_batches;

/// Cow module for managing copy-on-write data structures (mostly strings).
pub mod cow;

pub mod databricks;
pub mod duckdb;
pub mod errors;
pub mod load_catalogs;

pub mod map_reduce;
pub use map_reduce::ConnectionFactory;
pub use map_reduce::MapReduce;

pub mod noop_connection;
pub mod query_comment;
pub mod retry;
pub mod sidecar_client;
pub mod statement;

pub type Options = Vec<(String, OptionValue)>;
