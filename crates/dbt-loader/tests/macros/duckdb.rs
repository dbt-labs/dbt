use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, assert_executed_contains, executed_sql};

fn catalog_relation(write_strategy: &str) -> Value {
    let catalog_relation = Arc::new(MockJinjaObject::new());
    catalog_relation.set_attr("duckdb_write_strategy", Value::from(write_strategy));
    // Some macros may still read supports_stage_create directly; keep it consistent
    // with the strategy (the CTAS strategies stage-create directly from SELECT).
    catalog_relation.set_attr(
        "supports_stage_create",
        Value::from(matches!(
            write_strategy,
            "create_as_select" | "direct_create_as_select"
        )),
    );
    Value::from_dyn_object(catalog_relation)
}

fn relation_database(arg: Option<&Value>) -> Option<String> {
    arg.and_then(|v| v.get_attr("database").ok())
        .and_then(|v| v.as_str().map(str::to_string))
}

fn build_harness(
    iceberg_database: &'static str,
    ducklake_database: &'static str,
) -> MacroTestHarness {
    let mut harness = MacroTestHarness::for_adapter(AdapterType::DuckDB)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    let mock = harness.mock().clone();
    mock.on("table_format", move |args| {
        let database = relation_database(args.first());
        let table_format = match database.as_deref() {
            Some(database) if database == iceberg_database => "iceberg",
            Some(database) if database == ducklake_database => "ducklake",
            _ => "default",
        };
        Ok(Value::from(table_format))
    });
    mock.on("commit", |_| Ok(Value::UNDEFINED));
    mock.on("quote_as_configured", |args| {
        Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
    });
    mock.on("build_catalog_relation", |_| {
        Ok(catalog_relation("create_as_select"))
    });
    mock.on("get_column_schema_from_query", |_| {
        Ok(Value::from_serialize(vec![BTreeMap::from([
            ("name", Value::from("id")),
            ("quoted", Value::from("\"id\"")),
            ("dtype", Value::from("integer")),
        ])]))
    });
    mock.on("quote", |args| {
        let name = args
            .first()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        Ok(Value::from(format!("\"{name}\"")))
    });
    harness
        .env_mut()
        .env
        .add_function("load_result", |_name: Value| {
            Ok(Value::from_serialize(BTreeMap::from([(
                "table",
                Vec::<Vec<Value>>::new(),
            )])))
        });
    harness
        .env_mut()
        .env
        .add_global("execute", Value::from(true));
    harness
        .env_mut()
        .env
        .add_global("adapter", Value::from_dyn_object(mock));

    harness
}

#[test]
fn create_table_as_quotes_only_columns_marked_quote_true() {
    let harness = build_harness("iceberg_demo", "ducklake_demo");
    harness.mock().on("build_catalog_relation", |_| {
        Ok(catalog_relation("direct_create"))
    });
    // `quoted` mirrors real `Column::quoted()` under DuckDB's default resolved
    // quoting (identifier quoting on): always pre-quoted, regardless of any
    // per-column `quote` config.
    harness.mock().on("get_column_schema_from_query", |_| {
        Ok(Value::from_serialize(vec![
            BTreeMap::from([
                ("name", Value::from("id")),
                ("quoted", Value::from("\"id\"")),
                ("dtype", Value::from("integer")),
            ]),
            BTreeMap::from([
                ("name", Value::from("MixedCase")),
                ("quoted", Value::from("\"MixedCase\"")),
                ("dtype", Value::from("varchar")),
            ]),
        ]))
    });
    harness.mock().on("quote", |args| {
        let identifier = args.first().and_then(|v| v.as_str()).unwrap_or_default();
        Ok(Value::from(format!("\"{identifier}\"")))
    });

    let model = Value::from_serialize(BTreeMap::from([
        ("alias".to_string(), Value::from("orders")),
        (
            "unique_id".to_string(),
            Value::from("model.test_project.orders"),
        ),
        (
            "columns".to_string(),
            Value::from_serialize(BTreeMap::from([(
                "MixedCase",
                BTreeMap::from([
                    ("name", Value::from("MixedCase")),
                    ("quote", Value::from(true)),
                ]),
            )])),
        ),
    ]));

    let ctx = harness
        .materialization_context("orders", "select 1 as id, 2 as mixedcase")
        .relation_type(RelationType::Table)
        .with("model", model)
        .with("dbt_version", Value::from("2.0.0"))
        .build();

    let rendered = harness
        .render(
            "{{ duckdb__create_table_as(false, this, compiled_code) }}",
            ctx,
        )
        .expect("render should succeed");

    // `compiled_code` (the select statement) is passed through verbatim by the
    // macro, so assert only on the column lists the macro itself generates
    // (the `create table (...)` and `insert into (...)` clauses), not on
    // anything that could leak in from the query body.
    assert!(
        rendered.contains("\"MixedCase\" varchar"),
        "column configured with quote: true should be quoted in the create-table column list, got: {rendered}"
    );
    assert!(
        rendered.contains("id integer") && !rendered.contains("\"id\" integer"),
        "column without quote: true should remain unquoted in the create-table column list, got: {rendered}"
    );

    let insert_clause = rendered
        .split("insert into")
        .nth(1)
        .expect("rendered SQL should contain an insert into clause");
    assert!(
        insert_clause.contains("\"MixedCase\""),
        "column configured with quote: true should be quoted in the insert column list, got: {insert_clause}"
    );
    assert!(
        !insert_clause.contains("\"id\""),
        "column without quote: true should remain unquoted in the insert column list, got: {insert_clause}"
    );
}

