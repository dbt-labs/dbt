//! Support for the `dbt state explain` command.

mod log;
mod paths;
mod render;
mod select;
mod service;
mod types;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use dbt_common::FsResult;

use crate::{service_client::GrpcRunCacheServiceClient, service_config::RunCacheServiceConfig};

pub use log::{append_state_explain_log_record, read_explain_records, read_state_explain_log};
pub use paths::{new_state_explain_log_path, prune_state_explain_logs};
pub use render::{
    render_explain_records, render_merged_explain_records, render_service_explain_messages,
    render_service_explain_response,
};
pub use service::{
    EXPLAIN_MAX_BATCH_SIZE, execution_decision_ids, service_explain_response_with_client,
};
pub use types::{
    StateExplainDevClone, StateExplainExecutionConfirmed, StateExplainLog, StateExplainLogRecord,
    StateExplainNode, StateExplainNodeInfo, StateExplainOptions, StateExplainRecord,
    StateExplainRunConfig, StateExplainRunStart, StateExplainStatus,
};

use self::{
    log::read_input_for_state_explain,
    paths::{resolve_log_file, state_explain_log_config_from_env},
    render::render_explain_output,
    select::{filter_explain_records, sort_explain_records},
    service::{
        execution_ids, service_explain_response_for_ids, should_fetch_service_explain,
        upstream_dependency_changes_for_ids,
    },
};

/// Resolve inputs for `dbt state explain`.
pub async fn execute_state_explain(options: StateExplainOptions) -> FsResult<()> {
    let log_config = state_explain_log_config_from_env();
    let Some(log_file) = resolve_log_file(&options, &log_config)? else {
        println!("No log files found.");
        return Ok(());
    };
    let input = read_input_for_state_explain(&log_file)?;
    let mut records = filter_explain_records(input.records, &options)?;
    sort_explain_records(&mut records);
    let execution_decision_ids = execution_decision_ids(&records);
    let client = if should_fetch_service_explain(&execution_decision_ids, &options) {
        connect_state_explain_client().await
    } else {
        None
    };

    let mut service_response = None;
    let mut upstream_changes = HashMap::new();
    if let Some(client) = &client {
        match service_explain_response_for_ids(client, &execution_decision_ids).await {
            Ok(response) => service_response = Some(response),
            Err(err) => tracing::warn!(
                "Failed to fetch dbt State service explain messages for {} execution decision id(s), falling back to local records: {err}",
                execution_decision_ids.len()
            ),
        }

        let execution_ids = execution_ids(&records);
        if options.very_verbose {
            match upstream_dependency_changes_for_ids(client, &execution_ids).await {
                Ok(changes) => upstream_changes = changes,
                Err(err) => tracing::warn!(
                    "Failed to fetch dbt State service upstream dependency changes for {} execution id(s): {err}",
                    execution_ids.len()
                ),
            }
        }
    }

    println!(
        "{}",
        render_explain_output(
            &records,
            service_response.as_ref(),
            &upstream_changes,
            input.run_start.as_ref(),
            &options,
        )
    );

    Ok(())
}

async fn connect_state_explain_client() -> Option<GrpcRunCacheServiceClient> {
    if RunCacheServiceConfig::is_explicitly_disabled_from_env() {
        return None;
    }
    let service_config =
        run_cache_service_config_for_state_explain(|name| std::env::var(name).ok())?;
    GrpcRunCacheServiceClient::connect(service_config)
        .await
        .inspect_err(|err| {
            tracing::warn!(
                "Failed to connect to the dbt State service, falling back to local records: {err}"
            )
        })
        .ok()
}

fn run_cache_service_config_for_state_explain<F>(get_env: F) -> Option<RunCacheServiceConfig>
where
    F: FnMut(&str) -> Option<String>,
{
    match RunCacheServiceConfig::from_env_getter(get_env) {
        Ok(config) => config.enabled.then_some(config),
        Err(err) => {
            tracing::warn!("Failed to resolve the dbt State service configuration: {err}");
            None
        }
    }
}
