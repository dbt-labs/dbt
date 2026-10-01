//! COPY-path coverage that needs a DuckDB driver.
//!
//! Empty-metadata tests in `tests.rs` stay on Arrow so they pass without a
//! driver. This module checks the COPY writer when one is present.

use std::path::Path;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, TimeUnit};
use serde_json::{Value, json};

use super::schema::INFO_SCHEMA;

/// The COPY path, when a DuckDB driver is present, writes the same versioned
/// tree as Arrow: every table, `views.sql`, the parquet KV, and not
/// `epoch_views.sql`.
#[test]
fn copy_path_writes_versioned_tables_when_duckdb_available() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let staging = tempfile::tempdir().unwrap();
    super::write_info_schema_with(
        super::Materializer::Copy,
        metadata.path(),
        root.path(),
        staging.path(),
    )
    .unwrap();

    let versioned = super::versioned_dir(root.path());
    assert!(versioned.join("views.sql").exists());
    assert!(
        !versioned.join("epoch_views.sql").exists(),
        "epoch_views.sql reads the private metadata layout and must not ship"
    );
    let want = super::INFO_SCHEMA_VERSION.to_string();
    for table in INFO_SCHEMA {
        assert!(
            versioned.join(table.file_name()).exists(),
            "{} was not written",
            table.qualified_name()
        );
        let kv = out_kv(&versioned, &table.file_name());
        assert!(
            kv.iter()
                .any(|(k, v)| k == super::INFO_SCHEMA_VERSION_KEY && v.as_deref() == Some(&want)),
            "{} is missing {}",
            table.qualified_name(),
            super::INFO_SCHEMA_VERSION_KEY
        );
    }
    // COPY does not stage.
    assert!(
        std::fs::read_dir(staging.path())
            .map(|mut e| e.next().is_none())
            .unwrap_or(true)
    );
}

