use std::collections::HashMap;
use std::env;
use std::fmt;
use std::time::Duration;

use dbt_cloud_config::ResolvedCloudConfig;
use dbt_schemas::state::DbtProfile;

use crate::proto::query_cache::{Struct, Value, value};

pub const DEFAULT_API_URL: &str = "api.state.dbt.com:443";
pub const DEFAULT_OAUTH_TOKEN_URL: &str = "https://auth.state.dbt.com/token";
pub const DEFAULT_OAUTH_AUTH_URL: &str = "https://auth.state.dbt.com";
pub const DEFAULT_API_CLIENT_TIMEOUT_SECONDS: u64 = 60;
/// Total attempts per State API request when a transient (UNAVAILABLE / dropped
/// connection) transport error occurs.
pub const DEFAULT_API_CLIENT_MAX_ATTEMPTS: u32 = 3;
pub const DEFAULT_FRESHNESS_TOLERANCE_SECONDS: i64 = 2700;
pub const DEFAULT_OAUTH_CLIENT_ID: &str = "2fd87cd5-69a6-4c5f-9097-747a58f0edf6";
pub const DEFAULT_DEFER_TO: &str = "prod";
pub const DEFAULT_DEFER_LOG_LEVEL: &str = "off";
pub const DEFAULT_LOG_FILE_LIMIT: i64 = 20;
pub const DEFAULT_LOG_PREFIX: &str = "responses_";
pub const DEFAULT_METADATA_CACHE_TTL_SECONDS: i64 = 0;
const STATE_MANAGE_ENV: &str = "DBT_ENGINE_MANAGE_STATE";
const STATE_EMIT_REUSED_STATUS_ENV: &str = "DBT_ENGINE_STATE_EMIT_REUSED_STATUS";
const STATE_OAUTH_CLIENT_ID_ENV: &str = "DBT_ENGINE_STATE_OAUTH_CLIENT_ID";
const STATE_OAUTH_CLIENT_SECRET_ENV: &str = "DBT_ENV_SECRET_STATE_OAUTH_CLIENT_SECRET";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloneIncrementalInDev {
    Never,
    IfTableMissing,
    Always,
}

impl CloneIncrementalInDev {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "NEVER",
            Self::IfTableMissing => "IF_TABLE_MISSING",
            Self::Always => "ALWAYS",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunCacheServiceConfig {
    pub enabled: bool,
    pub api_url: String,
    pub secure: bool,
    pub org_id: Option<String>,
    /// dbt platform account id from `dbt-cloud: account_id` (or
    /// `DBT_CLOUD_ACCOUNT_ID`), trimmed. Used to pick the matching cached
    /// platform OAuth session when several accounts are logged in, and to
    /// detect that a cached dbt State token belongs to another account.
    pub platform_account_id: Option<String>,
    pub oauth_client_id: String,
    pub oauth_client_secret: Option<String>,
    pub oauth_token_url: String,
    pub oauth_auth_url: String,
    pub timeout: Duration,
    /// Total attempts per request on transient (dropped-connection) transport
    /// errors. `1` disables retries; clamped to gRPC's cap of 5 by the client.
    pub max_attempts: u32,
    pub defer_to: String,
    pub defer_log_level: String,
    pub enable_response_logging: bool,
    pub enable_data_tests: bool,
    pub emit_reused_status: bool,
    pub log_file_limit: i64,
    pub log_dir_override: Option<String>,
    pub log_prefix: String,
    pub freshness_tolerance_seconds: i64,
    pub tolerate_nondeterminism: bool,
    pub enable_lenient_dependencies: bool,
    pub clone_incremental_in_dev: CloneIncrementalInDev,
    pub clone_time_travel_limit_seconds: Option<i64>,
    pub metadata_cache_ttl_seconds: i64,
    pub run_hooks_on_no_op: bool,
    pub compare_unrendered_code: bool,
    pub snowflake_get_view_ddl_override: Option<String>,
    pub snowflake_metadata_warehouse: Option<String>,
}

impl RunCacheServiceConfig {
    pub fn from_env() -> Result<Self, RunCacheServiceConfigError> {
        Self::from_env_getter(|name| env::var(name).ok())
    }

    pub fn from_env_and_cloud_config(
        cloud_config: Option<&ResolvedCloudConfig>,
    ) -> Result<Self, RunCacheServiceConfigError> {
        Self::from_env_getter_and_cloud_config(|name| env::var(name).ok(), cloud_config)
    }

    pub fn is_explicitly_requested_from_env() -> bool {
        Self::is_explicitly_requested_from_env_getter(|name| env::var(name).ok())
    }

    pub fn is_explicitly_disabled_from_env() -> bool {
        Self::is_explicitly_disabled_from_env_getter(|name| env::var(name).ok())
    }

    pub fn is_explicitly_requested_from_env_getter<F>(mut get_env: F) -> bool
    where
        F: FnMut(&str) -> Option<String>,
    {
        get_env(STATE_MANAGE_ENV)
            .filter(|value| !value.trim().is_empty())
            .map(|value| parse_bool(STATE_MANAGE_ENV, &value).unwrap_or(true))
            .unwrap_or(false)
    }

    pub fn is_explicitly_disabled_from_env_getter<F>(mut get_env: F) -> bool
    where
        F: FnMut(&str) -> Option<String>,
    {
        match get_env(STATE_MANAGE_ENV).filter(|value| !value.trim().is_empty()) {
            Some(value) => parse_bool(STATE_MANAGE_ENV, &value)
                .map(|enabled| !enabled)
                .unwrap_or(false),
            None => config_value(&mut get_env, "ENABLED")
                .map(|value| {
                    parse_bool("ENABLED", &value)
                        .map(|enabled| !enabled)
                        .unwrap_or(false)
                })
                .unwrap_or(false),
        }
    }

