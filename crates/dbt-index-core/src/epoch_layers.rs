//! Epoch-append parquet layers for dbt-index.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

use crate::IndexError;

/// Epoch file-name prefix. `pub` because the generated epoch views glob on it
/// (`info_schema::epoch_views`), and a mismatch there would silently read no
/// files rather than fail.
pub const SCHEMA_VERSION: &str = "v1";

/// File name of the base epoch — epoch 0, the one rewritten in place by a full
/// parse or a consolidation.
pub fn base_epoch_filename() -> String {
    format!("{SCHEMA_VERSION}_0.parquet")
}

/// List epoch files matching `{SCHEMA_VERSION}_{N}.parquet` in `dir`, sorted ascending.
pub fn existing_epochs(dir: &Path) -> Vec<(u32, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let prefix = format!("{SCHEMA_VERSION}_");
    let mut epochs: Vec<(u32, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()?.to_str()? != "parquet" {
                return None;
            }
            let stem = p.file_stem()?.to_str()?;
            let n_str = stem.strip_prefix(prefix.as_str())?;
            let n: u32 = n_str.parse().ok()?;
            Some((n, p))
        })
        .collect();
    epochs.sort_by_key(|(n, _)| *n);
    epochs
}

/// Writes `batches` as the next epoch file of `dir`, for a table that keeps the rows of every
/// invocation: one file per append. Batches without rows write nothing.
///
/// An append never replaces an epoch. Two writers that pick the same next number, in one
/// invocation or in two, each get a file: the later one takes the number after.
pub fn append_epoch(dir: &Path, batches: &[RecordBatch]) -> Result<(), IndexError> {
    let batches: Vec<&RecordBatch> = batches.iter().filter(|b| b.num_rows() > 0).collect();
    let Some(first) = batches.first() else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)?;
    let tmp = write_tmp(dir, &first.schema(), batches.into_iter().cloned())?;
    let next = existing_epochs(dir).last().map_or(0, |(n, _)| n + 1);
    publish_epoch(dir, &tmp, next)
}

