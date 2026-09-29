//! Direct parquet writer — bypass DuckDB entirely.
//!
//! Uses `serde_arrow::to_record_batch` to convert in-memory `Serialize` items
//! directly into Arrow `RecordBatch`es, then writes them via `ArrowWriter` with
//! ZSTD compression. No DuckDB driver, no SQL strings, no JSON file I/O.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::Array;
use arrow_schema::{DataType, Field, FieldRef, Schema, SchemaRef, TimeUnit};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;
use serde_json::{Map, Value};

use crate::IndexError;
use crate::db::{DBT_RT_TABLES, DBT_TABLES, write_views_sql};
use crate::ingest::timings;

// ── Generic helpers ─────────────────────────────────────────────────────────

/// Max rows per RecordBatch for the `Value` (JSON) path. Kept small because
/// each `Value` row can be large and serde_arrow walks the JSON map per field.
const CHUNK_SIZE: usize = 256;

/// Max rows per RecordBatch for the typed path (`NodeColumnRow`, etc.).
/// Typed serde_arrow is cheap (~1µs/row), so a large chunk amortizes
/// the ArrowWriter::write() call overhead. 494k node_columns ÷ 256 = 1929
/// write calls; ÷ 65536 = 8 write calls.
const CHUNK_SIZE_TYPED: usize = 65536;

/// A parquet file written to its temp path but not yet moved into place.
///
/// A multi-table index write stages every table first and commits them
/// together, so a failure partway through leaves the previous index untouched
/// rather than a mix of new and stale files. Dropping an uncommitted stage
/// deletes its temp file
#[must_use = "a staged table is discarded unless it is committed"]
pub struct StagedTable {
    tmp: PathBuf,
    dest: PathBuf,
    committed: bool,
}

impl StagedTable {
    /// Create the temp file backing `dest` and a guard that deletes it unless
    /// committed.
    ///
    /// The suffix deliberately differs from the plain `.parquet.tmp` that the
    /// hydration and serve writers use for individual tables in the same
    /// directory. A staged index write holds its temp files open for the whole
    /// write rather than one table, so sharing those names would let the two
    /// truncate each other
    fn create(dest: &Path) -> Result<(Self, std::fs::File), IndexError> {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = dest.with_extension("parquet.staging.tmp");
        let file = std::fs::File::create(&tmp)?;
        let staged = Self {
            tmp,
            dest: dest.to_path_buf(),
            committed: false,
        };
        Ok((staged, file))
    }

    /// Move the staged file onto its destination path
    pub fn commit(mut self) -> Result<(), IndexError> {
        std::fs::rename(&self.tmp, &self.dest)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for StagedTable {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.tmp);
        }
    }
}

/// Move every staged table into place.
///
/// Renaming is a metadata operation, so the window in which the index is
/// half-updated is far shorter than the staging writes that precede it. It is
/// not zero: if a rename fails, the tables already renamed stay in place and
/// the rest are dropped, so an `Err` from here still leaves a mix. The same is
/// true of a crash between renames. Closing that gap entirely needs a directory
/// swap or a pointer file
pub fn commit_staged(staged: Vec<StagedTable>) -> Result<(), IndexError> {
    timings::time(timings::Stage::StagingWrite, || {
        for table in staged {
            table.commit()?;
        }
        Ok(())
    })
}

/// Write a single parquet file from an explicit schema + rows of flat serializable items.
///
/// Writes in chunks of `CHUNK_SIZE` rows to bound peak Arrow memory.
/// Each chunk is converted via `serde_arrow::to_record_batch` and flushed
/// to the `ArrowWriter` before the next chunk is built, so only one
/// chunk's worth of Arrow arrays is live at a time.
fn write_table_items<T: serde::Serialize>(
    path: &Path,
    schema: SchemaRef,
    items: &[T],
) -> Result<(), IndexError> {
    stage_table_items(path, schema, items)?.commit()
}

fn stage_table_items<T: serde::Serialize>(
    path: &Path,
    schema: SchemaRef,
    items: &[T],
) -> Result<StagedTable, IndexError> {
    timings::time(timings::Stage::StagingWrite, || {
        stage_table_items_at(path, schema, items)
    })
}

fn stage_table_items_at<T: serde::Serialize>(
    path: &Path,
    schema: SchemaRef,
    items: &[T],
) -> Result<StagedTable, IndexError> {
    // Any early return below drops `staged`, which deletes the temp file
    let (staged, file) = StagedTable::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(1).unwrap())) // fastest ZSTD; ~same size as level 3 at this scale
        .build();

    let fields: Vec<FieldRef> = schema.fields().iter().map(Arc::clone).collect();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))
        .map_err(|e| IndexError::Other(format!("ArrowWriter: {e}")))?;

    for chunk in items.chunks(CHUNK_SIZE) {
        let batch = serde_arrow::to_record_batch(&fields, &chunk)
            .map_err(|e| IndexError::Other(format!("serde_arrow: {e}")))?;
        writer
            .write(&batch)
            .map_err(|e| IndexError::Other(format!("ArrowWriter write: {e}")))?;
        // batch is dropped here — Arrow arrays freed before next chunk
    }

    writer
        .close()
        .map_err(|e| IndexError::Other(format!("ArrowWriter close: {e}")))?;
    Ok(staged)
}

/// Like `write_table_items` but uses `CHUNK_SIZE_TYPED` (64k rows/batch).
/// Use only with typed structs where serde_arrow is fast.
fn write_table_items_typed<T: serde::Serialize>(
    path: &Path,
    schema: SchemaRef,
    items: &[T],
) -> Result<(), IndexError> {
    timings::time(timings::Stage::StagingWrite, || {
        stage_table_items_typed_at(path, schema, items)
    })?
    .commit()
}

fn stage_table_items_typed_at<T: serde::Serialize>(
    path: &Path,
    schema: SchemaRef,
    items: &[T],
) -> Result<StagedTable, IndexError> {
    // Any early return below drops `staged`, which deletes the temp file
    let (staged, file) = StagedTable::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(1).unwrap()))
        .build();
    let fields: Vec<FieldRef> = schema.fields().iter().map(Arc::clone).collect();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))
        .map_err(|e| IndexError::Other(format!("ArrowWriter: {e}")))?;
    for chunk in items.chunks(CHUNK_SIZE_TYPED) {
        let batch = serde_arrow::to_record_batch(&fields, &chunk)
            .map_err(|e| IndexError::Other(format!("serde_arrow: {e}")))?;
        writer
            .write(&batch)
            .map_err(|e| IndexError::Other(format!("ArrowWriter write: {e}")))?;
    }
    writer
        .close()
        .map_err(|e| IndexError::Other(format!("ArrowWriter close: {e}")))?;
    Ok(staged)
}

/// Convenience wrapper: write a table from `serde_json::Value` rows.
pub fn write_table(path: &Path, schema: SchemaRef, rows: &[Value]) -> Result<(), IndexError> {
    stage_table(path, schema, rows)?.commit()
}

/// Like [`write_table`], but leaves the result staged for a later [`commit_staged`].
pub fn stage_table(
    path: &Path,
    schema: SchemaRef,
    rows: &[Value],
) -> Result<StagedTable, IndexError> {
    // `classifiers` is a non-nullable list column on `nodes`/`node_columns`.
    // serde_arrow requires every non-nullable field to be present in each JSON
    // row, but callers that build rows from `serde_json::Value` (manifest
    // extraction, tests) routinely omit it. Default it to an empty array so the
    // write succeeds, mirroring the catalog-column path which already does this.
    if schema.fields().iter().any(|f| f.name() == "classifiers") {
        let mut owned: Vec<Value> = rows.to_vec();
        for row in &mut owned {
            if let Some(obj) = row.as_object_mut() {
                obj.entry("classifiers")
                    .or_insert_with(|| Value::Array(vec![]));
            }
        }
        return stage_table_items(path, schema, &owned);
    }
    stage_table_items(path, schema, rows)
}

// ── Merge-on-write ──────────────────────────────────────────────────────────
// Three write modes for snapshot tables. Most tables use Overwrite (the default).
// Tables with incremental state (compiled fields on nodes, catalog data) use
// CarryForward or MergePrune to avoid losing data across parse/compile cycles.

/// Write mode for a snapshot table.
pub enum WriteMode<'a> {
    /// Full overwrite. New file replaces old. Default for most tables.
    Overwrite,
    /// Overwrite rows by primary key, but carry forward specified columns from
    /// existing data when the new value is NULL.
    CarryForward {
        key_cols: &'a [&'a str],
        carry_cols: &'a [&'a str],
    },
    /// Merge: keep old rows whose key is NOT in the new batch, prune rows
    /// whose key no longer exists in `valid_ids`.
    MergePrune {
        key_col: &'a str,
        valid_ids: &'a HashSet<String>,
    },
    /// Carry forward specified columns (like `CarryForward`), AND keep old rows
    /// whose key is NOT in the new batch but IS in `valid_ids` (like `MergePrune`).
    /// Use for tables where partial compiles should extend rather than replace.
    CarryForwardMerge {
        key_cols: &'a [&'a str],
        carry_cols: &'a [&'a str],
        valid_ids: &'a HashSet<String>,
    },
    /// Replace all lineage rows for recomputed targets while preserving the
    /// last-known lineage for other valid targets.
    ReplaceColumnLineage {
        recomputed_targets: &'a HashSet<String>,
        valid_node_ids: Option<&'a HashSet<String>>,
    },
}

/// Arrow-native merge-prune: read `path`, drop rows whose `key_col` is in `drop_keys`,
/// write those kept rows + `new_batch` to `path`. No JSON round-trip.
///
/// Returns the total rows written. No-op (writes only `new_batch`) if file absent.
pub fn merge_prune_arrow(
    path: &Path,
    schema: SchemaRef,
    new_batch: arrow_array::RecordBatch,
    key_col: &str,
    drop_keys: &HashSet<String>,
) -> Result<usize, IndexError> {
    timings::time(timings::Stage::StagingWrite, || {
        merge_prune_arrow_at(path, schema, new_batch, key_col, drop_keys)
    })
}

