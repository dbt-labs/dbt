//! Epoch-append parquet for column-level lineage (CLL).
//!
//! Files land at:
//! ```text
//! target/
//!   metadata/compile/column_lineage/v1_{N}.parquet   ← epoch-append, latest-wins by node
//! ```
//!
//! ## Invariant
//! All edges for a given `to_node_unique_id` are written together in a single
//! `write_cll_epoch` call, stamped with the same `ingested_at` (microseconds
//! since Unix epoch UTC, wall-clock time of the invoking run).  Latest-wins is determined by
//! `max(ingested_at)` per node across all epoch files; all edges for the
//! winning node come from exactly one file.  File-number order (the integer in
//! the filename) breaks ties when `ingested_at` values are equal (e.g. in
//! tests).
//!
//! ## Design
//! * **Delta writes** — only edges whose `to_node_unique_id` is in
//!   `recomputed_targets` are written per run.  Pass `None` on a full compile
//!   to write all edges.
//! * **Latest-wins** — for each `to_node_unique_id` the file with the highest
//!   `ingested_at` wins; all its rows replace any earlier file's rows for that
//!   node.  No row-level dedup is needed.
//! * **Compaction** — when delta row count or file count exceeds the threshold
//!   (see [`epoch_io::should_compact`]) epochs are merged into `v1_0.parquet`
//!   and deltas are deleted.  Compaction also evicts rows whose
//!   `to_node_unique_id` is absent from `alive_ids` (deleted nodes); between
//!   compactions ghost edges for deleted nodes are harmless.
//! * **Schema versioning** — the `v1_` filename prefix is [`CLL_SCHEMA_VERSION`].
//!   Files from other versions are silently ignored.  Bump the constant
//!   whenever `CllRow` fields change; old files accumulate until `dbt clean`.
//! * **Compile errors** — if a node fails to compile, CLL produces no rows for
//!   it and `write_cll_epoch` returns early (no file written).  Stale edges
//!   from a previous successful compile survive until the next successful
//!   recompile or compaction.  `ingested_at` lets consumers detect staleness.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use std::sync::Arc;

use arrow::datatypes::{DataType, Field, TimeUnit};
use dbt_common::{FsResult, stdfs};
use serde::{Deserialize, Serialize};

use crate::epoch_io;

// ── constants ─────────────────────────────────────────────────────────────────

const VERSION_PREFIX: &str = "v1_";

// ── row schema ────────────────────────────────────────────────────────────────

/// One row in a CLL epoch file — one directed edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CllRow {
    /// The downstream node — partition key for latest-wins.
    pub to_node_unique_id: String,
    /// The downstream column (`None` for scan-level / node-level edges).
    pub to_column_name: Option<String>,
    /// The upstream node.
    pub from_node_unique_id: String,
    /// The upstream column.
    pub from_column_name: String,
    /// Edge kind (e.g. `"direct"`, `"indirect"`, `"scan"`).
    pub lineage_kind: String,
    /// Microseconds since Unix epoch (UTC) of the invocation that wrote this row.
    /// Latest-wins is `max(ingested_at)` per `to_node_unique_id`.
    pub ingested_at: i64,
}

fn cll_row_fields() -> Vec<Field> {
    vec![
        Field::new("to_node_unique_id", DataType::Utf8, false),
        Field::new("to_column_name", DataType::Utf8, true),
        Field::new("from_node_unique_id", DataType::Utf8, false),
        Field::new("from_column_name", DataType::Utf8, false),
        Field::new("lineage_kind", DataType::Utf8, false),
        Field::new(
            "ingested_at",
            DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC"))),
            false,
        ),
    ]
}

// ── epoch helpers ─────────────────────────────────────────────────────────────

fn existing_epochs(dir: &Path) -> Vec<(u32, PathBuf)> {
    epoch_io::existing_epochs(dir, VERSION_PREFIX)
}

fn epoch_filename(n: u32) -> String {
    format!("{VERSION_PREFIX}{n}.parquet")
}

// ── latest-wins merge ─────────────────────────────────────────────────────────

