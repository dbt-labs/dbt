//! High-performance fusion ingest: reads target/private/metadata/ parquet with Arrow,
//! writes the index parquet directly with IndexWriter — no DuckDB involved.
//!
//! This is the CLI path for `dbt-index ingest --fusion`. The serve path
//! (`dbt-index serve --fusion`) uses `metadata_to_duckdb::apply_delta` instead,
//! which keeps an in-memory DuckDB so every query gets a live queryable DB.
//!
//! Speed: parquet→parquet with Arrow = no SQL parse/exec overhead.
//! Expected: ~200ms cold for 6k nodes vs 4.5s through DuckDB ADBC.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde_json::{Value, json};

use crate::IndexError;
use crate::epoch_layers;
use crate::ingest::ingest_state::{
    CATALOG_COLUMNS_SUBDIR, COMPILE_CLL_SUBDIR, COMPILE_COLUMNS_SUBDIR, COMPILE_NODES_SUBDIR,
    IngestState, PARSE_ALIVE, PARSE_COLUMNS_SUBDIR, PARSE_GENERATION, PARSE_NODES_SUBDIR,
    PARSE_PROJECT, PARSE_RESOLVER_STATE, RUN_CATALOG_STATS_SUBDIR, RUN_FRESHNESS_SUBDIR,
    RUN_INVOCATIONS_SUBDIR, RUN_RESULTS_SUBDIR, mtime_us,
};
use crate::ingest::payload::{
    ParsedDoc, ParsedExposure, ParsedGroup, ParsedMacro, ParsedMetric, ParsedSavedQuery,
    ParsedSemanticModel, ParsedTimeSpine, ParsedUnitTest,
};
use crate::ingest::timings;
use crate::parquet::{IndexWriter, WriteMode};

/// Columns projected when reading parse/nodes epoch parquet files.
/// Skips large unused columns (compiled_code etc.) for faster reads.
pub const PARSE_NODES_COLS: &[&str] = &[
    "unique_id",
    "resource_type",
    "name",
    "package_name",
    "original_path",
    "description",
    "database",
    "schema",
    "alias",
    "relation_name",
    "identifier",
    "materialization",
    "access",
    "group_name",
    "source_name",
    "tags",
    "fqn",
    "depends_on",
    "ingested_at",
    "is_disabled",
    "payload",
];

// ---------------------------------------------------------------------------
// IngestState persistence — saved to index_dir/.fusion_state.json
// ---------------------------------------------------------------------------

pub const STATE_FILE: &str = ".fusion_state.json";

/// Whether `index_dir` already holds a completed ingest — its persisted state
/// file is present. Lets a caller reuse an existing index as a staging
/// intermediate rather than building one, and conversely avoid materialising an
/// index the caller opted out of (`--no-write-index`).
pub fn has_persisted_state(index_dir: &Path) -> bool {
    index_dir.join(STATE_FILE).exists()
}

/// Serializable mirror of IngestState for disk persistence.
#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct PersistedState {
    /// micros-since-epoch for alive.parquet mtime
    pub alive_mtime_us: Option<u64>,
    /// last epoch number per subdir
    pub last_epoch: HashMap<String, u32>,
    /// micros-since-epoch for `{subdir}/v1_0.parquet` mtime. Detects an in-place
    /// rewrite of the base epoch, which epoch numbers alone cannot see. Absent in
    /// state files written between #10624 and this fix — `default` makes those
    /// reload in full once, then track normally.
    #[serde(default)]
    pub base_mtime_us: HashMap<String, u64>,
}

pub fn save_state(index_dir: &Path, state: &IngestState) {
    let alive_mtime_us = state.alive_mtime.and_then(mtime_us);
    let last_epoch = state
        .last_epoch
        .iter()
        .map(|(k, v)| ((*k).to_string(), *v))
        .collect();
    let base_mtime_us = state
        .base_mtime
        .iter()
        .filter_map(|(k, t)| mtime_us(*t).map(|us| ((*k).to_string(), us)))
        .collect();
    let ps = PersistedState {
        alive_mtime_us,
        last_epoch,
        base_mtime_us,
    };
    if let Ok(json) = serde_json::to_string(&ps) {
        let _ = std::fs::write(index_dir.join(STATE_FILE), json);
    }
}

pub fn load_state(index_dir: &Path) -> Option<IngestState> {
    let bytes = std::fs::read(index_dir.join(STATE_FILE)).ok()?;
    let ps: PersistedState = serde_json::from_slice(&bytes).ok()?;

    let alive_mtime = ps
        .alive_mtime_us
        .map(|us| UNIX_EPOCH + Duration::from_micros(us));

    // Every subdir that any write_* fn calls set_epoch for. A subdir missing here
    // never has its epoch restored, so it silently full-reloads on every delta.
    const ALL_SUBDIRS: &[&str] = &[
        PARSE_NODES_SUBDIR,
        PARSE_COLUMNS_SUBDIR,
        COMPILE_NODES_SUBDIR,
        COMPILE_COLUMNS_SUBDIR,
        COMPILE_CLL_SUBDIR,
        CATALOG_COLUMNS_SUBDIR,
        RUN_INVOCATIONS_SUBDIR,
        RUN_RESULTS_SUBDIR,
        RUN_FRESHNESS_SUBDIR,
        RUN_CATALOG_STATS_SUBDIR,
    ];
    let mut last_epoch = HashMap::new();
    let mut base_mtime = HashMap::new();
    for &subdir in ALL_SUBDIRS {
        if let Some(&epoch) = ps.last_epoch.get(subdir) {
            last_epoch.insert(subdir, epoch);
        }
        if let Some(&us) = ps.base_mtime_us.get(subdir) {
            base_mtime.insert(subdir, UNIX_EPOCH + Duration::from_micros(us));
        }
    }

    Some(IngestState {
        alive_mtime,
        last_epoch,
        index_dir: None,
        alive_ids: HashSet::new(),
        base_mtime,
    })
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Smart entry point: loads persisted IngestState from index_dir if present
/// (delta path), otherwise does a full cold ingest. Saves updated state on
/// success so the next call only processes new epochs.
pub fn ingest_from_metadata_direct(
    metadata_dir: &Path,
    index_dir: &Path,
    _state: &mut IngestState,
) -> Result<usize, IndexError> {
    // Auto-detect cold vs delta based on persisted state.
    match load_state(index_dir) {
        Some(mut state) => {
            let n = apply_delta_direct(metadata_dir, index_dir, &mut state)?;
            save_state(index_dir, &state);
            Ok(n)
        }
        None => {
            let mut state = IngestState::default();
            let n = cold_ingest(metadata_dir, index_dir, &mut state)?;
            save_state(index_dir, &state);
            Ok(n)
        }
    }
}

/// Attributed to [`timings::Stage::RowBuild`] as a whole, exclusive of the read,
/// payload-parse and write stages nested inside it — so the benchmark accounts for
/// every table's row building, not only the instrumented ones.
fn cold_ingest(
    metadata_dir: &Path,
    index_dir: &Path,
    state: &mut IngestState,
) -> Result<usize, IndexError> {
    timings::time(timings::Stage::RowBuild, || {
        cold_ingest_inner(metadata_dir, index_dir, state)
    })
}

fn cold_ingest_inner(
    metadata_dir: &Path,
    index_dir: &Path,
    state: &mut IngestState,
) -> Result<usize, IndexError> {
    let mut writer = IndexWriter::new(index_dir)?;
    let now = writer.now().to_string();
    let mut total = 0;

    let alive_ids = load_alive_ids(metadata_dir);
    let compile_map = load_compile_nodes_map(metadata_dir, state, true)?;
    total += write_parse_nodes(
        &mut writer,
        metadata_dir,
        state,
        &now,
        true,
        &compile_map,
        alive_ids.as_ref(),
    )?;
    total += write_parse_columns(
        &mut writer,
        metadata_dir,
        state,
        &now,
        true,
        alive_ids.as_ref(),
    )?;
    total += write_parse_project(&mut writer, metadata_dir, &now)?;
    total += write_parse_generation(&mut writer, metadata_dir, &now)?;
    total += write_compile_columns(
        &mut writer,
        metadata_dir,
        state,
        &now,
        true,
        alive_ids.as_ref(),
    )?;
    total += write_catalog_columns(
        &mut writer,
        metadata_dir,
        state,
        &now,
        true,
        alive_ids.as_ref(),
    )?;
    total += write_compile_cll(
        &mut writer,
        metadata_dir,
        state,
        &now,
        true,
        alive_ids.as_ref(),
    )?;
    total += write_run_invocations(&mut writer, metadata_dir, state, &now, true)?;
    total += write_run_results(&mut writer, metadata_dir, state, &now, true)?;
    total += write_column_stats(&mut writer, index_dir, &now)?;
    total += write_seed_catalog_stats(&mut writer, index_dir, metadata_dir, &now)?;
    total += write_warehouse_catalog_stats(&mut writer, metadata_dir, state, &now, true)?;
    total += write_run_freshness(&mut writer, metadata_dir, state, &now, true)?;
    timings::time(timings::Stage::StagingWrite, || writer.finish())?;

    // Store at µs precision to match what save_state serializes.
    let alive_us: Option<u64> = std::fs::metadata(metadata_dir.join(PARSE_ALIVE))
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(mtime_us);
    state.alive_mtime = alive_us.map(|us| UNIX_EPOCH + Duration::from_micros(us));

    Ok(total)
}

/// Whether `index_dir` already reflects everything written to `metadata_dir`.
///
/// The read-only counterpart to [`apply_delta_direct`]: this is true in exactly the cases where
/// that function would return `Ok(0)` without writing. Keep the two conditions in step — if they
/// drift, a caller either queries a stale index or refuses a current one.
///
/// Returns false when there is no ingest state to read, which covers both "no index" and "a
/// directory of parquet that no ingest ever vouched for".
///
/// Used by callers that must not write, notably the in-graph check task: checks are pure readers of
/// the index, so they need to ask whether it is safe to query rather than bringing it up to date
/// themselves.
pub fn index_is_current(metadata_dir: &Path, index_dir: &Path) -> bool {
    let Some(state) = load_state(index_dir) else {
        return false;
    };
    // Compare at microsecond precision — state is persisted as µs, OS mtime is ns.
    let current_us: Option<u64> = std::fs::metadata(metadata_dir.join(PARSE_ALIVE))
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(mtime_us);
    let stored_us: Option<u64> = state.alive_mtime.and_then(mtime_us);
    current_us == stored_us && !has_unseen_compile_epochs(metadata_dir, &state)
}

/// Incremental delta ingest: reads only new epochs since last run,
/// merges into existing index_dir parquet files.
///
/// Attributed to [`timings::Stage::RowBuild`] on the same terms as
/// [`cold_ingest`].
pub fn apply_delta_direct(
    metadata_dir: &Path,
    index_dir: &Path,
    state: &mut IngestState,
) -> Result<usize, IndexError> {
    timings::time(timings::Stage::RowBuild, || {
        apply_delta_direct_inner(metadata_dir, index_dir, state)
    })
}

fn apply_delta_direct_inner(
    metadata_dir: &Path,
    index_dir: &Path,
    state: &mut IngestState,
) -> Result<usize, IndexError> {
    // Compare at microsecond precision — state is persisted as µs, OS mtime is ns.
    let current_us: Option<u64> = std::fs::metadata(metadata_dir.join(PARSE_ALIVE))
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(mtime_us);
    let stored_us: Option<u64> = state.alive_mtime.and_then(mtime_us);
    let alive_changed = current_us != stored_us;

    // Compile-phase artifacts (CLL, compile/nodes, compile/columns) can be written
    // without a reparse — e.g. `--static-analysis strict` on a non-clean target.
    // Check for new compile epochs even when alive_mtime is unchanged.
    let has_new_compile_epochs = !alive_changed && has_unseen_compile_epochs(metadata_dir, state);

    if !alive_changed && !has_new_compile_epochs {
        return Ok(0);
    }
    if alive_changed {
        state.alive_mtime = current_us.map(|us| UNIX_EPOCH + Duration::from_micros(us));
    }

    let mut writer = IndexWriter::new(index_dir)?;
    let now = writer.now().to_string();
    let mut total = 0;

    let alive_ids = load_alive_ids(metadata_dir);

    if !alive_changed {
        // Parse layer unchanged, but compile epochs landed since the last ingest — the case where
        // an earlier publish in this same invocation already consumed the parse epochs (the index
        // is published right after parse, then the end-of-invocation ingest arrives here).
        //
        // `write_parse_nodes` cannot do this pass: it applies the compile map while *constructing*
        // node rows from parse epochs, and it filters to epochs newer than
        // `last_epoch_for(PARSE_NODES_SUBDIR)`, which the earlier publish already advanced past
        // every parse epoch. It would see no new epochs and write nothing, leaving `compiled_code`
        // NULL for the rest of the index's life.
        //
        // So apply the compile layer to the rows already on disk instead, exactly as the DuckDB
        // ingest does with `UPDATE dbt.nodes … FROM read_parquet(…)`. Nothing else in the index
        // reads the parse epochs a second time, so `edges`, `docs`, `macros` and the rest are left
        // untouched rather than rewritten with identical content and a fresh `ingested_at`.
        let compile_map = load_compile_nodes_map(metadata_dir, state, false)?;
        if !compile_map.is_empty() {
            total += writer.merge_compiled_nodes_into_nodes(&compile_map)?;
        }
    }

    if alive_changed {
        let compile_map = load_compile_nodes_map(metadata_dir, state, false)?;
        total += write_parse_nodes(
            &mut writer,
            metadata_dir,
            state,
            &now,
            false,
            &compile_map,
            alive_ids.as_ref(),
        )?;
        // `write_parse_nodes` applied the map only to the rows it rebuilt, i.e. to nodes that were
        // *reparsed*. Compile recompiles everything downstream of a change, so a node routinely
        // appears in the compile epoch without appearing in the parse delta — editing one model
        // recompiles its children while reparsing only itself. Those rows are carried forward
        // untouched, keeping the previous run's `compiled_code`, and the compile epoch is marked
        // consumed, so nothing would ever revisit them.
        //
        // Applying the map a second time here covers them. Rows that already hold these values are
        // left alone and the whole call writes nothing, so the common case where the compile epoch
        // and the parse delta name the same nodes costs a read.
        total += writer.merge_compiled_nodes_into_nodes(&compile_map)?;
        total += write_parse_columns(
            &mut writer,
            metadata_dir,
            state,
            &now,
            false,
            alive_ids.as_ref(),
        )?;
        total += write_parse_project(&mut writer, metadata_dir, &now)?;
        total += write_parse_generation(&mut writer, metadata_dir, &now)?;
    }
    total += write_compile_columns(
        &mut writer,
        metadata_dir,
        state,
        &now,
        false,
        alive_ids.as_ref(),
    )?;
    total += write_catalog_columns(
        &mut writer,
        metadata_dir,
        state,
        &now,
        false,
        alive_ids.as_ref(),
    )?;
    total += write_compile_cll(
        &mut writer,
        metadata_dir,
        state,
        &now,
        false,
        alive_ids.as_ref(),
    )?;
    if alive_changed {
        total += write_run_invocations(&mut writer, metadata_dir, state, &now, false)?;
        total += write_run_results(&mut writer, metadata_dir, state, &now, false)?;
        total += write_column_stats(&mut writer, index_dir, &now)?;
        total += write_seed_catalog_stats(&mut writer, index_dir, metadata_dir, &now)?;
        total += write_warehouse_catalog_stats(&mut writer, metadata_dir, state, &now, false)?;
        total += write_run_freshness(&mut writer, metadata_dir, state, &now, false)?;
    }
    timings::time(timings::Stage::StagingWrite, || writer.finish())?;

    Ok(total)
}

/// Returns true if any compile-phase subdirectory has epochs beyond what was last
/// ingested, or had its base epoch rewritten in place.
pub fn has_unseen_compile_epochs(metadata_dir: &Path, state: &IngestState) -> bool {
    const COMPILE_SUBDIRS: &[&str] = &[
        COMPILE_NODES_SUBDIR,
        COMPILE_COLUMNS_SUBDIR,
        COMPILE_CLL_SUBDIR,
        CATALOG_COLUMNS_SUBDIR,
    ];
    for &subdir in COMPILE_SUBDIRS {
        let dir = metadata_dir.join(subdir);
        if !dir.exists() {
            continue;
        }
        if state.base_differs(subdir, &dir) {
            return true;
        }
        let last = state.last_epoch_for(subdir);
        // Any epoch number other than `last` means something changed: higher = new
        // epochs, lower = compaction reset the numbering.
        match epoch_layers::existing_epochs(&dir)
            .iter()
            .map(|(n, _)| *n)
            .max()
        {
            Some(max) if last == u32::MAX || max != last => return true,
            _ => {}
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Helpers — read a parquet file into RecordBatches
// ---------------------------------------------------------------------------

pub(crate) fn read_parquet_batches(
    path: &Path,
) -> Result<Vec<arrow_array::RecordBatch>, IndexError> {
    read_parquet_batches_proj(path, &[])
}

/// Read parquet, projecting only the named columns (empty = all columns).
pub fn read_parquet_batches_proj(
    path: &Path,
    cols: &[&str],
) -> Result<Vec<arrow_array::RecordBatch>, IndexError> {
    timings::time(timings::Stage::EpochRead, || {
        read_parquet_batches_proj_inner(path, cols)
    })
}

fn read_parquet_batches_proj_inner(
    path: &Path,
    cols: &[&str],
) -> Result<Vec<arrow_array::RecordBatch>, IndexError> {
    let file = std::fs::File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| IndexError::Other(format!("parquet open {}: {e}", path.display())))?;
    let builder = builder.with_batch_size(65536);
    let builder = if cols.is_empty() {
        builder
    } else {
        let schema = builder.schema();
        let indices: Vec<usize> = cols
            .iter()
            .filter_map(|name| schema.index_of(name).ok())
            .collect();
        let mask = parquet::arrow::ProjectionMask::roots(builder.parquet_schema(), indices);
        builder.with_projection(mask)
    };
    let reader = builder
        .build()
        .map_err(|e| IndexError::Other(format!("parquet reader {}: {e}", path.display())))?;
    let mut batches = Vec::new();
    for batch in reader {
        batches.push(batch.map_err(|e| IndexError::Other(format!("parquet read: {e}")))?);
    }
    Ok(batches)
}

// ---------------------------------------------------------------------------
// Arrow column extraction helpers
// ---------------------------------------------------------------------------

pub fn str_col<'a>(
    batch: &'a arrow_array::RecordBatch,
    name: &str,
) -> Option<&'a arrow_array::StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<arrow_array::StringArray>())
}

fn i64_col<'a>(
    batch: &'a arrow_array::RecordBatch,
    name: &str,
) -> Option<&'a arrow_array::Int64Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<arrow_array::Int64Array>())
}

