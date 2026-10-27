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

fn write_constraints_corpus(metadata_dir: &Path) {
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC")));
    let utf8 = |name: &str| Field::new(name, DataType::Utf8, true);
    let list = |name: &str| {
        Field::new(
            name,
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        )
    };

    let nodes_dir = metadata_dir.join(crate::ingest::PARSE_NODES_SUBDIR);
    std::fs::create_dir_all(&nodes_dir).unwrap();
    let nodes_schema = Arc::new(Schema::new(vec![
        Field::new("unique_id", DataType::Utf8, false),
        utf8("resource_type"),
        utf8("name"),
        utf8("package_name"),
        utf8("original_path"),
        utf8("description"),
        utf8("database"),
        utf8("schema"),
        utf8("alias"),
        utf8("relation_name"),
        utf8("identifier"),
        utf8("materialization"),
        utf8("access"),
        utf8("group_name"),
        utf8("source_name"),
        list("tags"),
        list("fqn"),
        list("depends_on"),
        Field::new("ingested_at", ts.clone(), false),
        Field::new("is_disabled", DataType::Int32, true),
        utf8("payload"),
    ]));
    let model_constraints = json!([{"type": "primary_key", "columns": ["order_id"]}]);
    let payload = json!({"__model_attr__": {"constraints": model_constraints}});
    crate::parquet::write_table(
        &nodes_dir.join("v1_0.parquet"),
        nodes_schema,
        &[json!({
            "unique_id": "model.pkg.orders",
            "resource_type": "model",
            "name": "orders",
            "package_name": "pkg",
            "tags": [],
            "fqn": ["pkg", "orders"],
            "depends_on": [],
            "ingested_at": "2026-09-08T12:00:00Z",
            "is_disabled": 0,
            "payload": payload.to_string(),
        })],
    )
    .unwrap();

    let columns_dir = metadata_dir.join(crate::ingest::PARSE_COLUMNS_SUBDIR);
    std::fs::create_dir_all(&columns_dir).unwrap();
    let columns_schema = Arc::new(Schema::new(vec![
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("column_name", DataType::Utf8, false),
        utf8("description"),
        utf8("declared_type"),
        Field::new("is_primary_key", DataType::Boolean, false),
        utf8("constraints"),
        utf8("meta"),
        list("tags"),
        utf8("granularity"),
        Field::new("ingested_at", ts.clone(), false),
    ]));
    let row = |name: &str, pk: bool, constraints: Option<Value>| {
        json!({
            "unique_id": "model.pkg.orders",
            "column_name": name,
            "declared_type": "int",
            "is_primary_key": pk,
            "constraints": constraints.map(|c| c.to_string()),
            "tags": [],
            "ingested_at": "2026-09-08T12:00:00Z",
        })
    };
    crate::parquet::write_table(
        &columns_dir.join("v1_0.parquet"),
        columns_schema,
        &[
            row(
                "order_id",
                true,
                Some(json!([{"type": "not_null"}, {"type": "primary_key"}])),
            ),
            row("customer_id", false, None),
        ],
    )
    .unwrap();

    // Columns with no YAML entry: the compile and catalog merges append rows
    // for them rather than updating a parse row.
    let compile_dir = metadata_dir.join(crate::ingest::COMPILE_COLUMNS_SUBDIR);
    std::fs::create_dir_all(&compile_dir).unwrap();
    let compile_schema = Arc::new(Schema::new(vec![
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("column_name", DataType::Utf8, false),
        Field::new("column_index", DataType::Int32, false),
        utf8("column_type"),
        utf8("description"),
        list("classifiers"),
        Field::new("ingested_at", ts.clone(), false),
    ]));
    crate::parquet::write_table(
        &compile_dir.join("v1_0.parquet"),
        compile_schema,
        &[json!({
            "unique_id": "model.pkg.orders",
            "column_name": "created_at",
            "column_index": 2,
            "column_type": "timestamp",
            "classifiers": [],
            "ingested_at": "2026-09-08T12:00:00Z",
        })],
    )
    .unwrap();

    let catalog_dir = metadata_dir.join(crate::ingest::CATALOG_COLUMNS_SUBDIR);
    std::fs::create_dir_all(&catalog_dir).unwrap();
    let catalog_schema = Arc::new(Schema::new(vec![
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("column_name", DataType::Utf8, false),
        Field::new("column_index", DataType::Int32, false),
        utf8("catalog_type"),
        utf8("catalog_comment"),
        Field::new("ingested_at", ts, false),
    ]));
    crate::parquet::write_table(
        &catalog_dir.join("v1_0.parquet"),
        catalog_schema,
        &[json!({
            "unique_id": "model.pkg.orders",
            "column_name": "updated_at",
            "column_index": 3,
            "catalog_type": "TIMESTAMP",
            "ingested_at": "2026-09-08T12:00:00Z",
        })],
    )
    .unwrap();
}