/// Merges rows from all epoch files applying latest-wins by `to_node_unique_id`.
///
/// Winner per node = file with highest `ingested_at` among rows for that node;
/// file-number order (ascending) is the tiebreaker for equal timestamps.
/// If `alive_ids` is `Some`, rows for absent nodes are dropped.
fn merge_latest(epochs: &[(u32, PathBuf)], alive_ids: Option<&HashSet<String>>) -> Vec<CllRow> {
    // Pass 1: best ingested_at per node, with file-number as tiebreaker.
    // Key: to_node_unique_id → (ingested_at, file_number)
    let mut best: HashMap<String, (i64, u32)> = HashMap::new();
    for (n, path) in epochs {
        for row in epoch_io::read_rows::<CllRow>(path) {
            best.entry(row.to_node_unique_id.clone())
                .and_modify(|(best_ts, best_n)| {
                    if row.ingested_at > *best_ts || (row.ingested_at == *best_ts && n > best_n) {
                        *best_ts = row.ingested_at;
                        *best_n = *n;
                    }
                })
                .or_insert((row.ingested_at, *n));
        }
    }

    // Pass 2: collect winning rows, optionally filtering to alive nodes.
    let mut result = Vec::new();
    for (n, path) in epochs {
        for row in epoch_io::read_rows::<CllRow>(path) {
            if let Some(&(best_ts, best_n)) = best.get(&row.to_node_unique_id) {
                if row.ingested_at == best_ts && *n == best_n {
                    if alive_ids.is_none_or(|ids| ids.contains(&row.to_node_unique_id)) {
                        result.push(row);
                    }
                }
            }
        }
    }
    result
}

// ── compaction ────────────────────────────────────────────────────────────────

/// Merges all epoch files into `{CLL_SCHEMA_VERSION}_0.parquet` (latest-wins),
/// then deletes the delta files.
///
/// If `alive_ids` is `Some`, rows for nodes absent from the set are evicted
/// (deleted nodes).  Between compactions, ghost edges for deleted nodes survive
/// but are harmless — they don't appear in the manifest.
fn compact_epochs(dir: &Path, alive_ids: Option<&HashSet<String>>) -> FsResult<()> {
    let epochs = existing_epochs(dir);
    if epochs.is_empty() {
        return Ok(());
    }

    let mut best = merge_latest(&epochs, alive_ids);

    best.sort_by(|a, b| {
        a.to_node_unique_id
            .cmp(&b.to_node_unique_id)
            .then(a.to_column_name.cmp(&b.to_column_name))
            .then(a.from_node_unique_id.cmp(&b.from_node_unique_id))
            .then(a.from_column_name.cmp(&b.from_column_name))
    });

    let out = dir.join(epoch_filename(0));
    epoch_io::write_rows(&out, &cll_row_fields(), &best)?;

    for (n, path) in &epochs {
        if *n != 0 {
            let _ = std::fs::remove_file(path);
        }
    }

    Ok(())
}

// ── public API ────────────────────────────────────────────────────────────────

