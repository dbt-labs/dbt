#![allow(unused_qualifications)]

use crate::schemas::relations::DEFAULT_DATABRICKS_DATABASE;
use crate::schemas::serde::{DuckDbExtension, QueryTag, StringOrInteger, StringOrMap};

use dbt_adapter_core::AdapterType;
use dbt_yaml::DbtSchema;
use dbt_yaml::UntaggedEnumDeserialize;
use merge::Merge;
use serde_derive::Deserialize;
use serde_derive::Serialize;

use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::{self, Debug, Display};
use std::path::PathBuf;

type YmlValue = dbt_yaml::Value;

pub type ProfileName = String;
pub type TargetName = String;
pub type DefaultTargetName = String;

#[derive(Debug, Deserialize)]
pub struct DbtProfilesIntermediate {
    pub config: Option<dbt_yaml::Value>,
    pub __profiles__: HashMap<ProfileName, dbt_yaml::Value>,
}

#[derive(Debug, Deserialize, Clone, PartialEq, DbtSchema)]
pub struct DbtProfiles {
    pub __profiles__: HashMap<ProfileName, DbConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, UntaggedEnumDeserialize, DbtSchema)]
#[serde(tag = "type")]
#[serde(rename_all = "lowercase")]
#[allow(clippy::large_enum_variant)]
pub enum DbConfig {
    Redshift(Box<RedshiftDbConfig>),
    Snowflake(Box<SnowflakeDbConfig>),
    Postgres(Box<PostgresDbConfig>),
    Bigquery(Box<BigqueryDbConfig>),
    Trino(Box<TrinoDbConfig>),
    Datafusion(Box<DatafusionDbConfig>),
    // SqlServer,
    // SingleStore,
    Spark(Box<SparkDbConfig>),
    Databricks(Box<DatabricksDbConfig>),
    Salesforce(Box<SalesforceDbConfig>),
    DuckDB(Box<DuckDbConfig>),
    /// The dbt lake compute engine.
    LakeCompute(Box<LakeComputeConfig>),
    // Hive,
    Exasol(Box<ExasolDbConfig>),
    // Oracle,
    // Synapse,
    Fabric(Box<FabricDbConfig>),
    // Dremio,
    ClickHouse(Box<ClickHouseDbConfig>),
    // Materialize,
    // Rockset,
    // Firebolt,
    // Teradata,
    // Athena,
    // Vertica,
    // TiDB,
    // #[serde(rename = "glue")]
    // AWSGlue,
    // MindsDB,
    // Greenplum,
    // Impala,
    // #[serde(rename = "layer_bigquery")]
    // LayerBigquery,
    // Iomete,
    // SQLite,
    // MySQL,
    // IBMDB2,
    // AlloyDB,
    // Doris,
    // Infer,
    // Databend,
    // Fal,
    // Decodable,
    // Upsolver,
    // Starrocks,
}

macro_rules! impl_from_db_config {
    ($variant:ident, $config_type:ty) => {
        impl From<$config_type> for DbConfig {
            fn from(config: $config_type) -> Self {
                DbConfig::$variant(Box::new(config))
            }
        }
    };
}

impl_from_db_config!(Redshift, RedshiftDbConfig);
impl_from_db_config!(Snowflake, SnowflakeDbConfig);
impl_from_db_config!(Postgres, PostgresDbConfig);
impl_from_db_config!(Bigquery, BigqueryDbConfig);
impl_from_db_config!(Trino, TrinoDbConfig);
impl_from_db_config!(Datafusion, DatafusionDbConfig);
impl_from_db_config!(Databricks, DatabricksDbConfig);
impl_from_db_config!(DuckDB, DuckDbConfig);
impl_from_db_config!(Fabric, FabricDbConfig);
impl_from_db_config!(Exasol, ExasolDbConfig);
impl_from_db_config!(ClickHouse, ClickHouseDbConfig);

impl DbConfig {
    pub fn get_unique_field(&self) -> Option<&str> {
        match self {
            DbConfig::Snowflake(config) => config.account.as_deref(),
            DbConfig::Postgres(config) => config.host.as_deref(),
            DbConfig::Bigquery(config) => config.database.as_deref(),
            DbConfig::Trino(config) => config.host.as_deref(),
            DbConfig::Datafusion(config) => config.database.as_deref(),
            DbConfig::Redshift(config) => config.host.as_deref(),
            DbConfig::Databricks(config) => config.host.as_deref(),
            DbConfig::Salesforce(config) => config.client_id.as_deref(),
            // DuckDB `path` is optional — attach-only profiles default to `:memory:`.
            DbConfig::DuckDB(config) => Some(config.path.as_deref().unwrap_or(":memory:")),
            DbConfig::LakeCompute(config) => config.base_url.as_deref(),
            DbConfig::Spark(config) => config.host.as_deref(),
            DbConfig::Fabric(config) => config.host.as_deref(),
            DbConfig::Exasol(config) => config.host.as_deref(),
            DbConfig::ClickHouse(config) => config.host.as_deref(),
        }
    }

    pub fn get_adapter_unique_id(&self) -> Option<String> {
        // Generates a hash of a database-specific unique field (eg. hostname on redshift,
        // account on snowflake). Used for telemetry to anonymously identify a data warehouse.
        self.get_unique_field()
            .map(|unique_field| format!("{:x}", md5::compute(unique_field.as_bytes())))
    }

