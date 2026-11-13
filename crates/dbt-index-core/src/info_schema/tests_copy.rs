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
        Field::new("quote", DataType::Boolean, true),
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
            {
                let mut order_id = row(
                    "order_id",
                    true,
                    Some(json!([{"type": "not_null"}, {"type": "primary_key"}])),
                );
                order_id["granularity"] = json!("day");
                order_id
            },
            {
                let mut customer_id = row("customer_id", false, None);
                customer_id["quote"] = json!(true);
                customer_id
            },
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

/// A column's `granularity` and `quote` reach `dbt.node_columns` from the parse epoch.
#[test]
fn column_granularity_and_quote_reach_node_columns() {
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
        let col = |name: &str| {
            columns
                .iter()
                .find(|r| r["column_name"] == json!(name))
                .unwrap_or_else(|| panic!("{how:?}: no row for {name}"))
        };
        assert_eq!(col("order_id")["granularity"], json!("day"), "{how:?}");
        assert_eq!(col("order_id")["quote"], Value::Null, "{how:?}");
        assert_eq!(col("customer_id")["quote"], json!(true), "{how:?}");
        assert_eq!(col("customer_id")["granularity"], Value::Null, "{how:?}");
    }
}

/// The root project's `quoting` reaches `dbt.project`, and a resolver state written before it
/// was recorded still reads, as NULL.
#[test]
fn project_quoting_reaches_dbt_project() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC")));
    let utf8 = |name: &str| Field::new(name, DataType::Utf8, true);
    let generation = Arc::new(Schema::new(vec![
        utf8("project_name"),
        utf8("dbt_version"),
        utf8("adapter_type"),
        utf8("vars_json"),
        utf8("env_vars_json"),
        Field::new("ingested_at", ts, false),
    ]));
    let resolver = |with_quoting: bool| {
        let mut fields = vec![
            utf8("vars_json"),
            utf8("env_vars_json"),
            utf8("git_sha"),
            utf8("git_branch"),
            Field::new("git_is_dirty", DataType::Int32, true),
            utf8("pkg_kinds_json"),
            utf8("pkg_lock_json"),
        ];
        if with_quoting {
            fields.push(utf8("quoting_json"));
        }
        Arc::new(Schema::new(fields))
    };
    let quoting = r#"{"database":true,"identifier":false,"schema":true}"#;
    for with_quoting in [true, false] {
        let metadata = tempfile::tempdir().unwrap();
        crate::parquet::write_table(
            &metadata.path().join(crate::ingest::PARSE_GENERATION),
            generation.clone(),
            &[json!({
                "project_name": "pkg",
                "dbt_version": "2.0",
                "adapter_type": "duckdb",
                "ingested_at": "2026-09-08T12:00:00Z",
            })],
        )
        .unwrap();
        let mut state = json!({"pkg_kinds_json": "{}", "pkg_lock_json": "{}"});
        if with_quoting {
            state["quoting_json"] = json!(quoting);
        }
        crate::parquet::write_table(
            &metadata.path().join(crate::ingest::PARSE_RESOLVER_STATE),
            resolver(with_quoting),
            &[state],
        )
        .unwrap();
        for how in [super::Materializer::Arrow, super::Materializer::Copy] {
            let root = tempfile::tempdir().unwrap();
            let staging = tempfile::tempdir().unwrap();
            super::write_info_schema_with(how, metadata.path(), root.path(), staging.path())
                .unwrap();
            let dir = super::versioned_dir(root.path());
            let project = super::tests::out_rows(&dir, "dbt.project.parquet").remove(0);
            let want = if with_quoting {
                json!(quoting)
            } else {
                Value::Null
            };
            assert_eq!(
                project["quoting"], want,
                "{how:?} (recorded: {with_quoting})"
            );
        }
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

/// The freshness columns an epoch records reach `dbt_rt.freshness`, a failed check is a row
/// carrying its error, and an epoch written before the columns existed still reads.
#[test]
fn freshness_details_reach_freshness_and_old_epochs_still_read() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    let dir = metadata.path().join(crate::ingest::RUN_FRESHNESS_SUBDIR);
    std::fs::create_dir_all(&dir).unwrap();
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC")));
    let utf8 = |name: &str| Field::new(name, DataType::Utf8, true);
    let mut old_fields = vec![
        Field::new("invocation_id", DataType::Utf8, false),
        Field::new("unique_id", DataType::Utf8, false),
        utf8("resource_type"),
        Field::new("status", DataType::Utf8, false),
        utf8("max_loaded_at"),
        utf8("snapshotted_at"),
        Field::new("max_loaded_at_time_ago", DataType::Float64, true),
        Field::new("execution_time", DataType::Float64, true),
        Field::new("warn_after_count", DataType::Int32, true),
        utf8("warn_after_period"),
        Field::new("error_after_count", DataType::Int32, true),
        utf8("error_after_period"),
    ];
    let mut new_fields = old_fields.clone();
    new_fields.extend(
        [
            "freshness_filter",
            "thread_id",
            "adapter_response",
            "timing",
            "error",
        ]
        .map(utf8),
    );
    let schema = |fields: &mut Vec<Field>| {
        let mut f = fields.clone();
        f.push(Field::new("ingested_at", ts.clone(), false));
        Arc::new(Schema::new(f))
    };
    let row = |id: &str, uid: &str, status: &str| {
        json!({
            "invocation_id": id,
            "unique_id": uid,
            "resource_type": "source",
            "status": status,
            "ingested_at": "2026-09-08T12:00:00Z",
        })
    };
    crate::parquet::write_table(
        &dir.join("v1_0.parquet"),
        schema(&mut old_fields),
        &[row("old", "source.pkg.s.old", "pass")],
    )
    .unwrap();
    let mut measured = row("new", "source.pkg.s.ok", "pass");
    measured["execution_time"] = json!(1.5);
    measured["freshness_filter"] = json!("id > 0");
    measured["thread_id"] = json!("Thread-3");
    measured["adapter_response"] = json!("{}");
    measured["timing"] = json!("[]");
    let mut failed = row("new", "source.pkg.s.bad", "error");
    failed["error"] = json!("relation does not exist");
    crate::parquet::write_table(
        &dir.join("v1_1.parquet"),
        schema(&mut new_fields),
        &[measured, failed],
    )
    .unwrap();
    for how in [super::Materializer::Arrow, super::Materializer::Copy] {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        super::write_info_schema_with(how, metadata.path(), root.path(), staging.path()).unwrap();
        let out = super::versioned_dir(root.path());
        let rows = super::tests::out_rows(&out, "dbt_rt.freshness.parquet");
        let by_id = |uid: &str| {
            rows.iter()
                .find(|r| r["unique_id"] == json!(uid))
                .unwrap_or_else(|| panic!("{how:?}: no {uid}"))
        };
        let ok = by_id("source.pkg.s.ok");
        assert_eq!(ok["execution_time"], json!(1.5), "{how:?}");
        assert_eq!(ok["freshness_filter"], json!("id > 0"), "{how:?}");
        assert_eq!(ok["thread_id"], json!("Thread-3"), "{how:?}");
        assert_eq!(ok["adapter_response"], json!("{}"), "{how:?}");
        assert_eq!(ok["timing"], json!("[]"), "{how:?}");
        assert_eq!(ok["error"], Value::Null, "{how:?}");
        let bad = by_id("source.pkg.s.bad");
        assert_eq!(bad["status"], json!("error"), "{how:?}");
        assert_eq!(bad["error"], json!("relation does not exist"), "{how:?}");
        let old = by_id("source.pkg.s.old");
        assert_eq!(old["thread_id"], Value::Null, "{how:?}");
        assert_eq!(old["error"], Value::Null, "{how:?}");
    }
}

