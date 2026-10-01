//! Pure-Rust parquet-backed incremental parse cache.
//!
//! Replaces `duckdb_incremental.rs` with zero runtime dependencies on DuckDB / adbc.
//!
//! # Layout
//!
//! ```text
//! target/parse_state/
//!   project.parquet              — project-level key/value pairs (profile, vars, deps, kinds, …)
//!   filestamps.parquet           — per-package file timestamps
//!   alive.parquet                — snapshot of all currently-alive unique_ids
//!   nodes/v1_0.parquet           — base epoch (or compacted)
//!   nodes/v1_1.parquet           — delta epoch
//!   nodes/v1_2.parquet           …
//! ```
//!
//! # Epoch append strategy for nodes
//!
//! - Cold start (`changed_nodes = None`) or full-rewrite: write `nodes/0.parquet`,
//!   delete all other epoch files.
//! - Incremental (`changed_nodes = Some(set)`): write a new `nodes/N.parquet`
//!   with only the changed rows.
//! - Read: collect all epoch files in order, latest epoch wins by `unique_id`.
//! - Compact: when delta row count > 10% of alive nodes OR file count > 8,
//!   merge all into `nodes/0.parquet` and delete the rest.
//!
//! # Liveness invariant
//!
//! A node is alive if and only if its `unique_id` appears in the latest-epoch-wins
//! merge of all epoch files. There are no tombstones. This is safe because any event
//! that can delete a node (file deleted, `.yml` changed, macro changed, config changed)
//! triggers a FullParse, which rewrites `nodes/0.parquet` from scratch containing only
//! the currently-alive nodes. Absence from the merged view means the node was deleted.
//! The incremental path (delta epochs) is only taken when a model/analysis `.sql` file
//! changes — a 1:1 mapping to a single node — so no deletion can occur on that path.
//!
//! # No SQL, no DuckDB
//!
//! All filtering and graph traversal that used SQL is done in Rust over the
//! deserialized row vectors.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

// ── timing helper (enabled via DBT_PP_TIMING=1) ───────────────────────────────
fn t(label: &str, start: Instant) {
    if std::env::var_os("DBT_PP_TIMING").is_some() {
        eprintln!(
            "[pp] {:>7.2}ms  {label}",
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
}

use arrow::datatypes::{DataType, Field, FieldRef, TimeUnit};
use dbt_common::path::DbtPath;
use parquet::arrow::{ProjectionMask, arrow_reader::ParquetRecordBatchReaderBuilder};
use serde::{Deserialize, Serialize};

use dbt_schemas::{
    schemas::{
        DbtAnalysis, DbtCheck, DbtExposure, DbtFunction, DbtModel, DbtSeed, DbtSnapshot, DbtSource,
        DbtTest, DbtUnitTest, Nodes,
        common::ConstraintType,
        macros::{DbtDocsMacro, DbtMacro, MacroArgument},
        manifest::{DbtMetric, DbtOperation, DbtSavedQuery, DbtSemanticModel},
        nodes::{DbtGroup, InternalDbtNodeAttributes},
    },
    state::{DbtPackage, Macros, ManifestPathConfig, Operations, ResourcePathKind},
};

use crate::partial_parse::PackageSnapshot;

// ── constants ─────────────────────────────────────────────────────────────────

pub const CACHE_DIR_NAME: &str = "private/metadata/parse";

const SCHEMA_VERSION: u32 = 1;

// ── path helpers ──────────────────────────────────────────────────────────────

pub fn cache_dir(out_dir: &Path) -> PathBuf {
    out_dir.join(CACHE_DIR_NAME)
}

/// Path to the generation token: written only on cold start (changed_nodes == None).
/// Its `ingested_at` is the timestamp of the last cold start; compile rows are stale
/// when their `ingested_at` is older than this value.
pub fn generation_path(dir: &Path) -> PathBuf {
    dir.join("generation.parquet")
}

pub(crate) fn resolver_state_path(dir: &Path) -> PathBuf {
    dir.join("resolver_state.parquet")
}

fn filestamps_path(dir: &Path) -> PathBuf {
    dir.join("filestamps.parquet")
}

pub(crate) fn nodes_dir(dir: &Path) -> PathBuf {
    dir.join("nodes")
}

fn version_prefix() -> String {
    format!("v{}_", SCHEMA_VERSION)
}

fn node_epoch_path(dir: &Path, epoch: u32) -> PathBuf {
    nodes_dir(dir).join(format!("{}{epoch}.parquet", version_prefix()))
}

/// Return sorted list of existing epoch numbers from the nodes/ subdirectory.
fn existing_epochs(dir: &Path) -> Vec<u32> {
    let prefix = version_prefix();
    dbt_metadata_parquet::epoch_io::existing_epochs(&nodes_dir(dir), &prefix)
        .into_iter()
        .map(|(n, _)| n)
        .collect()
}

/// Returns `(epoch_number, path)` pairs for the nodes directory.
pub(crate) fn existing_epoch_paths(dir: &Path) -> Vec<(u32, PathBuf)> {
    let prefix = version_prefix();
    dbt_metadata_parquet::epoch_io::existing_epochs(&nodes_dir(dir), &prefix)
}

fn next_epoch(dir: &Path) -> u32 {
    let prefix = version_prefix();
    dbt_metadata_parquet::epoch_io::next_epoch(&nodes_dir(dir), &prefix)
}

pub(crate) fn system_time_to_nanos(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos() as u64
}

// ── row types (Serialize + Deserialize → serde_arrow) ────────────────────────

/// Single-row record written only on cold start. Its `ingested_at` is the timestamp
/// of the last cold start; compile rows with an older `ingested_at` are stale.
/// The other fields record what triggered the cold start — they don't change between
/// cold starts, so dbt-index ignores them and reads only `ingested_at`.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct GenerationRow {
    pub ingested_at: i64,
    pub version: String,
    pub dbt_version: String,
    pub project_name: String,
    pub adapter_type: String,
    pub vars_json: String,
    pub env_vars_json: String,
    pub profile_hash: String,
    pub profile_file_hash: String,
    pub project_file_hash: String,
    pub cli_vars_hash: String,
    pub packages_lock_hash: String,
}

/// Single-row record written every parse (cold and incremental). Contains resolver
/// output — data that may change on any parse, not just cold starts.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ResolverStateRow {
    pub ingested_at: i64,
    pub dbt_profile_json: String,
    pub vars_json: String,
    pub env_vars_json: String,
    pub get_relation_calls_json: String,
    pub get_columns_in_relation_calls_json: String,
    pub patterned_dangling_sources_json: String,
    pub nodes_with_resolution_errors_json: String,
    pub nodes_with_access_errors_json: String,
    pub operations_json: String,
    pub selectors_json: String,
    pub git_sha: String,
    pub git_branch: String,
    pub git_is_dirty: i32,
    pub pkg_deps_json: String,
    pub pkg_kinds_json: String,
    #[serde(default)]
    pub pkg_manifest_path_configs_json: String,
    pub any_uses_graph: i32,
}