    // XXX: this outdated and it affects the `dbt debug` command. A review is pending.
    //
    // These keys are what `dbt debug` prints as a connection's details, so a key
    // naming a credential leaks it to stdout and to any log that captures it. Since
    // `dbt debug` checks *every* declared adapter, that is now true of every entry
    // in a multi-adapter target, not just the default one.
    pub fn get_connection_keys(&self) -> &'static [&'static str] {
        Self::connection_keys_for(self.adapter_type())
    }

    /// The connection keys for an adapter type, independent of any config
    /// instance, so the credential audit in the tests below can walk every
    /// adapter without having to construct one.
    pub fn connection_keys_for(adapter_type: AdapterType) -> &'static [&'static str] {
        match adapter_type {
            AdapterType::Snowflake => &[
                "account",
                "user",
                "database",
                "warehouse",
                "role",
                "schema",
                "authenticator",
                "oauth_client_id",
                "query_tag",
                "client_session_keep_alive",
                "host",
                "port",
                "proxy_host",
                "proxy_port",
                "protocol",
                "connect_retries",
                "connect_timeout",
                "retry_on_database_errors",
                "retry_all",
                "insecure_mode",
                "reuse_connections",
                "application_name",
            ],
            AdapterType::Postgres => &[
                "host",
                "port",
                "user",
                "database",
                "schema",
                "connect_timeout",
                "role",
                "search_path",
                "keepalives_idle",
                "sslmode",
                "sslcert",
                "sslkey",
                "sslrootcert",
                "application_name",
                "retries",
            ],
            AdapterType::Bigquery => &[
                "method",
                "database",
                "execution_project",
                "schema",
                "api_endpoint",
                "location",
                "priority",
                "maximum_bytes_billed",
                "impersonate_service_account",
                "job_retry_deadline_seconds",
                "job_retries",
                "job_creation_timeout_seconds",
                "job_execution_timeout_seconds",
                "reservation",
                "timeout_seconds",
                "client_id",
                "token_uri",
                "compute_region",
                "dataproc_cluster_name",
                "gcs_bucket",
                "submission_method",
                "dataproc_batch",
            ],
            AdapterType::Redshift => &[
                "host",
                "user",
                "port",
                "database",
                "method",
                "cluster_id",
                "iam_profile",
                "schema",
                "sslmode",
                "region",
                "sslmode",
                "autocreate",
                "db_groups",
                "ra3_node",
                "datasharing",
                "drop_without_cascade",
                "connect_timeout",
                "role",
                "retries",
                "retry_all",
                "autocommit",
                "access_key_id",
                "is_serverless",
                "serverless_work_group",
                "serverless_acct_id",
            ],
            AdapterType::Databricks => &["host", "http_path", "schema"],
            // TODO: Salesforce connection keys
            AdapterType::Salesforce => &["login_url", "database", "data_transform_run_timeout"],
            // `secrets` and `motherduck_token` are deliberately absent: these keys
            // reach `dbt debug`'s connection display, which must not print
            // credentials. See `connection_keys_never_expose_a_credential`.
            AdapterType::DuckDB => &[
                "path",
                "database",
                "schema",
                "extensions",
                "settings",
                "attach",
            ],
            // `token` and `fivetran_credential` are deliberately absent, for the
            // same reason.
            AdapterType::LakeCompute => &[
                "path",
                "database",
                "schema",
                "base_url",
                "method",
                "organization",
                "fivetran_auth_url",
            ],
            // Adapter types with no `DbConfig` variant, so nothing to display.
            AdapterType::Athena
            | AdapterType::Starburst
            | AdapterType::Dremio
            | AdapterType::Oracle => &[],
            // TODO(serramatutu): Spark connection keys
            AdapterType::Spark => &[],
            // TODO: Trino and Datafusion connection keys
            AdapterType::Trino => &[],
            AdapterType::Datafusion => &[],
            AdapterType::Fabric => &[
                "server",
                "database",
                "schema",
                "warehouse_snapshot_name",
                "snapshot_timestamp",
                "UID",
                "workspace_id",
                "authentication",
                "retries",
                "login_timeout",
                "query_timeout",
                "trace_flag",
                "encrypt",
                "trust_cert",
                "api_url",
            ],
            AdapterType::Exasol => &[
                "host",
                "port",
                "user",
                "schema",
                "encryption",
                "certificate_validation",
                "certificate_fingerprint",
                "connection_timeout",
                "query_timeout",
                "idle_timeout",
            ],
            AdapterType::ClickHouse => &[
                "database",
                "schema",
                "driver",
                "host",
                "port",
                "user",
                "cluster",
                "verify",
                "secure",
                "client_cert",
                "client_cert_key",
                "retries",
                "compression",
                "connect_timeout",
                "send_receive_timeout",
                "cluster_mode",
                "use_lw_deletes",
                "check_exchange",
                "local_suffix",
                "local_db_prefix",
                "allow_automatic_deduplication",
                "custom_settings",
                "database_engine",
                "threads",
                "sync_request_timeout",
                "compress_block_size",
            ],
        }
    }

    /// Returns `true` if the profile contained the removed `execute:` field.
    pub fn has_removed_execute_field(&self) -> bool {
        match self {
            DbConfig::Snowflake(config) => config.execute.is_some(),
            DbConfig::Datafusion(config) => config.execute.is_some(),
            _ => false,
        }
    }

    pub fn get_execution_timezone(&self) -> Option<String> {
        match self {
            DbConfig::Snowflake(config) => config.execution_timezone.clone(),
            _ => None,
        }
    }

    pub fn to_yaml_value(&self) -> Result<YmlValue, dbt_yaml::Error> {
        match self {
            DbConfig::Snowflake(config) => dbt_yaml::to_value(config),
            DbConfig::Postgres(config) => dbt_yaml::to_value(config),
            DbConfig::Bigquery(config) => dbt_yaml::to_value(config),
            DbConfig::Trino(config) => dbt_yaml::to_value(config),
            DbConfig::Datafusion(config) => dbt_yaml::to_value(config),
            DbConfig::Redshift(config) => dbt_yaml::to_value(config),
            DbConfig::Databricks(config) => dbt_yaml::to_value(config),
            DbConfig::Salesforce(config) => dbt_yaml::to_value(config),
            DbConfig::Spark(config) => dbt_yaml::to_value(config),
            DbConfig::Fabric(config) => dbt_yaml::to_value(config),
            DbConfig::DuckDB(config) => dbt_yaml::to_value(config),
            DbConfig::LakeCompute(config) => dbt_yaml::to_value(config),
            DbConfig::Exasol(config) => dbt_yaml::to_value(config),
            DbConfig::ClickHouse(config) => dbt_yaml::to_value(config),
        }
    }

    pub fn adapter_type(&self) -> AdapterType {
        match self {
            DbConfig::Redshift(..) => AdapterType::Redshift,
            DbConfig::Snowflake(..) => AdapterType::Snowflake,
            DbConfig::Postgres(..) => AdapterType::Postgres,
            DbConfig::Bigquery(..) => AdapterType::Bigquery,
            DbConfig::Trino(..) => AdapterType::Trino,
            DbConfig::Datafusion(..) => AdapterType::Datafusion,
            DbConfig::Databricks(..) => AdapterType::Databricks,
            DbConfig::Salesforce(..) => AdapterType::Salesforce,
            DbConfig::DuckDB(..) => AdapterType::DuckDB,
            DbConfig::Spark(..) => AdapterType::Spark,
            DbConfig::Fabric(..) => AdapterType::Fabric,
            DbConfig::Exasol(..) => AdapterType::Exasol,
            DbConfig::ClickHouse(..) => AdapterType::ClickHouse,
            DbConfig::LakeCompute(..) => AdapterType::LakeCompute,
        }
    }

    pub fn get_database(&self) -> Option<&String> {
        match self {
            DbConfig::Redshift(config) => config.database.as_ref(),
            DbConfig::Snowflake(config) => config.database.as_ref(),
            DbConfig::Postgres(config) => config.database.as_ref().or(config.database.as_ref()),
            DbConfig::Bigquery(config) => config.database.as_ref(),
            DbConfig::Trino(config) => config.database.as_ref(),
            DbConfig::Datafusion(config) => config.database.as_ref(),
            DbConfig::Databricks(config) => config.database.as_ref(),
            DbConfig::Salesforce(config) => config.database.as_ref(),
            DbConfig::DuckDB(config) => config.database.as_ref(),
            DbConfig::Spark(_) => None,
            DbConfig::Fabric(config) => config.database.as_ref(),
            DbConfig::Exasol(config) => config.database.as_ref(),
            DbConfig::ClickHouse(config) => config.database.as_ref(),
            DbConfig::LakeCompute(config) => config.database.as_ref(),
        }
    }

    /// Returns the database name with adapter-specific defaults when not explicitly configured.
    /// - DuckDB: derived from file path stem (e.g., "jaffle_shop.duckdb" → "jaffle_shop"),
    ///   or "memory" for in-memory (":memory:") or when no path is specified
    /// - Databricks: uses hive_metastore as default catalog
    /// - Others: "dbt" as generic fallback
    pub fn get_database_or_default(&self) -> String {
        // First check if database is explicitly set
        if let Some(db) = self.get_database() {
            return db.clone();
        }

        // Otherwise, use adapter-specific defaults
        match self {
            DbConfig::DuckDB(config) => DuckDBPathInfo::parse_path(config.path.as_deref())
                .database
                .to_owned(),
            DbConfig::Databricks(_) => DEFAULT_DATABRICKS_DATABASE.to_string(),
            _ => "".to_string(),
        }
    }

    pub fn get_schema(&self) -> Option<&String> {
        match self {
            DbConfig::Redshift(config) => config.schema.as_ref(),
            DbConfig::Snowflake(config) => config.schema.as_ref(),
            DbConfig::Postgres(config) => config.schema.as_ref(),
            DbConfig::Trino(config) => config.schema.as_ref(),
            DbConfig::Bigquery(config) => config.schema.as_ref(),
            DbConfig::Datafusion(config) => config.schema.as_ref(),
            DbConfig::Databricks(config) => config.schema.as_ref(),
            DbConfig::Spark(config) => config.schema.as_ref(),
            DbConfig::DuckDB(config) => config.schema.as_ref(),
            DbConfig::LakeCompute(config) => config.schema.as_ref(),
            DbConfig::Salesforce(_) => None,
            DbConfig::Fabric(config) => config.schema.as_ref(),
            DbConfig::Exasol(config) => config.schema.as_ref(),
            DbConfig::ClickHouse(config) => config.schema.as_ref(),
        }
    }

    pub fn get_threads(&self) -> Option<&StringOrInteger> {
        match self {
            DbConfig::Snowflake(config) => config.threads.as_ref(),
            DbConfig::Databricks(config) => config.threads.as_ref(),
            DbConfig::Bigquery(config) => config.threads.as_ref(),
            DbConfig::Redshift(config) => config.threads.as_ref(),
            DbConfig::Postgres(config) => config.threads.as_ref(),
            DbConfig::Trino(config) => config.threads.as_ref(),
            DbConfig::DuckDB(config) => config.threads.as_ref(),
            DbConfig::Datafusion(_) => None,
            DbConfig::Salesforce(_) => None,
            DbConfig::Spark(_) => None,
            DbConfig::Fabric(_) => None,
            DbConfig::Exasol(config) => config.threads.as_ref(),
            DbConfig::ClickHouse(config) => config.threads.as_ref(),
            DbConfig::LakeCompute(config) => config.threads.as_ref(),
        }
    }

    pub fn set_threads(&mut self, threads: Option<StringOrInteger>) {
        match self {
            DbConfig::Snowflake(config) => config.threads = threads,
            DbConfig::Databricks(config) => config.threads = threads,
            DbConfig::Postgres(config) => config.threads = threads,
            DbConfig::Bigquery(config) => config.threads = threads,
            DbConfig::Trino(config) => config.threads = threads,
            DbConfig::Redshift(config) => config.threads = threads,
            DbConfig::DuckDB(config) => config.threads = threads,
            DbConfig::Datafusion(_) => (),
            DbConfig::Salesforce(_) => (),
            DbConfig::Spark(_) => (),
            DbConfig::Fabric(_) => (),
            DbConfig::Exasol(config) => config.threads = threads,
            DbConfig::ClickHouse(config) => config.threads = threads,
            DbConfig::LakeCompute(config) => config.threads = threads,
        }
    }

    pub fn to_connection_mapping(&self) -> Result<dbt_yaml::Mapping, dbt_yaml::Error> {
        let connection_keys = self.get_connection_keys();
        let mapping = self.to_mapping()?;
        let filtered = mapping
            .into_iter()
            .filter(|(key, _)| {
                key.as_str()
                    .map(|s| connection_keys.contains(&s))
                    .unwrap_or(false)
            })
            .collect();
        Ok(filtered)
    }

    pub fn to_mapping(&self) -> Result<dbt_yaml::Mapping, dbt_yaml::Error> {
        let mut mapping = dbt_yaml::Mapping::default();

        // Convert self to YmlValue and return it as a YAML Mapping value
        let mut yml_value = self.to_yaml_value()?;
        let tmp = yml_value.as_mapping_mut().unwrap();
        std::mem::swap(tmp, &mut mapping);

        Ok(mapping)
    }

    pub fn get_aliases(&self) -> Vec<String> {
        // TODO: Implement Aliases for databases that need them. Snowflake does not need aliases.
        vec![]
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Serialize, Deserialize, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub enum Execute {
    #[default]
    Remote,
    Sidecar,
    Service,
}

impl Display for Execute {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Execute::Remote => write!(f, "remote"),
            Execute::Sidecar => write!(f, "sidecar"),
            Execute::Service => write!(f, "service"),
        }
    }
}

impl std::str::FromStr for Execute {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "remote" => Ok(Execute::Remote),
            // "local" and "sidecar" both resolve to Sidecar; "local" is the
            // legacy string that predates the rename and must keep parsing.
            "local" | "sidecar" => Ok(Execute::Sidecar),
            _ => Err(format!("Invalid execute mode: {s}")),
        }
    }
}

impl Execute {
    pub fn is_default(&self) -> bool {
        matches!(self, Execute::Remote)
    }