/// A freshness epoch written after the information schema was built reaches
/// `dbt_rt.freshness` through `refresh_freshness`, and a directory no write finished is left
/// alone.
#[test]
fn refresh_freshness_brings_the_table_up_to_date() {
    let metadata = tempfile::tempdir().unwrap();
    let dir = metadata.path().join(crate::ingest::RUN_FRESHNESS_SUBDIR);
    std::fs::create_dir_all(&dir).unwrap();
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC")));
    let utf8 = |name: &str| Field::new(name, DataType::Utf8, true);
    let schema = Arc::new(Schema::new(vec![
        Field::new("invocation_id", DataType::Utf8, false),
        Field::new("unique_id", DataType::Utf8, false),
        utf8("resource_type"),
        Field::new("status", DataType::Utf8, false),
        Field::new("ingested_at", ts, false),
    ]));
    let row = |id: &str| {
        json!({
            "invocation_id": id,
            "unique_id": format!("source.pkg.s.{id}"),
            "resource_type": "source",
            "status": "pass",
            "ingested_at": "2026-09-08T12:00:00Z",
        })
    };
    let root = tempfile::tempdir().unwrap();
    // Nothing was written yet: there is nothing to refresh, and nothing is created.
    super::refresh_freshness(metadata.path(), root.path()).unwrap();
    assert!(!super::versioned_dir(root.path()).exists());

    crate::parquet::write_table(&dir.join("v1_0.parquet"), schema.clone(), &[row("before")])
        .unwrap();
    let staging = tempfile::tempdir().unwrap();
    super::write_info_schema_with(
        super::Materializer::Arrow,
        metadata.path(),
        root.path(),
        staging.path(),
    )
    .unwrap();
    let out = super::versioned_dir(root.path());
    let ids = |rows: Vec<Value>| -> Vec<String> {
        let mut ids: Vec<String> = rows
            .iter()
            .map(|r| r["invocation_id"].as_str().unwrap().to_string())
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(
        ids(super::tests::out_rows(&out, "dbt_rt.freshness.parquet")),
        vec!["before"]
    );

    crate::parquet::write_table(&dir.join("v1_1.parquet"), schema, &[row("after")]).unwrap();
    super::refresh_freshness(metadata.path(), root.path()).unwrap();
    assert_eq!(
        ids(super::tests::out_rows(&out, "dbt_rt.freshness.parquet")),
        vec!["after", "before"]
    );
}

/// Parse nodes whose `config` sets the columns `dbt.nodes` used to leave NULL on COPY.
fn write_node_config_corpus(metadata_dir: &Path) {
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
        Field::new("ingested_at", ts, false),
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
                "model.pkg.m_inc",
                "model",
                "m_inc",
                json!({
                    "config": {
                        "incremental_strategy": "delete+insert",
                        "on_schema_change": "fail",
                        "unique_key": "id",
                        "full_refresh": false,
                        "persist_docs": {"relation": true},
                        "pre_hook": [{"sql": "select 1"}],
                        "post-hook": [{"sql": "select 2"}],
                        "grants": {"select": ["analyst"]},
                        "docs": {"show": false},
                    },
                    "__model_attr__": {"time_spine": {"standard_granularity_column": "d"}},
                    "__common_attr__": {"name": "m_inc"},
                }),
            ),
            // The payload keeps `fqn` in `__common_attr__` and `depends_on` in
            // `__base_attr__` for every type below.
            node(
                "exposure.pkg.ex",
                "exposure",
                "ex",
                json!({
                    "__common_attr__": {"name": "ex", "fqn": ["pkg", "ex"]},
                    "__base_attr__": {"depends_on": {"nodes": ["model.pkg.m_inc"]}},
                }),
            ),
            node(
                "saved_query.pkg.sq",
                "saved_query",
                "sq",
                json!({
                    "__common_attr__": {"name": "sq", "fqn": ["pkg", "sq"], "tags": ["t1"]},
                    "__base_attr__": {"depends_on": {"nodes": ["metric.pkg.mt"]}},
                    "deprecated_config": {"enabled": true},
                }),
            ),
            node(
                "metric.pkg.mt",
                "metric",
                "mt",
                json!({
                    "__common_attr__": {
                        "name": "mt",
                        "fqn": ["pkg", "mt"],
                        "meta": {"owner": "me"},
                    },
                    "__metric_attr__": {"metric_type": "simple"},
                    "deprecated_config": {"enabled": true},
                }),
            ),
            node(
                "group.pkg.grp",
                "group",
                "grp",
                json!({
                    "__common_attr__": {
                        "name": "grp",
                        "package_name": "pkg",
                        "original_file_path": "models/groups.yml",
                        "meta": {"tier": 1},
                    },
                    "__group_attr__": {"owner": {"name": "o"}},
                }),
            ),
            node(
                "semantic_model.pkg.sm",
                "semantic_model",
                "sm",
                json!({
                    "__common_attr__": {
                        "name": "sm",
                        "package_name": "pkg",
                        "original_file_path": "models/sm.yml",
                    },
                    "__semantic_model_attr__": {
                        "model": "ref('m_inc')",
                        "dimensions": [{
                            "name": "valid_from",
                            "type": "time",
                            "type_params": {
                                "time_granularity": "day",
                                "validity_params": {"is_start": true},
                            },
                        }],
                    },
                }),
            ),
            // A generic test: its own path is the generated SQL, its config is under `config`.
            node(
                "test.pkg.not_null_m_inc_id",
                "test",
                "not_null_m_inc_id",
                json!({
                    "__common_attr__": {
                        "name": "not_null_m_inc_id",
                        "original_file_path": "target/generic_tests/not_null.sql",
                        "fqn": ["pkg", "not_null_m_inc_id"],
                    },
                    "manifest_original_file_path": "models/m_inc.yml",
                    "__test_attr__": {
                        "test_metadata": {"name": "not_null", "kwargs": {}},
                        "test_subject_unique_id": "model.pkg.m_inc",
                    },
                    "config": {
                        "severity": "warn",
                        "warn_if": ">1",
                        "error_if": ">2",
                        "fail_calc": "sum(x)",
                        "store_failures": true,
                        "store_failures_as": "view",
                        "where": "id > 0",
                        "limit": 5,
                    },
                }),
            ),
            node(
                "source.pkg.src.tbl",
                "source",
                "tbl",
                json!({
                    "__common_attr__": {"name": "tbl"},
                    "__source_attr__": {
                        "source_name": "src",
                        "loaded_at_query": "select max(ts) from t",
                        "freshness": {"warn_after": {"count": 1, "period": "day"}},
                        "external": {"location": "s3://b"},
                        "user_quoting": {"database": true},
                    },
                }),
            ),
            node(
                "unit_test.pkg.m_inc.ut",
                "unit_test",
                "ut",
                json!({
                    "__common_attr__": {"name": "ut", "fqn": ["pkg", "ut"]},
                }),
            ),
            // An unversioned model carries `version: null` under `__model_attr__`; a
            // versioned one a number, and one a string.
            node(
                "model.pkg.m_v_null",
                "model",
                "m_v_null",
                json!({
                    "__model_attr__": {"version": null, "latest_version": null},
                    "__common_attr__": {"name": "m_v_null"},
                }),
            ),
            node(
                "model.pkg.m_v_num",
                "model",
                "m_v_num",
                json!({
                    "__model_attr__": {"version": 2, "latest_version": 3},
                    "__common_attr__": {"name": "m_v_num"},
                }),
            ),
            node(
                "model.pkg.m_v_str",
                "model",
                "m_v_str",
                json!({
                    "__model_attr__": {"version": "v1", "latest_version": "v1"},
                    "__common_attr__": {"name": "m_v_str"},
                }),
            ),
            // A composite key is not a string, and empty containers are unset.
            node(
                "snapshot.pkg.snap_empty",
                "snapshot",
                "snap_empty",
                json!({
                    "config": {
                        "unique_key": ["a", "b"],
                        "grants": {},
                        "pre_hook": [],
                    },
                    "__common_attr__": {"name": "snap_empty"},
                }),
            ),
            node(
                "snapshot.pkg.snap_full",
                "snapshot",
                "snap_full",
                json!({
                    "config": {
                        "unique_key": "id",
                        "full_refresh": true,
                        "persist_docs": {"relation": true},
                        "pre_hook": [{"sql": "select 1"}],
                        "post-hook": [{"sql": "select 2"}],
                        "grants": {"select": ["analyst"]},
                        "docs": {"show": false},
                        "group": "finance",
                    },
                    "__common_attr__": {"name": "snap_full"},
                }),
            ),
            node(
                "seed.pkg.s",
                "seed",
                "s",
                json!({
                    "config": {
                        "unique_key": "id",
                        "full_refresh": true,
                        "persist_docs": {"relation": true},
                        "pre_hook": [{"sql": "select 1"}],
                        "post-hook": [{"sql": "select 2"}],
                        "grants": {"select": ["analyst"]},
                        "docs": {"show": false},
                        "group": "finance",
                    },
                    "__common_attr__": {"name": "s"},
                }),
            ),
            node(
                "function.pkg.f",
                "function",
                "f",
                json!({
                    "config": {
                        "unique_key": "id",
                        "full_refresh": true,
                        "persist_docs": {"relation": true},
                        "pre_hook": [{"sql": "select 1"}],
                        "post-hook": [{"sql": "select 2"}],
                        "grants": {"select": ["analyst"]},
                        "docs": {"show": false},
                        "group": "finance",
                    },
                    "__common_attr__": {"name": "f"},
                }),
            ),
            node(
                "analysis.pkg.a",
                "analysis",
                "a",
                json!({
                    "config": {
                        "unique_key": "id",
                        "full_refresh": true,
                        "persist_docs": {"relation": true},
                        "pre_hook": [{"sql": "select 1"}],
                        "post-hook": [{"sql": "select 2"}],
                        "grants": {"select": ["analyst"]},
                        "docs": {"show": false},
                        "group": "finance",
                    },
                    "__common_attr__": {"name": "a"},
                }),
            ),
        ],
    )
    .unwrap();
}