pub fn bool_col<'a>(
    batch: &'a arrow_array::RecordBatch,
    name: &str,
) -> Option<&'a arrow_array::BooleanArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<arrow_array::BooleanArray>())
}

pub fn timestamp_micros_col<'a>(
    batch: &'a arrow_array::RecordBatch,
    name: &str,
) -> Option<&'a arrow_array::TimestampMicrosecondArray> {
    batch.column_by_name(name).and_then(|c| {
        c.as_any()
            .downcast_ref::<arrow_array::TimestampMicrosecondArray>()
    })
}

fn i32_col<'a>(
    batch: &'a arrow_array::RecordBatch,
    name: &str,
) -> Option<&'a arrow_array::Int32Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<arrow_array::Int32Array>())
}

pub(crate) fn f64_col<'a>(
    batch: &'a arrow_array::RecordBatch,
    name: &str,
) -> Option<&'a arrow_array::Float64Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<arrow_array::Float64Array>())
}

fn list_col_batch<'a>(
    batch: &'a arrow_array::RecordBatch,
    name: &str,
) -> Option<&'a arrow_array::ListArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<arrow_array::ListArray>())
}

fn get_str(col: Option<&arrow_array::StringArray>, i: usize) -> Option<String> {
    crate::ingest::payload::str_col(col, i)
}

fn get_i64(col: Option<&arrow_array::Int64Array>, i: usize) -> Option<i64> {
    use arrow_array::Array;
    col.filter(|c| !c.is_null(i)).map(|c| c.value(i))
}

fn get_timestamp_micros(
    col: Option<&arrow_array::TimestampMicrosecondArray>,
    i: usize,
) -> Option<i64> {
    use arrow_array::Array;
    col.filter(|c| !c.is_null(i)).map(|c| c.value(i))
}

fn get_i32(col: Option<&arrow_array::Int32Array>, i: usize) -> Option<i32> {
    use arrow_array::Array;
    col.filter(|c| !c.is_null(i)).map(|c| c.value(i))
}

fn get_f64(col: Option<&arrow_array::Float64Array>, i: usize) -> Option<f64> {
    use arrow_array::Array;
    col.filter(|c| !c.is_null(i)).map(|c| c.value(i))
}

fn get_list(col: Option<&arrow_array::ListArray>, i: usize) -> Vec<String> {
    crate::ingest::payload::list_col(col, i)
}

/// The raw text of the *top-level* field `key` of the JSON object `json`, or
/// `None` if `json` is not an object or has no such field.
///
/// Walks the object's own members rather than searching the text for
/// `"key":`. A text search finds whichever occurrence comes first, which for a
/// model with documented columns is a column's nested `"config":` and not the
/// node's — the node's config then reads as `{}`.
pub fn extract_json_field_raw<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let bytes = json.as_bytes();
    let skip_ws = |mut i: usize| {
        while matches!(bytes.get(i), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            i += 1;
        }
        i
    };

    let mut i = skip_ws(0);
    if bytes.get(i) != Some(&b'{') {
        return None;
    }
    i += 1;
    loop {
        i = skip_ws(i);
        match bytes.get(i)? {
            b'}' => return None,
            b',' => {
                i += 1;
                continue;
            }
            b'"' => {}
            // Not a member of a well-formed object; stop rather than guess.
            _ => return None,
        }
        // The member name, quotes included.
        let name_end = find_json_value_end(json, i)?;
        let name = json.get(i + 1..name_end - 1)?;
        i = skip_ws(name_end);
        if bytes.get(i) != Some(&b':') {
            return None;
        }
        i = skip_ws(i + 1);
        let value_end = find_json_value_end(json, i)?;
        if name == key {
            return json.get(i..value_end);
        }
        i = value_end;
    }
}

/// A model payload reduced to the fields the parse-node ingest reads.
///
/// Model payloads are multi-KB — mostly `raw_code`, `compiled_code` and the
/// per-column documentation — so rather than pay for a full `simd_json` parse of
/// the whole thing this keeps two fields and drops the rest.
///
/// Dropping `raw_code` from `__common_attr__` keeps the trim's point (model
/// payloads are multi-KB mostly because of source and compiled SQL) while
/// leaving `patch_path`, `language`, `checksum` and `meta` available to the
/// row builder. Those used to be dropped wholesale with `__common_attr__`,
/// so every model published them as NULL.
///
/// `config` is kept as raw JSON text, since only some of the builders parse it.
/// `__model_attr__` is kept whole and parsed: `DbtModelAttr` is a fixed, small
/// struct with no code or column blobs in it, so keeping all of it costs little
/// and means a builder that starts reading a new attribute finds it here. An
/// allowlist of sub-objects would instead have to be widened in step with the
/// builders, and forgetting to is silent — that is how `time_spine` came to be
/// dropped, leaving `dbt.time_spines` empty for every project.
///
/// Shared so the delta path in `dbt-index` trims exactly the same way.
pub fn trim_model_payload(raw: Option<&str>) -> Value {
    let mut obj = serde_json::Map::new();
    if let Some(cfg) = raw.and_then(|r| extract_json_field_raw(r, "config")) {
        obj.insert("config".to_string(), Value::String(cfg.to_string()));
    }
    if let Some(ma) = raw
        .and_then(|r| extract_json_field_raw(r, "__model_attr__"))
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
    {
        obj.insert("__model_attr__".to_string(), ma);
    }
    if let Some(Value::Object(mut common)) = raw
        .and_then(|r| extract_json_field_raw(r, "__common_attr__"))
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
    {
        common.remove("raw_code");
        obj.insert("__common_attr__".to_string(), Value::Object(common));
    }
    Value::Object(obj)
}

/// Find the end offset of a JSON value starting at `start` in `s`.
fn find_json_value_end(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let b = *bytes.get(start)?;
    match b {
        b'{' => find_matching(bytes, start, b'{', b'}'),
        b'[' => find_matching(bytes, start, b'[', b']'),
        b'"' => {
            let mut i = start + 1;
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' => i += 2,
                    b'"' => return Some(i + 1),
                    _ => i += 1,
                }
            }
            None
        }
        _ => {
            // number, boolean, null — scan until delimiter
            let end = bytes[start..]
                .iter()
                .position(|&c| matches!(c, b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t'))?;
            Some(start + end)
        }
    }
}

fn find_matching(bytes: &[u8], start: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = start;
    let mut in_string = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 2,
            b'"' => {
                in_string = !in_string;
                i += 1;
            }
            c if !in_string && c == open => {
                depth += 1;
                i += 1;
            }
            c if !in_string && c == close => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// micros since epoch → RFC3339 timestamp string
fn micros_to_ts(micros: i64) -> String {
    use std::time::{Duration, UNIX_EPOCH};
    let d = UNIX_EPOCH + Duration::from_micros(micros as u64);
    let secs = d.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let nanos = (micros % 1_000_000) * 1000;
    chrono::DateTime::<chrono::Utc>::from(UNIX_EPOCH + Duration::new(secs, nanos as u32))
        .to_rfc3339()
}

fn micros_to_datetime(micros: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp_micros(micros).unwrap_or_else(chrono::Utc::now)
}

// ---------------------------------------------------------------------------
// Compile fields — loaded ahead of parse so they can be merged in one pass
// ---------------------------------------------------------------------------

pub struct CompileFields {
    pub compiled_code: Option<String>,
    pub compiled_code_hash: Option<String>,
    pub compiled_path: Option<String>,
    pub grain: Vec<String>,
    pub grain_declared: Vec<String>,
    pub grain_tested: Vec<String>,
    pub classifiers: Vec<String>,
    pub table_role: Option<String>,
}

pub type CompileMap = HashMap<String, CompileFields>;

/// Load alive.parquet → set of live unique_ids (None if file absent).
pub fn load_alive_ids(metadata_dir: &Path) -> Option<HashSet<String>> {
    let path = metadata_dir.join(PARSE_ALIVE);
    if !path.exists() {
        return None;
    }
    let batches = read_parquet_batches_proj(&path, &["unique_id"]).ok()?;
    let mut ids = HashSet::new();
    for b in &batches {
        if let Some(col) = str_col(b, "unique_id") {
            use arrow_array::Array;
            for i in 0..b.num_rows() {
                if !col.is_null(i) {
                    ids.insert(col.value(i).to_string());
                }
            }
        }
    }
    Some(ids)
}

/// Prune rows whose `unique_id` is not in `alive_ids`. No-op when `alive_ids` is None.
pub fn prune_by_alive(rows: &mut Vec<Value>, alive_ids: Option<&HashSet<String>>) {
    let Some(ids) = alive_ids else { return };
    rows.retain(|r| {
        r.get("unique_id")
            .and_then(|v| v.as_str())
            .map(|id| ids.contains(id))
            .unwrap_or(false)
    });
}

/// Read all compile/nodes epochs into a uid→CompileFields map.
/// Returns an empty map (not an error) if the directory doesn't exist.
/// `full`: when true, detects base compaction and reloads all epochs if needed.
pub fn load_compile_nodes_map(
    metadata_dir: &Path,
    state: &mut IngestState,
    full: bool,
) -> Result<CompileMap, IndexError> {
    let dir = metadata_dir.join(COMPILE_NODES_SUBDIR);
    if !dir.exists() {
        return Ok(CompileMap::new());
    }

    let epochs = epoch_layers::existing_epochs(&dir);
    if epochs.is_empty() {
        return Ok(CompileMap::new());
    }

    let last = state.last_epoch_for(COMPILE_NODES_SUBDIR);
    let curr_max_ep = epochs.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let need_full = state.needs_full_reload(COMPILE_NODES_SUBDIR, &dir, curr_max_ep, full);
    let new_epochs: Vec<_> = if need_full {
        epochs
    } else {
        epochs.into_iter().filter(|(n, _)| *n > last).collect()
    };

    if new_epochs.is_empty() {
        return Ok(CompileMap::new());
    }

    const COMPILE_COLS: &[&str] = &[
        "unique_id",
        "compiled_code",
        "compiled_code_hash",
        "compiled_path",
        "grain",
        "grain_declared",
        "grain_tested",
        "classifiers",
        "table_role",
    ];

    let mut map = CompileMap::new();
    let mut last_epoch = 0u32;
    for (epoch_num, path) in &new_epochs {
        let batches = read_parquet_batches_proj(path, COMPILE_COLS)?;
        for batch in &batches {
            let uid_col = str_col(batch, "unique_id");
            let ccode_col = str_col(batch, "compiled_code");
            let chash_col = str_col(batch, "compiled_code_hash");
            let cpath_col = str_col(batch, "compiled_path");
            let grain_col = str_col(batch, "grain");
            let gdecl_col = str_col(batch, "grain_declared");
            let gtest_col = str_col(batch, "grain_tested");
            let classifiers_col = str_col(batch, "classifiers");
            let trole_col = str_col(batch, "table_role");
            for i in 0..batch.num_rows() {
                let Some(uid) = get_str(uid_col, i) else {
                    continue;
                };
                let grain = get_str(grain_col, i)
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .unwrap_or_default();
                let grain_declared = get_str(gdecl_col, i)
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .unwrap_or_default();
                let grain_tested = get_str(gtest_col, i)
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .unwrap_or_default();
                let classifiers = get_str(classifiers_col, i)
                    .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
                    .unwrap_or_default();
                map.insert(
                    uid,
                    CompileFields {
                        compiled_code: get_str(ccode_col, i),
                        compiled_code_hash: get_str(chash_col, i),
                        compiled_path: get_str(cpath_col, i),
                        grain,
                        grain_declared,
                        grain_tested,
                        classifiers,
                        table_role: get_str(trole_col, i),
                    },
                );
            }
        }
        last_epoch = *epoch_num;
    }
    state.set_epoch(COMPILE_NODES_SUBDIR, last_epoch);
    Ok(map)
}

// ---------------------------------------------------------------------------
// Parse nodes → dbt.nodes + dbt.edges + dbt.test_metadata + special tables
// ---------------------------------------------------------------------------

pub struct OwnedNodeRow {
    pub unique_id: String,
    pub name: Option<String>,
    pub resource_type: String,
    pub package_name: Option<String>,
    pub file_path: Option<String>,
    pub original_file_path: Option<String>,
    pub fqn: Vec<String>,
    pub alias: Option<String>,
    pub checksum: Option<String>,
    pub description: Option<String>,
    pub node_language: Option<String>,
    pub raw_code: Option<String>,
    pub raw_code_hash: Option<String>,
    pub database_name: Option<String>,
    pub schema_name: Option<String>,
    pub relation_name: Option<String>,
    pub identifier: Option<String>,
    pub enabled: Option<bool>,
    pub materialized: Option<String>,
    pub incremental_strategy: Option<String>,
    pub on_schema_change: Option<String>,
    pub unique_key: Option<String>,
    pub full_refresh: Option<bool>,
    pub persist_docs: Option<String>,
    pub pre_hook: Option<String>,
    pub post_hook: Option<String>,
    pub grants: Option<String>,
    pub access_level: Option<String>,
    pub group_name: Option<String>,
    pub contract_enforced: bool,
    pub node_constraints: Option<String>,
    pub primary_key: Vec<String>,
    pub docs_show: bool,
    pub time_spine: Option<String>,
    pub tags: Vec<String>,
    pub source_name: Option<String>,
    pub source_description: Option<String>,
    pub loader: Option<String>,
    pub loaded_at_field: Option<String>,
    pub loaded_at_query: Option<String>,
    pub freshness: Option<String>,
    pub external_config: Option<String>,
    pub quoting: Option<String>,
    pub patch_path: Option<String>,
    pub meta: Option<String>,
    pub config: Option<String>,
    pub deprecation_date: Option<String>,
    pub version: Option<String>,
    pub latest_version: Option<String>,
    pub compiled_code: Option<String>,
    pub compiled_code_hash: Option<String>,
    pub compiled_path: Option<String>,
    pub table_role: Option<String>,
    pub grain: Vec<String>,
    pub grain_declared: Vec<String>,
    pub grain_tested: Vec<String>,
    pub classifiers: Vec<String>,
    pub ingested_at: String,
}

impl OwnedNodeRow {
    /// Borrowed typed view for serde_arrow serialization. Both the batch
    /// ingest and the serve delta path build `dbt.nodes` rows through this
    /// one mapping so the two cannot drift
    pub fn as_node_row(&self) -> crate::parquet::NodeRow<'_> {
        crate::parquet::NodeRow {
            unique_id: &self.unique_id,
            name: self.name.as_deref(),
            resource_type: &self.resource_type,
            package_name: self.package_name.as_deref(),
            file_path: self.file_path.clone(),
            original_file_path: self.original_file_path.clone(),
            fqn: self.fqn.clone(),
            alias: self.alias.as_deref(),
            checksum: self.checksum.as_deref(),
            description: self.description.as_deref(),
            node_language: self.node_language.as_deref(),
            raw_code: self.raw_code.as_deref(),
            database_name: self.database_name.as_deref(),
            schema_name: self.schema_name.as_deref(),
            relation_name: self.relation_name.as_deref(),
            identifier: self.identifier.as_deref(),
            enabled: self.enabled,
            materialized: self.materialized.clone(),
            incremental_strategy: self.incremental_strategy.clone(),
            on_schema_change: self.on_schema_change.clone(),
            unique_key: self.unique_key.clone(),
            full_refresh: self.full_refresh,
            persist_docs: self.persist_docs.clone(),
            pre_hook: self.pre_hook.clone(),
            post_hook: self.post_hook.clone(),
            grants: self.grants.clone(),
            config: self.config.clone(),
            access_level: self.access_level.clone(),
            group_name: self.group_name.as_deref(),
            contract_enforced: self.contract_enforced,
            version: self.version.clone(),
            latest_version: self.latest_version.clone(),
            deprecation_date: self.deprecation_date.as_deref(),
            node_constraints: self.node_constraints.clone(),
            primary_key: self.primary_key.clone(),
            docs_show: self.docs_show,
            patch_path: self.patch_path.clone(),
            time_spine: self.time_spine.clone(),
            tags: self.tags.clone(),
            meta: self.meta.clone(),
            // Not present in fusion metadata; only the manifest.json path
            // populates it
            ai_context: None,
            source_name: self.source_name.as_deref(),
            source_description: self.source_description.as_deref(),
            loader: self.loader.as_deref(),
            loaded_at_field: self.loaded_at_field.as_deref(),
            loaded_at_query: self.loaded_at_query.as_deref(),
            freshness: self.freshness.clone(),
            external_config: self.external_config.clone(),
            // Not present in fusion metadata (source-block meta is folded into
            // `meta`); only the manifest.json path populates it
            source_meta: None,
            quoting: self.quoting.clone(),
            compiled_code: self.compiled_code.as_deref(),
            compiled_code_hash: self.compiled_code_hash.as_deref(),
            compiled_path: self.compiled_path.as_deref(),
            // Compile-time manifest fields with no parse-metadata equivalent
            extra_ctes: None,
            compiled_at: None,
            raw_code_hash: self.raw_code_hash.clone(),
            search_text: None,
            grain: self.grain.clone(),
            grain_declared: self.grain_declared.clone(),
            grain_tested: self.grain_tested.clone(),
            grain_inferred: vec![],
            classifiers: self.classifiers.clone(),
            table_role: self.table_role.as_deref(),
            ingested_at: &self.ingested_at,
        }
    }
}

