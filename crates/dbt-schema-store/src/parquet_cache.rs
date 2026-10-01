//! Parquet-backed schema cache.
//!
//! Replaces the per-entry parquet files under `target/schemas/` with
//! epoch-append parquet files:
//!
//! ```text
//! target/
//!   private/metadata/compile/schemas/{N}.parquet    ← compile-time schemas (no TTL)
//!   private/metadata/warehouse/schemas/{N}.parquet  ← warehouse-fetched schemas (TTL)
//! ```
//!
//! Design:
//!
//! * **Load at startup** — all epochs are read, latest-epoch-wins by `lookup_key`.
//!   No per-entry I/O during compilation.
//! * **Save at end-of-run** — all in-memory entries are written as a new epoch
//!   file via [`ParquetSchemaCache::save`].
//! * **Build-hash guard** — each row stores `CARGO_PKG_VERSION`; mismatched rows
//!   are silently dropped on load (cold start for affected entries).
//! * **TTL eviction** — each row carries `cached_at_ms`; entries older than their
//!   configured interval are dropped during load.
//! * **Compaction** — when the epoch count exceeds [`COMPACT_THRESHOLD`], all
//!   epochs are merged into `0.parquet` and deltas are deleted.
//! * **No lock file** — save() writes a new epoch file atomically (new file);
//!   it never modifies existing files, so concurrent writers are safe.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use arrow::ipc::{
    reader::StreamReader as ArrowIpcStreamReader, writer::StreamWriter as ArrowIpcStreamWriter,
};
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
use parquet::arrow::ArrowWriter;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{ArrowPredicateFn, ParquetRecordBatchReaderBuilder, RowFilter};
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;
use serde::{Deserialize, Serialize};

use crate::{SchemaEntry, SchemaStoreResult};

// ── constants ─────────────────────────────────────────────────────────────────

pub(crate) const COMPACT_THRESHOLD: usize = 8;

// Keep in sync with dbt_frontend_common::error::DBT_CACHED_PARQUET_PATH_KEY
const DBT_CACHED_PARQUET_PATH_KEY: &str = "DBT:cached_parquet_path";

// ── on-disk row ───────────────────────────────────────────────────────────────

/// One row in a schema cache epoch file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SchemaRow {
    /// The `Display` output of [`crate::store::LookupEntry`] — partition key.
    pub lookup_key: String,
    /// Arrow IPC bytes for the canonical (SDF) schema.
    pub sdf_schema: Vec<u8>,
    /// Arrow IPC bytes for the original warehouse schema, if present.
    pub original_schema: Option<Vec<u8>>,
    /// Unix milliseconds at write time (for TTL eviction).
    pub cached_at_ms: i64,
    /// `CARGO_PKG_VERSION` at write time (for build-hash guard).
    pub build_hash: String,
    /// Epoch number written into each row for provenance.
    pub epoch: i32,
}

fn schema_row_fields() -> Vec<Field> {
    vec![
        Field::new("lookup_key", DataType::Utf8, false),
        Field::new("sdf_schema", DataType::Binary, false),
        Field::new("original_schema", DataType::Binary, true),
        Field::new("cached_at_ms", DataType::Int64, false),
        Field::new("build_hash", DataType::Utf8, false),
        Field::new("epoch", DataType::Int32, false),
    ]
}

// ── in-memory row ─────────────────────────────────────────────────────────────

/// One entry held in memory after load.
///
/// Schemas are kept as raw IPC bytes and deserialized lazily on first access via
/// [`OnceLock`], so unaccessed remote schemas do not inflate heap usage.
#[derive(Debug)]
pub(crate) struct CacheRow {
    /// Raw Arrow IPC bytes for the SDF (canonical) schema.
    pub sdf_bytes: Vec<u8>,
    /// Raw Arrow IPC bytes for the original warehouse schema, if any.
    pub original_bytes: Option<Vec<u8>>,
    pub cached_at_ms: u64,
    /// The lookup key for this entry (e.g. `Frontier(db.schema.table)`).
    /// Injected into schema metadata on deserialization so binder errors can
    /// show the source identity rather than a raw filesystem path.
    pub lookup_key: String,
    /// Cache directory injected into schema metadata on first deserialization.
    pub cache_dir: PathBuf,
    /// Lazily-deserialized [`SchemaEntry`]; `None` inner value means corrupt bytes.
    deserialized: std::sync::OnceLock<Option<SchemaEntry>>,
    /// True when this entry was written by [`ParquetSchemaCache::upsert`] in the
    /// current run and has not yet been persisted to a delta epoch file.
    /// False for entries loaded from existing epoch files (already on disk).
    pub dirty: bool,
}