#[derive(Serialize, Deserialize, Clone)]
struct FilestampRow {
    package_name: String,
    package_root: String,
    path_kind: String,
    path: String,
    mtime_ns: i64,
    #[serde(default)]
    ingested_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub(crate) struct NodeRow {
    pub unique_id: String,
    pub is_disabled: i32,
    pub resource_type: String,
    pub name: String,
    pub package_name: String,
    pub original_path: String,
    pub fqn: Vec<String>,
    pub tags: Vec<String>,
    pub depends_on: Vec<String>,
    pub uses_graph: i32,
    pub materialization: String,
    pub payload: String,
    pub ingested_at: i64,
    pub extended_model: i32,
    // Macro-only columns: fields that have #[serde(skip*)] so they are absent from the
    // JSON payload. Non-macro rows carry empty string defaults and ignore these on read.
    pub macro_arguments_json: String, // JSON array of MacroArgument — "" for non-macros
    pub macro_span_json: String,      // JSON-encoded Span (all 6 fields) — "" if absent
    pub macro_name_span_json: String, // JSON-encoded macro_name_span — "" if absent
    pub macro_absolute_path: String,  // absolute_path as lossy UTF-8 string — "" if absent
    // Promoted fields: redundant indexes into payload for DuckDB filter pushdown.
    // Nullable (Option) — backward-compatible with old epoch files (serde_arrow reads NULL).
    pub description: Option<String>,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub alias: Option<String>,
    pub relation_name: Option<String>,
    pub access: Option<String>,
    pub group_name: Option<String>,
    pub source_name: Option<String>,
    pub identifier: Option<String>,
}

/// Index-only view of NodeRow — columns needed for selector evaluation, excluding `payload`.
/// Used by resolve_unique_ids_from_index with column projection so the
/// payload column bytes are never read from disk.
#[derive(Deserialize)]
pub(crate) struct NodeIndexRow {
    pub unique_id: String,
    pub original_path: String,
    pub resource_type: String,
    pub name: String,
    pub package_name: String,
    pub fqn: Vec<String>,
    pub tags: Vec<String>,
    pub depends_on: Vec<String>,
    /// Kept for future lazy-load logic that distinguishes ephemeral / view materializations.
    #[allow(dead_code)]
    pub materialization: String,
}

// ── parquet read/write helpers ─────────────────────────────────────────────────

fn str_field(name: &str) -> FieldRef {
    Arc::new(Field::new(name, DataType::Utf8, false))
}

fn i32_field(name: &str) -> FieldRef {
    Arc::new(Field::new(name, DataType::Int32, false))
}

fn i64_field(name: &str) -> FieldRef {
    Arc::new(Field::new(name, DataType::Int64, false))
}

fn timestamp_micros_field(name: &str) -> FieldRef {
    Arc::new(Field::new(
        name,
        DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC"))),
        false,
    ))
}

fn write_rows_with_fields<T: Serialize>(path: &Path, fields: &[FieldRef], rows: &[T]) -> bool {
    let plain_fields: Vec<Field> = fields.iter().map(|f| f.as_ref().clone()).collect();
    dbt_metadata_parquet::epoch_io::write_rows(path, &plain_fields, rows).is_ok()
}

fn read_rows<T: serde::de::DeserializeOwned>(path: &Path) -> Vec<T> {
    dbt_metadata_parquet::epoch_io::read_rows(path)
}

fn generation_fields() -> Vec<FieldRef> {
    vec![
        timestamp_micros_field("ingested_at"),
        str_field("version"),
        str_field("dbt_version"),
        str_field("project_name"),
        str_field("adapter_type"),
        str_field("vars_json"),
        str_field("env_vars_json"),
        str_field("profile_hash"),
        str_field("profile_file_hash"),
        str_field("project_file_hash"),
        str_field("cli_vars_hash"),
        str_field("packages_lock_hash"),
    ]
}

fn resolver_state_fields() -> Vec<FieldRef> {
    vec![
        timestamp_micros_field("ingested_at"),
        str_field("dbt_profile_json"),
        str_field("vars_json"),
        str_field("env_vars_json"),
        str_field("get_relation_calls_json"),
        str_field("get_columns_in_relation_calls_json"),
        str_field("patterned_dangling_sources_json"),
        str_field("nodes_with_resolution_errors_json"),
        str_field("nodes_with_access_errors_json"),
        str_field("operations_json"),
        str_field("selectors_json"),
        str_field("git_sha"),
        str_field("git_branch"),
        i32_field("git_is_dirty"),
        str_field("pkg_deps_json"),
        str_field("pkg_kinds_json"),
        str_field("pkg_manifest_path_configs_json"),
        i32_field("any_uses_graph"),
    ]
}

fn filestamp_fields() -> Vec<FieldRef> {
    vec![
        str_field("package_name"),
        str_field("package_root"),
        str_field("path_kind"),
        str_field("path"),
        i64_field("mtime_ns"),
        timestamp_micros_field("ingested_at"),
    ]
}

fn nullable_str_field(name: &str) -> FieldRef {
    Arc::new(Field::new(name, DataType::Utf8, true))
}

fn list_str_field(name: &str) -> FieldRef {
    Arc::new(Field::new(
        name,
        DataType::List(Arc::new(Field::new("item", DataType::Utf8, false))),
        false,
    ))
}

fn node_fields() -> Vec<FieldRef> {
    vec![
        str_field("unique_id"),
        i32_field("is_disabled"),
        str_field("resource_type"),
        str_field("name"),
        str_field("package_name"),
        str_field("original_path"),
        list_str_field("fqn"),
        list_str_field("tags"),
        list_str_field("depends_on"),
        i32_field("uses_graph"),
        str_field("materialization"),
        str_field("payload"),
        timestamp_micros_field("ingested_at"),
        i32_field("extended_model"),
        str_field("macro_arguments_json"),
        str_field("macro_span_json"),
        str_field("macro_name_span_json"),
        str_field("macro_absolute_path"),
        nullable_str_field("description"),
        nullable_str_field("database"),
        nullable_str_field("schema"),
        nullable_str_field("alias"),
        nullable_str_field("relation_name"),
        nullable_str_field("access"),
        nullable_str_field("group_name"),
        nullable_str_field("source_name"),
        nullable_str_field("identifier"),
    ]
}

// ── node row builders ─────────────────────────────────────────────────────────

fn node_row_from_trait<N>(uid: &str, node: &N, kind: &str, is_disabled: i32) -> NodeRow
where
    N: dbt_schemas::schemas::nodes::InternalDbtNode + InternalDbtNodeAttributes + Serialize,
{
    let c = node.common();
    let b = node.base();
    let fqn = c.fqn.clone();
    let tags = c.tags.clone();
    let depends_on = b.depends_on.nodes.clone();
    let uses_graph = if c.raw_code.as_deref().unwrap_or("").contains("graph") {
        1
    } else {
        0
    };
    let materialization = b.materialized.to_string();
    let payload = serde_json::to_string(node).unwrap_or_else(|_| "{}".into());
    let description = c.description.clone();
    let database = if b.database.is_empty() {
        None
    } else {
        Some(b.database.clone())
    };
    let schema = if b.schema.is_empty() {
        None
    } else {
        Some(b.schema.clone())
    };
    let alias = if b.alias.is_empty() {
        None
    } else {
        Some(b.alias.clone())
    };
    let group_name = node.get_group();
    let access = node.get_access().map(|a| a.to_string());
    let source = node.as_any().downcast_ref::<DbtSource>();
    let identifier = source
        .map(|s| s.__source_attr__.identifier.clone())
        .or_else(|| alias.clone());
    let source_name = source.map(|s| s.__source_attr__.source_name.clone());
    NodeRow {
        unique_id: uid.to_string(),
        is_disabled,
        resource_type: kind.to_string(),
        name: c.name.clone(),
        package_name: c.package_name.clone(),
        original_path: c.original_file_path.display().to_string(),
        fqn,
        tags,
        depends_on,
        uses_graph,
        materialization,
        payload,
        extended_model: node.is_extended_model() as i32,
        description,
        database,
        schema,
        alias,
        relation_name: b.relation_name.clone(),
        access,
        group_name,
        source_name,
        identifier,
        ..Default::default()
    }
}

fn append_operation_rows(rows: &mut Vec<NodeRow>, operations_json: &str) {
    let Ok(ops) = serde_json::from_str::<Operations>(operations_json) else {
        return;
    };
    for op in ops.on_run_start.iter().chain(ops.on_run_end.iter()) {
        let node: &DbtOperation = op;
        rows.push(node_row_from_trait(
            &node.__common_attr__.unique_id,
            node,
            "operation",
            0,
        ));
    }
}

fn append_operation_alive(
    rows: &mut Vec<dbt_metadata_parquet::parse_alive::AliveRow>,
    operations_json: &str,
    ingested_at: i64,
) {
    let Ok(ops) = serde_json::from_str::<Operations>(operations_json) else {
        return;
    };
    for op in ops.on_run_start.iter().chain(ops.on_run_end.iter()) {
        let node: &DbtOperation = op;
        rows.push(dbt_metadata_parquet::parse_alive::AliveRow {
            unique_id: node.__common_attr__.unique_id.clone(),
            resource_type: "operation".to_string(),
            ingested_at,
        });
    }
}

fn node_row_from_group(uid: &str, node: &DbtGroup, is_disabled: i32) -> NodeRow {
    let c = &node.__common_attr__;
    let fqn = c.fqn.clone();
    let tags = c.tags.clone();
    let depends_on = node.__base_attr__.depends_on.nodes.clone();
    let uses_graph = if c.raw_code.as_deref().unwrap_or("").contains("graph") {
        1
    } else {
        0
    };
    let payload = serde_json::to_string(node).unwrap_or_else(|_| "{}".into());
    NodeRow {
        unique_id: uid.to_string(),
        is_disabled,
        resource_type: "group".into(),
        name: c.name.clone(),
        package_name: c.package_name.clone(),
        original_path: c.original_file_path.display().to_string(),
        fqn,
        tags,
        depends_on,
        uses_graph,
        payload,
        ..Default::default()
    }
}

fn node_row_from_macro(uid: &str, node: &DbtMacro, is_disabled: i32) -> NodeRow {
    let depends_on = node.depends_on.macros.clone();
    let uses_graph = if node.macro_sql.contains("graph") {
        1
    } else {
        0
    };
    let payload = serde_json::to_string(node).unwrap_or_else(|_| "{}".into());
    let macro_arguments_json =
        serde_json::to_string(&node.arguments).unwrap_or_else(|_| "[]".into());
    let macro_span_json = node
        .span
        .as_ref()
        .and_then(|s| serde_json::to_string(s).ok())
        .unwrap_or_default();
    let macro_name_span_json = node
        .macro_name_span
        .as_ref()
        .and_then(|s| serde_json::to_string(s).ok())
        .unwrap_or_default();
    let macro_absolute_path = node.absolute_path.to_string_lossy().to_string();
    NodeRow {
        unique_id: uid.to_string(),
        is_disabled,
        resource_type: "macro".into(),
        name: node.name.clone(),
        package_name: node.package_name.clone(),
        original_path: node.original_file_path.display().to_string(),
        fqn: vec![],
        tags: vec![],
        depends_on,
        uses_graph,
        payload,
        macro_arguments_json,
        macro_span_json,
        macro_name_span_json,
        macro_absolute_path,
        ..Default::default()
    }
}

fn node_row_from_docs_macro(uid: &str, node: &DbtDocsMacro, is_disabled: i32) -> NodeRow {
    let payload = serde_json::to_string(node).unwrap_or_else(|_| "{}".into());
    NodeRow {
        unique_id: uid.to_string(),
        is_disabled,
        resource_type: "docs_macro".into(),
        name: node.name.clone(),
        package_name: node.package_name.clone(),
        original_path: node.original_file_path.display().to_string(),
        fqn: vec![],
        tags: vec![],
        depends_on: vec![],
        payload,
        ..Default::default()
    }
}

#[allow(clippy::cognitive_complexity)]
fn collect_all_rows(
    nodes: &Nodes,
    disabled_nodes: &Nodes,
    macros: &Macros,
    operations_json: &str,
) -> Vec<NodeRow> {
    let mut rows = Vec::new();
    macro_rules! push_trait {
        ($map:expr, $kind:literal, $dis:expr) => {
            for (uid, node) in $map.iter() {
                rows.push(node_row_from_trait(uid, node.as_ref(), $kind, $dis));
            }
        };
    }
    push_trait!(&nodes.models, "model", 0);
    push_trait!(&nodes.seeds, "seed", 0);
    push_trait!(&nodes.tests, "test", 0);
    push_trait!(&nodes.unit_tests, "unit_test", 0);
    push_trait!(&nodes.sources, "source", 0);
    push_trait!(&nodes.snapshots, "snapshot", 0);
    push_trait!(&nodes.analyses, "analysis", 0);
    push_trait!(&nodes.checks, "check", 0);
    push_trait!(&nodes.exposures, "exposure", 0);
    push_trait!(&nodes.semantic_models, "semantic_model", 0);
    push_trait!(&nodes.metrics, "metric", 0);
    push_trait!(&nodes.saved_queries, "saved_query", 0);
    push_trait!(&nodes.functions, "function", 0);
    for (uid, node) in nodes.groups.iter() {
        rows.push(node_row_from_group(uid, node.as_ref(), 0));
    }
    for (uid, node) in nodes.macros.iter() {
        rows.push(node_row_from_macro(uid, node.as_ref(), 0));
    }
    push_trait!(&disabled_nodes.models, "model", 1);
    push_trait!(&disabled_nodes.seeds, "seed", 1);
    push_trait!(&disabled_nodes.tests, "test", 1);
    push_trait!(&disabled_nodes.unit_tests, "unit_test", 1);
    push_trait!(&disabled_nodes.sources, "source", 1);
    push_trait!(&disabled_nodes.snapshots, "snapshot", 1);
    push_trait!(&disabled_nodes.analyses, "analysis", 1);
    push_trait!(&disabled_nodes.checks, "check", 1);
    push_trait!(&disabled_nodes.exposures, "exposure", 1);
    push_trait!(&disabled_nodes.semantic_models, "semantic_model", 1);
    push_trait!(&disabled_nodes.metrics, "metric", 1);
    push_trait!(&disabled_nodes.saved_queries, "saved_query", 1);
    push_trait!(&disabled_nodes.functions, "function", 1);
    for (uid, node) in disabled_nodes.groups.iter() {
        rows.push(node_row_from_group(uid, node.as_ref(), 1));
    }
    for (uid, node) in disabled_nodes.macros.iter() {
        rows.push(node_row_from_macro(uid, node.as_ref(), 1));
    }
    for (uid, node) in macros.docs_macros.iter() {
        rows.push(node_row_from_docs_macro(uid, node, 0));
    }
    append_operation_rows(&mut rows, operations_json);
    rows
}

#[allow(clippy::cognitive_complexity)]
fn collect_delta_rows(
    nodes: &Nodes,
    disabled_nodes: &Nodes,
    macros: &Macros,
    changed: &HashSet<String>,
) -> Vec<NodeRow> {
    let mut rows = Vec::new();
    macro_rules! push_if_changed {
        ($map:expr, $kind:literal, $dis:expr) => {
            for (uid, node) in $map.iter() {
                if changed.contains(uid.as_str()) {
                    rows.push(node_row_from_trait(uid, node.as_ref(), $kind, $dis));
                }
            }
        };
    }
    push_if_changed!(&nodes.models, "model", 0);
    push_if_changed!(&nodes.seeds, "seed", 0);
    push_if_changed!(&nodes.tests, "test", 0);
    push_if_changed!(&nodes.unit_tests, "unit_test", 0);
    push_if_changed!(&nodes.sources, "source", 0);
    push_if_changed!(&nodes.snapshots, "snapshot", 0);
    push_if_changed!(&nodes.analyses, "analysis", 0);
    push_if_changed!(&nodes.checks, "check", 0);
    push_if_changed!(&nodes.exposures, "exposure", 0);
    push_if_changed!(&nodes.semantic_models, "semantic_model", 0);
    push_if_changed!(&nodes.metrics, "metric", 0);
    push_if_changed!(&nodes.saved_queries, "saved_query", 0);
    push_if_changed!(&nodes.functions, "function", 0);
    for (uid, node) in nodes.groups.iter() {
        if changed.contains(uid.as_str()) {
            rows.push(node_row_from_group(uid, node.as_ref(), 0));
        }
    }
    for (uid, node) in nodes.macros.iter() {
        if changed.contains(uid.as_str()) {
            rows.push(node_row_from_macro(uid, node.as_ref(), 0));
        }
    }
    push_if_changed!(&disabled_nodes.models, "model", 1);
    push_if_changed!(&disabled_nodes.seeds, "seed", 1);
    push_if_changed!(&disabled_nodes.tests, "test", 1);
    push_if_changed!(&disabled_nodes.unit_tests, "unit_test", 1);
    push_if_changed!(&disabled_nodes.sources, "source", 1);
    push_if_changed!(&disabled_nodes.snapshots, "snapshot", 1);
    push_if_changed!(&disabled_nodes.analyses, "analysis", 1);
    push_if_changed!(&disabled_nodes.checks, "check", 1);
    push_if_changed!(&disabled_nodes.exposures, "exposure", 1);
    push_if_changed!(&disabled_nodes.semantic_models, "semantic_model", 1);
    push_if_changed!(&disabled_nodes.metrics, "metric", 1);
    push_if_changed!(&disabled_nodes.saved_queries, "saved_query", 1);
    push_if_changed!(&disabled_nodes.functions, "function", 1);
    for (uid, node) in disabled_nodes.groups.iter() {
        if changed.contains(uid.as_str()) {
            rows.push(node_row_from_group(uid, node.as_ref(), 1));
        }
    }
    for (uid, node) in disabled_nodes.macros.iter() {
        if changed.contains(uid.as_str()) {
            rows.push(node_row_from_macro(uid, node.as_ref(), 1));
        }
    }
    for (uid, node) in macros.docs_macros.iter() {
        if changed.contains(uid.as_str()) {
            rows.push(node_row_from_docs_macro(uid, node, 0));
        }
    }
    rows
}

// ── alive collection ─────────────────────────────────────────────────────────

#[allow(clippy::cognitive_complexity)]
fn collect_alive_rows(
    nodes: &Nodes,
    disabled_nodes: &Nodes,
    macros: &Macros,
    operations_json: &str,
    ingested_at: i64,
) -> Vec<dbt_metadata_parquet::parse_alive::AliveRow> {
    use dbt_metadata_parquet::parse_alive::AliveRow;
    let mut rows = Vec::new();
    macro_rules! push_alive {
        ($map:expr, $kind:literal) => {
            for uid in $map.keys() {
                rows.push(AliveRow {
                    unique_id: uid.clone(),
                    resource_type: $kind.to_string(),
                    ingested_at,
                });
            }
        };
    }
    push_alive!(&nodes.models, "model");
    push_alive!(&nodes.seeds, "seed");
    push_alive!(&nodes.tests, "test");
    push_alive!(&nodes.unit_tests, "unit_test");
    push_alive!(&nodes.sources, "source");
    push_alive!(&nodes.snapshots, "snapshot");
    push_alive!(&nodes.analyses, "analysis");
    push_alive!(&nodes.checks, "check");
    push_alive!(&nodes.exposures, "exposure");
    push_alive!(&nodes.semantic_models, "semantic_model");
    push_alive!(&nodes.metrics, "metric");
    push_alive!(&nodes.saved_queries, "saved_query");
    push_alive!(&nodes.groups, "group");
    push_alive!(&nodes.functions, "function");
    push_alive!(&nodes.macros, "macro");
    push_alive!(&disabled_nodes.models, "model");
    push_alive!(&disabled_nodes.seeds, "seed");
    push_alive!(&disabled_nodes.tests, "test");
    push_alive!(&disabled_nodes.unit_tests, "unit_test");
    push_alive!(&disabled_nodes.sources, "source");
    push_alive!(&disabled_nodes.snapshots, "snapshot");
    push_alive!(&disabled_nodes.analyses, "analysis");
    push_alive!(&disabled_nodes.checks, "check");
    push_alive!(&disabled_nodes.exposures, "exposure");
    push_alive!(&disabled_nodes.semantic_models, "semantic_model");
    push_alive!(&disabled_nodes.metrics, "metric");
    push_alive!(&disabled_nodes.saved_queries, "saved_query");
    push_alive!(&disabled_nodes.functions, "function");
    push_alive!(&disabled_nodes.groups, "group");
    push_alive!(&disabled_nodes.macros, "macro");
    for uid in macros.docs_macros.keys() {
        rows.push(AliveRow {
            unique_id: uid.clone(),
            resource_type: "doc".to_string(),
            ingested_at,
        });
    }
    append_operation_alive(&mut rows, operations_json, ingested_at);
    rows
}

// ── epoch compaction ──────────────────────────────────────────────────────────

/// Merge all epoch files into `nodes/0.parquet`, delete the rest.
fn compact(dir: &Path) {
    let epochs = existing_epochs(dir);
    if epochs.len() <= 1 {
        return;
    }
    // Read all epochs in order; latest epoch wins by unique_id.
    // We build a HashMap keyed by unique_id; later epochs overwrite earlier ones.
    let mut by_id: HashMap<String, NodeRow> = HashMap::new();
    for epoch in &epochs {
        for row in read_rows::<NodeRow>(&node_epoch_path(dir, *epoch)) {
            by_id.insert(row.unique_id.clone(), row);
        }
    }
    let merged: Vec<NodeRow> = by_id.into_values().collect();
    // Write to epoch 0. ingested_at of each winning row is preserved naturally.
    if write_rows_with_fields(&node_epoch_path(dir, 0), &node_fields(), &merged) {
        // Delete all other epoch files.
        for epoch in epochs.iter().filter(|&&e| e != 0) {
            fs::remove_file(node_epoch_path(dir, *epoch)).ok();
        }
    }
}

// ── read all node rows (latest-epoch wins) ────────────────────────────────────

fn read_node_rows(dir: &Path) -> Vec<NodeRow> {
    let epochs = existing_epochs(dir);
    if epochs.is_empty() {
        return vec![];
    }
    if epochs.len() == 1 {
        return read_rows(&node_epoch_path(dir, epochs[0]));
    }
    // Multiple epochs: latest wins.
    let mut by_id: HashMap<String, NodeRow> = HashMap::new();
    for epoch in &epochs {
        for row in read_rows::<NodeRow>(&node_epoch_path(dir, *epoch)) {
            by_id.insert(row.unique_id.clone(), row);
        }
    }
    by_id.into_values().collect()
}

/// Minimal projected read: just unique_id + ingested_at, no payload.
fn read_ingested_at(dir: &Path) -> HashMap<String, i64> {
    #[derive(Deserialize)]
    struct Row {
        unique_id: String,
        ingested_at: i64,
    }
    let mut out: HashMap<String, i64> = HashMap::new();
    for epoch in existing_epochs(dir) {
        let path = node_epoch_path(dir, epoch);
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        let Ok(builder) = ParquetRecordBatchReaderBuilder::try_new(file) else {
            continue;
        };
        let schema_desc = builder.parquet_schema();
        let col_indices: Vec<usize> = ["unique_id", "ingested_at"]
            .iter()
            .filter_map(|name| {
                (0..schema_desc.num_columns()).find(|&i| schema_desc.column(i).name() == *name)
            })
            .collect();
        let mask = ProjectionMask::leaves(schema_desc, col_indices);
        let Ok(reader) = builder.with_projection(mask).build() else {
            continue;
        };
        for batch in reader {
            let Ok(batch) = batch else { break };
            if let Ok(rows) = serde_arrow::from_record_batch::<Vec<Row>>(&batch) {
                for r in rows {
                    out.insert(r.unique_id, r.ingested_at);
                }
            }
        }
    }
    out
}

// ── save ──────────────────────────────────────────────────────────────────────

pub struct SaveArgs<'a> {
    pub out_dir: &'a Path,
    pub version: u32,
    pub dbt_version: &'a str,
    pub project_name: &'a str,
    pub adapter_type: &'a str,
    pub profile_hash: &'a str,
    pub profile_file_hash: &'a str,
    pub project_file_hash: &'a str,
    pub cli_vars_hash: &'a str,
    /// blake3 hash of `package-lock.yml`. Empty string when no lock file exists.
    pub packages_lock_hash: &'a str,
    pub dbt_profile_json: &'a str,
    pub vars_json: &'a str,
    pub env_vars_json: &'a str,
    pub get_relation_calls_json: &'a str,
    pub get_columns_in_relation_calls_json: &'a str,
    pub patterned_dangling_sources_json: &'a str,
    pub nodes_with_resolution_errors_json: &'a str,
    pub nodes_with_access_errors_json: &'a str,
    pub operations_json: &'a str,
    pub packages: &'a [PackageSnapshot],
    pub nodes: &'a Nodes,
    pub disabled_nodes: &'a Nodes,
    pub macros: &'a Macros,
    pub changed_nodes: Option<&'a HashSet<String>>,
    pub ingested_at: i64,
    pub selectors_json: &'a str,
    pub git_sha: &'a str,
    pub git_branch: &'a str,
    pub git_is_dirty: bool,
}