fn merge_prune_arrow_at(
    path: &Path,
    schema: SchemaRef,
    new_batch: arrow_array::RecordBatch,
    key_col: &str,
    drop_keys: &HashSet<String>,
) -> Result<usize, IndexError> {
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(1).unwrap()))
        .build();

    // Write to a temp file, then rename atomically.
    let tmp = path.with_extension("parquet.tmp");
    let file = std::fs::File::create(&tmp)?;
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))
        .map_err(|e| IndexError::Other(format!("ArrowWriter: {e}")))?;

    let mut total = 0usize;

    // Pass 1: old rows, minus those being replaced.
    if path.exists() {
        let old_file = std::fs::File::open(path)?;
        if let Ok(builder) = ParquetRecordBatchReaderBuilder::try_new(old_file) {
            let key_idx = builder.schema().index_of(key_col).ok();
            if let Ok(reader) = builder.build() {
                for batch in reader.flatten() {
                    let keep: Vec<bool> = if let Some(ki) = key_idx {
                        let col = batch.column(ki);
                        let str_arr = col.as_any().downcast_ref::<arrow_array::StringArray>();
                        (0..batch.num_rows())
                            .map(|i| {
                                str_arr
                                    .map(|a| !a.is_null(i) && !drop_keys.contains(a.value(i)))
                                    .unwrap_or(true)
                            })
                            .collect()
                    } else {
                        vec![true; batch.num_rows()]
                    };
                    let indices: Vec<u32> = keep
                        .iter()
                        .enumerate()
                        .filter_map(|(i, &k)| if k { Some(i as u32) } else { None })
                        .collect();
                    if !indices.is_empty() {
                        let idx_arr = arrow_array::UInt32Array::from(indices);
                        let kept = arrow_select::take::take_record_batch(&batch, &idx_arr)
                            .map_err(|e| IndexError::Other(format!("take: {e}")))?;
                        total += kept.num_rows();
                        writer
                            .write(&kept)
                            .map_err(|e| IndexError::Other(format!("write: {e}")))?;
                    }
                }
            }
        }
    }

    // Pass 2: new rows.
    total += new_batch.num_rows();
    if new_batch.num_rows() > 0 {
        writer
            .write(&new_batch)
            .map_err(|e| IndexError::Other(format!("write new: {e}")))?;
    }

    writer
        .close()
        .map_err(|e| IndexError::Other(format!("close: {e}")))?;
    std::fs::rename(&tmp, path)?;
    Ok(total)
}

fn read_parquet_rows(path: &Path) -> Result<Option<Vec<Map<String, Value>>>, IndexError> {
    if !path.exists() {
        return Ok(None);
    }
    let file = std::fs::File::open(path)?;
    let builder = match ParquetRecordBatchReaderBuilder::try_new(file) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    let reader = builder
        .build()
        .map_err(|e| IndexError::Other(format!("read parquet: {e}")))?;

    let mut rows = Vec::new();
    for batch in reader {
        let batch = match batch {
            Ok(batch) => batch,
            Err(_e) => {
                return Ok(None);
            }
        };
        let schema = batch.schema();
        for row_idx in 0..batch.num_rows() {
            let mut row = Map::new();
            for (col_idx, field) in schema.fields().iter().enumerate() {
                let col = batch.column(col_idx);
                let val = arrow_value_at(col, row_idx);
                row.insert(field.name().clone(), val);
            }
            rows.push(row);
        }
    }
    Ok(Some(rows))
}

/// Extract a single cell from an Arrow array as `serde_json::Value`.
fn arrow_value_at(col: &dyn Array, idx: usize) -> Value {
    if col.is_null(idx) {
        return Value::Null;
    }
    match col.data_type() {
        DataType::Utf8 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::StringArray>()
                .unwrap();
            Value::String(arr.value(idx).to_string())
        }
        DataType::Boolean => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::BooleanArray>()
                .unwrap();
            Value::Bool(arr.value(idx))
        }
        DataType::Int64 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::Int64Array>()
                .unwrap();
            Value::Number(arr.value(idx).into())
        }
        DataType::Float64 => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::Float64Array>()
                .unwrap();
            serde_json::Number::from_f64(arr.value(idx))
                .map(Value::Number)
                .unwrap_or(Value::Null)
        }
        DataType::Timestamp(TimeUnit::Microsecond, _) => {
            let arr = col
                .as_any()
                .downcast_ref::<arrow_array::TimestampMicrosecondArray>()
                .unwrap();
            // Convert microseconds-since-epoch back to RFC 3339 string for serde_arrow re-ingestion.
            let micros = arr.value(idx);
            match arrow_array::temporal_conversions::timestamp_us_to_datetime(micros) {
                Some(dt) => Value::String(dt.and_utc().to_rfc3339()),
                None => Value::Null,
            }
        }
        DataType::List(_) => {
            let list_arr = col
                .as_any()
                .downcast_ref::<arrow_array::ListArray>()
                .unwrap();
            let values = list_arr.value(idx);
            if let Some(str_arr) = values.as_any().downcast_ref::<arrow_array::StringArray>() {
                let items: Vec<Value> = (0..str_arr.len())
                    .filter_map(|i| {
                        if str_arr.is_null(i) {
                            None
                        } else {
                            Some(Value::String(str_arr.value(i).to_string()))
                        }
                    })
                    .collect();
                Value::Array(items)
            } else {
                Value::Array(vec![])
            }
        }
        _ => Value::Null,
    }
}

/// Build a composite key string from a row by concatenating key column values.
fn composite_key(row: &Map<String, Value>, key_cols: &[&str]) -> Option<String> {
    let mut key = String::new();
    for (i, &col) in key_cols.iter().enumerate() {
        if i > 0 {
            key.push('\0');
        }
        key.push_str(row.get(col)?.as_str()?);
    }
    Some(key)
}

fn opt_str(v: &Option<String>) -> Value {
    v.as_ref()
        .map(|s| Value::String(s.clone()))
        .unwrap_or(Value::Null)
}

fn str_array(v: &[String]) -> Value {
    Value::Array(v.iter().map(|s| Value::String(s.clone())).collect())
}

/// Apply `CarryForward` merge: for rows in `new_rows` where a carry column is NULL,
/// copy the value from `old_rows` (matched by composite `key_cols`).
fn merge_carry_forward(
    old_rows: &[Map<String, Value>],
    new_rows: &mut [Value],
    key_cols: &[&str],
    carry_cols: &[&str],
) {
    let old_by_key: HashMap<String, &Map<String, Value>> = old_rows
        .iter()
        .filter_map(|row| composite_key(row, key_cols).map(|k| (k, row)))
        .collect();

    for new_row in new_rows.iter_mut() {
        let Some(obj) = new_row.as_object_mut() else {
            continue;
        };
        let Some(key) = composite_key(obj, key_cols) else {
            continue;
        };
        let Some(old) = old_by_key.get(&key) else {
            continue;
        };
        for &col in carry_cols {
            let is_null = obj.get(col).is_none_or(|v| v.is_null());
            if is_null {
                if let Some(old_val) = old.get(col) {
                    if !old_val.is_null() {
                        obj.insert(col.to_string(), old_val.clone());
                    }
                }
            }
        }
    }
}

/// Apply `MergePrune` merge: keep old rows whose key is NOT in the new batch AND
/// whose key IS in `valid_ids`. Prepend them to `new_rows`.
fn merge_prune(
    old_rows: &[Map<String, Value>],
    new_rows: &mut Vec<Value>,
    key_col: &str,
    valid_ids: &HashSet<String>,
) {
    let new_keys: HashSet<&str> = new_rows
        .iter()
        .filter_map(|r| r.get(key_col)?.as_str())
        .collect();

    let mut carried: Vec<Value> = old_rows
        .iter()
        .filter(|row| {
            let Some(key) = row.get(key_col).and_then(|v| v.as_str()) else {
                return false;
            };
            !new_keys.contains(key) && valid_ids.contains(key)
        })
        .map(|row| Value::Object(row.clone()))
        .collect();

    // Prepend old rows before new rows
    carried.append(new_rows);
    *new_rows = carried;
}

/// Apply `CarryForwardMerge`: carry forward specified columns for matching rows,
/// AND keep old rows whose first key column value is in `valid_ids` but not in the new batch.
fn merge_carry_forward_merge(
    old_rows: &[Map<String, Value>],
    new_rows: &mut Vec<Value>,
    key_cols: &[&str],
    carry_cols: &[&str],
    valid_ids: &HashSet<String>,
) {
    // First, carry forward values for matching rows.
    merge_carry_forward(old_rows, new_rows, key_cols, carry_cols);

    // Then, keep old rows not in the new batch whose primary id is still valid.
    let new_keys: HashSet<String> = new_rows
        .iter()
        .filter_map(|r| composite_key(r.as_object()?, key_cols))
        .collect();

    let mut carried: Vec<Value> = old_rows
        .iter()
        .filter(|row| {
            let Some(key) = composite_key(row, key_cols) else {
                return false;
            };
            // Row's primary id must still be valid (node exists in current manifest).
            let Some(primary_id) = row.get(key_cols[0]).and_then(|v| v.as_str()) else {
                return false;
            };
            !new_keys.contains(&key) && valid_ids.contains(primary_id)
        })
        .map(|row| Value::Object(row.clone()))
        .collect();

    carried.append(new_rows);
    *new_rows = carried;
}

fn merge_column_lineage(
    old_rows: &[Map<String, Value>],
    new_rows: &mut Vec<Value>,
    recomputed_targets: &HashSet<String>,
    valid_node_ids: Option<&HashSet<String>>,
) {
    // Build a set of composite keys from new rows to avoid duplicates.
    let new_keys: HashSet<(String, String, String, String)> = new_rows
        .iter()
        .filter_map(|r| {
            let obj = r.as_object()?;
            Some((
                obj.get("from_node_unique_id")?.as_str()?.to_string(),
                obj.get("from_column_name")?.as_str()?.to_string(),
                obj.get("to_node_unique_id")?.as_str()?.to_string(),
                obj.get("to_column_name")?.as_str()?.to_string(),
            ))
        })
        .collect();

    let mut carried: Vec<Value> = old_rows
        .iter()
        .filter(|row| {
            let Some(from_node_id) = row.get("from_node_unique_id").and_then(|v| v.as_str()) else {
                return false;
            };
            let Some(to_node_id) = row.get("to_node_unique_id").and_then(|v| v.as_str()) else {
                return false;
            };

            // Drop rows for nodes no longer in the manifest.
            if let Some(valid) = valid_node_ids {
                if !valid.contains(from_node_id) || !valid.contains(to_node_id) {
                    return false;
                }
            }
            // Drop rows whose to_node was recomputed (replaced by new data).
            if recomputed_targets.contains(to_node_id) {
                return false;
            }
            // Drop rows whose composite key already exists in new data.
            let from_col = row
                .get("from_column_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let to_col = row
                .get("to_column_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            !new_keys.contains(&(
                from_node_id.to_string(),
                from_col.to_string(),
                to_node_id.to_string(),
                to_col.to_string(),
            ))
        })
        .map(|row| Value::Object(row.clone()))
        .collect();

    carried.append(new_rows);
    *new_rows = carried;
}

/// Write a parquet table with merge-on-write semantics.
///
/// Reads existing data if present, applies the merge mode, then writes.
pub fn write_table_merged(
    path: &Path,
    schema: SchemaRef,
    rows: &mut Vec<Value>,
    mode: WriteMode<'_>,
) -> Result<(), IndexError> {
    stage_table_merged(path, schema, rows, mode)?.commit()
}