/// Writes a delta epoch of CLL rows to `cll_dir/{version}_{N}.parquet`.
///
/// `ingested_at`: wall-clock nanoseconds of the current invocation — all rows
/// written in this call receive the same value, which is the basis for
/// latest-wins resolution across epoch files.
///
/// `recomputed_targets`: filter — only rows whose `to_node_unique_id` is in
/// this set are written.  Pass `None` on a full compile to write all rows.
///
/// `alive_ids`: passed through to compaction when it fires; rows for absent
/// nodes are evicted at that point.
///
/// If `rows` is empty after filtering, no file is written and the function
/// returns early.  A node that fails to compile produces no rows; its stale
/// edges from a previous successful compile survive until the next successful
/// recompile or compaction.
///
/// `alive_node_count`: total alive nodes in the manifest — when `Some`, the
/// row-count compaction signal fires when delta > 10% of alive nodes.
/// Pass `None` to rely on the file-count signal only (fine for tests).
/// `alive_ids`: optional set for pruning dead nodes during compaction.
///
/// Triggers compaction when delta row count or file count exceeds the threshold
/// (see [`epoch_io::should_compact`]).
pub fn write_cll_epoch(
    cll_dir: &Path,
    rows: Vec<CllRow>,
    ingested_at: i64,
    recomputed_targets: Option<&HashSet<String>>,
    alive_node_count: Option<usize>,
    alive_ids: Option<&HashSet<String>>,
) -> FsResult<()> {
    let mut filtered: Vec<CllRow> = if let Some(targets) = recomputed_targets {
        rows.into_iter()
            .filter(|r| targets.contains(&r.to_node_unique_id))
            .collect()
    } else {
        rows
    };

    if filtered.is_empty() {
        return Ok(());
    }

    stdfs::create_dir_all(cll_dir)?;

    let epoch = epoch_io::next_epoch(cll_dir, VERSION_PREFIX);
    for row in &mut filtered {
        row.ingested_at = ingested_at;
    }

    filtered.sort_by(|a, b| {
        a.to_node_unique_id
            .cmp(&b.to_node_unique_id)
            .then(a.to_column_name.cmp(&b.to_column_name))
    });

    let path = cll_dir.join(epoch_filename(epoch));
    epoch_io::write_rows(&path, &cll_row_fields(), &filtered)?;

    // Skip compaction when epoch == 0: we just wrote the full compact form.
    // Compacting immediately would pointlessly re-read + re-write it.
    if epoch > 0 {
        let epoch_count = existing_epochs(cll_dir).len();
        if epoch_io::should_compact(filtered.len(), alive_node_count.unwrap_or(0), epoch_count) {
            let _ = compact_epochs(cll_dir, alive_ids);
        }
    }

    Ok(())
}

