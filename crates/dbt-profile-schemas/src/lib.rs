use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::profiles::DbConfig;

mod athena_config;
mod bigquery_config;
mod clickhouse_config;
mod databricks_config;
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

        match self.adapter_type {
            Snowflake => {
                let c0 = unwrap_db_config!(Snowflake);
                let c1 = snowflake_config::setup_snowflake_profile(c0.map(Box::as_ref))?;
                Ok(DbConfig::Snowflake(c1))
            }
            Bigquery => {
                let c0 = unwrap_db_config!(Bigquery);
                let c1 = bigquery_config::setup_bigquery_profile(c0.map(Box::as_ref))?;
                Ok(DbConfig::Bigquery(c1))
            }
            Databricks => {
                let c0 = unwrap_db_config!(Databricks);
                let c1 = databricks_config::setup_databricks_profile(c0.map(Box::as_ref))?;
                Ok(DbConfig::Databricks(c1))
            }
            Redshift => {
                let c0 = unwrap_db_config!(Redshift);
                let c1 = redshift_config::setup_redshift_profile(c0.map(Box::as_ref))?;
                Ok(DbConfig::Redshift(c1))
            }
            Spark => {
                let _c0 = unwrap_db_config!(Spark);
                todo!("setup_spark_profile")
            }
            DuckDB => {
                let _c0 = unwrap_db_config!(DuckDB);
                // DuckDB doesn't require credentials for local file-based operations
                // TODO: Create proper DuckDB profile setup
                Err(fs_err!(
                    ErrorCode::Generic,
                    "DuckDB profile setup not yet implemented. DuckDB runs locally without credentials."
                ))
            }
            Postgres => {
                let c0 = unwrap_db_config!(Postgres);
                let c1 = postgres_config::setup_postgres_profile(c0.map(Box::as_ref))?;
                Ok(DbConfig::Postgres(c1))
            }
            Salesforce => {
                let _c0 = unwrap_db_config!(Salesforce);
                todo!("setup_salesforce_profile")
            }
            Fabric => {
                let c0 = unwrap_db_config!(Fabric);
                let c1 = fabric_config::setup_fabric_profile(c0.map(Box::as_ref))?;
                Ok(DbConfig::Fabric(c1))
            }
            ClickHouse => {
                let c0 = unwrap_db_config!(ClickHouse);
                let c1 = clickhouse_config::setup_clickhouse_profile(c0.map(Box::as_ref))?;
                Ok(DbConfig::ClickHouse(c1))
            }
            Exasol => {
                let c0 = unwrap_db_config!(Exasol);
                let c1 = exasol_config::setup_exasol_profile(c0.map(Box::as_ref))?;
                Ok(DbConfig::Exasol(c1))
            }
            Athena => {
                let c0 = unwrap_db_config!(Athena);
                let c1 = athena_config::setup_athena_profile(c0.map(Box::as_ref))?;
                Ok(DbConfig::Athena(c1))
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
                let _c0 = unwrap_db_config!(LakeCompute);
                // TODO: Create proper lake compute profile setup
                Err(fs_err!(
                    ErrorCode::Generic,
                    "lake_compute profile setup not yet implemented."
                ))
            }
        }
    }
}