#[test]
fn get_columns_in_relation_uses_describe_for_iceberg_relations() {
    let harness = build_harness("iceberg_demo", "ducklake_demo");
    let relation = harness.relation("iceberg_demo", "main", "orders", Some(RelationType::Table));
    let ctx = BTreeMap::from([(
        "relation".to_string(),
        RelationObject::new(relation).into_value(),
    )]);

    harness
        .render("{{ duckdb__get_columns_in_relation(relation) }}", ctx)
        .expect("render should succeed");

    let executed = executed_sql(harness.mock()).join("\n");
    assert!(
        executed.contains("from (describe"),
        "Iceberg relations should use DESCRIBE, got: {executed}"
    );
    assert!(
        !executed.contains("information_schema.columns"),
        "Iceberg relations should not use information_schema.columns, got: {executed}"
    );
}

#[test]
fn drop_relation_omits_cascade_for_iceberg_relations() {
    let harness = build_harness("iceberg_demo", "ducklake_demo");
    let relation = harness.relation("iceberg_demo", "main", "orders", Some(RelationType::Table));
    let ctx = BTreeMap::from([(
        "relation".to_string(),
        RelationObject::new(relation).into_value(),
    )]);

    harness
        .render("{{ duckdb__drop_relation(relation) }}", ctx)
        .expect("render should succeed");

    assert_executed_contains(harness.mock(), "drop table if exists");
    let executed = executed_sql(harness.mock()).join("\n");
    assert!(
        !executed.contains("cascade"),
        "Iceberg drop should omit CASCADE, got: {executed}"
    );
}

#[test]
fn rename_relation_alters_iceberg_without_committing_connection() {
    let harness = build_harness("iceberg_demo", "ducklake_demo");
    let from_relation = harness.relation(
        "iceberg_demo",
        "main",
        "orders__dbt_tmp",
        Some(RelationType::Table),
    );
    let to_relation = harness.relation("regular_demo", "main", "orders", Some(RelationType::Table));
    let ctx = BTreeMap::from([
        (
            "from_relation".to_string(),
            RelationObject::new(from_relation).into_value(),
        ),
        (
            "to_relation".to_string(),
            RelationObject::new(to_relation).into_value(),
        ),
    ]);

    harness
        .render(
            "{{ duckdb__rename_relation(from_relation, to_relation) }}",
            ctx,
        )
        .expect("render should succeed");

    // Fusion connections are shared across nodes, so the iceberg rename must NOT
    // commit the connection; it relies on DuckDB autocommit (auto_begin=False).
    let calls = harness.mock().observed_calls();
    assert!(
        !calls.iter().any(|call| call.method == "commit"),
        "iceberg rename must not commit the shared connection: {calls:?}"
    );
    drop(calls);

    let executed = executed_sql(harness.mock()).join("\n");
    assert!(
        executed.contains("alter table"),
        "rename should use ALTER TABLE RENAME, got: {executed}"
    );
    assert!(
        !executed.contains("create table") && !executed.contains("drop table"),
        "rename should not use DROP/CREATE fallback, got: {executed}"
    );
}

fn render_table_materialization(write_strategy: &'static str) -> MacroTestHarness {
    render_table_materialization_with_columns(write_strategy, BTreeMap::new())
}