/// `constraints` must reach `dbt.node_columns` and `dbt.models` on both
/// materializers (dbt-labs/dbt#16553). They are separate code: COPY reads the
/// epoch files through the epoch views, Arrow goes through the ingest writer.
#[test]
fn constraints_reach_node_columns_and_models() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    write_constraints_corpus(metadata.path());

    for how in [super::Materializer::Arrow, super::Materializer::Copy] {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        super::write_info_schema_with(how, metadata.path(), root.path(), staging.path()).unwrap();
        let dir = super::versioned_dir(root.path());

        let columns = super::tests::out_rows(&dir, "dbt.node_columns.parquet");
        let constraints_of = |name: &str| {
            columns
                .iter()
                .find(|r| r["column_name"] == json!(name))
                .unwrap_or_else(|| panic!("no node_columns row for {name}: {columns:?}"))
                .get("constraints")
                .cloned()
                .unwrap_or(Value::Null)
        };
        let order_id: Value =
            serde_json::from_str(constraints_of("order_id").as_str().unwrap_or("null")).unwrap();
        assert_eq!(
            order_id,
            json!([{"type": "not_null"}, {"type": "primary_key"}]),
            "{how:?}: node_columns.constraints"
        );
        assert_eq!(
            constraints_of("customer_id"),
            json!("[]"),
            "{how:?}: a column without constraints is '[]', like manifest.json"
        );
        for name in ["created_at", "updated_at"] {
            assert_eq!(
                constraints_of(name),
                json!("[]"),
                "{how:?}: {name} has no parse row, and is still '[]'"
            );
        }

        let models = super::tests::out_rows(&dir, "dbt.models.parquet");
        let model: Value = serde_json::from_str(
            models[0]
                .get("constraints")
                .and_then(Value::as_str)
                .unwrap_or("null"),
        )
        .unwrap();
        assert_eq!(
            model,
            json!([{"type": "primary_key", "columns": ["order_id"]}]),
            "{how:?}: models.constraints"
        );
    }
}

