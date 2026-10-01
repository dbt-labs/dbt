//! Append-only parquet for warehouse catalog stats records.
//!
//! Files land at:
//! ```text
//! target/
//!   metadata/run/catalog_stats/v1_{N}.parquet   ← append-only, one row per node per invocation
//! ```
//!
//! ## Design
//! * **Append-only** — each invocation appends a new file with one row per node that
//!   has at least one non-null stat.
//! * **File consolidation** — when file count exceeds [`CONSOLIDATE_THRESHOLD`],
//!   all files are merged into a single file preserving all rows.
//! * **Schema versioning** — filename prefix `v1_` allows future schema changes.

use std::path::{Path, PathBuf};

use std::sync::Arc;

use arrow::datatypes::{DataType, Field, TimeUnit};
use dbt_common::{FsResult, stdfs};
use serde::{Deserialize, Serialize};

use crate::epoch_io;

// ── constants ─────────────────────────────────────────────────────────────────

pub const CATALOG_STATS_SUBDIR: &str = "run/catalog_stats";
const CONSOLIDATE_THRESHOLD: usize = 32;
const VERSION_PREFIX: &str = "v1_";

// ── row schema ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogStatEpochRow {
    pub unique_id: String,
    pub table_type: Option<String>,
    pub table_owner: Option<String>,
    pub database_name: Option<String>,
    pub schema_name: Option<String>,
    pub table_name: Option<String>,
    pub row_count: Option<i64>,
    pub bytes: Option<i64>,
    pub last_modified: Option<String>,
    pub ingested_at: i64,
}

fn catalog_stat_fields() -> Vec<Field> {
    vec![
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("table_type", DataType::Utf8, true),
        Field::new("table_owner", DataType::Utf8, true),
        Field::new("database_name", DataType::Utf8, true),
        Field::new("schema_name", DataType::Utf8, true),
        Field::new("table_name", DataType::Utf8, true),
        Field::new("row_count", DataType::Int64, true),
        Field::new("bytes", DataType::Int64, true),
        Field::new("last_modified", DataType::Utf8, true),
        Field::new(
            "ingested_at",
            DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC"))),
            false,
        ),
    ]
}

// ── epoch helpers ─────────────────────────────────────────────────────────────

fn existing_files(dir: &Path) -> Vec<(u32, PathBuf)> {
    epoch_io::existing_epochs(dir, VERSION_PREFIX)
}

// ── public API ────────────────────────────────────────────────────────────────

/// Writes catalog stat rows for one invocation to the catalog_stats directory.
pub fn write_catalog_stats(dir: &Path, rows: &[CatalogStatEpochRow]) -> FsResult<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let n = epoch_io::next_epoch(dir, VERSION_PREFIX);
    let path = dir.join(format!("{VERSION_PREFIX}{n}.parquet"));
    epoch_io::write_rows(&path, &catalog_stat_fields(), rows)?;

    let files = existing_files(dir);
    if files.len() > CONSOLIDATE_THRESHOLD {
        consolidate(dir, &files)?;
    }
    Ok(())
}

/// Reads all catalog stat rows from the directory, ordered by ingested_at.
pub fn read_catalog_stats(dir: &Path) -> Vec<CatalogStatEpochRow> {
    let files = existing_files(dir);
    let mut all_rows = Vec::new();
    for (_, path) in &files {
        all_rows.extend(epoch_io::read_rows::<CatalogStatEpochRow>(path));
    }
    all_rows.sort_by_key(|r| r.ingested_at);
    all_rows
}

// ── consolidation ─────────────────────────────────────────────────────────────

fn consolidate(dir: &Path, files: &[(u32, PathBuf)]) -> FsResult<()> {
    let mut all_rows = Vec::new();
    for (_, path) in files {
        all_rows.extend(epoch_io::read_rows::<CatalogStatEpochRow>(path));
    }
    all_rows.sort_by_key(|r| r.ingested_at);

    let consolidated_path = dir.join(format!("{VERSION_PREFIX}0.parquet"));
    let tmp_path = dir.join(format!("{VERSION_PREFIX}.tmp.parquet"));
    epoch_io::write_rows(&tmp_path, &catalog_stat_fields(), &all_rows)?;

    stdfs::rename(&tmp_path, &consolidated_path)?;
    for (n, path) in files {
        if *n != 0 {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_read_catalog_stats() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        let rows = vec![
            CatalogStatEpochRow {
                unique_id: "model.my_project.orders".to_string(),
                table_type: Some("TABLE".to_string()),
                table_owner: Some("dbt_user".to_string()),
                database_name: Some("analytics".to_string()),
                schema_name: Some("dbt_prod".to_string()),
                table_name: Some("orders".to_string()),
                row_count: Some(42_000),
                bytes: Some(1_048_576),
                last_modified: Some("2026-05-01T00:00:00Z".to_string()),
                ingested_at: 1_700_000_000_000_000,
            },
            CatalogStatEpochRow {
                unique_id: "model.my_project.customers".to_string(),
                table_type: Some("TABLE".to_string()),
                table_owner: None,
                database_name: Some("analytics".to_string()),
                schema_name: Some("dbt_prod".to_string()),
                table_name: Some("customers".to_string()),
                row_count: Some(5_000),
                bytes: None,
                last_modified: None,
                ingested_at: 1_700_000_000_000_001,
            },
        ];

        write_catalog_stats(dir_path, &rows).unwrap();

        let read_back = read_catalog_stats(dir_path);
        assert_eq!(read_back.len(), 2);
        assert_eq!(read_back[0].unique_id, "model.my_project.orders");
        assert_eq!(read_back[0].row_count, Some(42_000));
        assert_eq!(read_back[1].unique_id, "model.my_project.customers");
        assert_eq!(read_back[1].bytes, None);
    }

    #[test]
    fn test_multiple_invocations_append() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        for i in 0..3 {
            let rows = vec![CatalogStatEpochRow {
                unique_id: format!("model.pkg.m{i}"),
                table_type: Some("TABLE".to_string()),
                table_owner: None,
                database_name: None,
                schema_name: None,
                table_name: None,
                row_count: Some(i as i64 * 100),
                bytes: None,
                last_modified: None,
                ingested_at: 1_700_000_000_000_000 + i as i64,
            }];
            write_catalog_stats(dir_path, &rows).unwrap();
        }

        let all = read_catalog_stats(dir_path);
        assert_eq!(all.len(), 3);
        assert_eq!(existing_files(dir_path).len(), 3);
    }
}
