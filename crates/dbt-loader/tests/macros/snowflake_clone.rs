//! Tests for the Snowflake `dbt clone` macros.
//!
//! The clone target takes the source relation's table format. An Iceberg target
//! clones with `create or replace iceberg table ... clone ...`, and every other
//! target keeps the transient default. On `--full-refresh`, an existing target in
//! the other format is dropped first, because Snowflake cannot `create or replace`
//! across formats.

use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::{Relation, RelationObject};
use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::relations::SNOWFLAKE_RESOLVED_QUOTING;
use dbt_schemas::schemas::relations::base::TableFormat;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, default_mock_config, executed_sql};

fn build_harness() -> MacroTestHarness {
    MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("clone harness should build")
}

/// A Snowflake relation with the given table format, shaped the way
/// `adapter.get_relation` returns cached relations to Jinja.
fn source_relation(table_format: TableFormat) -> Value {
    let relation = Relation::new(
        AdapterType::Snowflake,
        "TEST_DB".to_string(),
        "OTHER_SCHEMA".to_string(),
        "MY_TABLE".to_string(),
    )
    .with_relation_type(Some(RelationType::Table))
    .with_quoting(SNOWFLAKE_RESOLVED_QUOTING)
    .with_table_format(table_format);
    RelationObject::new(Arc::from(relation)).into_value()
}