fn write_meta_config_corpus(metadata_dir: &Path) {
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC")));
    let utf8 = |name: &str| Field::new(name, DataType::Utf8, true);
    let list = |name: &str| {
        Field::new(
            name,
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        )
    };

    let nodes_dir = metadata_dir.join(crate::ingest::PARSE_NODES_SUBDIR);
    std::fs::create_dir_all(&nodes_dir).unwrap();
    let nodes_schema = Arc::new(Schema::new(vec![
        Field::new("unique_id", DataType::Utf8, false),
        utf8("resource_type"),
        utf8("name"),
        utf8("package_name"),
        utf8("original_path"),
        utf8("description"),
        utf8("database"),
        utf8("schema"),
        utf8("alias"),
        utf8("relation_name"),
        utf8("identifier"),
        utf8("materialization"),
        utf8("access"),
        utf8("group_name"),
        utf8("source_name"),
        list("tags"),
        list("fqn"),
        list("depends_on"),
        Field::new("ingested_at", ts.clone(), false),
        Field::new("is_disabled", DataType::Int32, true),
        utf8("payload"),
    ]));
    let node = |uid: &str, resource_type: &str, name: &str, payload: Value| {
        json!({
            "unique_id": uid,
            "resource_type": resource_type,
            "name": name,
            "package_name": "pkg",
            "tags": [],
            "fqn": ["pkg", name],
            "depends_on": [],
            "ingested_at": "2026-09-08T12:00:00Z",
            "is_disabled": 0,
            "payload": payload.to_string(),
        })
    };
    crate::parquet::write_table(
        &nodes_dir.join("v1_0.parquet"),
        nodes_schema,
        &[
            node(
                "model.pkg.orders",
                "model",
                "orders",
                json!({
                    "config": {"enabled": true, "materialized": "view", "meta": {"owner_team": "data"}},
                    "__model_attr__": {},
                    "__common_attr__": {"name": "orders"},
                }),
            ),
            node(
                "exposure.pkg.with_meta",
                "exposure",
                "with_meta",
                json!({
                    "__common_attr__": {"name": "with_meta", "meta": {"owner_team": "data"}},
                    "config": {"enabled": true, "meta": {"owner_team": "data"}},
                }),
            ),
            node(
                "exposure.pkg.disabled_one",
                "exposure",
                "disabled_one",
                json!({
                    "__common_attr__": {"name": "disabled_one", "meta": {}},
                    "config": {"enabled": false, "meta": {}},
                }),
            ),
            node(
                "unit_test.pkg.orders.ut_orders",
                "unit_test",
                "ut_orders",
                json!({
                    "__common_attr__": {"name": "ut_orders"},
                    "config": {"enabled": true, "meta": {"owner_team": "data"}, "tags": ["nightly"]},
                }),
            ),
            // Fusion serialises these two nodes' config as `deprecated_config`.
            node(
                "saved_query.pkg.sq_orders",
                "saved_query",
                "sq_orders",
                json!({
                    "__common_attr__": {"name": "sq_orders"},
                    "deprecated_config": {"enabled": true, "meta": {"owner_team": "data"}, "tags": ["nightly"]},
                }),
            ),
            node(
                "semantic_model.pkg.orders_sm",
                "semantic_model",
                "orders_sm",
                json!({
                    "__common_attr__": {"name": "orders_sm"},
                    "deprecated_config": {"enabled": true, "meta": {"owner_team": "data"}},
                    "__semantic_model_attr__": {
                        "model": "ref('orders')",
                        "entities": [
                            {"name": "order", "type": "primary", "config": {"meta": {"kind": "pk"}}},
                        ],
                        "measures": [{"name": "order_count", "agg": "count", "config": null}],
                        "dimensions": [
                            {"name": "ordered_at", "type": "time", "config": {"meta": {"kind": "time"}}},
                        ],
                    },
                }),
            ),
        ],
    )
    .unwrap();

    let columns_dir = metadata_dir.join(crate::ingest::PARSE_COLUMNS_SUBDIR);
    std::fs::create_dir_all(&columns_dir).unwrap();
    let columns_schema = Arc::new(Schema::new(vec![
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("column_name", DataType::Utf8, false),
        utf8("description"),
        utf8("declared_type"),
        Field::new("is_primary_key", DataType::Boolean, false),
        utf8("constraints"),
        utf8("meta"),
        list("tags"),
        utf8("granularity"),
        Field::new("ingested_at", ts, false),
    ]));
    let row = |name: &str, meta: Option<Value>| {
        json!({
            "unique_id": "model.pkg.orders",
            "column_name": name,
            "declared_type": "int",
            "is_primary_key": false,
            "meta": meta.map(|m| m.to_string()),
            "tags": [],
            "ingested_at": "2026-09-08T12:00:00Z",
        })
    };
    crate::parquet::write_table(
        &columns_dir.join("v1_0.parquet"),
        columns_schema,
        &[
            row("order_id", Some(json!({"pii": false}))),
            row("customer_id", None),
        ],
    )
    .unwrap();
}