    pub fn from_env_getter<F>(mut get_env: F) -> Result<Self, RunCacheServiceConfigError>
    where
        F: FnMut(&str) -> Option<String>,
    {
        let enabled = match get_env(STATE_MANAGE_ENV).filter(|value| !value.trim().is_empty()) {
            Some(value) => {
                let manage_state_enabled = parse_bool(STATE_MANAGE_ENV, &value)?;
                if manage_state_enabled {
                    config_enabled(&mut get_env)?.unwrap_or(true)
                } else {
                    false
                }
            }
            None => config_enabled(&mut get_env)?.unwrap_or(true),
        };
        let api_url =
            config_value(&mut get_env, "API_URL").unwrap_or_else(|| DEFAULT_API_URL.to_string());
        let secure = match config_value(&mut get_env, "API_SECURE") {
            Some(value) => parse_bool("API_SECURE", &value)?,
            None => default_secure(&api_url),
        };
        let timeout = match config_value(&mut get_env, "API_CLIENT_TIMEOUT") {
            Some(value) => Duration::from_secs(parse_u64_seconds("API_CLIENT_TIMEOUT", &value)?),
            None => Duration::from_secs(DEFAULT_API_CLIENT_TIMEOUT_SECONDS),
        };
        let max_attempts = match config_value(&mut get_env, "API_CLIENT_MAX_ATTEMPTS") {
            Some(value) => parse_u32("API_CLIENT_MAX_ATTEMPTS", &value)?,
            None => DEFAULT_API_CLIENT_MAX_ATTEMPTS,
        };
        let defer_to =
            config_value(&mut get_env, "DEFER_TO").unwrap_or_else(|| DEFAULT_DEFER_TO.to_string());
        let defer_log_level = config_value(&mut get_env, "DEFER_LOG_LEVEL")
            .unwrap_or_else(|| DEFAULT_DEFER_LOG_LEVEL.to_string());
        let enable_response_logging = match config_value(&mut get_env, "ENABLE_RESPONSE_LOGGING") {
            Some(value) => parse_bool("ENABLE_RESPONSE_LOGGING", &value)?,
            None => true,
        };
        let enable_data_tests = match config_value(&mut get_env, "ENABLE_DATA_TESTS") {
            Some(value) => parse_bool("ENABLE_DATA_TESTS", &value)?,
            None => true,
        };
        let emit_reused_status = match state_config_value(
            &mut get_env,
            STATE_EMIT_REUSED_STATUS_ENV,
            "EMIT_REUSED_STATUS",
        ) {
            Some(value) => parse_bool(STATE_EMIT_REUSED_STATUS_ENV, &value)?,
            None => false,
        };
        let log_file_limit = match config_value(&mut get_env, "LOG_FILE_LIMIT") {
            Some(value) => parse_i64("LOG_FILE_LIMIT", &value)?,
            None => DEFAULT_LOG_FILE_LIMIT,
        };
        let log_prefix = config_value(&mut get_env, "LOG_PREFIX")
            .unwrap_or_else(|| DEFAULT_LOG_PREFIX.to_string());
        let freshness_tolerance_seconds = match config_value(&mut get_env, "FRESHNESS_TOLERANCE") {
            Some(value) => parse_i64_seconds("FRESHNESS_TOLERANCE", &value)?,
            None => DEFAULT_FRESHNESS_TOLERANCE_SECONDS,
        };
        let tolerate_nondeterminism = match config_value(&mut get_env, "TOLERATE_NONDETERMINISM") {
            Some(value) => parse_bool("TOLERATE_NONDETERMINISM", &value)?,
            None => true,
        };
        let enable_lenient_dependencies =
            match config_value(&mut get_env, "ENABLE_LENIENT_DEPENDENCIES") {
                Some(value) => parse_bool("ENABLE_LENIENT_DEPENDENCIES", &value)?,
                None => true,
            };
        let clone_incremental_in_dev = match config_value(&mut get_env, "CLONE_INCREMENTAL_IN_DEV")
        {
            Some(value) => parse_clone_incremental_in_dev(&value)?,
            None => CloneIncrementalInDev::IfTableMissing,
        };
        let clone_time_travel_limit_seconds =
            match config_value(&mut get_env, "CLONE_TIME_TRAVEL_LIMIT") {
                Some(value) => Some(parse_i64_seconds("CLONE_TIME_TRAVEL_LIMIT", &value)?),
                None => None,
            };
        let metadata_cache_ttl_seconds = match config_value(&mut get_env, "METADATA_CACHE_TTL") {
            Some(value) => parse_i64_seconds("METADATA_CACHE_TTL", &value)?,
            None => DEFAULT_METADATA_CACHE_TTL_SECONDS,
        };
        let run_hooks_on_no_op = match config_value(&mut get_env, "RUN_HOOKS_ON_NO_OP") {
            Some(value) => parse_bool("RUN_HOOKS_ON_NO_OP", &value)?,
            None => false,
        };
        let compare_unrendered_code = match config_value(&mut get_env, "COMPARE_UNRENDERED_CODE") {
            Some(value) => parse_bool("COMPARE_UNRENDERED_CODE", &value)?,
            None => false,
        };

        Ok(Self {
            enabled,
            api_url,
            secure,
            org_id: config_value(&mut get_env, "ORG_ID"),
            platform_account_id: None,
            oauth_client_id: state_config_value(
                &mut get_env,
                STATE_OAUTH_CLIENT_ID_ENV,
                "OAUTH_CLIENT_ID",
            )
            .unwrap_or_else(|| DEFAULT_OAUTH_CLIENT_ID.to_string()),
            oauth_client_secret: state_config_value(
                &mut get_env,
                STATE_OAUTH_CLIENT_SECRET_ENV,
                "OAUTH_CLIENT_SECRET",
            ),
            oauth_token_url: config_value(&mut get_env, "TOKEN_URL")
                .unwrap_or_else(|| DEFAULT_OAUTH_TOKEN_URL.to_string()),
            oauth_auth_url: config_value(&mut get_env, "AUTH_URL")
                .unwrap_or_else(|| DEFAULT_OAUTH_AUTH_URL.to_string()),
            timeout,
            max_attempts,
            defer_to,
            defer_log_level,
            enable_response_logging,
            enable_data_tests,
            emit_reused_status,
            log_file_limit,
            log_dir_override: config_value(&mut get_env, "LOG_DIR_OVERRIDE"),
            log_prefix,
            freshness_tolerance_seconds,
            tolerate_nondeterminism,
            enable_lenient_dependencies,
            clone_incremental_in_dev,
            clone_time_travel_limit_seconds,
            metadata_cache_ttl_seconds,
            run_hooks_on_no_op,
            compare_unrendered_code,
            snowflake_get_view_ddl_override: config_value(
                &mut get_env,
                "SNOWFLAKE_GET_VIEW_DDL_OVERRIDE",
            ),
            snowflake_metadata_warehouse: config_value(
                &mut get_env,
                "SNOWFLAKE_METADATA_WAREHOUSE",
            ),
        })
    }