fn render_clone(harness: &MacroTestHarness, target_format: TableFormat) -> String {
    let target = Relation::new(
        AdapterType::Snowflake,
        "TEST_DB".to_string(),
        "TEST_SCHEMA".to_string(),
        "MY_TABLE".to_string(),
    )
    .with_relation_type(Some(RelationType::Table))
    .with_quoting(SNOWFLAKE_RESOLVED_QUOTING)
    .with_table_format(target_format);
    let ctx = BTreeMap::from([
        (
            "this_relation".to_string(),
            RelationObject::new(Arc::new(target)).into_value(),
        ),
        ("defer_relation".to_string(), source_relation(target_format)),
    ]);
    let rendered = harness
        .render(
            "{{ snowflake__create_or_replace_clone(this_relation, defer_relation) }}",
            ctx,
        )
        .expect("clone macro should render");
    // Collapse Jinja's intra-clause whitespace so assertions can match phrases
    // that span template lines.
    rendered.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn iceberg_target_clones_as_iceberg_table() {
    let rendered = render_clone(&build_harness(), TableFormat::Iceberg).to_lowercase();
    assert!(
        rendered.contains("create or replace iceberg table"),
        "an Iceberg clone target must emit `create or replace iceberg table`, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("transient"),
        "an Iceberg clone of a permanent source must not be transient, got:\n{rendered}"
    );
}

#[test]
fn regular_target_keeps_transient_default() {
    let rendered = render_clone(&build_harness(), TableFormat::Default).to_lowercase();
    assert!(
        rendered.contains("create or replace transient table"),
        "a regular clone target keeps the transient default, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("iceberg"),
        "a regular clone target must not render the iceberg keyword, got:\n{rendered}"
    );
}

fn assert_clone_persist_docs(
    source_format: TableFormat,
    target_format: TableFormat,
    full_refresh: bool,
) {
    let harness = build_harness();
    let source = source_relation(source_format);
    let target = RelationObject::new(Arc::new(
        Relation::new(
            AdapterType::Snowflake,
            "TargetDb".to_string(),
            "TargetSchema".to_string(),
            "TargetTable".to_string(),
        )
        .with_relation_type(Some(RelationType::Table))
        .with_quoting(dbt_schemas::schemas::common::ResolvedQuoting {
            database: true,
            schema: true,
            identifier: true,
        })
        .with_table_format(target_format),
    ))
    .into_value();
    let cached_source = source.clone();
    let cached_target = target.clone();
    harness.mock().on("get_relation", move |args| {
        if args.last().unwrap().get_attr("schema")?.as_str() == Some("OTHER_SCHEMA") {
            Ok(cached_source.clone())
        } else if full_refresh {
            Ok(cached_target.clone())
        } else {
            Ok(Value::from(()))
        }
    });
    harness.mock().on("drop_relation", |_| Ok(Value::from(())));
    harness.mock().on("get_columns_in_relation", |_| {
        Ok(Value::from_serialize(vec![BTreeMap::from([(
            "name", "ID",
        )])]))
    });
    harness
        .mock()
        .on("quote", |args| Ok(Value::from(format!("\"{}\"", args[0]))));
    let config = default_mock_config();
    config.on("persist_relation_docs", |_| Ok(Value::from(true)));
    config.on("persist_column_docs", |_| Ok(Value::from(true)));
    let ctx = harness
        .materialization_context("TargetTable", "select 1 as id")
        .config(Value::from_dyn_object(config))
        .with("this", target)
        .with("defer_relation", source)
        .with(
            "flags",
            Value::from_serialize(BTreeMap::from([("FULL_REFRESH", full_refresh)])),
        )
        .with(
            "model",
            Value::from_serialize(BTreeMap::from([
                ("unique_id", Value::from("model.test.clone")),
                ("description", Value::from("Table docs")),
                (
                    "columns",
                    Value::from_serialize(BTreeMap::from([(
                        "ID",
                        BTreeMap::from([
                            ("description", Value::from("Column docs")),
                            ("quote", Value::from(false)),
                        ]),
                    )])),
                ),
            ])),
        )
        .build();
    let result = harness.render("{% set result = materialization_clone_default() %}{{ result.relations[0].is_iceberg_format }}", ctx).expect("clone with persist_docs should render");
    let iceberg = source_format == TableFormat::Iceberg;
    assert_eq!(result.trim(), if iceberg { "True" } else { "False" });
    let sql = executed_sql(harness.mock())
        .into_iter()
        .map(|sql| sql.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>();
    let calls = harness.mock().observed_calls();
    let drop_at = calls.iter().position(|c| c.method == "drop_relation");
    let clone_at = calls.iter().position(|c| {
        c.method == "execute"
            && c.args
                .first()
                .and_then(|v| v.as_str())
                .is_some_and(|sql| sql.contains(" clone "))
    });
    if full_refresh && source_format != target_format {
        assert!(drop_at.is_some() && drop_at < clone_at, "{calls:#?}");
    } else {
        assert!(drop_at.is_none(), "{calls:#?}");
    }
    drop(calls);
    let target_name = "\"TargetDb\".\"TargetSchema\".\"TargetTable\"";
    let clone_kind = if iceberg { "iceberg" } else { "transient" };
    assert!(
        sql.iter().any(|sql| sql.contains(&format!(
            "create or replace {clone_kind} table {target_name} clone TEST_DB.OTHER_SCHEMA.MY_TABLE"
        ))),
        "{sql:#?}"
    );
    let relation_comment = if iceberg {
        format!("alter iceberg table {target_name} set comment = $$Table docs$$;")
    } else {
        format!("comment on table {target_name} IS $$Table docs$$;")
    };
    assert!(sql.contains(&relation_comment), "{sql:#?}");
    let column_kind = if iceberg { "iceberg table" } else { "table" };
    assert!(
        sql.contains(&format!(
            "alter {column_kind} {target_name} alter \"ID\" COMMENT $$Column docs$$;"
        )),
        "{sql:#?}"
    );
}

#[test]
fn iceberg_clone_persist_docs_uses_source_format() {
    assert_clone_persist_docs(TableFormat::Iceberg, TableFormat::Default, false);
}

#[test]
fn regular_clone_persist_docs_ignores_iceberg_model_format() {
    assert_clone_persist_docs(TableFormat::Default, TableFormat::Iceberg, false);
}

#[test]
fn full_refresh_iceberg_clone_drops_regular_target() {
    assert_clone_persist_docs(TableFormat::Iceberg, TableFormat::Default, true);
}

#[test]
fn full_refresh_regular_clone_drops_iceberg_target() {
    assert_clone_persist_docs(TableFormat::Default, TableFormat::Iceberg, true);
}

#[test]
fn full_refresh_same_format_clone_replaces_in_place() {
    assert_clone_persist_docs(TableFormat::Iceberg, TableFormat::Iceberg, true);
}

#[test]
fn format_flip_drop_reason_names_both_formats() {
    let harness = build_harness();
    let ctx = BTreeMap::from([
        (
            "existing".to_string(),
            source_relation(TableFormat::Iceberg),
        ),
        ("target".to_string(), source_relation(TableFormat::Default)),
    ]);
    let reason = harness
        .render(
            "{{ snowflake__clone_target_drop_reason(existing, target) }}",
            ctx,
        )
        .expect("drop reason should render");
    assert_eq!(
        reason.trim(),
        "it is an Iceberg table and the clone source is a regular table"
    );
}