fn write_parse_nodes(
    writer: &mut IndexWriter,
    metadata_dir: &Path,
    state: &mut IngestState,
    now: &str,
    full: bool,
    compile_map: &CompileMap,
    alive_ids: Option<&HashSet<String>>,
) -> Result<usize, IndexError> {
    let dir = metadata_dir.join(PARSE_NODES_SUBDIR);
    if !dir.exists() {
        return Ok(0);
    }

    let epochs = epoch_layers::existing_epochs(&dir);
    let curr_max_ep = epochs.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let need_full = state.needs_full_reload(PARSE_NODES_SUBDIR, &dir, curr_max_ep, full);
    if epochs.is_empty() {
        return Ok(0);
    }

    let new_epochs: Vec<_> = if need_full {
        epochs
    } else {
        let last = state.last_epoch_for(PARSE_NODES_SUBDIR);
        epochs.into_iter().filter(|(n, _)| *n > last).collect()
    };
    if new_epochs.is_empty() {
        return Ok(0);
    }

    // Accumulate rows across all new epochs
    let mut node_rows: Vec<OwnedNodeRow> = Vec::new();
    let mut edge_rows: Vec<Value> = Vec::new();
    let mut test_meta_rows: Vec<Value> = Vec::new();
    // Special table rows
    let mut metric_rows: Vec<Value> = Vec::new();
    let mut semantic_model_rows: Vec<Value> = Vec::new();
    let mut semantic_entity_rows: Vec<Value> = Vec::new();
    let mut semantic_measure_rows: Vec<Value> = Vec::new();
    let mut semantic_dimension_rows: Vec<Value> = Vec::new();
    let mut saved_query_rows: Vec<Value> = Vec::new();
    let mut exposure_rows: Vec<Value> = Vec::new();
    let mut group_rows: Vec<Value> = Vec::new();
    let mut macro_rows: Vec<Value> = Vec::new();
    let mut doc_rows: Vec<Value> = Vec::new();
    let mut unit_test_rows: Vec<Value> = Vec::new();
    let mut time_spine_rows: Vec<Value> = Vec::new();

    // Columns projected from parse/nodes epochs — see module-level PARSE_NODES_COLS.

    // Process epochs newest-first: for full ingest the latest epoch wins per unique_id.
    // seen_node_ids tracks which nodes have already been emitted (from a newer epoch).
    let mut seen_node_ids: HashSet<String> = HashSet::new();
    let iterate_newest_first = need_full;

    let mut last_epoch_num = 0u32;
    let epoch_iter: Vec<_> = if iterate_newest_first {
        new_epochs.iter().rev().collect()
    } else {
        new_epochs.iter().collect()
    };
    for (epoch_num, path) in &epoch_iter {
        let batches = read_parquet_batches_proj(path, PARSE_NODES_COLS)?;
        for batch in &batches {
            // Parse payloads selectively: models only have `config` in their payload
            // (no raw_code, meta, patch_path, etc.), so we avoid the expensive full
            // simd_json parse for them and extract config as a raw JSON string instead.
            let payloads: Vec<Value> = timings::time(timings::Stage::PayloadParse, || {
                let payload_col = str_col(batch, "payload");
                let rt_col_inner = str_col(batch, "resource_type");
                use arrow_array::Array;
                (0..batch.num_rows())
                    .map(|i| {
                        let is_model = rt_col_inner
                            .filter(|c| !c.is_null(i))
                            .map(|c| c.value(i) == "model")
                            .unwrap_or(false);
                        if is_model {
                            trim_model_payload(
                                payload_col.filter(|c| !c.is_null(i)).map(|c| c.value(i)),
                            )
                        } else {
                            // Non-model nodes (macros, docs, sources, etc.): full parse needed.
                            payload_col
                                .filter(|c| !c.is_null(i))
                                .and_then(|c| {
                                    let mut buf = c.value(i).as_bytes().to_vec();
                                    simd_json::serde::from_slice::<Value>(&mut buf).ok()
                                })
                                .unwrap_or(Value::Null)
                        }
                    })
                    .collect()
            });
            extract_parse_nodes_batch(
                batch,
                now,
                &payloads,
                compile_map,
                if iterate_newest_first {
                    Some(&mut seen_node_ids)
                } else {
                    None
                },
                alive_ids,
                &mut node_rows,
                &mut edge_rows,
                &mut test_meta_rows,
                &mut metric_rows,
                &mut semantic_model_rows,
                &mut semantic_entity_rows,
                &mut semantic_measure_rows,
                &mut semantic_dimension_rows,
                &mut saved_query_rows,
                &mut exposure_rows,
                &mut group_rows,
                &mut macro_rows,
                &mut doc_rows,
                &mut unit_test_rows,
                &mut time_spine_rows,
            );
        }
        last_epoch_num = last_epoch_num.max(*epoch_num);
    }

    // On delta ingest (oldest-first, no inline dedup): prune against alive_ids after.
    if !iterate_newest_first {
        if let Some(ids) = alive_ids {
            node_rows.retain(|r| ids.contains(&r.unique_id));
            edge_rows.retain(|r| {
                r.get("parent_unique_id")
                    .and_then(|v| v.as_str())
                    .map(|id| ids.contains(id))
                    .unwrap_or(false)
                    && r.get("child_unique_id")
                        .and_then(|v| v.as_str())
                        .map(|id| ids.contains(id))
                        .unwrap_or(false)
            });
        }
        prune_by_alive(&mut test_meta_rows, alive_ids);
        prune_by_alive(&mut metric_rows, alive_ids);
        prune_by_alive(&mut semantic_model_rows, alive_ids);
        prune_by_alive(&mut semantic_entity_rows, alive_ids);
        prune_by_alive(&mut semantic_measure_rows, alive_ids);
        prune_by_alive(&mut semantic_dimension_rows, alive_ids);
        prune_by_alive(&mut saved_query_rows, alive_ids);
        prune_by_alive(&mut exposure_rows, alive_ids);
        prune_by_alive(&mut group_rows, alive_ids);
        prune_by_alive(&mut macro_rows, alive_ids);
        prune_by_alive(&mut doc_rows, alive_ids);
        prune_by_alive(&mut unit_test_rows, alive_ids);
        prune_by_alive(&mut time_spine_rows, alive_ids);
    }

    let count = node_rows.len();

    // Convert owned node rows to typed NodeRow<'_> for fast serde_arrow serialization.
    use crate::parquet::NodeRow;
    let typed_nodes: Vec<NodeRow<'_>> = node_rows.iter().map(OwnedNodeRow::as_node_row).collect();

    if need_full {
        writer.write_dbt_items("nodes", &typed_nodes)?;
        drop(typed_nodes);
        writer.write_dbt_table("edges", edge_rows)?;
        writer.write_dbt_table("test_metadata", test_meta_rows)?;
        writer.write_dbt_table("metrics", metric_rows)?;
        writer.write_dbt_table("semantic_models", semantic_model_rows)?;
        writer.write_dbt_table("semantic_entities", semantic_entity_rows)?;
        writer.write_dbt_table("semantic_measures", semantic_measure_rows)?;
        writer.write_dbt_table("semantic_dimensions", semantic_dimension_rows)?;
        writer.write_dbt_table("saved_queries", saved_query_rows)?;
        writer.write_dbt_table("exposures", exposure_rows)?;
        writer.write_dbt_table("groups", group_rows)?;
        writer.write_dbt_table("macros", macro_rows)?;
        writer.write_dbt_table("docs", doc_rows)?;
        writer.write_dbt_table("unit_tests", unit_test_rows)?;
        writer.write_dbt_table("time_spines", time_spine_rows)?;
    } else {
        // Delta: merge new rows into existing index (latest wins).
        // alive_ids determines which old rows to preserve — if unavailable, keep all.
        let all_valid: HashSet<String>;
        let valid = match alive_ids {
            Some(ids) => ids,
            None => {
                all_valid = node_rows.iter().map(|r| r.unique_id.clone()).collect();
                &all_valid
            }
        };

        // nodes always written — even an empty delta must carry forward compiled fields.
        writer.write_dbt_items_merged(
            "nodes",
            &typed_nodes,
            WriteMode::CarryForwardMerge {
                key_cols: &["unique_id"],
                carry_cols: &[
                    "compiled_code",
                    "compiled_code_hash",
                    "compiled_path",
                    "grain",
                    "grain_declared",
                    "grain_tested",
                    "table_role",
                ],
                valid_ids: valid,
            },
        )?;
        // Edges: replace edges for changed child nodes; preserve edges for alive unchanged nodes.
        if !edge_rows.is_empty() {
            writer.write_dbt_table_merged(
                "edges",
                edge_rows,
                WriteMode::MergePrune {
                    key_col: "child_unique_id",
                    valid_ids: valid,
                },
            )?;
        }
        // For snapshot tables: merge new rows, keep old rows for alive nodes.
        macro_rules! merge_if_nonempty {
            ($rows:expr, $table:literal) => {
                if !$rows.is_empty() {
                    writer.write_dbt_table_merged(
                        $table,
                        $rows,
                        WriteMode::MergePrune {
                            key_col: "unique_id",
                            valid_ids: valid,
                        },
                    )?;
                }
            };
        }
        merge_if_nonempty!(test_meta_rows, "test_metadata");
        merge_if_nonempty!(metric_rows, "metrics");
        merge_if_nonempty!(semantic_model_rows, "semantic_models");
        merge_if_nonempty!(semantic_entity_rows, "semantic_entities");
        merge_if_nonempty!(semantic_measure_rows, "semantic_measures");
        merge_if_nonempty!(semantic_dimension_rows, "semantic_dimensions");
        merge_if_nonempty!(saved_query_rows, "saved_queries");
        merge_if_nonempty!(exposure_rows, "exposures");
        merge_if_nonempty!(group_rows, "groups");
        merge_if_nonempty!(macro_rows, "macros");
        merge_if_nonempty!(doc_rows, "docs");
        merge_if_nonempty!(unit_test_rows, "unit_tests");
        merge_if_nonempty!(time_spine_rows, "time_spines");
    }

    state.set_epoch(PARSE_NODES_SUBDIR, last_epoch_num);
    Ok(count)
}