/// Reads all CLL rows from `cll_dir`, applying latest-wins by node.
///
/// Primarily used in tests; production queries go through DuckDB views.
/// Returns rows sorted for deterministic output.
pub fn read_cll_latest(cll_dir: &Path) -> Vec<CllRow> {
    let epochs = existing_epochs(cll_dir);
    let mut result = merge_latest(&epochs, None);
    result.sort_by(|a, b| {
        a.to_node_unique_id
            .cmp(&b.to_node_unique_id)
            .then(a.to_column_name.cmp(&b.to_column_name))
            .then(a.from_node_unique_id.cmp(&b.from_node_unique_id))
            .then(a.from_column_name.cmp(&b.from_column_name))
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const T0: i64 = 1_000_000_000;
    const T1: i64 = 2_000_000_000;
    const T2: i64 = 3_000_000_000;

    fn edge(to_node: &str, to_col: Option<&str>, from_node: &str, from_col: &str) -> CllRow {
        CllRow {
            to_node_unique_id: to_node.to_string(),
            to_column_name: to_col.map(str::to_string),
            from_node_unique_id: from_node.to_string(),
            from_column_name: from_col.to_string(),
            lineage_kind: "direct".to_string(),
            ingested_at: 0,
        }
    }

    #[test]
    fn full_compile_writes_all_rows() {
        let dir = TempDir::new().unwrap();
        let cll_dir = dir.path().join("column_lineage");

        let rows = vec![
            edge("model.pkg.b", Some("id"), "model.pkg.a", "id"),
            edge("model.pkg.b", Some("name"), "model.pkg.a", "name"),
            edge("model.pkg.c", Some("id"), "model.pkg.b", "id"),
        ];
        write_cll_epoch(&cll_dir, rows, T0, None, None, None).unwrap();

        let epochs = existing_epochs(&cll_dir);
        assert_eq!(epochs.len(), 1);
        assert_eq!(epochs[0].0, 0);

        let loaded = read_cll_latest(&cll_dir);
        assert_eq!(loaded.len(), 3);
        assert!(loaded.iter().all(|r| r.ingested_at == T0));
    }

    #[test]
    fn incremental_write_adds_new_epoch() {
        let dir = TempDir::new().unwrap();
        let cll_dir = dir.path().join("column_lineage");

        let rows0 = vec![
            edge("model.pkg.b", Some("id"), "model.pkg.a", "id"),
            edge("model.pkg.c", Some("id"), "model.pkg.b", "id"),
        ];
        write_cll_epoch(&cll_dir, rows0, T0, None, None, None).unwrap();

        let mut targets = HashSet::new();
        targets.insert("model.pkg.b".to_string());
        let rows1 = vec![edge("model.pkg.b", Some("id"), "model.pkg.x", "id")];
        write_cll_epoch(&cll_dir, rows1, T1, Some(&targets), None, None).unwrap();

        assert_eq!(existing_epochs(&cll_dir).len(), 2);

        let latest = read_cll_latest(&cll_dir);
        let b_edges: Vec<_> = latest
            .iter()
            .filter(|r| r.to_node_unique_id == "model.pkg.b")
            .collect();
        assert_eq!(b_edges.len(), 1);
        assert_eq!(b_edges[0].from_node_unique_id, "model.pkg.x");
        assert_eq!(b_edges[0].ingested_at, T1);

        let c_edges: Vec<_> = latest
            .iter()
            .filter(|r| r.to_node_unique_id == "model.pkg.c")
            .collect();
        assert_eq!(c_edges.len(), 1);
        assert_eq!(c_edges[0].ingested_at, T0);
    }

    #[test]
    fn filter_excludes_non_recomputed_targets() {
        let dir = TempDir::new().unwrap();
        let cll_dir = dir.path().join("column_lineage");

        let mut targets = HashSet::new();
        targets.insert("model.pkg.b".to_string());

        let rows = vec![
            edge("model.pkg.b", Some("id"), "model.pkg.a", "id"),
            edge("model.pkg.c", Some("id"), "model.pkg.b", "id"),
        ];
        write_cll_epoch(&cll_dir, rows, T0, Some(&targets), None, None).unwrap();

        let loaded = read_cll_latest(&cll_dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].to_node_unique_id, "model.pkg.b");
    }

    #[test]
    fn empty_rows_writes_no_file() {
        let dir = TempDir::new().unwrap();
        let cll_dir = dir.path().join("column_lineage");

        write_cll_epoch(&cll_dir, vec![], T0, None, None, None).unwrap();

        assert!(existing_epochs(&cll_dir).is_empty());
    }

    #[test]
    fn compaction_reduces_to_single_file() {
        let dir = TempDir::new().unwrap();
        let cll_dir = dir.path().join("column_lineage");

        // 9 writes: file-count signal fires at >8 files (epochs 0..=8).
        for i in 0..=8usize {
            let rows = vec![edge(
                "model.pkg.b",
                Some("id"),
                &format!("model.pkg.a{i}"),
                "id",
            )];
            write_cll_epoch(&cll_dir, rows, i as i64 + 1, None, None, None).unwrap();
        }

        let epochs = existing_epochs(&cll_dir);
        assert_eq!(epochs.len(), 1, "should be compacted to one file");
        assert_eq!(epochs[0].0, 0, "compacted file is epoch 0");

        let latest = read_cll_latest(&cll_dir);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].from_node_unique_id, "model.pkg.a8");
    }

    #[test]
    fn scan_level_edge_no_column_name() {
        let dir = TempDir::new().unwrap();
        let cll_dir = dir.path().join("column_lineage");

        let rows = vec![CllRow {
            to_node_unique_id: "model.pkg.b".to_string(),
            to_column_name: None,
            from_node_unique_id: "model.pkg.a".to_string(),
            from_column_name: "".to_string(),
            lineage_kind: "scan".to_string(),
            ingested_at: 0,
        }];
        write_cll_epoch(&cll_dir, rows, T0, None, None, None).unwrap();

        let loaded = read_cll_latest(&cll_dir);
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].to_column_name.is_none());
        assert_eq!(loaded[0].lineage_kind, "scan");
    }

    #[test]
    fn successful_incremental_after_stale_epoch_replaces_edges() {
        let dir = TempDir::new().unwrap();
        let cll_dir = dir.path().join("column_lineage");

        let rows0 = vec![edge("model.pkg.b", Some("id"), "model.pkg.a", "id")];
        write_cll_epoch(&cll_dir, rows0, T0, None, None, None).unwrap();

        let mut targets = HashSet::new();
        targets.insert("model.pkg.b".to_string());
        let rows1 = vec![edge("model.pkg.b", Some("id"), "model.pkg.x", "id")];
        write_cll_epoch(&cll_dir, rows1, T1, Some(&targets), None, None).unwrap();

        let latest = read_cll_latest(&cll_dir);
        let b_edges: Vec<_> = latest
            .iter()
            .filter(|r| r.to_node_unique_id == "model.pkg.b")
            .collect();
        assert_eq!(b_edges.len(), 1);
        assert_eq!(b_edges[0].from_node_unique_id, "model.pkg.x");
    }

    #[test]
    fn old_schema_files_are_ignored() {
        let dir = TempDir::new().unwrap();
        let cll_dir = dir.path().join("column_lineage");
        std::fs::create_dir_all(&cll_dir).unwrap();

        // Simulate a file from an old schema (no version prefix, or different prefix).
        std::fs::write(cll_dir.join("0.parquet"), b"garbage").unwrap();
        std::fs::write(cll_dir.join("old_0.parquet"), b"garbage").unwrap();

        // Current-version write should start at epoch 0 and ignore the old files.
        let rows = vec![edge("model.pkg.b", Some("id"), "model.pkg.a", "id")];
        write_cll_epoch(&cll_dir, rows, T0, None, None, None).unwrap();

        let epochs = existing_epochs(&cll_dir);
        assert_eq!(epochs.len(), 1);
        assert_eq!(epochs[0].0, 0);

        // Old files are still on disk but not returned by read_cll_latest.
        let loaded = read_cll_latest(&cll_dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].to_node_unique_id, "model.pkg.b");
    }

    #[test]
    fn compaction_evicts_dead_nodes() {
        let dir = TempDir::new().unwrap();
        let cll_dir = dir.path().join("column_lineage");

        // Write edges for two nodes.
        let rows = vec![
            edge("model.pkg.a", Some("id"), "model.pkg.src", "id"),
            edge("model.pkg.b", Some("id"), "model.pkg.src", "id"),
        ];
        write_cll_epoch(&cll_dir, rows, T0, None, None, None).unwrap();

        // Compact with only node A alive — B's edges should be evicted.
        let mut alive = HashSet::new();
        alive.insert("model.pkg.a".to_string());
        compact_epochs(&cll_dir, Some(&alive)).unwrap();

        let latest = read_cll_latest(&cll_dir);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].to_node_unique_id, "model.pkg.a");
    }

    #[test]
    fn latest_wins_uses_ingested_at_not_file_order() {
        let dir = TempDir::new().unwrap();
        let cll_dir = dir.path().join("column_lineage");

        // Write epoch 0 with T1 (higher timestamp).
        let rows0 = vec![edge("model.pkg.b", Some("id"), "model.pkg.old", "id")];
        write_cll_epoch(&cll_dir, rows0, T1, None, None, None).unwrap();

        // Write epoch 1 with T0 (lower timestamp — simulates clock skew or reordering).
        let mut targets = HashSet::new();
        targets.insert("model.pkg.b".to_string());
        let rows1 = vec![edge("model.pkg.b", Some("id"), "model.pkg.new", "id")];
        write_cll_epoch(&cll_dir, rows1, T0, Some(&targets), None, None).unwrap();

        // T1 > T0, so epoch 0 wins despite lower file number.
        let latest = read_cll_latest(&cll_dir);
        let b: Vec<_> = latest
            .iter()
            .filter(|r| r.to_node_unique_id == "model.pkg.b")
            .collect();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].from_node_unique_id, "model.pkg.old");

        // Write epoch 2 with T2 (newer than both) — should now win.
        let rows2 = vec![edge("model.pkg.b", Some("id"), "model.pkg.newest", "id")];
        write_cll_epoch(&cll_dir, rows2, T2, Some(&targets), None, None).unwrap();

        let latest2 = read_cll_latest(&cll_dir);
        let b2: Vec<_> = latest2
            .iter()
            .filter(|r| r.to_node_unique_id == "model.pkg.b")
            .collect();
        assert_eq!(b2.len(), 1);
        assert_eq!(b2[0].from_node_unique_id, "model.pkg.newest");
    }
}