/// Like [`write_table_merged`], but leaves the result staged for a later
/// [`commit_staged`].
///
/// The merge modes read the table's own destination file, so staging a table
/// without committing it does not change what any other table reads.
pub fn stage_table_merged(
    path: &Path,
    schema: SchemaRef,
    rows: &mut Vec<Value>,
    mode: WriteMode<'_>,
) -> Result<StagedTable, IndexError> {
    match mode {
        WriteMode::Overwrite => {}
        WriteMode::CarryForward {
            key_cols,
            carry_cols,
        } => {
            if let Some(old_rows) = read_parquet_rows(path)? {
                merge_carry_forward(&old_rows, rows, key_cols, carry_cols);
            }
        }
        WriteMode::CarryForwardMerge {
            key_cols,
            carry_cols,
            valid_ids,
        } => {
            if let Some(old_rows) = read_parquet_rows(path)? {
                merge_carry_forward_merge(&old_rows, rows, key_cols, carry_cols, valid_ids);
            }
        }
        WriteMode::MergePrune { key_col, valid_ids } => {
            if let Some(old_rows) = read_parquet_rows(path)? {
                merge_prune(&old_rows, rows, key_col, valid_ids);
            }
        }
        WriteMode::ReplaceColumnLineage {
            recomputed_targets,
            valid_node_ids,
        } => {
            if let Some(old_rows) = read_parquet_rows(path)? {
                merge_column_lineage(&old_rows, rows, recomputed_targets, valid_node_ids);
            }
        }
    }
    // Backfill ingested_at on merged-in rows that lack it (e.g. rows written
    // by cloud enrichment export which bypasses the normal ingestion path).
    if schema.fields().iter().any(|f| f.name() == "ingested_at") {
        let now = Value::String(chrono::Utc::now().to_rfc3339());
        for row in rows.iter_mut() {
            if let Some(obj) = row.as_object_mut() {
                if matches!(obj.get("ingested_at"), None | Some(Value::Null)) {
                    obj.insert("ingested_at".to_string(), now.clone());
                }
            }
        }
    }
    stage_table(path, schema, rows)
}

// ── Streaming IndexWriter ───────────────────────────────────────────────────
// Writes tables one-at-a-time so callers can drop rows between tables.
// Peak memory = one table's rows + one RecordBatch, not all tables at once.

/// Streaming parquet index writer.
///
/// Write tables individually via [`IndexWriter::write_dbt_table`] /
/// [`IndexWriter::write_dbt_items`] for `dbt.*` tables, and
/// [`IndexWriter::write_rt_table`] / [`IndexWriter::write_rt_items`]
/// for `dbt_rt.*` tables. All tables use snapshot semantics (single file,
/// full overwrite). Call [`IndexWriter::finish`] to write `views.sql`
/// and return the total file count.
pub struct IndexWriter {
    index_dir: PathBuf,
    now: String,
    count: usize,
}

impl IndexWriter {
    pub fn new(index_dir: &Path) -> Result<Self, IndexError> {
        std::fs::create_dir_all(index_dir)?;
        Ok(Self {
            index_dir: index_dir.to_path_buf(),
            now: chrono::Utc::now().to_rfc3339(),
            count: 0,
        })
    }

    /// Write a `dbt.*` snapshot table from JSON Value rows. Auto-injects `ingested_at`.
    /// If `rows` is empty an empty parquet file with the correct schema is written.
    pub fn write_dbt_table(&mut self, table: &str, mut rows: Vec<Value>) -> Result<(), IndexError> {
        let schema = schema_for(table);
        for row in &mut rows {
            if let Some(obj) = row.as_object_mut() {
                obj.insert("ingested_at".to_string(), Value::String(self.now.clone()));
            }
        }
        let path = self.index_dir.join(format!("dbt.{table}.parquet"));
        write_table(&path, schema, &rows)?;
        self.count += 1;
        Ok(())
    }

    /// Append rows to a `dbt.*` table, preserving existing rows.
    /// Deduplicates by `key_col` (new rows win). Auto-injects `ingested_at`.
    pub fn write_dbt_table_append(
        &mut self,
        table: &str,
        mut new_rows: Vec<Value>,
        key_col: &str,
    ) -> Result<(), IndexError> {
        let schema = schema_for(table);
        for row in &mut new_rows {
            if let Some(obj) = row.as_object_mut() {
                obj.insert("ingested_at".to_string(), Value::String(self.now.clone()));
            }
        }
        let path = self.index_dir.join(format!("dbt.{table}.parquet"));
        let mut rows: Vec<Value> = Vec::new();

        if let Some(existing) = read_parquet_rows(&path)? {
            rows.extend(existing.into_iter().map(Value::Object));
        }

        let new_keys: HashSet<String> = new_rows
            .iter()
            .filter_map(|r| r.get(key_col)?.as_str().map(|s| s.to_string()))
            .collect();
        rows.retain(|r| {
            r.get(key_col)
                .and_then(|v| v.as_str())
                .map(|k| !new_keys.contains(k))
                .unwrap_or(true)
        });
        rows.extend(new_rows);

        write_table(&path, schema, &rows)?;
        self.count += 1;
        Ok(())
    }

    /// Write a `dbt.*` snapshot table with merge-on-write semantics.
    /// Auto-injects `ingested_at`, then applies the specified merge mode.
    pub fn write_dbt_table_merged(
        &mut self,
        table: &str,
        mut rows: Vec<Value>,
        mode: WriteMode<'_>,
    ) -> Result<(), IndexError> {
        let schema = schema_for(table);
        for row in &mut rows {
            if let Some(obj) = row.as_object_mut() {
                obj.insert("ingested_at".to_string(), Value::String(self.now.clone()));
            }
        }
        let path = self.index_dir.join(format!("dbt.{table}.parquet"));
        write_table_merged(&path, schema, &mut rows, mode)?;
        self.count += 1;
        Ok(())
    }

    /// Write a `dbt_rt.*` snapshot table from JSON Value rows.
    pub fn write_rt_table(&mut self, table: &str, rows: Vec<Value>) -> Result<(), IndexError> {
        let schema = schema_for(table);
        let path = self.index_dir.join(format!("dbt_rt.{table}.parquet"));
        write_table(&path, schema, &rows)?;
        self.count += 1;
        Ok(())
    }

    /// Append rows to a `dbt_rt.*` table, preserving existing rows.
    /// Reads existing data first, appends new rows, deduplicates by `key_col`.
    pub fn write_rt_table_append(
        &mut self,
        table: &str,
        new_rows: Vec<Value>,
        key_col: &str,
    ) -> Result<(), IndexError> {
        let schema = schema_for(table);
        let path = self.index_dir.join(format!("dbt_rt.{table}.parquet"));
        let mut rows: Vec<Value> = Vec::new();

        // Read existing rows from the file
        if let Some(existing) = read_parquet_rows(&path)? {
            rows.extend(existing.into_iter().map(Value::Object));
        }

        // Append new rows, dedup by key (new rows win)
        let new_keys: HashSet<String> = new_rows
            .iter()
            .filter_map(|r| r.get(key_col)?.as_str().map(|s| s.to_string()))
            .collect();
        rows.retain(|r| {
            r.get(key_col)
                .and_then(|v| v.as_str())
                .map(|k| !new_keys.contains(k))
                .unwrap_or(true)
        });
        rows.extend(new_rows);

        write_table(&path, schema, &rows)?;
        self.count += 1;
        Ok(())
    }

    /// Write a `dbt.*` snapshot table from typed Serialize rows.
    /// The row structs must include `ingested_at` as their last field.
    /// No `serde_json::Value` intermediate — `serde_arrow` walks the Serialize impl directly.
    /// Uses `CHUNK_SIZE_TYPED` (64k rows/batch) to minimize ArrowWriter call overhead.
    pub fn write_dbt_items<T: serde::Serialize>(
        &mut self,
        table: &str,
        items: &[T],
    ) -> Result<(), IndexError> {
        let schema = schema_for(table);
        let path = self.index_dir.join(format!("dbt.{table}.parquet"));
        write_table_items_typed(&path, schema, items)?;
        self.count += 1;
        Ok(())
    }

    /// Write a `dbt.*` snapshot table from typed Serialize rows with merge-on-write.
    ///
    /// Serializes items to `Vec<Value>`, applies the merge mode against
    /// existing parquet data, then writes. Slightly more overhead than
    /// `write_dbt_items` due to the JSON round-trip, but these are small
    /// tables (~500 rows for nodes in a large project).
    pub fn write_dbt_items_merged<T: serde::Serialize>(
        &mut self,
        table: &str,
        items: &[T],
        mode: WriteMode<'_>,
    ) -> Result<(), IndexError> {
        let schema = schema_for(table);
        let mut rows: Vec<Value> = items
            .iter()
            .filter_map(|item| serde_json::to_value(item).ok())
            .collect();
        let path = self.index_dir.join(format!("dbt.{table}.parquet"));
        write_table_merged(&path, schema, &mut rows, mode)?;
        self.count += 1;
        Ok(())
    }

    /// Arrow-native MergePrune: serialize `items` to a RecordBatch, then merge with
    /// the existing parquet file by dropping rows whose `key_col` is in `drop_keys`.
    /// No JSON round-trip for the old rows — reads and filters Arrow directly.
    pub fn write_dbt_items_merge_prune_arrow<T: serde::Serialize>(
        &mut self,
        table: &str,
        items: &[T],
        key_col: &str,
        drop_keys: &HashSet<String>,
    ) -> Result<(), IndexError> {
        let schema = schema_for(table);
        let path = self.index_dir.join(format!("dbt.{table}.parquet"));
        let fields: Vec<FieldRef> = schema.fields().iter().map(Arc::clone).collect();
        // Serialize new items to a single RecordBatch.
        let new_batch = if items.is_empty() {
            arrow_array::RecordBatch::new_empty(schema.clone())
        } else {
            serde_arrow::to_record_batch(&fields, &items)
                .map_err(|e| IndexError::Other(format!("serde_arrow: {e}")))?
        };
        merge_prune_arrow(&path, schema, new_batch, key_col, drop_keys)?;
        self.count += 1;
        Ok(())
    }

    /// Write a `dbt.*` table from pre-built Arrow RecordBatches (zero serde overhead).
    pub fn write_arrow_batches(
        &mut self,
        table: &str,
        batches: Vec<arrow_array::RecordBatch>,
    ) -> Result<(), IndexError> {
        timings::time(timings::Stage::StagingWrite, || {
            self.write_arrow_batches_at(table, batches)
        })
    }

    fn write_arrow_batches_at(
        &mut self,
        table: &str,
        batches: Vec<arrow_array::RecordBatch>,
    ) -> Result<(), IndexError> {
        let schema = schema_for(table);
        let path = self.index_dir.join(format!("dbt.{table}.parquet"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("parquet.tmp");
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(1).unwrap()))
            .build();
        let file = std::fs::File::create(&tmp)?;
        let mut writer = ArrowWriter::try_new(file, schema, Some(props))
            .map_err(|e| IndexError::Other(format!("ArrowWriter: {e}")))?;
        for batch in batches {
            writer
                .write(&batch)
                .map_err(|e| IndexError::Other(format!("ArrowWriter write: {e}")))?;
        }
        writer
            .close()
            .map_err(|e| IndexError::Other(format!("ArrowWriter close: {e}")))?;
        std::fs::rename(&tmp, &path)?;
        self.count += 1;
        Ok(())
    }

