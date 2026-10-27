//! Generic epoch-append parquet infrastructure.
//!
//! All epoch tables share the same file-management pattern:
//! - Files named `v{VERSION}_{N}.parquet` in a directory
//! - Write: serialize rows via `serde_arrow`, compress with ZSTD
//! - Read: deserialize all rows from a file via `serde_arrow`
//! - Epochs: scan directory for versioned files, determine next epoch number
//!
//! This module provides the generic building blocks so each table module
//! only defines its row struct, fields, and domain-specific logic.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use arrow::datatypes::{Field, Schema};
use dbt_common::{ErrorCode, FsError, FsResult, stdfs};
use parquet::{
    arrow::ArrowWriter,
    basic::{Compression, ZstdLevel},
    file::properties::WriterProperties,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_arrow::to_record_batch;

/// Scan `dir` for files matching `{prefix}{N}.parquet`, return sorted by N ascending.
pub fn existing_epochs(dir: &Path, prefix: &str) -> Vec<(u32, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut epochs: Vec<(u32, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()?.to_str()? != "parquet" {
                return None;
            }
            let stem = p.file_stem()?.to_str()?;
            let rest = stem.strip_prefix(prefix)?;
            let n: u32 = rest.parse().ok()?;
            Some((n, p))
        })
        .collect();
    epochs.sort_by_key(|(n, _)| *n);
    epochs
}

/// Next epoch number for the given directory and version prefix.
pub fn next_epoch(dir: &Path, prefix: &str) -> u32 {
    existing_epochs(dir, prefix)
        .last()
        .map(|(n, _)| n + 1)
        .unwrap_or(0)
}

/// Write rows to a parquet file using serde_arrow + ZSTD compression.
///
/// Writes in chunks of 256 rows to keep memory bounded for large batches.
/// The file is written under a temporary name and renamed into place, so a
/// concurrent reader never sees a partially written file.
pub fn write_rows<T: Serialize>(path: &Path, fields: &[Field], rows: &[T]) -> FsResult<()> {
    if let Some(parent) = path.parent() {
        stdfs::create_dir_all(parent)?;
    }
    let tmp = tmp_path(path);
    let res = write_rows_to(&tmp, fields, rows).and_then(|()| stdfs::rename(&tmp, path));
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// Unique sibling path for `path`. The `.tmp` extension keeps it out of
/// [`existing_epochs`].
fn tmp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    path.with_file_name(name)
}

fn write_rows_to<T: Serialize>(path: &Path, fields: &[Field], rows: &[T]) -> FsResult<()> {
    let file = stdfs::File::create(path)?;
    let arrow_schema = Arc::new(Schema::new(fields.to_vec()));
    let field_refs: Vec<_> = arrow_schema.fields().iter().map(Arc::clone).collect();
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(1).unwrap()))
        .build();
    let mut writer = ArrowWriter::try_new(file, arrow_schema, Some(props))
        .map_err(|e| FsError::new(ErrorCode::IoError, format!("ArrowWriter: {e}")))?;
    for chunk in rows.chunks(256) {
        let chunk_refs: Vec<&T> = chunk.iter().collect();
        let batch = to_record_batch(&field_refs, &chunk_refs)
            .map_err(|e| FsError::new(ErrorCode::IoError, format!("serde_arrow: {e}")))?;
        writer
            .write(&batch)
            .map_err(|e| FsError::new(ErrorCode::IoError, format!("write batch: {e}")))?;
    }
    writer
        .close()
        .map_err(|e| FsError::new(ErrorCode::IoError, format!("close: {e}")))?;
    Ok(())
}

/// Read all rows from a parquet file using serde_arrow.
/// Returns empty Vec if the file doesn't exist or can't be read.
pub fn read_rows<T: DeserializeOwned>(path: &Path) -> Vec<T> {
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(builder) = ParquetRecordBatchReaderBuilder::try_new(file) else {
        return Vec::new();
    };
    let Ok(reader) = builder.build() else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for batch in reader {
        // The reader doesn't move past a failed batch, it keeps returning the
        // same error, so give up on the whole file instead of skipping.
        let Ok(batch) = batch else {
            return Vec::new();
        };
        let Ok(mut chunk) = serde_arrow::from_record_batch::<Vec<T>>(&batch) else {
            return Vec::new();
        };
        rows.append(&mut chunk);
    }
    rows
}

/// Delete all epoch files in a directory for the given prefix.
pub fn remove_epochs(dir: &Path, prefix: &str) {
    for (_, path) in existing_epochs(dir, prefix) {
        let _ = std::fs::remove_file(path);
    }
}

/// Returns true when compaction should fire.
///
/// Combines two signals so the epoch tables stay bounded both in space and in
/// read-time file count:
/// - Row-count signal: `delta_len > max(total_alive / 10, 10)` — keeps disk
///   usage within ~2× the minimum (at most one extra 10%-batch on top of the
///   compacted base).
/// - File-count signal: `file_count > 8` — caps read cost at ≤9 files
///   regardless of how large each individual delta was.
pub fn should_compact(delta_len: usize, total_alive: usize, file_count: usize) -> bool {
    let row_threshold = (total_alive / 10).max(10);
    delta_len > row_threshold || file_count > 8
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::DataType;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Row {
        id: i64,
        name: String,
    }

    fn fields() -> Vec<Field> {
        vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]
    }

    fn rows(n: i64) -> Vec<Row> {
        (0..n)
            .map(|id| Row {
                id,
                name: format!("name-{id}"),
            })
            .collect()
    }

    #[test]
    fn write_then_read_leaves_no_tmp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v1_0.parquet");
        write_rows(&path, &fields(), &rows(1000)).unwrap();
        write_rows(&path, &fields(), &rows(10)).unwrap();

        assert_eq!(read_rows::<Row>(&path), rows(10));
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec!["v1_0.parquet"]);
    }

    #[test]
    fn read_rows_gives_up_on_corrupt_pages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v1_0.parquet");
        write_rows(&path, &fields(), &rows(5000)).unwrap();

        // Clobber the data pages but keep the footer, so the reader opens fine
        // and only fails once it starts decoding batches.
        let mut bytes = std::fs::read(&path).unwrap();
        let len = bytes.len();
        bytes[8..len / 2].fill(0xAB);
        std::fs::write(&path, &bytes).unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read_rows::<Row>(&path));
        });
        let got = rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("read_rows did not return");
        assert!(got.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn rewrite_does_not_break_open_reader() {
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v1_0.parquet");
        write_rows(&path, &fields(), &rows(5000)).unwrap();

        let reader = ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&path).unwrap())
            .unwrap()
            .with_batch_size(64)
            .build()
            .unwrap();
        write_rows(&path, &fields(), &rows(10)).unwrap();

        let total: usize = reader.map(|b| b.unwrap().num_rows()).sum();
        assert_eq!(total, 5000);
        assert_eq!(read_rows::<Row>(&path), rows(10));
    }
}