/// Makes `tmp` an epoch of `dir`, trying the number `next` first. A link fails rather than
/// replace an epoch, and then the number after is tried.
fn publish_epoch(dir: &Path, tmp: &Path, mut next: u32) -> Result<(), IndexError> {
    loop {
        let epoch = dir.join(format!("{SCHEMA_VERSION}_{next}.parquet"));
        match std::fs::hard_link(tmp, &epoch) {
            Ok(()) => {
                let _ = std::fs::remove_file(tmp);
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => next += 1,
            // A file system without hard links: a rename, which can't tell another epoch is there.
            Err(_) => {
                std::fs::rename(tmp, &epoch)?;
                return Ok(());
            }
        }
    }
}

/// Merges the epoch files of `dir` into the base epoch when there are more than `threshold`
/// of them, so that a table with a file per append stays quick to read.
///
/// It merges only files that it can read and that have the schema of the newest one. Any
/// other file stays as it is, as does a file appended meanwhile, so no row is lost.
///
/// Merges in one process take turns: tasks that append at once would otherwise merge at
/// once, and one could remove a file that the other's merged file lacks. Two invocations
/// that merge one directory at the same moment are not kept apart.
pub fn merge_epochs(dir: &Path, threshold: usize) -> Result<(), IndexError> {
    static MERGING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _merging = MERGING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let epochs = existing_epochs(dir);
    if epochs.len() <= threshold {
        return Ok(());
    }
    let files: Vec<(PathBuf, SchemaRef, Vec<RecordBatch>)> = epochs
        .iter()
        .filter_map(|(_, path)| {
            let (schema, batches) = read_epoch(path).ok()?;
            Some((path.clone(), schema, batches))
        })
        .collect();
    let Some((_, schema, _)) = files.last() else {
        return Ok(());
    };
    let schema = Arc::clone(schema);
    let same: Vec<&(PathBuf, SchemaRef, Vec<RecordBatch>)> = files
        .iter()
        .filter(|(_, file_schema, _)| *file_schema == schema)
        .collect();
    if same.len() < 2 {
        return Ok(());
    }
    let merged = write_tmp(
        dir,
        &schema,
        same.iter()
            .flat_map(|(_, _, batches)| batches.iter().cloned()),
    )?;
    // The merged file takes the place of the oldest file it read, so the rows keep their
    // order among the files that stay.
    let (target, _, _) = same[0];
    std::fs::rename(merged, target)?;
    for (path, _, _) in &same[1..] {
        // A failure leaves rows twice, never loses them.
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

/// The query that reads every epoch file of `dir`, for a table whose rows are those files.
/// Files take each other's columns by name, with nulls where one lacks a column. `None`
/// while `dir` has no epoch.
pub fn select_epochs(dir: &Path) -> Result<Option<String>, IndexError> {
    if existing_epochs(dir).is_empty() {
        return Ok(None);
    }
    let dir = std::path::absolute(dir)?;
    let epochs = dir.join(format!("{SCHEMA_VERSION}_*.parquet"));
    Ok(Some(format!(
        "SELECT * FROM read_parquet('{}', union_by_name = true)",
        epochs.display().to_string().replace('\'', "''")
    )))
}

fn read_epoch(path: &Path) -> Result<(SchemaRef, Vec<RecordBatch>), IndexError> {
    let error =
        |e: &dyn std::fmt::Display| IndexError::Other(format!("epoch {}: {e}", path.display()));
    let file = std::fs::File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| error(&e))?;
    let schema = Arc::clone(builder.schema());
    let batches = builder
        .build()
        .map_err(|e| error(&e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| error(&e))?;
    Ok((schema, batches))
}

/// Writes `batches` to a file of its own in `dir`, named for this process and moment so that
/// writers at once never share one, and returns its path.
fn write_tmp(
    dir: &Path,
    schema: &SchemaRef,
    batches: impl Iterator<Item = RecordBatch>,
) -> Result<PathBuf, IndexError> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let tmp = dir.join(format!(".{}.{nanos}.parquet.tmp", std::process::id()));
    let error =
        |e: &dyn std::fmt::Display| IndexError::Other(format!("epoch {}: {e}", tmp.display()));
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(
            ZstdLevel::try_new(1).map_err(|e| error(&e))?,
        ))
        .build();
    let mut writer = ArrowWriter::try_new(
        std::fs::File::create(&tmp)?,
        Arc::clone(schema),
        Some(props),
    )
    .map_err(|e| error(&e))?;
    for batch in batches {
        writer.write(&batch).map_err(|e| error(&e))?;
    }
    writer.close().map_err(|e| error(&e))?;
    Ok(tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_epochs_empty_on_missing_dir() {
        let dir = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(existing_epochs(&dir).is_empty());
    }

    #[test]
    fn existing_epochs_lists_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("v1_2.parquet"), b"").unwrap();
        std::fs::write(dir.path().join("v1_0.parquet"), b"").unwrap();
        std::fs::write(dir.path().join("v1_1.parquet"), b"").unwrap();
        std::fs::write(dir.path().join("other.parquet"), b"").unwrap();
        std::fs::write(dir.path().join("v1_0.txt"), b"").unwrap();

        let epochs = existing_epochs(dir.path());
        assert_eq!(epochs.len(), 3);
        assert_eq!(epochs[0].0, 0);
        assert_eq!(epochs[1].0, 1);
        assert_eq!(epochs[2].0, 2);
    }

    use arrow_array::{ArrayRef, Int64Array, StringArray};
    use arrow_schema::{DataType, Field, Schema};

    fn batch(ids: &[i64], with_note: bool) -> RecordBatch {
        let mut fields = vec![Field::new("id", DataType::Int64, false)];
        let mut columns: Vec<ArrayRef> = vec![Arc::new(Int64Array::from(ids.to_vec()))];
        if with_note {
            fields.push(Field::new("note", DataType::Utf8, true));
            columns.push(Arc::new(StringArray::from(vec![Some("n"); ids.len()])));
        }
        RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()
    }

    /// The ids in every epoch of `dir`, file by file.
    fn ids(dir: &Path) -> Vec<Vec<i64>> {
        existing_epochs(dir)
            .iter()
            .filter_map(|(_, path)| read_epoch(path).ok())
            .map(|(_, batches)| {
                batches
                    .iter()
                    .flat_map(|b| {
                        b.column(0)
                            .as_any()
                            .downcast_ref::<Int64Array>()
                            .unwrap()
                            .values()
                            .to_vec()
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn each_append_is_one_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let table = dir.path().join("widget_runs");
        assert!(select_epochs(&table).unwrap().is_none());
        append_epoch(&table, &[batch(&[1, 2], false)]).unwrap();
        append_epoch(&table, &[]).unwrap();
        append_epoch(&table, &[batch(&[3], true)]).unwrap();
        assert_eq!(ids(&table), [vec![1, 2], vec![3]]);
        let select = select_epochs(&table).unwrap().unwrap();
        assert!(
            select.starts_with("SELECT * FROM read_parquet('"),
            "{select}"
        );
        assert!(
            select.ends_with("v1_*.parquet', union_by_name = true)"),
            "{select}"
        );
    }

    /// An append whose epoch number another writer took meanwhile takes the next one.
    #[test]
    fn an_append_never_replaces_an_epoch() {
        let dir = tempfile::tempdir().unwrap();
        append_epoch(dir.path(), &[batch(&[1], false)]).unwrap();
        let schema = batch(&[2], false).schema();
        let tmp = write_tmp(dir.path(), &schema, std::iter::once(batch(&[2], false))).unwrap();
        // Another writer's epoch 1, written after this one counted the epochs.
        let other = write_tmp(dir.path(), &schema, std::iter::once(batch(&[3], false))).unwrap();
        std::fs::rename(other, dir.path().join("v1_1.parquet")).unwrap();
        publish_epoch(dir.path(), &tmp, 1).unwrap();
        assert_eq!(ids(dir.path()), [vec![1], vec![3], vec![2]]);
    }

    #[test]
    fn epochs_merge_past_the_threshold() {
        let dir = tempfile::tempdir().unwrap();
        for id in 0..4 {
            append_epoch(dir.path(), &[batch(&[id], false)]).unwrap();
        }
        merge_epochs(dir.path(), 4).unwrap();
        assert_eq!(
            existing_epochs(dir.path()).len(),
            4,
            "not past the threshold"
        );
        merge_epochs(dir.path(), 3).unwrap();
        assert_eq!(ids(dir.path()), [vec![0, 1, 2, 3]]);
    }

    /// Tasks that append and merge at once lose no row.
    #[test]
    fn appends_and_merges_at_once_keep_every_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let writers: Vec<_> = (0..8)
            .map(|writer| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for row in 0..10 {
                        append_epoch(&path, &[batch(&[writer * 10 + row], false)]).unwrap();
                        merge_epochs(&path, 2).unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let mut all: Vec<i64> = ids(dir.path()).into_iter().flatten().collect();
        all.sort_unstable();
        assert_eq!(all, (0..80).collect::<Vec<i64>>());
    }

    /// A file with another schema, or one that can't be read, is never merged away.
    #[test]
    fn a_merge_leaves_what_it_cannot_merge() {
        let dir = tempfile::tempdir().unwrap();
        append_epoch(dir.path(), &[batch(&[1], false)]).unwrap();
        append_epoch(dir.path(), &[batch(&[2], true)]).unwrap();
        let broken = dir.path().join("v1_7.parquet");
        std::fs::write(&broken, "not parquet").unwrap();
        append_epoch(dir.path(), &[batch(&[3], true)]).unwrap();
        append_epoch(dir.path(), &[batch(&[4], true)]).unwrap();

        merge_epochs(dir.path(), 2).unwrap();
        // The older schema's file stays, the three of the newest schema are one, and the
        // unreadable file is where it was.
        assert_eq!(ids(dir.path()), [vec![1], vec![2, 3, 4]]);
        assert!(broken.exists());
    }
}