/// `dbt.nodes` columns that come from a node's `config` must be filled on COPY as they are on
/// Arrow, and `group` must read the config for the types that keep it only there.
#[test]
fn config_columns_reach_node_tables_on_both_materializers() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    write_node_config_corpus(metadata.path());

    let json_of = |row: &Value, col: &str| -> Value {
        serde_json::from_str(row.get(col).and_then(Value::as_str).unwrap_or("null")).unwrap()
    };
    for how in [super::Materializer::Arrow, super::Materializer::Copy] {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        super::write_info_schema_with(how, metadata.path(), root.path(), staging.path()).unwrap();
        let dir = super::versioned_dir(root.path());
        let first = |file: &str| super::tests::out_rows(&dir, file).remove(0);
        let named = |file: &str, name: &str| {
            super::tests::out_rows(&dir, file)
                .into_iter()
                .find(|r| r["name"] == json!(name))
                .unwrap_or_else(|| panic!("{how:?}: no {name} in {file}"))
        };
        // The same `config` on every type below, so one set of expectations covers them.
        let assert_hooks_grants_docs = |row: &Value, label: &str| {
            assert_eq!(
                json_of(row, "pre_hook"),
                json!([{"sql": "select 1"}]),
                "{how:?}: {label}"
            );
            assert_eq!(
                json_of(row, "post_hook"),
                json!([{"sql": "select 2"}]),
                "{how:?}: {label}: the dashed spelling is read"
            );
            assert_eq!(
                json_of(row, "grants"),
                json!({"select": ["analyst"]}),
                "{how:?}: {label}"
            );
            assert_eq!(row["docs_show"], json!(false), "{how:?}: {label}");
        };

        let model = named("dbt.models.parquet", "m_inc");
        assert_eq!(
            model["incremental_strategy"],
            json!("delete+insert"),
            "{how:?}"
        );
        assert_eq!(model["on_schema_change"], json!("fail"), "{how:?}");
        assert_eq!(model["unique_key"], json!("id"), "{how:?}");
        assert_eq!(model["full_refresh"], json!(false), "{how:?}");
        assert_eq!(model["docs_show"], json!(false), "{how:?}");
        assert_eq!(
            json_of(&model, "persist_docs"),
            json!({"relation": true}),
            "{how:?}"
        );
        assert_eq!(
            json_of(&model, "pre_hook"),
            json!([{"sql": "select 1"}]),
            "{how:?}"
        );
        assert_eq!(
            json_of(&model, "post_hook"),
            json!([{"sql": "select 2"}]),
            "{how:?}: the dashed spelling is read"
        );
        assert_eq!(
            json_of(&model, "grants"),
            json!({"select": ["analyst"]}),
            "{how:?}"
        );
        assert_eq!(
            json_of(&model, "time_spine"),
            json!({"standard_granularity_column": "d"}),
            "{how:?}"
        );

        let snapshot = named("dbt.snapshots.parquet", "snap_empty");
        assert_eq!(
            snapshot["unique_key"],
            Value::Null,
            "{how:?}: a list key is not a string"
        );
        assert_eq!(
            snapshot["grants"],
            Value::Null,
            "{how:?}: empty grants are unset"
        );
        assert_eq!(
            snapshot["pre_hook"],
            Value::Null,
            "{how:?}: empty hooks are unset"
        );
        assert_eq!(
            snapshot["docs_show"],
            json!(true),
            "{how:?}: docs.show defaults to true"
        );

        let snapshot = named("dbt.snapshots.parquet", "snap_full");
        assert_eq!(
            snapshot["unique_key"],
            json!("id"),
            "{how:?}: snapshots.unique_key"
        );
        assert_eq!(
            json_of(&snapshot, "persist_docs"),
            json!({"relation": true}),
            "{how:?}: snapshots.persist_docs"
        );
        assert_eq!(
            snapshot["group"],
            json!("finance"),
            "{how:?}: snapshots.group"
        );
        assert_hooks_grants_docs(&snapshot, "snapshots");

        let seed = first("dbt.seeds.parquet");
        assert_eq!(
            seed["full_refresh"],
            json!(true),
            "{how:?}: seeds.full_refresh"
        );
        assert_eq!(
            json_of(&seed, "persist_docs"),
            json!({"relation": true}),
            "{how:?}: seeds.persist_docs"
        );
        assert_eq!(seed["group"], json!("finance"), "{how:?}: seeds.group");
        assert_hooks_grants_docs(&seed, "seeds");

        let function = first("dbt.functions.parquet");
        assert_hooks_grants_docs(&function, "functions");

        let analysis = first("dbt.analyses.parquet");
        assert_eq!(
            analysis["group"],
            json!("finance"),
            "{how:?}: analyses.group"
        );
        assert_eq!(
            analysis["docs_show"],
            json!(false),
            "{how:?}: analyses.docs_show"
        );
    }
}

