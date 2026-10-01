use std::collections::BTreeMap;

use dbt_adapter_core::AdapterType;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, default_mock_config};

fn build_harness(replace_on: bool, insert_by_name: bool) -> MacroTestHarness {
    let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    harness.mock().on("has_dbr_capability", move |args| {
        Ok(Value::from(match args.first().and_then(Value::as_str) {
            Some("replace_on") => replace_on,
            Some("insert_by_name") => insert_by_name,
            _ => false,
        }))
    });
    harness.mock().on("quote", |args| {
        let identifier = args
            .first()
            .and_then(Value::as_str)
            .expect("quote receives an identifier");
        Ok(Value::from(format!("`{identifier}`")))
    });
    harness
}

fn render(
    harness: &MacroTestHarness,
    unique_key: Value,
    predicates: Value,
    target_columns: Value,
) -> String {
    harness
        .render(
            "{{ delete_insert_sql_impl('source', 'target', target_columns, unique_key, predicates) }}",
            BTreeMap::from([
                ("unique_key".to_string(), unique_key),
                ("predicates".to_string(), predicates),
                ("target_columns".to_string(), target_columns),
            ]),
        )
        .expect("delete+insert macro should render")
}

fn normalized(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn replace_on_quotes_non_ascii_unique_key() {
    let harness = build_harness(true, true);
    let sql = render(
        &harness,
        Value::from("あ"),
        Value::UNDEFINED,
        Value::from(vec![Value::from("`あ`"), Value::from("msg")]),
    );

    assert!(
        normalized(&sql).contains("replace on (target.`あ` <=> temp.`あ`)"),
        "non-ASCII key must be adapter-quoted: {sql}"
    );
}

#[test]
fn replace_on_supports_composite_key_and_predicates() {
    let harness = build_harness(true, true);
    let sql = render(
        &harness,
        Value::from(vec![Value::from("id"), Value::from("region")]),
        Value::from(vec![
            Value::from("id >= 2"),
            Value::from("region is not null"),
        ]),
        Value::from(vec![Value::from("id"), Value::from("region")]),
    );
    let sql = normalized(&sql);

    assert!(
        sql.contains(
            "replace on (target.`id` <=> temp.`id` and target.`region` <=> temp.`region`)"
        )
    );
    assert!(sql.contains("where id >= 2 and region is not null"));
}

#[test]
fn legacy_path_returns_ordered_delete_then_insert_statements() {
    let harness = build_harness(false, true);
    let sql = render(
        &harness,
        Value::from(vec![Value::from("id"), Value::from("region")]),
        Value::from("id >= 2"),
        Value::from(vec![Value::from("id"), Value::from("region")]),
    );
    let sql = normalized(&sql);
    let lower = sql.to_lowercase();

    let delete = lower.find("delete from target").expect("DELETE statement");
    let insert = lower
        .find("insert into target by name")
        .expect("INSERT statement");
    assert!(delete < insert, "DELETE must precede INSERT: {sql}");
    assert!(
        lower.contains("target.`id` in (select `id` from source)"),
        "id key must be quoted: {sql}"
    );
    assert!(
        lower.contains("target.`region` in (select `region` from source)"),
        "region key must be quoted: {sql}"
    );
    assert!(lower.contains("and id >= 2"));
}

#[test]
fn delete_insert_without_unique_key_is_rejected() {
    let mut harness = build_harness(true, true);
    harness.mock().on("valid_incremental_strategies", |_| {
        Ok(Value::from(vec![
            Value::from("append"),
            Value::from("merge"),
            Value::from("insert_overwrite"),
            Value::from("replace_where"),
            Value::from("delete+insert"),
        ]))
    });
    let config = default_mock_config();
    config.on("get", |args| {
        let key = args.first().and_then(Value::as_str);
        if key == Some("unique_key") {
            Ok(Value::from(()))
        } else {
            Ok(args.get(1).cloned().unwrap_or(Value::UNDEFINED))
        }
    });
    harness
        .env_mut()
        .env
        .add_global("config", Value::from_dyn_object(config));

    let error = harness
        .render(
            "{{ dbt_databricks_validate_get_incremental_strategy('delete+insert', 'delta') }}",
            BTreeMap::<String, Value>::new(),
        )
        .expect_err("delete+insert without unique_key must fail");

    assert!(
        error
            .to_string()
            .contains("This strategy requires 'unique_key' to be configured"),
        "unexpected validation error: {error}"
    );
}
