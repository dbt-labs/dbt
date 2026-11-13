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
    git_is_dirty: Option<i32>,
    vars_override: Option<String>,
    start: std::time::Instant,
}

/// Names that mark a `--vars` value as a secret, matched anywhere in the key.
const SECRET_VAR_MARKERS: &[&str] = &["secret", "password", "passwd", "token", "key", "credential"];

/// Masks `value` when `name` looks like a secret, else masks the secret-named keys nested in it.
fn mask_secret_var(name: &str, value: serde_json::Value) -> serde_json::Value {
    let lower = name.to_lowercase();
    if SECRET_VAR_MARKERS.iter().any(|m| lower.contains(m)) {
        return serde_json::Value::String("***".to_string());
    }
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(k, v)| {
                    let v = mask_secret_var(&k, v);
                    (k, v)
                })
                .collect(),
        ),
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .into_iter()
                .map(|v| mask_secret_var(name, v))
                .collect(),
        ),
        other => other,
    }
}

/// The `--vars` a run was given as a JSON object, with the value of any var whose name looks
/// like a secret replaced, or `None` when there were none. The record is kept in the project's
/// `target/` directory and published in the information schema, so a secret passed on the
/// command line must not land in it.
pub(crate) fn vars_override_json<V: serde::Serialize>(
    vars: &BTreeMap<String, V>,
) -> Option<String> {
    if vars.is_empty() {
        return None;
    }
    let masked: serde_json::Map<String, serde_json::Value> = vars
        .iter()
        .map(|(name, value)| {
            let value = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
            (name.clone(), mask_secret_var(name, value))
        })
        .collect();
    serde_json::to_string(&masked).ok()
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
            .and_then(|cwd| dbt_metadata::partial_parse::read_git_info_cached(&cwd));
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
            git_is_dirty: git.as_ref().map(|g| i32::from(g.is_dirty)),
            vars_override: None,
            start: std::time::Instant::now(),
        }
    }

    /// Records the `--vars` the run was given, with secrets masked.
    pub fn with_vars<V: serde::Serialize>(mut self, vars: &BTreeMap<String, V>) -> Self {
        self.vars_override = vars_override_json(vars);
        self
    }

    /// Writes the record. `facts` is what the run resolved about its target, when it got that
    /// far; the fields stay unset otherwise.
    pub fn write(self, status: &str, facts: Option<&dbt_schemas::schemas::InvocationFacts>) {
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
            adapter_type: facts.and_then(|f| f.adapter_type.clone()),
            // The resolved target; the flag only when the run never got that far.
            target_name: facts
                .and_then(|f| f.target_name.clone())
                .or(self.target_name),
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
            git_is_dirty: self.git_is_dirty,
            elapsed_secs: Some(elapsed_secs),
            node_count: facts.and_then(|f| f.node_count),
            target_database: facts.and_then(|f| f.target_database.clone()),
            target_schema: facts.and_then(|f| f.target_schema.clone()),
            target_threads: facts.and_then(|f| f.target_threads),
            vars_override: self.vars_override,
            ingested_at,
        };
        if let Err(e) = write_invocation(&self.invocations_dir, row) {
            eprintln!("warning: failed to write invocation parquet: {e}");
        }
    }
}

/// Returns free heap memory to the OS.
///
/// glibc keeps freed memory resident, so the process keeps the footprint of a
/// finished phase. Call this after a phase drops its working state and before
/// the next phase allocates.
pub(crate) fn release_free_heap_memory() {
    // SAFETY: `malloc_trim` only releases memory that no allocation uses.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
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
                batch_results: result
                    .batch_results
                    .as_ref()
                    .and_then(|b| serde_json::to_string(b).ok()),
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
    resolved_state: &dbt_schemas::state::ResolverState,
) {
    if macro_depends_on.is_empty() {
        return;
    }

    for (unique_id, node) in dbt_manifest.nodes.iter_mut() {
        let Some(macros) = macro_depends_on.get(unique_id) else {
            continue;
        };
        let base = match node {
            DbtNode::Model(node) => &mut node.__base_attr__,
            DbtNode::Test(node) => &mut node.__base_attr__,
            DbtNode::Snapshot(node) => &mut node.__base_attr__,
            DbtNode::Seed(node) => &mut node.__base_attr__,
            DbtNode::Analysis(node) => &mut node.__base_attr__,
            DbtNode::Operation(node) => &mut node.__base_attr__,
            DbtNode::Function(node) => &mut node.__base_attr__,
            DbtNode::Check(_) => continue,
        };
        let mut dependencies = macros.clone();
        if resolved_state
            .nodes
            .get_node(unique_id)
            .is_some_and(|node| node.node_adapter() != resolved_state.adapter_type)
        {
            // Shared macro typechecking cannot describe each caller's dispatch.
            dependencies.extend(base.depends_on.macros.iter().cloned());
        }
        base.depends_on.macros = dependencies.into_iter().collect();
    }
    for (unique_id, macro_node) in dbt_manifest.macros.iter_mut() {
        if let Some(macros) = macro_depends_on.get(unique_id) {
            macro_node.depends_on.macros = macros.iter().cloned().collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vars_override_masks_secret_looking_names() {
        let vars: BTreeMap<String, serde_json::Value> = [
            ("start_date", serde_json::json!("2024-01-01")),
            ("limit", serde_json::json!(5)),
            ("API_Token", serde_json::json!("abc")),
            ("db_password", serde_json::json!("hunter2")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let got: serde_json::Value =
            serde_json::from_str(&vars_override_json(&vars).unwrap()).unwrap();
        assert_eq!(
            got,
            serde_json::json!({
                "start_date": "2024-01-01",
                "limit": 5,
                "API_Token": "***",
                "db_password": "***",
            })
        );
    }

    #[test]
    fn vars_override_masks_secrets_nested_in_a_value() {
        let vars: BTreeMap<String, serde_json::Value> = [(
            "creds".to_string(),
            serde_json::json!({"user": "bob", "password": "hunter2", "more": [{"api_token": "x"}]}),
        )]
        .into_iter()
        .collect();
        let got: serde_json::Value =
            serde_json::from_str(&vars_override_json(&vars).unwrap()).unwrap();
        assert_eq!(
            got,
            serde_json::json!({"creds": {"user": "bob", "password": "***", "more": [{"api_token": "***"}]}})
        );
    }

    #[test]
    fn vars_override_is_none_without_vars() {
        let none: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        assert!(vars_override_json(&none).is_none());
    }
}