/// A JSON `null` version is SQL NULL, not the text `null`; numbers and strings come out as text.
#[test]
fn model_versions_are_text_or_null_on_both_materializers() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    write_node_config_corpus(metadata.path());
    for how in [super::Materializer::Arrow, super::Materializer::Copy] {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        super::write_info_schema_with(how, metadata.path(), root.path(), staging.path()).unwrap();
        let dir = super::versioned_dir(root.path());
        let named = |name: &str| {
            super::tests::out_rows(&dir, "dbt.models.parquet")
                .into_iter()
                .find(|r| r["name"] == json!(name))
                .unwrap_or_else(|| panic!("{how:?}: no {name}"))
        };
        let m = named("m_v_null");
        assert_eq!(m["version"], Value::Null, "{how:?}: version null");
        assert_eq!(
            m["latest_version"],
            Value::Null,
            "{how:?}: latest_version null"
        );
        let m = named("m_v_num");
        assert_eq!(m["version"], json!("2"), "{how:?}: numeric version");
        assert_eq!(m["latest_version"], json!("3"), "{how:?}");
        let m = named("m_v_str");
        assert_eq!(m["version"], json!("v1"), "{how:?}: string version");
        assert_eq!(m["latest_version"], json!("v1"), "{how:?}");
    }
}