impl Clone for CacheRow {
    fn clone(&self) -> Self {
        let deserialized = std::sync::OnceLock::new();
        if let Some(v) = self.deserialized.get() {
            let _ = deserialized.set(v.clone());
        }
        Self {
            sdf_bytes: self.sdf_bytes.clone(),
            original_bytes: self.original_bytes.clone(),
            cached_at_ms: self.cached_at_ms,
            lookup_key: self.lookup_key.clone(),
            cache_dir: self.cache_dir.clone(),
            deserialized,
            dirty: self.dirty,
        }
    }
}

impl CacheRow {
    fn new(
        sdf_bytes: Vec<u8>,
        original_bytes: Option<Vec<u8>>,
        cached_at_ms: u64,
        lookup_key: String,
        cache_dir: PathBuf,
        dirty: bool,
    ) -> Self {
        Self {
            sdf_bytes,
            original_bytes,
            cached_at_ms,
            lookup_key,
            cache_dir,
            deserialized: std::sync::OnceLock::new(),
            dirty,
        }
    }

    /// Returns the deserialized [`SchemaEntry`], deserializing on first call.
    /// Returns `None` if the bytes are corrupt (logs a warning).
    pub fn entry(&self) -> Option<&SchemaEntry> {
        self.deserialized
            .get_or_init(|| {
                let sdf = match deserialize_schema(&self.sdf_bytes) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(
                            lookup_key = %self.lookup_key,
                            "schema cache: corrupt IPC bytes, skipping entry: {e}"
                        );
                        return None;
                    }
                };
                let sdf = inject_cache_path_metadata(sdf, &self.lookup_key);
                let original = self
                    .original_bytes
                    .as_deref()
                    .and_then(|b| deserialize_schema(b).ok());
                Some(SchemaEntry::from_sdf_arrow_schema(original, sdf))
            })
            .as_ref()
    }
}

// ── Arrow IPC helpers ─────────────────────────────────────────────────────────

pub(crate) fn serialize_schema(schema: &SchemaRef) -> SchemaStoreResult<Vec<u8>> {
    let mut buf = Vec::<u8>::new();
    ArrowIpcStreamWriter::try_new(&mut buf, schema.as_ref()).and_then(|mut w| w.finish())?;
    Ok(buf)
}

pub(crate) fn deserialize_schema(bytes: &[u8]) -> SchemaStoreResult<SchemaRef> {
    ArrowIpcStreamReader::try_new(bytes, None).map(|r| r.schema())
}

// ── epoch helpers ─────────────────────────────────────────────────────────────

/// Lists all epoch files in `dir`, sorted ascending by epoch number.
/// Epoch files are named `{N}.parquet`.
fn existing_epochs(dir: &Path) -> Vec<(u32, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut epochs: Vec<(u32, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let stem = p.file_stem()?.to_str()?;
            let n: u32 = stem.parse().ok()?;
            if p.extension()?.to_str()? == "parquet" {
                Some((n, p))
            } else {
                None
            }
        })
        .collect();
    epochs.sort_by_key(|(n, _)| *n);
    epochs
}

fn next_epoch(dir: &Path) -> u32 {
    existing_epochs(dir).last().map(|(n, _)| n + 1).unwrap_or(0)
}

// ── parquet write / read helpers ──────────────────────────────────────────────

fn write_rows(path: &Path, rows: &[SchemaRow]) -> SchemaStoreResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ArrowError::IoError(format!("create_dir_all {}", parent.display()), e))?;
    }
    let file = std::fs::File::create(path)
        .map_err(|e| ArrowError::IoError(format!("create {}", path.display()), e))?;
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(1).unwrap()))
        .build();
    let fields = schema_row_fields();
    let arrow_schema = Arc::new(Schema::new(fields));
    let field_refs: Vec<_> = arrow_schema.fields().iter().map(Arc::clone).collect();
    let mut writer = ArrowWriter::try_new(file, arrow_schema, Some(props))
        .map_err(|e| ArrowError::ParquetError(format!("ArrowWriter: {e}")))?;
    for chunk in rows.chunks(256) {
        let chunk_vec: Vec<&SchemaRow> = chunk.iter().collect();
        let batch = serde_arrow::to_record_batch(&field_refs, &chunk_vec)
            .map_err(|e| ArrowError::ExternalError(Box::new(e)))?;
        writer
            .write(&batch)
            .map_err(|e| ArrowError::ParquetError(format!("write: {e}")))?;
    }
    writer
        .close()
        .map_err(|e| ArrowError::ParquetError(format!("close: {e}")))?;
    Ok(())
}

