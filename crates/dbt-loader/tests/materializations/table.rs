use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, cache_added_types, default_mock_config};

const POST_HOOK_SQL: &str = "select 'post_hook_sentinel'";

fn render_table_over(
    existing_type: RelationType,
    use_materialization_v2: bool,
) -> MacroTestHarness {
    let mut harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .load_all_macros()
        .with_stub_functions()
        .with_behavior_flag("use_materialization_v2", use_materialization_v2)
        .with_behavior_flag("use_catalogs_v2", false)
        .with_behavior_flag("use_managed_iceberg", false)
        .build()
        .expect("harness should build");
    harness
        .env_mut()
        .env
        .add_function("var", |_name: Value, default: Option<Value>| {
            Ok(default.unwrap_or(Value::UNDEFINED))
        });

    let mock = harness.mock();
    let existing = harness.relation("TEST_DB", "TEST_SCHEMA", "my_table", Some(existing_type));
    mock.on("get_relation", move |_| {
        Ok(RelationObject::new(Arc::clone(&existing)).into_value())
    });
    for method in [
        "drop_relation",
        "cache_dropped",
        "cache_added",
        "commit",
        "optimize",
    ] {
        mock.on(method, |_| Ok(Value::UNDEFINED));
    }
    mock.on("clean_sql", |args| {
        Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
    });
    mock.on("get_column_tags_from_model", |_| Ok(Value::UNDEFINED));
    mock.on("get_columns_in_relation", |_| {
        Ok(Value::from(Vec::<Value>::new()))
    });
    mock.on("parse_columns_and_constraints", |_| {
        Ok(Value::from(vec![
            Value::from(Vec::<Value>::new()),
            Value::from(Vec::<Value>::new()),
        ]))
    });
    mock.on("resolve_file_format", |_| Ok(Value::from("delta")));
    mock.on("is_uniform", |_| Ok(Value::from(false)));
    let catalog_relation = Value::from_serialize(BTreeMap::from([
        ("file_format", "delta"),
        ("table_format", "delta"),
    ]));
    mock.on("build_catalog_relation", move |_| {
        Ok(catalog_relation.clone())
    });
    mock.on("has_dbr_capability", |_| Ok(Value::from(false)));
    mock.on("is_cluster", |_| Ok(Value::from(false)));

    let post_hooks = Value::from_serialize(vec![BTreeMap::from([
        ("sql", Value::from(POST_HOOK_SQL)),
        ("transaction", Value::from(true)),
    ])]);
    let ctx = harness
        .materialization_context("my_table", "select 1 as id")
        .relation_type(RelationType::Table)
        .with("post_hooks", post_hooks)
        .with(
            "model",
            Value::from_serialize(BTreeMap::from([
                ("alias", Value::from("my_table")),
                ("unique_id", Value::from("model.test_project.my_table")),
                ("columns", Value::from(BTreeMap::<String, Value>::new())),
                ("language", Value::from("sql")),
                (
                    "config",
                    Value::from_serialize(BTreeMap::from([("materialized", "table")])),
                ),
            ])),
        )
        .config({
            let config = default_mock_config();
            config.set_attr("materialized", Value::from("table"));
            Value::from_dyn_object(config)
        })
        .build();
    harness
        .render("{{ materialization_table_databricks() }}", ctx)
        .unwrap_or_else(|e| panic!("table materialization over {existing_type:?} failed: {e:?}"));
    harness
}

fn cache_added_before_post_hook(harness: &MacroTestHarness) -> bool {
    let calls = harness.mock().observed_calls();
    let cached = calls.iter().position(|call| call.method == "cache_added");
    let post_hook = calls
        .iter()
        .position(|call| {
            call.method == "execute"
                && call
                    .args
                    .first()
                    .and_then(Value::as_str)
                    .is_some_and(|sql| sql.contains(POST_HOOK_SQL))
        })
        .expect("post-hook should execute");
    cached.is_some_and(|cached| cached < post_hook)
}

#[test]
fn table_replacing_a_view_is_cached_before_post_hooks() {
    for use_materialization_v2 in [false, true] {
        let harness = render_table_over(RelationType::View, use_materialization_v2);
        assert_eq!(
            cache_added_types(harness.mock()),
            ["table"],
            "use_materialization_v2={use_materialization_v2}"
        );
        assert!(
            cache_added_before_post_hook(&harness),
            "use_materialization_v2={use_materialization_v2}: the new table must be cached before post-hooks"
        );
    }
}

#[test]
fn table_rebuilt_in_place_keeps_its_cache_entry() {
    for use_materialization_v2 in [false, true] {
        let harness = render_table_over(RelationType::Table, use_materialization_v2);
        assert!(
            cache_added_types(harness.mock()).is_empty(),
            "use_materialization_v2={use_materialization_v2}"
        );
    }
}