#[allow(clippy::too_many_arguments)]
pub fn extract_parse_nodes_batch(
    batch: &arrow_array::RecordBatch,
    now: &str,
    payloads: &[Value],
    compile_map: &CompileMap,
    mut seen_ids: Option<&mut HashSet<String>>,
    alive_ids: Option<&HashSet<String>>,
    node_rows: &mut Vec<OwnedNodeRow>,
    edge_rows: &mut Vec<Value>,
    test_meta_rows: &mut Vec<Value>,
    metric_rows: &mut Vec<Value>,
    semantic_model_rows: &mut Vec<Value>,
    semantic_entity_rows: &mut Vec<Value>,
    semantic_measure_rows: &mut Vec<Value>,
    semantic_dimension_rows: &mut Vec<Value>,
    saved_query_rows: &mut Vec<Value>,
    exposure_rows: &mut Vec<Value>,
    group_rows: &mut Vec<Value>,
    macro_rows: &mut Vec<Value>,
    doc_rows: &mut Vec<Value>,
    unit_test_rows: &mut Vec<Value>,
    time_spine_rows: &mut Vec<Value>,
) {
    let uid_col = str_col(batch, "unique_id");
    let rt_col = str_col(batch, "resource_type");
    let name_col = str_col(batch, "name");
    let pkg_col = str_col(batch, "package_name");
    let orig_col = str_col(batch, "original_path");
    let desc_col = str_col(batch, "description");
    let db_col = str_col(batch, "database");
    let schema_col = str_col(batch, "schema");
    let alias_col = str_col(batch, "alias");
    let rel_col = str_col(batch, "relation_name");
    let ident_col = str_col(batch, "identifier");
    let mat_col = str_col(batch, "materialization");
    let access_col = str_col(batch, "access");
    let grp_col = str_col(batch, "group_name");
    let src_col = str_col(batch, "source_name");
    let tags_col = list_col_batch(batch, "tags");
    let fqn_col = list_col_batch(batch, "fqn");
    let deps_col = list_col_batch(batch, "depends_on");
    let ts_col = i64_col(batch, "ingested_at");
    let dis_col = i32_col(batch, "is_disabled");

    for i in 0..batch.num_rows() {
        let Some(uid) = get_str(uid_col, i) else {
            continue;
        };

        // Alive check — skip nodes not in the live set.
        if let Some(ids) = alive_ids {
            if !ids.contains(&uid) {
                continue;
            }
        }
        // Dedup check — skip nodes already emitted from a newer epoch.
        if let Some(s) = seen_ids.as_mut() {
            if !s.insert(uid.clone()) {
                continue;
            }
        }

        let rt = get_str(rt_col, i).unwrap_or_default();

        let ts = get_i64(ts_col, i)
            .map(micros_to_ts)
            .unwrap_or_else(|| now.to_string());
        let tags = get_list(tags_col, i);
        let fqn = get_list(fqn_col, i);
        let deps = get_list(deps_col, i);

        // Use pre-parsed payload (parsed in parallel before this loop)
        let payload: &Value = payloads.get(i).unwrap_or(&Value::Null);

        // Fusion payloads nest node fields under attr envelopes
        // (`__common_attr__`, `__model_attr__`, `__source_attr__`); every read
        // below tries the envelope first and falls back to the same key at the
        // payload top level for legacy payload shapes
        let common = payload
            .get("__common_attr__")
            .cloned()
            .unwrap_or(Value::Null);
        let model_attr = payload
            .get("__model_attr__")
            .cloned()
            .unwrap_or(Value::Null);
        let source_attr = payload
            .get("__source_attr__")
            .cloned()
            .unwrap_or(Value::Null);

        let str_at = |v: &Value, key: &str| -> Option<String> {
            v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
        };
        // JSON-typed columns store the value's JSON text (matching the
        // manifest ingest's stringification)
        let json_at = |v: &Value, key: &str| -> Option<String> {
            v.get(key).filter(|x| !x.is_null()).map(|x| x.to_string())
        };

        let file_path = str_at(&common, "path");

        // `checksum` is either the {"name", "checksum"} object or a bare string
        let checksum = common
            .get("checksum")
            .or_else(|| payload.get("checksum"))
            .and_then(|c| {
                if c.is_object() {
                    c.get("checksum")
                } else {
                    Some(c)
                }
            })
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                payload
                    .get("checksum")
                    .and_then(|c| c.get("checksum"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            });

        let raw_code = common
            .get("raw_code")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                payload
                    .get("raw_code")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            });
        let node_language = common
            .get("language")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let raw_code_hash = raw_code.as_deref().map(crate::parquet::raw_code_hash);
        let patch_path = str_at(&common, "patch_path").or_else(|| str_at(payload, "patch_path"));
        let meta = common
            .get("meta")
            .filter(|v| !v.is_null())
            .or_else(|| payload.get("meta"))
            .cloned();

        // `contract` is mirrored on the model `config` (and on `DbtModelAttr`
        // under `__model_attr__`); it is NOT at the payload top level. For model
        // nodes, `config` is stored as a *raw JSON string*, not a parsed object,
        // so we must parse that string to reach `config.contract.enforced`.
        // `__model_attr__.contract` and a bare top-level `contract` are kept as
        // fallbacks for other (non-model / fully-parsed) payload shapes. The
        // original read targeted top-level `contract` (the old `manifest.json`
        // serialization) and silently always returned `false` once the source
        // became the trimmed `DbtModel` payload.
        let config_obj: Option<Value> = match payload.get("config") {
            Some(Value::String(s)) => serde_json::from_str(s).ok(),
            Some(v) => Some(v.clone()),
            None => None,
        };
        let contract_enforced = config_obj
            .as_ref()
            .and_then(|c| c.get("contract"))
            .or_else(|| model_attr.get("contract"))
            .or_else(|| payload.get("contract"))
            .and_then(|c| c.get("enforced"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let cfg = |key: &str| -> Option<&Value> {
            config_obj
                .as_ref()
                .and_then(|c| c.get(key))
                .filter(|v| !v.is_null())
        };
        let cfg_str = |key: &str| cfg(key).and_then(|v| v.as_str()).map(|s| s.to_string());
        // Empty containers count as unset, matching the manifest ingest
        let cfg_json = |key: &str| {
            cfg(key)
                .filter(|v| !matches!(v, Value::Object(m) if m.is_empty()))
                .filter(|v| !matches!(v, Value::Array(a) if a.is_empty()))
                .map(|v| v.to_string())
        };
        let incremental_strategy = cfg_str("incremental_strategy");
        let on_schema_change = cfg_str("on_schema_change");
        let unique_key = cfg_str("unique_key");
        let full_refresh = cfg("full_refresh").and_then(|v| v.as_bool());
        let persist_docs = cfg_json("persist_docs");
        // Fusion serializes hooks as pre_hook/post_hook; manifest configs use
        // the dashed spelling
        let pre_hook = cfg_json("pre_hook").or_else(|| cfg_json("pre-hook"));
        let post_hook = cfg_json("post_hook").or_else(|| cfg_json("post-hook"));
        let grants = cfg_json("grants");
        let docs_show = cfg("docs")
            .and_then(|d| d.get("show"))
            .or_else(|| payload.get("docs").and_then(|d| d.get("show")))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let node_constraints =
            json_at(&model_attr, "constraints").or_else(|| json_at(payload, "constraints"));
        let time_spine =
            json_at(&model_attr, "time_spine").or_else(|| json_at(payload, "time_spine"));
        let deprecation_date =
            str_at(&model_attr, "deprecation_date").or_else(|| str_at(payload, "deprecation_date"));

        // Model versions are strings or integers; both become their string form
        let version_str = |v: &Value| -> Option<String> {
            match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            }
        };
        let version = model_attr
            .get("version")
            .and_then(version_str)
            .or_else(|| payload.get("version").and_then(version_str));
        let latest_version = model_attr
            .get("latest_version")
            .and_then(version_str)
            .or_else(|| payload.get("latest_version").and_then(version_str));

        let primary_key: Vec<String> = model_attr
            .get("primary_key")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let enabled = get_i32(dis_col, i).map(|d| d == 0).unwrap_or(true);

        let source_name = get_str(src_col, i)
            .or_else(|| str_at(&source_attr, "source_name"))
            .or_else(|| str_at(payload, "source_name"));
        let source_description = str_at(&source_attr, "source_description")
            .or_else(|| str_at(payload, "source_description"));
        let loader = str_at(&source_attr, "loader").or_else(|| str_at(payload, "loader"));
        let loaded_at_field =
            str_at(&source_attr, "loaded_at_field").or_else(|| str_at(payload, "loaded_at_field"));
        let loaded_at_query =
            str_at(&source_attr, "loaded_at_query").or_else(|| str_at(payload, "loaded_at_query"));
        let freshness =
            json_at(&source_attr, "freshness").or_else(|| json_at(payload, "freshness"));
        let external_config =
            json_at(&source_attr, "external").or_else(|| json_at(payload, "external"));
        // The user-declared quoting block; `__base_attr__.quoting` is the
        // resolved form and intentionally not read here
        let quoting = json_at(&source_attr, "user_quoting").or_else(|| json_at(payload, "quoting"));

        let config = payload.get("config").cloned();

        // Nodes that go into dbt.nodes
        let write_to_nodes = matches!(
            rt.as_str(),
            "model"
                | "source"
                | "seed"
                | "snapshot"
                | "analysis"
                | "test"
                | "operation"
                | "function"
                | "check"
        );

        if write_to_nodes {
            let (
                compiled_code,
                compiled_code_hash,
                compiled_path,
                table_role,
                grain,
                grain_declared,
                grain_tested,
                classifiers,
            ) = if let Some(c) = compile_map.get(&uid) {
                (
                    c.compiled_code.clone(),
                    c.compiled_code_hash.clone(),
                    c.compiled_path.clone(),
                    c.table_role.clone(),
                    c.grain.clone(),
                    c.grain_declared.clone(),
                    c.grain_tested.clone(),
                    c.classifiers.clone(),
                )
            } else {
                (None, None, None, None, vec![], vec![], vec![], vec![])
            };
            node_rows.push(OwnedNodeRow {
                unique_id: uid.clone(),
                name: get_str(name_col, i),
                resource_type: rt.clone(),
                package_name: get_str(pkg_col, i),
                file_path,
                original_file_path: get_str(orig_col, i),
                fqn,
                alias: get_str(alias_col, i),
                checksum,
                description: get_str(desc_col, i),
                node_language,
                raw_code,
                raw_code_hash,
                database_name: get_str(db_col, i),
                schema_name: get_str(schema_col, i),
                relation_name: get_str(rel_col, i),
                identifier: get_str(ident_col, i)
                    .or_else(|| str_at(&source_attr, "identifier"))
                    .or_else(|| get_str(alias_col, i)),
                enabled: Some(enabled),
                materialized: get_str(mat_col, i),
                incremental_strategy,
                on_schema_change,
                unique_key,
                full_refresh,
                persist_docs,
                pre_hook,
                post_hook,
                grants,
                access_level: get_str(access_col, i).or_else(|| str_at(&model_attr, "access")),
                group_name: get_str(grp_col, i),
                contract_enforced,
                node_constraints,
                primary_key,
                docs_show,
                time_spine,
                tags: tags.clone(),
                source_name,
                source_description,
                loader,
                loaded_at_field,
                loaded_at_query,
                freshness,
                external_config,
                quoting,
                patch_path,
                meta: meta
                    .as_ref()
                    .filter(|v| !v.is_null())
                    .map(|v| v.to_string()),
                config: config.as_ref().map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                }),
                deprecation_date,
                version: version.as_ref().map(|v| v.to_string()),
                latest_version: latest_version.as_ref().map(|v| v.to_string()),
                compiled_code,
                compiled_code_hash,
                compiled_path,
                table_role,
                grain,
                grain_declared,
                grain_tested,
                classifiers,
                ingested_at: ts.clone(),
            });
        }

        // Edges from depends_on. Macro dependencies are typed 'macro' so
        // graph traversals over dbt.nodes-joined edges can exclude them
        for parent in &deps {
            let edge_type = if parent.starts_with("macro.") {
                "macro"
            } else {
                "ref"
            };
            edge_rows.push(json!({
                "parent_unique_id": parent,
                "child_unique_id": uid,
                "edge_type": edge_type,
                "ingested_at": ts,
            }));
        }

        // test_metadata for test nodes (from __test_attr__ in payload)
        if rt == "test" {
            let test_attr = payload.get("__test_attr__");
            let tm = test_attr
                .and_then(|a| a.get("test_metadata"))
                .or_else(|| payload.get("test_metadata"));
            if let Some(tm) = tm {
                let attached = test_attr
                    .and_then(|a| a.get("attached_node"))
                    .and_then(|v| v.as_str())
                    .or_else(|| payload.get("attached_node").and_then(|v| v.as_str()))
                    .or_else(|| payload.get("file_key_name").and_then(|v| v.as_str()))
                    .map(|s| s.to_string());
                // severity and fail/warn thresholds live in deprecated_config or config
                let config = payload.get("config");
                let dep_config = payload.get("deprecated_config");
                let severity = dep_config
                    .and_then(|c| c.get("severity"))
                    .or_else(|| config.and_then(|c| c.get("severity")))
                    .and_then(|v| v.as_str());
                // Fusion configs omit unset thresholds; the manifest carries
                // dbt's materialized defaults, so default the same way
                let warn_if = dep_config
                    .and_then(|c| c.get("warn_if"))
                    .or_else(|| config.and_then(|c| c.get("warn_if")))
                    .and_then(|v| v.as_str())
                    .unwrap_or("!= 0");
                let error_if = dep_config
                    .and_then(|c| c.get("error_if"))
                    .or_else(|| config.and_then(|c| c.get("error_if")))
                    .and_then(|v| v.as_str())
                    .unwrap_or("!= 0");
                let fail_calc = dep_config
                    .and_then(|c| c.get("fail_calc"))
                    .or_else(|| config.and_then(|c| c.get("fail_calc")))
                    .and_then(|v| v.as_str())
                    .unwrap_or("count(*)");
                let store_failures = dep_config
                    .and_then(|c| c.get("store_failures"))
                    .and_then(|v| v.as_bool());
                let store_failures_as = dep_config
                    .and_then(|c| c.get("store_failures_as"))
                    .and_then(|v| v.as_str());
                let test_where = dep_config
                    .and_then(|c| c.get("where"))
                    .or_else(|| config.and_then(|c| c.get("where")))
                    .and_then(|v| v.as_str());
                let test_limit = dep_config
                    .and_then(|c| c.get("limit"))
                    .or_else(|| config.and_then(|c| c.get("limit")))
                    .and_then(|v| v.as_i64());
                test_meta_rows.push(json!({
                    "unique_id": uid,
                    "test_name": tm.get("name").and_then(|v| v.as_str()),
                    "test_namespace": tm.get("namespace").and_then(|v| v.as_str()),
                    "kwargs": tm.get("kwargs").map(|v| v.to_string()),
                    "column_name": test_attr.and_then(|a| a.get("column_name")).and_then(|v| v.as_str()),
                    "attached_node": attached,
                    "severity": severity,
                    "warn_if": warn_if,
                    "error_if": error_if,
                    "fail_calc": fail_calc,
                    "store_failures": store_failures,
                    "store_failures_as": store_failures_as,
                    "test_where": test_where,
                    "test_limit": test_limit,
                    "ingested_at": ts,
                }));
            }
        }

        // Special tables by resource_type
        match rt.as_str() {
            "metric" => {
                let node_name = get_str(name_col, i).unwrap_or_default();
                build_metric_from_payload(
                    &uid,
                    &node_name,
                    payload,
                    &tags,
                    enabled,
                    &ts,
                    metric_rows,
                );
            }
            "semantic_model" => {
                build_semantic_model_from_payload(
                    &uid,
                    payload,
                    &ts,
                    semantic_model_rows,
                    semantic_entity_rows,
                    semantic_measure_rows,
                    semantic_dimension_rows,
                );
            }
            "saved_query" => {
                build_saved_query_from_payload(&uid, payload, &ts, saved_query_rows);
            }
            "exposure" => {
                build_exposure_from_payload(&uid, payload, &tags, enabled, &ts, exposure_rows);
            }
            "group" => {
                build_group_from_payload(&uid, payload, &ts, group_rows);
            }
            "macro" => {
                build_macro_from_payload(&uid, payload, &ts, macro_rows);
            }
            "docs_macro" => {
                build_doc_from_payload(&uid, payload, &ts, doc_rows);
            }
            "unit_test" => {
                build_unit_test_from_payload(&uid, payload, enabled, &ts, unit_test_rows);
            }
            "model" => {
                // time_spine: embedded in __model_attr__
                if let Some(ts_val) = payload
                    .get("__model_attr__")
                    .and_then(|m| m.get("time_spine"))
                    .filter(|v| !v.is_null())
                {
                    build_time_spine_from_payload(&uid, ts_val, &ts, time_spine_rows);
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Special table builders — translate payload to output row format
// ---------------------------------------------------------------------------

fn build_metric_from_payload(
    uid: &str,
    node_name: &str,
    payload: &Value,
    tags: &[String],
    enabled: bool,
    now: &str,
    rows: &mut Vec<Value>,
) {
    let m = ParsedMetric::from_payload(uid, payload, tags);
    // m.name comes from __metric_attr__.name which may be absent in the payload;
    // fall back to the name column from parse/nodes (always populated).
    let name = if m.name.is_empty() {
        node_name
    } else {
        &m.name
    };
    rows.push(json!({
        "unique_id": uid,
        "name": name,
        "label": m.label,
        "metric_type": m.metric_type,
        "description": m.description,
        "package_name": m.package_name,
        "file_path": m.file_path,
        "original_file_path": m.original_file_path,
        "fqn": m.fqn,
        "type_params": m.type_params.as_ref().map(|v| v.to_string()),
        "metric_filter": m.metric_filter.as_ref().map(|v| v.to_string()),
        "time_granularity": m.time_granularity,
        "input_metric_names": m.input_metric_names,
        "depends_on_nodes": m.depends_on_nodes,
        "depends_on_macros": m.depends_on_macros,
        "group_name": m.group_name,
        "tags": m.tags,
        "meta": m.meta.as_ref().map(|v| v.to_string()),
        "config": m.config.as_ref().map(|v| v.to_string()),
        "enabled": enabled,
        "created_at": m.created_at,
        "ingested_at": now,
    }));
}

fn build_semantic_model_from_payload(
    uid: &str,
    payload: &Value,
    now: &str,
    sm_rows: &mut Vec<Value>,
    ent_rows: &mut Vec<Value>,
    meas_rows: &mut Vec<Value>,
    dim_rows: &mut Vec<Value>,
) {
    let m = ParsedSemanticModel::from_payload(payload);
    sm_rows.push(json!({
        "unique_id": uid,
        "name": m.name,
        "model": m.model_ref,
        "label": m.label,
        "description": m.description,
        "fqn": m.fqn,
        "node_relation": m.node_relation.as_ref().map(|v| v.to_string()),
        "primary_entity": m.primary_entity,
        "defaults": m.defaults.as_ref().map(|v| v.to_string()),
        "depends_on_nodes": m.depends_on_nodes,
        "depends_on_macros": m.depends_on_macros,
        "group_name": m.group_name,
        "created_at": m.created_at,
        "ingested_at": now,
    }));
    for ent in &m.entities {
        ent_rows.push(json!({
            "unique_id": uid,
            "name": ent.name,
            "entity_type": ent.entity_type,
            "description": ent.description,
            "label": ent.label,
            "entity_role": ent.entity_role,
            "expr": ent.expr,
            "ingested_at": now,
        }));
    }
    for meas in &m.measures {
        meas_rows.push(json!({
            "unique_id": uid,
            "name": meas.name,
            "agg": meas.agg,
            "description": meas.description,
            "label": meas.label,
            "expr": meas.expr,
            "create_metric": meas.create_metric,
            "agg_time_dimension": meas.agg_time_dimension,
            "agg_params": meas.agg_params.as_ref().map(|v| v.to_string()),
            "non_additive_dimension": meas.non_additive_dimension.as_ref().map(|v| v.to_string()),
            "ingested_at": now,
        }));
    }
    for dim in &m.dimensions {
        dim_rows.push(json!({
            "unique_id": uid,
            "name": dim.name,
            "dimension_type": dim.dimension_type,
            "description": dim.description,
            "label": dim.label,
            "expr": dim.expr,
            "is_partition": dim.is_partition,
            "time_granularity": dim.time_granularity,
            "validity_params": dim.validity_params.as_ref().map(|v| v.to_string()),
            "ingested_at": now,
        }));
    }
}

fn build_saved_query_from_payload(uid: &str, payload: &Value, now: &str, rows: &mut Vec<Value>) {
    let m = ParsedSavedQuery::from_payload(payload);
    rows.push(json!({
        "unique_id": uid,
        "name": m.name,
        "label": m.label,
        "description": m.description,
        "package_name": m.package_name,
        "file_path": m.file_path,
        "original_file_path": m.original_file_path,
        "fqn": m.fqn,
        "query_params": m.query_params.as_ref().map(|v| v.to_string()),
        "exports": m.exports.as_ref().map(|v| v.to_string()),
        "depends_on_nodes": m.depends_on_nodes,
        "depends_on_macros": m.depends_on_macros,
        "tags": m.tags,
        "group_name": m.group_name,
        "created_at": m.created_at,
        "ingested_at": now,
    }));
}

fn build_exposure_from_payload(
    uid: &str,
    payload: &Value,
    tags: &[String],
    enabled: bool,
    now: &str,
    rows: &mut Vec<Value>,
) {
    let m = ParsedExposure::from_payload(payload, tags);
    rows.push(json!({
        "unique_id": uid,
        "name": m.name,
        "exposure_type": m.exposure_type,
        "label": m.label,
        "owner_name": m.owner_name,
        "owner_email": m.owner_email,
        "url": m.url,
        "maturity": m.maturity,
        "description": m.description,
        "package_name": m.package_name,
        "file_path": m.file_path,
        "original_file_path": m.original_file_path,
        "fqn": m.fqn,
        "depends_on_nodes": m.depends_on_nodes,
        "depends_on_macros": m.depends_on_macros,
        "tags": m.tags,
        "enabled": enabled,
        "created_at": m.created_at,
        "ingested_at": now,
    }));
}

fn build_group_from_payload(uid: &str, payload: &Value, now: &str, rows: &mut Vec<Value>) {
    let m = ParsedGroup::from_payload(payload);
    rows.push(json!({
        "unique_id": uid,
        "name": m.name,
        "description": m.description,
        "owner_name": m.owner_name,
        "owner_email": m.owner_email,
        "ingested_at": now,
    }));
}

fn build_macro_from_payload(uid: &str, payload: &Value, now: &str, rows: &mut Vec<Value>) {
    let m = ParsedMacro::from_payload(payload);
    rows.push(json!({
        "unique_id": uid,
        "name": m.name,
        "package_name": m.package_name,
        "file_path": m.file_path,
        "original_file_path": m.original_file_path,
        "macro_sql": m.macro_sql.as_deref().unwrap_or(""),
        "description": m.description.as_deref().unwrap_or(""),
        "depends_on_macros": m.depends_on_macros,
        "supported_languages": m.supported_languages,
        "arguments": m.arguments.as_ref().map(|v| v.to_string()),
        "docs_show": m.docs_show,
        "patch_path": m.patch_path,
        "meta": m.meta.as_ref().map(|v| v.to_string()),
        "created_at": m.created_at,
        "ingested_at": now,
    }));
}

fn build_doc_from_payload(uid: &str, payload: &Value, now: &str, rows: &mut Vec<Value>) {
    let m = ParsedDoc::from_payload(payload);
    rows.push(json!({
        "unique_id": uid,
        "name": m.name,
        "package_name": m.package_name,
        "file_path": m.file_path,
        "original_file_path": m.original_file_path,
        "block_contents": m.block_contents,
        "ingested_at": now,
    }));
}

fn build_unit_test_from_payload(
    uid: &str,
    payload: &Value,
    enabled: bool,
    now: &str,
    rows: &mut Vec<Value>,
) {
    let m = ParsedUnitTest::from_payload(payload);
    rows.push(json!({
        "unique_id": uid,
        "name": m.name,
        "model": m.model,
        "description": m.description,
        "package_name": m.package_name,
        "file_path": m.file_path,
        "original_file_path": m.original_file_path,
        "fqn": m.fqn,
        "given": m.given.as_ref().map(|v| v.to_string()),
        "expect": m.expect.as_ref().map(|v| v.to_string()),
        "overrides": m.overrides.as_ref().map(|v| v.to_string()),
        "versions": m.versions.as_ref().map(|v| v.to_string()),
        "version": m.version,
        "schema_name": m.schema_name,
        "depends_on_nodes": m.depends_on_nodes,
        "depends_on_macros": m.depends_on_macros,
        "enabled": enabled,
        "ingested_at": now,
    }));
}

fn build_time_spine_from_payload(uid: &str, ts_val: &Value, now: &str, rows: &mut Vec<Value>) {
    let m = ParsedTimeSpine::from_payload(ts_val);
    rows.push(json!({
        "unique_id": uid,
        "primary_column": m.primary_column,
        "primary_granularity": m.primary_granularity,
        "custom_granularities": m.custom_granularities.as_ref().map(|v| v.to_string()),
        "node_relation": m.node_relation.as_ref().map(|v| v.to_string()),
        "ingested_at": now,
    }));
}

// ---------------------------------------------------------------------------
// Parse columns → dbt.node_columns (declared_type, description)
// ---------------------------------------------------------------------------

fn write_parse_columns(
    writer: &mut IndexWriter,
    metadata_dir: &Path,
    state: &mut IngestState,
    now: &str,
    full: bool,
    alive_ids: Option<&HashSet<String>>,
) -> Result<usize, IndexError> {
    let dir = metadata_dir.join(PARSE_COLUMNS_SUBDIR);
    if !dir.exists() {
        return Ok(0);
    }

    let epochs = epoch_layers::existing_epochs(&dir);
    let curr_max_ep = epochs.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let need_full = state.needs_full_reload(PARSE_COLUMNS_SUBDIR, &dir, curr_max_ep, full);
    let new_epochs: Vec<_> = if need_full {
        epochs
    } else {
        let last = state.last_epoch_for(PARSE_COLUMNS_SUBDIR);
        epochs.into_iter().filter(|(n, _)| *n > last).collect()
    };
    if new_epochs.is_empty() {
        return Ok(0);
    }

    let mut rows: Vec<Value> = Vec::new();
    // Newest epoch wins wholesale per node — an epoch republishes a node's whole
    // column set, so walk newest-first and skip a node a later epoch already
    // covered. The same rule as `dedup_epoch_groups` for the other two column
    // sources, and what the delta path's `MergePrune` does to the rows already in
    // the table; without it a cold ingest over two parse epochs emits every column
    // of an unchanged node twice.
    //
    // Marked seen per *epoch* rather than per row: a node's columns can span
    // batches within one file, and marking on the first row would keep one column.
    let mut seen_ids: HashSet<String> = HashSet::new();
    let last_epoch = new_epochs.last().map(|(n, _)| *n).unwrap_or(0);
    for (_epoch_num, path) in new_epochs.iter().rev() {
        let batches = read_parquet_batches(path)?;
        let mut epoch_ids: HashSet<String> = HashSet::new();
        for batch in &batches {
            let uid_col = str_col(batch, "unique_id");
            let col_col = str_col(batch, "column_name");
            let type_col = str_col(batch, "declared_type");
            let desc_col = str_col(batch, "description");
            let tags_col = list_col_batch(batch, "tags");
            let ts_col = i64_col(batch, "ingested_at");
            for i in 0..batch.num_rows() {
                let Some(uid) = get_str(uid_col, i) else {
                    continue;
                };
                if seen_ids.contains(&uid) {
                    continue;
                }
                epoch_ids.insert(uid.clone());
                let ts = get_i64(ts_col, i)
                    .map(micros_to_ts)
                    .unwrap_or_else(|| now.to_string());
                let tags = get_list(tags_col, i);
                rows.push(json!({
                    "unique_id": uid,
                    "column_name": get_str(col_col, i).unwrap_or_default(),
                    "declared_type": get_str(type_col, i),
                    "description": get_str(desc_col, i),
                    "tags": tags,
                    // Placeholder; the real (declared ∪ propagated) classifiers
                    // arrive via the compile/columns merge.
                    "classifiers": Vec::<String>::new(),
                    "ingested_at": ts,
                }));
            }
        }
        seen_ids.extend(epoch_ids);
    }

    if need_full {
        prune_by_alive(&mut rows, alive_ids);
    }
    let count = rows.len();
    if need_full {
        writer.write_dbt_table("node_columns", rows)?;
    } else {
        // Latest-wins per node: replace all columns for affected unique_ids.
        let changed_ids: HashSet<String> = rows
            .iter()
            .filter_map(|r| {
                r.get("unique_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        writer.write_dbt_table_merged(
            "node_columns",
            rows,
            WriteMode::MergePrune {
                key_col: "unique_id",
                valid_ids: &changed_ids,
            },
        )?;
    }
    state.set_epoch(PARSE_COLUMNS_SUBDIR, last_epoch);
    Ok(count)
}

// ---------------------------------------------------------------------------
// Parse project → dbt.project + dbt.project_vars + dbt.project_env_vars + dbt.packages
// ---------------------------------------------------------------------------

/// Packages dbt installs itself, which no one writes `vars` for: the adapter's
/// dispatch package, its parent where one exists, and `dbt` itself.
fn internal_packages(adapter_type: &str) -> Vec<String> {
    let mut v = vec!["dbt".to_string()];
    if !adapter_type.is_empty() {
        v.push(format!("dbt_{adapter_type}"));
        match adapter_type {
            "redshift" => v.push("dbt_postgres".to_string()),
            "databricks" => v.push("dbt_spark".to_string()),
            _ => {}
        }
    }
    v
}

/// A string variable reads as itself; anything else as the JSON it was written
/// as. `Value::to_string` would quote the string.
fn render_var_value(value: Value) -> String {
    match value {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

/// `dbt.project_vars` rows: one per (project, variable).
///
/// `vars_json` is `{package: {var: value}}` and each package's map is already
/// *resolved for that package* -- a dependency's map carries the value the
/// dependency sees, root default or scoped override -- so there is nothing to
/// un-nest here. Walking each package's own map is the whole job; scoping has
/// already happened upstream.
///
/// Two kinds of key are not variables:
///
/// - a package nobody sets vars for. `dbt` and `dbt_<adapter>` are installed by
///   dbt itself, and their maps are a copy of the root's with no one to read them.
/// - inside a map, a nested object whose key names an installed package. That is
///   the scope *declaration* (`vars: {dbt_utils: {...}}`), not a variable of the
///   enclosing package, and the scope's effect is already in `dbt_utils`' own map.
///   A nested object whose key is *not* installed is an ordinary object-valued
///   variable, which dbt allows, so it stays.
fn project_var_rows(
    vars_json: Option<&str>,
    installed: &HashSet<String>,
    adapter_type: &str,
    now: &str,
) -> Vec<Value> {
    let internal = internal_packages(adapter_type);
    vars_json
        .and_then(|s| serde_json::from_str::<serde_json::Map<String, Value>>(s).ok())
        .map(|obj| {
            obj.into_iter()
                .filter(|(package, _)| !internal.iter().any(|i| i == package))
                .flat_map(|(package, vars)| {
                    let vars = match vars {
                        Value::Object(map) => map,
                        _ => serde_json::Map::new(),
                    };
                    vars.into_iter()
                        .filter(|(name, value)| !(value.is_object() && installed.contains(name)))
                        .map(|(name, value)| {
                            json!({
                                "project_name": package,
                                "var_name": name,
                                "var_value": render_var_value(value),
                                "ingested_at": now,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .collect()
        })
        .unwrap_or_default()
}

#[allow(clippy::cognitive_complexity)]
fn write_parse_project(
    writer: &mut IndexWriter,
    metadata_dir: &Path,
    now: &str,
) -> Result<usize, IndexError> {
    use arrow_array::Array;

    // ── Primary: generation.parquet — cold-start-stable project metadata (#10623) ──
    // project_name, adapter_type, dbt_version, vars_json, env_vars_json are written
    // only on cold start (their corresponding hash fields in GenerationRow all gate
    // cache invalidation), so they belong here rather than in resolver_state.parquet.
    let mut project_name = String::new();
    let mut adapter_type = String::new();
    let mut dbt_version = String::new();
    let mut vars_json: Option<String> = None;
    let mut env_vars_json: Option<String> = None;

    macro_rules! read_str_opt {
        ($batch:expr, $field:expr, $target:expr) => {
            if $target.is_none() {
                if let Some(col) = str_col($batch, $field) {
                    if !col.is_empty() && !col.is_null(0) && !col.value(0).is_empty() {
                        $target = Some(col.value(0).to_string());
                    }
                }
            }
        };
    }

    let gen_path = metadata_dir.join(PARSE_GENERATION);
    if gen_path.exists() {
        let batches = read_parquet_batches(&gen_path)?;
        for batch in &batches {
            if project_name.is_empty() {
                if let Some(col) = str_col(batch, "project_name") {
                    if !col.is_empty() && !col.is_null(0) && !col.value(0).is_empty() {
                        project_name = col.value(0).to_string();
                    }
                }
            }
            if adapter_type.is_empty() {
                if let Some(col) = str_col(batch, "adapter_type") {
                    if !col.is_empty() && !col.is_null(0) && !col.value(0).is_empty() {
                        adapter_type = col.value(0).to_string();
                    }
                }
            }
            if dbt_version.is_empty() {
                if let Some(col) = str_col(batch, "dbt_version") {
                    if !col.is_empty() && !col.is_null(0) && !col.value(0).is_empty() {
                        dbt_version = col.value(0).to_string();
                    }
                }
            }
            read_str_opt!(batch, "vars_json", vars_json);
            read_str_opt!(batch, "env_vars_json", env_vars_json);
        }
    }

    // ── Legacy fallback: parse/project.parquet (retired KV store, pre-#10600) ──
    if project_name.is_empty() {
        let legacy_path = metadata_dir.join(PARSE_PROJECT);
        if legacy_path.exists() {
            let batches = read_parquet_batches(&legacy_path)?;
            let mut kv: HashMap<String, String> = HashMap::new();
            for batch in &batches {
                let key_col = str_col(batch, "key");
                let val_col = str_col(batch, "value");
                if let (Some(keys), Some(vals)) = (key_col, val_col) {
                    for i in 0..batch.num_rows() {
                        if !keys.is_null(i) && !vals.is_null(i) {
                            kv.insert(keys.value(i).to_string(), vals.value(i).to_string());
                        }
                    }
                }
            }
            project_name = kv.get("project_name").cloned().unwrap_or_default();
            if adapter_type.is_empty() {
                adapter_type = kv.get("adapter_type").cloned().unwrap_or_default();
            }
            if dbt_version.is_empty() {
                dbt_version = kv.get("dbt_version").cloned().unwrap_or_default();
            }
        }
    }

    if project_name.is_empty() {
        return Ok(0);
    }

    // ── resolver_state.parquet: git fields (updated every parse) ─────────────
    let mut git_sha: Option<String> = None;
    let mut git_branch: Option<String> = None;
    let mut git_is_dirty: Option<bool> = None;
    let mut pkg_kinds_json: Option<String> = None;

    let rs_path = metadata_dir.join(PARSE_RESOLVER_STATE);
    if rs_path.exists() {
        let batches = read_parquet_batches(&rs_path)?;
        for batch in &batches {
            // vars_json / env_vars_json: fall back to resolver_state for metadata
            // dirs written before generation.parquet gained these fields (#10623).
            read_str_opt!(batch, "vars_json", vars_json);
            read_str_opt!(batch, "env_vars_json", env_vars_json);
            read_str_opt!(batch, "pkg_kinds_json", pkg_kinds_json);
            read_str_opt!(batch, "git_sha", git_sha);
            read_str_opt!(batch, "git_branch", git_branch);
            if git_is_dirty.is_none() {
                if let Some(col) = i32_col(batch, "git_is_dirty") {
                    if !col.is_empty() && !col.is_null(0) {
                        git_is_dirty = Some(col.value(0) != 0);
                    }
                }
            }
        }
    }

    writer.write_dbt_table(
        "project",
        vec![json!({
            "project_name": project_name,
            "dbt_version": dbt_version,
            "adapter_type": adapter_type,
            "git_sha": git_sha,
            "git_branch": git_branch,
            "git_is_dirty": git_is_dirty,
            "ingested_at": now,
        })],
    )?;

    let installed: HashSet<String> = pkg_kinds_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Map<String, Value>>(s).ok())
        .map(|obj| obj.into_iter().map(|(k, _)| k).collect())
        .unwrap_or_default();
    let var_rows = project_var_rows(vars_json.as_deref(), &installed, &adapter_type, now);
    writer.write_dbt_table("project_vars", var_rows)?;

    // project_env_vars
    let env_rows: Vec<Value> = env_vars_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Map<String, Value>>(s).ok())
        .map(|obj| {
            obj.into_iter()
                .map(|(k, v)| {
                    let used_in: Vec<String> = v
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|s| s.as_str())
                                .map(|s| s.to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    json!({
                        "env_var_name": k,
                        "used_in": used_in,
                        "ingested_at": now,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    writer.write_dbt_table("project_env_vars", env_rows)?;

    // One row per installed package. `pkg_kinds_json` is `{package: path-kinds}`;
    // the other package columns have no epoch source yet.
    let pkg_rows: Vec<Value> = pkg_kinds_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Map<String, Value>>(s).ok())
        .map(|obj| {
            obj.into_iter()
                .map(|(package_name, _)| {
                    json!({
                        "package_name": package_name,
                        "ingested_at": now,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    writer.write_dbt_table("packages", pkg_rows)?;

    Ok(1)
}

// ---------------------------------------------------------------------------
// Parse generation → dbt.generation
// ---------------------------------------------------------------------------

fn write_parse_generation(
    writer: &mut IndexWriter,
    metadata_dir: &Path,
    now: &str,
) -> Result<usize, IndexError> {
    let path = metadata_dir.join(PARSE_GENERATION);
    if !path.exists() {
        return Ok(0);
    }

    let batches = read_parquet_batches(&path)?;
    for batch in &batches {
        let ts_col = i64_col(batch, "ingested_at");
        if let Some(col) = ts_col {
            use arrow_array::Array;
            if !col.is_empty() && !col.is_null(0) {
                let ts = micros_to_ts(col.value(0));
                writer.write_dbt_table("generation", vec![json!({ "ingested_at": ts })])?;
                return Ok(1);
            }
        }
    }

    // generation.parquet exists but has no rows — treat as absent.
    // Write a sentinel using now so dbt.generation is never empty.
    writer.write_dbt_table("generation", vec![json!({ "ingested_at": now })])?;
    Ok(1)
}

// ---------------------------------------------------------------------------
// Compile columns → dbt.node_columns (inferred_type, column_index)
// Fast path: Arrow→Arrow projection, no serde_json round-trip.
// ---------------------------------------------------------------------------

/// Newest-epoch-wins rows from `epochs`, grouped by `key_col`, alive-pruned.
///
/// The rule every per-node-republished source shares: a whole group is written by
/// one epoch, so a later epoch supersedes an earlier one for that entire group.
/// Read newest-first and keep every row of a group not already seen. Used by both
/// column sources (`compile/columns`, `catalog/columns`) keyed on `unique_id`,
/// where the group is a node's column set, and by `compile/column_lineage` keyed
/// on `to_node_unique_id`, where it is a target's incoming edges.
///
/// The dedup is per *group*, not per row, so a group is only marked seen once the
/// whole batch has been walked — marking it on its first row would keep just the
/// first row of each group and drop the rest. Alive pruning is folded into the
/// same pass, so a row costs one hash lookup in total.
///
/// `Supersede::LatestGroup` is the view layer's spelling of this.
fn dedup_epoch_groups(
    epochs: &[(u32, std::path::PathBuf)],
    key_col: &str,
    alive_ids: Option<&HashSet<String>>,
) -> Result<Vec<arrow_array::RecordBatch>, IndexError> {
    use arrow_array::Array;
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut out: Vec<arrow_array::RecordBatch> = Vec::new();
    for (_epoch_num, path) in epochs.iter().rev() {
        for batch in read_parquet_batches(path)? {
            let uid_col = str_col(&batch, key_col);
            let mut keep = Vec::with_capacity(batch.num_rows());
            let mut any_keep = false;
            let mut batch_uids: HashSet<String> = HashSet::new();
            for i in 0..batch.num_rows() {
                let Some(uid) = uid_col.filter(|c| !c.is_null(i)).map(|c| c.value(i)) else {
                    keep.push(false);
                    continue;
                };
                let dead = alive_ids.map(|ids| !ids.contains(uid)).unwrap_or(false);
                if dead || seen_ids.contains(uid) {
                    keep.push(false);
                    continue;
                }
                batch_uids.insert(uid.to_string());
                keep.push(true);
                any_keep = true;
            }
            seen_ids.extend(batch_uids);
            if !any_keep {
                continue;
            }
            let mask = arrow_array::BooleanArray::from(keep);
            if let Ok(filtered) = arrow_select::filter::filter_record_batch(&batch, &mask) {
                out.push(filtered);
            }
        }
    }
    Ok(out)
}

fn write_compile_columns(
    writer: &mut IndexWriter,
    metadata_dir: &Path,
    state: &mut IngestState,
    _now: &str,
    full: bool,
    alive_ids: Option<&HashSet<String>>,
) -> Result<usize, IndexError> {
    let dir = metadata_dir.join(COMPILE_COLUMNS_SUBDIR);
    if !dir.exists() {
        return Ok(0);
    }

    let epochs = epoch_layers::existing_epochs(&dir);
    let curr_max_ep = epochs.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let need_full = state.needs_full_reload(COMPILE_COLUMNS_SUBDIR, &dir, curr_max_ep, full);
    let new_epochs: Vec<_> = if need_full {
        epochs
    } else {
        let last = state.last_epoch_for(COMPILE_COLUMNS_SUBDIR);
        epochs.into_iter().filter(|(n, _)| *n > last).collect()
    };
    if new_epochs.is_empty() {
        return Ok(0);
    }

    let out_schema = crate::parquet::schema_for("node_columns");
    let batches = dedup_epoch_groups(&new_epochs, "unique_id", alive_ids)?;

    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    if total == 0 {
        let last = new_epochs.last().map(|(n, _)| *n).unwrap_or(0);
        state.set_epoch(COMPILE_COLUMNS_SUBDIR, last);
        return Ok(0);
    }

    let projected = project_compile_columns_batches(batches, &out_schema)?;

    writer.merge_compile_columns_into_node_columns(projected)?;

    let last = new_epochs.last().map(|(n, _)| *n).unwrap_or(0);
    state.set_epoch(COMPILE_COLUMNS_SUBDIR, last);
    Ok(total)
}

// ---------------------------------------------------------------------------
// catalog/columns — warehouse-fetched column types and comments
// ---------------------------------------------------------------------------

fn write_catalog_columns(
    writer: &mut IndexWriter,
    metadata_dir: &Path,
    state: &mut IngestState,
    _now: &str,
    full: bool,
    alive_ids: Option<&HashSet<String>>,
) -> Result<usize, IndexError> {
    let dir = metadata_dir.join(CATALOG_COLUMNS_SUBDIR);
    if !dir.exists() {
        return Ok(0);
    }

    let epochs = epoch_layers::existing_epochs(&dir);
    let last_saved = state.last_epoch_for(CATALOG_COLUMNS_SUBDIR);
    let curr_max_ep = epochs.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let need_full = state.needs_full_reload(CATALOG_COLUMNS_SUBDIR, &dir, curr_max_ep, full);
    let new_epochs: Vec<_> = if need_full {
        epochs
    } else {
        epochs
            .into_iter()
            .filter(|(n, _)| *n > last_saved)
            .collect()
    };
    if new_epochs.is_empty() {
        return Ok(0);
    }

    let out_schema = crate::parquet::schema_for("node_columns");
    let batches = dedup_epoch_groups(&new_epochs, "unique_id", alive_ids)?;

    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    if total == 0 {
        let last = new_epochs.last().map(|(n, _)| *n).unwrap_or(0);
        state.set_epoch(CATALOG_COLUMNS_SUBDIR, last);
        return Ok(0);
    }

    let projected = project_catalog_columns_batches(batches, &out_schema)?;

    writer.merge_catalog_columns_into_node_columns(projected)?;

    let last = new_epochs.last().map(|(n, _)| *n).unwrap_or(0);
    state.set_epoch(CATALOG_COLUMNS_SUBDIR, last);
    Ok(total)
}

fn project_catalog_columns_batches(
    batches: Vec<arrow_array::RecordBatch>,
    out_schema: &arrow_schema::SchemaRef,
) -> Result<Vec<arrow_array::RecordBatch>, IndexError> {
    use arrow_array::{Int64Array, new_null_array};
    use arrow_schema::DataType;
    use std::sync::Arc;

    batches
        .into_iter()
        .map(|batch| {
            let nrows = batch.num_rows();

            let uid_out = col_or_null(&batch, "unique_id", &DataType::Utf8, nrows);
            let cname_out = col_or_null(&batch, "column_name", &DataType::Utf8, nrows);
            let catalog_type_out = col_or_null(&batch, "catalog_type", &DataType::Utf8, nrows);
            let catalog_comment_out =
                col_or_null(&batch, "catalog_comment", &DataType::Utf8, nrows);

            // column_index: cast i32 → i64
            let idx_out: arrow_array::ArrayRef =
                if let Ok(ki) = batch.schema().index_of("column_index") {
                    let col = batch.column(ki);
                    arrow_cast::cast_with_options(
                        col,
                        &DataType::Int64,
                        &arrow_cast::CastOptions::default(),
                    )
                    .map(|a| a as arrow_array::ArrayRef)
                    .unwrap_or_else(|_| Arc::new(Int64Array::from(vec![None::<i64>; nrows])))
                } else {
                    Arc::new(Int64Array::from(vec![None::<i64>; nrows]))
                };

            // ingested_at: pass through from input
            let ingested_at: arrow_array::ArrayRef = batch
                .schema()
                .index_of("ingested_at")
                .ok()
                .map(|i| batch.column(i).clone())
                .unwrap_or_else(|| {
                    use arrow_array::TimestampMicrosecondArray;
                    let now_micros = chrono::Utc::now().timestamp_micros();
                    Arc::new(
                        TimestampMicrosecondArray::from(vec![now_micros; nrows])
                            .with_timezone("UTC"),
                    )
                });

            let null_str = new_null_array(&DataType::Utf8, nrows);
            let null_bool = new_null_array(&DataType::Boolean, nrows);
            let null_list = empty_list_array(nrows);
            // classifiers: catalog columns carry none; the catalog merge into
            // node_columns preserves any classifiers already set by the compile
            // merge (it modifies existing rows in place).
            let classifiers_null = empty_list_array(nrows);

            let columns: Vec<arrow_array::ArrayRef> = vec![
                uid_out,                  // unique_id
                cname_out,                // column_name
                idx_out,                  // column_index
                null_str.clone(),         // declared_type
                null_str.clone(),         // inferred_type
                catalog_type_out.clone(), // catalog_type
                catalog_type_out, // data_type (initially set to catalog_type, merge refines)
                null_str.clone(), // description
                null_str.clone(), // label
                null_str.clone(), // expression
                null_bool,        // quote
                null_str.clone(), // granularity
                null_list,        // tags
                classifiers_null, // classifiers
                null_str.clone(), // meta
                null_str.clone(), // column_constraints
                null_str.clone(), // tests
                catalog_comment_out, // catalog_comment
                ingested_at,      // ingested_at
            ];

            arrow_array::RecordBatch::try_new(out_schema.clone(), columns)
                .map_err(|e| IndexError::Other(format!("project_catalog_columns: {e}")))
        })
        .collect()
}

/// Project CLL input batches to the column_lineage output schema.
/// Input columns map 1:1 — just reorder/rename and fill ingested_at.
fn project_cll_batches(
    batches: Vec<arrow_array::RecordBatch>,
    out_schema: &arrow_schema::SchemaRef,
    now: &str,
) -> Result<Vec<arrow_array::RecordBatch>, IndexError> {
    use arrow_array::{StringArray, new_null_array};
    use arrow_schema::DataType;
    use std::sync::Arc;

    batches
        .into_iter()
        .map(|batch| {
            let nrows = batch.num_rows();

            let from_n = col_or_null(&batch, "from_node_unique_id", &DataType::Utf8, nrows);
            let from_c = col_or_null(&batch, "from_column_name", &DataType::Utf8, nrows);
            let to_n = col_or_null(&batch, "to_node_unique_id", &DataType::Utf8, nrows);
            let to_c = col_or_null(&batch, "to_column_name", &DataType::Utf8, nrows);
            let kind = col_or_null(&batch, "lineage_kind", &DataType::Utf8, nrows);

            // ingested_at: use existing timestamp column if present, else fill with now.
            let ingested_at: arrow_array::ArrayRef = {
                let ts_field = out_schema.field_with_name("ingested_at").ok();
                let ts_type = ts_field
                    .map(|f| f.data_type().clone())
                    .unwrap_or_else(|| DataType::Utf8);
                batch
                    .schema()
                    .index_of("ingested_at")
                    .ok()
                    .map(|i| {
                        let col = batch.column(i);
                        if col.data_type() == &ts_type {
                            col.clone()
                        } else {
                            new_null_array(&ts_type, nrows)
                        }
                    })
                    .unwrap_or_else(|| {
                        // Fill with current timestamp string — write_dbt_table will overwrite it.
                        Arc::new(StringArray::from(vec![now; nrows])) as arrow_array::ArrayRef
                    })
            };

            // Output column order must match schema_column_lineage field order:
            //   from_node_unique_id, from_column_name, to_node_unique_id, to_column_name,
            //   lineage_kind, ingested_at
            let columns: Vec<arrow_array::ArrayRef> =
                vec![from_n, from_c, to_n, to_c, kind, ingested_at];

            arrow_array::RecordBatch::try_new(out_schema.clone(), columns)
                .map_err(|e| IndexError::Other(format!("project_cll_batches: {e}")))
        })
        .collect()
}

/// Project compile/columns input batches to the node_columns output schema.
/// Maps: unique_id, column_name, column_index(i32→i64 cast), column_type→inferred_type,
/// description; all other output columns are null-filled.
fn project_compile_columns_batches(
    batches: Vec<arrow_array::RecordBatch>,
    out_schema: &arrow_schema::SchemaRef,
) -> Result<Vec<arrow_array::RecordBatch>, IndexError> {
    use arrow_array::{Int64Array, new_null_array};
    use arrow_schema::DataType;
    use std::sync::Arc;

    batches
        .into_iter()
        .map(|batch| {
            let nrows = batch.num_rows();

            // Reuse input columns zero-copy when types match.
            let uid_out = col_or_null(&batch, "unique_id", &DataType::Utf8, nrows);
            let cname_out = col_or_null(&batch, "column_name", &DataType::Utf8, nrows);
            let itype_out = col_or_null(&batch, "column_type", &DataType::Utf8, nrows);
            let desc_out = col_or_null(&batch, "description", &DataType::Utf8, nrows);

            // column_index: cast i32 → i64 (avoids per-row alloc).
            let idx_out: arrow_array::ArrayRef =
                if let Ok(ki) = batch.schema().index_of("column_index") {
                    let col = batch.column(ki);
                    arrow_cast::cast_with_options(
                        col,
                        &DataType::Int64,
                        &arrow_cast::CastOptions::default(),
                    )
                    .map(|a| a as arrow_array::ArrayRef)
                    .unwrap_or_else(|_| Arc::new(Int64Array::from(vec![None::<i64>; nrows])))
                } else {
                    Arc::new(Int64Array::from(vec![None::<i64>; nrows]))
                };

            // ingested_at: pass through from input (already TimestampMicrosecond UTC).
            let ingested_at: arrow_array::ArrayRef = batch
                .schema()
                .index_of("ingested_at")
                .ok()
                .map(|i| batch.column(i).clone())
                .unwrap_or_else(|| {
                    use arrow_array::TimestampMicrosecondArray;
                    let now_micros = chrono::Utc::now().timestamp_micros();
                    Arc::new(
                        TimestampMicrosecondArray::from(vec![now_micros; nrows])
                            .with_timezone("UTC"),
                    )
                });

            // All-null columns: use new_null_array — O(1) allocation, just a validity bitmap.
            let null_str = new_null_array(&DataType::Utf8, nrows);
            let null_bool = new_null_array(&DataType::Boolean, nrows);
            // tags: List<Utf8> non-nullable — all-zero offsets, empty values buffer.
            let null_list = empty_list_array(nrows);
            // classifiers: carry the real List<Utf8> from the compile/columns batch.
            let classifiers_out: arrow_array::ArrayRef = batch
                .schema()
                .index_of("classifiers")
                .ok()
                .map(|i| batch.column(i).clone())
                .unwrap_or_else(|| empty_list_array(nrows));

            let columns: Vec<arrow_array::ArrayRef> = vec![
                uid_out,          // unique_id
                cname_out,        // column_name
                idx_out,          // column_index
                null_str.clone(), // declared_type
                itype_out,        // inferred_type
                null_str.clone(), // catalog_type
                null_str.clone(), // data_type
                desc_out,         // description
                null_str.clone(), // label
                null_str.clone(), // expression
                null_bool,        // quote
                null_str.clone(), // granularity
                null_list,        // tags
                classifiers_out,  // classifiers
                null_str.clone(), // meta
                null_str.clone(), // column_constraints
                null_str.clone(), // tests
                null_str.clone(), // catalog_comment
                ingested_at,      // ingested_at
            ];

            arrow_array::RecordBatch::try_new(out_schema.clone(), columns)
                .map_err(|e| IndexError::Other(format!("project_compile_columns: {e}")))
        })
        .collect()
}

/// Return a column from `batch` by name if it matches `expected_type`, else a null array.
fn col_or_null(
    batch: &arrow_array::RecordBatch,
    name: &str,
    expected_type: &arrow_schema::DataType,
    nrows: usize,
) -> arrow_array::ArrayRef {
    use arrow_array::new_null_array;
    if let Ok(idx) = batch.schema().index_of(name) {
        let col = batch.column(idx);
        if col.data_type() == expected_type {
            return col.clone();
        }
    }
    new_null_array(expected_type, nrows)
}

/// Build an all-empty (non-null) List<Utf8> array of length `n`.
/// Offsets are all-zero (every list has length 0); values buffer is empty.
/// O(n) for the offsets allocation but zero per-row work — much faster than ListBuilder.
fn empty_list_array(n: usize) -> arrow_array::ArrayRef {
    use arrow_array::{ListArray, StringArray};
    use arrow_buffer::OffsetBuffer;
    use arrow_schema::{DataType, Field};
    use std::sync::Arc;

    // n+1 zero offsets: every list starts and ends at position 0 (empty).
    let offsets: arrow_buffer::ScalarBuffer<i32> = vec![0i32; n + 1].into();
    let offsets = OffsetBuffer::new(offsets);
    let values = Arc::new(StringArray::from(Vec::<Option<&str>>::new())) as arrow_array::ArrayRef;
    let field = Arc::new(Field::new("item", DataType::Utf8, true));
    Arc::new(ListArray::new(field, offsets, values, None))
}

// ---------------------------------------------------------------------------
// Compile column lineage → dbt.column_lineage
// ---------------------------------------------------------------------------

fn write_compile_cll(
    writer: &mut IndexWriter,
    metadata_dir: &Path,
    state: &mut IngestState,
    now: &str,
    full: bool,
    alive_ids: Option<&HashSet<String>>,
) -> Result<usize, IndexError> {
    let dir = metadata_dir.join(COMPILE_CLL_SUBDIR);
    if !dir.exists() {
        return Ok(0);
    }

    let epochs = epoch_layers::existing_epochs(&dir);
    let curr_max_ep = epochs.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let need_full = state.needs_full_reload(COMPILE_CLL_SUBDIR, &dir, curr_max_ep, full);
    let new_epochs: Vec<_> = if need_full {
        epochs
    } else {
        let last = state.last_epoch_for(COMPILE_CLL_SUBDIR);
        epochs.into_iter().filter(|(n, _)| *n > last).collect()
    };
    if new_epochs.is_empty() {
        return Ok(0);
    }

    // Newest-epoch-wins per *target* node, which is the unit a compile epoch
    // republishes: `WriteMode::ReplaceColumnLineage` below deletes every edge into
    // a recomputed target before inserting the new set, so a full ingest that
    // merely unioned the epochs would publish an edge a later compile removed —
    // and duplicate every edge it kept. Alive pruning only on full ingest, as
    // before: a delta run's rows are all newly recomputed.
    let batches = dedup_epoch_groups(
        &new_epochs,
        "to_node_unique_id",
        if need_full { alive_ids } else { None },
    )?;

    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    let last = new_epochs.last().map(|(n, _)| *n).unwrap_or(0);

    if total == 0 {
        state.set_epoch(COMPILE_CLL_SUBDIR, last);
        return Ok(0);
    }

    // Project to output schema and write as Arrow — avoids JSON round-trip.
    let out_schema = crate::parquet::schema_for("column_lineage");
    let projected = project_cll_batches(batches, &out_schema, now)?;

    if need_full {
        writer.write_arrow_batches("column_lineage", projected)?;
    } else {
        // Delta: collect recomputed target ids for merge.
        let mut recomputed_targets: HashSet<String> = HashSet::new();
        for b in &projected {
            if let Some(col) = str_col(b, "to_node_unique_id") {
                use arrow_array::Array;
                for i in 0..b.num_rows() {
                    if !col.is_null(i) {
                        recomputed_targets.insert(col.value(i).to_string());
                    }
                }
            }
        }
        // Fall back to JSON merge for delta (rare path, low row count).
        let mut rows: Vec<Value> = Vec::new();
        for b in projected {
            let from_n_col = str_col(&b, "from_node_unique_id");
            let from_c_col = str_col(&b, "from_column_name");
            let to_n_col = str_col(&b, "to_node_unique_id");
            let to_c_col = str_col(&b, "to_column_name");
            let kind_col = str_col(&b, "lineage_kind");
            let ts_col_b = str_col(&b, "ingested_at");
            for i in 0..b.num_rows() {
                let Some(from_n) = get_str(from_n_col, i) else {
                    continue;
                };
                let Some(to_n) = get_str(to_n_col, i) else {
                    continue;
                };
                rows.push(json!({
                    "from_node_unique_id": from_n,
                    "from_column_name":    get_str(from_c_col, i).unwrap_or_default(),
                    "to_node_unique_id":   to_n,
                    "to_column_name":      get_str(to_c_col, i).unwrap_or_default(),
                    "lineage_kind":        get_str(kind_col, i),
                    "ingested_at":         get_str(ts_col_b, i).unwrap_or_else(|| now.to_string()),
                }));
            }
        }
        writer.write_dbt_table_merged(
            "column_lineage",
            rows,
            WriteMode::ReplaceColumnLineage {
                recomputed_targets: &recomputed_targets,
                valid_node_ids: alive_ids,
            },
        )?;
    }
    state.set_epoch(COMPILE_CLL_SUBDIR, last);
    Ok(total)
}

// ---------------------------------------------------------------------------
// Run invocations → dbt_rt.invocations
// ---------------------------------------------------------------------------

fn write_run_invocations(
    writer: &mut IndexWriter,
    metadata_dir: &Path,
    state: &mut IngestState,
    _now: &str,
    full: bool,
) -> Result<usize, IndexError> {
    #[derive(serde::Serialize)]
    struct InvocationOutputRow {
        invocation_id: String,
        command: Option<String>,
        selector: Option<String>,
        dbt_version: String,
        generated_at: Option<chrono::DateTime<chrono::Utc>>,
        elapsed_time: Option<f64>,
        args: Option<String>,
        node_count: Option<i64>,
        target_name: Option<String>,
        target_type: Option<String>,
        target_database: Option<String>,
        target_schema: Option<String>,
        target_threads: Option<i64>,
        vars_override: Option<String>,
        git_sha: Option<String>,
        git_branch: Option<String>,
        git_is_dirty: Option<bool>,
    }

    let dir = metadata_dir.join(RUN_INVOCATIONS_SUBDIR);
    if !dir.exists() {
        return Ok(0);
    }

    let epochs = epoch_layers::existing_epochs(&dir);
    let last_saved = state.last_epoch_for(RUN_INVOCATIONS_SUBDIR);
    let curr_max_ep = epochs.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let need_full = full || last_saved == u32::MAX || curr_max_ep < last_saved;
    let has_new_epochs = need_full || epochs.iter().any(|(n, _)| *n > last_saved);
    if !has_new_epochs {
        return Ok(0);
    }

    let mut rows = Vec::new();
    let mut last_epoch = 0u32;
    for (epoch_num, path) in &epochs {
        let batches = read_parquet_batches(path)?;
        for batch in &batches {
            let inv_col = str_col(batch, "invocation_id");
            let cmd_col = str_col(batch, "command");
            let sel_col = str_col(batch, "selector");
            let ver_col = str_col(batch, "dbt_version");
            let nc_col = i64_col(batch, "node_count");
            let tname_col = str_col(batch, "target_name");
            let atype_col = str_col(batch, "adapter_type");
            let sha_col = str_col(batch, "git_sha");
            let branch_col = str_col(batch, "git_branch");
            let dirty_col = i32_col(batch, "git_is_dirty");
            let elapsed_col = f64_col(batch, "elapsed_secs");
            let ts_col = timestamp_micros_col(batch, "ingested_at");
            for i in 0..batch.num_rows() {
                let Some(inv) = get_str(inv_col, i) else {
                    continue;
                };
                let generated_at = get_timestamp_micros(ts_col, i).map(micros_to_datetime);
                let git_dirty = get_i32(dirty_col, i).map(|v| v == 1);
                rows.push(InvocationOutputRow {
                    invocation_id: inv,
                    command: get_str(cmd_col, i),
                    selector: get_str(sel_col, i),
                    dbt_version: get_str(ver_col, i).unwrap_or_default(),
                    generated_at,
                    elapsed_time: get_f64(elapsed_col, i),
                    args: None,
                    node_count: get_i64(nc_col, i),
                    target_name: get_str(tname_col, i),
                    target_type: get_str(atype_col, i),
                    target_database: None,
                    target_schema: None,
                    target_threads: None,
                    vars_override: None,
                    git_sha: get_str(sha_col, i),
                    git_branch: get_str(branch_col, i),
                    git_is_dirty: git_dirty,
                });
            }
        }
        last_epoch = *epoch_num;
    }

    let count = rows.len();
    writer.write_rt_items("invocations", &rows)?;
    state.set_epoch(RUN_INVOCATIONS_SUBDIR, last_epoch);
    Ok(count)
}

// ---------------------------------------------------------------------------
// Run results → dbt_rt.run_results
// ---------------------------------------------------------------------------

fn write_run_results(
    writer: &mut IndexWriter,
    metadata_dir: &Path,
    state: &mut IngestState,
    _now: &str,
    full: bool,
) -> Result<usize, IndexError> {
    #[derive(serde::Serialize)]
    struct RunResultOutputRow {
        unique_id: String,
        invocation_id: String,
        status: Option<String>,
        execution_time: Option<f64>,
        thread_id: Option<String>,
        message: Option<String>,
        failures: Option<i64>,
        compiled: Option<bool>,
        compiled_code_hash: Option<String>,
        relation_name: Option<String>,
        adapter_response: Option<String>,
        timing: Option<String>,
        batch_results: Option<String>,
        rows_affected: Option<i64>,
        created_at: Option<chrono::DateTime<chrono::Utc>>,
    }

    let dir = metadata_dir.join(RUN_RESULTS_SUBDIR);
    if !dir.exists() {
        return Ok(0);
    }

    let epochs = epoch_layers::existing_epochs(&dir);
    let last_saved = state.last_epoch_for(RUN_RESULTS_SUBDIR);
    let curr_max_ep = epochs.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let need_full = full || last_saved == u32::MAX || curr_max_ep < last_saved;
    let has_new_epochs = need_full || epochs.iter().any(|(n, _)| *n > last_saved);
    if !has_new_epochs {
        return Ok(0);
    }

    let mut rows = Vec::new();
    let mut last_epoch = 0u32;
    for (epoch_num, path) in &epochs {
        let batches = read_parquet_batches(path)?;
        for batch in &batches {
            let uid_col = str_col(batch, "unique_id");
            let inv_col = str_col(batch, "invocation_id");
            let stat_col = str_col(batch, "status");
            let exec_col = f64_col(batch, "execution_time");
            let thr_col = str_col(batch, "thread_id");
            let msg_col = str_col(batch, "message");
            let fail_col = i64_col(batch, "failures");
            let chash_col = str_col(batch, "compiled_code_hash");
            let rel_col = str_col(batch, "relation_name");
            let resp_col = str_col(batch, "adapter_response");
            let tim_col = str_col(batch, "timing");
            let ts_col = timestamp_micros_col(batch, "ingested_at");
            for i in 0..batch.num_rows() {
                let Some(uid) = get_str(uid_col, i) else {
                    continue;
                };
                let Some(inv) = get_str(inv_col, i) else {
                    continue;
                };
                let created_at = get_timestamp_micros(ts_col, i).map(micros_to_datetime);
                rows.push(RunResultOutputRow {
                    unique_id: uid,
                    invocation_id: inv,
                    status: get_str(stat_col, i),
                    execution_time: get_f64(exec_col, i),
                    thread_id: get_str(thr_col, i),
                    message: get_str(msg_col, i),
                    failures: get_i64(fail_col, i),
                    compiled: None,
                    compiled_code_hash: get_str(chash_col, i),
                    relation_name: get_str(rel_col, i),
                    adapter_response: get_str(resp_col, i),
                    timing: get_str(tim_col, i),
                    batch_results: None,
                    rows_affected: None,
                    created_at,
                });
            }
        }
        last_epoch = *epoch_num;
    }

    let count = rows.len();
    writer.write_rt_items("run_results", &rows)?;
    state.set_epoch(RUN_RESULTS_SUBDIR, last_epoch);
    Ok(count)
}

// ---------------------------------------------------------------------------
// Run freshness → dbt.source_freshness (published as dbt_rt.freshness)
// ---------------------------------------------------------------------------

fn write_run_freshness(
    writer: &mut IndexWriter,
    metadata_dir: &Path,
    state: &mut IngestState,
    now: &str,
    full: bool,
) -> Result<usize, IndexError> {
    let dir = metadata_dir.join(RUN_FRESHNESS_SUBDIR);
    if !dir.exists() {
        return Ok(0);
    }

    let epochs = epoch_layers::existing_epochs(&dir);
    let last_saved = state.last_epoch_for(RUN_FRESHNESS_SUBDIR);
    let curr_max_ep = epochs.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let need_full = full || last_saved == u32::MAX || curr_max_ep < last_saved;
    let new_epochs: Vec<_> = if need_full {
        epochs
    } else {
        let last = state.last_epoch_for(RUN_FRESHNESS_SUBDIR);
        epochs.into_iter().filter(|(n, _)| *n > last).collect()
    };
    if new_epochs.is_empty() {
        return Ok(0);
    }

    let mut rows: Vec<Value> = Vec::new();
    let mut last_epoch = 0u32;
    for (epoch_num, path) in &new_epochs {
        let batches = read_parquet_batches(path)?;
        for batch in &batches {
            let uid_col = str_col(batch, "unique_id");
            let inv_col = str_col(batch, "invocation_id");
            let stat_col = str_col(batch, "status");
            // `source` or `model`. The epoch has carried it since model freshness shipped;
            // dropping it here is what left the published table unable to tell them apart.
            let rt_col = str_col(batch, "resource_type");
            let mla_col = str_col(batch, "max_loaded_at");
            let snap_col = str_col(batch, "snapshotted_at");
            let ago_col = f64_col(batch, "max_loaded_at_time_ago");
            let exec_col = f64_col(batch, "execution_time");
            let wc_col = i64_col(batch, "warn_after_count");
            let wp_col = str_col(batch, "warn_after_period");
            let ec_col = i64_col(batch, "error_after_count");
            let ep_col = str_col(batch, "error_after_period");
            let ts_col = i64_col(batch, "ingested_at");
            for i in 0..batch.num_rows() {
                let Some(uid) = get_str(uid_col, i) else {
                    continue;
                };
                let ts = get_i64(ts_col, i)
                    .map(micros_to_ts)
                    .unwrap_or_else(|| now.to_string());
                rows.push(json!({
                    "unique_id": uid,
                    "resource_type": get_str(rt_col, i),
                    "invocation_id": get_str(inv_col, i),
                    "status": get_str(stat_col, i),
                    "max_loaded_at": get_str(mla_col, i),
                    "snapshotted_at": get_str(snap_col, i),
                    "max_loaded_at_time_ago": get_f64(ago_col, i),
                    "execution_time": get_f64(exec_col, i),
                    "warn_after_count": get_i64(wc_col, i),
                    "warn_after_period": get_str(wp_col, i),
                    "error_after_count": get_i64(ec_col, i),
                    "error_after_period": get_str(ep_col, i),
                    "ingested_at": ts,
                }));
            }
        }
        last_epoch = *epoch_num;
    }

    let count = rows.len();
    if need_full {
        writer.write_dbt_table("source_freshness", rows)?;
    } else {
        writer.write_dbt_table_append("source_freshness", rows, "unique_id")?;
    }
    state.set_epoch(RUN_FRESHNESS_SUBDIR, last_epoch);
    Ok(count)
}

// ---------------------------------------------------------------------------
// Column stats → dbt.column_stats
// ---------------------------------------------------------------------------

fn write_column_stats(
    writer: &mut IndexWriter,
    index_dir: &Path,
    now: &str,
) -> Result<usize, IndexError> {
    let rr_path = index_dir.join("dbt_rt.run_results.parquet");
    let nc_path = index_dir.join("dbt.node_columns.parquet");
    if !rr_path.exists() || !nc_path.exists() {
        return Ok(0);
    }

    let mut row_counts: HashMap<String, i64> = HashMap::new();
    for batch in read_parquet_batches(&rr_path)? {
        let uid_col = str_col(&batch, "unique_id");
        let ra_col = i64_col(&batch, "rows_affected");
        for i in 0..batch.num_rows() {
            let Some(uid) = get_str(uid_col, i) else {
                continue;
            };
            let Some(ra) = get_i64(ra_col, i) else {
                continue;
            };
            row_counts.insert(uid, ra);
        }
    }
    if row_counts.is_empty() {
        return Ok(0);
    }

    let mut entries: Vec<(String, String)> = Vec::new();
    for batch in read_parquet_batches(&nc_path)? {
        let uid_col = str_col(&batch, "unique_id");
        let col_name_col = str_col(&batch, "column_name");
        for i in 0..batch.num_rows() {
            let Some(uid) = get_str(uid_col, i) else {
                continue;
            };
            let Some(col) = get_str(col_name_col, i) else {
                continue;
            };
            if row_counts.contains_key(&uid) {
                entries.push((uid, col));
            }
        }
    }

    let typed_rows: Vec<crate::parquet::ColumnStatRow<'_>> = entries
        .iter()
        .map(|(uid, col)| crate::parquet::ColumnStatRow {
            unique_id: uid.as_str(),
            column_name: Some(col.as_str()),
            column_type: None,
            row_count: row_counts.get(uid.as_str()).copied(),
            distinct_count: None,
            null_pct: None,
            min_value: None,
            max_value: None,
            avg_value: None,
            std_value: None,
            q25: None,
            q50: None,
            q75: None,
            top_values: None,
            ingested_at: now,
        })
        .collect();

    let count = typed_rows.len();
    writer.write_dbt_items("column_stats", &typed_rows)?;
    Ok(count)
}

// ---------------------------------------------------------------------------
// Seed catalog stats → dbt.catalog_tables + dbt.catalog_stats
// ---------------------------------------------------------------------------

fn write_seed_catalog_stats(
    writer: &mut IndexWriter,
    index_dir: &Path,
    metadata_dir: &Path,
    now: &str,
) -> Result<usize, IndexError> {
    let nodes_path = index_dir.join("dbt.nodes.parquet");
    if !nodes_path.exists() {
        return Ok(0);
    }
    let target_dir = metadata_dir.parent().unwrap_or(metadata_dir);
    let data_dir = target_dir.join("data");
    if !data_dir.exists() {
        return Ok(0);
    }

    let mut catalog_table_rows: Vec<Value> = Vec::new();
    let mut catalog_stat_rows: Vec<Value> = Vec::new();

    for batch in read_parquet_batches(&nodes_path)? {
        let uid_col = str_col(&batch, "unique_id");
        let rt_col = str_col(&batch, "resource_type");
        let rn_col = str_col(&batch, "relation_name");

        for i in 0..batch.num_rows() {
            let Some(resource_type) = get_str(rt_col, i) else {
                continue;
            };
            if resource_type != "seed" {
                continue;
            }
            let Some(uid) = get_str(uid_col, i) else {
                continue;
            };
            let Some(relation_name) = get_str(rn_col, i) else {
                continue;
            };

            let parts: Vec<&str> = relation_name.splitn(3, '.').collect();
            if parts.len() != 3 {
                continue;
            }
            let data_path = data_dir
                .join(parts[0])
                .join(parts[1])
                .join(parts[2])
                .join("output.parquet");
            if !data_path.exists() {
                continue;
            }

            let row_count = read_parquet_batches(&data_path)?
                .iter()
                .map(|b| b.num_rows() as i64)
                .sum::<i64>();

            catalog_table_rows.push(json!({
                "unique_id": uid,
                "table_type": "TABLE",
                "database_name": parts[0],
                "schema_name": parts[1],
                "table_name": parts[2],
                "table_owner": null,
                "table_comment": null,
                "ingested_at": now,
            }));
            catalog_stat_rows.push(json!({
                "unique_id": uid,
                "stat_id": "row_count",
                "stat_label": "Row Count",
                "stat_value": row_count.to_string(),
                "description": "Row count derived from seed data parquet",
                "include_in_stats": true,
                "ingested_at": now,
            }));
        }
    }

    let count = catalog_stat_rows.len();
    if count > 0 {
        writer.write_dbt_table("catalog_tables", catalog_table_rows)?;
        writer.write_dbt_table("catalog_stats", catalog_stat_rows)?;
    }
    Ok(count)
}

// ---------------------------------------------------------------------------
// Warehouse catalog stats → dbt.catalog_tables + dbt.catalog_stats
// ---------------------------------------------------------------------------

fn write_warehouse_catalog_stats(
    writer: &mut IndexWriter,
    metadata_dir: &Path,
    state: &mut IngestState,
    now: &str,
    full: bool,
) -> Result<usize, IndexError> {
    let dir = metadata_dir.join(RUN_CATALOG_STATS_SUBDIR);
    if !dir.exists() {
        return Ok(0);
    }

    let epochs = epoch_layers::existing_epochs(&dir);
    let last_saved = state.last_epoch_for(RUN_CATALOG_STATS_SUBDIR);
    let curr_max_ep = epochs.iter().map(|(n, _)| *n).max().unwrap_or(0);
    let need_full = full || last_saved == u32::MAX || curr_max_ep < last_saved;
    let new_epochs: Vec<_> = if need_full {
        epochs
    } else {
        let last = state.last_epoch_for(RUN_CATALOG_STATS_SUBDIR);
        epochs.into_iter().filter(|(n, _)| *n > last).collect()
    };
    if new_epochs.is_empty() {
        return Ok(0);
    }

    let mut catalog_table_rows: Vec<Value> = Vec::new();
    let mut catalog_stat_rows: Vec<Value> = Vec::new();
    let mut last_epoch = 0u32;

    for (epoch_num, path) in &new_epochs {
        let batches = read_parquet_batches(path)?;
        for batch in &batches {
            let uid_col = str_col(batch, "unique_id");
            let tt_col = str_col(batch, "table_type");
            let owner_col = str_col(batch, "table_owner");
            let db_col = str_col(batch, "database_name");
            let schema_col = str_col(batch, "schema_name");
            let tname_col = str_col(batch, "table_name");
            let rc_col = i64_col(batch, "row_count");
            let bytes_col = i64_col(batch, "bytes");
            let lm_col = str_col(batch, "last_modified");

            for i in 0..batch.num_rows() {
                let Some(uid) = get_str(uid_col, i) else {
                    continue;
                };

                let table_type = get_str(tt_col, i);
                let table_owner = get_str(owner_col, i);
                let database_name = get_str(db_col, i);
                let schema_name = get_str(schema_col, i);
                let table_name = get_str(tname_col, i);
                let row_count = get_i64(rc_col, i);
                let bytes = get_i64(bytes_col, i);
                let last_modified = get_str(lm_col, i);

                catalog_table_rows.push(json!({
                    "unique_id": uid,
                    "table_type": table_type,
                    "database_name": database_name,
                    "schema_name": schema_name,
                    "table_name": table_name,
                    "table_owner": table_owner,
                    "table_comment": null,
                    "ingested_at": now,
                }));

                if let Some(rc) = row_count {
                    catalog_stat_rows.push(json!({
                        "unique_id": uid,
                        "stat_id": "row_count",
                        "stat_label": "Row Count",
                        "stat_value": rc.to_string(),
                        "description": "Row count from warehouse catalog",
                        "include_in_stats": true,
                        "ingested_at": now,
                    }));
                }
                if let Some(b) = bytes {
                    catalog_stat_rows.push(json!({
                        "unique_id": uid,
                        "stat_id": "bytes",
                        "stat_label": "Bytes",
                        "stat_value": b.to_string(),
                        "description": "Table size in bytes from warehouse catalog",
                        "include_in_stats": true,
                        "ingested_at": now,
                    }));
                }
                if let Some(lm) = last_modified {
                    catalog_stat_rows.push(json!({
                        "unique_id": uid,
                        "stat_id": "last_modified",
                        "stat_label": "Last Modified",
                        "stat_value": lm,
                        "description": "Last modified timestamp from warehouse catalog",
                        "include_in_stats": true,
                        "ingested_at": now,
                    }));
                }
            }
        }
        last_epoch = *epoch_num;
    }

    let count = catalog_stat_rows.len();
    if need_full {
        writer.write_dbt_table("catalog_tables", catalog_table_rows)?;
        writer.write_dbt_table("catalog_stats", catalog_stat_rows)?;
    } else {
        writer.write_dbt_table_append("catalog_tables", catalog_table_rows, "unique_id")?;
        writer.write_dbt_table_append("catalog_stats", catalog_stat_rows, "unique_id")?;
    }
    state.set_epoch(RUN_CATALOG_STATS_SUBDIR, last_epoch);
    Ok(count)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{Int32Array, RecordBatch, StringArray, TimestampMicrosecondArray};
    use arrow_schema::{DataType, Field, Schema, TimeUnit};

    use super::{
        CATALOG_COLUMNS_SUBDIR, COMPILE_COLUMNS_SUBDIR, IngestState, extract_json_field_raw,
        project_var_rows, read_parquet_batches, trim_model_payload, write_catalog_columns,
        write_compile_columns,
    };
    use crate::parquet::IndexWriter;

    /// `(project_name, var_name, var_value)` of each row, sorted, for comparing
    /// against an expected table.
    fn var_table(rows: &[serde_json::Value]) -> Vec<(String, String, String)> {
        let mut got: Vec<_> = rows
            .iter()
            .map(|r| {
                let f = |k: &str| r[k].as_str().expect(k).to_string();
                (f("project_name"), f("var_name"), f("var_value"))
            })
            .collect();
        got.sort();
        got
    }

    /// The shape a real parse produces: one entry per package, each holding that
    /// package's *resolved* vars, plus the root's scope declaration repeated in
    /// every entry. The scope is already applied in `my_pkg`'s own entry, so the
    /// answer is four rows -- one per variable per project.
    ///
    /// Reading the declaration as well as the resolved entry is what produced
    /// duplicates: `my_pkg`'s two variables came back once from its own entry and
    /// once from every other package's copy of the declaration.
    #[test]
    fn project_vars_is_one_row_per_project_and_variable() {
        let root = r#"{"grace":"goheen","peanut":"jelly","my_pkg":{"peanut":"butter"}}"#;
        let scoped = r#"{"grace":"goheen","peanut":"butter","my_pkg":{"peanut":"butter"}}"#;
        let vars =
            format!(r#"{{"proj":{root},"my_pkg":{scoped},"dbt":{root},"dbt_duckdb":{root}}}"#,);
        let installed = ["proj", "my_pkg"].into_iter().map(String::from).collect();

        let rows = project_var_rows(Some(&vars), &installed, "duckdb", "2026-09-03T00:00:00Z");

        assert_eq!(
            var_table(&rows),
            vec![
                ("my_pkg".into(), "grace".into(), "goheen".into()),
                ("my_pkg".into(), "peanut".into(), "butter".into()),
                ("proj".into(), "grace".into(), "goheen".into()),
                ("proj".into(), "peanut".into(), "jelly".into()),
            ],
        );
    }

    /// The packages dbt installs itself carry a copy of the root's vars that no
    /// one set and no one reads. `dbt_postgres` is in for Redshift, whose
    /// dispatch package is a child of it.
    #[test]
    fn project_vars_leaves_out_the_packages_dbt_installs() {
        let map = r#"{"grace":"goheen"}"#;
        let vars =
            format!(r#"{{"proj":{map},"dbt":{map},"dbt_redshift":{map},"dbt_postgres":{map}}}"#);
        let installed = ["proj"].into_iter().map(String::from).collect();

        let rows = project_var_rows(Some(&vars), &installed, "redshift", "2026-09-03T00:00:00Z");

        assert_eq!(
            var_table(&rows),
            vec![("proj".into(), "grace".into(), "goheen".into())],
        );
    }

    /// A nested map is a scope only when its key names an installed package.
    /// `dbt` supports object-valued variables, so a nested map keyed by anything
    /// else is a variable and keeps its JSON as the value -- and a string value is
    /// rendered bare rather than re-quoted.
    #[test]
    fn project_vars_keeps_an_object_valued_variable() {
        let vars = r#"{"proj":{"thresholds":{"warn":10},"dbt_utils":{"x":1},"note":"hi","n":90}}"#;
        let installed = ["proj", "dbt_utils"]
            .into_iter()
            .map(String::from)
            .collect();

        let rows = project_var_rows(Some(vars), &installed, "duckdb", "2026-09-03T00:00:00Z");

        assert_eq!(
            var_table(&rows),
            vec![
                ("proj".into(), "n".into(), "90".into()),
                ("proj".into(), "note".into(), "hi".into()),
                ("proj".into(), "thresholds".into(), r#"{"warn":10}"#.into()),
            ],
            "'dbt_utils' is a scope; 'thresholds' is a variable",
        );
    }

    /// A project with no `vars` block at all, and metadata too old to carry the
    /// field, both mean no rows rather than an error.
    #[test]
    fn project_vars_tolerates_missing_vars() {
        let installed = ["proj"].into_iter().map(String::from).collect();
        let now = "2026-09-03T00:00:00Z";
        assert!(project_var_rows(None, &installed, "duckdb", now).is_empty());
        assert!(project_var_rows(Some("{}"), &installed, "duckdb", now).is_empty());
        assert!(project_var_rows(Some("not json"), &installed, "duckdb", now).is_empty());
        assert!(project_var_rows(Some(r#"{"proj":{}}"#), &installed, "duckdb", now).is_empty());
    }

    /// A `catalog/columns` epoch holding `nodes × columns_per_node` rows.
    fn write_catalog_epoch(dir: &std::path::Path, nodes: &[&str], columns_per_node: &[&str]) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("unique_id", DataType::Utf8, false),
            Field::new("column_name", DataType::Utf8, false),
            Field::new("column_index", DataType::Int32, true),
            Field::new("catalog_type", DataType::Utf8, true),
            Field::new("catalog_comment", DataType::Utf8, true),
            Field::new(
                "ingested_at",
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                true,
            ),
        ]));
        let (mut uids, mut names, mut idxs, mut types) = (vec![], vec![], vec![], vec![]);
        for uid in nodes {
            for (i, col) in columns_per_node.iter().enumerate() {
                uids.push(*uid);
                names.push(*col);
                idxs.push(i as i32);
                types.push("VARCHAR");
            }
        }
        let n = uids.len();
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(uids)),
                Arc::new(StringArray::from(names)),
                Arc::new(Int32Array::from(idxs)),
                Arc::new(StringArray::from(types)),
                Arc::new(StringArray::from(vec![None::<&str>; n])),
                Arc::new(TimestampMicrosecondArray::from(vec![0i64; n]).with_timezone("UTC")),
            ],
        )
        .expect("catalog batch");
        std::fs::create_dir_all(dir).expect("epoch dir");
        let file = std::fs::File::create(dir.join("v1_0.parquet")).expect("epoch file");
        let mut writer =
            parquet::arrow::ArrowWriter::try_new(file, schema, None).expect("epoch writer");
        writer.write(&batch).expect("epoch write");
        writer.close().expect("epoch close");
    }

    /// Catalog rows are deduplicated per node across epochs, not per row: a node
    /// has one row per column in the same batch, so marking it seen mid-batch
    /// would keep only its first column.
    #[test]
    fn a_catalog_epoch_keeps_every_column_of_every_node() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let metadata_dir = tmp.path().join("metadata");
        write_catalog_epoch(
            &metadata_dir.join(CATALOG_COLUMNS_SUBDIR),
            &["model.p.a", "model.p.b"],
            &["c1", "c2", "c3"],
        );

        let index_dir = tmp.path().join("index");
        let mut writer = IndexWriter::new(&index_dir).expect("writer");
        let mut state = IngestState::default();
        let kept = write_catalog_columns(
            &mut writer,
            &metadata_dir,
            &mut state,
            "2026-08-25T00:00:00Z",
            true,
            None,
        )
        .expect("write catalog columns");

        assert_eq!(kept, 6, "2 nodes × 3 columns");
        let written = read_parquet_batches(&index_dir.join("dbt.node_columns.parquet"))
            .expect("read node_columns");
        let rows: usize = written.iter().map(|b| b.num_rows()).sum();
        assert_eq!(rows, 6);
    }

    /// A `compile/columns` epoch holding one row per `(unique_id, column_name)`.
    fn write_compile_columns_epoch(dir: &std::path::Path, file: &str, rows: &[(&str, &str)]) {
        let schema = Arc::new(Schema::new(vec![
            Field::new("unique_id", DataType::Utf8, false),
            Field::new("column_name", DataType::Utf8, false),
            Field::new("column_index", DataType::Int32, true),
            Field::new("column_type", DataType::Utf8, true),
            Field::new(
                "ingested_at",
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                true,
            ),
        ]));
        let n = rows.len();
        let batch = RecordBatch::try_new(
            Arc::clone(&schema),
            vec![
                Arc::new(StringArray::from(
                    rows.iter().map(|(uid, _)| *uid).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    rows.iter().map(|(_, col)| *col).collect::<Vec<_>>(),
                )),
                Arc::new(Int32Array::from((0..n as i32).collect::<Vec<_>>())),
                Arc::new(StringArray::from(vec!["INT"; n])),
                Arc::new(TimestampMicrosecondArray::from(vec![0i64; n]).with_timezone("UTC")),
            ],
        )
        .expect("compile columns batch");
        std::fs::create_dir_all(dir).expect("epoch dir");
        let out = std::fs::File::create(dir.join(file)).expect("epoch file");
        let mut writer =
            parquet::arrow::ArrowWriter::try_new(out, schema, None).expect("epoch writer");
        writer.write(&batch).expect("epoch write");
        writer.close().expect("epoch close");
    }

    /// A cold ingest reading several epochs at once applies the group rule itself.
    ///
    /// A delta ingest is handed only the epochs it has not seen, so replacing a
    /// node's whole column set falls out of the write. A cold one — fresh
    /// `target/`, CI, `dbt clean`, or any first ingest after several compiles —
    /// reads every epoch in one pass and has to supersede within that pass:
    /// concatenating them publishes a column a later compile dropped, and
    /// duplicates every column it kept. `dedup_epoch_groups` is what does it, and
    /// the unit is the whole group, not the row: `a.c` below exists only in the
    /// older epoch, so a per-`(node, column)` dedup would keep it. The view layer
    /// states the same rule as `Supersede::LatestGroup` — see
    /// `info_schema::tests_epoch::latest_group_supersedes_the_whole_group`.
    #[test]
    fn a_cold_ingest_supersedes_a_whole_group_across_epochs() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let metadata_dir = tmp.path().join("metadata");
        let dir = metadata_dir.join(COMPILE_COLUMNS_SUBDIR);
        // Epoch 0: `a` has three columns, `b` one. Epoch 1: `a` is recompiled and
        // now has two — `c` is gone — and does not mention `b` at all.
        write_compile_columns_epoch(
            &dir,
            "v1_0.parquet",
            &[
                ("model.p.a", "x"),
                ("model.p.a", "y"),
                ("model.p.a", "c"),
                ("model.p.b", "z"),
            ],
        );
        write_compile_columns_epoch(
            &dir,
            "v1_1.parquet",
            &[("model.p.a", "x"), ("model.p.a", "y")],
        );

        let index_dir = tmp.path().join("index");
        let mut writer = IndexWriter::new(&index_dir).expect("writer");
        let mut state = IngestState::default();
        let kept = write_compile_columns(
            &mut writer,
            &metadata_dir,
            &mut state,
            "2026-08-25T00:00:00Z",
            true,
            None,
        )
        .expect("write compile columns");
        assert_eq!(kept, 3, "`a`'s two columns from epoch 1 plus `b`'s one");

        let written = read_parquet_batches(&index_dir.join("dbt.node_columns.parquet"))
            .expect("read node_columns");
        let mut got: Vec<String> = Vec::new();
        for batch in &written {
            let uids = super::str_col(batch, "unique_id").expect("unique_id");
            let names = super::str_col(batch, "column_name").expect("column_name");
            for i in 0..batch.num_rows() {
                got.push(format!("{}.{}", uids.value(i), names.value(i)));
            }
        }
        got.sort();
        assert_eq!(
            got,
            vec!["model.p.a.x", "model.p.a.y", "model.p.b.z"],
            "the newest epoch that mentions `a` replaces every row it had — `a.c` \
             is dropped and not duplicated — while `b`, which epoch 1 does not \
             mention, keeps its epoch-0 row"
        );
    }

    #[test]
    fn field_is_read_from_the_top_level_only() {
        // A model with documented columns carries a column-level `config` ahead
        // of its own in the payload text; the node's config is the one wanted.
        let payload = r#"{"name":"m","columns":{"id":{"config":{"tags":[]}}},"config":{"materialized":"table"}}"#;
        assert_eq!(
            extract_json_field_raw(payload, "config"),
            Some(r#"{"materialized":"table"}"#)
        );
    }

    #[test]
    fn a_key_inside_a_string_value_is_not_a_field() {
        let payload = r#"{"raw_code":"select 1 -- \"config\": {}","config":{"enabled":true}}"#;
        assert_eq!(
            extract_json_field_raw(payload, "config"),
            Some(r#"{"enabled":true}"#)
        );
    }

    #[test]
    fn every_value_shape_is_delimited() {
        let payload = r#"{"a":"s","b":12,"c":true,"d":null,"e":[1,2],"f":{"g":1}}"#;
        for (key, want) in [
            ("a", "\"s\""),
            ("b", "12"),
            ("c", "true"),
            ("d", "null"),
            ("e", "[1,2]"),
            ("f", "{\"g\":1}"),
        ] {
            assert_eq!(
                extract_json_field_raw(payload, key),
                Some(want),
                "key {key}"
            );
        }
        assert_eq!(extract_json_field_raw(payload, "missing"), None);
        assert_eq!(extract_json_field_raw("[1]", "a"), None);
    }

    #[test]
    fn the_model_trim_keeps_what_the_row_builders_read() {
        let payload = r#"{"raw_code":"select 1","config":{"materialized":"view"},
            "__model_attr__":{"contract":{"enforced":true},"primary_key":["id"],
            "time_spine":{"standard_granularity_column":"d"},"other":"kept too"}}"#;
        let trimmed = trim_model_payload(Some(payload));
        assert_eq!(
            trimmed.get("config").and_then(|v| v.as_str()),
            Some(r#"{"materialized":"view"}"#)
        );
        let ma = trimmed.get("__model_attr__").expect("model attr");
        assert!(ma.get("contract").is_some());
        assert!(ma.get("primary_key").is_some());
        assert!(ma.get("time_spine").is_some());
        // Whole-object, so an attribute no builder reads yet is there when one
        // starts to. `raw_code` is what the trim is for, and is gone.
        assert!(ma.get("other").is_some());
        assert!(trimmed.get("raw_code").is_none());
    }

    #[test]
    fn the_model_trim_keeps_common_attr_minus_raw_code() {
        let payload = r#"{"__common_attr__":{"raw_code":"select 1","language":"sql",
            "patch_path":"models/schema.yml","meta":{"owner":"data"}}}"#;
        let trimmed = trim_model_payload(Some(payload));
        let common = trimmed.get("__common_attr__").expect("common attr");
        assert_eq!(common.get("language").and_then(|v| v.as_str()), Some("sql"));
        assert_eq!(
            common.get("patch_path").and_then(|v| v.as_str()),
            Some("models/schema.yml")
        );
        assert!(common.get("meta").is_some());
        assert!(common.get("raw_code").is_none());
    }

    #[test]
    fn the_model_trim_omits_absent_fields() {
        let trimmed = trim_model_payload(Some(r#"{"other":1}"#));
        assert!(trimmed.get("config").is_none());
        assert!(trimmed.get("__model_attr__").is_none());
        assert_eq!(trim_model_payload(None), serde_json::json!({}));
    }
}
