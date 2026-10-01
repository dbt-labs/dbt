//! Append-only parquet for run result records.
//!
//! Files land at:
//! ```text
//! target/
//!   metadata/run/results/v1_{N}.parquet   ← append-only, one row per node per invocation
//! ```
//!
//! ## Design
//! * **Append-only** — each invocation appends a new file with one row per executed node.
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

const CONSOLIDATE_THRESHOLD: usize = 32;
const VERSION_PREFIX: &str = "v1_";

// ── row schema ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeResultRow {
    pub invocation_id: String,
    pub unique_id: String,
    pub status: String,
    pub message: Option<String>,
    pub execution_time: Option<f64>,
    pub thread_id: Option<String>,
    pub failures: Option<i64>,
    pub compiled_code_hash: Option<String>,
    pub relation_name: Option<String>,
    pub adapter_response: Option<String>,
    pub timing: Option<String>,
    pub ingested_at: i64,
}

fn result_fields() -> Vec<Field> {
    vec![
        Field::new("invocation_id", DataType::Utf8, false),
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("message", DataType::Utf8, true),
        Field::new("execution_time", DataType::Float64, true),
        Field::new("thread_id", DataType::Utf8, true),
        Field::new("failures", DataType::Int64, true),
        Field::new("compiled_code_hash", DataType::Utf8, true),
        Field::new("relation_name", DataType::Utf8, true),
        Field::new("adapter_response", DataType::Utf8, true),
        Field::new("timing", DataType::Utf8, true),
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

/// Writes runtime result rows for one invocation to the results directory.
pub fn write_runtime_results(dir: &Path, rows: &[RuntimeResultRow]) -> FsResult<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let n = epoch_io::next_epoch(dir, VERSION_PREFIX);
    let path = dir.join(format!("{VERSION_PREFIX}{n}.parquet"));
    epoch_io::write_rows(&path, &result_fields(), rows)?;

    let files = existing_files(dir);
    if files.len() > CONSOLIDATE_THRESHOLD {
        consolidate(dir, &files)?;
    }
    Ok(())
}

/// Reads all runtime result rows from the directory, ordered by ingested_at.
pub fn read_runtime_results(dir: &Path) -> Vec<RuntimeResultRow> {
    let files = existing_files(dir);
    let mut all_rows = Vec::new();
    for (_, path) in &files {
        all_rows.extend(epoch_io::read_rows::<RuntimeResultRow>(path));
    }
    all_rows.sort_by_key(|r| r.ingested_at);
    all_rows
}

// ── consolidation ─────────────────────────────────────────────────────────────

fn consolidate(dir: &Path, files: &[(u32, PathBuf)]) -> FsResult<()> {
    let mut all_rows = Vec::new();
    for (_, path) in files {
        all_rows.extend(epoch_io::read_rows::<RuntimeResultRow>(path));
    }
    all_rows.sort_by_key(|r| r.ingested_at);

    let consolidated_path = dir.join(format!("{VERSION_PREFIX}0.parquet"));
    let tmp_path = dir.join(format!("{VERSION_PREFIX}.tmp.parquet"));
    epoch_io::write_rows(&tmp_path, &result_fields(), &all_rows)?;

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
    fn test_write_and_read_results() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        let rows = vec![
            RuntimeResultRow {
                invocation_id: "inv-001".to_string(),
                unique_id: "model.my_project.model_a".to_string(),
                status: "success".to_string(),
                message: None,
                execution_time: Some(2.5),
                thread_id: Some("Thread-1".to_string()),
                failures: None,
                compiled_code_hash: Some("abc123".to_string()),
                relation_name: Some("db.schema.model_a".to_string()),
                adapter_response: Some(r#"{"rows_affected": 100}"#.to_string()),
                timing: Some(
                    r#"[{"name":"compile","started_at":"t1","completed_at":"t2"}]"#.to_string(),
                ),
                ingested_at: 1_700_000_000_000_000,
            },
            RuntimeResultRow {
                invocation_id: "inv-001".to_string(),
                unique_id: "model.my_project.model_b".to_string(),
                status: "error".to_string(),
                message: Some("Compilation Error: column 'foo' not found".to_string()),
                execution_time: Some(0.1),
                thread_id: Some("Thread-2".to_string()),
                failures: None,
                compiled_code_hash: None,
                relation_name: None,
                adapter_response: None,
                timing: None,
                ingested_at: 1_700_000_000_000_000,
            },
        ];

        write_runtime_results(dir_path, &rows).unwrap();

        let read_back = read_runtime_results(dir_path);
        assert_eq!(read_back.len(), 2);
        assert_eq!(read_back[0].unique_id, "model.my_project.model_a");
        assert_eq!(read_back[0].status, "success");
        assert_eq!(read_back[1].status, "error");
        assert_eq!(
            read_back[1].message.as_deref(),
            Some("Compilation Error: column 'foo' not found")
        );
    }

    #[test]
    fn test_multiple_invocations_append() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        for i in 0..3 {
            let rows = vec![RuntimeResultRow {
                invocation_id: format!("inv-{i:03}"),
                unique_id: "model.pkg.m".to_string(),
                status: "success".to_string(),
                message: None,
                execution_time: Some(i as f64),
                thread_id: None,
                failures: None,
                compiled_code_hash: None,
                relation_name: None,
                adapter_response: None,
                timing: None,
                ingested_at: 1_700_000_000_000_000 + i as i64,
            }];
            write_runtime_results(dir_path, &rows).unwrap();
        }

        let all = read_runtime_results(dir_path);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].invocation_id, "inv-000");
        assert_eq!(all[2].invocation_id, "inv-002");
        assert_eq!(existing_files(dir_path).len(), 3);
    }

    #[test]
    fn test_consolidation() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path();

        for i in 0..CONSOLIDATE_THRESHOLD + 2 {
            let rows = vec![RuntimeResultRow {
                invocation_id: format!("inv-{i:03}"),
                unique_id: "model.pkg.m".to_string(),
                status: "success".to_string(),
                message: None,
                execution_time: None,
                thread_id: None,
                failures: None,
                compiled_code_hash: None,
                relation_name: None,
                adapter_response: None,
                timing: None,
                ingested_at: i as i64,
            }];
            write_runtime_results(dir_path, &rows).unwrap();
        }

        let files = existing_files(dir_path);
        assert!(files.len() <= 2);

        let all = read_runtime_results(dir_path);
        assert_eq!(all.len(), CONSOLIDATE_THRESHOLD + 2);
    }
}
