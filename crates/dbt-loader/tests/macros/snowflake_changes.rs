//! Tests for `dbt-snowflake/macros/utils/changes.sql`, the change source of a custom incremental
//! dynamic table.

use std::collections::BTreeMap;

use dbt_adapter_core::AdapterType;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

const CHANGES_PATH: &str = "dbt-snowflake/macros/utils/changes.sql";
const CHANGES_SQL: &str =
    include_str!("../../src/dbt_macro_assets/dbt-snowflake/macros/utils/changes.sql");

/// Render `template` with `model` holding `model_keys`, as the run context's `model` does.
fn render(template: &str, model_keys: &[(&str, Value)]) -> Result<String, String> {
    let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .with_macro_at_path("dbt_snowflake", "changes", CHANGES_SQL, CHANGES_PATH)
        .build()
        .expect("harness should build");
    let model: BTreeMap<String, Value> = model_keys
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect();
    let ctx = BTreeMap::from([("model".to_string(), Value::from_serialize(model))]);
    harness
        .render(template, ctx)
        .map(|rendered| rendered.trim().to_string())
        .map_err(|e| format!("{e:?}"))
}

#[test]
fn renders_unbounded_changes_by_default() {
    assert_eq!(
        render("{{ changes('DB.SC.ORDERS') }}", &[]).unwrap(),
        "DB.SC.ORDERS changes(information => DEFAULT)"
    );
}

#[test]
fn append_only_mode() {
    assert_eq!(
        render("{{ changes('DB.SC.ORDERS', mode='append_only') }}", &[]).unwrap(),
        "DB.SC.ORDERS changes(information => APPEND_ONLY)"
    );
}

#[test]
fn discovery_starts_the_changes_now() {
    assert_eq!(
        render(
            "{{ changes('DB.SC.ORDERS') }} as o",
            &[("_dbt_changes_discovery", Value::from(true))]
        )
        .unwrap(),
        "DB.SC.ORDERS changes(information => DEFAULT) at(timestamp => current_timestamp()) as o"
    );
}

#[test]
fn renders_without_a_model_in_context() {
    let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .with_macro_at_path("dbt_snowflake", "changes", CHANGES_SQL, CHANGES_PATH)
        .build()
        .expect("harness should build");
    let rendered = harness
        .render(
            "{{ changes('DB.SC.ORDERS') }}",
            BTreeMap::<String, Value>::new(),
        )
        .expect("changes() should render outside a model");
    assert_eq!(
        rendered.trim(),
        "DB.SC.ORDERS changes(information => DEFAULT)"
    );
}

#[test]
fn unknown_mode_raises() {
    let err = render("{{ changes('DB.SC.ORDERS', mode='latest') }}", &[]).unwrap_err();
    assert!(err.contains("unsupported mode 'latest'"), "got: {err}");
}
