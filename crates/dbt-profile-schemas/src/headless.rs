//! Non-interactive (headless) construction of profile configurations.
//!
//! This mirrors the interactive wizard's field schema and value-application
//! logic, but collects values from a caller-supplied map instead of prompting.
//! It never uses `dialoguer`.

use std::collections::HashMap;

use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::profiles::{
    BigqueryDbConfig, ClickHouseDbConfig, DatabricksDbConfig, DbConfig, ExasolDbConfig,
    FabricDbConfig, PostgresDbConfig, RedshiftDbConfig, SnowflakeDbConfig,
};
use dbt_schemas::schemas::serde::StringOrInteger;

use crate::common::{ConfigField, ConfigProcessor, FieldValue, InteractiveSetup};
use crate::fabric_config::default_fabric_config;
use crate::profile::ProfileTarget;

/// The adapters that support headless profile construction, in display order.
///
/// Mirrors `ProfileSetup::get_available_adapters()` in `dbt-init`.
pub fn supported_adapters() -> Vec<AdapterType> {
    vec![
        AdapterType::Snowflake,
        AdapterType::Databricks,
        AdapterType::Bigquery,
        AdapterType::ClickHouse,
        AdapterType::Exasol,
        AdapterType::Postgres,
        AdapterType::Redshift,
        AdapterType::Fabric,
    ]
}

/// Return the declarative field schema for a supported adapter.
pub fn adapter_fields(adapter: AdapterType) -> FsResult<Vec<ConfigField>> {
    let fields = match adapter {
        AdapterType::Snowflake => SnowflakeDbConfig::get_fields(),
        AdapterType::Databricks => DatabricksDbConfig::get_fields(),
        AdapterType::Bigquery => BigqueryDbConfig::get_fields(),
        AdapterType::ClickHouse => ClickHouseDbConfig::get_fields(),
        AdapterType::Exasol => ExasolDbConfig::get_fields(),
        AdapterType::Postgres => PostgresDbConfig::get_fields(),
        AdapterType::Redshift => RedshiftDbConfig::get_fields(),
        AdapterType::Fabric => FabricDbConfig::get_fields(),
        other => {
            return Err(fs_err!(
                ErrorCode::InvalidConfig,
                "Headless profile setup is not supported for adapter '{}'",
                other
            ));
        }
    };
    Ok(fields)
}

/// Apply a map of collected field values to a base config.
///
/// Iterates the config type's declarative fields in order, evaluating each
/// field's visibility condition against the values collected so far (using the
/// same logic as the interactive [`ConfigProcessor`]). For every visible field
/// that has a provided value, the value is set on the config and recorded so
/// later conditions can reference it. Fields with no provided value are skipped.
pub fn apply_values<T: InteractiveSetup>(
    base: &T,
    values: &HashMap<String, FieldValue>,
) -> FsResult<T> {
    let mut config = base.clone();
    let fields = T::get_fields();

    let mut collected_values: HashMap<String, FieldValue> = HashMap::new();

    for field in &fields {
        if !ConfigProcessor::should_show_field(field, &collected_values, &config) {
            continue;
        }

        if let Some(value) = values.get(&field.name) {
            collected_values.insert(field.name.clone(), value.clone());
            config.set_field(&field.name, value.clone())?;
        }
    }

    Ok(config)
}