    /// Map the `--compute` CLI flag to the effective `Execute` mode.
    ///
    /// The execute mode is fully determined by the CLI flag; no profile field
    /// participates. Adapter-specific defaults (e.g. Datafusion auto-promoting
    /// to `Inline`) are handled upstream in `validate_compute_for_adapter`.
    pub fn from_compute_flag(compute_flag: dbt_common::io_args::LocalExecutionBackendKind) -> Self {
        use dbt_common::io_args::LocalExecutionBackendKind;
        match compute_flag {
            LocalExecutionBackendKind::Remote => Execute::Remote,
            LocalExecutionBackendKind::Inline | LocalExecutionBackendKind::Worker => {
                Execute::Sidecar
            }
            LocalExecutionBackendKind::Service => Execute::Service,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct DbTargets {
    #[serde(rename = "target", default = "default_target")]
    pub default_target: DefaultTargetName,
    pub outputs: HashMap<TargetName, YmlValue>,
}

fn default_target() -> String {
    "default".to_string()
}

/// Extend merge_strategies from `merge` crate
mod merge_strategies_extend {
    pub fn overwrite_always<T>(left: &mut T, right: T) {
        *left = right;
    }

    pub fn overwrite_option<T>(left: &mut Option<T>, right: Option<T>) {
        if left.is_none() {
            *left = right;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, DbtSchema, Merge)]
#[serde(rename_all = "snake_case")]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
pub struct RedshiftDbConfig {
    // Configuration Parameters
    pub port: Option<StringOrInteger>, // Setting as Option but required as of dbt 1.7.1
    #[serde(alias = "dbname")] // Same as Postgres, it allows either dbname or database
    pub database: Option<String>, // Setting as Option but required as of dbt 1.7.1
    pub schema: Option<String>,        // Setting as Option but required as of dbt 1.7.1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_timeout: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sslmode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autocreate: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub db_groups: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ra3_node: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datasharing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_without_cascade: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autocommit: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retries: Option<i64>,
    // Authentication Parameters (Password)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    pub host: Option<String>, // Setting as Option but required as of dbt 1.7.1
    pub user: Option<String>, // Setting as Option but required as of dbt 1.7.1
    #[serde(skip_serializing_if = "Option::is_none", alias = "pass")]
    pub password: Option<String>,
    // Authentication Parameters (IAM)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iam_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_access_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_serverless: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serverless_work_group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serverless_acct_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<StringOrInteger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[merge(strategy = merge_strategies_extend::overwrite_always)]
    pub token_endpoint: Option<HashMap<String, YmlValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idc_region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idp_listen_port: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idc_client_display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idp_response_timeout: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct SnowflakeDbConfig {
    // Configuration Parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Route auth through flock-service using dbt Cloud credentials instead of
    /// adapter-specific auth (see dbt-auth's `AdapterConfig::use_dbt_cloud_credentials`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_dbt_cloud_credentials: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_session_keep_alive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<StringOrInteger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_tag: Option<QueryTag>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_all: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_on_database_errors: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_retries: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_timeout: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_timeout: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reuse_connections: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authenticator: Option<String>,
    // Authentication Parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>, // Setting as Option but required as of dbt 1.7.1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>, // Setting as Option but required as of dbt 1.7.1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>, // Setting as Option but required as of dbt 1.7.1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warehouse: Option<String>, // Setting as Option but required as of dbt 1.7.1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_warehouse: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adaptive_metadata_fetch: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>, // Setting as Option but required as of dbt 1.7.1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key_passphrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workload_identity_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workload_identity_entra_resource: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub s3_stage_vpce_dns_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<StringOrInteger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_log_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application_name: Option<String>,
    /// Removed field — kept only so that profiles containing it still parse.
    /// The value is ignored; a deprecation warning is emitted at startup.
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    #[merge(skip)]
    pub execute: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct PostgresDbConfig {
    // Configuration Parameters
    pub port: Option<StringOrInteger>, // Setting as Option but required as of dbt 1.7.1
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "dbname")] // Postgres allows either dbname or database
    pub database: Option<String>,
    pub schema: Option<String>, // Setting as Option but required as of dbt 1.7.1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<StringOrInteger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keepalives_idle: Option<StringOrInteger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retries: Option<StringOrInteger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sslmode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sslcert: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sslkey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sslrootcert: Option<String>,
    // Authentication Parameters (Password)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    pub host: Option<String>, // Setting as Option but required as of dbt 1.7.1
    pub user: Option<String>, // Setting as Option but required as of dbt 1.7.1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct BigqueryDbConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<StringOrInteger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "project")]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "dataset")]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum_bytes_billed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impersonate_service_account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyfile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retries: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyfile_json: Option<StringOrMap>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", alias = "dataproc_region")]
    pub compute_region: Option<String>,
    // TODO: support this https://docs.getdbt.com/docs/core/connect-data-platform/bigquery-setup
    pub dataproc_batch: Option<YmlValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dataproc_cluster_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gcs_bucket: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submission_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_creation_timeout_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_execution_timeout_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reservation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_retries: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_retry_deadline_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workload_pool_provider_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_account_impersonation_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_endpoint: Option<YmlValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct TrinoDbConfig {
    // Configuration Parameters
    pub port: Option<StringOrInteger>, // Setting as Option but required as of dbt 1.7.1
    pub user: Option<String>,          // Setting as Option but required as of dbt 1.7.1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    pub host: Option<String>, // Setting as Option but required as of dbt 1.7.1
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<StringOrInteger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct DatafusionDbConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default, skip_serializing)]
    #[schemars(skip)]
    #[merge(skip)]
    pub execute: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct DatabricksDbConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "catalog", default = "default_databricks_database")]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub azure_client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub azure_client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub azure_tenant_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_redirect_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oauth_scopes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_tags: Option<YmlValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[merge(strategy = merge_strategies_extend::overwrite_always)]
    pub session_properties: Option<HashMap<String, YmlValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[merge(strategy = merge_strategies_extend::overwrite_always)]
    pub connection_parameters: Option<HashMap<String, YmlValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[merge(strategy = merge_strategies_extend::overwrite_always)]
    pub compute: Option<HashMap<String, YmlValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_retries: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_timeout: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_all: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_max_idle: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<StringOrInteger>,
}

fn default_databricks_database() -> Option<String> {
    Some(DEFAULT_DATABRICKS_DATABASE.to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct SalesforceDbConfig {
    /// The method to use to authenticate with Salesforce.
    /// `jwt_bearer`, `username_password`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    // schema is not applicable here
    #[serde(alias = "data_space", default = "default_salesforce_database")]
    pub database: Option<String>,
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key_path: Option<PathBuf>,
    pub login_url: Option<String>,
    pub username: Option<String>,
    #[serde(default = "default_data_transform_run_timeout")]
    pub data_transform_run_timeout: Option<i64>,
}

fn default_salesforce_database() -> Option<String> {
    Some("default".to_string())
}

fn default_data_transform_run_timeout() -> Option<i64> {
    Some(180000) // 3 mins
}

/// A DuckDB secret for the Secrets Manager.
/// Corresponds to upstream dbt-duckdb `Secret` dataclass.
/// Generates a `CREATE OR REPLACE SECRET` statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct DuckDbSecret {
    /// Secret type: s3, azure, gcs, r2, huggingface, etc.
    #[serde(rename = "type")]
    pub secret_type: String,
    /// Optional name. Auto-generated as `__dbt_secret_{index}` if omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional provider (e.g., "credential_chain" for AWS default creds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Optional scope restriction (e.g., "s3://my-bucket").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Whether this is a persistent secret (survives restarts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistent: Option<bool>,
    /// All other key-value params become CREATE SECRET parameters.
    /// E.g., key_id, secret, region, endpoint, account_id, token, etc.
    #[serde(flatten)]
    pub params: HashMap<String, YmlValue>,
}

/// A DuckDB database attachment.
/// Corresponds to upstream dbt-duckdb `Attachment` dataclass.
/// Generates an `ATTACH IF NOT EXISTS` statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct DuckDbAttachment {
    /// Path to the database file or connection string.
    pub path: String,
    /// Alias name for the attached database.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Database type (e.g., "duckdb", "sqlite", "postgres").
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub db_type: Option<String>,
    /// Whether to attach read-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    /// Whether this attachment is a DuckLake database.
    /// Set to true to enable DuckLake-specific query generation (e.g., DROP without CASCADE).
    /// Not needed when the path uses the `ducklake:` scheme, but required for MotherDuck
    /// managed DuckLake where the path uses `md:` instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_ducklake: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct LakeComputeConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Long-lived Fivetran personal access token, for `method: fivetran`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fivetran_credential: Option<String>,
    /// Fivetran public API base URL; only needed to reach a non-production one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fivetran_auth_url: Option<String>,
    /// Days a minted Snowflake PAT stays valid before it needs re-minting.
    /// Only applies when lake compute mints a PAT for Horizon catalog access
    /// (i.e. non-keypair auth on the native Snowflake target). Default: 30.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snowflake_pat_duration_days: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<StringOrInteger>,
}

/// DuckDB adapter configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct DuckDbConfig {
    /// Path to the DuckDB database file. Defaults to in-memory (:memory:)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Database name (defaults to the name DuckDB derives from `path`)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    /// Schema name (defaults to "main")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Extensions to load (e.g., ["httpfs", "parquet"] or [{name: "duckpgq", repo: "community"}])
    #[serde(skip_serializing_if = "Option::is_none")]
    #[merge(strategy = merge_strategies_extend::overwrite_always)]
    pub extensions: Option<Vec<DuckDbExtension>>,
    /// DuckDB configuration settings (e.g., {"memory_limit": "4GB"})
    #[serde(skip_serializing_if = "Option::is_none")]
    #[merge(strategy = merge_strategies_extend::overwrite_always)]
    pub settings: Option<HashMap<String, YmlValue>>,
    /// Number of threads
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<StringOrInteger>,
    /// Secrets to register with DuckDB's Secrets Manager.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[merge(strategy = merge_strategies_extend::overwrite_always)]
    pub secrets: Option<Vec<DuckDbSecret>>,
    /// External databases to attach.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[merge(strategy = merge_strategies_extend::overwrite_always)]
    pub attach: Option<Vec<DuckDbAttachment>>,
    /// Whether the primary database is a DuckLake database.
    /// Set to true to enable DuckLake-specific query generation (e.g., DROP without CASCADE).
    /// Not needed when the path uses the `ducklake:` scheme, but required for MotherDuck
    /// managed DuckLake where the path uses `md:` instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_ducklake: Option<bool>,
    /// Root path for external materializations (defaults to ".")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_root: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub okta_auth_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub okta_token_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub okta_client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, DbtSchema)]
pub enum SparkMethod {
    #[serde(rename = "thrift")]
    ThriftBinary,
    #[serde(rename = "http")]
    ThriftHttp,
    #[serde(rename = "livy")]
    Livy,
    #[serde(rename = "spark-connect")]
    SparkConnect,
    // TODO: EMR StartJob (?)
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Serialize, Deserialize, DbtSchema)]
pub enum SparkAuth {
    #[serde(rename = "NOSASL")]
    NoSasl,
    #[serde(rename = "CUSTOM")]
    SaslCustom,
    #[serde(rename = "KERBEROS")]
    SaslKerberos,
    #[serde(rename = "LDAP")]
    SaslLdap,
    #[default]
    #[serde(rename = "NONE")]
    SaslNone,
    #[serde(rename = "PLAIN")]
    SaslPlain,
    #[serde(rename = "BASIC")]
    Basic,
    #[serde(rename = "TOKEN")]
    Token,
    #[serde(rename = "AWS_SIGV4")]
    AwsSigV4,
}

impl Display for SparkAuth {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NoSasl => write!(f, "NOSASL"),
            Self::SaslCustom => write!(f, "CUSTOM"),
            Self::SaslKerberos => write!(f, "KERBEROS"),
            Self::SaslLdap => write!(f, "LDAP"),
            Self::SaslNone => write!(f, "NONE"),
            Self::SaslPlain => write!(f, "PLAIN"),
            Self::Basic => write!(f, "BASIC"),
            Self::AwsSigV4 => write!(f, "AWS_SIGV4"),
            Self::Token => write!(f, "TOKEN"),
        }
    }
}