/// Reads rows from a parquet epoch file, optionally filtering to a known key set.
///
/// When `key_filter` is `Some`, a parquet `RowFilter` is used so that only the
/// `lookup_key` column is decoded for evaluation; the heavy `sdf_schema` /
/// `original_schema` binary columns are decoded **only for matching rows**.
/// This avoids decompressing the IPC blobs for entries not needed in this run.
///
/// When `key_filter` is `None` (e.g. compaction), all rows are read.
fn read_rows(path: &Path, key_filter: Option<&HashSet<&str>>) -> Vec<SchemaRow> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(builder) = ParquetRecordBatchReaderBuilder::try_new(file) else {
        tracing::warn!(
            path = %path.display(),
            "schema cache: corrupt parquet file, skipping"
        );
        return Vec::new();
    };

    let reader = if let Some(keys) = key_filter {
        // Column 0 is `lookup_key` (Utf8). Read only that column for the predicate;
        // the parquet reader will then decode the remaining columns only for matching rows.
        let parquet_schema = builder.parquet_schema();
        let key_col = ProjectionMask::leaves(parquet_schema, [0]);
        let keys: Arc<HashSet<String>> =
            Arc::new(keys.iter().copied().map(|s| s.to_string()).collect());
        let predicate = ArrowPredicateFn::new(key_col, move |batch| {
            use arrow::array::StringArray;
            let col = batch
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("lookup_key must be Utf8");
            Ok(arrow::array::BooleanArray::from(
                col.iter()
                    .map(|v| v.map(|s| keys.contains(s)))
                    .collect::<Vec<_>>(),
            ))
        });
        match builder
            .with_row_filter(RowFilter::new(vec![Box::new(predicate)]))
            .build()
        {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        }
    } else {
        match builder.build() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        }
    };

    let mut rows = Vec::new();
    for batch in reader.flatten() {
        if let Ok(mut chunk) = serde_arrow::from_record_batch::<Vec<SchemaRow>>(&batch) {
            rows.append(&mut chunk);
        }
    }
    rows
}

// ── compaction ────────────────────────────────────────────────────────────────

/// Context passed to [`compact_epochs`] to enable filtering during compaction.
///
/// Currently only carries TTL information. In the future this should also carry
/// the active frontier (set of known `lookup_key` values) so that entries that
/// have fallen off the frontier (deleted nodes/sources) are pruned during
/// compaction rather than accumulating indefinitely on disk.
///
/// TODO: extend with `active_keys: Option<&HashSet<String>>` once the caller
/// (SchemaStore::save) has access to the full frontier key set. Pass `None`
/// today via `CompactionContext::default()`.
#[derive(Default)]
pub(crate) struct CompactionContext<'a> {
    /// TTL intervals by lookup_key — same map used during load. Entries older
    /// than their interval are dropped during compaction so the compacted epoch
    /// 0 file stays clean after a build upgrade or source removal.
    pub ttl_map: HashMap<&'a str, Option<Duration>>,
}