/// `user_columns` populates the model's yaml `columns:` block, keyed by column
/// name — this is what the `direct_create` column list consults for `quote:`.
fn render_table_materialization_with_columns(
    write_strategy: &'static str,
    user_columns: BTreeMap<String, Value>,
) -> MacroTestHarness {
    let harness = build_harness("iceberg_demo", "ducklake_demo");
    harness
        .mock()
        .on("table_format", |_| Ok(Value::from("iceberg")));
    harness.mock().on("build_catalog_relation", move |_| {
        Ok(catalog_relation(write_strategy))
    });
    harness.mock().on("get_relation", |_| Ok(Value::from(())));
    harness.mock().on("drop_relation", |_| Ok(Value::UNDEFINED));
    harness
        .mock()
        .on("rename_relation", |_| Ok(Value::UNDEFINED));
    harness
        .mock()
        .on("drop_indexes_on_relation", |_| Ok(Value::UNDEFINED));

    let model = Value::from_serialize(BTreeMap::from([
        ("alias", Value::from("orders")),
        ("unique_id", Value::from("model.test_project.orders")),
        ("columns", Value::from_serialize(user_columns)),
        ("language", Value::from("sql")),
        ("compiled_code", Value::from("select 1 as id")),
    ]));
    let config = Arc::new(MockJinjaObject::new());
    config.on("get", |args| {
        let key = args.first().and_then(|v| v.as_str());
        let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
        match key {
            Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                "enforced".to_string(),
                Value::from(false),
            )]))),
            Some("indexes") => Ok(Value::from(Vec::<Value>::new())),
            _ => Ok(default),
        }
    });
    config.on("persist_column_docs", |_| Ok(Value::from(false)));
    config.on("persist_relation_docs", |_| Ok(Value::from(false)));
    config.set_attr("model", model.clone());

    let ctx = harness
        .materialization_context("orders", "select 1 as id")
        .database("iceberg_demo")
        .schema("main")
        .relation_type(RelationType::Table)
        .config(Value::from_dyn_object(config))
        .with("model", model)
        .build();

    harness
        .render("{{ materialization_table_duckdb() }}", ctx)
        .expect("render should succeed");
    harness
}

#[test]
fn table_materialization_writes_iceberg_target_directly() {
    let harness = render_table_materialization("direct_create");

    harness
        .mock()
        .observed_calls()
        .assert_not_called("rename_relation");
    harness
        .mock()
        .observed_calls()
        .assert_not_called("get_relation");
    let executed = executed_sql(harness.mock()).join("\n");
    assert!(
        executed.contains("create table"),
        "Iceberg table materialization should create the target relation, got: {executed}"
    );
    assert!(
        !executed.contains("__dbt_tmp"),
        "Iceberg table materialization should not use an intermediate relation, got: {executed}"
    );
    assert!(
        executed.contains("insert into"),
        "Iceberg table materialization should create then insert, got: {executed}"
    );
    assert!(
        !executed.contains(" as ("),
        "Iceberg table materialization should not use CTAS, got: {executed}"
    );
}

/// The `direct_create` branch derives its column list from the SELECT via
/// `get_column_schema_from_query`, so those columns carry no `quote:` flag of
/// their own. Undeclared columns must be emitted bare rather than quoted
/// unconditionally, matching every other adapter's DDL.
#[test]
fn table_materialization_does_not_quote_undeclared_column_names() {
    let harness = render_table_materialization("direct_create");
    let executed = executed_sql(harness.mock()).join("\n");

    assert!(
        executed.contains("id integer"),
        "column spec should name the column unquoted, got: {executed}"
    );
    assert!(
        !executed.contains("\"id\""),
        "undeclared column name must not be quoted in generated DDL, got: {executed}"
    );
}

/// Counterpart to the above: the `quote:` flag is looked up by name in the
/// model's yaml `columns:` block, so declaring `quote: true` still quotes.
/// Without this the fix would be indistinguishable from always emitting
/// `col.name`.
#[test]
fn table_materialization_quotes_column_names_flagged_in_yaml() {
    let user_columns = BTreeMap::from([(
        "id".to_string(),
        Value::from_serialize(BTreeMap::from([
            ("name".to_string(), Value::from("id")),
            ("quote".to_string(), Value::from(true)),
        ])),
    )]);
    let harness = render_table_materialization_with_columns("direct_create", user_columns);
    let executed = executed_sql(harness.mock()).join("\n");

    assert!(
        executed.contains("\"id\" integer"),
        "column flagged `quote: true` should be quoted, got: {executed}"
    );
    assert!(
        executed.contains("insert into"),
        "expected the create-then-insert path, got: {executed}"
    );
}

#[test]
fn table_materialization_ctas_in_place_for_staged_create_opt_in() {
    // `stage_create_tables: true` (duckdb-iceberg#1017) → direct_create_as_select:
    // CTAS straight into the target, still no temp-table + rename dance.
    let harness = render_table_materialization("direct_create_as_select");

    harness
        .mock()
        .observed_calls()
        .assert_not_called("rename_relation");
    harness
        .mock()
        .observed_calls()
        .assert_not_called("get_relation");
    let executed = executed_sql(harness.mock()).join("\n");
    assert!(
        !executed.contains("__dbt_tmp"),
        "staged-create opt-in should not use an intermediate relation, got: {executed}"
    );
    assert!(
        executed.contains(" as ("),
        "staged-create opt-in should CTAS the target, got: {executed}"
    );
    assert!(
        !executed.contains("insert into"),
        "staged-create opt-in should not create-then-insert, got: {executed}"
    );
}
