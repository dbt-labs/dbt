//! Epoch-append parquet for compiled node state.
//!
//! Files land at:
//! ```text
//! target/
//!   metadata/compile/nodes/v1_{N}.parquet   ← epoch-append, latest-wins by unique_id
//! ```
//!
//! Design:
//! * **Schema versioning** — files are named `v{VERSION}_{N}.parquet`. On read,
//!   files that don't match the current version prefix are ignored. A schema
//!   change bumps `SCHEMA_VERSION` and old files become invisible.
//! * **Split from parse state** — `metadata/parse/node_facts.{N}.parquet` holds
//!   parse-time fields (`name`, `resource_type`, `depends_on`, etc.).  This
//!   file holds compile-time fields only.  A DuckDB view joins them on
//!   `unique_id` with a LEFT JOIN so parse-only queries never touch this file.
//! * **Delta writes** — only nodes in `recomputed_nodes` are written per run.
//!   A full compile writes epoch 0 with all nodes; incremental compiles write
//!   epoch N with changed nodes only.
//! * **Latest-wins** — partition key is `unique_id`.  The highest `ingested_at`
//!   for a node wins entirely.
//! * **Compaction** — when delta row count or file count exceeds the threshold
//!   (see [`epoch_io::should_compact`]) the epochs are merged into `v1_0.parquet`.
//!   An optional `valid_ids` filter prunes dead nodes during compaction.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use arrow::datatypes::{DataType, Field, TimeUnit};
use dbt_common::{FsResult, stdfs};
use serde::{Deserialize, Serialize};

use crate::epoch_io;

// ── row schema ────────────────────────────────────────────────────────────────

/// One row per compiled node — the compile-time complement to `NodeFactRow`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledNodeRow {
    /// Partition key — matches `unique_id` in `node_facts`.
    pub unique_id: String,
    /// SQL after Jinja rendering and ref/source resolution.
    pub compiled_code: Option<String>,
    /// SHA-256 of `compiled_code` for change detection.
    pub compiled_code_hash: Option<String>,
    /// Project-relative path to the compiled SQL file.
    pub compiled_path: Option<String>,
    /// Resolved grain columns (JSON array string). LP-inferred (GROUP BY / DISTINCT).
    pub grain: String,
    /// Grain from `primary_key` / `grain:` config (JSON array string).
    pub grain_declared: String,
    /// Grain from uniqueness tests and `unique_key` config (JSON array string).
    pub grain_tested: String,
    /// Node-level classifier labels (JSON array string): the merge of
    /// YAML-declared `+classifiers` and propagated model-level labels.
    pub classifiers: String,
    /// Semantic table role (`fact`, `dimension`, `scd`, …).
    pub table_role: Option<String>,
    /// Microseconds since Unix epoch (UTC) — the `run_started_at` of the dbt
    /// invocation that wrote this row.  Latest-wins uses `max(ingested_at)`.
    pub ingested_at: i64,
}

fn compiled_node_fields() -> Vec<Field> {
    vec![
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("compiled_code", DataType::Utf8, true),
        Field::new("compiled_code_hash", DataType::Utf8, true),
        Field::new("compiled_path", DataType::Utf8, true),
        Field::new("grain", DataType::Utf8, false),
        Field::new("grain_declared", DataType::Utf8, false),
        Field::new("grain_tested", DataType::Utf8, false),
        Field::new("classifiers", DataType::Utf8, false),
        Field::new("table_role", DataType::Utf8, true),
        Field::new(
            "ingested_at",
            DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC"))),
            false,
        ),
    ]
}

// ── epoch helpers ─────────────────────────────────────────────────────────────

fn version_prefix() -> &'static str {
    "v1_"
}

fn existing_epochs(dir: &Path) -> Vec<(u32, std::path::PathBuf)> {
    epoch_io::existing_epochs(dir, version_prefix())
}

// ── compaction ────────────────────────────────────────────────────────────────