fn out_kv(dir: &Path, file: &str) -> Vec<(String, Option<String>)> {
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    let f = std::fs::File::open(dir.join(file)).unwrap();
    let builder = ParquetRecordBatchReaderBuilder::try_new(f).unwrap();
    builder
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .map(|kvs| {
            kvs.iter()
                .map(|kv| (kv.key.clone(), kv.value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The two materializers must agree on `dbt_rt.relations`.
///
/// Every other table is a column projection on both paths, so they agree by
/// construction. This one is not: COPY projects the wide `run/catalog_stats`
/// epoch straight through, while Arrow has to undo the split the staging ingest
/// applies on the way in — identity into `catalog_tables`, each number into its
/// own entity-attribute-value `catalog_stats` row. Two different computations of
/// the same rows is exactly where they can drift, and nothing else covers it:
/// both sides of the epoch-view differential test are the epoch path.
///
/// DuckDB reports no catalog stats, so no local corpus has these rows. The epoch
/// is written here instead, in the shape `dbt_metadata_parquet`'s
/// `CatalogStatEpochRow` declares.
///
/// `ingested_at` is excluded, and not because of this table. The two paths mean
/// different things by it *everywhere*: `IndexWriter::write_dbt_table` stamps
/// every staged row with the ingest's own clock, overwriting whatever the row
/// builder put there, so the Arrow path reports when the ingest ran — while
/// COPY reads the epoch and reports when the warehouse was asked. That is a
/// pre-existing, schema-wide divergence with no test naming it; excluding one
/// column here rather than fixing `ingested_at`'s meaning across every table is
/// deliberate scope, not an oversight.
#[test]
fn arrow_and_copy_agree_on_relations() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    write_catalog_stats_epoch(
        metadata.path(),
        1,
        &[
            json!({
                "unique_id": "model.pkg.a",
                "table_type": "BASE TABLE",
                "table_owner": "owner",
                "database_name": "db",
                "schema_name": "sch",
                "table_name": "a",
                "row_count": 935,
                "bytes": 40960,
                "last_modified": "2026-09-08 12:00:00",
                "ingested_at": "2026-09-08T12:00:00Z",
            }),
            // A relation the warehouse reported no numbers for: its row still
            // has to survive, since identity alone answers "is it catalogued".
            json!({
                "unique_id": "model.pkg.bare",
                "table_type": "VIEW",
                "database_name": "db",
                "schema_name": "sch",
                "table_name": "bare",
                "ingested_at": "2026-09-08T12:00:00Z",
            }),
        ],
    );

    let arrow = materialize(super::Materializer::Arrow, metadata.path());
    let copy = materialize(super::Materializer::Copy, metadata.path());
    assert_eq!(
        arrow.len(),
        2,
        "the Arrow path lost rows the epoch carried: {arrow:?}"
    );
    assert_eq!(arrow, copy, "Arrow and COPY disagree on dbt_rt.relations");
}

/// Run one materializer over `metadata_dir` and read `dbt_rt.relations` back,
/// sorted so the comparison is about content rather than row order.
fn materialize(how: super::Materializer, metadata_dir: &Path) -> Vec<Value> {
    let root = tempfile::tempdir().unwrap();
    let staging = tempfile::tempdir().unwrap();
    super::write_info_schema_with(how, metadata_dir, root.path(), staging.path()).unwrap();

    let dir = super::versioned_dir(root.path());
    let mut rows = super::tests::out_rows(&dir, "dbt_rt.relations.parquet");
    for row in &mut rows {
        if let Some(obj) = row.as_object_mut() {
            obj.remove("ingested_at");
        }
    }
    rows.sort_by_key(|r| r["unique_id"].as_str().unwrap_or_default().to_string());
    rows
}

/// Write one `run/catalog_stats` epoch, in `CatalogStatEpochRow`'s shape.
/// `epoch` picks the file name the supersede logic orders on
/// (`v1_<epoch>.parquet`), not the write order.
fn write_catalog_stats_epoch(metadata_dir: &Path, epoch: u32, rows: &[Value]) {
    let dir = metadata_dir.join("run").join("catalog_stats");
    std::fs::create_dir_all(&dir).unwrap();
    let utf8 = |name: &str| Field::new(name, DataType::Utf8, true);
    let schema = Arc::new(Schema::new(vec![
        Field::new("unique_id", DataType::Utf8, false),
        utf8("table_type"),
        utf8("table_owner"),
        utf8("database_name"),
        utf8("schema_name"),
        utf8("table_name"),
        Field::new("row_count", DataType::Int64, true),
        Field::new("bytes", DataType::Int64, true),
        utf8("last_modified"),
        Field::new(
            "ingested_at",
            DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC"))),
            false,
        ),
    ]));
    crate::parquet::write_table(&dir.join(format!("v1_{epoch}.parquet")), schema, rows).unwrap();
}

/// A relation re-catalogued in a later epoch must replace its old row on the
/// COPY path, not accumulate one row per invocation the way `dbt_rt.run_results`
/// does. Nothing else catches this: `arrow_and_copy_agree_on_relations` writes a
/// single epoch, where a per-invocation log and a latest-wins view look
/// identical.
#[test]
fn copy_relations_supersede_across_epochs() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    write_catalog_stats_epoch(
        metadata.path(),
        1,
        &[json!({
            "unique_id": "model.pkg.a",
            "table_type": "BASE TABLE",
            "database_name": "db",
            "schema_name": "sch",
            "table_name": "a",
            "row_count": 100,
            "bytes": 1000,
            "last_modified": "2026-09-01 00:00:00",
            "ingested_at": "2026-09-01T00:00:00Z",
        })],
    );
    write_catalog_stats_epoch(
        metadata.path(),
        2,
        &[json!({
            "unique_id": "model.pkg.a",
            "table_type": "BASE TABLE",
            "database_name": "db",
            "schema_name": "sch",
            "table_name": "a",
            "row_count": 200,
            "bytes": 2000,
            "last_modified": "2026-09-08 00:00:00",
            "ingested_at": "2026-09-08T00:00:00Z",
        })],
    );

    let rows = materialize(super::Materializer::Copy, metadata.path());
    assert_eq!(
        rows.len(),
        1,
        "a re-catalogued relation must not keep its stale row: {rows:?}"
    );
    assert_eq!(rows[0]["row_count"], json!(200));
}