    fn from_env_getter_and_cloud_config<F>(
        get_env: F,
        cloud_config: Option<&ResolvedCloudConfig>,
    ) -> Result<Self, RunCacheServiceConfigError>
    where
        F: FnMut(&str) -> Option<String>,
    {
        Self::from_env_getter(get_env).map(|config| config.with_cloud_config(cloud_config))
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            api_url: DEFAULT_API_URL.to_string(),
            secure: true,
            org_id: None,
            platform_account_id: None,
            oauth_client_id: DEFAULT_OAUTH_CLIENT_ID.to_string(),
            oauth_client_secret: None,
            oauth_token_url: DEFAULT_OAUTH_TOKEN_URL.to_string(),
            oauth_auth_url: DEFAULT_OAUTH_AUTH_URL.to_string(),
            timeout: Duration::from_secs(DEFAULT_API_CLIENT_TIMEOUT_SECONDS),
            max_attempts: DEFAULT_API_CLIENT_MAX_ATTEMPTS,
            defer_to: DEFAULT_DEFER_TO.to_string(),
            defer_log_level: DEFAULT_DEFER_LOG_LEVEL.to_string(),
            enable_response_logging: true,
            enable_data_tests: true,
            emit_reused_status: false,
            log_file_limit: DEFAULT_LOG_FILE_LIMIT,
            log_dir_override: None,
            log_prefix: DEFAULT_LOG_PREFIX.to_string(),
            freshness_tolerance_seconds: DEFAULT_FRESHNESS_TOLERANCE_SECONDS,
            tolerate_nondeterminism: true,
            enable_lenient_dependencies: true,
            clone_incremental_in_dev: CloneIncrementalInDev::IfTableMissing,
            clone_time_travel_limit_seconds: None,
            metadata_cache_ttl_seconds: DEFAULT_METADATA_CACHE_TTL_SECONDS,
            run_hooks_on_no_op: false,
            compare_unrendered_code: false,
            snowflake_get_view_ddl_override: None,
            snowflake_metadata_warehouse: None,
        }
    }

    pub fn endpoint_uri(&self) -> String {
        let api_url = self.api_url.trim();
        let scheme = if self.secure { "https" } else { "http" };
        if let Some((_, authority)) = api_url.split_once("://") {
            format!("{scheme}://{authority}")
        } else {
            format!("{scheme}://{api_url}")
        }
    }

    fn with_cloud_config(mut self, cloud_config: Option<&ResolvedCloudConfig>) -> Self {
        if self.org_id.is_none() {
            self.org_id = cloud_config.and_then(|config| config.state_org_id.clone());
        }
        if self.platform_account_id.is_none() {
            self.platform_account_id = cloud_config
                .and_then(|config| config.account_id.as_deref())
                .map(str::trim)
                .filter(|account_id| !account_id.is_empty())
                .map(str::to_string);
        }
        self
    }

    pub fn defer_to_target(&self, active_profile: &DbtProfile) -> String {
        active_profile
            .defer_to_target
            .as_deref()
            .filter(|target| !target.is_empty())
            .unwrap_or(&self.defer_to)
            .to_string()
    }

    pub fn is_defer_to_target(&self, active_profile: &DbtProfile) -> bool {
        // is the current target the same as the "defer to" target
        // this is essentially an "is prod?" check
        self.defer_to_target(active_profile) == active_profile.target
    }