impl std::str::FromStr for SparkAuth {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "CUSTOM" => Ok(Self::SaslCustom),
            "KERBEROS" => Ok(Self::SaslKerberos),
            "LDAP" => Ok(Self::SaslLdap),
            "NONE" => Ok(Self::SaslNone),
            "NOSASL" => Ok(Self::NoSasl),
            "PLAIN" => Ok(Self::SaslPlain),
            "BASIC" => Ok(Self::Basic),
            "TOKEN" => Ok(Self::Token),
            "AWS_SIGV4" => Ok(Self::AwsSigV4),
            _ => Err(format!("Invalid Spark auth mode: {s}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct SparkDbConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<SparkAuth>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_ssl: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<SparkMethod>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", alias = "token")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kerberos_service_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", alias = "conf")]
    #[merge(strategy = merge_strategies_extend::overwrite_always)]
    pub server_side_parameters: Option<HashMap<String, YmlValue>>,
    // TODO: python supports some extra properties:
    // - cluster
    // - connect_retries
    // - connect_timeout
    // - connection_string_suffix
    // - database (same as schema)
    // - driver
    // - endpoint
    // - organization
    // - poll_interval
    // - query_retries
    // - query_timeout
    // - retry_all
    // - token
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct FabricDbConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>, // TODO: this seems to be ODBC related... keep it?
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "server")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "UID", alias = "user", alias = "username")]
    pub user: Option<String>, // default = None
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "PWD", alias = "pass", alias = "password")]
    pub password: Option<String>, // default = None
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "trusted_connection")]
    pub windows_login: Option<bool>, // default = False
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "SQL_ATTR_TRACE")]
    pub trace_flag: Option<bool>, // default = False
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>, // default = None
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "app_id")]
    pub client_id: Option<String>, // default = None
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "app_secret")]
    pub client_secret: Option<String>, // default = None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>, // default = None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token_expires_on: Option<String>, // default = 0 | Added for access token expiration for oAuth and integration tests scenarios.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "auth")]
    pub authentication: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypt: Option<bool>, // default = True  | default value in MS ODBC Driver 18 as well
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "TrustServerCertificate")]
    pub trust_cert: Option<bool>, // default = False  | default value in MS ODBC Driver 18 as well
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retries: Option<i64>, // default = 3
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(alias = "schema_auth")]
    pub schema_authorization: Option<String>, // default = None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_timeout: Option<i64>, // default = 0
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_timeout: Option<i64>, // default = 0
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>, // default = None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warehouse_snapshot_name: Option<String>, // default = None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warehouse_snapshot_id: Option<String>, // default = None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_timestamp: Option<String>, // default = None
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>, // default = "https://api.fabric.microsoft.com/v1"
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct ExasolDbConfig {
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", alias = "pass")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<StringOrInteger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encryption: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate_validation: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_timeout: Option<StringOrInteger>,
    /// Per-statement execution timeout in seconds (0 = no limit)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_timeout: Option<StringOrInteger>,
    /// Idle connection timeout in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_timeout: Option<StringOrInteger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<StringOrInteger>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct ClickHouseDbConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_schema"
    )]
    pub schema: Option<String>,

    /// Auto-detected from `port` and `secure` when not set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_host"
    )]
    pub host: Option<String>,

    /// Auto-detected from `driver` and `secure` when not set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<StringOrInteger>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_user"
    )]
    pub user: Option<String>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        alias = "pass",
        default = "default_clickhouse_empty_string"
    )]
    pub password: Option<String>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_empty_string"
    )]
    pub cluster: Option<String>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_true"
    )]
    pub verify: Option<bool>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_false"
    )]
    pub secure: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_cert_key: Option<String>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_retries"
    )]
    pub retries: Option<i64>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_empty_string"
    )]
    pub compression: Option<String>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_connect_timeout"
    )]
    pub connect_timeout: Option<i64>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_send_receive_timeout"
    )]
    pub send_receive_timeout: Option<i64>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_false"
    )]
    pub cluster_mode: Option<bool>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_false"
    )]
    pub use_lw_deletes: Option<bool>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_true"
    )]
    pub check_exchange: Option<bool>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_local_suffix"
    )]
    pub local_suffix: Option<String>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_empty_string"
    )]
    pub local_db_prefix: Option<String>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_false"
    )]
    pub allow_automatic_deduplication: Option<bool>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_custom_settings"
    )]
    #[merge(strategy = merge_strategies_extend::overwrite_always)]
    pub custom_settings: Option<HashMap<String, YmlValue>>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_empty_string"
    )]
    pub database_engine: Option<String>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_threads"
    )]
    pub threads: Option<StringOrInteger>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_sync_request_timeout"
    )]
    pub sync_request_timeout: Option<i64>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default = "default_clickhouse_compress_block_size"
    )]
    pub compress_block_size: Option<i64>,
}

fn default_clickhouse_schema() -> Option<String> {
    Some("default".to_string())
}

fn default_clickhouse_host() -> Option<String> {
    Some("localhost".to_string())
}

fn default_clickhouse_user() -> Option<String> {
    Some("default".to_string())
}

fn default_clickhouse_empty_string() -> Option<String> {
    Some(String::new())
}

fn default_clickhouse_true() -> Option<bool> {
    Some(true)
}

fn default_clickhouse_false() -> Option<bool> {
    Some(false)
}

fn default_clickhouse_retries() -> Option<i64> {
    Some(1)
}

fn default_clickhouse_connect_timeout() -> Option<i64> {
    Some(10)
}

fn default_clickhouse_send_receive_timeout() -> Option<i64> {
    Some(300)
}

fn default_clickhouse_local_suffix() -> Option<String> {
    Some("_local".to_string())
}

fn default_clickhouse_custom_settings() -> Option<HashMap<String, YmlValue>> {
    Some(HashMap::new())
}

fn default_clickhouse_threads() -> Option<StringOrInteger> {
    Some(StringOrInteger::Integer(1))
}

fn default_clickhouse_sync_request_timeout() -> Option<i64> {
    Some(5)
}

fn default_clickhouse_compress_block_size() -> Option<i64> {
    Some(1_048_576)
}

#[derive(Serialize, DbtSchema)]
#[serde(untagged)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum TargetContext {
    Snowflake(SnowflakeTargetEnv),
    Trino(TrinoTargetEnv),
    Datafusion(DatafusionTargetEnv),
    Postgres(PostgresTargetEnv),
    Bigquery(BigqueryTargetEnv),
    Databricks(DatabricksTargetEnv),
    Redshift(RedshiftTargetEnv),
    Salesforce(SalesforceTargetEnv),
    DuckDB(DuckDbTargetEnv),
    Spark(SparkTargetEnv),
    Fabric(FabricTargetEnv),
    Exasol(ExasolTargetEnv),
    ClickHouse(ClickHouseTargetEnv),
    // Add other variants as needed
}

#[derive(Serialize, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct TrinoTargetEnv {
    pub __common__: CommonTargetContext,
}

#[derive(Serialize, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct DatafusionTargetEnv {
    pub database: String,
    pub __common__: CommonTargetContext,
}

#[derive(Serialize, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct PostgresTargetEnv {
    pub dbname: String,
    pub host: String,
    pub user: String,
    pub port: StringOrInteger,
    pub __common__: CommonTargetContext,
}

#[derive(Serialize, DbtSchema)]
pub struct SnowflakeTargetEnv {
    pub account: String,
    pub user: Option<String>,
    pub warehouse: Option<String>,
    pub metadata_warehouse: Option<String>,
    pub adaptive_metadata_fetch: Option<bool>,
    pub role: Option<String>,
    pub authenticator: Option<String>,
    pub oauth_client_id: Option<String>,
    pub workload_identity_provider: Option<String>,
    pub workload_identity_entra_resource: Option<String>,
    pub query_tag: Option<QueryTag>,
    pub client_session_keep_alive: bool, // Default: false
    pub host: Option<String>,
    pub port: Option<StringOrInteger>,
    pub protocol: Option<String>,
    pub proxy_host: Option<String>,
    pub proxy_port: Option<StringOrInteger>,
    pub insecure_mode: bool,  // Default: false
    pub connect_retries: i64, // Default: 1
    pub connect_timeout: Option<i64>,
    pub request_timeout: Option<i64>,
    pub retry_on_database_errors: bool, // Default: false
    pub retry_all: bool,                // Default: false
    pub reuse_connections: Option<bool>,
    pub s3_stage_vpce_dns_name: Option<String>,
    pub __common__: CommonTargetContext,
}

#[derive(Serialize, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct BigqueryTargetEnv {
    pub project: String,
    pub dataset: String,
    pub client_id: Option<String>,
    pub compute_region: Option<String>,
    pub dataproc_batch: Option<YmlValue>,
    pub dataproc_cluster_name: Option<String>,
    pub dataproc_region: Option<String>,
    pub execution_project: Option<String>,
    pub gcs_bucket: Option<String>,
    pub impersonate_service_account: Option<String>,
    pub job_creation_timeout_seconds: Option<i64>,
    pub job_execution_timeout_seconds: Option<i64>,
    pub reservation: Option<String>,
    pub job_retries: Option<i64>,
    pub job_retry_deadline_seconds: Option<i64>,
    pub location: Option<String>,
    pub maximum_bytes_billed: Option<i64>,
    pub method: Option<String>,
    pub priority: Option<String>,
    pub quota_project: Option<String>,
    pub retries: Option<i64>,
    pub target_name: Option<String>,
    pub timeout_seconds: Option<i64>,
    pub token_uri: Option<String>,
    pub __common__: CommonTargetContext,
}

#[derive(Serialize, DbtSchema)]
pub struct CommonTargetContext {
    pub database: String,
    pub schema: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub threads: Option<u16>,
}

#[derive(Serialize, DbtSchema)]
pub struct DatabricksTargetEnv {
    pub host: Option<String>,
    pub http_path: Option<String>,
    pub catalog: Option<String>,
    pub __common__: CommonTargetContext,
}

#[derive(Serialize, DbtSchema)]
pub struct RedshiftTokenEndpoint {
    pub r#type: String,
    pub request_url: String,
    pub idp_auth_credentials: Option<String>,
    pub request_data: String,
}

#[derive(Serialize, DbtSchema)]
pub struct RedshiftTargetEnv {
    pub host: String,
    pub user: Option<String>,
    pub port: StringOrInteger,
    pub method: Option<String>,
    pub cluster_id: Option<String>,
    pub iam_profile: Option<String>,
    pub sslmode: Option<String>,
    pub region: Option<String>,
    pub autocreate: bool,
    pub db_groups: Option<Vec<String>>,
    pub ra3_node: Option<bool>,
    pub datasharing: Option<bool>,
    pub drop_without_cascade: Option<bool>,
    pub connect_timeout: Option<i64>,
    pub role: Option<String>,
    pub retries: i64,
    pub autocommit: Option<bool>,
    pub dbname: String,
    pub __common__: CommonTargetContext,
    pub retry_all: bool,
    pub access_key_id: Option<String>,
    pub is_serverless: Option<bool>,
    pub serverless_work_group: Option<String>,
    pub serverless_acct_id: Option<String>,
    pub token_endpoint: Option<HashMap<String, YmlValue>>,
    pub idc_region: Option<String>,
    pub issuer_url: Option<String>,
    pub idp_listen_port: Option<i64>,
    pub idc_client_display_name: Option<String>,
    pub idp_response_timeout: Option<i64>,
}

