//! Tests for `dbt-databricks/macros/materializations/seeds/seeds.sql`.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::RecordBatch;
use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_agate::AgateTable;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;
use minijinja::value::Kwargs;

use crate::macro_test_harness::{MacroTestHarness, default_mock_config, executed_sql};

const ADAPTER: AdapterType = AdapterType::Databricks;
const MATERIALIZATION: &str = "{{ materialization_seed_databricks() }}";
const SEED: &str = "my_seed";

fn seed_rows() -> RecordBatch {
    arrow_array::record_batch!(("id", Int64, [1, 2]), ("name", Utf8, ["a", "b"]))
        .expect("seed batch should build")
}

fn empty_seed_rows() -> RecordBatch {
    seed_rows().slice(0, 0)
}

fn build_seed_harness(materialization_v2: bool, rows: RecordBatch) -> MacroTestHarness {
    let mut harness = MacroTestHarness::for_adapter(ADAPTER)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    let agate_table = Value::from_object(AgateTable::from_record_batch(Arc::new(rows)));
    let env = &mut harness.env_mut().env;
    env.add_function("load_agate_table", move || agate_table.clone());
    env.add_function("store_raw_result", |_name: Value, _kwargs: Kwargs| {
        Value::UNDEFINED
    });
    env.add_global(
        "flags",
        Value::from_serialize(BTreeMap::from([("FULL_REFRESH", false)])),
    );

    let mock = harness.mock();
    mock.set_attr(
        "behavior",
        Value::from_serialize(BTreeMap::from([
            ("use_materialization_v2", Value::from(materialization_v2)),
            (
                "use_catalogs_v2",
                Value::from_serialize(BTreeMap::from([("no_warn", false)])),
            ),
        ])),
    );
    mock.on("commit", |_| Ok(Value::UNDEFINED));
    mock.on("add_query", |_| Ok(Value::UNDEFINED));
    mock.on("drop_relation", |_| Ok(Value::UNDEFINED));
    mock.on("convert_type", |_| Ok(Value::from("string")));
    mock.on("quote_seed_column", |args| {
        Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
    });
    mock.on("resolve_file_format", |_| Ok(Value::from("delta")));
    mock.on("build_catalog_relation", |_| {
        Ok(Value::from_serialize(BTreeMap::from([
            ("catalog_type", Value::from("unity")),
            ("table_format", Value::from("default")),
            ("file_format", Value::from("delta")),
            ("location", Value::from(())),
        ])))
    });
    mock.on("is_uniform", |_| Ok(Value::from(false)));
    harness
}

fn existing_relation(harness: &MacroTestHarness, relation_type: Option<RelationType>) {
    let existing =
        relation_type.map(|rt| harness.relation("TEST_DB", "TEST_SCHEMA", SEED, Some(rt)));
    harness.mock().on("get_relation", move |_| {
        Ok(existing
            .as_ref()
            .map(|relation| RelationObject::new(Arc::clone(relation)).into_value())
            .unwrap_or_else(|| Value::from(())))
    });
}

fn render_seed(harness: &MacroTestHarness) -> dbt_common::FsResult<String> {
    let config = default_mock_config();
    config.set_attr("materialized", Value::from("seed"));
    let ctx = harness
        .materialization_context(SEED, "")
        .config(Value::from_dyn_object(config))
        .relation_type(RelationType::Table)
        .with("dbt_version", Value::from("2.0.0"))
        .with(
            "model",
            Value::from_serialize(BTreeMap::from([
                ("alias", Value::from(SEED)),
                (
                    "unique_id",
                    Value::from(format!("seed.test_project.{SEED}")),
                ),
                ("columns", Value::from(BTreeMap::<String, Value>::new())),
                (
                    "config",
                    Value::from_serialize(BTreeMap::from([("materialized", "seed")])),
                ),
            ])),
        )
        .build();
    harness.render(MATERIALIZATION, ctx)
}

/// Every SQL statement the materialization sent to the adapter, in call order.
fn issued_sql(harness: &MacroTestHarness) -> Vec<String> {
    harness
        .mock()
        .observed_calls()
        .iter()
        .filter(|call| call.method == "execute" || call.method == "add_query")
        .filter_map(|call| call.args.first().and_then(|v| v.as_str()))
        .map(|sql| sql.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

mod databricks {
    use super::*;

    #[test]
    fn seed_sql_does_not_depend_on_materialization_v2() {
        for (existing, create) in [
            (None, "create table"),
            (Some(RelationType::Table), "create or replace table"),
        ] {
            let issued: Vec<Vec<String>> = [false, true]
                .into_iter()
                .map(|materialization_v2| {
                    let harness = build_seed_harness(materialization_v2, seed_rows());
                    existing_relation(&harness, existing);
                    render_seed(&harness).unwrap_or_else(|e| {
                        panic!(
                            "seed (v2={materialization_v2}, existing={existing:?}) failed: {e:?}"
                        )
                    });
                    issued_sql(&harness)
                })
                .collect();

            assert!(
                issued[0].first().is_some_and(|sql| sql.starts_with(create))
                    && issued[0]
                        .iter()
                        .any(|sql| sql.starts_with("insert overwrite")),
                "expected {create} + insert for existing={existing:?}, got: {:?}",
                issued[0]
            );
            assert_eq!(
                issued[0], issued[1],
                "use_materialization_v2 changed seed SQL for existing={existing:?}"
            );
        }
    }

    #[test]
    fn seed_onto_existing_view_is_rejected() {
        for materialization_v2 in [false, true] {
            let harness = build_seed_harness(materialization_v2, seed_rows());
            existing_relation(&harness, Some(RelationType::View));

            let error = render_seed(&harness).expect_err("seeding onto a view must fail");

            assert!(
                error
                    .to_string()
                    .contains("it is a view or a materialized view"),
                "unexpected error (v2={materialization_v2}): {error}"
            );
            assert!(executed_sql(harness.mock()).is_empty());
        }
    }

    #[test]
    fn empty_seed_skips_row_insert() {
        let harness = build_seed_harness(false, empty_seed_rows());
        existing_relation(&harness, None);

        render_seed(&harness).expect("empty seed should materialize");

        let issued = issued_sql(&harness);
        assert!(
            issued.iter().any(|sql| sql.starts_with("create table")),
            "expected a create statement, got: {issued:?}"
        );
        assert!(
            !issued.iter().any(|sql| sql.starts_with("insert")),
            "empty seed must not insert rows, got: {issued:?}"
        );
    }
}