// ── SaveArgs default (for tests) ─────────────────────────────────────────────

static EMPTY_NODES: std::sync::OnceLock<Nodes> = std::sync::OnceLock::new();
static EMPTY_MACROS: std::sync::OnceLock<Macros> = std::sync::OnceLock::new();

impl<'a> SaveArgs<'a> {
    /// Returns a `SaveArgs` with all fields set to safe empty/zero defaults.
    /// Use with struct-update syntax in tests: `SaveArgs { version: 9999, ..SaveArgs::test_default(out_dir) }`.
    pub fn test_default(out_dir: &'a Path) -> Self {
        SaveArgs {
            out_dir,
            version: 0,
            dbt_version: "",
            project_name: "",
            adapter_type: "",
            profile_hash: "",
            profile_file_hash: "",
            project_file_hash: "",
            cli_vars_hash: "",
            packages_lock_hash: "",
            dbt_profile_json: "{}",
            vars_json: "{}",
            env_vars_json: "{}",
            get_relation_calls_json: "{}",
            get_columns_in_relation_calls_json: "{}",
            patterned_dangling_sources_json: "{}",
            nodes_with_resolution_errors_json: "[]",
            nodes_with_access_errors_json: "[]",
            operations_json: "{\"on_run_start\":[],\"on_run_end\":[]}",
            selectors_json: "",
            git_sha: "",
            git_branch: "",
            git_is_dirty: false,
            packages: &[],
            nodes: EMPTY_NODES.get_or_init(Nodes::default),
            disabled_nodes: EMPTY_NODES.get_or_init(Nodes::default),
            macros: EMPTY_MACROS.get_or_init(Macros::default),
            changed_nodes: None,
            ingested_at: 0,
        }
    }
}

