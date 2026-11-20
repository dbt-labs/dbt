//! Snapshot parquet for parse/alive — the authority on node liveness.
//!
//! Files land at:
//! ```text
//! target/
//!   private/metadata/parse/alive.parquet   ← snapshot, rewritten every parse
//! ```
//!
//! ## Design
//! * **Snapshot** — full parses replace it; delta parses merge their changed ids.
//!   No epoch logic needed.
//! * **Authority** — a node is alive iff it appears in this file.
//!   All other tables may contain stale rows for deleted nodes;
//!   compaction uses this table as the filter.

use std::collections::HashSet;
use std::path::Path;

use std::sync::Arc;

use arrow::datatypes::{DataType, Field, TimeUnit};
use dbt_common::{ErrorCode, FsError, FsResult};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::{Deserialize, Serialize};

use crate::epoch_io;

// ── row schema ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliveRow {
    pub unique_id: String,
    pub resource_type: String,
    pub ingested_at: i64,
}

fn alive_fields() -> Vec<Field> {
    vec![
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("resource_type", DataType::Utf8, false),
        Field::new(
            "ingested_at",
            DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC"))),
            false,
        ),
    ]
}

// ── write ─────────────────────────────────────────────────────────────────────

pub fn write_alive(path: &Path, rows: &[AliveRow]) -> FsResult<()> {
    epoch_io::write_rows(path, &alive_fields(), rows)
}

// ── read ──────────────────────────────────────────────────────────────────────

pub fn read_alive(path: &Path) -> Vec<AliveRow> {
    epoch_io::read_rows::<AliveRow>(path)
}

/// Read the complete liveness set without treating missing or corrupt metadata as empty.
pub fn read_alive_ids(path: &Path) -> FsResult<Option<HashSet<String>>> {
    Ok(read_alive_snapshot(path)?.map(|rows| rows.into_iter().map(|row| row.unique_id).collect()))
}

/// Read a liveness snapshot, returning `None` only when the file is absent.
pub fn read_alive_snapshot(path: &Path) -> FsResult<Option<Vec<AliveRow>>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .and_then(|builder| builder.build())
        .map_err(|error| FsError::new(ErrorCode::IoError, format!("Read liveness: {error}")))?;
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch
            .map_err(|error| FsError::new(ErrorCode::IoError, format!("Read liveness: {error}")))?;
        let chunk: Vec<AliveRow> = serde_arrow::from_record_batch(&batch)
            .map_err(|error| FsError::new(ErrorCode::IoError, format!("Read liveness: {error}")))?;
        rows.extend(chunk);
    }
    Ok(Some(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_read_alive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alive.parquet");

        let rows = vec![
            AliveRow {
                unique_id: "model.my_project.model_a".to_string(),
                resource_type: "model".to_string(),
                ingested_at: 1_700_000_000_000_000,
            },
            AliveRow {
                unique_id: "source.my_project.raw.users".to_string(),
                resource_type: "source".to_string(),
                ingested_at: 1_700_000_000_000_000,
            },
            AliveRow {
                unique_id: "test.my_project.not_null_a".to_string(),
                resource_type: "test".to_string(),
                ingested_at: 1_700_000_000_000_000,
            },
        ];

        write_alive(&path, &rows).unwrap();

        let read_back = read_alive(&path);
        assert_eq!(read_back.len(), 3);
        assert_eq!(read_back[0].unique_id, "model.my_project.model_a");
        assert_eq!(read_back[0].resource_type, "model");
        assert_eq!(read_back[1].resource_type, "source");
        assert_eq!(read_back[2].resource_type, "test");
        assert_eq!(
            read_alive_ids(&path).unwrap().unwrap(),
            rows.into_iter().map(|row| row.unique_id).collect()
        );
    }

    #[test]
    fn missing_and_empty_liveness_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alive.parquet");
        assert_eq!(read_alive_ids(&path).unwrap(), None);

        write_alive(&path, &[]).unwrap();
        assert_eq!(read_alive_ids(&path).unwrap(), Some(HashSet::new()));
    }

    #[test]
    fn corrupt_liveness_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alive.parquet");
        std::fs::write(&path, b"not parquet").unwrap();
        assert!(read_alive_ids(&path).is_err());
    }

    #[test]
    fn test_overwrite_alive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alive.parquet");

        let rows1 = vec![
            AliveRow {
                unique_id: "model.pkg.a".to_string(),
                resource_type: "model".to_string(),
                ingested_at: 1,
            },
            AliveRow {
                unique_id: "model.pkg.b".to_string(),
                resource_type: "model".to_string(),
                ingested_at: 1,
            },
        ];
        write_alive(&path, &rows1).unwrap();

        let rows2 = vec![AliveRow {
            unique_id: "model.pkg.a".to_string(),
            resource_type: "model".to_string(),
            ingested_at: 2,
        }];
        write_alive(&path, &rows2).unwrap();

        let read_back = read_alive(&path);
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].unique_id, "model.pkg.a");
        assert_eq!(read_back[0].ingested_at, 2);
    }
}
