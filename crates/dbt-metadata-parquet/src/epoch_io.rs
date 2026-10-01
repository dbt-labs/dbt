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
    sync::Arc,
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
pub fn write_rows<T: Serialize>(path: &Path, fields: &[Field], rows: &[T]) -> FsResult<()> {
    if let Some(parent) = path.parent() {
        stdfs::create_dir_all(parent)?;
    }
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
    for batch in reader.flatten() {
        if let Ok(mut chunk) = serde_arrow::from_record_batch::<Vec<T>>(&batch) {
            rows.append(&mut chunk);
        }
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