/// `meta` and `config` must reach `dbt.exposures`, `dbt.unit_tests`, `dbt.node_columns`,
/// `dbt.saved_queries` and the semantic tables on both materializers (dbt-labs/dbt#16584). They are
/// separate code: COPY reads the epoch files through the epoch views, Arrow goes
/// through the ingest writer.
#[test]
fn meta_and_config_reach_exposures_unit_tests_and_node_columns() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    write_meta_config_corpus(metadata.path());

    let json_of = |row: &Value, col: &str| -> Value {
        serde_json::from_str(row.get(col).and_then(Value::as_str).unwrap_or("null")).unwrap()
    };
    for how in [super::Materializer::Arrow, super::Materializer::Copy] {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        super::write_info_schema_with(how, metadata.path(), root.path(), staging.path()).unwrap();
        let dir = super::versioned_dir(root.path());

        let exposures = super::tests::out_rows(&dir, "dbt.exposures.parquet");
        let exposure = |name: &str| {
            exposures
                .iter()
                .find(|r| r["name"] == json!(name))
                .unwrap_or_else(|| panic!("{how:?}: no exposure {name}: {exposures:?}"))
        };
        assert_eq!(
            json_of(exposure("with_meta"), "meta"),
            json!({"owner_team": "data"}),
            "{how:?}: exposures.meta"
        );
        assert_eq!(
            json_of(exposure("with_meta"), "config")["meta"],
            json!({"owner_team": "data"}),
            "{how:?}: exposures.config"
        );
        assert_eq!(
            json_of(exposure("disabled_one"), "config")["enabled"],
            json!(false),
            "{how:?}: a disabled exposure is visible in config"
        );

        let unit_tests = super::tests::out_rows(&dir, "dbt.unit_tests.parquet");
        let config = json_of(&unit_tests[0], "config");
        assert_eq!(config["meta"], json!({"owner_team": "data"}), "{how:?}");
        assert_eq!(
            config["tags"],
            json!(["nightly"]),
            "{how:?}: unit_tests.config"
        );

        // A model's `config` is the object's JSON text, like every other node's. It was once
        // the text encoded a second time on COPY, so `json_extract` found nothing in it.
        let models = super::tests::out_rows(&dir, "dbt.models.parquet");
        assert_eq!(
            json_of(&models[0], "config"),
            json!({"enabled": true, "materialized": "view", "meta": {"owner_team": "data"}}),
            "{how:?}: models.config"
        );

        let first = |file: &str| super::tests::out_rows(&dir, file).remove(0);
        let sq = first("dbt.saved_queries.parquet");
        assert_eq!(
            json_of(&sq, "config")["meta"],
            json!({"owner_team": "data"}),
            "{how:?}: saved_queries.config"
        );
        assert_eq!(
            json_of(&sq, "config")["tags"],
            json!(["nightly"]),
            "{how:?}"
        );
        let sm = first("dbt.semantic_models.parquet");
        assert_eq!(
            json_of(&sm, "config")["meta"],
            json!({"owner_team": "data"}),
            "{how:?}: semantic_models.config"
        );
        assert_eq!(
            json_of(&first("dbt.semantic_entities.parquet"), "config"),
            json!({"meta": {"kind": "pk"}}),
            "{how:?}: semantic_entities.config"
        );
        assert_eq!(
            json_of(&first("dbt.semantic_dimensions.parquet"), "config"),
            json!({"meta": {"kind": "time"}}),
            "{how:?}: semantic_dimensions.config"
        );
        assert_eq!(
            first("dbt.semantic_measures.parquet").get("config"),
            Some(&Value::Null),
            "{how:?}: a measure without config is NULL, not the text 'null'"
        );

        let columns = super::tests::out_rows(&dir, "dbt.node_columns.parquet");
        let meta_of = |name: &str| {
            let row = columns
                .iter()
                .find(|r| r["column_name"] == json!(name))
                .unwrap_or_else(|| panic!("{how:?}: no node_columns row for {name}"));
            json_of(row, "meta")
        };
        assert_eq!(
            meta_of("order_id"),
            json!({"pii": false}),
            "{how:?}: node_columns.meta"
        );
        assert_eq!(
            meta_of("customer_id"),
            json!({}),
            "{how:?}: a column without meta is '{{}}', like manifest.json"
        );
    }
}

