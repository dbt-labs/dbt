//! DuckDB `COPY` materializer: the information schema as parquet written
//! straight from the epoch-view layer.
//!
//! Replaces the Arrow ingest-and-project path when a DuckDB driver is
//! available. The views already express the public contract over
//! `target/private/metadata/**`; `COPY (SELECT * FROM <view>) TO '<file>'` is that
//! contract as a snapshot, with no staging directory and no per-row Rust.
//!
//! Tables whose epoch relation has no files on disk are not in the generated
//! SQL (see [`super::epoch_views`]); those land as empty Arrow files so
//! `views.sql` still has something to point at. The public directory never
//! receives `epoch_views.sql` — that file reads the private metadata layout.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use crate::IndexError;
use crate::db::Db;
use crate::ingest::timings;

use super::schema::INFO_SCHEMA;
use super::spec::TableSpec;
use super::{
    INFO_SCHEMA_VERSION, INFO_SCHEMA_VERSION_KEY, epoch_views, project, versioned_dir, views,
    write_output, write_parquet,
};

/// Materialize every information-schema table into `info_schema_dir` by
/// executing the epoch views and `COPY`-ing each one out.
///
/// Fails if the DuckDB driver cannot be loaded or if a `COPY` statement
/// fails. The caller that wants Arrow as a fallback is [`super::write_info_schema_with`].
pub(super) fn write_via_copy(
    metadata_dir: &Path,
    info_schema_dir: &Path,
) -> Result<usize, IndexError> {
    let generated = epoch_views::generate(metadata_dir)?;
    let out_dir = versioned_dir(info_schema_dir);
    std::fs::create_dir_all(&out_dir)?;

    let skipped: BTreeSet<String> = generated.skipped.iter().cloned().collect();
    let to_copy: Vec<&TableSpec> = INFO_SCHEMA
        .iter()
        .filter(|spec| !skipped.contains(&spec.qualified_name()))
        .collect();

    if !to_copy.is_empty() {
        timings::time(timings::Stage::Copy, || -> Result<(), IndexError> {
            let mut db = Db::open_memory().map_err(|e| {
                IndexError::Other(format!("info schema COPY: DuckDB driver unavailable: {e}"))
            })?;
            for stmt in &generated.statements {
                db.execute_update(stmt).map_err(|e| {
                    IndexError::Other(format!("info schema COPY: executing view DDL: {e}"))
                })?;
            }

            // Prefer DuckDB's parquet KV so we do not re-encode. If this driver
            // cannot set it, fall back to stamping after write — correctness of
            // the version signal over the COPY speed win for that file.
            //
            // `kv_in_copy` is only whether later COPY statements should still
            // *request* KV_METADATA. A file that landed without the key is
            // always stamped, including after the probe has already failed:
            // otherwise the rest of the tree would ship unversioned.
            let mut kv_in_copy = true;
            for spec in &to_copy {
                let path = std::path::absolute(out_dir.join(spec.file_name())).map_err(|e| {
                    IndexError::Other(format!(
                        "info schema COPY {}: {}: {e}",
                        spec.qualified_name(),
                        out_dir.display()
                    ))
                })?;
                match copy_table(&mut db, spec, &path, kv_in_copy) {
                    Ok(()) => {
                        let (next, stamp) = after_copy_kv(kv_in_copy, has_version_kv(&path));
                        kv_in_copy = next;
                        if stamp {
                            stamp_version_kv(&path)?;
                        }
                    }
                    Err(_) if kv_in_copy => {
                        kv_in_copy = false;
                        copy_table(&mut db, spec, &path, false).map_err(|e| {
                            IndexError::Other(format!(
                                "info schema COPY {}: {e}",
                                spec.qualified_name()
                            ))
                        })?;
                        stamp_version_kv(&path)?;
                    }
                    Err(e) => {
                        return Err(IndexError::Other(format!(
                            "info schema COPY {}: {e}",
                            spec.qualified_name()
                        )));
                    }
                }
            }
            Ok(())
        })?;
    }

    for spec in INFO_SCHEMA {
        if skipped.contains(&spec.qualified_name()) {
            let out = project::out_schema(spec)?;
            write_output(&out_dir, spec, &out, &[])?;
        }
    }

    timings::time(timings::Stage::ViewsSql, || {
        views::write_views_sql(&out_dir)
    })?;
    Ok(INFO_SCHEMA.len())
}

fn copy_table(db: &mut Db, spec: &TableSpec, path: &Path, with_kv: bool) -> Result<(), IndexError> {
    let sql = copy_sql(spec, path, with_kv);
    db.execute_update(&sql)?;
    Ok(())
}

fn copy_sql(spec: &TableSpec, path: &Path, with_kv: bool) -> String {
    let quoted = path.display().to_string().replace('\'', "''");
    let base = format!(
        "COPY (SELECT * FROM {q}) TO '{quoted}' \
         (FORMAT PARQUET, COMPRESSION ZSTD, COMPRESSION_LEVEL 1",
        q = spec.qualified_name(),
    );
    if with_kv {
        format!(
            "{base}, KV_METADATA {{'{key}': '{ver}'}})",
            key = INFO_SCHEMA_VERSION_KEY,
            ver = INFO_SCHEMA_VERSION,
        )
    } else {
        format!("{base})")
    }
}

fn has_version_kv(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(builder) = ParquetRecordBatchReaderBuilder::try_new(file) else {
        return false;
    };
    let want = INFO_SCHEMA_VERSION.to_string();
    builder
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .map(|kvs| {
            kvs.iter().any(|kv| {
                kv.key == INFO_SCHEMA_VERSION_KEY && kv.value.as_deref() == Some(want.as_str())
            })
        })
        .unwrap_or(false)
}

/// Re-encode a parquet file solely to attach [`INFO_SCHEMA_VERSION_KEY`].
/// Used only when DuckDB `COPY` did not write the KV itself.
fn stamp_version_kv(path: &Path) -> Result<(), IndexError> {
    if has_version_kv(path) {
        return Ok(());
    }
    let file = std::fs::File::open(path)?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| IndexError::Other(format!("info schema stamp {}: {e}", path.display())))?;
    let schema = Arc::clone(builder.schema());
    let reader = builder
        .build()
        .map_err(|e| IndexError::Other(format!("info schema stamp {}: {e}", path.display())))?;
    let batches: Vec<_> = reader.flatten().collect();
    write_parquet(path, &schema, &batches)
}

/// After a COPY: keep requesting DuckDB KV only while files actually carry it.
/// Always stamp a file that is missing the version key — including after the
/// probe has already concluded the driver will not attach one.
fn after_copy_kv(kv_in_copy: bool, file_has_kv: bool) -> (bool, bool) {
    (kv_in_copy && file_has_kv, !file_has_kv)
}

#[cfg(test)]
mod tests {
    use super::after_copy_kv;

    #[test]
    fn after_copy_keeps_requesting_kv_when_the_file_has_it() {
        assert_eq!(after_copy_kv(true, true), (true, false));
    }

    #[test]
    fn after_copy_stamps_the_probe_miss_and_stops_requesting_kv() {
        assert_eq!(after_copy_kv(true, false), (false, true));
    }

    #[test]
    fn after_copy_still_stamps_once_the_probe_has_failed() {
        // Regression: `kv_in_copy && !has_kv` skipped the stamp for every
        // table after the first probe miss, leaving the rest unversioned.
        assert_eq!(after_copy_kv(false, false), (false, true));
    }
}