/// Columns that sit under a different key in the payload than the mapping once read:
/// `fqn`, `depends_on`, a metric's `meta` and `config`, and a saved query's `tags`.
#[test]
fn payload_keys_reach_exposures_metrics_saved_queries_and_unit_tests() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    write_node_config_corpus(metadata.path());
    for how in [super::Materializer::Arrow, super::Materializer::Copy] {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        super::write_info_schema_with(how, metadata.path(), root.path(), staging.path()).unwrap();
        let dir = super::versioned_dir(root.path());
        let first = |file: &str| super::tests::out_rows(&dir, file).remove(0);
        let json_of = |row: &Value, col: &str| -> Value {
            serde_json::from_str(row.get(col).and_then(Value::as_str).unwrap_or("null")).unwrap()
        };

        let exposure = first("dbt.exposures.parquet");
        assert_eq!(
            exposure["fqn"],
            json!(["pkg", "ex"]),
            "{how:?}: exposure fqn"
        );
        assert_eq!(
            exposure["depends_on"],
            json!(["model.pkg.m_inc"]),
            "{how:?}: exposure depends_on"
        );

        let sq = first("dbt.saved_queries.parquet");
        assert_eq!(sq["fqn"], json!(["pkg", "sq"]), "{how:?}: saved query fqn");
        assert_eq!(sq["depends_on"], json!(["metric.pkg.mt"]), "{how:?}");
        assert_eq!(sq["tags"], json!(["t1"]), "{how:?}: saved query tags");

        let metric = first("dbt.metrics.parquet");
        assert_eq!(metric["fqn"], json!(["pkg", "mt"]), "{how:?}: metric fqn");
        assert_eq!(json_of(&metric, "meta"), json!({"owner": "me"}), "{how:?}");
        assert_eq!(
            json_of(&metric, "config"),
            json!({"enabled": true}),
            "{how:?}"
        );

        let group = first("dbt.groups.parquet");
        assert_eq!(
            group["package_name"],
            json!("pkg"),
            "{how:?}: group package"
        );
        assert_eq!(
            group["original_file_path"],
            json!("models/groups.yml"),
            "{how:?}"
        );
        assert_eq!(
            json_of(&group, "config"),
            json!({"meta": {"tier": 1}}),
            "{how:?}: group config"
        );

        let sm = first("dbt.semantic_models.parquet");
        assert_eq!(sm["package_name"], json!("pkg"), "{how:?}: sm package");
        assert_eq!(sm["original_file_path"], json!("models/sm.yml"), "{how:?}");

        let ut = first("dbt.unit_tests.parquet");
        assert_eq!(ut["fqn"], json!(["pkg", "ut"]), "{how:?}: unit test fqn");
    }
}