    /// Merge-prune a `dbt.*` table using pre-built Arrow RecordBatches.
    pub fn write_arrow_batches_merge_prune(
        &mut self,
        table: &str,
        new_batches: Vec<arrow_array::RecordBatch>,
        key_col: &str,
        drop_keys: &HashSet<String>,
    ) -> Result<(), IndexError> {
        timings::time(timings::Stage::StagingWrite, || {
            self.write_arrow_batches_merge_prune_at(table, new_batches, key_col, drop_keys)
        })
    }

    fn write_arrow_batches_merge_prune_at(
        &mut self,
        table: &str,
        new_batches: Vec<arrow_array::RecordBatch>,
        key_col: &str,
        drop_keys: &HashSet<String>,
    ) -> Result<(), IndexError> {
        let schema = schema_for(table);
        let path = self.index_dir.join(format!("dbt.{table}.parquet"));
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(1).unwrap()))
            .build();
        let tmp = path.with_extension("parquet.tmp");
        let file = std::fs::File::create(&tmp)?;
        let mut writer = ArrowWriter::try_new(file, schema, Some(props))
            .map_err(|e| IndexError::Other(format!("ArrowWriter: {e}")))?;
        // Pass 1: old rows minus drop_keys
        if path.exists() {
            let old_file = std::fs::File::open(&path)?;
            if let Ok(builder) = ParquetRecordBatchReaderBuilder::try_new(old_file) {
                let key_idx = builder.schema().index_of(key_col).ok();
                if let Ok(reader) = builder.build() {
                    for batch in reader.flatten() {
                        let indices: Vec<u32> = if let Some(ki) = key_idx {
                            let col = batch.column(ki);
                            let sa = col.as_any().downcast_ref::<arrow_array::StringArray>();
                            (0..batch.num_rows() as u32)
                                .filter(|&i| {
                                    sa.map(|a| {
                                        !a.is_null(i as usize)
                                            && !drop_keys.contains(a.value(i as usize))
                                    })
                                    .unwrap_or(true)
                                })
                                .collect()
                        } else {
                            (0..batch.num_rows() as u32).collect()
                        };
                        if !indices.is_empty() {
                            let idx_arr = arrow_array::UInt32Array::from(indices);
                            let kept = arrow_select::take::take_record_batch(&batch, &idx_arr)
                                .map_err(|e| IndexError::Other(format!("take: {e}")))?;
                            writer
                                .write(&kept)
                                .map_err(|e| IndexError::Other(format!("write: {e}")))?;
                        }
                    }
                }
            }
        }
        // Pass 2: new batches
        for batch in new_batches {
            if batch.num_rows() > 0 {
                writer
                    .write(&batch)
                    .map_err(|e| IndexError::Other(format!("write new: {e}")))?;
            }
        }
        writer
            .close()
            .map_err(|e| IndexError::Other(format!("close: {e}")))?;
        std::fs::rename(&tmp, &path)?;
        self.count += 1;
        Ok(())
    }

    /// Update the compile-filled columns on an existing `dbt.nodes` parquet, keyed by `unique_id`.
    ///
    /// The counterpart of the join `write_parse_nodes` performs when it builds node rows: same
    /// eight fields, same source, but applied to rows that already exist rather than to rows being
    /// constructed. That distinction is the point — it makes the compile layer applicable whenever
    /// it happens to land, instead of only while the parse epochs are still unconsumed.
    ///
    /// Mirrors the DuckDB path's `UPDATE dbt.nodes SET compiled_code = src.compiled_code, … FROM
    /// read_parquet(…) WHERE dbt.nodes.unique_id = src.unique_id`, including its `WHERE`: a node
    /// absent from `compile_map` is left exactly as it is, so a partial compile extends the index
    /// rather than truncating it. `classifiers` is updated here but not in the DuckDB statement,
    /// because on this path it is the compile epoch that populates it.
    ///
    /// Rows whose compile columns already hold these values are left alone, and a call that changes
    /// nothing writes no file. That makes it safe to call unconditionally after `write_parse_nodes`,
    /// which has usually applied the same map to the subset of nodes it rebuilt.
    ///
    /// Returns the number of rows changed.
    pub fn merge_compiled_nodes_into_nodes(
        &mut self,
        compile_map: &crate::ingest::metadata_to_parquet::CompileMap,
    ) -> Result<usize, IndexError> {
        fn set_if_changed(row: &mut Map<String, Value>, col: &str, val: Value, changed: &mut bool) {
            if row.get(col) != Some(&val) {
                row.insert(col.to_string(), val);
                *changed = true;
            }
        }

        let table = "nodes";
        let path = self.index_dir.join(format!("dbt.{table}.parquet"));
        let Some(existing) = read_parquet_rows(&path)? else {
            return Ok(0);
        };

        let mut updated = 0usize;
        let mut rows: Vec<Value> = Vec::with_capacity(existing.len());
        for mut row in existing {
            let uid = row
                .get("unique_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if let Some(c) = compile_map.get(&uid) {
                let mut changed = false;
                let r = &mut row;
                set_if_changed(r, "compiled_code", opt_str(&c.compiled_code), &mut changed);
                set_if_changed(
                    r,
                    "compiled_code_hash",
                    opt_str(&c.compiled_code_hash),
                    &mut changed,
                );
                set_if_changed(r, "compiled_path", opt_str(&c.compiled_path), &mut changed);
                set_if_changed(r, "grain", str_array(&c.grain), &mut changed);
                set_if_changed(
                    r,
                    "grain_declared",
                    str_array(&c.grain_declared),
                    &mut changed,
                );
                set_if_changed(r, "grain_tested", str_array(&c.grain_tested), &mut changed);
                set_if_changed(r, "classifiers", str_array(&c.classifiers), &mut changed);
                set_if_changed(r, "table_role", opt_str(&c.table_role), &mut changed);
                if changed {
                    row.insert("ingested_at".to_string(), Value::String(self.now.clone()));
                    updated += 1;
                }
            }
            rows.push(Value::Object(row));
        }

        if updated == 0 {
            return Ok(0);
        }
        write_table(&path, schema_for(table), &rows)?;
        self.count += 1;
        Ok(updated)
    }

    /// Merge compile-column data into an existing `node_columns` parquet file.
    ///
    /// Semantics mirror the DuckDB path's ON CONFLICT behaviour:
    /// - For rows matching on (unique_id, column_name): update column_index and
    ///   inferred_type from compile data; COALESCE(compile.description, parse.description).
    /// - Parse-originated fields (declared_type, tags, meta, etc.) are preserved.
    /// - Compile rows with no existing parse match are appended as new rows.
    /// - Parse rows with no matching compile data are kept as-is.
    pub fn merge_compile_columns_into_node_columns(
        &mut self,
        compile_batches: Vec<arrow_array::RecordBatch>,
    ) -> Result<(), IndexError> {
        use arrow_array::Array;
        use std::collections::HashMap;

        let table = "node_columns";
        let path = self.index_dir.join(format!("dbt.{table}.parquet"));

        // Build lookup keyed by (unique_id, LOWERCASED column_name) so compile
        // rows match the parse rows case-insensitively. Snowflake uppercases
        // unquoted identifiers in the inferred schema (`EMAIL`) while parse rows
        // use the manifest casing (`email`); a case-sensitive key would miss the
        // match and append a duplicate row, splitting classifiers across two rows.
        struct CompileInfo {
            column_name: String,
            column_index: Option<i64>,
            inferred_type: Option<String>,
            description: Option<String>,
            classifiers: Vec<String>,
        }
        let mut compile_map: HashMap<(String, String), CompileInfo> = HashMap::new();
        for batch in &compile_batches {
            let uid_col = batch
                .column_by_name("unique_id")
                .and_then(|c| c.as_any().downcast_ref::<arrow_array::StringArray>());
            let cname_col = batch
                .column_by_name("column_name")
                .and_then(|c| c.as_any().downcast_ref::<arrow_array::StringArray>());
            let idx_col = batch
                .column_by_name("column_index")
                .and_then(|c| c.as_any().downcast_ref::<arrow_array::Int64Array>());
            let itype_col = batch
                .column_by_name("inferred_type")
                .and_then(|c| c.as_any().downcast_ref::<arrow_array::StringArray>());
            let desc_col = batch
                .column_by_name("description")
                .and_then(|c| c.as_any().downcast_ref::<arrow_array::StringArray>());
            let class_col = batch
                .column_by_name("classifiers")
                .and_then(|c| c.as_any().downcast_ref::<arrow_array::ListArray>());
            for i in 0..batch.num_rows() {
                let Some(uid) = uid_col.filter(|c| !c.is_null(i)).map(|c| c.value(i)) else {
                    continue;
                };
                let Some(cname) = cname_col.filter(|c| !c.is_null(i)).map(|c| c.value(i)) else {
                    continue;
                };
                let classifiers: Vec<String> = class_col
                    .filter(|c| !c.is_null(i))
                    .map(|c| {
                        let item = c.value(i);
                        item.as_any()
                            .downcast_ref::<arrow_array::StringArray>()
                            .map(|sa| {
                                (0..sa.len())
                                    .filter(|&j| !sa.is_null(j))
                                    .map(|j| sa.value(j).to_string())
                                    .collect()
                            })
                            .unwrap_or_default()
                    })
                    .unwrap_or_default();
                compile_map.insert(
                    (uid.to_string(), cname.to_ascii_lowercase()),
                    CompileInfo {
                        column_name: cname.to_string(),
                        column_index: idx_col.filter(|c| !c.is_null(i)).map(|c| c.value(i)),
                        inferred_type: itype_col
                            .filter(|c| !c.is_null(i))
                            .map(|c| c.value(i).to_string()),
                        description: desc_col
                            .filter(|c| !c.is_null(i))
                            .map(|c| c.value(i).to_string()),
                        classifiers,
                    },
                );
            }
        }

        // Read existing parse columns, merge compile fields, collect as JSON rows.
        let mut rows: Vec<Value> = Vec::new();
        let mut seen_keys: HashSet<(String, String)> = HashSet::new();

        if let Some(existing) = read_parquet_rows(&path)? {
            for mut map in existing {
                let uid = map
                    .get("unique_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let cname = map
                    .get("column_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let key = (uid, cname.to_ascii_lowercase());
                if let Some(ci) = compile_map.get(&key) {
                    if let Some(idx) = ci.column_index {
                        map.insert("column_index".to_string(), Value::Number(idx.into()));
                    }
                    if let Some(ref it) = ci.inferred_type {
                        map.insert("inferred_type".to_string(), Value::String(it.clone()));
                    }
                    // COALESCE: only override description if compile has one
                    if let Some(ref d) = ci.description {
                        map.insert("description".to_string(), Value::String(d.clone()));
                    }
                    // Classifiers are authoritative from the compile epoch (the
                    // merged declared ∪ propagated set); overwrite the parse
                    // placeholder when present.
                    if !ci.classifiers.is_empty() {
                        map.insert(
                            "classifiers".to_string(),
                            Value::Array(
                                ci.classifiers
                                    .iter()
                                    .map(|s| Value::String(s.clone()))
                                    .collect(),
                            ),
                        );
                    }
                }
                seen_keys.insert(key);
                rows.push(Value::Object(map));
            }
        }

        // Append compile rows that had no existing parse match. `cname` here is
        // the lowercased match key; use `ci.column_name` for the stored name to
        // preserve the original (warehouse) casing.
        for ((uid, cname), ci) in &compile_map {
            if seen_keys.contains(&(uid.clone(), cname.clone())) {
                continue;
            }
            let mut obj = Map::new();
            obj.insert("unique_id".to_string(), Value::String(uid.clone()));
            obj.insert(
                "column_name".to_string(),
                Value::String(ci.column_name.clone()),
            );
            if let Some(idx) = ci.column_index {
                obj.insert("column_index".to_string(), Value::Number(idx.into()));
            }
            if let Some(ref it) = ci.inferred_type {
                obj.insert("inferred_type".to_string(), Value::String(it.clone()));
            }
            if let Some(ref d) = ci.description {
                obj.insert("description".to_string(), Value::String(d.clone()));
            }
            obj.insert("tags".to_string(), Value::Array(vec![]));
            obj.insert(
                "classifiers".to_string(),
                Value::Array(
                    ci.classifiers
                        .iter()
                        .map(|s| Value::String(s.clone()))
                        .collect(),
                ),
            );
            obj.insert("ingested_at".to_string(), Value::String(self.now.clone()));
            rows.push(Value::Object(obj));
        }

        // Write merged result
        let schema = schema_for(table);
        write_table(&path, schema, &rows)?;
        self.count += 1;
        Ok(())
    }

    /// Merge catalog columns into the existing `node_columns` parquet file.
    ///
    /// For matching `(unique_id, column_name)` pairs: sets `catalog_type`,
    /// `catalog_comment`, and computes `data_type = coalesce(catalog_type, inferred_type)`.
    /// New columns discovered only via catalog are appended as new rows.
    pub fn merge_catalog_columns_into_node_columns(
        &mut self,
        catalog_batches: Vec<arrow_array::RecordBatch>,
    ) -> Result<(), IndexError> {
        use arrow_array::Array;
        use std::collections::HashMap;

        let table = "node_columns";
        let path = self.index_dir.join(format!("dbt.{table}.parquet"));

        // Key by (unique_id, LOWERCASED column_name) so catalog rows match the
        // existing parse/compile rows case-insensitively. Snowflake uppercases
        // unquoted identifiers in the catalog (`EMAIL`) while the merged rows use
        // the manifest casing (`email`); a case-sensitive key would miss the
        // match and append a duplicate row, splitting a column's classifiers and
        // types across two rows.
        struct CatalogInfo {
            column_name: String,
            column_index: Option<i64>,
            catalog_type: Option<String>,
            catalog_comment: Option<String>,
        }
        let mut catalog_map: HashMap<(String, String), CatalogInfo> = HashMap::new();
        for batch in &catalog_batches {
            let uid_col = batch
                .column_by_name("unique_id")
                .and_then(|c| c.as_any().downcast_ref::<arrow_array::StringArray>());
            let cname_col = batch
                .column_by_name("column_name")
                .and_then(|c| c.as_any().downcast_ref::<arrow_array::StringArray>());
            let idx_col = batch
                .column_by_name("column_index")
                .and_then(|c| c.as_any().downcast_ref::<arrow_array::Int64Array>());
            let ctype_col = batch
                .column_by_name("catalog_type")
                .and_then(|c| c.as_any().downcast_ref::<arrow_array::StringArray>());
            let ccomment_col = batch
                .column_by_name("catalog_comment")
                .and_then(|c| c.as_any().downcast_ref::<arrow_array::StringArray>());
            for i in 0..batch.num_rows() {
                let Some(uid) = uid_col.filter(|c| !c.is_null(i)).map(|c| c.value(i)) else {
                    continue;
                };
                let Some(cname) = cname_col.filter(|c| !c.is_null(i)).map(|c| c.value(i)) else {
                    continue;
                };
                catalog_map.insert(
                    (uid.to_string(), cname.to_ascii_lowercase()),
                    CatalogInfo {
                        column_name: cname.to_string(),
                        column_index: idx_col.filter(|c| !c.is_null(i)).map(|c| c.value(i)),
                        catalog_type: ctype_col
                            .filter(|c| !c.is_null(i))
                            .map(|c| c.value(i).to_string()),
                        catalog_comment: ccomment_col
                            .filter(|c| !c.is_null(i))
                            .map(|c| c.value(i).to_string()),
                    },
                );
            }
        }

        let mut rows: Vec<Value> = Vec::new();
        let mut seen_keys: HashSet<(String, String)> = HashSet::new();

        if let Some(existing) = read_parquet_rows(&path)? {
            for mut map in existing {
                let uid = map
                    .get("unique_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let cname = map
                    .get("column_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let key = (uid, cname.to_ascii_lowercase());
                if let Some(ci) = catalog_map.get(&key) {
                    if let Some(idx) = ci.column_index {
                        map.insert("column_index".to_string(), Value::Number(idx.into()));
                    }
                    if let Some(ref ct) = ci.catalog_type {
                        map.insert("catalog_type".to_string(), Value::String(ct.clone()));
                        // data_type = coalesce(catalog_type, inferred_type)
                        map.insert("data_type".to_string(), Value::String(ct.clone()));
                    } else {
                        // No catalog_type: data_type = inferred_type (preserve existing)
                        let inferred = map
                            .get("inferred_type")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        if let Some(it) = inferred {
                            map.insert("data_type".to_string(), Value::String(it));
                        }
                    }
                    if let Some(ref cc) = ci.catalog_comment {
                        map.insert("catalog_comment".to_string(), Value::String(cc.clone()));
                    }
                }
                seen_keys.insert(key);
                rows.push(Value::Object(map));
            }
        }

        // Append catalog rows that had no existing match. `cname` here is the
        // lowercased match key; use `ci.column_name` for the stored name to
        // preserve the original (warehouse) casing.
        for ((uid, cname), ci) in &catalog_map {
            if seen_keys.contains(&(uid.clone(), cname.clone())) {
                continue;
            }
            let mut obj = Map::new();
            obj.insert("unique_id".to_string(), Value::String(uid.clone()));
            obj.insert(
                "column_name".to_string(),
                Value::String(ci.column_name.clone()),
            );
            if let Some(idx) = ci.column_index {
                obj.insert("column_index".to_string(), Value::Number(idx.into()));
            }
            if let Some(ref ct) = ci.catalog_type {
                obj.insert("catalog_type".to_string(), Value::String(ct.clone()));
                obj.insert("data_type".to_string(), Value::String(ct.clone()));
            }
            if let Some(ref cc) = ci.catalog_comment {
                obj.insert("catalog_comment".to_string(), Value::String(cc.clone()));
            }
            obj.insert("tags".to_string(), Value::Array(vec![]));
            // `classifiers` is a non-nullable list column; catalog-only columns
            // carry none, but the field must be present for serde_arrow.
            obj.insert("classifiers".to_string(), Value::Array(vec![]));
            obj.insert("ingested_at".to_string(), Value::String(self.now.clone()));
            rows.push(Value::Object(obj));
        }

        let schema = schema_for(table);
        write_table(&path, schema, &rows)?;
        self.count += 1;
        Ok(())
    }

    /// Write a `dbt_rt.*` snapshot table from typed Serialize rows.
    pub fn write_rt_items<T: serde::Serialize>(
        &mut self,
        table: &str,
        items: &[T],
    ) -> Result<(), IndexError> {
        let schema = schema_for(table);
        let path = self.index_dir.join(format!("dbt_rt.{table}.parquet"));
        write_table_items(&path, schema, items)?;
        self.count += 1;
        Ok(())
    }

    /// Write empty parquet files for any `dbt.*` tables not yet written.
    pub fn ensure_dbt_tables(&mut self) -> Result<(), IndexError> {
        for table in DBT_TABLES {
            let path = self.index_dir.join(format!("dbt.{table}.parquet"));
            if !path.exists() {
                let schema = schema_for(table);
                write_table_items::<Value>(&path, schema, &[])?;
            }
        }
        Ok(())
    }

    /// Write empty parquet files for any `dbt_rt.*` tables not yet written.
    pub fn ensure_rt_tables(&mut self) -> Result<(), IndexError> {
        for table in DBT_RT_TABLES {
            let path = self.index_dir.join(format!("dbt_rt.{table}.parquet"));
            if !path.exists() {
                let schema = schema_for(table);
                write_table_items::<Value>(&path, schema, &[])?;
            }
        }
        Ok(())
    }

    /// Get the timestamp for this writer (for setting `ingested_at` on row structs).
    pub fn now(&self) -> &str {
        &self.now
    }

    pub fn index_dir(&self) -> &Path {
        &self.index_dir
    }

    /// Ensure all tables exist (writing empty parquet for missing ones),
    /// write `views.sql`, and return total files written.
    /// Standard finish: writes empty schema-placeholder parquet files for any
    /// tables not yet written (for standalone DuckDB / views.sql compatibility),
    /// then writes views.sql. Placeholders are only written for files that
    /// don't exist yet, so repeated ingests into the same index dir pay a
    /// stat per table, not a rewrite.
    pub fn finish(mut self) -> Result<usize, IndexError> {
        self.ensure_dbt_tables()?;
        self.ensure_rt_tables()?;
        write_views_sql(&self.index_dir)?;
        Ok(self.count)
    }
}

/// Very simple hash for raw_code (not cryptographic). Every writer of
/// `dbt.nodes.raw_code_hash` must use this so the two ingest paths stamp
/// identical values for identical code
pub fn raw_code_hash(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

// ── Schema definitions ──────────────────────────────────────────────────────
// One per table. Column order matches ddl.sql.
// All columns nullable except where noted, for robustness against missing data.

fn utf8(name: &str) -> Field {
    Field::new(name, DataType::Utf8, true)
}
fn utf8_nn(name: &str) -> Field {
    Field::new(name, DataType::Utf8, false)
}
fn bool_f(name: &str) -> Field {
    Field::new(name, DataType::Boolean, true)
}
fn bool_nn(name: &str) -> Field {
    Field::new(name, DataType::Boolean, false)
}
fn int_f(name: &str) -> Field {
    Field::new(name, DataType::Int64, true)
}
fn float_f(name: &str) -> Field {
    Field::new(name, DataType::Float64, true)
}
fn list_utf8_nn(name: &str) -> Field {
    Field::new(
        name,
        DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
        false,
    )
}
fn timestamp_f(name: &str) -> Field {
    Field::new(
        name,
        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
        true,
    )
}
fn timestamp_nn(name: &str) -> Field {
    Field::new(
        name,
        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
        false,
    )
}

/// Define a `#[derive(Serialize)]` row struct AND its corresponding `schema_*()` function
/// from a single field list. Eliminates duplication between typed rows and Arrow schemas.
///
/// Each field is annotated with an Arrow type tag:
/// - `utf8` → nullable Utf8
/// - `utf8!` → non-null Utf8
/// - `bool` → nullable Boolean
/// - `bool!` → non-null Boolean
/// - `int` → nullable Int64
/// - `int!` → non-null Int64
/// - `float` → nullable Float64
/// - `list_utf8!` → non-null List<Utf8> (string array, empty list = no items)
///
/// Usage:
/// ```ignore
/// define_row! {
///     /// doc comment
///     pub struct GroupRow<'a> => schema_groups {
///         [utf8!] pub unique_id: &'a str,
///         [utf8!] pub name: &'a str,
///         [utf8]  pub description: Option<&'a str>,
///     }
/// }
/// ```
macro_rules! define_row {
    // Lifetime variant
    (
        $(#[$meta:meta])*
        pub struct $name:ident <$lt:lifetime> => $schema_fn:ident {
            $([$arrow:ident $($nn:tt)?] pub $field:ident : $ty:ty),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(serde::Serialize)]
        pub struct $name<$lt> {
            $(pub $field: $ty),*
        }

        fn $schema_fn() -> SchemaRef {
            Arc::new(Schema::new(vec![
                $(define_row!(@field stringify!($field), $arrow $($nn)?)),*
            ]))
        }
    };
    // No-lifetime variant
    (
        $(#[$meta:meta])*
        pub struct $name:ident => $schema_fn:ident {
            $([$arrow:ident $($nn:tt)?] pub $field:ident : $ty:ty),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(serde::Serialize)]
        pub struct $name {
            $(pub $field: $ty),*
        }

        fn $schema_fn() -> SchemaRef {
            Arc::new(Schema::new(vec![
                $(define_row!(@field stringify!($field), $arrow $($nn)?)),*
            ]))
        }
    };
    // Internal: map arrow tag to Field constructor
    (@field $name:expr, utf8 !) => { utf8_nn($name) };
    (@field $name:expr, utf8)    => { utf8($name) };
    (@field $name:expr, bool !)  => { bool_nn($name) };
    (@field $name:expr, bool)    => { bool_f($name) };
    (@field $name:expr, int)     => { int_f($name) };
    (@field $name:expr, float)   => { float_f($name) };
    (@field $name:expr, list_utf8 !) => { list_utf8_nn($name) };
    (@field $name:expr, timestamp !) => { timestamp_nn($name) };
    (@field $name:expr, timestamp)   => { timestamp_f($name) };
}

// ── dbt.* row types + schemas (generated by define_row!) ────────────────────
// Each `define_row!` produces BOTH a `#[derive(Serialize)]` struct AND the
// corresponding `schema_*() -> SchemaRef` function from a single definition.
// Field annotations: [utf8!] = non-null Utf8, [utf8] = nullable Utf8,
// [bool!]/[bool] = Boolean, [int!]/[int] = Int64, [float] = nullable Float64.

define_row! {
    /// dbt.project row.
    pub struct ProjectRow<'a> => schema_project {
        [utf8!] pub project_name: &'a str,
        [utf8]  pub project_id: Option<&'a str>,
        [utf8]  pub description: Option<&'a str>,
        [utf8]  pub dbt_version: &'a str,
        [utf8]  pub adapter_type: Option<&'a str>,
        [utf8]  pub quoting: Option<&'a str>,
        [utf8]  pub ai_context: Option<&'a str>,
        [utf8]  pub git_sha: Option<&'a str>,
        [utf8]  pub git_branch: Option<&'a str>,
        [bool]  pub git_is_dirty: Option<bool>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.packages row.
    pub struct PackageRow<'a> => schema_packages {
        [utf8!] pub package_name: &'a str,
        [utf8]  pub package_source: Option<&'a str>,
        [utf8]  pub version: Option<&'a str>,
        [utf8]  pub git_url: Option<&'a str>,
        [utf8]  pub git_revision: Option<&'a str>,
        [utf8]  pub local_path: Option<&'a str>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.project_vars row.
    pub struct ProjectVarRow<'a> => schema_project_vars {
        [utf8!] pub project_name: &'a str,
        [utf8!] pub var_name: &'a str,
        [utf8]  pub var_value: String,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.project_env_vars row.
    pub struct ProjectEnvVarRow<'a> => schema_project_env_vars {
        [utf8!] pub env_var_name: &'a str,
        [list_utf8!] pub used_in: Vec<String>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.nodes row — also used for sources.
    /// Path fields are `String` (owned) because `PathBuf::to_string_lossy()` returns `Cow`.
    pub struct NodeRow<'a> => schema_nodes {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub name: Option<&'a str>,
        [utf8]  pub resource_type: &'a str,
        [utf8]  pub package_name: Option<&'a str>,
        [utf8]  pub file_path: Option<String>,
        [utf8]  pub original_file_path: Option<String>,
        [list_utf8!] pub fqn: Vec<String>,
        [utf8]  pub alias: Option<&'a str>,
        [utf8]  pub checksum: Option<&'a str>,
        [utf8]  pub description: Option<&'a str>,
        [utf8]  pub node_language: Option<&'a str>,
        [utf8]  pub raw_code: Option<&'a str>,
        [utf8]  pub database_name: Option<&'a str>,
        [utf8]  pub schema_name: Option<&'a str>,
        [utf8]  pub relation_name: Option<&'a str>,
        [utf8]  pub identifier: Option<&'a str>,
        [bool]  pub enabled: Option<bool>,
        [utf8]  pub materialized: Option<String>,
        [utf8]  pub incremental_strategy: Option<String>,
        [utf8]  pub on_schema_change: Option<String>,
        [utf8]  pub unique_key: Option<String>,
        [bool]  pub full_refresh: Option<bool>,
        [utf8]  pub persist_docs: Option<String>,
        [utf8]  pub pre_hook: Option<String>,
        [utf8]  pub post_hook: Option<String>,
        [utf8]  pub grants: Option<String>,
        [utf8]  pub config: Option<String>,
        [utf8]  pub access_level: Option<String>,
        [utf8]  pub group_name: Option<&'a str>,
        [bool!] pub contract_enforced: bool,
        [utf8]  pub version: Option<String>,
        [utf8]  pub latest_version: Option<String>,
        [utf8]  pub deprecation_date: Option<&'a str>,
        [utf8]  pub node_constraints: Option<String>,
        [list_utf8!] pub primary_key: Vec<String>,
        [bool!] pub docs_show: bool,
        [utf8]  pub patch_path: Option<String>,
        [utf8]  pub time_spine: Option<String>,
        [list_utf8!] pub tags: Vec<String>,
        [list_utf8!] pub classifiers: Vec<String>,
        [utf8]  pub meta: Option<String>,
        [utf8]  pub ai_context: Option<String>,
        [utf8]  pub source_name: Option<&'a str>,
        [utf8]  pub source_description: Option<&'a str>,
        [utf8]  pub loader: Option<&'a str>,
        [utf8]  pub loaded_at_field: Option<&'a str>,
        [utf8]  pub loaded_at_query: Option<&'a str>,
        [utf8]  pub freshness: Option<String>,
        [utf8]  pub external_config: Option<String>,
        [utf8]  pub source_meta: Option<String>,
        [utf8]  pub quoting: Option<String>,
        [utf8]  pub compiled_code: Option<&'a str>,
        [utf8]  pub compiled_code_hash: Option<&'a str>,
        [utf8]  pub compiled_path: Option<&'a str>,
        [utf8]  pub extra_ctes: Option<String>,
        [timestamp] pub compiled_at: Option<&'a str>,
        [utf8]  pub raw_code_hash: Option<String>,
        [utf8]  pub search_text: Option<&'a str>,
        [list_utf8!] pub grain: Vec<String>,
        [list_utf8!] pub grain_declared: Vec<String>,
        [list_utf8!] pub grain_tested: Vec<String>,
        [list_utf8!] pub grain_inferred: Vec<String>,
        [utf8]  pub table_role: Option<&'a str>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.node_columns row.
    ///
    /// Column types have three sources (provenance):
    /// - `declared_type`: from YAML schema (user-declared)
    /// - `inferred_type`: from SQL comprehension (Fusion's type inference)
    /// - `catalog_type`: from warehouse catalog (dbt docs generate)
    /// - `data_type`: resolved = coalesce(catalog_type, inferred_type)
    pub struct NodeColumnRow<'a> => schema_node_columns {
        [utf8!] pub unique_id: &'a str,
        [utf8!] pub column_name: &'a str,
        [int]   pub column_index: Option<i64>,
        [utf8]  pub declared_type: Option<&'a str>,
        [utf8]  pub inferred_type: Option<&'a str>,
        [utf8]  pub catalog_type: Option<&'a str>,
        [utf8]  pub data_type: Option<&'a str>,
        [utf8]  pub description: Option<&'a str>,
        [utf8]  pub label: Option<&'a str>,
        [utf8]  pub expression: Option<&'a str>,
        [bool]  pub quote: Option<bool>,
        [utf8]  pub granularity: Option<&'a str>,
        [list_utf8!] pub tags: Vec<String>,
        [list_utf8!] pub classifiers: Vec<String>,
        [utf8]  pub meta: Option<String>,
        [utf8]  pub column_constraints: Option<String>,
        [utf8]  pub tests: Option<String>,
        [utf8]  pub catalog_comment: Option<&'a str>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.edges row.
    pub struct EdgeRow<'a> => schema_edges {
        [utf8!] pub parent_unique_id: &'a str,
        [utf8!] pub child_unique_id: &'a str,
        [utf8]  pub edge_type: &'a str,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.column_lineage row.
    pub struct ColumnLineageRow<'a> => schema_column_lineage {
        [utf8!] pub from_node_unique_id: &'a str,
        [utf8!] pub from_column_name: &'a str,
        [utf8!] pub to_node_unique_id: &'a str,
        [utf8!] pub to_column_name: &'a str,
        [utf8]  pub lineage_kind: Option<&'a str>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.node_input_files row.
    pub struct InputFileRow<'a> => schema_node_input_files {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub file_path: String,
        [utf8]  pub file_hash: &'a str,
        [utf8]  pub input_kind: &'a str,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.test_metadata row.
    pub struct TestMetadataRow<'a> => schema_test_metadata {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub test_name: Option<&'a str>,
        [utf8]  pub test_namespace: Option<&'a str>,
        [utf8]  pub kwargs: Option<String>,
        [utf8]  pub column_name: Option<&'a str>,
        [utf8]  pub attached_node: Option<&'a str>,
        [utf8]  pub severity: Option<String>,
        [utf8]  pub warn_if: Option<String>,
        [utf8]  pub error_if: Option<String>,
        [utf8]  pub fail_calc: Option<String>,
        [bool]  pub store_failures: Option<bool>,
        [utf8]  pub store_failures_as: Option<String>,
        [utf8]  pub test_where: Option<String>,
        [int]   pub test_limit: Option<i64>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.exposures row.
    pub struct ExposureRow<'a> => schema_exposures {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub name: &'a str,
        [utf8]  pub exposure_type: Option<String>,
        [utf8]  pub label: Option<&'a str>,
        [utf8]  pub owner_name: Option<&'a str>,
        [utf8]  pub owner_email: Option<String>,
        [utf8]  pub url: Option<&'a str>,
        [utf8]  pub maturity: Option<&'a str>,
        [utf8]  pub description: Option<&'a str>,
        [utf8]  pub package_name: &'a str,
        [utf8]  pub file_path: String,
        [utf8]  pub original_file_path: String,
        [list_utf8!] pub fqn: Vec<String>,
        [list_utf8!] pub depends_on_nodes: Vec<String>,
        [list_utf8!] pub depends_on_macros: Vec<String>,
        [utf8]  pub refs: Option<String>,
        [utf8]  pub sources: Option<String>,
        [utf8]  pub metrics: Option<String>,
        [list_utf8!] pub tags: Vec<String>,
        [utf8]  pub meta: Option<String>,
        [utf8]  pub config: Option<String>,
        // From the epoch relation's inverted `is_disabled`; NULL when the row
        // came from a source that does not carry it (manifest extraction).
        [bool]  pub enabled: Option<bool>,
        [float] pub created_at: Option<f64>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.metrics row.
    pub struct MetricRow<'a> => schema_metrics {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub name: &'a str,
        [utf8]  pub label: &'a str,
        [utf8]  pub metric_type: Option<String>,
        [utf8]  pub description: Option<&'a str>,
        [utf8]  pub package_name: &'a str,
        [utf8]  pub file_path: String,
        [utf8]  pub original_file_path: String,
        [list_utf8!] pub fqn: Vec<String>,
        [utf8]  pub type_params: Option<String>,
        [utf8]  pub metric_filter: Option<String>,
        [utf8]  pub time_granularity: Option<String>,
        [utf8]  pub semantic_model_name: Option<&'a str>,
        [list_utf8!] pub input_metric_names: Vec<String>,
        [list_utf8!] pub depends_on_nodes: Vec<String>,
        [list_utf8!] pub depends_on_macros: Vec<String>,
        [utf8]  pub refs: Option<String>,
        [utf8]  pub sources: Option<String>,
        [utf8]  pub metrics: Option<String>,
        [utf8]  pub group_name: Option<&'a str>,
        [list_utf8!] pub tags: Vec<String>,
        [utf8]  pub meta: Option<String>,
        [utf8]  pub ai_context: Option<String>,
        [utf8]  pub config: Option<String>,
        // See `ExposureRow::enabled`.
        [bool]  pub enabled: Option<bool>,
        [float] pub created_at: Option<f64>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.groups row.
    pub struct GroupRow<'a> => schema_groups {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub name: &'a str,
        [utf8]  pub description: Option<&'a str>,
        [utf8]  pub package_name: &'a str,
        [utf8]  pub file_path: String,
        [utf8]  pub original_file_path: String,
        [utf8]  pub owner_name: Option<&'a str>,
        [utf8]  pub owner_email: Option<String>,
        [utf8]  pub config: Option<&'a str>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.macros row.
    pub struct MacroRow<'a> => schema_macros {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub name: &'a str,
        [utf8]  pub package_name: &'a str,
        [utf8]  pub file_path: String,
        [utf8]  pub original_file_path: String,
        [utf8]  pub macro_sql: &'a str,
        [utf8]  pub description: &'a str,
        [list_utf8!] pub depends_on_macros: Vec<String>,
        [utf8]  pub arguments: Option<String>,
        [bool!] pub docs_show: bool,
        [utf8]  pub patch_path: Option<String>,
        [list_utf8!] pub supported_languages: Vec<String>,
        [utf8]  pub meta: Option<String>,
        [float] pub created_at: Option<f64>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.semantic_models row.
    pub struct SemanticModelRow<'a> => schema_semantic_models {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub name: Option<&'a str>,
        [utf8]  pub model: Option<&'a str>,
        [utf8]  pub label: Option<&'a str>,
        [utf8]  pub description: Option<&'a str>,
        [utf8]  pub package_name: Option<&'a str>,
        [utf8]  pub file_path: Option<&'a str>,
        [utf8]  pub original_file_path: Option<&'a str>,
        [list_utf8!] pub fqn: Vec<String>,
        [utf8]  pub node_relation: Option<String>,
        [utf8]  pub primary_entity: Option<&'a str>,
        [utf8]  pub defaults: Option<String>,
        [list_utf8!] pub depends_on_nodes: Vec<String>,
        [list_utf8!] pub depends_on_macros: Vec<String>,
        [utf8]  pub refs: Option<String>,
        [utf8]  pub group_name: Option<&'a str>,
        [utf8]  pub config: Option<String>,
        [float] pub created_at: Option<f64>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.semantic_entities row.
    pub struct SemanticEntityRow<'a> => schema_semantic_entities {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub name: Option<&'a str>,
        [utf8]  pub entity_type: Option<&'a str>,
        [utf8]  pub description: Option<&'a str>,
        [utf8]  pub label: Option<&'a str>,
        [utf8]  pub entity_role: Option<&'a str>,
        [utf8]  pub expr: Option<&'a str>,
        [utf8]  pub config: Option<String>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.semantic_measures row.
    pub struct SemanticMeasureRow<'a> => schema_semantic_measures {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub name: Option<&'a str>,
        [utf8]  pub agg: Option<&'a str>,
        [utf8]  pub description: Option<&'a str>,
        [utf8]  pub label: Option<&'a str>,
        [utf8]  pub expr: Option<&'a str>,
        [bool]  pub create_metric: Option<bool>,
        [utf8]  pub agg_time_dimension: Option<&'a str>,
        [utf8]  pub agg_params: Option<String>,
        [utf8]  pub non_additive_dimension: Option<String>,
        [utf8]  pub config: Option<String>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.semantic_dimensions row.
    pub struct SemanticDimensionRow<'a> => schema_semantic_dimensions {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub name: Option<&'a str>,
        [utf8]  pub dimension_type: Option<&'a str>,
        [utf8]  pub description: Option<&'a str>,
        [utf8]  pub label: Option<&'a str>,
        [utf8]  pub expr: Option<&'a str>,
        [bool]  pub is_partition: Option<bool>,
        [utf8]  pub time_granularity: Option<&'a str>,
        [utf8]  pub validity_params: Option<String>,
        [utf8]  pub config: Option<String>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.time_spines row.
    pub struct TimeSpineRow<'a> => schema_time_spines {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub primary_column: Option<&'a str>,
        [utf8]  pub primary_granularity: Option<&'a str>,
        [utf8]  pub custom_granularities: Option<String>,
        [utf8]  pub node_relation: Option<String>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.semantic_relationships row.
    pub struct SemanticRelationshipRow<'a> => schema_semantic_relationships {
        [utf8]  pub name: Option<&'a str>,
        [utf8!] pub from_unique_id: &'a str,
        [utf8!] pub to_unique_id: &'a str,
        [list_utf8!] pub from_columns: Vec<String>,
        [list_utf8!] pub to_columns: Vec<String>,
        [utf8]  pub cardinality: Option<&'a str>,
        [utf8]  pub relationship_type: Option<&'a str>,
        [utf8]  pub ai_context: Option<String>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.saved_queries row.
    pub struct SavedQueryRow<'a> => schema_saved_queries {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub name: &'a str,
        [utf8]  pub label: Option<&'a str>,
        [utf8]  pub description: Option<&'a str>,
        [utf8]  pub package_name: &'a str,
        [utf8]  pub file_path: String,
        [utf8]  pub original_file_path: String,
        [list_utf8!] pub fqn: Vec<String>,
        [utf8]  pub query_params: Option<String>,
        [utf8]  pub exports: Option<String>,
        [list_utf8!] pub depends_on_nodes: Vec<String>,
        [list_utf8!] pub depends_on_macros: Vec<String>,
        [utf8]  pub refs: Option<String>,
        [utf8]  pub group_name: Option<&'a str>,
        [list_utf8!] pub tags: Vec<String>,
        [utf8]  pub config: Option<String>,
        [float] pub created_at: Option<f64>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.unit_tests row.
    pub struct UnitTestRow<'a> => schema_unit_tests {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub name: &'a str,
        [utf8]  pub model: &'a str,
        [utf8]  pub description: Option<&'a str>,
        [utf8]  pub package_name: &'a str,
        [utf8]  pub file_path: String,
        [utf8]  pub original_file_path: String,
        [list_utf8!] pub fqn: Vec<String>,
        [utf8]  pub given: Option<String>,
        [utf8]  pub expect: Option<String>,
        [utf8]  pub overrides: Option<String>,
        [utf8]  pub versions: Option<String>,
        [utf8]  pub version: Option<String>,
        [utf8]  pub schema_name: &'a str,
        [list_utf8!] pub depends_on_nodes: Vec<String>,
        [list_utf8!] pub depends_on_macros: Vec<String>,
        [utf8]  pub checksum: Option<&'a str>,
        [utf8]  pub config: Option<String>,
        // See `ExposureRow::enabled`.
        [bool]  pub enabled: Option<bool>,
        [float] pub created_at: Option<f64>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.docs row.
    pub struct DocRow<'a> => schema_docs {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub name: &'a str,
        [utf8]  pub package_name: &'a str,
        [utf8]  pub file_path: String,
        [utf8]  pub original_file_path: String,
        [utf8]  pub block_contents: &'a str,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.catalog_tables row.
    pub struct CatalogTableRow<'a> => schema_catalog_tables {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub table_type: Option<&'a str>,
        [utf8]  pub database_name: Option<&'a str>,
        [utf8]  pub schema_name: Option<&'a str>,
        [utf8]  pub table_name: Option<&'a str>,
        [utf8]  pub table_owner: Option<&'a str>,
        [utf8]  pub table_comment: Option<&'a str>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.catalog_stats row.
    pub struct CatalogStatRow<'a> => schema_catalog_stats {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub stat_id: Option<&'a str>,
        [utf8]  pub stat_label: Option<&'a str>,
        [utf8]  pub stat_value: Option<String>,
        [utf8]  pub description: Option<&'a str>,
        [bool]  pub include_in_stats: Option<bool>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.column_stats row.
    pub struct ColumnStatRow<'a> => schema_column_stats {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub column_name: Option<&'a str>,
        [utf8]  pub column_type: Option<&'a str>,
        [int]   pub row_count: Option<i64>,
        [int]   pub distinct_count: Option<i64>,
        [float] pub null_pct: Option<f64>,
        [utf8]  pub min_value: Option<String>,
        [utf8]  pub max_value: Option<String>,
        [utf8]  pub avg_value: Option<String>,
        [utf8]  pub std_value: Option<String>,
        [utf8]  pub q25: Option<String>,
        [utf8]  pub q50: Option<String>,
        [utf8]  pub q75: Option<String>,
        [utf8]  pub top_values: Option<String>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.sample_data row.
    pub struct SampleDataRow<'a> => schema_sample_data {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub sample_rows: Option<String>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

// ── dbt_rt.* row types + schemas ────────────────────────────────────────────

define_row! {
    /// dbt_rt.invocations row.
    pub struct InvocationRow<'a> => schema_invocations {
        [utf8!] pub invocation_id: &'a str,
        [utf8]  pub command: &'a str,
        [utf8]  pub selector: Option<&'a str>,
        [utf8]  pub dbt_version: &'a str,
        [timestamp] pub generated_at: Option<String>,
        [float] pub elapsed_time: Option<f64>,
        [utf8]  pub args: Option<String>,
        [int]   pub node_count: Option<i64>,
        [utf8]  pub target_name: Option<&'a str>,
        [utf8]  pub target_type: Option<&'a str>,
        [utf8]  pub target_database: Option<&'a str>,
        [utf8]  pub target_schema: Option<&'a str>,
        [int]   pub target_threads: Option<i64>,
        [utf8]  pub vars_override: Option<&'a str>,
        [utf8]  pub git_sha: Option<&'a str>,
        [utf8]  pub git_branch: Option<&'a str>,
        [bool]  pub git_is_dirty: Option<bool>,
    }
}

define_row! {
    /// dbt_rt.invocation_nodes row.
    pub struct InvocationNodeRow<'a> => schema_invocation_nodes {
        [utf8!] pub invocation_id: &'a str,
        [utf8!] pub unique_id: &'a str,
    }
}

define_row! {
    /// dbt_rt.run_results row.
    pub struct RunResultRow<'a> => schema_run_results {
        [utf8!] pub unique_id: &'a str,
        [utf8!] pub invocation_id: &'a str,
        [utf8]  pub status: Option<String>,
        [float] pub execution_time: Option<f64>,
        [utf8]  pub thread_id: Option<String>,
        [utf8]  pub message: Option<String>,
        [int]   pub failures: Option<i64>,
        [bool]  pub compiled: Option<bool>,
        [utf8]  pub compiled_code_hash: Option<&'a str>,
        [utf8]  pub relation_name: Option<String>,
        [utf8]  pub adapter_response: Option<String>,
        [utf8]  pub timing: Option<String>,
        [utf8]  pub batch_results: Option<String>,
        [int]   pub rows_affected: Option<i64>,
        [timestamp] pub created_at: Option<String>,
    }
}

define_row! {
    /// dbt.source_freshness row (snapshot — merge-on-write, one row per resource).
    ///
    /// Sources and models both, since `dbt freshness` measures both; `resource_type`
    /// says which. Published as `dbt_rt.freshness`.
    pub struct SourceFreshnessRow<'a> => schema_source_freshness {
        [utf8!] pub unique_id: &'a str,
        [utf8]  pub resource_type: Option<&'a str>,
        [utf8]  pub invocation_id: Option<&'a str>,
        [utf8]  pub status: Option<&'a str>,
        [timestamp] pub max_loaded_at: Option<&'a str>,
        [timestamp] pub snapshotted_at: Option<&'a str>,
        [float] pub max_loaded_at_time_ago: Option<f64>,
        [float] pub execution_time: Option<f64>,
        [utf8]  pub thread_id: Option<&'a str>,
        [utf8]  pub error: Option<&'a str>,
        [int]   pub warn_after_count: Option<i64>,
        [utf8]  pub warn_after_period: Option<&'a str>,
        [int]   pub error_after_count: Option<i64>,
        [utf8]  pub error_after_period: Option<&'a str>,
        [utf8]  pub freshness_filter: Option<&'a str>,
        [utf8]  pub adapter_response: Option<String>,
        [utf8]  pub timing: Option<String>,
        [timestamp] pub created_at: Option<&'a str>,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt_rt.diagnostics row.
    pub struct DiagnosticRow<'a> => schema_diagnostics {
        [utf8]  pub unique_id: Option<&'a str>,
        [utf8]  pub invocation_id: Option<&'a str>,
        [utf8]  pub severity: Option<&'a str>,
        [utf8]  pub code: Option<&'a str>,
        [utf8]  pub message: Option<&'a str>,
        [utf8]  pub detail: Option<&'a str>,
        [utf8]  pub source_phase: Option<&'a str>,
        [timestamp] pub created_at: Option<&'a str>,
    }
}

define_row! {
    /// dbt_rt.adapter_queries row.
    pub struct AdapterQueryRow<'a> => schema_adapter_queries {
        [utf8!] pub unique_id: &'a str,
        [utf8!] pub invocation_id: &'a str,
        [int]   pub query_index: Option<i64>,
        [utf8]  pub query_sql: Option<&'a str>,
        [utf8]  pub query_id: Option<&'a str>,
        [int]   pub rows_affected: Option<i64>,
        [int]   pub bytes_scanned: Option<i64>,
        [float] pub query_cost: Option<f64>,
        [utf8]  pub error_message: Option<&'a str>,
        [timestamp] pub started_at: Option<&'a str>,
        [timestamp] pub completed_at: Option<&'a str>,
    }
}

define_row! {
    /// dbt_rt.test_failures row.
    pub struct TestFailureRow<'a> => schema_test_failures {
        [utf8!] pub unique_id: &'a str,
        [utf8!] pub invocation_id: &'a str,
        [utf8]  pub failure_rows: Option<String>,
    }
}

define_row! {
    /// dbt.context row — one per H2 heading in a dbt_context/ markdown file.
    pub struct ContextRow<'a> => schema_context {
        [utf8!] pub id: &'a str,
        [utf8!] pub context_type: &'a str,
        [utf8!] pub title: &'a str,
        [utf8!] pub body: &'a str,
        [utf8]  pub group_name: Option<&'a str>,
        [list_utf8!] pub tags: Vec<String>,
        [utf8!] pub package_name: &'a str,
        [utf8!] pub source_file: &'a str,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.context_terms row — synonym lookup index.
    pub struct ContextTermRow<'a> => schema_context_terms {
        [utf8!] pub term: &'a str,
        [utf8!] pub synonym: &'a str,
        [utf8!] pub context_id: &'a str,
        [utf8!] pub package_name: &'a str,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.context_links row — cross-reference from context entry to dbt node.
    pub struct ContextLinkRow<'a> => schema_context_links {
        [utf8!] pub context_id: &'a str,
        [utf8!] pub target_unique_id: &'a str,
        [utf8!] pub link_source: &'a str,
        [utf8!] pub matched_on: &'a str,
        [utf8!] pub package_name: &'a str,
        [timestamp !] pub ingested_at: &'a str,
    }
}

