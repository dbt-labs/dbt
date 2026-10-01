use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::Utc;
use dbt_adapter::response::AdapterResponse;
use dbt_common::ErrorCode;
use dbt_common::io_args::EvalArgs;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_schemas::schemas::RunResultOutput;
use dbt_schemas::schemas::manifest::{DbtManifest, DbtNode};
use dbt_schemas::stats::Stats;
use dbt_tasks_core::generate_run_results;

/// Minimal context captured before `SystemArgs` is moved into the async runtime.
/// Passed back to `run_cli` so it can write the invocation record unconditionally at exit,
/// regardless of which exit path (success / error / NoFilesChanged / early checkpoint) was taken.
pub struct InvocationContext {
    invocations_dir: std::path::PathBuf,
    invocation_id: String,
    command: String,
    selector: Option<String>,
    cli_args: Vec<String>,
    target_name: Option<String>,
    profile_name: Option<String>,
    git_sha: Option<String>,
    git_branch: Option<String>,
    start: std::time::Instant,
}

impl InvocationContext {
    pub fn new(
        metadata_dir: std::path::PathBuf,
        io: &dbt_common::io_args::IoArgs,
        command: dbt_common::io_args::FsCommand,
        common_args: &dbt_clap_core::CommonArgs,
    ) -> Self {
        let selector = common_args.select.as_ref().map(|v| v.join(" "));
        let git = std::env::current_dir()
            .ok()
            .and_then(|cwd| dbt_metadata::partial_parse::read_git_info(&cwd));
        Self {
            invocations_dir: metadata_dir.join("run").join("invocations"),
            invocation_id: io.invocation_id.to_string(),
            command: command.as_str().to_string(),
            selector,
            cli_args: std::env::args().collect(),
            target_name: common_args.target.clone(),
            profile_name: common_args.profile.clone(),
            git_sha: git
                .as_ref()
                .map(|g| g.sha.clone())
                .filter(|s| !s.is_empty()),
            git_branch: git
                .as_ref()
                .map(|g| g.branch.clone())
                .filter(|s| !s.is_empty()),
            start: std::time::Instant::now(),
        }
    }

    pub fn write(self, status: &str) {
        use dbt_metadata_parquet::invocations::{InvocationRow, write_invocation};
        let elapsed_secs = self.start.elapsed().as_secs_f64();
        let ingested_at: i64 = Utc::now().timestamp_micros();
        let row = InvocationRow {
            invocation_id: self.invocation_id,
            command: self.command,
            status: status.to_string(),
            selector: self.selector,
            cli_args: self.cli_args,
            project_name: None,
            adapter_type: None,
            target_name: self.target_name,
            profile_name: self.profile_name,
            environment_id: None,
            environment_name: None,
            account_identifier: None,
            defer_env_id: None,
            user_id: None,
            user_name: None,
            dbt_version: env!("CARGO_PKG_VERSION").to_string(),
            git_sha: self.git_sha,
            git_branch: self.git_branch,
            git_is_dirty: None,
            elapsed_secs: Some(elapsed_secs),
            // TODO: populate from resolved_state.nodes.len() after compilation.
            // Currently NULL because the single write site (execute_fs_and_shutdown) has
            // no access to the node count. Nobody queries this column from parquet yet —
            // dbt-index timings reads from dbt_rt.invocations (DuckDB), not this file.
            node_count: None,
            ingested_at,
        };
        if let Err(e) = write_invocation(&self.invocations_dir, row) {
            eprintln!("warning: failed to write invocation parquet: {e}");
        }
    }
}

/// Write runtime result rows to the metadata/runtime/results parquet directory.
/// Called alongside `write_run_results_json` at end of run/test/build.
pub(crate) fn write_runtime_results_parquet(
    stats: &Stats,
    adapter_responses: &HashMap<String, AdapterResponse>,
    arg: &EvalArgs,
) {
    use dbt_metadata_parquet::runtime_results::{RuntimeResultRow, write_runtime_results};

    let results_dir = arg.metadata_dir().join("run").join("results");

    let ingested_at: i64 = Utc::now().timestamp_micros();

    if stats.nodes.is_none() {
        return;
    }

    let rows: Vec<RuntimeResultRow> = stats
        .stats
        .iter()
        .map(|stat| {
            let result: RunResultOutput =
                generate_run_results(stat, stats, adapter_responses).into();
            RuntimeResultRow {
                invocation_id: arg.io.invocation_id.to_string(),
                unique_id: result.unique_id.clone(),
                status: result.status.clone(),
                message: result.message.clone(),
                execution_time: Some(result.execution_time),
                thread_id: Some(result.thread_id.clone()),
                failures: result.failures,
                compiled_code_hash: None,
                relation_name: result.relation_name.clone(),
                adapter_response: Some(
                    serde_json::to_string(&result.adapter_response).unwrap_or_default(),
                ),
                timing: Some(serde_json::to_string(&result.timing).unwrap_or_default()),
                ingested_at,
            }
        })
        .collect();

    if let Err(e) = write_runtime_results(&results_dir, &rows) {
        emit_warn_log_message(
            ErrorCode::IoError,
            format!("Failed to write runtime results parquet: {e}"),
        );
    }
}