/// Build a single-target [`ProfileTarget`] for a supported adapter from a map of
/// collected field values.
///
/// The base config, per-adapter, matches the base used by the corresponding
/// interactive `setup_*_profile` function (including its `threads` default).
pub fn build_profile_target(
    adapter: AdapterType,
    target: &str,
    values: &HashMap<String, FieldValue>,
) -> FsResult<ProfileTarget> {
    let db_config = match adapter {
        AdapterType::Snowflake => {
            let mut config = apply_values(&SnowflakeDbConfig::default(), values)?;
            if config.threads.is_none() {
                config.threads = Some(StringOrInteger::Integer(16));
            }
            DbConfig::Snowflake(Box::new(config))
        }
        AdapterType::Databricks => {
            let base = DatabricksDbConfig {
                database: None,
                schema: None,
                host: None,
                http_path: None,
                token: None,
                client_id: None,
                client_secret: None,
                azure_client_id: None,
                azure_client_secret: None,
                azure_tenant_id: None,
                oauth_redirect_url: None,
                oauth_scopes: None,
                query_tags: None,
                session_properties: None,
                connection_parameters: None,
                auth_type: None,
                compute: None,
                connect_retries: None,
                connect_timeout: None,
                retry_all: None,
                connect_max_idle: None,
                threads: None,
            };
            let mut config = apply_values(&base, values)?;
            if config.threads.is_none() {
                config.threads = Some(StringOrInteger::Integer(16));
            }
            DbConfig::Databricks(Box::new(config))
        }
        AdapterType::Bigquery => {
            let base = BigqueryDbConfig {
                threads: None,
                profile_type: None,
                database: None,
                schema: None,
                timeout_seconds: None,
                priority: None,
                method: None,
                maximum_bytes_billed: None,
                impersonate_service_account: None,
                refresh_token: None,
                client_id: None,
                client_secret: None,
                token_uri: None,
                token: None,
                keyfile: None,
                quota_project: None,
                retries: None,
                location: None,
                scopes: None,
                keyfile_json: None,
                execution_project: None,
                api_endpoint: None,
                compute_region: None,
                dataproc_batch: None,
                dataproc_cluster_name: None,
                gcs_bucket: None,
                submission_method: None,
                job_creation_timeout_seconds: None,
                job_execution_timeout_seconds: None,
                reservation: None,
                job_retries: None,
                job_retry_deadline_seconds: None,
                target_name: None,
                workload_pool_provider_path: None,
                service_account_impersonation_url: None,
                token_endpoint: None,
            };
            let mut config = apply_values(&base, values)?;
            if config.threads.is_none() {
                config.threads = Some(StringOrInteger::Integer(16));
            }
            DbConfig::Bigquery(Box::new(config))
        }
        AdapterType::ClickHouse => {
            let mut config = apply_values(&ClickHouseDbConfig::default(), values)?;
            if config.threads.is_none() {
                config.threads = Some(StringOrInteger::Integer(16));
            }
            DbConfig::ClickHouse(Box::new(config))
        }
        AdapterType::Exasol => {
            let mut config = apply_values(&ExasolDbConfig::default(), values)?;
            if config.threads.is_none() {
                config.threads = Some(StringOrInteger::Integer(16));
            }
            DbConfig::Exasol(Box::new(config))
        }
        AdapterType::Postgres => {
            let mut config = apply_values(&PostgresDbConfig::default(), values)?;
            if config.threads.is_none() {
                config.threads = Some(StringOrInteger::Integer(16));
            }
            DbConfig::Postgres(Box::new(config))
        }
        AdapterType::Redshift => {
            let base = RedshiftDbConfig {
                port: None,
                database: None,
                schema: None,
                connect_timeout: None,
                sslmode: None,
                role: None,
                autocreate: None,
                db_groups: None,
                ra3_node: None,
                datasharing: None,
                drop_without_cascade: None,
                autocommit: None,
                retries: None,
                method: None,
                host: None,
                user: None,
                password: None,
                iam_profile: None,
                access_key_id: None,
                secret_access_key: None,
                cluster_id: None,
                region: None,
                is_serverless: None,
                serverless_work_group: None,
                serverless_acct_id: None,
                threads: None,
                token_endpoint: None,
                idc_client_display_name: None,
                idc_region: None,
                idp_response_timeout: None,
                issuer_url: None,
                idp_listen_port: None,
            };
            let mut config = apply_values(&base, values)?;
            if config.threads.is_none() {
                config.threads = Some(StringOrInteger::Integer(16));
            }
            DbConfig::Redshift(Box::new(config))
        }
        AdapterType::Fabric => {
            // Fabric's interactive setup does not apply a `threads` default.
            let config = apply_values(&default_fabric_config(), values)?;
            DbConfig::Fabric(Box::new(config))
        }
        other => {
            return Err(fs_err!(
                ErrorCode::InvalidConfig,
                "Headless profile setup is not supported for adapter '{}'",
                other
            ));
        }
    };

    let mut outputs = HashMap::new();
    outputs.insert(target.to_string(), db_config);

    Ok(ProfileTarget {
        target: target.to_string(),
        outputs,
    })
}