    pub fn telemetry_config(&self, active_profile: &DbtProfile) -> Struct {
        let mut fields = HashMap::new();
        insert_string(&mut fields, "api_url", &self.api_url);
        insert_bool(&mut fields, "secure", self.secure);
        insert_string_option(&mut fields, "org_id", self.org_id.as_deref());
        // `oauth_client_id`, like `oauth_client_secret`, is intentionally left
        // out: the Python dbt State client marks both sensitive and excludes
        // them from the config it reports as telemetry.
        insert_string(&mut fields, "oauth_token_url", &self.oauth_token_url);
        insert_string(&mut fields, "oauth_auth_url", &self.oauth_auth_url);
        insert_i64(
            &mut fields,
            "api_client_timeout",
            i64::try_from(self.timeout.as_secs()).unwrap_or(i64::MAX),
        );
        insert_i64(
            &mut fields,
            "api_client_max_attempts",
            i64::from(self.max_attempts),
        );
        insert_string(
            &mut fields,
            "defer_to",
            &self.defer_to_target(active_profile),
        );
        insert_string(&mut fields, "defer_log_level", &self.defer_log_level);
        insert_bool(
            &mut fields,
            "enable_response_logging",
            self.enable_response_logging,
        );
        insert_bool(&mut fields, "enable_data_tests", self.enable_data_tests);
        insert_i64(&mut fields, "log_file_limit", self.log_file_limit);
        insert_string_option(
            &mut fields,
            "log_dir_override",
            self.log_dir_override.as_deref(),
        );
        insert_string(&mut fields, "log_prefix", &self.log_prefix);
        insert_i64(
            &mut fields,
            "freshness_tolerance_seconds",
            self.freshness_tolerance_seconds,
        );
        insert_bool(
            &mut fields,
            "tolerate_nondeterminism",
            self.tolerate_nondeterminism,
        );
        insert_bool(
            &mut fields,
            "enable_lenient_dependencies",
            self.enable_lenient_dependencies,
        );
        insert_string(
            &mut fields,
            "clone_incremental_in_dev",
            self.clone_incremental_in_dev.as_str(),
        );
        insert_i64_option(
            &mut fields,
            "clone_time_travel_limit",
            self.clone_time_travel_limit_seconds,
        );
        insert_i64(
            &mut fields,
            "metadata_cache_ttl",
            self.metadata_cache_ttl_seconds,
        );
        insert_bool(&mut fields, "run_hooks_on_no_op", self.run_hooks_on_no_op);
        insert_string_option(
            &mut fields,
            "snowflake_get_view_ddl_override",
            self.snowflake_get_view_ddl_override.as_deref(),
        );
        insert_string_option(
            &mut fields,
            "snowflake_metadata_warehouse",
            self.snowflake_metadata_warehouse.as_deref(),
        );
        Struct { fields }
    }
}

fn insert_string(fields: &mut HashMap<String, Value>, key: &str, value: &str) {
    fields.insert(
        key.to_string(),
        Value {
            kind: Some(value::Kind::StringValue(value.to_string())),
        },
    );
}

fn insert_string_option(fields: &mut HashMap<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        insert_string(fields, key, value);
    }
}

fn insert_bool(fields: &mut HashMap<String, Value>, key: &str, value: bool) {
    fields.insert(
        key.to_string(),
        Value {
            kind: Some(value::Kind::BoolValue(value)),
        },
    );
}

fn insert_i64(fields: &mut HashMap<String, Value>, key: &str, value: i64) {
    fields.insert(
        key.to_string(),
        Value {
            kind: Some(value::Kind::IntValue(value)),
        },
    );
}

fn insert_i64_option(fields: &mut HashMap<String, Value>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        insert_i64(fields, key, value);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunCacheServiceConfigError {
    InvalidBool { name: &'static str, value: String },
    InvalidDuration { name: &'static str, value: String },
    InvalidInteger { name: &'static str, value: String },
    InvalidCloneIncrementalInDev { value: String },
    ProjectIdRequired,
}

impl fmt::Display for RunCacheServiceConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBool { name, value } => {
                write!(f, "invalid boolean for {}: {value}", env_name(name))
            }
            Self::InvalidDuration { name, value } => {
                write!(f, "invalid duration for {}: {value}", env_name(name))
            }
            Self::InvalidInteger { name, value } => {
                write!(f, "invalid integer for {}: {value}", env_name(name))
            }
            Self::InvalidCloneIncrementalInDev { value } => write!(
                f,
                "invalid value for RUN_CACHE_CLONE_INCREMENTAL_IN_DEV: {value}"
            ),
            Self::ProjectIdRequired => write!(
                f,
                "To use the state:* selector, please define the 'project-id' field in your dbt_project.yml:\n\ndbt-cloud:\n  project-id: 'your-project-id'\n\n",
            ),
        }
    }
}

impl std::error::Error for RunCacheServiceConfigError {}

fn env_name(name: &'static str) -> String {
    if name.starts_with("DBT_") {
        name.to_string()
    } else {
        format!("RUN_CACHE_{name}")
    }
}

fn config_value<F>(get_env: &mut F, name: &'static str) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    let primary = format!("RUN_CACHE_{name}");
    get_env(&primary).filter(|value| !value.trim().is_empty())
}

fn config_enabled<F>(get_env: &mut F) -> Result<Option<bool>, RunCacheServiceConfigError>
where
    F: FnMut(&str) -> Option<String>,
{
    config_value(get_env, "ENABLED")
        .map(|value| parse_bool("ENABLED", &value))
        .transpose()
}

fn state_config_value<F>(
    get_env: &mut F,
    state_name: &'static str,
    run_cache_name: &'static str,
) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    get_env(state_name)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| config_value(get_env, run_cache_name))
}

fn default_secure(api_url: &str) -> bool {
    let normalized = api_url.trim().to_ascii_lowercase();
    normalized.starts_with("https://") || normalized.ends_with(":443")
}

fn parse_bool(name: &'static str, value: &str) -> Result<bool, RunCacheServiceConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "t" | "yes" | "y" | "on" => Ok(true),
        "0" | "false" | "f" | "no" | "n" | "off" => Ok(false),
        _ => Err(RunCacheServiceConfigError::InvalidBool {
            name,
            value: value.to_string(),
        }),
    }
}

fn parse_u64_seconds(name: &'static str, value: &str) -> Result<u64, RunCacheServiceConfigError> {
    parse_seconds(name, value).and_then(|seconds| {
        u64::try_from(seconds).map_err(|_| RunCacheServiceConfigError::InvalidDuration {
            name,
            value: value.to_string(),
        })
    })
}

fn parse_i64_seconds(name: &'static str, value: &str) -> Result<i64, RunCacheServiceConfigError> {
    parse_seconds(name, value)
}

fn parse_u32(name: &'static str, value: &str) -> Result<u32, RunCacheServiceConfigError> {
    value
        .trim()
        .parse::<u32>()
        .map_err(|_| RunCacheServiceConfigError::InvalidInteger {
            name,
            value: value.to_string(),
        })
}