#[derive(Serialize, DbtSchema)]
pub struct SalesforceTargetEnv {
    pub __common__: CommonTargetContext,
}

#[derive(Serialize, DbtSchema)]
pub struct DuckDbTargetEnv {
    pub path: Option<String>,
    pub __common__: CommonTargetContext,
}

#[derive(Serialize, DbtSchema)]
pub struct SparkTargetEnv {
    pub __common__: CommonTargetContext,
    pub method: SparkMethod,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub auth: SparkAuth,
    pub use_ssl: bool,
    pub kerberos_service_name: String,
}

#[derive(Serialize, DbtSchema)]
pub struct FabricTargetEnv {
    pub __common__: CommonTargetContext,
    pub authentication: String,
    // TODO: ...
}

#[derive(Serialize, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct ExasolTargetEnv {
    pub host: Option<String>,
    pub user: Option<String>,
    pub __common__: CommonTargetContext,
}

#[derive(Serialize, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct ClickHouseTargetEnv {
    pub driver: Option<String>,
    pub host: Option<String>,
    pub port: Option<StringOrInteger>,
    pub user: Option<String>,
    pub retries: Option<i64>,
    pub cluster: Option<String>,
    pub database_engine: Option<String>,
    pub cluster_mode: Option<bool>,
    pub secure: Option<bool>,
    pub verify: Option<bool>,
    pub connect_timeout: Option<i64>,
    pub send_receive_timeout: Option<i64>,
    pub sync_request_timeout: Option<i64>,
    pub compress_block_size: Option<i64>,
    pub compression: Option<String>,
    pub check_exchange: Option<bool>,
    pub custom_settings: Option<HashMap<String, YmlValue>>,
    pub use_lw_deletes: Option<bool>,
    pub allow_automatic_deduplication: Option<bool>,
    pub local_suffix: Option<String>,
    pub local_db_prefix: Option<String>,
    pub __common__: CommonTargetContext,
}

/// The location type of a DuckDB database path.
#[derive(Debug, Clone, PartialEq)]
pub enum DuckDBLocation<'a> {
    /// In-memory database (`:memory:` or no path).
    Memory,
    /// MotherDuck cloud database (`md:` or `motherduck:` prefix).
    Motherduck { url: &'a str },
    /// Local filesystem database file.
    Filesystem { path: &'a str },
}

/// Parsed information about a DuckDB connection path.
#[derive(Debug, Clone, PartialEq)]
pub struct DuckDBPathInfo<'a> {
    pub location: DuckDBLocation<'a>,
    /// The effective database name derived from the path.
    pub database: &'a str,
    /// Whether the path uses the `ducklake:` scheme.
    pub is_ducklake: bool,
}

impl<'a> DuckDBPathInfo<'a> {
    /// Parse a DuckDB connection path into its component parts.
    ///
    /// Handles a single `ducklake:` prefix, MotherDuck (`md:` / `motherduck:`)
    /// URLs, `:memory:`, and filesystem paths.
    pub fn parse_path(path: Option<&'a str>) -> Self {
        let Some(effective) = path else {
            return Self {
                location: DuckDBLocation::Memory,
                database: "memory",
                is_ducklake: false,
            };
        };

        // Strip a single `ducklake:` prefix.
        let (is_ducklake, effective) = match effective.strip_prefix("ducklake:") {
            Some(inner) => (true, inner),
            None => (false, effective),
        };

        if effective == ":memory:" || effective.is_empty() {
            return Self {
                location: DuckDBLocation::Memory,
                database: "memory",
                is_ducklake,
            };
        }

        // Check for MotherDuck prefix.
        let lower = effective.to_lowercase();
        let md_prefix_len = if lower.starts_with("motherduck:") {
            Some("motherduck:".len())
        } else if lower.starts_with("md:") {
            Some("md:".len())
        } else {
            None
        };

        if let Some(prefix_len) = md_prefix_len {
            let stripped = &effective[prefix_len..];
            let name = stripped.split('?').next().unwrap_or("");
            let database = if name.is_empty() { "my_db" } else { name };
            return Self {
                location: DuckDBLocation::Motherduck { url: effective },
                database,
                is_ducklake,
            };
        }

        // Filesystem path — derive database name from file stem.
        let database = std::path::Path::new(effective)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("main");
        Self {
            location: DuckDBLocation::Filesystem { path: effective },
            database,
            is_ducklake,
        }
    }
}

fn missing(field: &str) -> String {
    format!("In file `profiles.yml`, field `{field}` is required.")
}

// This target context is only to be used in rendering yml's
// See: https://docs.getdbt.com/reference/dbt-jinja-functions/target
impl TryFrom<DbConfig> for TargetContext {
    type Error = String;