/// YAML-declared columns in manifest casing, and a Snowflake-style catalog that
/// uppercases every name and also has a column the YAML never declared.
fn write_catalog_casing_corpus(metadata_dir: &Path) {
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC")));
    let utf8 = |name: &str| Field::new(name, DataType::Utf8, true);

    let columns_dir = metadata_dir.join(crate::ingest::PARSE_COLUMNS_SUBDIR);
    std::fs::create_dir_all(&columns_dir).unwrap();
    let columns_schema = Arc::new(Schema::new(vec![
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("column_name", DataType::Utf8, false),
        utf8("description"),
        utf8("declared_type"),
        Field::new("is_primary_key", DataType::Boolean, false),
        utf8("constraints"),
        utf8("meta"),
        Field::new(
            "tags",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        ),
        utf8("granularity"),
        Field::new("ingested_at", ts.clone(), false),
    ]));
    let parse_row = |name: &str| {
        json!({
            "unique_id": "model.pkg.typed",
            "column_name": name,
            "description": format!("the {name}"),
            "is_primary_key": false,
            "tags": [],
            "ingested_at": "2026-09-08T12:00:00Z",
        })
    };
    crate::parquet::write_table(
        &columns_dir.join("v1_0.parquet"),
        columns_schema,
        &[parse_row("id"), parse_row("name")],
    )
    .unwrap();

    let catalog_dir = metadata_dir.join(crate::ingest::CATALOG_COLUMNS_SUBDIR);
    std::fs::create_dir_all(&catalog_dir).unwrap();
    let catalog_schema = Arc::new(Schema::new(vec![
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("column_name", DataType::Utf8, false),
        Field::new("column_index", DataType::Int32, false),
        utf8("catalog_type"),
        utf8("catalog_comment"),
        Field::new("ingested_at", ts, false),
    ]));
    let catalog_row = |name: &str, index: i32, ty: &str| {
        json!({
            "unique_id": "model.pkg.typed",
            "column_name": name,
            "column_index": index,
            "catalog_type": ty,
            "ingested_at": "2026-09-08T12:00:00Z",
        })
    };
    crate::parquet::write_table(
        &catalog_dir.join("v1_0.parquet"),
        catalog_schema,
        &[
            catalog_row("ID", 0, "NUMBER"),
            catalog_row("NAME", 1, "TEXT"),
            catalog_row("TS", 2, "TIMESTAMP_LTZ"),
        ],
    )
    .unwrap();
}

/// Catalog types reach the declared columns whatever the warehouse's casing,
/// and undeclared catalog columns get their own row, on both materializers
/// (dbt-labs/dbt#16514).
#[test]
fn catalog_types_match_declared_columns_case_insensitively() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    write_catalog_casing_corpus(metadata.path());

    for how in [super::Materializer::Arrow, super::Materializer::Copy] {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        super::write_info_schema_with(how, metadata.path(), root.path(), staging.path()).unwrap();
        let dir = super::versioned_dir(root.path());

        let mut columns: Vec<_> = super::tests::out_rows(&dir, "dbt.node_columns.parquet")
            .into_iter()
            .map(|r| {
                (
                    r["column_name"].clone(),
                    r["data_type_actual"].clone(),
                    r["data_type"].clone(),
                    r["description"].clone(),
                )
            })
            .collect();
        columns.sort_by_key(|c| c.0.to_string());
        assert_eq!(
            columns,
            vec![
                (
                    json!("TS"),
                    json!("TIMESTAMP_LTZ"),
                    json!("TIMESTAMP_LTZ"),
                    Value::Null
                ),
                (
                    json!("id"),
                    json!("NUMBER"),
                    json!("NUMBER"),
                    json!("the id")
                ),
                (
                    json!("name"),
                    json!("TEXT"),
                    json!("TEXT"),
                    json!("the name")
                ),
            ],
            "{how:?}: one row per column, typed from the catalog"
        );
    }
}
