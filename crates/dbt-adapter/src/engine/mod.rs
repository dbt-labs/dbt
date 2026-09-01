use crate::adapter::adapter_impl::{DEFAULT_BASE_BEHAVIOR_FLAGS, adapter_specific_behavior_flags};

use arrow::array::RecordBatch;
use dbt_adapter_core::AdapterType;
use dbt_adbc::*;
use dbt_common::AdapterResult;
use dbt_common::behavior_flags::Behavior;
use dbt_common::cancellation::CancellationToken;
use minijinja::State;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::{thread, time::Duration};

mod adapter_engine;
mod databricks_query_tags;
pub use adapter_engine::AdapterEngine;
pub use adapter_engine::Options;

mod concat_batches;

pub mod query_comment;
pub mod retry;

mod adbc;
pub(crate) mod clickhouse;
mod databricks;
pub mod duckdb_attach;
pub use adbc::AdbcEngine;
pub(crate) use adbc::resolve_connection_config;

mod noop_connection;
pub use noop_connection::NoopConnection;

mod record_replay;
pub use record_replay::RecordReplayEngine;

mod sidecar;
pub use sidecar::SidecarEngine;

mod sidecar_client;
pub use sidecar_client::ColumnInfo;
pub use sidecar_client::SidecarClient;

/// Behavior flags from dbt-core that have been removed in Fusion.
/// The new behavior is always enabled; these flags are accepted but ignored.
/// See: https://docs.getdbt.com/reference/global-configs/behavior-changes
const REMOVED_IN_FUSION: &[&str] = &[
    // dbt-core flags
    // ---
    "use_info_schema_for_columns",
    "require_explicit_package_overrides_for_builtin_materializations",
    "require_resource_names_without_spaces",
    "source_freshness_run_project_hooks",
    "skip_nodes_if_on_run_start_fails",
    "state_modified_compare_more_unrendered_values",
    "require_yaml_configuration_for_mf_time_spines",
    "require_nested_cumulative_type_params",
    "require_generic_test_arguments_property",
    "require_unique_project_resource_names",
    "require_valid_schema_from_generate_schema_name",
    "require_sql_header_in_test_configs",
    "require_batched_execution_for_custom_microbatch_strategy",
    // dbt-redshift flags
    // ---
    // metadata adapter already uses SVV views
    "restrict_direct_pg_catalog_access",
    // dbt-bigquery flags
    // ---
    // metadata adapter already uses batched __TABLES__ queries for freshness.
    "bigquery_use_batch_source_freshness",
];

pub(crate) fn make_behavior(
    adapter_type: AdapterType,
    behavior_flag_overrides: &BTreeMap<String, bool>,
) -> Arc<Behavior> {
    let mut behavior_flags = adapter_specific_behavior_flags(adapter_type);
    for flag in DEFAULT_BASE_BEHAVIOR_FLAGS.iter() {
        behavior_flags.push(flag.clone());
    }
    for key in behavior_flag_overrides.keys() {
        if !behavior_flags.iter().any(|f| f.name == key)
            && REMOVED_IN_FUSION.contains(&key.as_str())
        {
            // Suppressed until dbt Core v1.12 ships support for Fusion warning names in
            // warn_error_options, so users running both engines can silence cross-engine
            // warnings without breaking Core.
            // emit_warn_log_message(
            //     ErrorCode::InvalidConfig,
            //     format!(
            //         "Behavior flag '{key}' has been removed in dbt Fusion. \
            //          This flag can be safely removed from your dbt_project.yml."
            //     ),
            //     None,
            // );
        }
    }
    Arc::new(Behavior::new(behavior_flags, behavior_flag_overrides))
}

/// Execute query and retry in case of an error. Retry is done (up to
/// the given limit) regardless of the error encountered.
///
/// https://github.com/dbt-labs/dbt-adapters/blob/996a302fa9107369eb30d733dadfaf307023f33d/dbt-adapters/src/dbt/adapters/sql/connections.py#L84
#[allow(clippy::too_many_arguments)]
pub fn execute_query_with_retry(
    engine: Arc<dyn AdapterEngine>,
    state: Option<&State>,
    conn: &'_ mut dyn Connection,
    ctx: &QueryCtx,
    sql: &str,
    retry_limit: u32,
    options: &Options,
    fetch: bool,
    token: CancellationToken,
) -> AdapterResult<RecordBatch> {
    let mut attempt = 0;
    let mut last_error = None;

    while attempt < retry_limit {
        match engine.execute_with_options(
            state,
            ctx,
            conn,
            sql,
            options.clone(),
            fetch,
            token.clone(),
        ) {
            Ok(result) => return Ok(result),
            Err(err) => {
                last_error = Some(err.clone());
                attempt += 1;
                if attempt < retry_limit {
                    // Only back off when another attempt will follow.
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }

    if let Some(err) = last_error {
        Err(err)
    } else {
        unreachable!("last_error should not be None if we exit the loop")
    }
}
