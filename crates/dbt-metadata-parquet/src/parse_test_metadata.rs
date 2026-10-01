//! Epoch-append parquet for parse-time test metadata.
//!
//! Files land at:
//! ```text
//! target/
//!   metadata/parse/test_metadata/v1_{N}.parquet   ← epoch-append, latest-wins by unique_id
//! ```
//!
//! ## Design
//! * One row per test node, written at parse time.
//! * Key: `test_unique_id` (the test node's own unique_id).
//! * `column_name` and `attached_node` are string references — a test can
//!   reference a column that has no `parse/columns` row (no YAML declaration).
//! * `severity`, `warn_if`, `error_if`, `kwargs` come from the test config.
//! * Latest-wins per `unique_id`: a re-parse replaces the row entirely.

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
pub struct ParseTestMetadataRow {
    /// The test node's own unique_id (e.g. `test.pkg.not_null_orders_id.abc123`).
    pub unique_id: String,
    /// Short test name (e.g. `not_null`, `accepted_values`, `expression_is_true`).
    pub test_name: Option<String>,
    /// Package namespace for generic tests (e.g. `dbt_utils`).
    pub test_namespace: Option<String>,
    /// kwargs JSON object (all test arguments).
    pub kwargs: Option<String>,
    /// Column under test, if column-level (None for model-level tests).
    pub column_name: Option<String>,
    /// The model/source node this test is attached to.
    pub attached_node: Option<String>,
    /// Severity: ERROR or WARN.
    pub severity: Option<String>,
    pub warn_if: Option<String>,
    pub error_if: Option<String>,
    pub fail_calc: Option<String>,
    pub store_failures: Option<bool>,
    pub store_failures_as: Option<String>,
    pub ingested_at: i64,
}

fn test_metadata_fields() -> Vec<Field> {
    vec![
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("test_name", DataType::Utf8, true),
        Field::new("test_namespace", DataType::Utf8, true),
        Field::new("kwargs", DataType::Utf8, true),
        Field::new("column_name", DataType::Utf8, true),
        Field::new("attached_node", DataType::Utf8, true),
        Field::new("severity", DataType::Utf8, true),
        Field::new("warn_if", DataType::Utf8, true),
        Field::new("error_if", DataType::Utf8, true),
        Field::new("fail_calc", DataType::Utf8, true),
        Field::new("store_failures", DataType::Boolean, true),
        Field::new("store_failures_as", DataType::Utf8, true),
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

    let mut best: HashMap<String, (i64, ParseTestMetadataRow)> = HashMap::new();
    for (_, path) in &epochs {
        for row in epoch_io::read_rows::<ParseTestMetadataRow>(path) {
            let entry = best
                .entry(row.unique_id.clone())
                .or_insert_with(|| (0, row.clone()));
            if row.ingested_at > entry.0 {
                entry.0 = row.ingested_at;
                entry.1 = row;
            }
        }
    }

    let mut merged: Vec<ParseTestMetadataRow> = best
        .into_values()
        .filter_map(|(_, row)| {
            if let Some(valid) = valid_ids {
                if !valid.contains(&row.unique_id) {
                    return None;
                }
            }
            Some(row)
        })
        .collect();
    merged.sort_by(|a, b| a.unique_id.cmp(&b.unique_id));

    let consolidated = dir.join(format!("{VERSION_PREFIX}0.parquet"));
    let tmp = dir.join(format!("{VERSION_PREFIX}.tmp.parquet"));
    epoch_io::write_rows(&tmp, &test_metadata_fields(), &merged)?;
    stdfs::rename(&tmp, &consolidated)?;

    for (n, path) in &epochs {
        if *n != 0 {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

// ── public API ────────────────────────────────────────────────────────────────

/// Write parse-time test metadata rows for one invocation.
///
/// `recomputed_nodes`: if Some, only these test nodes were re-parsed (delta).
/// If None, this is a full parse (epoch 0).
pub fn write_parse_test_metadata(
    dir: &Path,
    rows: Vec<ParseTestMetadataRow>,
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
    epoch_io::write_rows(&path, &test_metadata_fields(), &rows)?;

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

/// Read all parse test metadata rows (latest-wins per unique_id).
pub fn read_parse_test_metadata(dir: &Path) -> Vec<ParseTestMetadataRow> {
    let epochs = existing_epochs(dir);
    let mut best: HashMap<String, (i64, ParseTestMetadataRow)> = HashMap::new();
    for (_, path) in &epochs {
        for row in epoch_io::read_rows::<ParseTestMetadataRow>(path) {
            let entry = best
                .entry(row.unique_id.clone())
                .or_insert_with(|| (0, row.clone()));
            if row.ingested_at > entry.0 {
                entry.0 = row.ingested_at;
                entry.1 = row;
            }
        }
    }
    let mut result: Vec<ParseTestMetadataRow> = best.into_values().map(|(_, row)| row).collect();
    result.sort_by(|a, b| a.unique_id.cmp(&b.unique_id));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_row(uid: &str, test_name: &str, col: Option<&str>, ts: i64) -> ParseTestMetadataRow {
        ParseTestMetadataRow {
            unique_id: uid.to_string(),
            test_name: Some(test_name.to_string()),
            test_namespace: None,
            kwargs: None,
            column_name: col.map(|s| s.to_string()),
            attached_node: Some("model.pkg.orders".to_string()),
            severity: Some("ERROR".to_string()),
            warn_if: Some("!= 0".to_string()),
            error_if: Some("!= 0".to_string()),
            fail_calc: Some("count(*)".to_string()),
            store_failures: None,
            store_failures_as: None,
            ingested_at: ts,
        }
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let rows = vec![
            make_row("test.pkg.not_null_orders_id.abc", "not_null", Some("id"), 1),
            make_row(
                "test.pkg.accepted_values_orders_status.def",
                "accepted_values",
                Some("status"),
                1,
            ),
        ];
        write_parse_test_metadata(dir.path(), rows, None, None, None).unwrap();
        let back = read_parse_test_metadata(dir.path());
        assert_eq!(back.len(), 2);
    }

    #[test]
    fn latest_wins() {
        let dir = tempfile::tempdir().unwrap();
        let uid = "test.pkg.not_null_orders_id.abc";
        let mut targets = HashSet::new();
        targets.insert(uid.to_string());

        let rows1 = vec![make_row(uid, "not_null", Some("id"), 1)];
        write_parse_test_metadata(dir.path(), rows1, Some(&targets), None, None).unwrap();

        let mut row2 = make_row(uid, "not_null", Some("id"), 2);
        row2.severity = Some("WARN".to_string());
        write_parse_test_metadata(dir.path(), vec![row2], Some(&targets), None, None).unwrap();

        let back = read_parse_test_metadata(dir.path());
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].severity.as_deref(), Some("WARN"));
    }

    #[test]
    fn model_level_test_no_column() {
        let dir = tempfile::tempdir().unwrap();
        let rows = vec![make_row(
            "test.pkg.expression_is_true_orders_total.xyz",
            "expression_is_true",
            None,
            1,
        )];
        write_parse_test_metadata(dir.path(), rows, None, None, None).unwrap();
        let back = read_parse_test_metadata(dir.path());
        assert!(back[0].column_name.is_none());
    }
}