/// Write catalog stat rows to the metadata/run/catalog_stats parquet directory.
/// Called after a catalog fetch when `write_metadata=true`.
pub(crate) async fn write_catalog_stats_parquet(
    catalog: &dbt_schemas::schemas::legacy_catalog::DbtCatalog,
    arg: &EvalArgs,
) {
    use chrono::Utc;
    use dbt_metadata_parquet::catalog_stats::{CatalogStatEpochRow, write_catalog_stats};

    let dir = arg.metadata_dir().join("run").join("catalog_stats");
    let ingested_at: i64 = Utc::now().timestamp_micros();

    let rows: Vec<CatalogStatEpochRow> = catalog
        .nodes
        .iter()
        .chain(catalog.sources.iter())
        .filter_map(|(uid, table)| {
            let row_count = table.stats.get("row_count").and_then(|s| {
                s.value
                    .as_i64()
                    .or_else(|| s.value.as_f64().map(|f| f as i64))
            });
            let bytes = table.stats.get("bytes").and_then(|s| {
                s.value
                    .as_i64()
                    .or_else(|| s.value.as_f64().map(|f| f as i64))
            });
            let last_modified = table
                .stats
                .get("last_modified")
                .and_then(|s| s.value.as_str().map(|v| v.to_string()));

            // Only write rows where we have at least one useful stat
            if row_count.is_none() && bytes.is_none() && last_modified.is_none() {
                return None;
            }

            let meta = &table.metadata;
            Some(CatalogStatEpochRow {
                unique_id: uid.clone(),
                table_type: Some(meta.materialization_type.clone()).filter(|s| !s.is_empty()),
                table_owner: meta.owner.clone(),
                database_name: meta.database.clone(),
                schema_name: Some(meta.schema.clone()).filter(|s| !s.is_empty()),
                table_name: Some(meta.name.clone()).filter(|s| !s.is_empty()),
                row_count,
                bytes,
                last_modified,
                ingested_at,
            })
        })
        .collect();

    if rows.is_empty() {
        return;
    }

    if let Err(e) = write_catalog_stats(&dir, &rows) {
        emit_warn_log_message(
            ErrorCode::IoError,
            format!("Failed to write catalog stats parquet: {e}"),
        );
    }
}

pub(crate) fn update_manifest_with_macro_depends_on(
    dbt_manifest: &mut DbtManifest,
    macro_depends_on: &BTreeMap<String, BTreeSet<String>>,
) {
    if macro_depends_on.is_empty() {
        return;
    }

    for (unique_id, node) in dbt_manifest.nodes.iter_mut() {
        if let DbtNode::Model(model_node) = node
            && let Some(macros) = macro_depends_on.get(unique_id)
        {
            model_node.__base_attr__.depends_on.macros = macros.iter().cloned().collect();
        }
        if let DbtNode::Test(test_node) = node
            && let Some(macros) = macro_depends_on.get(unique_id)
        {
            test_node.__base_attr__.depends_on.macros = macros.iter().cloned().collect();
        }
        if let DbtNode::Snapshot(snapshot_node) = node
            && let Some(macros) = macro_depends_on.get(unique_id)
        {
            snapshot_node.__base_attr__.depends_on.macros = macros.iter().cloned().collect();
        }
        if let DbtNode::Seed(seed_node) = node
            && let Some(macros) = macro_depends_on.get(unique_id)
        {
            seed_node.__base_attr__.depends_on.macros = macros.iter().cloned().collect();
        }
        if let DbtNode::Analysis(analysis_node) = node
            && let Some(macros) = macro_depends_on.get(unique_id)
        {
            analysis_node.__base_attr__.depends_on.macros = macros.iter().cloned().collect();
        }
        if let DbtNode::Operation(operation_node) = node
            && let Some(macros) = macro_depends_on.get(unique_id)
        {
            operation_node.__base_attr__.depends_on.macros = macros.iter().cloned().collect();
        }
    }
    for (unique_id, macro_node) in dbt_manifest.macros.iter_mut() {
        if let Some(macros) = macro_depends_on.get(unique_id) {
            macro_node.depends_on.macros = macros.iter().cloned().collect();
        }
    }
}