/// Merges all epoch files in `dir` into `dir/0.parquet` (latest-wins by
/// `lookup_key`), applying the build-hash guard and TTL filter from `ctx`.
///
/// Writes atomically via a `.tmp` file. Delta files are deleted after a
/// successful rename.
pub(crate) fn compact_epochs(
    dir: &Path,
    epochs: &[(u32, PathBuf)],
    ctx: &CompactionContext<'_>,
) -> SchemaStoreResult<()> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64;

    let mut merged: HashMap<String, SchemaRow> = HashMap::new();
    for (_n, path) in epochs {
        for row in read_rows(path, None) {
            merged.insert(row.lookup_key.clone(), row);
        }
    }

    let compacted: Vec<SchemaRow> = merged
        .into_values()
        .filter(|row| {
            // Drop rows written by a different build — they are cold-cache on
            // next load anyway, no point keeping them on disk.
            if row.build_hash != env!("CARGO_PKG_VERSION") {
                return false;
            }
            // Apply TTL: drop rows whose age exceeds their configured interval.
            if let Some(interval) = ctx.ttl_map.get(row.lookup_key.as_str()).copied().flatten() {
                let age_ms = now_ms.saturating_sub(row.cached_at_ms as u64);
                if Duration::from_millis(age_ms) > interval {
                    return false;
                }
            }
            true
        })
        .map(|mut r| {
            r.epoch = 0;
            r
        })
        .collect();

    let tmp = dir.join("0.parquet.tmp");
    write_rows(&tmp, &compacted)?;
    let target = dir.join("0.parquet");
    // On Windows, rename fails if the target exists. Remove it first.
    if cfg!(windows) {
        let _ = std::fs::remove_file(&target);
    }
    std::fs::rename(&tmp, &target)
        .map_err(|e| ArrowError::IoError("compaction rename".to_string(), e))?;
    for (n, path) in epochs {
        if *n != 0 {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

// ── ParquetSchemaCache ────────────────────────────────────────────────────────

/// Parquet-backed schema cache.
///
/// Loaded once at startup from epoch files under `cache_dir`, held in memory
/// during compilation, and flushed back to disk via [`save`] at end of run.
#[derive(Debug)]
pub struct ParquetSchemaCache {
    entries: HashMap<String, CacheRow>,
    cache_dir: PathBuf,
    /// TTL intervals by lookup_key, used during compaction to prune stale entries.
    ttl_intervals: HashMap<String, Option<Duration>>,
    /// True when there are dirty entries that need flushing. Reset after a
    /// successful save so that only genuinely new entries are written next time.
    has_dirty: AtomicBool,
}

impl ParquetSchemaCache {
    /// Loads the cache from all epoch files under `cache_dir`.
    ///
    /// Applies TTL and build-hash filtering. Missing or corrupt files degrade
    /// gracefully to an empty cache for the affected entries.
    ///
    /// `load_all_rows` controls whether a parquet row filter is applied:
    /// - `false` (analyzed cache): only rows whose `lookup_key` is in
    ///   `entries_with_intervals` are decoded. This is safe because the full set
    ///   of analyzed keys (Selected/snapshot unique_ids) is known at startup.
    /// - `true` (remote cache): all rows are loaded regardless of key. Required
    ///   because External entries are discovered lazily and are not present in
    ///   `entries_with_intervals` at startup; loading only known keys would drop
    ///   External schemas fetched in a previous run.
    pub fn load(
        cache_dir: &Path,
        entries_with_intervals: &[(String, Option<Duration>)],
        load_all_rows: bool,
    ) -> Self {
        let entries = Self::try_load(cache_dir, entries_with_intervals, load_all_rows);
        let ttl_intervals: HashMap<String, Option<Duration>> = entries_with_intervals
            .iter()
            .map(|(k, d)| (k.clone(), *d))
            .collect();
        Self {
            entries,
            cache_dir: cache_dir.to_path_buf(),
            ttl_intervals,
            has_dirty: AtomicBool::new(false),
        }
    }

    fn try_load(
        cache_dir: &Path,
        entries_with_intervals: &[(String, Option<Duration>)],
        load_all_rows: bool,
    ) -> HashMap<String, CacheRow> {
        let epochs = existing_epochs(cache_dir);
        if epochs.is_empty() {
            return HashMap::new();
        }

        let interval_map: HashMap<&str, Option<Duration>> = entries_with_intervals
            .iter()
            .map(|(k, d)| (k.as_str(), *d))
            .collect();

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;

        // Read all epochs in order; later epochs overwrite earlier for the same key.
        // For the analyzed cache (load_all_rows=false), apply a parquet RowFilter so
        // the heavy sdf_schema/original_schema blobs are decoded only for matching
        // rows — gives 18× speedup when only a small frontier subset is needed.
        // For the remote cache (load_all_rows=true), skip the filter so External
        // entries from prior runs are not silently dropped.
        let key_set: HashSet<&str> = interval_map.keys().copied().collect();
        let key_filter = if load_all_rows || key_set.is_empty() {
            None
        } else {
            Some(&key_set)
        };
        let mut raw: HashMap<String, SchemaRow> = HashMap::new();
        for (_epoch_n, path) in &epochs {
            for row in read_rows(path, key_filter) {
                raw.insert(row.lookup_key.clone(), row);
            }
        }

        let mut result = HashMap::new();
        for (key, row) in raw {
            // Build-hash guard
            if row.build_hash != env!("CARGO_PKG_VERSION") {
                continue;
            }
            // TTL check
            if let Some(interval) = interval_map.get(key.as_str()).copied().flatten() {
                let age_ms = now_ms.saturating_sub(row.cached_at_ms as u64);
                if Duration::from_millis(age_ms) > interval {
                    continue;
                }
            }
            result.insert(
                key.clone(),
                CacheRow::new(
                    row.sdf_schema,
                    row.original_schema,
                    row.cached_at_ms as u64,
                    key,
                    cache_dir.to_path_buf(),
                    false, // loaded from disk — already persisted
                ),
            );
        }
        result
    }

    /// Returns the cached [`SchemaEntry`] for `lookup_key`, if present.
    ///
    /// Deserializes the IPC bytes on first call; subsequent calls return the
    /// cached [`SchemaEntry`] directly.
    pub fn get(&self, lookup_key: &str) -> Option<&SchemaEntry> {
        self.entries.get(lookup_key).and_then(|r| r.entry())
    }

    /// Returns `true` if `lookup_key` has a cached entry.
    pub fn contains(&self, lookup_key: &str) -> bool {
        self.entries.contains_key(lookup_key)
    }

    /// Inserts or replaces an entry in the in-memory map.
    ///
    /// The IPC bytes are serialized eagerly (needed for `save()`); the
    /// deserialized [`SchemaEntry`] is stored in the `OnceLock` immediately so
    /// subsequent `get()` calls don't re-deserialize.
    pub fn upsert(&mut self, lookup_key: String, entry: SchemaEntry) -> SchemaStoreResult<()> {
        let cached_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis() as u64;
        let sdf_bytes = serialize_schema(entry.inner())?;
        let original_bytes = entry.original().map(serialize_schema).transpose()?;
        let row = CacheRow::new(
            sdf_bytes,
            original_bytes,
            cached_at_ms,
            lookup_key.clone(),
            self.cache_dir.clone(),
            true, // newly written this run — must be persisted
        );
        // Pre-populate the OnceLock so get() is free for this freshly-computed entry.
        let _ = row.deserialized.set(Some(entry));
        self.entries.insert(lookup_key, row);
        self.has_dirty.store(true, Ordering::Release);
        Ok(())
    }

    /// Removes an entry from the in-memory map.
    pub fn remove(&mut self, lookup_key: &str) {
        self.entries.remove(lookup_key);
    }

    /// Writes all in-memory entries as a new epoch parquet file under `cache_dir`.
    ///
    /// Compacts to `0.parquet` when the epoch count exceeds [`COMPACT_THRESHOLD`].
    pub fn save(&mut self) -> SchemaStoreResult<()> {
        let dir = self.cache_dir.clone();
        self.save_to(&dir)
    }

    /// Writes dirty entries as a new epoch parquet file under `dir`.
    ///
    /// Only dirty entries (written by [`upsert`] since the last save) are
    /// written — clean entries are already on disk.
    /// If nothing is dirty, no file is written and no epoch number is consumed.
    /// Compacts to `0.parquet` when the epoch count exceeds [`COMPACT_THRESHOLD`].
    pub fn save_to(&mut self, dir: &Path) -> SchemaStoreResult<()> {
        if !self.has_dirty.swap(false, Ordering::AcqRel) {
            return Ok(()); // nothing new since last save
        }

        let epoch_n = next_epoch(dir);
        let rows = self.build_dirty_rows(epoch_n as i32)?;
        if rows.is_empty() {
            return Ok(()); // nothing new to persist
        }
        std::fs::create_dir_all(dir)
            .map_err(|e| ArrowError::IoError(format!("create_dir_all {}", dir.display()), e))?;
        let path = dir.join(format!("{epoch_n}.parquet"));
        write_rows(&path, &rows)?;

        // Mark all entries clean so they aren't re-written on the next save.
        for row in self.entries.values_mut() {
            row.dirty = false;
        }

        let epochs = existing_epochs(dir);
        if epochs.len() > COMPACT_THRESHOLD {
            let ttl_map: HashMap<&str, Option<Duration>> = self
                .ttl_intervals
                .iter()
                .map(|(k, d)| (k.as_str(), *d))
                .collect();
            compact_epochs(dir, &epochs, &CompactionContext { ttl_map })?;
        }
        Ok(())
    }

    /// Builds rows for dirty (newly upserted) entries only.
    fn build_dirty_rows(&self, epoch: i32) -> SchemaStoreResult<Vec<SchemaRow>> {
        let mut rows = Vec::new();
        for (key, cache_row) in &self.entries {
            if !cache_row.dirty {
                continue;
            }
            rows.push(SchemaRow {
                lookup_key: key.clone(),
                sdf_schema: cache_row.sdf_bytes.clone(),
                original_schema: cache_row.original_bytes.clone(),
                cached_at_ms: cache_row.cached_at_ms as i64,
                build_hash: env!("CARGO_PKG_VERSION").to_string(),
                epoch,
            });
        }
        Ok(rows)
    }

    /// Returns an iterator over all lookup keys in the cache.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|s| s.as_str())
    }

    /// Number of in-memory entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if there are no in-memory entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Creates an empty cache rooted at `cache_dir` (used in tests).
    #[cfg(test)]
    fn empty(cache_dir: &Path) -> Self {
        Self {
            entries: HashMap::new(),
            cache_dir: cache_dir.to_path_buf(),
            ttl_intervals: HashMap::new(),
            has_dirty: AtomicBool::new(false),
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn inject_cache_path_metadata(schema: SchemaRef, lookup_key: &str) -> SchemaRef {
    let mut metadata = schema.metadata().clone();
    let identity = extract_source_identity_from_key(lookup_key);
    metadata.insert(DBT_CACHED_PARQUET_PATH_KEY.to_string(), identity);
    Arc::new(Schema::new_with_metadata(schema.fields().clone(), metadata))
}

/// Extracts the logical source identity from a lookup key.
/// `Frontier(CATALOG.SCHEMA.TABLE)` → `CATALOG.SCHEMA.TABLE`
/// `Selected(model.project.name)` → `model.project.name`
fn extract_source_identity_from_key(lookup_key: &str) -> String {
    if let Some(start) = lookup_key.find('(') {
        if let Some(end) = lookup_key.rfind(')') {
            return lookup_key[start + 1..end].to_string();
        }
    }
    lookup_key.to_string()
}

// ── unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_schema::{DataType, Field, Schema};

    fn make_schema(name: &str) -> SchemaRef {
        Arc::new(Schema::new(vec![Field::new(name, DataType::Utf8, false)]))
    }

    #[test]
    fn round_trip_schema_ipc() {
        let s = make_schema("amount");
        let bytes = serialize_schema(&s).unwrap();
        let back = deserialize_schema(&bytes).unwrap();
        assert_eq!(back.field(0).name(), "amount");
    }

    #[test]
    fn save_and_load_basic() {
        let dir = tempfile::tempdir().unwrap();
        let key = "Selected(model.pkg.orders)";

        let mut cache = ParquetSchemaCache::empty(dir.path());
        cache
            .upsert(
                key.to_string(),
                SchemaEntry::from_sdf_arrow_schema(None, make_schema("col")),
            )
            .unwrap();
        cache.save().unwrap();

        let loaded = ParquetSchemaCache::load(dir.path(), &[(key.to_string(), None)], false);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.get(key).unwrap().inner().field(0).name(), "col");
    }

    #[test]
    fn save_and_load_with_original() {
        let dir = tempfile::tempdir().unwrap();
        let key = "Frontier(db.s.t)";

        let mut cache = ParquetSchemaCache::empty(dir.path());
        cache
            .upsert(
                key.to_string(),
                SchemaEntry::from_sdf_arrow_schema(Some(make_schema("orig")), make_schema("sdf")),
            )
            .unwrap();
        cache.save().unwrap();

        let loaded = ParquetSchemaCache::load(dir.path(), &[(key.to_string(), None)], false);
        let entry = loaded.get(key).unwrap();
        assert_eq!(entry.inner().field(0).name(), "sdf");
        assert_eq!(entry.original().unwrap().field(0).name(), "orig");
    }

    #[test]
    fn ttl_evicts_stale() {
        let dir = tempfile::tempdir().unwrap();
        let key = "Frontier(db.s.src)";

        let mut cache = ParquetSchemaCache::empty(dir.path());
        cache
            .upsert(
                key.to_string(),
                SchemaEntry::from_sdf_arrow_schema(None, make_schema("col")),
            )
            .unwrap();
        cache.save().unwrap();

        std::thread::sleep(Duration::from_millis(5));
        let loaded = ParquetSchemaCache::load(
            dir.path(),
            &[(key.to_string(), Some(Duration::from_millis(1)))],
            false,
        );
        assert_eq!(loaded.len(), 0, "stale entry must be evicted");
    }

    #[test]
    fn ttl_keeps_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let key = "Frontier(db.s.src)";

        let mut cache = ParquetSchemaCache::empty(dir.path());
        cache
            .upsert(
                key.to_string(),
                SchemaEntry::from_sdf_arrow_schema(None, make_schema("col")),
            )
            .unwrap();
        cache.save().unwrap();

        let loaded = ParquetSchemaCache::load(
            dir.path(),
            &[(key.to_string(), Some(Duration::from_secs(3600)))],
            false,
        );
        assert_eq!(loaded.len(), 1, "fresh entry must survive");
    }

    #[test]
    fn missing_dir_gives_empty_cache() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = ParquetSchemaCache::load(&dir.path().join("nonexistent"), &[], false);
        assert_eq!(loaded.len(), 0);
    }

    #[test]
    fn build_hash_mismatch_gives_empty_cache() {
        let dir = tempfile::tempdir().unwrap();
        let key = "Selected(model.pkg.orders)";

        // Write a row with a wrong build hash directly.
        let path = dir.path().join("0.parquet");
        let rows = vec![SchemaRow {
            lookup_key: key.to_string(),
            sdf_schema: serialize_schema(&make_schema("col")).unwrap(),
            original_schema: None,
            cached_at_ms: 0,
            build_hash: "old-0.0.0".to_string(),
            epoch: 0,
        }];
        write_rows(&path, &rows).unwrap();

        let loaded = ParquetSchemaCache::load(dir.path(), &[(key.to_string(), None)], false);
        assert_eq!(loaded.len(), 0, "build hash mismatch must discard entries");
    }

    #[test]
    fn cache_path_metadata_injected_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let key = "Selected(model.pkg.t)";

        let mut cache = ParquetSchemaCache::empty(dir.path());
        cache
            .upsert(
                key.to_string(),
                SchemaEntry::from_sdf_arrow_schema(None, make_schema("col")),
            )
            .unwrap();
        cache.save().unwrap();

        let loaded = ParquetSchemaCache::load(dir.path(), &[(key.to_string(), None)], false);
        assert!(
            loaded
                .get(key)
                .unwrap()
                .inner()
                .metadata()
                .contains_key(DBT_CACHED_PARQUET_PATH_KEY),
            "must inject cache path metadata on load"
        );
    }

    #[test]
    fn epoch_append_latest_wins() {
        let dir = tempfile::tempdir().unwrap();
        let key = "Frontier(db.s.t)";

        let mut cache0 = ParquetSchemaCache::empty(dir.path());
        cache0
            .upsert(
                key.to_string(),
                SchemaEntry::from_sdf_arrow_schema(None, make_schema("old")),
            )
            .unwrap();
        cache0.save().unwrap();

        let mut cache1 = ParquetSchemaCache::empty(dir.path());
        cache1
            .upsert(
                key.to_string(),
                SchemaEntry::from_sdf_arrow_schema(None, make_schema("new")),
            )
            .unwrap();
        cache1.save().unwrap();

        let loaded = ParquetSchemaCache::load(dir.path(), &[(key.to_string(), None)], false);
        assert_eq!(
            loaded.get(key).unwrap().inner().field(0).name(),
            "new",
            "later epoch must win"
        );
    }

    #[test]
    fn compaction_merges_epochs() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..=(COMPACT_THRESHOLD as u32) {
            let key = format!("Frontier(db.s.t{i})");
            let mut cache = ParquetSchemaCache::empty(dir.path());
            cache
                .upsert(
                    key,
                    SchemaEntry::from_sdf_arrow_schema(None, make_schema("col")),
                )
                .unwrap();
            cache.save().unwrap();
        }
        let epochs = existing_epochs(dir.path());
        assert_eq!(epochs.len(), 1, "compaction must leave exactly one file");
        assert_eq!(epochs[0].0, 0, "compacted file must be epoch 0");
    }

    #[test]
    fn compaction_applies_ttl_context() {
        let dir = tempfile::tempdir().unwrap();
        let fresh_key = "Frontier(db.s.fresh)";
        let stale_key = "Frontier(db.s.stale)";

        // Write both entries as epoch 0.
        let mut cache = ParquetSchemaCache::empty(dir.path());
        cache
            .upsert(
                fresh_key.to_string(),
                SchemaEntry::from_sdf_arrow_schema(None, make_schema("f")),
            )
            .unwrap();
        cache
            .upsert(
                stale_key.to_string(),
                SchemaEntry::from_sdf_arrow_schema(None, make_schema("s")),
            )
            .unwrap();
        cache.save().unwrap();

        // Wait so the stale entry ages past TTL.
        std::thread::sleep(Duration::from_millis(5));

        // Write a second epoch to trigger compaction threshold check.
        // We'll compact manually with a TTL context.
        let epochs = existing_epochs(dir.path());
        let mut ttl_map = HashMap::new();
        ttl_map.insert(stale_key, Some(Duration::from_millis(1))); // stale: expired
        ttl_map.insert(fresh_key, Some(Duration::from_secs(3600))); // fresh: not expired
        let ctx = CompactionContext { ttl_map };

        compact_epochs(dir.path(), &epochs, &ctx).unwrap();

        // Reload: fresh must survive, stale must be pruned.
        let loaded = ParquetSchemaCache::load(
            dir.path(),
            &[
                (fresh_key.to_string(), Some(Duration::from_secs(3600))),
                (stale_key.to_string(), Some(Duration::from_secs(3600))), // generous TTL on load
            ],
            false,
        );
        assert!(
            loaded.get(fresh_key).is_some(),
            "fresh entry must survive compaction"
        );
        assert!(
            loaded.get(stale_key).is_none(),
            "stale entry must be pruned during compaction"
        );
    }

    #[test]
    fn compaction_succeeds_when_zero_parquet_exists() {
        // Simulates the Windows rename concern: 0.parquet already exists from a
        // previous compaction, and we compact again.
        let dir = tempfile::tempdir().unwrap();
        let key = "Frontier(db.s.t)";

        // Create initial epoch 0.
        let mut cache = ParquetSchemaCache::empty(dir.path());
        cache
            .upsert(
                key.to_string(),
                SchemaEntry::from_sdf_arrow_schema(None, make_schema("col")),
            )
            .unwrap();
        cache.save().unwrap();

        // Manually trigger compaction (even though threshold isn't met).
        let epochs = existing_epochs(dir.path());
        compact_epochs(dir.path(), &epochs, &CompactionContext::default()).unwrap();

        // Now write more epochs to force a second compaction.
        for i in 1..=(COMPACT_THRESHOLD as u32 + 1) {
            let k = format!("Frontier(db.s.extra{i})");
            let mut c = ParquetSchemaCache::empty(dir.path());
            c.upsert(
                k,
                SchemaEntry::from_sdf_arrow_schema(None, make_schema("x")),
            )
            .unwrap();
            c.save().unwrap();
        }

        // Second compaction must succeed (0.parquet already exists).
        let epochs2 = existing_epochs(dir.path());
        assert!(
            compact_epochs(dir.path(), &epochs2, &CompactionContext::default()).is_ok(),
            "compaction must succeed when 0.parquet already exists"
        );

        // Original entry must survive.
        let loaded = ParquetSchemaCache::load(dir.path(), &[(key.to_string(), None)], false);
        assert!(
            loaded.get(key).is_some(),
            "original entry must survive double compaction"
        );
    }

    #[test]
    fn corrupt_epoch_file_skipped_gracefully() {
        let dir = tempfile::tempdir().unwrap();
        let key = "Frontier(db.s.t)";

        // Write a valid epoch 0.
        let mut cache = ParquetSchemaCache::empty(dir.path());
        cache
            .upsert(
                key.to_string(),
                SchemaEntry::from_sdf_arrow_schema(None, make_schema("col")),
            )
            .unwrap();
        cache.save().unwrap();

        // Write garbage as epoch 1.
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(dir.path().join("1.parquet"), b"this is not valid parquet").unwrap();

        // Load must succeed — corrupt file skipped, valid epoch 0 still loaded.
        let loaded = ParquetSchemaCache::load(dir.path(), &[(key.to_string(), None)], false);
        assert_eq!(
            loaded.len(),
            1,
            "valid entries must survive despite corrupt epoch file"
        );
        assert_eq!(loaded.get(key).unwrap().inner().field(0).name(), "col");
    }

    #[test]
    fn corrupt_ipc_bytes_in_row_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let valid_key = "Frontier(db.s.valid)";
        let corrupt_key = "Frontier(db.s.corrupt)";

        // Write a file with one valid row and one corrupt row.
        let valid_schema = serialize_schema(&make_schema("ok")).unwrap();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let rows = vec![
            SchemaRow {
                lookup_key: valid_key.to_string(),
                sdf_schema: valid_schema,
                original_schema: None,
                cached_at_ms: now_ms,
                build_hash: env!("CARGO_PKG_VERSION").to_string(),
                epoch: 0,
            },
            SchemaRow {
                lookup_key: corrupt_key.to_string(),
                sdf_schema: b"not valid ipc bytes".to_vec(),
                original_schema: None,
                cached_at_ms: now_ms,
                build_hash: env!("CARGO_PKG_VERSION").to_string(),
                epoch: 0,
            },
        ];
        std::fs::create_dir_all(dir.path()).unwrap();
        write_rows(&dir.path().join("0.parquet"), &rows).unwrap();

        // Load: valid row must survive, corrupt row must be silently skipped.
        let loaded = ParquetSchemaCache::load(
            dir.path(),
            &[
                (valid_key.to_string(), None),
                (corrupt_key.to_string(), None),
            ],
            false,
        );
        assert!(loaded.get(valid_key).is_some(), "valid row must load");
        assert!(
            loaded.get(corrupt_key).is_none(),
            "corrupt IPC row must be skipped"
        );
    }
}