/// A generic test reads its thresholds, `where` and `limit` from `config`, and its
/// `original_file_path` is the YAML that declares it rather than the generated SQL.
#[test]
fn generic_test_config_and_path_reach_data_tests_on_both_materializers() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    write_node_config_corpus(metadata.path());
    for how in [super::Materializer::Arrow, super::Materializer::Copy] {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        super::write_info_schema_with(how, metadata.path(), root.path(), staging.path()).unwrap();
        let dir = super::versioned_dir(root.path());
        let test = super::tests::out_rows(&dir, "dbt.data_tests.parquet").remove(0);
        assert_eq!(
            test["original_file_path"],
            json!("models/m_inc.yml"),
            "{how:?}"
        );
        assert_eq!(test["severity"], json!("warn"), "{how:?}");
        assert_eq!(test["warn_if"], json!(">1"), "{how:?}");
        assert_eq!(test["error_if"], json!(">2"), "{how:?}");
        assert_eq!(test["fail_calc"], json!("sum(x)"), "{how:?}");
        assert_eq!(test["store_failures"], json!(true), "{how:?}");
        assert_eq!(test["store_failures_as"], json!("view"), "{how:?}");
        assert_eq!(test["where"], json!("id > 0"), "{how:?}");
        assert_eq!(test["limit"], json!(5), "{how:?}");
    }
}

/// Parse nodes written straight to `parse/nodes`, one JSON object per row with the fixed
/// column set below.
fn write_node_rows(metadata_dir: &Path, rows: &[Value]) {
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
    let schema = Arc::new(Schema::new(vec![
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
        Field::new("ingested_at", ts, false),
        Field::new("is_disabled", DataType::Int32, true),
        utf8("payload"),
    ]));
    crate::parquet::write_table(&nodes_dir.join("v1_0.parquet"), schema, rows).unwrap();
}

/// A JSON-typed column read back from a published row, parsed.
fn json_cell(row: &Value, key: &str) -> Value {
    match &row[key] {
        Value::String(text) => serde_json::from_str(text).unwrap(),
        other => other.clone(),
    }
}

/// A singular test has no `test_metadata`, but its `config` still carries a severity and
/// thresholds, and dbt's manifest gives it the same defaults a generic test gets. Its name,
/// namespace and arguments stay NULL.
#[test]
fn singular_data_test_config_reaches_data_tests_on_both_materializers() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    let test = |id: &str, config: Value| {
        json!({
            "unique_id": id,
            "resource_type": "test",
            "name": id,
            "package_name": "pkg",
            "tags": [],
            "fqn": ["pkg", id],
            "depends_on": [],
            "ingested_at": "2026-09-08T12:00:00Z",
            "is_disabled": 0,
            "payload": json!({"__test_attr__": {}, "config": config}).to_string(),
        })
    };
    write_node_rows(
        metadata.path(),
        &[
            test(
                "test.pkg.plain",
                json!({"severity": "ERROR", "error_if": "!= 0", "warn_if": "!= 0"}),
            ),
            test(
                "test.pkg.tuned",
                json!({"severity": "warn", "error_if": ">5", "where": "id > 0", "limit": 3}),
            ),
        ],
    );
    for how in [super::Materializer::Arrow, super::Materializer::Copy] {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        super::write_info_schema_with(how, metadata.path(), root.path(), staging.path()).unwrap();
        let rows =
            super::tests::out_rows(&super::versioned_dir(root.path()), "dbt.data_tests.parquet");
        let get = |id: &str| {
            rows.iter()
                .find(|r| r["unique_id"] == json!(id))
                .unwrap_or_else(|| panic!("{how:?}: no {id}: {rows:?}"))
        };
        let plain = get("test.pkg.plain");
        assert_eq!(plain["severity"], json!("ERROR"), "{how:?}");
        assert_eq!(plain["error_if"], json!("!= 0"), "{how:?}");
        assert_eq!(plain["warn_if"], json!("!= 0"), "{how:?}");
        assert_eq!(plain["fail_calc"], json!("count(*)"), "{how:?}");
        assert_eq!(plain["test_name"], Value::Null, "{how:?}");
        let tuned = get("test.pkg.tuned");
        assert_eq!(tuned["severity"], json!("warn"), "{how:?}");
        assert_eq!(tuned["error_if"], json!(">5"), "{how:?}");
        assert_eq!(
            tuned["warn_if"],
            json!("!= 0"),
            "{how:?}: unset threshold defaults"
        );
        assert_eq!(tuned["where"], json!("id > 0"), "{how:?}");
        assert_eq!(tuned["limit"], json!(3), "{how:?}");
    }
}