fn push_column_rows(
    uid: &str,
    cols: &[Arc<dbt_schemas::schemas::dbt_column::DbtColumn>],
    ingested_at: i64,
    rows: &mut Vec<dbt_metadata_parquet::parse_columns::ParseColumnRow>,
) {
    use dbt_metadata_parquet::parse_columns::ParseColumnRow;
    for col in cols {
        let is_primary_key = col
            .constraints
            .iter()
            .any(|c| matches!(c.type_, ConstraintType::PrimaryKey));
        rows.push(ParseColumnRow {
            unique_id: uid.to_string(),
            column_name: col.name.clone(),
            description: col.description.clone(),
            declared_type: col.data_type.clone(),
            is_primary_key,
            constraints: if col.constraints.is_empty() {
                None
            } else {
                serde_json::to_string(&col.constraints).ok()
            },
            meta: if col.meta.is_empty() {
                None
            } else {
                serde_json::to_string(&col.meta).ok()
            },
            tags: col.tags.clone(),
            granularity: None,
            ingested_at,
        });
    }
}

fn collect_parse_column_rows(
    nodes: &Nodes,
    changed_nodes: Option<&HashSet<String>>,
    ingested_at: i64,
) -> Vec<dbt_metadata_parquet::parse_columns::ParseColumnRow> {
    let mut rows = Vec::new();
    for (uid, model) in &nodes.models {
        if changed_nodes.is_some_and(|s| !s.contains(uid.as_str())) {
            continue;
        }
        push_column_rows(uid, &model.__base_attr__.columns, ingested_at, &mut rows);
    }
    for (uid, src) in &nodes.sources {
        if changed_nodes.is_some_and(|s| !s.contains(uid.as_str())) {
            continue;
        }
        push_column_rows(uid, &src.__base_attr__.columns, ingested_at, &mut rows);
    }
    rows
}

fn collect_parse_test_metadata_rows(
    nodes: &Nodes,
    changed_nodes: Option<&HashSet<String>>,
    ingested_at: i64,
) -> Vec<dbt_metadata_parquet::parse_test_metadata::ParseTestMetadataRow> {
    use dbt_metadata_parquet::parse_test_metadata::ParseTestMetadataRow;
    let mut rows = Vec::new();

    for (uid, test) in &nodes.tests {
        if changed_nodes.is_some_and(|s| !s.contains(uid.as_str())) {
            continue;
        }
        if let Some(tm) = &test.__test_attr__.test_metadata {
            let severity = test
                .deprecated_config
                .severity
                .as_ref()
                .and_then(|v| serde_json::to_value(v).ok())
                .and_then(|v| v.as_str().map(String::from));
            rows.push(ParseTestMetadataRow {
                unique_id: uid.clone(),
                test_name: Some(tm.name.clone()),
                test_namespace: tm.namespace.clone(),
                kwargs: serde_json::to_string(&tm.kwargs)
                    .ok()
                    .filter(|s| s != "null"),
                column_name: test.__test_attr__.column_name.clone(),
                attached_node: test.__test_attr__.attached_node.clone(),
                severity,
                warn_if: test.deprecated_config.warn_if.clone(),
                error_if: test.deprecated_config.error_if.clone(),
                fail_calc: test.deprecated_config.fail_calc.clone(),
                store_failures: test.deprecated_config.store_failures,
                store_failures_as: test
                    .deprecated_config
                    .store_failures_as
                    .as_ref()
                    .and_then(|v| serde_json::to_value(v).ok())
                    .and_then(|v| v.as_str().map(String::from)),
                ingested_at,
            });
        }
    }
    rows
}