    fn try_from(db_config: DbConfig) -> Result<Self, Self::Error> {
        let adapter_type = db_config.adapter_type().to_string();
        match db_config {
            // Snowflake case
            DbConfig::Snowflake(config) => {
                let database = config.database.ok_or_else(|| missing("database"))?;
                Ok(TargetContext::Snowflake(SnowflakeTargetEnv {
                    account: config.account.ok_or_else(|| missing("account"))?,
                    user: config.user,
                    warehouse: config.warehouse,
                    metadata_warehouse: config.metadata_warehouse,
                    adaptive_metadata_fetch: config.adaptive_metadata_fetch,
                    role: config.role.clone(),
                    authenticator: config.authenticator,
                    oauth_client_id: config.oauth_client_id,
                    workload_identity_provider: config.workload_identity_provider,
                    workload_identity_entra_resource: config.workload_identity_entra_resource,
                    query_tag: config.query_tag,
                    client_session_keep_alive: config.client_session_keep_alive.unwrap_or(false),
                    host: config.host,
                    port: config.port,
                    protocol: config.protocol,
                    connect_retries: config.connect_retries.unwrap_or(1),
                    connect_timeout: config.connect_timeout,
                    request_timeout: config.request_timeout,
                    retry_on_database_errors: config.retry_on_database_errors.unwrap_or(false),
                    retry_all: config.retry_all.unwrap_or(false),
                    reuse_connections: config.reuse_connections,
                    s3_stage_vpce_dns_name: config.s3_stage_vpce_dns_name,
                    __common__: CommonTargetContext {
                        database,
                        schema: config.schema.ok_or_else(|| missing("schema"))?,
                        type_: adapter_type,
                        threads: match config.threads {
                            Some(StringOrInteger::String(threads)) => {
                                Some(threads.parse::<u16>().map_err(|_| {
                                    "threads must be a positive integer".to_string()
                                })?)
                            }
                            Some(StringOrInteger::Integer(threads)) => Some(threads as u16),
                            None => None,
                        },
                    },
                    proxy_host: None,
                    proxy_port: None,
                    insecure_mode: false,
                }))
            }

            // Trino case
            DbConfig::Trino(config) => {
                let database = config.database.ok_or_else(|| missing("database"))?;
                Ok(TargetContext::Trino(TrinoTargetEnv {
                    __common__: CommonTargetContext {
                        database,
                        schema: config.schema.ok_or_else(|| missing("schema"))?,
                        type_: adapter_type,
                        threads: match config.threads {
                            Some(StringOrInteger::String(threads)) => {
                                Some(threads.parse::<u16>().map_err(|_| {
                                    "threads must be a positive integer".to_string()
                                })?)
                            }
                            Some(StringOrInteger::Integer(threads)) => Some(threads as u16),
                            None => None,
                        },
                    },
                }))
            }

            // Datafusion case
            DbConfig::Datafusion(config) => {
                let database = config.database.ok_or_else(|| missing("database"))?;
                Ok(TargetContext::Datafusion(DatafusionTargetEnv {
                    database: database.clone(),
                    __common__: CommonTargetContext {
                        database,
                        schema: config.schema.ok_or_else(|| missing("schema"))?,
                        type_: adapter_type,
                        threads: None, // Assuming Datafusion does not have threads configuration
                    },
                }))
            }

            DbConfig::Postgres(config) => {
                let database = config
                    .database
                    .ok_or_else(|| missing("dbname or database"))?;
                Ok(TargetContext::Postgres(PostgresTargetEnv {
                    dbname: database.clone(),
                    host: config.host.ok_or_else(|| missing("host"))?,
                    user: config.user.ok_or_else(|| missing("user"))?,
                    port: config.port.ok_or_else(|| missing("port"))?,
                    __common__: CommonTargetContext {
                        database,
                        schema: config.schema.ok_or_else(|| missing("schema"))?,
                        type_: adapter_type,
                        threads: None,
                    },
                }))
            }

            // Bigquery case
            DbConfig::Bigquery(config) => {
                let database = config
                    .database
                    .ok_or_else(|| missing("database or project"))?;
                let schema = config.schema.ok_or_else(|| missing("schema or dataset"))?;
                Ok(TargetContext::Bigquery(BigqueryTargetEnv {
                    project: database.clone(),
                    dataset: schema.clone(),
                    __common__: CommonTargetContext {
                        database,
                        schema,
                        type_: adapter_type,
                        threads: None,
                    },
                    client_id: config.client_id.clone(),
                    compute_region: config.compute_region.clone(),
                    dataproc_batch: config.dataproc_batch.clone(),
                    dataproc_cluster_name: config.dataproc_cluster_name.clone(),
                    dataproc_region: config.compute_region.clone(),
                    execution_project: config.execution_project.clone(),
                    gcs_bucket: config.gcs_bucket.clone(),
                    impersonate_service_account: config.impersonate_service_account.clone(),
                    job_creation_timeout_seconds: config.job_creation_timeout_seconds,
                    job_execution_timeout_seconds: config.job_execution_timeout_seconds,
                    reservation: config.reservation.clone(),
                    job_retries: config.job_retries,
                    job_retry_deadline_seconds: config.job_retry_deadline_seconds,
                    location: config.location.clone(),
                    maximum_bytes_billed: config.maximum_bytes_billed,
                    method: config.method.clone(),
                    priority: config.priority.clone(),
                    quota_project: config.quota_project.clone(),
                    retries: config.retries,
                    target_name: config.target_name.clone(),
                    timeout_seconds: config.timeout_seconds,
                    token_uri: config.token_uri,
                }))
            }

            DbConfig::Databricks(config) => {
                let database = config
                    .database
                    .unwrap_or_else(|| DEFAULT_DATABRICKS_DATABASE.to_string());
                Ok(TargetContext::Databricks(DatabricksTargetEnv {
                    host: config.host,
                    http_path: config.http_path,
                    catalog: Some(database.clone()),
                    __common__: CommonTargetContext {
                        database,
                        schema: config.schema.ok_or_else(|| missing("schema"))?,
                        type_: adapter_type,
                        threads: None,
                    },
                }))
            }

            DbConfig::Redshift(config) => {
                let database = config
                    .database
                    .ok_or_else(|| missing("dbname or database"))?;
                Ok(TargetContext::Redshift(RedshiftTargetEnv {
                    host: config.host.ok_or_else(|| missing("host"))?,
                    user: config.user,
                    port: config.port.ok_or_else(|| missing("port"))?,
                    method: config.method,
                    cluster_id: config.cluster_id,
                    iam_profile: config.iam_profile,
                    sslmode: config.sslmode,
                    region: config.region,
                    autocreate: config.autocreate.unwrap_or(false),
                    db_groups: config.db_groups,
                    ra3_node: config.ra3_node,
                    datasharing: config.datasharing,
                    drop_without_cascade: config.drop_without_cascade,
                    connect_timeout: config.connect_timeout,
                    role: config.role,
                    retries: config.retries.unwrap_or(1),
                    autocommit: config.autocommit,
                    dbname: database.clone(),
                    __common__: CommonTargetContext {
                        database,
                        schema: config.schema.ok_or_else(|| missing("schema"))?,
                        type_: adapter_type,
                        threads: None,
                    },
                    retry_all: false,
                    access_key_id: config.access_key_id,
                    is_serverless: config.is_serverless,
                    serverless_work_group: config.serverless_work_group,
                    serverless_acct_id: config.serverless_acct_id,
                    token_endpoint: config.token_endpoint,
                    idc_region: config.idc_region,
                    idc_client_display_name: config.idc_client_display_name,
                    issuer_url: config.issuer_url,
                    idp_listen_port: config.idp_listen_port,
                    idp_response_timeout: config.idp_response_timeout,
                }))
            }

            DbConfig::Salesforce(config) => Ok(TargetContext::Salesforce(SalesforceTargetEnv {
                __common__: CommonTargetContext {
                    database: config.database.ok_or_else(|| missing("database"))?,
                    // `SalesforceDbConfig` doesn't have `schema`
                    schema: "".to_string(),
                    type_: adapter_type,
                    threads: None,
                },
            })),

            DbConfig::DuckDB(config) => Ok(TargetContext::DuckDB(DuckDbTargetEnv {
                path: config.path.clone(),
                __common__: CommonTargetContext {
                    // Derive database name from path if not explicitly set (same logic as get_database())
                    database: config.database.clone().unwrap_or_else(|| {
                        DuckDBPathInfo::parse_path(config.path.as_deref())
                            .database
                            .to_owned()
                    }),
                    schema: config.schema.unwrap_or_else(|| "main".to_string()),
                    type_: adapter_type,
                    threads: match config.threads {
                        Some(StringOrInteger::String(threads)) => Some(
                            threads
                                .parse::<u16>()
                                .map_err(|_| "threads must be a positive integer".to_string())?,
                        ),
                        Some(StringOrInteger::Integer(threads)) => Some(threads as u16),
                        None => None,
                    },
                },
            })),

            DbConfig::LakeCompute(config) => {
                Ok(TargetContext::DuckDB(DuckDbTargetEnv {
                    path: config.path.clone(),
                    __common__: CommonTargetContext {
                        // Derive database name from path if not explicitly set (same logic as get_database())
                        database: config.database.clone().unwrap_or_else(|| {
                            DuckDBPathInfo::parse_path(config.path.as_deref())
                                .database
                                .to_owned()
                        }),
                        schema: config.schema.unwrap_or_else(|| "main".to_string()),
                        type_: adapter_type,
                        threads: match config.threads {
                            Some(StringOrInteger::String(threads)) => {
                                Some(threads.parse::<u16>().map_err(|_| {
                                    "threads must be a positive integer".to_string()
                                })?)
                            }
                            Some(StringOrInteger::Integer(threads)) => Some(threads as u16),
                            None => None,
                        },
                    },
                }))
            }

            DbConfig::Spark(config) => Ok(TargetContext::Spark(SparkTargetEnv {
                method: config.method.ok_or_else(|| missing("method"))?,
                host: config.host.ok_or_else(|| missing("host"))?,
                port: config.port.unwrap_or(10000),
                user: config.user.unwrap_or_default(),
                password: config.password.unwrap_or_default(),

                kerberos_service_name: config.kerberos_service_name.unwrap_or_default(),
                use_ssl: config.use_ssl.unwrap_or(false),
                auth: config.auth.unwrap_or_default(),

                __common__: CommonTargetContext {
                    database: "".to_string(),
                    schema: config.schema.ok_or_else(|| missing("schema"))?,
                    type_: adapter_type,
                    threads: None,
                },
            })),
            DbConfig::Fabric(config) => {
                let common = CommonTargetContext {
                    database: config.database.ok_or_else(|| missing("database"))?,
                    schema: config.schema.ok_or_else(|| missing("schema"))?,
                    type_: adapter_type,
                    threads: None,
                };

                let authentication = config.authentication.unwrap_or_default();

                Ok(TargetContext::Fabric(FabricTargetEnv {
                    __common__: common,
                    authentication,
                }))
            }
            DbConfig::Exasol(config) => Ok(TargetContext::Exasol(ExasolTargetEnv {
                host: config.host.clone(),
                user: config.user.clone(),
                __common__: CommonTargetContext {
                    database: config
                        .database
                        .clone()
                        .unwrap_or_else(|| "exasol".to_string()),
                    schema: config.schema.ok_or_else(|| missing("schema"))?,
                    type_: adapter_type,
                    threads: None,
                },
            })),

            DbConfig::ClickHouse(config) => Ok(TargetContext::ClickHouse(ClickHouseTargetEnv {
                driver: config.driver.clone(),
                host: config.host.clone(),
                port: config.port.clone(),
                user: config.user.clone(),
                retries: config.retries,
                cluster: config.cluster.clone(),
                database_engine: config.database_engine.clone(),
                cluster_mode: config.cluster_mode,
                secure: config.secure,
                verify: config.verify,
                connect_timeout: config.connect_timeout,
                send_receive_timeout: config.send_receive_timeout,
                sync_request_timeout: config.sync_request_timeout,
                compress_block_size: config.compress_block_size,
                compression: config.compression.clone(),
                check_exchange: config.check_exchange,
                custom_settings: config.custom_settings.clone(),
                use_lw_deletes: config.use_lw_deletes,
                allow_automatic_deduplication: config.allow_automatic_deduplication,
                local_suffix: config.local_suffix.clone(),
                local_db_prefix: config.local_db_prefix.clone(),
                __common__: CommonTargetContext {
                    database: String::new(),
                    schema: config.schema.clone().ok_or_else(|| missing("schema"))?,
                    type_: adapter_type,
                    threads: match config.threads {
                        Some(StringOrInteger::String(threads)) => Some(
                            threads
                                .parse::<u16>()
                                .map_err(|_| "threads must be a positive integer".to_string())?,
                        ),
                        Some(StringOrInteger::Integer(threads)) => Some(threads as u16),
                        None => None,
                    },
                },
            })),
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    /// `DbConfig` is `#[serde(tag = "type", rename_all = "lowercase")]`, so the
    /// tag is the variant identifier lowercased. `dbt-profile` hard-codes the
    /// resulting string (`LAKE_COMPUTE_TYPE`) because it deliberately does not
    /// depend on this crate. Neither can notice a rename of the variant, so pin
    /// the tag here and round trip through it.
    #[test]
    fn lake_compute_is_tagged_by_its_lowercased_identifier() {
        let value = dbt_yaml::to_value(DbConfig::LakeCompute(Box::default()))
            .expect("lake compute config should serialize");
        assert_eq!(
            value
                .get(dbt_yaml::Value::from("type"))
                .and_then(|v| v.as_str()),
            Some("lakecompute"),
            "`dbt_profile::adapters::LAKE_COMPUTE_TYPE` hard-codes this string"
        );

        let round_tripped: DbConfig =
            dbt_yaml::from_value(value).expect("the tag it emits must be the tag it accepts");
        assert!(matches!(round_tripped, DbConfig::LakeCompute(_)));

        // The adapter's retired external name is not a tag this enum accepts
        // either, same as any other unrecognized `type:`.
        assert!(
            dbt_yaml::from_str::<DbConfig>(
                "type: lake_compute\nbase_url: https://example.invalid\n"
            )
            .is_err(),
        );
    }

    #[test]
    fn test_databricks_profile_deserializes_query_tags_from_both_profile_shapes() {
        let config: DbConfig = dbt_yaml::from_str(
            r#"
type: databricks
query_tags: '{"team":"analytics"}'
"#,
        )
        .unwrap();
        let DbConfig::Databricks(config) = config else {
            panic!("Expected Databricks config");
        };

        assert_eq!(
            config.query_tags.as_ref().and_then(YmlValue::as_str),
            Some(r#"{"team":"analytics"}"#)
        );

        let config: DbConfig = dbt_yaml::from_str(
            r#"
type: databricks
query_tags:
  team: analytics
  cost_center: "3000"
"#,
        )
        .unwrap();
        let DbConfig::Databricks(config) = config else {
            panic!("Expected Databricks config");
        };

        let query_tags = config.query_tags.as_ref().unwrap();
        assert_eq!(
            query_tags.get("team").and_then(YmlValue::as_str),
            Some("analytics")
        );
        assert_eq!(
            query_tags.get("cost_center").and_then(YmlValue::as_str),
            Some("3000")
        );
    }

    #[test]
    fn test_snowflake_adapter_unique_id() {
        let config: DbConfig = SnowflakeDbConfig {
            account: Some("kw27752".to_string()),
            ..Default::default()
        }
        .into();

        assert_eq!(config.get_unique_field(), Some("kw27752"));
        assert_eq!(
            config.get_adapter_unique_id(),
            Some("c27a9a57d35df4a8f81aec929cbdc7cd".to_string())
        );
    }

    #[test]
    fn test_snowflake_adapter_unique_id_with_missing_account() {
        let config: DbConfig = SnowflakeDbConfig {
            account: None,
            ..Default::default()
        }
        .into();

        assert_eq!(config.get_unique_field(), None);
        assert_eq!(config.get_adapter_unique_id(), None);
    }

    #[test]
    fn test_duckdb_adapter_unique_id_defaults_to_memory_when_path_missing() {
        let config: DbConfig = DuckDbConfig {
            path: None,
            attach: Some(vec![DuckDbAttachment {
                path: "md:some_db".to_string(),
                is_ducklake: Some(true),
                ..Default::default()
            }]),
            ..Default::default()
        }
        .into();

        assert_eq!(config.get_unique_field(), Some(":memory:"));
        assert_eq!(
            config.get_adapter_unique_id(),
            Some("f7935d72c5941a25cc019e2fb05ae050".to_string())
        );
    }

    #[test]
    fn test_bigquery_adapter_config_parsing() {
        let config: DbConfig = dbt_yaml::from_str(
            "type: bigquery\n\
             job_creation_timeout_seconds: 123\n\
             job_execution_timeout_seconds: 456\n\
             job_retry_deadline_seconds: 789",
        )
        .unwrap();
        if let DbConfig::Bigquery(bigquery_config) = config {
            assert_eq!(bigquery_config.job_creation_timeout_seconds, Some(123));
            assert_eq!(bigquery_config.job_execution_timeout_seconds, Some(456));
            assert_eq!(bigquery_config.job_retry_deadline_seconds, Some(789));
        } else {
            panic!("Expected DbConfig::Bigquery, got {config:?}",);
        }
    }

    #[test]
    fn test_snowflake_target_context_allows_missing_user() {
        let config: DbConfig = SnowflakeDbConfig {
            account: Some("acct".to_string()),
            database: Some("db".to_string()),
            schema: Some("schema".to_string()),
            ..Default::default()
        }
        .into();

        let target = TargetContext::try_from(config).expect("snowflake target context");
        let TargetContext::Snowflake(target) = target else {
            panic!("expected snowflake target context");
        };
        assert_eq!(target.user, None);
    }

    #[test]
    fn test_snowflake_target_context_preserves_user_when_present() {
        let config: DbConfig = SnowflakeDbConfig {
            account: Some("acct".to_string()),
            user: Some("user".to_string()),
            database: Some("db".to_string()),
            schema: Some("schema".to_string()),
            ..Default::default()
        }
        .into();

        let target = TargetContext::try_from(config).expect("snowflake target context");
        let TargetContext::Snowflake(target) = target else {
            panic!("expected snowflake target context");
        };
        assert_eq!(target.user.as_deref(), Some("user"));
    }

    #[test]
    fn test_snowflake_port_accepts_integer_and_string() {
        let cases = [
            ("port: 9999", StringOrInteger::Integer(9999)),
            (
                "port: \"9999\"",
                StringOrInteger::String("9999".to_string()),
            ),
        ];

        for (port_yaml, expected_port) in cases {
            let config: DbConfig = dbt_yaml::from_str(&format!(
                "type: snowflake\n\
                 account: acct\n\
                 database: db\n\
                 schema: schema\n\
                 host: 127.0.0.1\n\
                 {port_yaml}\n\
                 protocol: http"
            ))
            .unwrap_or_else(|err| {
                panic!("failed to parse snowflake config with {port_yaml}: {err}")
            });

            let DbConfig::Snowflake(snowflake_config) = config.clone() else {
                panic!("expected snowflake config");
            };
            assert_eq!(snowflake_config.port, Some(expected_port.clone()));

            let target = TargetContext::try_from(config).expect("snowflake target context");
            let TargetContext::Snowflake(target) = target else {
                panic!("expected snowflake target context");
            };
            assert_eq!(target.port, Some(expected_port));
        }
    }

    #[test]
    fn test_snowflake_metadata_warehouse_parses_and_reaches_target_context() {
        let config: DbConfig = dbt_yaml::from_str(
            "type: snowflake\n\
             account: acct\n\
             database: db\n\
             schema: schema\n\
             warehouse: build_wh\n\
             metadata_warehouse: metadata_wh",
        )
        .unwrap();

        let DbConfig::Snowflake(snowflake_config) = config.clone() else {
            panic!("expected snowflake config");
        };
        assert_eq!(
            snowflake_config.metadata_warehouse.as_deref(),
            Some("metadata_wh")
        );

        let target = TargetContext::try_from(config).expect("snowflake target context");
        let TargetContext::Snowflake(target) = target else {
            panic!("expected snowflake target context");
        };
        assert_eq!(target.warehouse.as_deref(), Some("build_wh"));
        assert_eq!(target.metadata_warehouse.as_deref(), Some("metadata_wh"));
    }

    #[test]
    fn test_snowflake_adaptive_metadata_fetch_parses_and_reaches_target_context() {
        let config: DbConfig = dbt_yaml::from_str(
            "type: snowflake\n\
             account: acct\n\
             database: db\n\
             schema: schema\n\
             warehouse: build_wh\n\
             adaptive_metadata_fetch: false",
        )
        .unwrap();

        let DbConfig::Snowflake(snowflake_config) = config.clone() else {
            panic!("expected snowflake config");
        };
        assert_eq!(snowflake_config.adaptive_metadata_fetch, Some(false));

        let target = TargetContext::try_from(config).expect("snowflake target context");
        let TargetContext::Snowflake(target) = target else {
            panic!("expected snowflake target context");
        };
        assert_eq!(target.adaptive_metadata_fetch, Some(false));
    }

    #[test]
    fn test_snowflake_adaptive_metadata_fetch_defaults_to_unset() {
        let config: DbConfig = dbt_yaml::from_str(
            "type: snowflake\n\
             account: acct\n\
             database: db\n\
             schema: schema\n\
             warehouse: build_wh",
        )
        .unwrap();

        let DbConfig::Snowflake(snowflake_config) = config else {
            panic!("expected snowflake config");
        };
        assert_eq!(snowflake_config.adaptive_metadata_fetch, None);
    }

    #[test]
    fn test_duckdb_parse_path_variants() {
        // Memory
        let info = DuckDBPathInfo::parse_path(None);
        assert_eq!(info.location, DuckDBLocation::Memory);
        assert_eq!(info.database, "memory");
        assert!(!info.is_ducklake);

        let info = DuckDBPathInfo::parse_path(Some(":memory:"));
        assert_eq!(info.location, DuckDBLocation::Memory);
        assert_eq!(info.database, "memory");

        // MotherDuck
        let info = DuckDBPathInfo::parse_path(Some("md:my_db"));
        assert!(matches!(info.location, DuckDBLocation::Motherduck { .. }));
        assert_eq!(info.database, "my_db");
        assert!(!info.is_ducklake);

        let info = DuckDBPathInfo::parse_path(Some("motherduck:my_db?token=secret"));
        assert!(matches!(info.location, DuckDBLocation::Motherduck { .. }));
        assert_eq!(info.database, "my_db");

        // Filesystem
        let info = DuckDBPathInfo::parse_path(Some("/tmp/jaffle_shop.duckdb"));
        assert!(matches!(info.location, DuckDBLocation::Filesystem { .. }));
        assert_eq!(info.database, "jaffle_shop");
        assert!(!info.is_ducklake);

        // DuckLake with MotherDuck
        let info = DuckDBPathInfo::parse_path(Some("ducklake:md:my_db"));
        assert!(matches!(info.location, DuckDBLocation::Motherduck { .. }));
        assert_eq!(info.database, "my_db");
        assert!(info.is_ducklake);
    }

    #[test]
    fn test_duckdb_config_parses_top_level_is_ducklake() {
        let config: DbConfig = dbt_yaml::from_str(
            "type: duckdb\n\
             path: md:my_db\n\
             is_ducklake: true",
        )
        .unwrap();

        assert_eq!(config.get_database_or_default(), "my_db");

        let DbConfig::DuckDB(duckdb_config) = config else {
            panic!("Expected DbConfig::DuckDB");
        };
        assert_eq!(duckdb_config.is_ducklake, Some(true));
    }

    #[test]
    fn test_duckdb_extensions_as_strings() {
        // Test that extensions can be simple strings (existing behavior)
        let config: DbConfig = dbt_yaml::from_str(
            "type: duckdb\n\
             path: db.duckdb\n\
             extensions:\n\
               - httpfs\n\
               - parquet",
        )
        .unwrap();

        let DbConfig::DuckDB(duckdb_config) = config else {
            panic!("Expected DbConfig::DuckDB");
        };
        assert!(duckdb_config.extensions.is_some());
        let extensions = duckdb_config.extensions.unwrap();
        assert_eq!(extensions.len(), 2);
    }

    #[test]
    fn test_duckdb_extensions_as_objects() {
        // Test that extensions can be objects with name and repo (issue #1530)
        let config: DbConfig = dbt_yaml::from_str(
            "type: duckdb\n\
             path: db.duckdb\n\
             extensions:\n\
               - name: duckpgq\n\
                 repo: community",
        )
        .unwrap();

        let DbConfig::DuckDB(duckdb_config) = config else {
            panic!("Expected DbConfig::DuckDB");
        };
        assert!(duckdb_config.extensions.is_some());
        let extensions = duckdb_config.extensions.unwrap();
        assert_eq!(extensions.len(), 1);
    }

    #[test]
    fn test_duckdb_extensions_mixed() {
        // Test that extensions can mix strings and objects
        let yaml_str = r#"
type: duckdb
path: db.duckdb
extensions:
  - httpfs
  - name: duckpgq
    repo: community
  - parquet
"#;
        let config: DbConfig = dbt_yaml::from_str(yaml_str).unwrap();

        let DbConfig::DuckDB(duckdb_config) = config else {
            panic!("Expected DbConfig::DuckDB");
        };
        assert!(duckdb_config.extensions.is_some());
        let extensions = duckdb_config.extensions.unwrap();
        assert_eq!(extensions.len(), 3);
    }

    #[test]
    fn test_redshift_drop_without_cascade_round_trips_through_connection_mapping() {
        let config: DbConfig = dbt_yaml::from_str(
            "type: redshift\n\
             host: localhost\n\
             user: u\n\
             port: 5439\n\
             database: mydb\n\
             schema: public\n\
             drop_without_cascade: true",
        )
        .unwrap();

        let DbConfig::Redshift(ref redshift_config) = config else {
            panic!("Expected DbConfig::Redshift, got {config:?}");
        };
        assert_eq!(redshift_config.drop_without_cascade, Some(true));

        let mapping = config.to_connection_mapping().unwrap();
        let value = mapping
            .get(dbt_yaml::Value::from("drop_without_cascade"))
            .expect("drop_without_cascade should be present in connection mapping");
        assert_eq!(value.as_bool(), Some(true));
    }

    /// `get_connection_keys` decides what `dbt debug` prints as a connection's
    /// details. Since `dbt debug` checks every declared adapter, a credential named
    /// here leaks for every adapter in a multi-adapter target, not just the default.
    /// This walks every variant so a newly added adapter cannot reintroduce one.
    #[test]
    fn connection_keys_never_expose_a_credential() {
        // Substrings, so `motherduck_token` and `client_secret` are caught as well
        // as bare `token` / `secret`. `token_uri` is an OAuth endpoint URL rather
        // than a credential, and `secrets` is DuckDB's credential block.
        const CREDENTIAL_MARKERS: &[&str] = &[
            "password",
            "token",
            "secret",
            "private_key",
            "passphrase",
            "keyfile",
            "api_key",
            "credential",
        ];
        const ALLOWED: &[&str] = &["token_uri"];

        // Driven off `AdapterType::iter()` so a newly supported adapter is audited
        // the moment it exists, without this test needing a constructible config.
        use strum::IntoEnumIterator;

        for adapter_type in AdapterType::iter() {
            for key in DbConfig::connection_keys_for(adapter_type) {
                if ALLOWED.contains(key) {
                    continue;
                }
                for marker in CREDENTIAL_MARKERS {
                    assert!(
                        !key.contains(marker),
                        "`{adapter_type}` lists connection key `{key}`, which looks like a \
                         credential (matched `{marker}`). `dbt debug` prints these, so remove \
                         it or add it to ALLOWED if it is genuinely not secret."
                    );
                }
            }
        }
    }

    #[test]
    fn test_redshift_serverless_fields_reach_connection_mapping() {
        // #14621: is_serverless / serverless_work_group must survive into the
        // connection mapping, since that is what the auth layer reads to select
        // the serverless path. They used to be dropped on deserialization.
        let config: DbConfig = dbt_yaml::from_str(
            "type: redshift\n\
             method: iam\n\
             host: 127.0.0.1\n\
             port: 5439\n\
             database: mydb\n\
             schema: public\n\
             region: us-east-1\n\
             is_serverless: true\n\
             serverless_work_group: my-workgroup",
        )
        .unwrap();

        let DbConfig::Redshift(ref redshift_config) = config else {
            panic!("Expected DbConfig::Redshift, got {config:?}");
        };
        assert_eq!(redshift_config.is_serverless, Some(true));
        assert_eq!(
            redshift_config.serverless_work_group.as_deref(),
            Some("my-workgroup")
        );

        let mapping = config.to_connection_mapping().unwrap();
        assert_eq!(
            mapping
                .get(dbt_yaml::Value::from("is_serverless"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            mapping
                .get(dbt_yaml::Value::from("serverless_work_group"))
                .and_then(|v| v.as_str()),
            Some("my-workgroup")
        );
    }

    #[test]
    fn test_clickhouse_minimal_config_parses() {
        let config: DbConfig = dbt_yaml::from_str(
            "type: clickhouse\n\
             schema: analytics\n",
        )
        .unwrap();

        let DbConfig::ClickHouse(clickhouse_config) = config else {
            panic!("Expected DbConfig::ClickHouse");
        };
        assert_eq!(clickhouse_config.host, Some("localhost".to_string()));
        assert_eq!(clickhouse_config.port, None);
        assert_eq!(clickhouse_config.user, Some("default".to_string()));
        assert_eq!(clickhouse_config.password, Some(String::new()));
        assert_eq!(clickhouse_config.schema, Some("analytics".to_string()));
        assert_eq!(clickhouse_config.secure, Some(false));
    }

    #[test]
    fn test_clickhouse_full_config_parses() {
        let config: DbConfig = dbt_yaml::from_str(
            "type: clickhouse\n\
             host: ch.prod.internal\n\
             port: 9000\n\
             user: alice\n\
             password: secret\n\
             schema: analytics\n\
             secure: true\n\
             threads: 4\n",
        )
        .unwrap();

        let DbConfig::ClickHouse(clickhouse_config) = config else {
            panic!("Expected DbConfig::ClickHouse");
        };
        assert_eq!(clickhouse_config.host, Some("ch.prod.internal".to_string()));
        assert_eq!(clickhouse_config.port, Some(StringOrInteger::Integer(9000)));
        assert_eq!(clickhouse_config.user, Some("alice".to_string()));
        assert_eq!(clickhouse_config.password, Some("secret".to_string()));
        assert_eq!(clickhouse_config.schema, Some("analytics".to_string()));
        assert_eq!(clickhouse_config.secure, Some(true));
        assert_eq!(clickhouse_config.threads, Some(StringOrInteger::Integer(4)));
    }

    #[test]
    fn test_clickhouse_schema_is_target_database() {
        let config: DbConfig = dbt_yaml::from_str(
            "type: clickhouse\n\
             schema: analytics\n",
        )
        .unwrap();

        assert_eq!(config.get_database(), None);
        let target = TargetContext::try_from(config).expect("clickhouse target context");
        let TargetContext::ClickHouse(target) = target else {
            panic!("expected clickhouse target context");
        };
        assert_eq!(target.__common__.database, "");
        assert_eq!(target.__common__.schema, "analytics");
    }

    #[test]
    fn test_clickhouse_adapter_type_dispatch() {
        let config: DbConfig = ClickHouseDbConfig {
            host: Some("ch.example.com".to_string()),
            schema: Some("analytics".to_string()),
            ..Default::default()
        }
        .into();

        assert_eq!(config.adapter_type(), AdapterType::ClickHouse);
        assert_eq!(config.get_unique_field(), Some("ch.example.com"));
    }

    fn clickhouse_profile_parity_target_context() -> ClickHouseTargetEnv {
        let yaml_str = r#"
type: clickhouse
driver: native
host: ch.prod.internal
port: 9000
user: analytics
password: secret
schema: analytics
cluster: my_cluster
database_engine: Replicated
cluster_mode: true
secure: true
verify: false
client_cert: /path/to/cert.pem
client_cert_key: /path/to/key.pem
retries: 5
compression: lz4
connect_timeout: 30
send_receive_timeout: 600
sync_request_timeout: 10
compress_block_size: 524288
check_exchange: false
use_lw_deletes: true
allow_automatic_deduplication: true
custom_settings:
  async_insert: 1
local_suffix: _shard
local_db_prefix: shard_
threads: 8
"#;
        let config: DbConfig = dbt_yaml::from_str(yaml_str).unwrap();
        let target = TargetContext::try_from(config).expect("clickhouse target context");
        let TargetContext::ClickHouse(target) = target else {
            panic!("expected clickhouse target context");
        };

        target
    }

    #[test]
    fn test_clickhouse_target_context_exposes_connection_fields() {
        let t = clickhouse_profile_parity_target_context();

        assert_eq!(t.driver.as_deref(), Some("native"));
        assert_eq!(t.host.as_deref(), Some("ch.prod.internal"));
        match t.port {
            Some(StringOrInteger::Integer(p)) => assert_eq!(p, 9000),
            other => panic!("expected Integer(9000), got {other:?}"),
        }
        assert_eq!(t.user.as_deref(), Some("analytics"));
        assert_eq!(t.retries, Some(5));
        assert_eq!(t.cluster.as_deref(), Some("my_cluster"));
        assert_eq!(t.database_engine.as_deref(), Some("Replicated"));
        assert_eq!(t.cluster_mode, Some(true));
        assert_eq!(t.secure, Some(true));
        assert_eq!(t.verify, Some(false));
        assert_eq!(t.connect_timeout, Some(30));
        assert_eq!(t.send_receive_timeout, Some(600));
        assert_eq!(t.sync_request_timeout, Some(10));
        assert_eq!(t.compress_block_size, Some(524_288));
        assert_eq!(t.compression.as_deref(), Some("lz4"));
        assert_eq!(t.check_exchange, Some(false));
    }

    #[test]
    fn test_clickhouse_target_context_exposes_materialization_fields() {
        let t = clickhouse_profile_parity_target_context();

        assert_eq!(t.use_lw_deletes, Some(true));
        assert_eq!(t.allow_automatic_deduplication, Some(true));
        assert_eq!(t.local_suffix.as_deref(), Some("_shard"));
        assert_eq!(t.local_db_prefix.as_deref(), Some("shard_"));
        let custom_settings = t.custom_settings.as_ref().expect("custom_settings");
        assert!(custom_settings.contains_key("async_insert"));
        assert_eq!(t.__common__.database, "");
        assert_eq!(t.__common__.schema, "analytics");
        assert_eq!(t.__common__.threads, Some(8));

        let serialized = dbt_yaml::to_value(&t).expect("serialize");
        let mapping = serialized.as_mapping().expect("mapping");
        assert!(!mapping.contains_key(YmlValue::string("client_cert".to_string())));
        assert!(!mapping.contains_key(YmlValue::string("client_cert_key".to_string())));
    }

    #[test]
    fn test_clickhouse_profile_parity_default_connection_fields() {
        let config: DbConfig = dbt_yaml::from_str("type: clickhouse").unwrap();
        let DbConfig::ClickHouse(ch) = &config else {
            panic!("Expected DbConfig::ClickHouse");
        };
        assert!(ch.driver.is_none());
        assert!(ch.port.is_none());
        assert!(ch.client_cert.is_none());
        assert!(ch.client_cert_key.is_none());
        assert_eq!(ch.host.as_deref(), Some("localhost"));
        assert_eq!(ch.user.as_deref(), Some("default"));
        assert_eq!(ch.password.as_deref(), Some(""));
        assert_eq!(ch.schema.as_deref(), Some("default"));
        assert_eq!(ch.cluster.as_deref(), Some(""));
        assert_eq!(ch.database_engine.as_deref(), Some(""));
        assert_eq!(ch.compression.as_deref(), Some(""));
        assert_eq!(ch.local_suffix.as_deref(), Some("_local"));
        assert_eq!(ch.local_db_prefix.as_deref(), Some(""));
        let custom_settings = ch.custom_settings.as_ref().expect("custom_settings");
        assert!(custom_settings.is_empty());
    }

    #[test]
    fn test_clickhouse_profile_parity_default_boolean_fields() {
        let config: DbConfig = dbt_yaml::from_str("type: clickhouse").unwrap();
        let DbConfig::ClickHouse(ch) = &config else {
            panic!("Expected DbConfig::ClickHouse");
        };

        assert_eq!(ch.cluster_mode, Some(false));
        assert_eq!(ch.secure, Some(false));
        assert_eq!(ch.verify, Some(true));
        assert_eq!(ch.check_exchange, Some(true));
        assert_eq!(ch.use_lw_deletes, Some(false));
        assert_eq!(ch.allow_automatic_deduplication, Some(false));
    }

    #[test]
    fn test_clickhouse_profile_parity_default_numeric_and_target_fields() {
        let config: DbConfig = dbt_yaml::from_str("type: clickhouse").unwrap();
        let DbConfig::ClickHouse(ch) = &config else {
            panic!("Expected DbConfig::ClickHouse");
        };

        assert_eq!(ch.retries, Some(1));
        assert_eq!(ch.connect_timeout, Some(10));
        assert_eq!(ch.send_receive_timeout, Some(300));
        assert_eq!(ch.sync_request_timeout, Some(5));
        assert_eq!(ch.compress_block_size, Some(1_048_576));

        let target = TargetContext::try_from(config).expect("clickhouse target context");
        let TargetContext::ClickHouse(t) = target else {
            panic!("expected clickhouse target context");
        };
        assert_eq!(t.__common__.database, "");
        assert_eq!(t.__common__.schema, "default");
        assert_eq!(t.__common__.threads, Some(1));
    }

    #[test]
    fn test_clickhouse_get_database_returns_none() {
        let config: DbConfig = ClickHouseDbConfig {
            schema: Some("analytics".to_string()),
            ..Default::default()
        }
        .into();

        assert_eq!(config.get_database(), None);
        assert_eq!(config.get_database_or_default(), "");
    }
}
