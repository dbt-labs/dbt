pub mod bigquery_config;
pub mod clickhouse_config;
pub mod common;
pub mod databricks_config;
pub mod exasol_config;
pub mod fabric_config;
pub mod headless;
pub mod postgres_config;
pub mod profile;
pub mod redshift_config;
pub mod snowflake_config;

// Re-export the commonly used items so a single `use dbt_profile_schemas::*`
// gives everything the old `dbt-init::adapter_config::*` provided, plus the
// profile types and the headless (non-interactive) API.
pub use common::*;
pub use profile::*;

pub use bigquery_config::setup_bigquery_profile;
pub use clickhouse_config::setup_clickhouse_profile;
pub use databricks_config::setup_databricks_profile;
pub use exasol_config::setup_exasol_profile;
pub use fabric_config::setup_fabric_profile;
pub use postgres_config::setup_postgres_profile;
pub use redshift_config::setup_redshift_profile;
pub use snowflake_config::setup_snowflake_profile;

pub use headless::{adapter_fields, apply_values, build_profile_target, supported_adapters};