pub fn save(args: &SaveArgs<'_>) -> Result<(), String> {
    // Nothing changed — skip all I/O.
    if let Some(set) = args.changed_nodes {
        if set.is_empty() {
            return Ok(());
        }
    }

    let t0 = Instant::now();
    let dir = cache_dir(args.out_dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    // ── filestamps + pkg_deps/pkg_kinds ─────────────────────────────────────────
    let mut filestamp_rows: Vec<FilestampRow> = Vec::new();
    let mut pkg_deps_map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut pkg_kinds_map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut pkg_manifest_path_configs: BTreeMap<String, ManifestPathConfig> = BTreeMap::new();
    for pkg in args.packages {
        pkg_manifest_path_configs
            .insert(pkg.package_name.clone(), pkg.manifest_path_config.clone());
        for dep in &pkg.dependencies {
            pkg_deps_map
                .entry(pkg.package_name.clone())
                .or_default()
                .insert(dep.clone());
        }
        for (path_kind, entries) in &pkg.all_paths {
            let kind_str = serde_json::to_string(path_kind)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string();
            pkg_kinds_map
                .entry(pkg.package_name.clone())
                .or_default()
                .insert(kind_str.clone());
            for (path, mtime_ns) in entries {
                filestamp_rows.push(FilestampRow {
                    package_name: pkg.package_name.clone(),
                    package_root: pkg.package_root_path.clone(),
                    path_kind: kind_str.clone(),
                    path: path.clone(),
                    mtime_ns: *mtime_ns as i64,
                    ingested_at: args.ingested_at,
                });
            }
        }
    }

    let tfs = Instant::now();
    write_rows_with_fields(&filestamps_path(&dir), &filestamp_fields(), &filestamp_rows);
    t(
        &format!("write filestamps.parquet ({} rows)", filestamp_rows.len()),
        tfs,
    );

    let pkg_deps_json = serde_json::to_string(&pkg_deps_map).unwrap_or_else(|_| "{}".into());
    let pkg_kinds_json = serde_json::to_string(&pkg_kinds_map).unwrap_or_else(|_| "{}".into());
    let pkg_manifest_path_configs_json =
        serde_json::to_string(&pkg_manifest_path_configs).unwrap_or_else(|_| "{}".into());

    // ── nodes ─────────────────────────────────────────────────────────────────
    let nf_fields = node_fields();

    let any_uses_graph = match args.changed_nodes {
        None => {
            // Cold start: write epoch 0, remove any delta epochs.
            let tc = Instant::now();
            let mut all_rows = collect_all_rows(
                args.nodes,
                args.disabled_nodes,
                args.macros,
                args.operations_json,
            );
            for r in &mut all_rows {
                r.ingested_at = args.ingested_at;
            }
            t(&format!("collect_all_rows ({} rows)", all_rows.len()), tc);
            let any_uses_graph = all_rows.iter().any(|r| r.uses_graph != 0);
            let total = all_rows.len();
            let tw = Instant::now();
            write_rows_with_fields(&node_epoch_path(&dir, 0), &nf_fields, &all_rows);
            t(&format!("write nodes/0.parquet ({total} rows, cold)"), tw);
            for epoch in existing_epochs(&dir).into_iter().filter(|&e| e != 0) {
                fs::remove_file(node_epoch_path(&dir, epoch)).ok();
            }

            // Stamp this cold start with a generation token. dbt-index uses this
            // to surface compile-row staleness: a compile row with ingested_at older
            // than generation.ingested_at was produced before the last cold start.
            // The decision of what to do with stale rows is left to the query layer
            // (dbt.generation join) — we never delete compile data here.
            write_rows_with_fields(
                &generation_path(&dir),
                &generation_fields(),
                &[GenerationRow {
                    ingested_at: args.ingested_at,
                    version: args.version.to_string(),
                    dbt_version: args.dbt_version.to_string(),
                    project_name: args.project_name.to_string(),
                    adapter_type: args.adapter_type.to_string(),
                    vars_json: args.vars_json.to_string(),
                    env_vars_json: args.env_vars_json.to_string(),
                    profile_hash: args.profile_hash.to_string(),
                    profile_file_hash: args.profile_file_hash.to_string(),
                    project_file_hash: args.project_file_hash.to_string(),
                    cli_vars_hash: args.cli_vars_hash.to_string(),
                    packages_lock_hash: args.packages_lock_hash.to_string(),
                }],
            );

            any_uses_graph
        }
        Some(changed) => {
            // Approximate total rows (excludes groups/macros in nodes, but close enough for a threshold).
            let total_nodes = args.nodes.iter().count()
                + args.disabled_nodes.iter().count()
                + args.nodes.macros.len()
                + args.disabled_nodes.macros.len()
                + args.nodes.groups.len()
                + args.disabled_nodes.groups.len()
                + args.macros.macros.len()
                + args.macros.docs_macros.len();
            let threshold = (total_nodes / 10).max(10);
            let exceeds_threshold = changed.len() > threshold;

            if exceeds_threshold {
                // Full rewrite as epoch 0. Read back existing ingested_at values (projected,
                // no payload) so unchanged rows keep their original timestamp; only rows in
                // `changed` get the current invocation timestamp.
                let prior_ingested = read_ingested_at(&dir);
                let tc = Instant::now();
                let mut all_rows = collect_all_rows(
                    args.nodes,
                    args.disabled_nodes,
                    args.macros,
                    args.operations_json,
                );
                for r in &mut all_rows {
                    r.ingested_at = if changed.contains(&r.unique_id) {
                        args.ingested_at
                    } else {
                        prior_ingested
                            .get(&r.unique_id)
                            .copied()
                            .unwrap_or(args.ingested_at)
                    };
                }
                t(&format!("collect_all_rows ({} rows)", all_rows.len()), tc);
                let any_uses_graph = all_rows.iter().any(|r| r.uses_graph != 0);
                let total = all_rows.len();
                let tw = Instant::now();
                write_rows_with_fields(&node_epoch_path(&dir, 0), &nf_fields, &all_rows);
                t(
                    &format!("write nodes/0.parquet ({total} rows, full-rewrite)"),
                    tw,
                );
                for epoch in existing_epochs(&dir).into_iter().filter(|&e| e != 0) {
                    fs::remove_file(node_epoch_path(&dir, epoch)).ok();
                }
                any_uses_graph
            } else {
                let tc = Instant::now();
                let mut delta =
                    collect_delta_rows(args.nodes, args.disabled_nodes, args.macros, changed);
                let epoch = next_epoch(&dir);
                for r in &mut delta {
                    r.ingested_at = args.ingested_at;
                }
                t(&format!("collect_delta_rows ({} rows)", delta.len()), tc);
                let any_uses_graph = delta.iter().any(|r| r.uses_graph != 0);
                let tw = Instant::now();
                write_rows_with_fields(&node_epoch_path(&dir, epoch), &nf_fields, &delta);
                t(
                    &format!("write nodes/{epoch}.parquet ({} rows, delta)", delta.len()),
                    tw,
                );

                // Periodic compaction: row-count (delta > 10% of alive) OR file-count (>8).
                if dbt_metadata_parquet::epoch_io::should_compact(
                    delta.len(),
                    total_nodes,
                    existing_epochs(&dir).len(),
                ) {
                    compact(&dir);
                }
                any_uses_graph
            }
        }
    };
    // ── resolver_state ────────────────────────────────────────────────────────
    let tr = Instant::now();
    write_rows_with_fields(
        &resolver_state_path(&dir),
        &resolver_state_fields(),
        &[ResolverStateRow {
            ingested_at: args.ingested_at,
            dbt_profile_json: args.dbt_profile_json.to_string(),
            vars_json: args.vars_json.to_string(),
            env_vars_json: args.env_vars_json.to_string(),
            get_relation_calls_json: args.get_relation_calls_json.to_string(),
            get_columns_in_relation_calls_json: args.get_columns_in_relation_calls_json.to_string(),
            patterned_dangling_sources_json: args.patterned_dangling_sources_json.to_string(),
            nodes_with_resolution_errors_json: args.nodes_with_resolution_errors_json.to_string(),
            nodes_with_access_errors_json: args.nodes_with_access_errors_json.to_string(),
            operations_json: args.operations_json.to_string(),
            selectors_json: args.selectors_json.to_string(),
            git_sha: args.git_sha.to_string(),
            git_branch: args.git_branch.to_string(),
            git_is_dirty: args.git_is_dirty as i32,
            pkg_deps_json,
            pkg_kinds_json,
            pkg_manifest_path_configs_json,
            any_uses_graph: any_uses_graph as i32,
        }],
    );
    t("write resolver_state.parquet", tr);

    // ── alive ────────────────────────────────────────────────────────────────
    let ta = Instant::now();
    let alive_rows = collect_alive_rows(
        args.nodes,
        args.disabled_nodes,
        args.macros,
        args.operations_json,
        args.ingested_at,
    );
    let alive_path = dir.join("alive.parquet");
    if let Err(e) = dbt_metadata_parquet::parse_alive::write_alive(&alive_path, &alive_rows) {
        eprintln!("[warning] Failed to write alive.parquet: {e}");
    }
    t(
        &format!("write alive.parquet ({} rows)", alive_rows.len()),
        ta,
    );

    // ── parse/columns ─────────────────────────────────────────────────────────
    let tc = Instant::now();
    let parse_cols_dir = dir.join("columns");
    fs::create_dir_all(&parse_cols_dir).ok();
    let parse_col_rows =
        collect_parse_column_rows(args.nodes, args.changed_nodes, args.ingested_at);
    let alive_node_count = args.nodes.iter().count() + args.disabled_nodes.iter().count();
    if let Err(e) = dbt_metadata_parquet::parse_columns::write_parse_columns(
        &parse_cols_dir,
        parse_col_rows,
        args.changed_nodes,
        Some(alive_node_count),
        None,
    ) {
        eprintln!("[warning] Failed to write parse/columns: {e}");
    }
    t("write parse/columns", tc);

    // ── parse/test_metadata ────────────────────────────────────────────────────
    let tt = Instant::now();
    let test_meta_dir = dir.join("test_metadata");
    fs::create_dir_all(&test_meta_dir).ok();
    let test_meta_rows =
        collect_parse_test_metadata_rows(args.nodes, args.changed_nodes, args.ingested_at);
    if let Err(e) = dbt_metadata_parquet::parse_test_metadata::write_parse_test_metadata(
        &test_meta_dir,
        test_meta_rows,
        args.changed_nodes,
        Some(alive_node_count),
        None,
    ) {
        eprintln!("[warning] Failed to write parse/test_metadata: {e}");
    }
    t("write parse/test_metadata", tt);

    // Clean up legacy pkg_deps.parquet / pkg_kinds.parquet if still present.
    let _ = fs::remove_file(dir.join("pkg_deps.parquet"));
    let _ = fs::remove_file(dir.join("pkg_kinds.parquet"));

    t("save total", t0);
    Ok(())
}

// ── load ──────────────────────────────────────────────────────────────────────

pub struct LoadedState {
    pub version: u32,
    pub dbt_version: String,
    pub profile_hash: String,
    pub profile_file_hash: String,
    pub project_file_hash: String,
    pub cli_vars_hash: String,
    pub packages_lock_hash: String,
    pub dbt_profile_json: String,
    pub vars_json: String,
    pub env_vars_json: String,
    pub get_relation_calls_json: String,
    pub get_columns_in_relation_calls_json: String,
    pub patterned_dangling_sources_json: String,
    pub nodes_with_resolution_errors_json: String,
    pub nodes_with_access_errors_json: String,
    pub operations_json: String,
    pub packages: Vec<PackageSnapshot>,
    pub nodes: Nodes,
    pub disabled_nodes: Nodes,
    pub docs_macros: BTreeMap<String, DbtDocsMacro>,
    pub any_uses_graph: bool,
    pub selectors_json: String,
    pub git_sha: String,
    pub git_branch: String,
    pub git_is_dirty: bool,
}

pub fn peek_any_uses_graph(out_dir: &Path) -> bool {
    let dir = cache_dir(out_dir);
    let rows: Vec<ResolverStateRow> = read_rows(&resolver_state_path(&dir));
    rows.first().map(|r| r.any_uses_graph != 0).unwrap_or(false)
}

pub fn load(out_dir: &Path) -> Option<LoadedState> {
    load_filtered(out_dir, None)
}

pub fn load_filtered(
    out_dir: &Path,
    allowed_kinds: Option<&HashSet<&'static str>>,
) -> Option<LoadedState> {
    load_filtered_with_unique_ids(out_dir, allowed_kinds, None)
}

pub fn load_filtered_with_unique_ids(
    out_dir: &Path,
    allowed_kinds: Option<&HashSet<&'static str>>,
    allowed_unique_ids: Option<&HashSet<String>>,
) -> Option<LoadedState> {
    let t0 = Instant::now();
    let dir = cache_dir(out_dir);
    if !dir.exists() {
        return None;
    }
    // generation.parquet is the presence sentinel — if it's missing the cache is invalid.
    let gen_file = generation_path(&dir);
    if !gen_file.exists() {
        return None;
    }
    t("filecheck (dir+generation exists)", t0);

    let t1 = Instant::now();
    let generation_rows: Vec<GenerationRow> = read_rows(&gen_file);
    let generation = generation_rows.into_iter().next()?;
    // Empty dbt_version means the file is from before this field existed — treat as cache miss.
    if generation.dbt_version.is_empty() {
        return None;
    }
    let version: u32 = generation.version.parse().ok()?;
    t("read generation.parquet", t1);

    let t1b = Instant::now();
    let rs_rows: Vec<ResolverStateRow> = read_rows(&resolver_state_path(&dir));
    let rs = rs_rows.into_iter().next().unwrap_or_default();
    t("read resolver_state.parquet", t1b);

    let t2 = Instant::now();
    let packages = load_packages_from_filestamps(
        &dir,
        &rs.pkg_deps_json,
        &rs.pkg_kinds_json,
        &rs.pkg_manifest_path_configs_json,
    )?;
    t("load_packages (filestamps + pkg from resolver_state)", t2);

    let t3 = Instant::now();
    let (nodes, disabled_nodes, docs_macros) = load_nodes(&dir, allowed_kinds, allowed_unique_ids)?;
    t(
        &format!(
            "load_nodes (nodes, {} + {} nodes)",
            nodes.iter().count(),
            disabled_nodes.iter().count()
        ),
        t3,
    );

    t("load total", t0);

    Some(LoadedState {
        version,
        dbt_version: generation.dbt_version,
        profile_hash: generation.profile_hash,
        profile_file_hash: generation.profile_file_hash,
        project_file_hash: generation.project_file_hash,
        cli_vars_hash: generation.cli_vars_hash,
        packages_lock_hash: generation.packages_lock_hash,
        dbt_profile_json: rs.dbt_profile_json,
        vars_json: rs.vars_json,
        env_vars_json: rs.env_vars_json,
        get_relation_calls_json: rs.get_relation_calls_json,
        get_columns_in_relation_calls_json: rs.get_columns_in_relation_calls_json,
        patterned_dangling_sources_json: rs.patterned_dangling_sources_json,
        nodes_with_resolution_errors_json: rs.nodes_with_resolution_errors_json,
        nodes_with_access_errors_json: rs.nodes_with_access_errors_json,
        operations_json: rs.operations_json,
        packages,
        nodes,
        disabled_nodes,
        docs_macros,
        any_uses_graph: rs.any_uses_graph != 0,
        selectors_json: rs.selectors_json,
        git_sha: rs.git_sha,
        git_branch: rs.git_branch,
        git_is_dirty: rs.git_is_dirty != 0,
    })
}

pub(crate) fn load_packages_from_filestamps(
    dir: &Path,
    pkg_deps_json: &str,
    pkg_kinds_json: &str,
    pkg_manifest_path_configs_json: &str,
) -> Option<Vec<PackageSnapshot>> {
    let mut pkg_roots: HashMap<String, String> = HashMap::new();
    let mut pkg_paths: HashMap<String, HashMap<ResourcePathKind, Vec<(String, u64)>>> =
        HashMap::new();

    for row in read_rows::<FilestampRow>(&filestamps_path(dir)) {
        pkg_roots.insert(row.package_name.clone(), row.package_root.clone());
        let kind: ResourcePathKind = serde_json::from_str(&format!("\"{}\"", row.path_kind))
            .unwrap_or(ResourcePathKind::ModelPaths);
        pkg_paths
            .entry(row.package_name)
            .or_default()
            .entry(kind)
            .or_default()
            .push((row.path, row.mtime_ns as u64));
    }

    let mut pkg_deps: HashMap<String, BTreeSet<String>> =
        serde_json::from_str(pkg_deps_json).unwrap_or_default();
    let mut pkg_manifest_path_configs: BTreeMap<String, ManifestPathConfig> =
        serde_json::from_str(pkg_manifest_path_configs_json).unwrap_or_default();

    let pkg_kinds_raw: BTreeMap<String, BTreeSet<String>> =
        serde_json::from_str(pkg_kinds_json).unwrap_or_default();
    for (pkg_name, kinds) in &pkg_kinds_raw {
        for kind_str in kinds {
            let kind: ResourcePathKind = serde_json::from_str(&format!("\"{kind_str}\""))
                .unwrap_or(ResourcePathKind::ModelPaths);
            pkg_paths
                .entry(pkg_name.clone())
                .or_default()
                .entry(kind)
                .or_default();
        }
    }

    // Fallback: read legacy separate files if KV entries are empty (first run after upgrade).
    if pkg_deps.is_empty() {
        #[derive(Deserialize)]
        struct PkgDepRow {
            package_name: String,
            dependency: String,
        }
        for row in read_rows::<PkgDepRow>(&dir.join("pkg_deps.parquet")) {
            pkg_deps
                .entry(row.package_name)
                .or_default()
                .insert(row.dependency);
        }
    }
    if pkg_kinds_raw.is_empty() {
        #[derive(Deserialize)]
        struct PkgKindRow {
            package_name: String,
            path_kind: String,
        }
        for row in read_rows::<PkgKindRow>(&dir.join("pkg_kinds.parquet")) {
            let kind: ResourcePathKind = serde_json::from_str(&format!("\"{}\"", row.path_kind))
                .unwrap_or(ResourcePathKind::ModelPaths);
            pkg_paths
                .entry(row.package_name)
                .or_default()
                .entry(kind)
                .or_default();
        }
    }

    let mut snapshots: Vec<PackageSnapshot> = pkg_roots
        .into_iter()
        .map(|(name, root)| PackageSnapshot {
            package_root_path: root,
            package_name: name.clone(),
            manifest_path_config: pkg_manifest_path_configs.remove(&name).unwrap_or_default(),
            all_paths: pkg_paths.remove(&name).unwrap_or_default(),
            dependencies: pkg_deps.remove(&name).unwrap_or_default(),
            is_local_dep: false, // re-derived below after root is placed at index 0
        })
        .collect();

    // Ensure root package (the one not listed as a dependency of any other) is at index 0.
    // HashMap iteration order is non-deterministic; without this, validate() would use a
    // random package's root path to resolve profiles.yml, causing spurious cache invalidation.
    let all_deps: HashSet<&str> = snapshots
        .iter()
        .flat_map(|p| p.dependencies.iter().map(|d| d.as_str()))
        .collect();
    if let Some(root_idx) = snapshots
        .iter()
        .position(|p| !all_deps.contains(p.package_name.as_str()))
    {
        snapshots.swap(0, root_idx);
    }

    // Re-derive is_local_dep: non-root packages without a `dbt_packages` path component
    // are local-path deps. Root (index 0) is always false.
    for (i, snap) in snapshots.iter_mut().enumerate() {
        snap.is_local_dep = i > 0
            && !Path::new(&snap.package_root_path)
                .components()
                .any(|c| c.as_os_str() == "dbt_packages");
    }

    Some(snapshots)
}

fn load_nodes(
    dir: &Path,
    allowed_kinds: Option<&HashSet<&'static str>>,
    allowed_unique_ids: Option<&HashSet<String>>,
) -> Option<(Nodes, Nodes, BTreeMap<String, DbtDocsMacro>)> {
    let mut nodes = Nodes::default();
    let mut disabled_nodes = Nodes::default();
    let mut docs_macros: BTreeMap<String, DbtDocsMacro> = BTreeMap::new();

    let all_rows = read_node_rows(dir);
    for row in all_rows {
        if row.resource_type == "docs_macro" {
            if let Ok(v) = serde_json::from_str::<DbtDocsMacro>(&row.payload) {
                docs_macros.insert(row.unique_id, v);
            }
            continue;
        }
        if let Some(kinds) = allowed_kinds {
            if !kinds.contains(row.resource_type.as_str()) {
                continue;
            }
        }
        if let Some(ids) = allowed_unique_ids {
            if !ids.contains(&row.unique_id) {
                continue;
            }
        }
        let target = if row.is_disabled != 0 {
            &mut disabled_nodes
        } else {
            &mut nodes
        };
        deserialize_into(
            &row.unique_id,
            &row.resource_type,
            &row.payload,
            row.extended_model != 0,
            &row.macro_arguments_json,
            &row.macro_span_json,
            &row.macro_name_span_json,
            &row.macro_absolute_path,
            target,
        );
    }

    Some((nodes, disabled_nodes, docs_macros))
}

#[allow(clippy::too_many_arguments)]
fn deserialize_into(
    uid: &str,
    kind: &str,
    payload: &str,
    extended_model: bool,
    macro_arguments_json: &str,
    macro_span_json: &str,
    macro_name_span_json: &str,
    macro_absolute_path: &str,
    nodes: &mut Nodes,
) {
    macro_rules! deser {
        ($map:expr, $T:ty) => {
            if let Ok(v) = serde_json::from_str::<$T>(payload) {
                $map.insert(uid.to_string(), Arc::new(v));
            }
        };
    }
    match kind {
        "model" => {
            if let Ok(mut v) = serde_json::from_str::<DbtModel>(payload) {
                v.__base_attr__.extended_model = extended_model;
                nodes.models.insert(uid.to_string(), Arc::new(v));
            }
        }
        "macro" => {
            if let Ok(mut v) = serde_json::from_str::<DbtMacro>(payload) {
                if !macro_arguments_json.is_empty() {
                    if let Ok(args) =
                        serde_json::from_str::<Vec<MacroArgument>>(macro_arguments_json)
                    {
                        v.arguments = args;
                    }
                }
                if !macro_span_json.is_empty() {
                    v.span = serde_json::from_str(macro_span_json).ok();
                }
                if !macro_name_span_json.is_empty() {
                    v.macro_name_span = serde_json::from_str(macro_name_span_json).ok();
                }
                if !macro_absolute_path.is_empty() {
                    v.absolute_path = DbtPath::from(macro_absolute_path);
                }
                nodes.macros.insert(uid.to_string(), Arc::new(v));
            }
        }
        "seed" => deser!(nodes.seeds, DbtSeed),
        "test" => deser!(nodes.tests, DbtTest),
        "unit_test" => deser!(nodes.unit_tests, DbtUnitTest),
        "source" => deser!(nodes.sources, DbtSource),
        "snapshot" => deser!(nodes.snapshots, DbtSnapshot),
        "analysis" => deser!(nodes.analyses, DbtAnalysis),
        "check" => deser!(nodes.checks, DbtCheck),
        "exposure" => deser!(nodes.exposures, DbtExposure),
        "semantic_model" => deser!(nodes.semantic_models, DbtSemanticModel),
        "metric" => deser!(nodes.metrics, DbtMetric),
        "saved_query" => deser!(nodes.saved_queries, DbtSavedQuery),
        "group" => deser!(nodes.groups, DbtGroup),
        "function" => deser!(nodes.functions, DbtFunction),
        _ => {}
    }
}

// ── PackageSnapshot ↔ DbtPackage ─────────────────────────────────────────────

pub fn snapshot_packages(packages: &[DbtPackage]) -> Vec<PackageSnapshot> {
    let root_package_path = packages
        .first()
        .map(|package| package.package_root_path.as_path())
        .unwrap_or_else(|| Path::new(""));
    packages
        .iter()
        .enumerate()
        .filter(|(_, pkg)| {
            // Internal packages are embedded in the binary and reconstructed at load time.
            // Saving them to the parquet cache is redundant and causes package-count mismatches
            // when the loader re-adds them fresh on top of a prev_dbt_state that already has them.
            !pkg.package_root_path
                .components()
                .any(|c| c.as_os_str() == "dbt_internal_packages")
                // The inline ("") package holds the transient --inline SQL asset. It is
                // reconstructed fresh by `prepare_inline_sql` on every run (its temp file is
                // regenerated with a new uuid), so it must not be persisted to the cache —
                // otherwise the incremental path would restore a stale, empty "" stub and the
                // fresh injection would create a duplicate "" package.
                && !pkg.dbt_project.name.is_empty()
        })
        .map(|(i, pkg)| {
            let all_paths: HashMap<ResourcePathKind, Vec<(String, u64)>> = pkg
                .all_paths
                .iter()
                .map(|(kind, entries)| {
                    let rows = entries
                        .iter()
                        .map(|(path, mtime)| {
                            (path.display().to_string(), system_time_to_nanos(*mtime))
                        })
                        .collect();
                    (kind.clone(), rows)
                })
                .collect();
            // Non-root deps installed via `dbt deps` land under dbt_packages/<name>.
            // Local-path deps (`local: ../foo`) live at an arbitrary path outside dbt_packages.
            // We detect local deps by the absence of the `dbt_packages` path component.
            let is_local_dep = i > 0
                && !pkg
                    .package_root_path
                    .components()
                    .any(|c| c.as_os_str() == "dbt_packages");
            PackageSnapshot {
                package_root_path: pkg.package_root_path.display().to_string(),
                package_name: pkg.dbt_project.name.clone(),
                manifest_path_config: ManifestPathConfig::from_package(pkg, root_package_path),
                all_paths,
                dependencies: pkg.dependencies.clone(),
                is_local_dep,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index_resolution::resolve_dirty_unique_ids_from_index;
    use dbt_schemas::state::DbtAsset;
    use minijinja::machinery::Span;
    use tempfile::TempDir;

    fn make_row(uid: &str, payload: &str) -> NodeRow {
        NodeRow {
            unique_id: uid.to_string(),
            resource_type: "model".to_string(),
            name: uid.to_string(),
            package_name: "pkg".to_string(),
            original_path: format!("models/{uid}.sql"),
            fqn: vec!["pkg".to_string(), uid.to_string()],
            tags: vec![],
            depends_on: vec![],
            materialization: "table".to_string(),
            payload: payload.to_string(),
            ..Default::default()
        }
    }

    /// Write epoch 0 with 3 rows, then 9 delta epochs each updating row "a".
    /// After the 9th delta save() triggers compaction; only epoch 0 must remain,
    /// and "a" must carry the payload from the last delta.
    #[test]
    fn compaction_merges_epochs_and_latest_wins() {
        let tmp = TempDir::new().unwrap();
        let dir = cache_dir(tmp.path());
        fs::create_dir_all(nodes_dir(&dir)).unwrap();

        // Write base epoch: rows a, b, c.
        let base = vec![
            make_row("a", r#"{"v":0}"#),
            make_row("b", r#"{"v":0}"#),
            make_row("c", r#"{"v":0}"#),
        ];
        write_rows_with_fields(&node_epoch_path(&dir, 0), &node_fields(), &base);

        // Write 9 delta epochs, each updating "a" with an incrementing payload.
        // The 9th delta triggers compact() (file-count > 8).
        for i in 1u32..=9 {
            let delta = vec![make_row("a", &format!(r#"{{"v":{i}}}"#))];
            write_rows_with_fields(&node_epoch_path(&dir, i), &node_fields(), &delta);
        }
        // Manually trigger compact (save() does this internally; here we call directly).
        compact(&dir);

        // Only epoch 0 must remain.
        let epochs = existing_epochs(&dir);
        assert_eq!(
            epochs,
            vec![0],
            "expected only epoch 0 after compaction, got {epochs:?}"
        );

        // Read back and verify latest-wins: "a" has v=9, "b" and "c" have v=0.
        let rows = read_node_rows(&dir);
        let mut by_id: HashMap<String, String> =
            rows.into_iter().map(|r| (r.unique_id, r.payload)).collect();
        assert_eq!(
            by_id.remove("a").unwrap(),
            r#"{"v":9}"#,
            "a should carry last delta"
        );
        assert_eq!(
            by_id.remove("b").unwrap(),
            r#"{"v":0}"#,
            "b should be unchanged"
        );
        assert_eq!(
            by_id.remove("c").unwrap(),
            r#"{"v":0}"#,
            "c should be unchanged"
        );
        assert!(by_id.is_empty(), "unexpected extra rows: {by_id:?}");
    }

    /// Verify that all new NodeRow columns survive a write→read round-trip through parquet.
    #[test]
    fn node_fact_row_new_columns_round_trip() {
        let tmp = TempDir::new().unwrap();
        let dir = cache_dir(tmp.path());
        fs::create_dir_all(nodes_dir(&dir)).unwrap();

        let span = Span {
            start_line: 3,
            start_col: 5,
            start_offset: 42,
            end_line: 3,
            end_col: 20,
            end_offset: 57,
        };
        let name_span = Span {
            start_line: 3,
            start_col: 9,
            start_offset: 46,
            end_line: 3,
            end_col: 15,
            end_offset: 52,
        };
        let args = vec![
            MacroArgument {
                name: "amount".into(),
                type_: Some("number".into()),
                description: "the amount".into(),
            },
            MacroArgument {
                name: "currency".into(),
                type_: None,
                description: String::new(),
            },
        ];

        let row = NodeRow {
            unique_id: "macro.pkg.my_macro".to_string(),
            resource_type: "macro".to_string(),
            name: "my_macro".to_string(),
            package_name: "pkg".to_string(),
            original_path: "macros/my_macro.sql".to_string(),
            fqn: vec![],
            tags: vec![],
            depends_on: vec![],
            payload: r#"{"name":"my_macro","package_name":"pkg"}"#.to_string(),
            ingested_at: 1_700_000_000_000_000,
            macro_arguments_json: serde_json::to_string(&args).unwrap(),
            macro_span_json: serde_json::to_string(&span).unwrap(),
            macro_name_span_json: serde_json::to_string(&name_span).unwrap(),
            ..Default::default()
        };

        write_rows_with_fields(
            &node_epoch_path(&dir, 0),
            &node_fields(),
            std::slice::from_ref(&row),
        );
        let rows: Vec<NodeRow> = read_rows(&node_epoch_path(&dir, 0));
        assert_eq!(rows.len(), 1);
        let got = &rows[0];

        assert_eq!(got.ingested_at, 1_700_000_000_000_000);
        assert_eq!(got.extended_model, 0);
        assert_eq!(got.macro_arguments_json, row.macro_arguments_json);
        assert_eq!(got.macro_span_json, row.macro_span_json);
        assert_eq!(got.macro_name_span_json, row.macro_name_span_json);

        // Verify span JSON round-trips correctly.
        let got_span: Span = serde_json::from_str(&got.macro_span_json).unwrap();
        assert_eq!(got_span.start_line, 3);
        assert_eq!(got_span.start_col, 5);
        assert_eq!(got_span.start_offset, 42);
        assert_eq!(got_span.end_line, 3);
        assert_eq!(got_span.end_col, 20);
        assert_eq!(got_span.end_offset, 57);

        let got_args: Vec<MacroArgument> = serde_json::from_str(&got.macro_arguments_json).unwrap();
        assert_eq!(got_args.len(), 2);
        assert_eq!(got_args[0].name, "amount");
        assert_eq!(got_args[0].type_, Some("number".into()));
        assert_eq!(got_args[1].name, "currency");
        assert_eq!(got_args[1].type_, None);
    }

    /// Verify that extended_model=true round-trips through parquet + deserialize_into.
    #[test]
    fn extended_model_survives_deserialize_into() {
        use dbt_schemas::schemas::nodes::DbtModel;

        let tmp = TempDir::new().unwrap();
        let dir = cache_dir(tmp.path());
        fs::create_dir_all(nodes_dir(&dir)).unwrap();

        // Build a minimal DbtModel and serialize it as payload.
        let mut model = DbtModel::default();
        model.__base_attr__.extended_model = true;
        // serde_json will skip extended_model (it has #[serde(skip_serializing)]).
        let payload = serde_json::to_string(&model).unwrap();

        // Confirm extended_model is absent from the JSON payload.
        assert!(
            !payload.contains("extended_model"),
            "extended_model should be skip_serializing"
        );

        let row = NodeRow {
            unique_id: "model.pkg.orders".to_string(),
            resource_type: "model".to_string(),
            extended_model: 1,
            payload,
            ..make_row("model.pkg.orders", "{}")
        };
        write_rows_with_fields(&node_epoch_path(&dir, 0), &node_fields(), &[row]);

        let mut nodes = Nodes::default();
        let rows: Vec<NodeRow> = read_rows(&node_epoch_path(&dir, 0));
        let r = &rows[0];
        deserialize_into(
            &r.unique_id,
            &r.resource_type,
            &r.payload,
            r.extended_model != 0,
            &r.macro_arguments_json,
            &r.macro_span_json,
            &r.macro_name_span_json,
            &r.macro_absolute_path,
            &mut nodes,
        );

        let loaded = nodes
            .models
            .get("model.pkg.orders")
            .expect("model should be loaded");
        assert!(
            loaded.__base_attr__.extended_model,
            "extended_model must survive the round-trip"
        );
    }

    /// Verify that macro span, name_span, and arguments survive parquet + deserialize_into.
    #[test]
    fn macro_fields_survive_deserialize_into() {
        use dbt_schemas::schemas::macros::DbtMacro;

        let tmp = TempDir::new().unwrap();
        let dir = cache_dir(tmp.path());
        fs::create_dir_all(nodes_dir(&dir)).unwrap();

        let span = Span {
            start_line: 10,
            start_col: 1,
            start_offset: 200,
            end_line: 10,
            end_col: 30,
            end_offset: 229,
        };
        let name_span = Span {
            start_line: 10,
            start_col: 10,
            start_offset: 209,
            end_line: 10,
            end_col: 20,
            end_offset: 219,
        };
        let args = vec![MacroArgument {
            name: "col".into(),
            type_: Some("string".into()),
            description: "a column".into(),
        }];

        let macro_node = DbtMacro {
            name: "cents_to_dollars".into(),
            package_name: "pkg".into(),
            span: Some(span),
            macro_name_span: Some(name_span),
            arguments: args.clone(),
            ..DbtMacro::default()
        };

        // payload is produced by serde_json which skips span/macro_name_span/arguments.
        let payload = serde_json::to_string(&macro_node).unwrap();
        assert!(
            !payload.contains("start_line"),
            "span should be skip_serializing in payload"
        );

        let row = NodeRow {
            unique_id: "macro.pkg.cents_to_dollars".to_string(),
            resource_type: "macro".to_string(),
            name: "cents_to_dollars".to_string(),
            package_name: "pkg".to_string(),
            payload,
            macro_arguments_json: serde_json::to_string(&args).unwrap(),
            macro_span_json: serde_json::to_string(&span).unwrap(),
            macro_name_span_json: serde_json::to_string(&name_span).unwrap(),
            ..make_row("macro.pkg.cents_to_dollars", "{}")
        };

        write_rows_with_fields(&node_epoch_path(&dir, 0), &node_fields(), &[row]);

        let mut nodes = Nodes::default();
        let rows: Vec<NodeRow> = read_rows(&node_epoch_path(&dir, 0));
        let r = &rows[0];
        deserialize_into(
            &r.unique_id,
            &r.resource_type,
            &r.payload,
            r.extended_model != 0,
            &r.macro_arguments_json,
            &r.macro_span_json,
            &r.macro_name_span_json,
            &r.macro_absolute_path,
            &mut nodes,
        );

        let loaded = nodes
            .macros
            .get("macro.pkg.cents_to_dollars")
            .expect("macro should be loaded");

        let loaded_span = loaded.span.expect("span must be restored");
        assert_eq!(loaded_span.start_line, 10);
        assert_eq!(loaded_span.start_col, 1);
        assert_eq!(loaded_span.start_offset, 200);
        assert_eq!(loaded_span.end_line, 10);
        assert_eq!(loaded_span.end_col, 30);
        assert_eq!(loaded_span.end_offset, 229);

        let loaded_name_span = loaded
            .macro_name_span
            .expect("macro_name_span must be restored");
        assert_eq!(loaded_name_span.start_offset, 209);

        assert_eq!(loaded.arguments.len(), 1);
        assert_eq!(loaded.arguments[0].name, "col");
        assert_eq!(loaded.arguments[0].type_, Some("string".into()));
    }

    // ── state:dirty tests ────────────────────────────────────────────────────────

    /// Build a minimal filestamps.parquet + node index in a tempdir.
    /// Returns (TempDir, root_path_for_package).
    fn write_dirty_fixture(
        nodes: &[(&str, &str, &[&str])], // (uid, rel_path, deps)
    ) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let out_dir = tmp.path().to_path_buf();
        let pkg_root = out_dir.join("project");
        fs::create_dir_all(pkg_root.join("models")).unwrap();

        // Write actual .sql files so mtime checks work.
        for (uid, rel_path, _) in nodes {
            let full = pkg_root.join(rel_path);
            if let Some(p) = full.parent() {
                fs::create_dir_all(p).unwrap();
            }
            fs::write(&full, uid.as_bytes()).unwrap();
        }

        // Build NodeRow vec.
        let node_rows: Vec<NodeRow> = nodes
            .iter()
            .map(|&(uid, rel_path, deps)| NodeRow {
                unique_id: uid.to_string(),
                resource_type: "model".to_string(),
                name: uid.to_string(),
                package_name: "pkg".to_string(),
                original_path: rel_path.to_string(),
                fqn: vec!["pkg".to_string(), uid.to_string()],
                tags: vec![],
                depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
                materialization: "table".to_string(),
                payload: "{}".to_string(),
                ..Default::default()
            })
            .collect();

        // Write to parquet cache.
        let dir = cache_dir(&out_dir);
        fs::create_dir_all(nodes_dir(&dir)).unwrap();
        write_rows_with_fields(&node_epoch_path(&dir, 0), &node_fields(), &node_rows);

        // Write filestamps.parquet.
        let filestamp_rows: Vec<FilestampRow> = nodes
            .iter()
            .map(|&(_, rel_path, _)| {
                let mtime = fs::metadata(pkg_root.join(rel_path))
                    .and_then(|m| m.modified())
                    .map(system_time_to_nanos)
                    .unwrap_or(0);
                FilestampRow {
                    package_name: "pkg".to_string(),
                    package_root: pkg_root.display().to_string(),
                    path_kind: "ModelPaths".to_string(),
                    path: rel_path.to_string(),
                    mtime_ns: mtime as i64,
                    ingested_at: 1_700_000_000_000_000,
                }
            })
            .collect();
        write_rows_with_fields(&filestamps_path(&dir), &filestamp_fields(), &filestamp_rows);

        // Write resolver_state.parquet with pkg_deps_json and pkg_kinds_json.
        let mut pkg_kinds_map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        pkg_kinds_map
            .entry("pkg".to_string())
            .or_default()
            .insert("ModelPaths".to_string());
        write_rows_with_fields(
            &resolver_state_path(&dir),
            &resolver_state_fields(),
            &[ResolverStateRow {
                pkg_deps_json: "{}".to_string(),
                pkg_kinds_json: serde_json::to_string(&pkg_kinds_map).unwrap(),
                ..Default::default()
            }],
        );

        (tmp, pkg_root)
    }

    /// Touch a file by sleeping 15ms then rewriting it (advances mtime).
    fn touch_file(path: &Path) {
        std::thread::sleep(Duration::from_millis(15));
        let content = fs::read(path).unwrap_or_default();
        fs::write(path, content).unwrap();
    }

    /// Graph: a←b←c (c depends on b, b depends on a), plus independent d.
    #[test]
    fn test_resolve_dirty_basic() {
        let nodes = &[
            ("model.pkg.a", "models/a.sql", &[][..]),
            ("model.pkg.b", "models/b.sql", &["model.pkg.a"][..]),
            ("model.pkg.c", "models/c.sql", &["model.pkg.b"][..]),
            ("model.pkg.d", "models/d.sql", &[][..]),
        ];
        let (tmp, pkg_root) = write_dirty_fixture(nodes);

        // Touch b — b is dirty; a is pulled in as its ancestor (so all_deps_present passes);
        // c and d are not included (no children walk, d is independent).
        touch_file(&pkg_root.join("models/b.sql"));

        let result = resolve_dirty_unique_ids_from_index(tmp.path(), None, None, false).unwrap();
        assert!(result.contains("model.pkg.b"), "b is dirty");
        assert!(
            result.contains("model.pkg.a"),
            "a pulled in as ancestor of b"
        );
        assert!(
            !result.contains("model.pkg.c"),
            "c not included (no + walk)"
        );
        assert!(
            !result.contains("model.pkg.d"),
            "d not included (independent)"
        );
    }

    /// state:dirty+ (children walk): touching a should include a, b, c.
    #[test]
    fn test_resolve_dirty_children_walk() {
        let nodes = &[
            ("model.pkg.a", "models/a.sql", &[][..]),
            ("model.pkg.b", "models/b.sql", &["model.pkg.a"][..]),
            ("model.pkg.c", "models/c.sql", &["model.pkg.b"][..]),
        ];

        let (tmp, pkg_root) = write_dirty_fixture(nodes);
        touch_file(&pkg_root.join("models/a.sql"));

        let result =
            resolve_dirty_unique_ids_from_index(tmp.path(), None, Some(u32::MAX), false).unwrap();
        assert!(result.contains("model.pkg.a"), "a is dirty seed");
        assert!(result.contains("model.pkg.b"), "b is downstream of a");
        assert!(result.contains("model.pkg.c"), "c is downstream of b");
    }

    /// +state:dirty (parents walk): touching c should include a, b, c via ancestor closure.
    #[test]
    fn test_resolve_dirty_parent_walk() {
        let nodes = &[
            ("model.pkg.a", "models/a.sql", &[][..]),
            ("model.pkg.b", "models/b.sql", &["model.pkg.a"][..]),
            ("model.pkg.c", "models/c.sql", &["model.pkg.b"][..]),
        ];
        let (tmp, pkg_root) = write_dirty_fixture(nodes);
        touch_file(&pkg_root.join("models/c.sql"));

        let result =
            resolve_dirty_unique_ids_from_index(tmp.path(), Some(u32::MAX), None, false).unwrap();
        assert!(result.contains("model.pkg.a"), "a is ancestor of c");
        assert!(result.contains("model.pkg.b"), "b is ancestor of c");
        assert!(result.contains("model.pkg.c"), "c is dirty seed");
    }

    /// Nothing touched → Some(empty set) not None (cache exists, just clean).
    #[test]
    fn test_resolve_dirty_nothing_touched() {
        let nodes = &[
            ("model.pkg.a", "models/a.sql", &[][..]),
            ("model.pkg.b", "models/b.sql", &["model.pkg.a"][..]),
        ];
        let (tmp, _pkg_root) = write_dirty_fixture(nodes);

        let result = resolve_dirty_unique_ids_from_index(tmp.path(), None, None, false).unwrap();
        // No model nodes — only macros (none in this fixture).
        assert!(!result.contains("model.pkg.a"));
        assert!(!result.contains("model.pkg.b"));
    }

    /// No cache → None.
    #[test]
    fn test_resolve_dirty_no_cache() {
        let tmp = TempDir::new().unwrap();
        let result = resolve_dirty_unique_ids_from_index(tmp.path(), None, None, false);
        assert!(result.is_none(), "no cache should return None");
    }

    /// Diamond: b depends on a, c depends on b AND e. Touching a with + walk
    /// should include a, b, c, and e (e is ancestor of c, pulled in by ancestor closure).
    #[test]
    fn test_resolve_dirty_diamond() {
        let nodes = &[
            ("model.pkg.a", "models/a.sql", &[][..]),
            ("model.pkg.e", "models/e.sql", &[][..]),
            ("model.pkg.b", "models/b.sql", &["model.pkg.a"][..]),
            (
                "model.pkg.c",
                "models/c.sql",
                &["model.pkg.b", "model.pkg.e"][..],
            ),
        ];
        let (tmp, pkg_root) = write_dirty_fixture(nodes);
        touch_file(&pkg_root.join("models/a.sql"));

        let result =
            resolve_dirty_unique_ids_from_index(tmp.path(), None, Some(u32::MAX), false).unwrap();
        assert!(result.contains("model.pkg.a"), "a is dirty seed");
        assert!(result.contains("model.pkg.b"), "b downstream of a");
        assert!(result.contains("model.pkg.c"), "c downstream of b");
        // e is a parent of c (which is in the result), so ancestor closure adds it.
        assert!(
            result.contains("model.pkg.e"),
            "e pulled in as ancestor of c"
        );
    }

    // ── Internal package regression tests ─────────────────────────────────────

    fn make_user_package(root: &Path, name: &str) -> DbtPackage {
        DbtPackage {
            dbt_project: serde_json::from_value(serde_json::json!({"name": name})).unwrap(),
            package_root_path: root.join(name),
            dbt_properties: vec![],
            analysis_files: vec![],
            check_files: vec![],
            model_sql_files: vec![],
            function_sql_files: vec![],
            macro_files: vec![],
            test_files: vec![],
            fixture_files: vec![],
            seed_files: vec![],
            docs_files: vec![],
            snapshot_files: vec![],
            dependencies: BTreeSet::new(),
            all_paths: HashMap::new(),
            embedded_file_contents: None,
            raw_project_yml: dbt_yaml::Value::default(),
        }
    }

    fn make_internal_package(root: &Path, name: &str) -> DbtPackage {
        let internal_root = root.join("dbt_internal_packages").join(name);
        DbtPackage {
            dbt_project: serde_json::from_value(serde_json::json!({"name": name})).unwrap(),
            package_root_path: internal_root,
            dbt_properties: vec![],
            analysis_files: vec![],
            check_files: vec![],
            model_sql_files: vec![],
            function_sql_files: vec![],
            macro_files: vec![],
            test_files: vec![],
            fixture_files: vec![],
            seed_files: vec![],
            docs_files: vec![],
            snapshot_files: vec![],
            dependencies: BTreeSet::new(),
            all_paths: HashMap::new(),
            embedded_file_contents: Some(HashMap::new()),
            raw_project_yml: dbt_yaml::Value::default(),
        }
    }

    fn make_inline_package(out_dir: &Path) -> DbtPackage {
        let asset = DbtAsset {
            base_path: out_dir.to_path_buf(),
            path: PathBuf::from("inline_00000000.sql"),
            original_path: PathBuf::from("inline_00000000.sql"),
            package_name: String::new(),
        };
        DbtPackage {
            package_root_path: out_dir.to_path_buf(),
            model_sql_files: vec![asset],
            ..DbtPackage::default()
        }
    }

    /// snapshot_packages must exclude packages whose path contains dbt_internal_packages/.
    /// This is the regression test for the double-add bug: if internal packages were saved to
    /// parquet, the loader would add them again on reload, producing m+2n packages where m+n
    /// are expected.
    #[test]
    fn snapshot_packages_excludes_internal_packages() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let user = make_user_package(root, "my_project");
        let dep = make_user_package(root, "my_dep");
        let internal = make_internal_package(root, "dbt_utils");

        let all = vec![user, dep, internal];
        let snaps = snapshot_packages(&all);

        assert_eq!(
            snaps.len(),
            2,
            "only user packages should be serialized; internal packages must be excluded"
        );
        assert!(
            snaps.iter().any(|s| s.package_name == "my_project"),
            "root package must be present"
        );
        assert!(
            snaps.iter().any(|s| s.package_name == "my_dep"),
            "dep package must be present"
        );
        assert!(
            snaps.iter().all(|s| s.package_name != "dbt_utils"),
            "internal package must be excluded"
        );
    }

    /// snapshot_packages must exclude the inline ("") package. It holds the
    /// transient --inline SQL asset which is regenerated (with a fresh uuid) on
    /// every run, so persisting it would cause a stale, empty "" stub to be
    /// restored on the incremental path while `prepare_inline_sql` injects a
    /// fresh one — producing a duplicate "" package.
    #[test]
    fn snapshot_packages_excludes_inline_package() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let user = make_user_package(root, "my_project");
        let inline = make_inline_package(root);

        let snaps = snapshot_packages(&[user, inline]);

        assert_eq!(
            snaps.len(),
            1,
            "the inline package must be excluded from the parquet snapshot"
        );
        assert_eq!(snaps[0].package_name, "my_project");
    }

    /// After a round-trip through snapshot_packages + reconstruct_package_metadata,
    /// the reconstructed package list matches the original user packages.
    #[test]
    fn snapshot_packages_round_trip_excludes_internal() {
        use crate::partial_parse::reconstruct_package_metadata;
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let user = make_user_package(root, "my_project");
        let internal = make_internal_package(root, "dbt_utils");

        let snaps = snapshot_packages(&[user.clone(), internal]);

        // Only the user package should be in snaps.
        assert_eq!(snaps.len(), 1);
        let reconstructed = reconstruct_package_metadata(&snaps[0]).unwrap();
        assert_eq!(reconstructed.dbt_project.name, "my_project");
        assert_eq!(reconstructed.package_root_path, user.package_root_path);
    }
}