/// Hooks are published as dbt's manifest spells them, a list of `{sql, transaction}`, whatever
/// shape the author used: one string, a list of strings, or a list of objects.
#[test]
fn hook_configs_are_normalized_to_a_list_of_hooks_on_both_materializers() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    let model = |id: &str, config: Value| {
        json!({
            "unique_id": id,
            "resource_type": "model",
            "name": id,
            "package_name": "pkg",
            "tags": [],
            "fqn": ["pkg", id],
            "depends_on": [],
            "ingested_at": "2026-09-08T12:00:00Z",
            "is_disabled": 0,
            "payload": json!({"config": config}).to_string(),
        })
    };
    write_node_rows(
        metadata.path(),
        &[
            model(
                "model.pkg.one_string",
                json!({"pre_hook": "select 1", "post-hook": "select 2"}),
            ),
            model(
                "model.pkg.mixed",
                json!({"pre_hook": ["select 3", {"sql": "select 4", "transaction": false}]}),
            ),
            model("model.pkg.none", json!({"pre_hook": [], "post_hook": null})),
        ],
    );
    for how in [super::Materializer::Arrow, super::Materializer::Copy] {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        super::write_info_schema_with(how, metadata.path(), root.path(), staging.path()).unwrap();
        let rows = super::tests::out_rows(&super::versioned_dir(root.path()), "dbt.models.parquet");
        let get = |id: &str| {
            rows.iter()
                .find(|r| r["unique_id"] == json!(id))
                .unwrap_or_else(|| panic!("{how:?}: no {id}: {rows:?}"))
        };
        let one = get("model.pkg.one_string");
        assert_eq!(
            json_cell(one, "pre_hook"),
            json!([{"sql": "select 1", "transaction": true}]),
            "{how:?}"
        );
        assert_eq!(
            json_cell(one, "post_hook"),
            json!([{"sql": "select 2", "transaction": true}]),
            "{how:?}"
        );
        let mixed = get("model.pkg.mixed");
        assert_eq!(
            json_cell(mixed, "pre_hook"),
            json!([
                {"sql": "select 3", "transaction": true},
                {"sql": "select 4", "transaction": false}
            ]),
            "{how:?}"
        );
        let none = get("model.pkg.none");
        assert_eq!(
            none["pre_hook"],
            Value::Null,
            "{how:?}: an empty list is unset"
        );
        assert_eq!(none["post_hook"], Value::Null, "{how:?}");
    }
}

/// Source-only columns come from `__source_attr__`, and a dimension's `validity_params` from
/// its `type_params`.
#[test]
fn source_and_dimension_columns_reach_their_tables_on_both_materializers() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    write_node_config_corpus(metadata.path());
    for how in [super::Materializer::Arrow, super::Materializer::Copy] {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        super::write_info_schema_with(how, metadata.path(), root.path(), staging.path()).unwrap();
        let dir = super::versioned_dir(root.path());
        let json_of = |row: &Value, col: &str| -> Value {
            serde_json::from_str(row.get(col).and_then(Value::as_str).unwrap_or("null")).unwrap()
        };
        let source = super::tests::out_rows(&dir, "dbt.sources.parquet").remove(0);
        assert_eq!(
            source["loaded_at_query"],
            json!("select max(ts) from t"),
            "{how:?}"
        );
        assert_eq!(
            json_of(&source, "freshness"),
            json!({"warn_after": {"count": 1, "period": "day"}}),
            "{how:?}"
        );
        assert_eq!(
            json_of(&source, "external"),
            json!({"location": "s3://b"}),
            "{how:?}"
        );
        assert_eq!(
            json_of(&source, "quoting"),
            json!({"database": true}),
            "{how:?}"
        );
        let dim = super::tests::out_rows(&dir, "dbt.semantic_dimensions.parquet").remove(0);
        assert_eq!(
            json_of(&dim, "validity_params"),
            json!({"is_start": true}),
            "{how:?}"
        );
    }
}

