#![allow(clippy::let_and_return)]
#![allow(clippy::collapsible_else_if)]

use std::io;

use dbt_adbc::{Backend, database};

mod config;

// Database-specific auth implementations
mod athena;
mod bigquery;
mod clickhouse;
mod databricks;
mod duckdb;
mod exasol;
#[cfg(test)]
mod flock;
mod lake_compute;
mod postgres;
mod redshift;
mod salesforce;
pub mod snowflake;
mod spark;
mod sqlserver;
#[cfg(test)]
mod test_options;

pub use config::AdapterConfig;
pub use duckdb::init::{DuckDbTarget, generate_duckdb_init_sql};

pub trait AuthWarningPrinter: Send + Sync {
    fn warn(&self, message: &str);
}

pub struct NoopAuthWarningPrinter;

impl AuthWarningPrinter for NoopAuthWarningPrinter {
    fn warn(&self, _message: &str) {}
}

/// Authorization trait.
pub trait Auth: Send + Sync {
    /// Return the XDBC backend this authenticator is for.
    fn backend(&self) -> Backend;

    /// Configure the XDBC database builder.
    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError>;
}

/// Macro used to structure the AdapterConfig -> database::Builder pipeline
#[macro_export]
macro_rules! auth_configure_pipeline {
    ($self:expr, $cfg:expr, $parse_auth:path, $apply_connection_args:path) => {{
        let authentication_args = $parse_auth($cfg, $self.warning_printer.as_ref())?;

        let builder = database::Builder::new($self.backend());
        let builder = authentication_args.apply(builder, $self.warning_printer.as_ref())?;
        let builder = $apply_connection_args($cfg, builder, $self.warning_printer.as_ref())?;

        Ok(builder)
    }};
}

/// Factory function to create an Auth instance based on the backend type.
pub fn auth_for_backend(backend: Backend) -> Box<dyn Auth> {
    auth_for_backend_with_warnings(backend, Box::new(NoopAuthWarningPrinter))
}

pub fn auth_for_backend_with_warnings(
    backend: Backend,
    warning_printer: Box<dyn AuthWarningPrinter>,
) -> Box<dyn Auth> {
    match backend {
        Backend::Snowflake => Box::new(snowflake::SnowflakeAuth::new(warning_printer)),
        Backend::Postgres => Box::new(postgres::PostgresAuth::new(warning_printer)),
        Backend::BigQuery => Box::new(bigquery::BigqueryAuth::new(warning_printer)),
        Backend::Databricks => Box::new(databricks::DatabricksAuth::new(warning_printer)),
        Backend::Redshift => Box::new(redshift::RedshiftAuth::new(warning_printer)),
        Backend::Salesforce => Box::new(salesforce::SalesforceAuth::new(warning_printer)),
        Backend::Spark => Box::new(spark::SparkAuth::new(warning_printer)),
        Backend::DuckDB | Backend::DuckDBExtended => {
            Box::new(duckdb::DuckDbAuth::new(backend, warning_printer))
        }
        Backend::LakeCompute => Box::new(lake_compute::LakeComputeAuth::new(warning_printer)),
        Backend::SQLServer => Box::new(sqlserver::SQLServerAuth::new(warning_printer)),
        Backend::ClickHouse => Box::new(clickhouse::ClickHouseAuth::new(warning_printer)),
        Backend::Athena => Box::new(athena::AthenaAuth::new(warning_printer)),
        Backend::Exasol => Box::new(exasol::ExasolAuth::new(warning_printer)),
        Backend::Generic { .. } => unimplemented!("generic backend authentication"),
    }
}

/// Error type for [dbt_auth].
///
/// For display purposes, it must be converted into an [AdapterError] first, outside of this crate.
#[derive(Debug)]
pub enum AuthError {
    /// Error from the [adbc_core] crate
    Adbc(adbc_core::error::Error),
    /// A generic configuration error
    Config(String),
    /// An error from the [serde_json] crate
    JSON(serde_json::Error),
    /// An error from the [dbt_yaml] crate
    YAML(dbt_yaml::Error),
    /// I/O error
    Io(io::Error),
}

impl AuthError {
    /// Creates a new [AuthError] from a custom message describing a configuration error.
    pub fn config(message: impl Into<String>) -> Self {
        AuthError::Config(message.into())
    }

    /// Returns a non-owned string with an error message.
    ///
    /// Used for test assertions. For display purposes, it must be converted into an
    /// [AdapterError] first outside of this crate.
    pub fn msg(&self) -> &str {
        match self {
            AuthError::Adbc(_) => "ADBC Error",
            AuthError::Config(msg) => msg,
            AuthError::JSON(_) => "JSON Error",
            AuthError::YAML(_) => "YAML Error",
            AuthError::Io(_) => "I/O Error",
        }
    }
}

impl From<adbc_core::error::Error> for AuthError {
    fn from(err: adbc_core::error::Error) -> Self {
        AuthError::Adbc(err)
    }
}

impl From<io::Error> for AuthError {
    fn from(err: io::Error) -> Self {
        AuthError::Io(err)
    }
}

impl From<serde_json::Error> for AuthError {
    fn from(err: serde_json::Error) -> Self {
        AuthError::JSON(err)
    }
}

impl From<dbt_yaml::Error> for AuthError {
    fn from(err: dbt_yaml::Error) -> Self {
        AuthError::YAML(err)
    }
}

// Enum for private key providers
//
// Cross-adapter spec for how users may provide private keys,
// either via paths to the keys or the extract key values themselves.
// Prefer strictness about including PEM headers where possible.
// For Snowflake, we are forced to support a plethora of legacy
// compliant PEM encodings. See snowflake/key_format.rs for more
#[derive(Debug)]
pub(crate) enum PrivateKeySource<'a> {
    FilePath(&'a str),
    Raw(&'a str),
}
