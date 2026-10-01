use std::collections::BTreeMap;

use dbt_adapter_core::AdapterType;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, assert_executed_contains};

fn build_harness() -> MacroTestHarness {
    let mut harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    harness
        .env_mut()
        .env
        .add_global("execute", Value::from(true));

    harness
}

#[test]
fn create_schema_accepts_string_argument() {
    let harness = build_harness();
    let ctx = BTreeMap::from([("relation".to_string(), Value::from("func"))]);

    harness
        .render("{{ spark__create_schema(relation) }}", ctx)
        .expect("create_schema should accept a string argument");

    assert_executed_contains(harness.mock(), "create schema if not exists func");
}

#[test]
fn drop_schema_accepts_string_argument() {
    let harness = build_harness();
    let ctx = BTreeMap::from([("relation".to_string(), Value::from("func"))]);

    harness
        .render("{{ spark__drop_schema(relation) }}", ctx)
        .expect("drop_schema should accept a string argument");

    assert_executed_contains(harness.mock(), "drop schema if exists func cascade");
}