/// Merges all epoch files into `v1_0.parquet` (latest-wins by `unique_id`).
///
/// When `valid_ids` is `Some`, rows whose `unique_id` is not in the set are
/// pruned.  Pass `None` to only deduplicate without pruning dead nodes.
fn compact_epochs(dir: &Path, valid_ids: Option<&HashSet<String>>) -> FsResult<()> {
    let epochs = existing_epochs(dir);
    if epochs.is_empty() {
        return Ok(());
    }

    let mut best: HashMap<String, CompiledNodeRow> = HashMap::new();
    for (_, path) in &epochs {
        for row in epoch_io::read_rows::<CompiledNodeRow>(path) {
            best.entry(row.unique_id.clone())
                .and_modify(|existing| {
                    if row.ingested_at > existing.ingested_at {
                        *existing = row.clone();
                    }
                })
                .or_insert(row);
        }
    }

    let mut compacted: Vec<CompiledNodeRow> = if let Some(ids) = valid_ids {
        best.into_values()
            .filter(|r| ids.contains(&r.unique_id))
            .collect()
    } else {
        best.into_values().collect()
    };

    compacted.sort_by(|a, b| a.unique_id.cmp(&b.unique_id));

    let out = dir.join(format!("{prefix}0.parquet", prefix = version_prefix()));
    epoch_io::write_rows(&out, &compiled_node_fields(), &compacted)?;

    for (n, path) in &epochs {
        if *n != 0 {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

// ── public API ────────────────────────────────────────────────────────────────

/// Writes a delta epoch of compiled-node rows to `nodes_dir/v1_{N}.parquet`.
///
/// `ingested_at_nanos` should be `run_started_at.timestamp_nanos()` — the
/// wall-clock time when the dbt invocation started.
///
/// Only rows whose `unique_id` is in `recomputed_nodes` are written.
/// Pass `None` for a full compile to write all rows.
///
/// `alive_node_count`: total alive nodes in the manifest — when `Some`, the
/// row-count compaction signal fires when delta > 10% of alive nodes.
/// Pass `None` to rely on the file-count signal only (fine for tests).
/// `valid_ids`: optional set for pruning dead nodes during compaction.
///
/// Triggers compaction when the delta row count or epoch file count exceeds the
/// threshold (see [`epoch_io::should_compact`]).
pub fn write_compiled_nodes(
    nodes_dir: &Path,
    rows: Vec<CompiledNodeRow>,
    recomputed_nodes: Option<&HashSet<String>>,
    alive_node_count: Option<usize>,
    valid_ids: Option<&HashSet<String>>,
) -> FsResult<()> {
    let filtered: Vec<CompiledNodeRow> = if let Some(targets) = recomputed_nodes {
        rows.into_iter()
            .filter(|r| targets.contains(&r.unique_id))
            .collect()
    } else {
        rows
    };

    if filtered.is_empty() {
        return Ok(());
    }

    stdfs::create_dir_all(nodes_dir)?;

    let epoch = epoch_io::next_epoch(nodes_dir, version_prefix());
    let mut sorted = filtered;
    sorted.sort_by(|a, b| a.unique_id.cmp(&b.unique_id));

    let path = nodes_dir.join(format!(
        "{prefix}{epoch}.parquet",
        prefix = version_prefix()
    ));
    epoch_io::write_rows(&path, &compiled_node_fields(), &sorted)?;

    let file_count = existing_epochs(nodes_dir).len();
    if epoch_io::should_compact(sorted.len(), alive_node_count.unwrap_or(0), file_count) {
        let _ = compact_epochs(nodes_dir, valid_ids);
    }
    Ok(())
}

/// Reads compiled-node rows applying latest-wins by `unique_id` (max `ingested_at`).
/// Primarily used in tests; production queries go through DuckDB views.
pub fn read_compiled_nodes_latest(nodes_dir: &Path) -> Vec<CompiledNodeRow> {
    let epochs = existing_epochs(nodes_dir);
    let mut best: HashMap<String, CompiledNodeRow> = HashMap::new();
    for (_, path) in &epochs {
        for row in epoch_io::read_rows::<CompiledNodeRow>(path) {
            best.entry(row.unique_id.clone())
                .and_modify(|existing| {
                    if row.ingested_at > existing.ingested_at {
                        *existing = row.clone();
                    }
                })
                .or_insert(row);
        }
    }
    let mut result: Vec<CompiledNodeRow> = best.into_values().collect();
    result.sort_by(|a, b| a.unique_id.cmp(&b.unique_id));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn node(unique_id: &str, compiled_code: &str, ingested_at: i64) -> CompiledNodeRow {
        CompiledNodeRow {
            unique_id: unique_id.to_string(),
            compiled_code: Some(compiled_code.to_string()),
            compiled_code_hash: None,
            compiled_path: None,
            grain: "[]".to_string(),
            grain_declared: "[]".to_string(),
            grain_tested: "[]".to_string(),
            classifiers: "[]".to_string(),
            table_role: None,
            ingested_at,
        }
    }

    #[test]
    fn full_compile_writes_all_nodes() {
        let dir = TempDir::new().unwrap();
        let nodes_dir = dir.path().join("nodes");

        let rows = vec![
            node("model.pkg.a", "select 1", 1000),
            node("model.pkg.b", "select * from a", 1000),
        ];
        write_compiled_nodes(&nodes_dir, rows, None, None, None).unwrap();

        let epochs = existing_epochs(&nodes_dir);
        assert_eq!(epochs.len(), 1);
        assert_eq!(epochs[0].0, 0);
        assert!(
            epochs[0]
                .1
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("v1_")
        );

        let loaded = read_compiled_nodes_latest(&nodes_dir);
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn incremental_write_latest_wins_per_node() {
        let dir = TempDir::new().unwrap();
        let nodes_dir = dir.path().join("nodes");

        // epoch 0 — full compile at t=1000
        write_compiled_nodes(
            &nodes_dir,
            vec![
                node("model.pkg.a", "select 1", 1000),
                node("model.pkg.b", "select * from a", 1000),
            ],
            None,
            None,
            None,
        )
        .unwrap();

        // epoch 1 — only model.pkg.b recompiled at t=2000
        let mut targets = HashSet::new();
        targets.insert("model.pkg.b".to_string());
        write_compiled_nodes(
            &nodes_dir,
            vec![node("model.pkg.b", "select id from a", 2000)],
            Some(&targets),
            None,
            None,
        )
        .unwrap();

        assert_eq!(existing_epochs(&nodes_dir).len(), 2);

        let latest = read_compiled_nodes_latest(&nodes_dir);
        assert_eq!(latest.len(), 2);

        let b = latest
            .iter()
            .find(|r| r.unique_id == "model.pkg.b")
            .unwrap();
        assert_eq!(b.compiled_code.as_deref(), Some("select id from a"));
        assert_eq!(b.ingested_at, 2000);

        // model.pkg.a unchanged from epoch 0
        let a = latest
            .iter()
            .find(|r| r.unique_id == "model.pkg.a")
            .unwrap();
        assert_eq!(a.compiled_code.as_deref(), Some("select 1"));
        assert_eq!(a.ingested_at, 1000);
    }

    #[test]
    fn filter_excludes_non_recomputed_nodes() {
        let dir = TempDir::new().unwrap();
        let nodes_dir = dir.path().join("nodes");

        let mut targets = HashSet::new();
        targets.insert("model.pkg.b".to_string());

        let rows = vec![
            node("model.pkg.a", "select 1", 1000),
            node("model.pkg.b", "select * from a", 1000),
        ];
        write_compiled_nodes(&nodes_dir, rows, Some(&targets), None, None).unwrap();

        let loaded = read_compiled_nodes_latest(&nodes_dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].unique_id, "model.pkg.b");
    }

    #[test]
    fn empty_rows_writes_no_file() {
        let dir = TempDir::new().unwrap();
        let nodes_dir = dir.path().join("nodes");

        write_compiled_nodes(&nodes_dir, vec![], None, None, None).unwrap();
        assert!(existing_epochs(&nodes_dir).is_empty());
    }

    #[test]
    fn compaction_reduces_to_single_file() {
        let dir = TempDir::new().unwrap();
        let nodes_dir = dir.path().join("nodes");

        // 9 writes: file-count signal fires at >8 files (epochs 0..=8 → 9 files).
        for i in 0..=8usize {
            let rows = vec![node(
                "model.pkg.a",
                &format!("select {i}"),
                (i as i64 + 1) * 1000,
            )];
            write_compiled_nodes(&nodes_dir, rows, None, None, None).unwrap();
        }

        let epochs = existing_epochs(&nodes_dir);
        assert_eq!(epochs.len(), 1, "should compact to one file");
        assert_eq!(epochs[0].0, 0);

        let latest = read_compiled_nodes_latest(&nodes_dir);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].compiled_code.as_deref(), Some("select 8"));
    }

    #[test]
    fn compaction_with_valid_ids_prunes_dead_nodes() {
        let dir = TempDir::new().unwrap();
        let nodes_dir = dir.path().join("nodes");

        // Write enough epochs to trigger compaction (file-count > 8 → 9 files).
        for i in 0..=8usize {
            let rows = vec![
                node(
                    "model.pkg.alive",
                    &format!("select {i}"),
                    (i as i64 + 1) * 1000,
                ),
                node("model.pkg.dead", "select dead", (i as i64 + 1) * 1000),
            ];
            write_compiled_nodes(&nodes_dir, rows, None, None, None).unwrap();
        }

        // Before compaction with valid_ids, both nodes exist
        // Now force compaction with valid_ids
        let mut valid = HashSet::new();
        valid.insert("model.pkg.alive".to_string());
        compact_epochs(&nodes_dir, Some(&valid)).unwrap();

        let latest = read_compiled_nodes_latest(&nodes_dir);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].unique_id, "model.pkg.alive");
    }

    #[test]
    fn grain_fields_round_trip() {
        let dir = TempDir::new().unwrap();
        let nodes_dir = dir.path().join("nodes");

        let rows = vec![CompiledNodeRow {
            unique_id: "model.pkg.orders".to_string(),
            compiled_code: Some("select order_id from raw".to_string()),
            compiled_code_hash: Some("abc123".to_string()),
            compiled_path: Some("target/compiled/orders.sql".to_string()),
            grain: r#"["order_id"]"#.to_string(),
            grain_declared: r#"["order_id"]"#.to_string(),
            grain_tested: r#"[]"#.to_string(),
            classifiers: r#"[]"#.to_string(),
            table_role: Some("fact".to_string()),
            ingested_at: 1_700_000_000_000_000,
        }];
        write_compiled_nodes(&nodes_dir, rows, None, None, None).unwrap();

        let loaded = read_compiled_nodes_latest(&nodes_dir);
        assert_eq!(loaded.len(), 1);
        let r = &loaded[0];
        assert_eq!(r.compiled_code_hash.as_deref(), Some("abc123"));
        assert_eq!(r.table_role.as_deref(), Some("fact"));
        assert_eq!(r.grain, r#"["order_id"]"#);
        assert_eq!(r.grain_declared, r#"["order_id"]"#);
        assert_eq!(r.ingested_at, 1_700_000_000_000_000);
    }

    #[test]
    fn ignores_files_with_wrong_version_prefix() {
        let dir = TempDir::new().unwrap();
        let nodes_dir = dir.path().join("nodes");
        stdfs::create_dir_all(&nodes_dir).unwrap();

        // Write a valid v1 file
        let rows = vec![node("model.pkg.a", "select 1", 1000)];
        write_compiled_nodes(&nodes_dir, rows, None, None, None).unwrap();

        // Manually create a v2 file (future schema) and an unversioned file
        std::fs::write(nodes_dir.join("v2_0.parquet"), b"fake").unwrap();
        std::fs::write(nodes_dir.join("0.parquet"), b"fake").unwrap();

        // Only v1 files are seen
        let epochs = existing_epochs(&nodes_dir);
        assert_eq!(epochs.len(), 1);
        assert_eq!(epochs[0].0, 0);
    }
}
