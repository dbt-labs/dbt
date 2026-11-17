use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::profiles::DbConfig;

mod bigquery_config;
mod clickhouse_config;
mod databricks_config;
mod duckdb_config;
mod exasol_config;
mod fabric_config;
mod postgres_config;
mod redshift_config;
mod snowflake_config;

pub mod common;
pub mod headless;
pub mod profile;

pub use headless::apply_values;

pub struct ProfileSetup {
    adapter_type: AdapterType,
}

impl ProfileSetup {
    pub fn new(adapter_type: AdapterType) -> Self {
        Self { adapter_type }
    }

    /// Takes a type-erased config, unpacks the typed config, then boxes the
    /// resulting config again.
    ///
    /// NOTE(felipecrv): this should be simplified by using type-erased
    /// configurations across the complete configuration pipeline.
    #[allow(clippy::cognitive_complexity)]
    pub fn setup(self, existing_config: Option<&DbConfig>) -> FsResult<DbConfig> {
        use AdapterType::*;

        macro_rules! unwrap_db_config {
            ($variant:ident) => {
                match existing_config {
                    None => None,
                    Some(DbConfig::$variant(c)) => Some(c),
                    Some(_) => {
                        debug_assert!(false, "DbConfig variant doesn't match AdapterType");
                        None
                    }
                }
            };
        }

        macro_rules! setup_profile {
            ($variant:ident, $setup:path) => {
                $setup(unwrap_db_config!($variant).map(Box::as_ref)).map(DbConfig::$variant)
            };
        }

        match self.adapter_type {
            RedisAdbc => Err(fs_err!(
                ErrorCode::InvalidConfig,
                "Create a profiles.yml entry with type: redis_adbc; interactive Redis setup is not yet supported"
            )),
            Snowflake => setup_profile!(Snowflake, snowflake_config::setup_snowflake_profile),
            Bigquery => setup_profile!(Bigquery, bigquery_config::setup_bigquery_profile),
            Databricks => setup_profile!(Databricks, databricks_config::setup_databricks_profile),
            Redshift => setup_profile!(Redshift, redshift_config::setup_redshift_profile),
            Spark => {
                let _c0 = unwrap_db_config!(Spark);
                todo!("setup_spark_profile")
            }
            DuckDB => setup_profile!(DuckDB, duckdb_config::setup_duckdb_profile),
            Postgres => setup_profile!(Postgres, postgres_config::setup_postgres_profile),
            Salesforce => {
                let _c0 = unwrap_db_config!(Salesforce);
                todo!("setup_salesforce_profile")
            }
            Fabric => setup_profile!(Fabric, fabric_config::setup_fabric_profile),
            ClickHouse => setup_profile!(ClickHouse, clickhouse_config::setup_clickhouse_profile),
            Exasol => setup_profile!(Exasol, exasol_config::setup_exasol_profile),
            Athena => {
                todo!("setup_athena_profile")
            }
            Starburst => {
                todo!("setup_starburst_profile")
            }
            Trino => {
                let _c0 = unwrap_db_config!(Trino);
                todo!("setup_trino_profile")
            }
            Datafusion => {
                let _c0 = unwrap_db_config!(Datafusion);
                todo!("setup_datafusion_profile")
            }
            Dremio => {
                todo!("setup_dremio_profile")
            }
            Oracle => {
                todo!("setup_oracle_profile")
            }
            LakeCompute => {
                // Nothing to ask: it signs in with a Fivetran token, read from
                // the environment so it stays out of profiles.yml.
                let existing = unwrap_db_config!(LakeCompute);
                Ok(DbConfig::LakeCompute(Box::new(lake_compute_config(
                    existing.map(|c| c.as_ref()),
                ))))
            }
        }
    }
}

/// The env var a lakecompute output reads its Fivetran token from.
pub const LAKE_COMPUTE_TOKEN_ENV: &str = "DBT_LAKE_COMPUTE_TOKEN";

/// A lakecompute output: `method: fivetran`, its token from
/// [`LAKE_COMPUTE_TOKEN_ENV`]. Empty by default, so a profile with it loads where
/// the token isn't set (dbt renders every output of a target). An existing
/// output keeps its settings.
fn lake_compute_config(
    existing: Option<&dbt_schemas::schemas::profiles::LakeComputeConfig>,
) -> dbt_schemas::schemas::profiles::LakeComputeConfig {
    let mut config = existing.cloned().unwrap_or_default();
    config.method.get_or_insert_with(|| "fivetran".to_string());
    config
        .fivetran_credential
        .get_or_insert_with(|| format!("{{{{ env_var('{LAKE_COMPUTE_TOKEN_ENV}', '') }}}}"));
    config
}