/// `dbt_rt.invocations` carries the command line the epoch recorded as a JSON array, and the
/// resolved target and vars when the epoch has them.
#[test]
fn invocation_details_reach_invocations_on_both_materializers() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    let dir = metadata.path().join(crate::ingest::RUN_INVOCATIONS_SUBDIR);
    std::fs::create_dir_all(&dir).unwrap();
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC")));
    let utf8 = |name: &str| Field::new(name, DataType::Utf8, true);
    let schema = Arc::new(Schema::new(vec![
        Field::new("invocation_id", DataType::Utf8, false),
        utf8("command"),
        utf8("dbt_version"),
        utf8("selector"),
        utf8("target_name"),
        utf8("adapter_type"),
        utf8("git_sha"),
        utf8("git_branch"),
        Field::new("git_is_dirty", DataType::Int32, true),
        Field::new("elapsed_secs", DataType::Float64, true),
        Field::new("node_count", DataType::Int32, true),
        Field::new(
            "cli_args",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        ),
        Field::new("ingested_at", ts, false),
    ]));
    let row = |id: &str, args: Value| {
        json!({
            "invocation_id": id,
            "command": "run",
            "dbt_version": "2.0",
            "cli_args": args,
            "ingested_at": "2026-09-08T12:00:00Z",
        })
    };
    crate::parquet::write_table(
        &dir.join("v1_0.parquet"),
        schema.clone(),
        &[
            row("with_args", json!(["dbt", "run", "--threads", "2"])),
            row("no_args", json!([])),
        ],
    )
    .unwrap();
    // A later epoch also records the resolved target and the vars.
    let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
    fields.extend([
        utf8("target_database"),
        utf8("target_schema"),
        Field::new("target_threads", DataType::Int32, true),
        utf8("vars_override"),
    ]);
    let mut full = row("full", json!(["dbt", "build"]));
    full["target_database"] = json!("analytics");
    full["target_schema"] = json!("dbt_dev");
    full["target_threads"] = json!(4);
    full["node_count"] = json!(7);
    full["vars_override"] = json!(r#"{"start":"2024-01-01"}"#);
    crate::parquet::write_table(
        &dir.join("v1_1.parquet"),
        Arc::new(Schema::new(fields)),
        &[full],
    )
    .unwrap();
    for how in [super::Materializer::Arrow, super::Materializer::Copy] {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        super::write_info_schema_with(how, metadata.path(), root.path(), staging.path()).unwrap();
        let out = super::versioned_dir(root.path());
        let rows = super::tests::out_rows(&out, "dbt_rt.invocations.parquet");
        let by_id = |id: &str| {
            rows.iter()
                .find(|r| r["invocation_id"] == json!(id))
                .unwrap_or_else(|| panic!("{how:?}: no {id}"))
        };
        let args: Value =
            serde_json::from_str(by_id("with_args")["args"].as_str().unwrap()).unwrap();
        assert_eq!(args, json!(["dbt", "run", "--threads", "2"]), "{how:?}");
        assert_eq!(by_id("no_args")["args"], Value::Null, "{how:?}");
        let full = by_id("full");
        assert_eq!(full["target_database"], json!("analytics"), "{how:?}");
        assert_eq!(full["target_schema"], json!("dbt_dev"), "{how:?}");
        assert_eq!(full["target_threads"], json!(4), "{how:?}");
        assert_eq!(full["node_count"], json!(7), "{how:?}");
        assert_eq!(
            full["vars_override"],
            json!(r#"{"start":"2024-01-01"}"#),
            "{how:?}"
        );
        // An epoch from before these were recorded still reads, as NULL.
        assert_eq!(by_id("no_args")["target_database"], Value::Null, "{how:?}");
    }
}

/// `dbt_rt.run_results.batch_results` is what the epoch recorded, and an epoch written before
/// the column existed still reads, as NULL.
#[test]
fn batch_results_reach_run_results_and_old_epochs_still_read() {
    if crate::db::Db::open_memory().is_err() {
        eprintln!("skipping: DuckDB driver unavailable");
        return;
    }
    let metadata = tempfile::tempdir().unwrap();
    let dir = metadata.path().join(crate::ingest::RUN_RESULTS_SUBDIR);
    std::fs::create_dir_all(&dir).unwrap();
    let ts = DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC")));
    let utf8 = |name: &str| Field::new(name, DataType::Utf8, true);
    let mut fields = vec![
        Field::new("invocation_id", DataType::Utf8, false),
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        utf8("message"),
        Field::new("execution_time", DataType::Float64, true),
        utf8("thread_id"),
        Field::new("failures", DataType::Int64, true),
        utf8("compiled_code_hash"),
        utf8("relation_name"),
        utf8("adapter_response"),
        utf8("timing"),
    ];
    let old_fields = fields.clone();
    fields.push(utf8("batch_results"));
    let row = |id: &str, batch: Option<&str>| {
        let mut r = json!({
            "invocation_id": id,
            "unique_id": "model.pkg.mb",
            "status": "success",
            "ingested_at": "2026-09-08T12:00:00Z",
        });
        if let Some(b) = batch {
            r["batch_results"] = json!(b);
        }
        r
    };
    let schema = |mut f: Vec<Field>| {
        f.push(Field::new("ingested_at", ts.clone(), false));
        Arc::new(Schema::new(f))
    };
    // The earlier epoch has no `batch_results` column at all.
    crate::parquet::write_table(
        &dir.join("v1_0.parquet"),
        schema(old_fields),
        &[row("old", None)],
    )
    .unwrap();
    crate::parquet::write_table(
        &dir.join("v1_1.parquet"),
        schema(fields),
        &[row(
            "new",
            Some(r#"{"successful":[["a","b"]],"failed":[]}"#),
        )],
    )
    .unwrap();
    for how in [super::Materializer::Arrow, super::Materializer::Copy] {
        let root = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        super::write_info_schema_with(how, metadata.path(), root.path(), staging.path()).unwrap();
        let out = super::versioned_dir(root.path());
        let rows = super::tests::out_rows(&out, "dbt_rt.run_results.parquet");
        let by_id = |id: &str| {
            rows.iter()
                .find(|r| r["invocation_id"] == json!(id))
                .unwrap_or_else(|| panic!("{how:?}: no {id}"))
        };
        assert_eq!(
            by_id("new")["batch_results"],
            json!(r#"{"successful":[["a","b"]],"failed":[]}"#),
            "{how:?}"
        );
        assert_eq!(by_id("old")["batch_results"], Value::Null, "{how:?}");
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