define_row! {
    /// dbt.generation row — single-row cold-start timestamp.
    pub struct GenerationRow<'a> => schema_generation {
        [timestamp !] pub ingested_at: &'a str,
    }
}

/// Get schema for a given table name.
pub fn schema_for(table: &str) -> SchemaRef {
    match table {
        "generation" => schema_generation(),
        "project" => schema_project(),
        "packages" => schema_packages(),
        "project_vars" => schema_project_vars(),
        "project_env_vars" => schema_project_env_vars(),
        "nodes" => schema_nodes(),
        "node_columns" => schema_node_columns(),
        "edges" => schema_edges(),
        "column_lineage" => schema_column_lineage(),
        "node_input_files" => schema_node_input_files(),
        "test_metadata" => schema_test_metadata(),
        "exposures" => schema_exposures(),
        "metrics" => schema_metrics(),
        "groups" => schema_groups(),
        "macros" => schema_macros(),
        "semantic_models" => schema_semantic_models(),
        "semantic_entities" => schema_semantic_entities(),
        "semantic_measures" => schema_semantic_measures(),
        "semantic_dimensions" => schema_semantic_dimensions(),
        "time_spines" => schema_time_spines(),
        "semantic_relationships" => schema_semantic_relationships(),
        "saved_queries" => schema_saved_queries(),
        "unit_tests" => schema_unit_tests(),
        "docs" => schema_docs(),
        "catalog_tables" => schema_catalog_tables(),
        "catalog_stats" => schema_catalog_stats(),
        "column_stats" => schema_column_stats(),
        "sample_data" => schema_sample_data(),
        "invocations" => schema_invocations(),
        "invocation_nodes" => schema_invocation_nodes(),
        "run_results" => schema_run_results(),
        "source_freshness" => schema_source_freshness(),
        "diagnostics" => schema_diagnostics(),
        "adapter_queries" => schema_adapter_queries(),
        "test_failures" => schema_test_failures(),
        "context" => schema_context(),
        "context_terms" => schema_context_terms(),
        "context_links" => schema_context_links(),
        // Delta variants — same schema as base table, written to separate files.
        "nodes_delta" => schema_nodes(),
        "edges_delta" => schema_edges(),
        "test_metadata_delta" => schema_test_metadata(),
        "metrics_delta" => schema_metrics(),
        "semantic_models_delta" => schema_semantic_models(),
        "semantic_entities_delta" => schema_semantic_entities(),
        "semantic_measures_delta" => schema_semantic_measures(),
        "semantic_dimensions_delta" => schema_semantic_dimensions(),
        "saved_queries_delta" => schema_saved_queries(),
        "exposures_delta" => schema_exposures(),
        "groups_delta" => schema_groups(),
        "macros_delta" => schema_macros(),
        "docs_delta" => schema_docs(),
        "unit_tests_delta" => schema_unit_tests(),
        "time_spines_delta" => schema_time_spines(),
        "alive_ids" => Arc::new(Schema::new(vec![Field::new(
            "unique_id",
            DataType::Utf8,
            false,
        )])),
        _ => Arc::new(Schema::empty()),
    }
}

