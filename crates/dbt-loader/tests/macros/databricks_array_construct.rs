use std::collections::BTreeMap;

use dbt_adapter_core::AdapterType;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

fn render(expression: &str) -> String {
    let sql =
        include_str!("../../src/dbt_macro_assets/dbt-databricks/macros/utils/array_construct.sql");
    let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .with_macro_at_path(
            "dbt_databricks",
            "databricks__array_construct",
            sql,
            "dbt_macro_assets/dbt-databricks/macros/utils/array_construct.sql",
        )
        .build()
        .expect("harness should build");
    let rendered = harness
        .render(expression, BTreeMap::<String, Value>::new())
        .expect("render should succeed");
    rendered.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn test_empty_array_is_cast_to_the_requested_type() {
    assert_eq!(
        render("{{ databricks__array_construct([], 'string') }}"),
        "cast(array() as array<string>)"
    );
}

#[test]
fn test_empty_array_with_default_integer_type() {
    assert_eq!(
        render("{{ databricks__array_construct([], 'int') }}"),
        "cast(array() as array<int>)"
    );
}

#[test]
fn test_non_empty_array_is_built_from_the_inputs() {
    assert_eq!(
        render("{{ databricks__array_construct([1, 2, 3], 'int') }}"),
        "array( 1 , 2 , 3 )"
    );
}
