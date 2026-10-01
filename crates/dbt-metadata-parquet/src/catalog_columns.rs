//! Epoch-append parquet for catalog-time column types and comments.
//!
//! Files land at:
//! ```text
//! target/
//!   metadata/catalog/columns/v1_{N}.parquet   ← epoch-append, latest-wins by unique_id
//! ```
//!
//! ## Design
//! * **Schema versioning** — `v1_` prefix, same pattern as compile/columns.
//! * **Delta writes** — append as next epoch; compaction merges when threshold hit.
//! * **Latest-wins** — partition key is `unique_id`. All columns for a node share
//!   the same `ingested_at`; the set with the highest timestamp wins entirely.
//! * **Compaction** — when delta row count or file count exceeds the threshold
//!   (see [`epoch_io::should_compact`]) epochs are merged into `v1_0.parquet`.
//!   Optional `valid_ids` prunes dead nodes.

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogColumnRow {
    pub unique_id: String,
    pub column_name: String,
    pub column_index: i32,
    pub catalog_type: Option<String>,
    pub catalog_comment: Option<String>,
    pub ingested_at: i64,
}

fn column_fields() -> Vec<Field> {
    vec![
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("column_name", DataType::Utf8, false),
        Field::new("column_index", DataType::Int32, false),
        Field::new("catalog_type", DataType::Utf8, true),
        Field::new("catalog_comment", DataType::Utf8, true),
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

// ── compaction ────────────────────────────────────────────────────────────────

fn compact_epochs(dir: &Path, valid_ids: Option<&HashSet<String>>) -> FsResult<()> {
    let epochs = existing_epochs(dir);
    if epochs.is_empty() {
        return Ok(());
    }

    let mut best: HashMap<String, (i64, Vec<CatalogColumnRow>)> = HashMap::new();
    for (_, path) in &epochs {
        for row in epoch_io::read_rows::<CatalogColumnRow>(path) {
            let entry = best.entry(row.unique_id.clone()).or_insert((0, Vec::new()));
            if row.ingested_at > entry.0 {
                entry.0 = row.ingested_at;
                entry.1.clear();
                entry.1.push(row);
            } else if row.ingested_at == entry.0 {
                entry.1.push(row);
            }
        }
    }

    let mut merged: Vec<CatalogColumnRow> = Vec::new();
    for (uid, (_, rows)) in best {
        if let Some(valid) = valid_ids {
            if !valid.contains(&uid) {
                continue;
            }
        }
        merged.extend(rows);
    }
    merged.sort_by(|a, b| {
        a.unique_id
            .cmp(&b.unique_id)
            .then(a.column_index.cmp(&b.column_index))
    });

    let consolidated = dir.join(format!("{VERSION_PREFIX}0.parquet"));
    let tmp = dir.join(format!("{VERSION_PREFIX}.tmp.parquet"));
    epoch_io::write_rows(&tmp, &column_fields(), &merged)?;
    stdfs::rename(&tmp, &consolidated)?;

    for (n, path) in &epochs {
        if *n != 0 {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

// ── public API ────────────────────────────────────────────────────────────────

/// Write catalog column rows for one invocation.
///
/// `recomputed_nodes`: if Some, only these nodes were fetched (delta write).
/// If None, this is a full catalog fetch (epoch 0).
/// `alive_node_count`: total alive nodes in the manifest — when `Some`, the
/// row-count compaction signal fires when delta > 10% of alive nodes.
/// Pass `None` to rely on the file-count signal only (fine for tests).
/// `valid_ids`: optional set for pruning dead nodes during compaction.
pub fn write_catalog_columns(
    dir: &Path,
    rows: Vec<CatalogColumnRow>,
    recomputed_nodes: Option<&HashSet<String>>,
    alive_node_count: Option<usize>,
    valid_ids: Option<&HashSet<String>>,
) -> FsResult<()> {
    if rows.is_empty() {
        return Ok(());
    }

    let epoch = if recomputed_nodes.is_some() {
        epoch_io::next_epoch(dir, VERSION_PREFIX)
    } else {
        0
    };
    let path = dir.join(format!("{VERSION_PREFIX}{epoch}.parquet"));
    epoch_io::write_rows(&path, &column_fields(), &rows)?;

    if recomputed_nodes.is_none() {
        for (n, p) in existing_epochs(dir) {
            if n != 0 {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    let epochs = existing_epochs(dir);
    if epoch_io::should_compact(rows.len(), alive_node_count.unwrap_or(0), epochs.len()) {
        compact_epochs(dir, valid_ids)?;
    }
    Ok(())
}

/// Read all catalog-column rows (latest-wins per unique_id).
pub fn read_catalog_columns(dir: &Path) -> Vec<CatalogColumnRow> {
    let epochs = existing_epochs(dir);
    let mut best: HashMap<String, (i64, Vec<CatalogColumnRow>)> = HashMap::new();
    for (_, path) in &epochs {
        for row in epoch_io::read_rows::<CatalogColumnRow>(path) {
            let entry = best.entry(row.unique_id.clone()).or_insert((0, Vec::new()));
            if row.ingested_at > entry.0 {
                entry.0 = row.ingested_at;
                entry.1.clear();
                entry.1.push(row);
            } else if row.ingested_at == entry.0 {
                entry.1.push(row);
            }
        }
    }
    let mut result: Vec<CatalogColumnRow> = best.into_values().flat_map(|(_, rows)| rows).collect();
    result.sort_by(|a, b| {
        a.unique_id
            .cmp(&b.unique_id)
            .then(a.column_index.cmp(&b.column_index))
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_read_columns() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        let rows = vec![
            CatalogColumnRow {
                unique_id: "model.pkg.users".to_string(),
                column_name: "id".to_string(),
                column_index: 0,
                catalog_type: Some("INTEGER".to_string()),
                catalog_comment: Some("Primary key".to_string()),
                ingested_at: 100,
            },
            CatalogColumnRow {
                unique_id: "model.pkg.users".to_string(),
                column_name: "email".to_string(),
                column_index: 1,
                catalog_type: Some("VARCHAR".to_string()),
                catalog_comment: None,
                ingested_at: 100,
            },
        ];

        write_catalog_columns(dir_path, rows, None, None, None).unwrap();

        let read_back = read_catalog_columns(dir_path);
        assert_eq!(read_back.len(), 2);
        assert_eq!(read_back[0].column_name, "id");
        assert_eq!(read_back[0].catalog_type.as_deref(), Some("INTEGER"));
        assert_eq!(read_back[0].catalog_comment.as_deref(), Some("Primary key"));
        assert_eq!(read_back[1].column_name, "email");
    }

    #[test]
    fn test_latest_wins_per_node() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        let rows1 = vec![
            CatalogColumnRow {
                unique_id: "model.pkg.a".to_string(),
                column_name: "x".to_string(),
                column_index: 0,
                catalog_type: Some("INT".to_string()),
                catalog_comment: None,
                ingested_at: 1,
            },
            CatalogColumnRow {
                unique_id: "model.pkg.a".to_string(),
                column_name: "y".to_string(),
                column_index: 1,
                catalog_type: Some("INT".to_string()),
                catalog_comment: None,
                ingested_at: 1,
            },
        ];
        let mut targets = HashSet::new();
        targets.insert("model.pkg.a".to_string());
        write_catalog_columns(dir_path, rows1, Some(&targets), None, None).unwrap();

        let rows2 = vec![CatalogColumnRow {
            unique_id: "model.pkg.a".to_string(),
            column_name: "x".to_string(),
            column_index: 0,
            catalog_type: Some("BIGINT".to_string()),
            catalog_comment: Some("Updated".to_string()),
            ingested_at: 2,
        }];
        write_catalog_columns(dir_path, rows2, Some(&targets), None, None).unwrap();

        let read_back = read_catalog_columns(dir_path);
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].catalog_type.as_deref(), Some("BIGINT"));
        assert_eq!(read_back[0].catalog_comment.as_deref(), Some("Updated"));
    }

    #[test]
    fn test_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        let mut targets = HashSet::new();
        targets.insert("model.pkg.m".to_string());

        for i in 0..10usize {
            let rows = vec![CatalogColumnRow {
                unique_id: "model.pkg.m".to_string(),
                column_name: "col".to_string(),
                column_index: 0,
                catalog_type: Some(format!("TYPE_{i}")),
                catalog_comment: None,
                ingested_at: i as i64,
            }];
            write_catalog_columns(dir_path, rows, Some(&targets), None, None).unwrap();
        }

        let epochs = existing_epochs(dir_path);
        assert!(epochs.len() <= 2);

        let read_back = read_catalog_columns(dir_path);
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].catalog_type.as_deref(), Some("TYPE_9"));
    }
}