#[cfg(test)]
mod staged_table_tests {
    use super::*;

    fn schema() -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new("id", DataType::Utf8, false)]))
    }

    fn rows() -> Vec<Value> {
        vec![serde_json::json!({"id": "a"})]
    }

    #[test]
    fn dropping_a_staged_table_removes_its_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dbt.t.parquet");

        let tmp = {
            let staged = stage_table(&dest, schema(), &rows()).unwrap();
            let tmp = staged.tmp.clone();
            assert!(tmp.exists(), "staging must write a temp file");
            tmp
        };

        assert!(
            !tmp.exists(),
            "an uncommitted stage must delete its temp file"
        );
        assert!(
            !dest.exists(),
            "an uncommitted stage must not touch the destination"
        );
    }

    #[test]
    fn committing_moves_the_staged_file_onto_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("dbt.t.parquet");

        let staged = stage_table(&dest, schema(), &rows()).unwrap();
        let tmp = staged.tmp.clone();
        staged.commit().unwrap();

        assert!(dest.exists());
        assert!(!tmp.exists(), "commit must consume the temp file");
    }

    /// `commit_staged` renames in sequence, so a failure partway through leaves
    /// the tables it already renamed in place. The remaining stages still clean
    /// up their temp files.
    #[test]
    fn a_failed_commit_keeps_earlier_renames_and_drops_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("dbt.first.parquet");
        let blocked = dir.path().join("dbt.blocked.parquet");
        let last = dir.path().join("dbt.last.parquet");

        // A directory cannot be replaced by a rename, so `blocked` fails
        std::fs::create_dir(&blocked).unwrap();

        let staged = vec![
            stage_table(&first, schema(), &rows()).unwrap(),
            stage_table(&blocked, schema(), &rows()).unwrap(),
            stage_table(&last, schema(), &rows()).unwrap(),
        ];
        let last_tmp = staged[2].tmp.clone();

        commit_staged(staged).expect_err("renaming onto a directory must fail");

        assert!(first.is_file(), "a rename before the failure stays applied");
        assert!(
            blocked.is_dir(),
            "the failing rename leaves the destination alone"
        );
        assert!(!last.exists(), "a stage after the failure is never applied");
        assert!(
            !last_tmp.exists(),
            "a stage after the failure cleans up its temp file"
        );
    }
}