fn parse_i64(name: &'static str, value: &str) -> Result<i64, RunCacheServiceConfigError> {
    value
        .trim()
        .parse::<i64>()
        .map_err(|_| RunCacheServiceConfigError::InvalidInteger {
            name,
            value: value.to_string(),
        })
}

fn parse_clone_incremental_in_dev(
    value: &str,
) -> Result<CloneIncrementalInDev, RunCacheServiceConfigError> {
    match value.trim().to_ascii_uppercase().as_str() {
        "NEVER" => Ok(CloneIncrementalInDev::Never),
        "IF_TABLE_MISSING" => Ok(CloneIncrementalInDev::IfTableMissing),
        "ALWAYS" => Ok(CloneIncrementalInDev::Always),
        _ => Err(RunCacheServiceConfigError::InvalidCloneIncrementalInDev {
            value: value.to_string(),
        }),
    }
}

fn parse_seconds(name: &'static str, value: &str) -> Result<i64, RunCacheServiceConfigError> {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    let (number, multiplier) = lower
        .strip_suffix("seconds")
        .map(|number| (number, 1))
        .or_else(|| lower.strip_suffix("second").map(|number| (number, 1)))
        .or_else(|| lower.strip_suffix("secs").map(|number| (number, 1)))
        .or_else(|| lower.strip_suffix("sec").map(|number| (number, 1)))
        .or_else(|| lower.strip_suffix("minutes").map(|number| (number, 60)))
        .or_else(|| lower.strip_suffix("minute").map(|number| (number, 60)))
        .or_else(|| lower.strip_suffix("mins").map(|number| (number, 60)))
        .or_else(|| lower.strip_suffix("min").map(|number| (number, 60)))
        .or_else(|| lower.strip_suffix('m').map(|number| (number, 60)))
        .or_else(|| lower.strip_suffix("hours").map(|number| (number, 3600)))
        .or_else(|| lower.strip_suffix("hour").map(|number| (number, 3600)))
        .or_else(|| lower.strip_suffix("hrs").map(|number| (number, 3600)))
        .or_else(|| lower.strip_suffix("hr").map(|number| (number, 3600)))
        .or_else(|| lower.strip_suffix('h').map(|number| (number, 3600)))
        .or_else(|| lower.strip_suffix("days").map(|number| (number, 86400)))
        .or_else(|| lower.strip_suffix("day").map(|number| (number, 86400)))
        .or_else(|| lower.strip_suffix('d').map(|number| (number, 86400)))
        .or_else(|| lower.strip_suffix('s').map(|number| (number, 1)))
        .unwrap_or((trimmed, 1));

    number
        .trim()
        .parse::<i64>()
        .map(|n| n * multiplier)
        .map_err(|_| RunCacheServiceConfigError::InvalidDuration {
            name,
            value: value.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::IndexMap;
    use dbt_schemas::schemas::profiles::{DbConfig, DuckDbConfig};
    use dbt_schemas::state::ProfileAdapter;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn test_profile(target: &str, defer_to_target: Option<&str>) -> DbtProfile {
        let db_config = DbConfig::DuckDB(Box::<DuckDbConfig>::default());
        let default_adapter = db_config.adapter_type();
        let adapters = IndexMap::from([(default_adapter, ProfileAdapter::single(db_config))]);
        DbtProfile {
            profile: "default".to_string(),
            target: target.to_string(),
            defer_to_target: defer_to_target.map(|target| target.to_string()),
            allow_clones: true,
            adapters,
            default_adapter,
            schema: "dbt_test".to_string(),
            database: "db".to_string(),
            relative_profile_path: PathBuf::new(),
            threads: None,
        }
    }

    fn config_from_pairs(
        pairs: &[(&str, &str)],
    ) -> Result<RunCacheServiceConfig, RunCacheServiceConfigError> {
        let values = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<BTreeMap<_, _>>();
        RunCacheServiceConfig::from_env_getter(|name| values.get(name).cloned())
    }

    fn config_from_pairs_and_cloud_config(
        pairs: &[(&str, &str)],
        cloud_config: Option<&ResolvedCloudConfig>,
    ) -> Result<RunCacheServiceConfig, RunCacheServiceConfigError> {
        let values = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<BTreeMap<_, _>>();
        RunCacheServiceConfig::from_env_getter_and_cloud_config(
            |name| values.get(name).cloned(),
            cloud_config,
        )
    }

    #[test]
    fn defaults_match_python_client_surface() {
        let config = config_from_pairs(&[]).unwrap();

        assert_eq!(config.api_url, DEFAULT_API_URL);
        assert_eq!(config.oauth_token_url, DEFAULT_OAUTH_TOKEN_URL);
        assert!(config.enabled);
        assert!(config.secure);
        assert_eq!(config.timeout, Duration::from_secs(60));
        assert_eq!(config.max_attempts, DEFAULT_API_CLIENT_MAX_ATTEMPTS);
        assert_eq!(config.defer_to, "prod");
        assert_eq!(config.defer_log_level, "off");
        assert!(config.enable_response_logging);
        assert!(config.enable_data_tests);
        assert!(!config.emit_reused_status);
        assert_eq!(config.log_file_limit, 20);
        assert_eq!(config.log_dir_override, None);
        assert_eq!(config.log_prefix, "responses_");
        assert_eq!(config.freshness_tolerance_seconds, 2700);
        assert!(config.tolerate_nondeterminism);
        assert!(config.enable_lenient_dependencies);
        assert_eq!(
            config.clone_incremental_in_dev,
            CloneIncrementalInDev::IfTableMissing
        );
        assert_eq!(config.clone_time_travel_limit_seconds, None);
        assert_eq!(config.metadata_cache_ttl_seconds, 0);
        assert!(!config.run_hooks_on_no_op);
        assert!(!config.compare_unrendered_code);
        assert_eq!(config.snowflake_get_view_ddl_override, None);
        assert_eq!(config.snowflake_metadata_warehouse, None);
    }

    #[test]
    fn manage_state_can_be_disabled_from_config() {
        let config = config_from_pairs(&[("DBT_ENGINE_MANAGE_STATE", "false")]).unwrap();

        assert!(!config.enabled);
        assert!(!RunCacheServiceConfig::disabled().enabled);
        assert!(
            !RunCacheServiceConfig::is_explicitly_requested_from_env_getter(|name| {
                (name == "DBT_ENGINE_MANAGE_STATE").then(|| "false".to_string())
            })
        );
        assert!(
            RunCacheServiceConfig::is_explicitly_disabled_from_env_getter(|name| {
                (name == "RUN_CACHE_ENABLED").then(|| "false".to_string())
            })
        );
    }

    #[test]
    fn legacy_dbt_run_cache_env_prefix_is_ignored() {
        let config =
            config_from_pairs(&[("DBT_RUN_CACHE_API_URL", "legacy.example:1234")]).unwrap();

        assert_eq!(config.api_url, DEFAULT_API_URL);
        assert!(
            !RunCacheServiceConfig::is_explicitly_requested_from_env_getter(|name| {
                (name == "DBT_RUN_CACHE_API_URL").then(|| "legacy.example:1234".to_string())
            })
        );
    }

    #[test]
    fn cloud_config_state_org_id_sets_org_id() {
        let cloud_config = ResolvedCloudConfig {
            state_org_id: Some("project-org".to_string()),
            ..Default::default()
        };

        let config = config_from_pairs_and_cloud_config(&[], Some(&cloud_config)).unwrap();

        assert_eq!(config.org_id.as_deref(), Some("project-org"));
    }

    #[test]
    fn cloud_config_account_id_sets_platform_account_id() {
        let cloud_config = ResolvedCloudConfig {
            account_id: Some("456".to_string()),
            ..Default::default()
        };

        let config = config_from_pairs_and_cloud_config(&[], Some(&cloud_config)).unwrap();

        assert_eq!(config.platform_account_id.as_deref(), Some("456"));
    }

    #[test]
    fn cloud_config_account_id_is_trimmed_and_empty_is_ignored() {
        let padded = ResolvedCloudConfig {
            account_id: Some("  456  ".to_string()),
            ..Default::default()
        };
        let config = config_from_pairs_and_cloud_config(&[], Some(&padded)).unwrap();
        assert_eq!(config.platform_account_id.as_deref(), Some("456"));

        let blank = ResolvedCloudConfig {
            account_id: Some("   ".to_string()),
            ..Default::default()
        };
        let config = config_from_pairs_and_cloud_config(&[], Some(&blank)).unwrap();
        assert_eq!(config.platform_account_id, None);
    }

    #[test]
    fn env_org_id_overrides_cloud_config_state_org_id() {
        let cloud_config = ResolvedCloudConfig {
            state_org_id: Some("project-org".to_string()),
            ..Default::default()
        };

        let config = config_from_pairs_and_cloud_config(
            &[("RUN_CACHE_ORG_ID", "env-org")],
            Some(&cloud_config),
        )
        .unwrap();

        assert_eq!(config.org_id.as_deref(), Some("env-org"));
    }

    #[test]
    fn explicit_enabled_env_requests_service_without_api_url() {
        assert!(
            RunCacheServiceConfig::is_explicitly_requested_from_env_getter(|name| {
                (name == "DBT_ENGINE_MANAGE_STATE").then(|| "true".to_string())
            })
        );
        assert!(
            !RunCacheServiceConfig::is_explicitly_disabled_from_env_getter(|name| {
                (name == "RUN_CACHE_ENABLED").then(|| "true".to_string())
            })
        );

        assert!(
            RunCacheServiceConfig::is_explicitly_requested_from_env_getter(|name| {
                (name == "DBT_ENGINE_MANAGE_STATE").then(|| "not-a-bool".to_string())
            })
        );
        assert!(
            !RunCacheServiceConfig::is_explicitly_disabled_from_env_getter(|name| {
                (name == "RUN_CACHE_ENABLED").then(|| "not-a-bool".to_string())
            })
        );
    }

    #[test]
    fn service_config_env_does_not_request_service() {
        assert!(
            !RunCacheServiceConfig::is_explicitly_requested_from_env_getter(|name| {
                (name == "RUN_CACHE_API_URL").then(|| "localhost:50051".to_string())
            })
        );

        assert!(
            !RunCacheServiceConfig::is_explicitly_requested_from_env_getter(|name| {
                (name == "DBT_ENGINE_STATE_OAUTH_CLIENT_ID").then(|| "state-client-id".to_string())
            })
        );

        assert!(
            !RunCacheServiceConfig::is_explicitly_requested_from_env_getter(|name| {
                (name == "DBT_ENV_SECRET_STATE_OAUTH_CLIENT_SECRET")
                    .then(|| "state-client-secret".to_string())
            })
        );
    }

    #[test]
    fn dbt_state_manage_env_controls_enabled() {
        let config = config_from_pairs(&[("DBT_ENGINE_MANAGE_STATE", "false")]).unwrap();

        assert!(!config.enabled);
        assert!(
            !RunCacheServiceConfig::is_explicitly_requested_from_env_getter(|name| {
                (name == "DBT_ENGINE_MANAGE_STATE").then(|| "false".to_string())
            })
        );

        assert!(
            RunCacheServiceConfig::is_explicitly_requested_from_env_getter(|name| {
                (name == "DBT_ENGINE_MANAGE_STATE").then(|| "true".to_string())
            })
        );
    }

    #[test]
    fn dbt_state_manage_env_controls_enabled_without_service_config() {
        let config = config_from_pairs(&[
            ("DBT_ENGINE_MANAGE_STATE", "false"),
            ("RUN_CACHE_API_URL", "localhost:50051"),
        ])
        .unwrap();

        assert!(!config.enabled);
    }

    #[test]
    fn legacy_enabled_false_can_disable_managed_state() {
        let config = config_from_pairs(&[
            ("DBT_ENGINE_MANAGE_STATE", "true"),
            ("RUN_CACHE_ENABLED", "false"),
        ])
        .unwrap();

        assert!(!config.enabled);
    }

    #[test]
    fn manage_state_false_overrides_legacy_enabled_true() {
        let config = config_from_pairs(&[
            ("DBT_ENGINE_MANAGE_STATE", "false"),
            ("RUN_CACHE_ENABLED", "true"),
        ])
        .unwrap();

        assert!(!config.enabled);
    }

    #[test]
    fn invalid_manage_state_env_reports_manage_state_name() {
        let err = config_from_pairs(&[("DBT_ENGINE_MANAGE_STATE", "invalid")]).unwrap_err();

        assert!(err.to_string().contains("DBT_ENGINE_MANAGE_STATE"));
    }

    #[test]
    fn dbt_state_oauth_env_takes_precedence_over_legacy_run_cache_env() {
        let config = config_from_pairs(&[
            ("DBT_ENGINE_STATE_OAUTH_CLIENT_ID", "state-client-id"),
            ("RUN_CACHE_OAUTH_CLIENT_ID", "legacy-client-id"),
            (
                "DBT_ENV_SECRET_STATE_OAUTH_CLIENT_SECRET",
                "state-client-secret",
            ),
            ("RUN_CACHE_OAUTH_CLIENT_SECRET", "legacy-client-secret"),
        ])
        .unwrap();

        assert_eq!(config.oauth_client_id, "state-client-id");
        assert_eq!(
            config.oauth_client_secret.as_deref(),
            Some("state-client-secret")
        );
    }

    #[test]
    fn parses_booleans_and_duration_units() {
        let config = config_from_pairs(&[
            ("RUN_CACHE_API_URL", "localhost:50051"),
            ("RUN_CACHE_API_SECURE", "0"),
            ("RUN_CACHE_API_CLIENT_TIMEOUT", "2m"),
            ("RUN_CACHE_API_CLIENT_MAX_ATTEMPTS", "5"),
            ("RUN_CACHE_FRESHNESS_TOLERANCE", "45min"),
            ("RUN_CACHE_TOLERATE_NONDETERMINISM", "yes"),
            ("RUN_CACHE_ENABLE_LENIENT_DEPENDENCIES", "off"),
            ("RUN_CACHE_ENABLE_RESPONSE_LOGGING", "false"),
            ("RUN_CACHE_ENABLE_DATA_TESTS", "0"),
            ("DBT_ENGINE_STATE_EMIT_REUSED_STATUS", "true"),
            ("RUN_CACHE_RUN_HOOKS_ON_NO_OP", "true"),
            ("RUN_CACHE_COMPARE_UNRENDERED_CODE", "true"),
        ])
        .unwrap();

        assert!(!config.secure);
        assert_eq!(config.timeout, Duration::from_secs(120));
        assert_eq!(config.max_attempts, 5);
        assert_eq!(config.freshness_tolerance_seconds, 2700);
        assert!(config.tolerate_nondeterminism);
        assert!(!config.enable_lenient_dependencies);
        assert!(!config.enable_response_logging);
        assert!(!config.enable_data_tests);
        assert!(config.emit_reused_status);
        assert!(config.run_hooks_on_no_op);
        assert!(config.compare_unrendered_code);
    }

    #[test]
    fn reused_status_legacy_env_is_supported() {
        let config = config_from_pairs(&[("RUN_CACHE_EMIT_REUSED_STATUS", "true")]).unwrap();

        assert!(config.emit_reused_status);
    }

    #[test]
    fn compare_unrendered_code_rejects_unparseable_values() {
        assert!(config_from_pairs(&[("RUN_CACHE_COMPARE_UNRENDERED_CODE", "maybe")]).is_err());
    }

    #[test]
    fn max_attempts_rejects_non_integer_values() {
        let err = config_from_pairs(&[("RUN_CACHE_API_CLIENT_MAX_ATTEMPTS", "lots")]).unwrap_err();

        assert!(err.to_string().contains("API_CLIENT_MAX_ATTEMPTS"));
    }

    #[test]
    fn explicit_secure_config_overrides_api_url_scheme() {
        let config = config_from_pairs(&[
            ("RUN_CACHE_API_URL", "http://localhost:50051"),
            ("RUN_CACHE_API_SECURE", "true"),
        ])
        .unwrap();

        assert!(config.secure);
        assert_eq!(config.endpoint_uri(), "https://localhost:50051");

        let config = config_from_pairs(&[
            ("RUN_CACHE_API_URL", "https://localhost:50051"),
            ("RUN_CACHE_API_SECURE", "false"),
        ])
        .unwrap();

        assert!(!config.secure);
        assert_eq!(config.endpoint_uri(), "http://localhost:50051");
    }

    #[test]
    fn parses_remaining_python_client_config_surface() {
        let config = config_from_pairs(&[
            ("RUN_CACHE_DEFER_TO", "production"),
            ("RUN_CACHE_DEFER_LOG_LEVEL", "debug"),
            ("RUN_CACHE_LOG_FILE_LIMIT", "7"),
            ("RUN_CACHE_LOG_DIR_OVERRIDE", "/tmp/run-cache-logs"),
            ("RUN_CACHE_LOG_PREFIX", "fusion_"),
            ("RUN_CACHE_CLONE_INCREMENTAL_IN_DEV", "always"),
            ("RUN_CACHE_CLONE_TIME_TRAVEL_LIMIT", "4h"),
            ("RUN_CACHE_METADATA_CACHE_TTL", "10m"),
            ("RUN_CACHE_SNOWFLAKE_GET_VIEW_DDL_OVERRIDE", "show views"),
            ("RUN_CACHE_SNOWFLAKE_METADATA_WAREHOUSE", "metadata_wh"),
        ])
        .unwrap();

        assert_eq!(config.defer_to, "production");
        assert_eq!(config.defer_log_level, "debug");
        assert_eq!(config.log_file_limit, 7);
        assert_eq!(
            config.log_dir_override.as_deref(),
            Some("/tmp/run-cache-logs")
        );
        assert_eq!(config.log_prefix, "fusion_");
        assert_eq!(
            config.clone_incremental_in_dev,
            CloneIncrementalInDev::Always
        );
        assert_eq!(config.clone_incremental_in_dev.as_str(), "ALWAYS");
        assert_eq!(config.clone_time_travel_limit_seconds, Some(14400));
        assert_eq!(config.metadata_cache_ttl_seconds, 600);
        assert_eq!(
            config.snowflake_get_view_ddl_override.as_deref(),
            Some("show views")
        );
        assert_eq!(
            config.snowflake_metadata_warehouse.as_deref(),
            Some("metadata_wh")
        );
    }

    #[test]
    fn telemetry_config_excludes_oauth_credentials() {
        let mut config = RunCacheServiceConfig::disabled();
        config.oauth_client_secret = Some("secret".to_string());
        config.org_id = Some("org-1".to_string());

        let telemetry = config.telemetry_config(&test_profile("dev", None));

        assert!(telemetry.fields.contains_key("api_url"));
        assert!(telemetry.fields.contains_key("org_id"));
        assert!(!telemetry.fields.contains_key("oauth_client_id"));
        assert!(!telemetry.fields.contains_key("oauth_client_secret"));
    }

    #[test]
    fn telemetry_config_reports_profile_defer_to_target() {
        let mut config = RunCacheServiceConfig::disabled();
        config.defer_to = "prod".to_string();

        let telemetry = config.telemetry_config(&test_profile("dev", Some("staging")));

        assert_eq!(
            telemetry
                .fields
                .get("defer_to")
                .and_then(|value| match &value.kind {
                    Some(value::Kind::StringValue(value)) => Some(value.as_str()),
                    _ => None,
                }),
            Some("staging")
        );
    }

    #[test]
    fn rejects_unknown_clone_incremental_in_dev() {
        let err =
            config_from_pairs(&[("RUN_CACHE_CLONE_INCREMENTAL_IN_DEV", "sometimes")]).unwrap_err();

        assert!(err.to_string().contains("CLONE_INCREMENTAL_IN_DEV"));
    }

    #[test]
    fn defaults_include_oauth_authorize_endpoint() {
        let config = config_from_pairs(&[]).unwrap();
        assert_eq!(config.oauth_auth_url, "https://auth.state.dbt.com");
    }

    #[test]
    fn oauth_auth_url_can_be_overridden() {
        let config =
            config_from_pairs(&[("RUN_CACHE_AUTH_URL", "https://auth.example.com")]).unwrap();
        assert_eq!(config.oauth_auth_url, "https://auth.example.com");
    }

    #[test]
    fn parses_plural_duration_units_before_bare_seconds_suffix() {
        assert_eq!(
            parse_seconds("API_CLIENT_TIMEOUT", "2 minutes").unwrap(),
            120
        );
        assert_eq!(parse_seconds("API_CLIENT_TIMEOUT", "45mins").unwrap(), 2700);
        assert_eq!(
            parse_seconds("API_CLIENT_TIMEOUT", "3 hours").unwrap(),
            10800
        );
        assert_eq!(
            parse_seconds("API_CLIENT_TIMEOUT", "7days").unwrap(),
            604800
        );
    }

    #[test]
    fn defer_to_target_uses_profile_override_when_set() {
        let config = config_from_pairs(&[]).unwrap();
        let profile = test_profile("dev", Some("staging"));

        assert_eq!(config.defer_to_target(&profile), "staging");
        assert!(!config.is_defer_to_target(&profile));
    }

    #[test]
    fn defer_to_target_falls_back_to_config_when_profile_unset() {
        let config = config_from_pairs(&[("RUN_CACHE_DEFER_TO", "production")]).unwrap();
        let profile = test_profile("dev", None);

        assert_eq!(config.defer_to_target(&profile), "production");
    }

    #[test]
    fn defer_to_target_falls_back_to_config_when_profile_override_empty() {
        let config = config_from_pairs(&[("RUN_CACHE_DEFER_TO", "production")]).unwrap();
        let profile = test_profile("dev", Some(""));

        assert_eq!(config.defer_to_target(&profile), "production");
    }

    #[test]
    fn is_defer_to_target_true_when_target_matches_defer_to() {
        let config = config_from_pairs(&[]).unwrap();
        let profile = test_profile("prod", None);

        assert!(config.is_defer_to_target(&profile));
    }

    #[test]
    fn is_defer_to_target_false_when_target_differs_from_defer_to() {
        let config = config_from_pairs(&[]).unwrap();
        let profile = test_profile("dev", None);

        assert!(!config.is_defer_to_target(&profile));
    }

    #[test]
    fn is_defer_to_target_respects_profile_override() {
        let config = config_from_pairs(&[]).unwrap();
        let profile = test_profile("staging", Some("staging"));

        assert!(config.is_defer_to_target(&profile));
    }

    #[test]
    fn profile_defer_to_target_overrides_legacy_env_config() {
        let mut config = RunCacheServiceConfig::disabled();
        config.defer_to = "legacy_prod".to_string();

        assert_eq!(
            config.defer_to_target(&test_profile("prod", Some("prod"))),
            "prod"
        );
    }

    #[test]
    fn legacy_defer_to_remains_fallback() {
        let mut config = RunCacheServiceConfig::disabled();
        config.defer_to = "legacy_prod".to_string();

        assert_eq!(
            config.defer_to_target(&test_profile("prod", None)),
            "legacy_prod"
        );
    }
}
