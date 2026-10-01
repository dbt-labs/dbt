//! Macro resolution when one environment serves several adapters.
//!
//! Two properties have to hold together, and neither is provable from a
//! single-adapter harness:
//!
//! * *intra-chain overriding is preserved* — an `lake_compute` node gets DuckDB's
//!   `run_hooks`, which overrides `dbt-adapters`' by being later in the chain;
//! * *chains stay isolated* — a Snowflake node in the same environment gets
//!   `dbt-adapters`' `run_hooks`, not DuckDB's.
//!
//! `run_hooks` is the sharpest probe available: `dbt-adapters` and `dbt-duckdb`
//! both define it *unprefixed*, and they differ observably. DuckDB's version
//! exists precisely to drop the extra `commit;` that `dbt-adapters` emits, since
//! DuckDB opens no transaction when a connection is created.

use std::collections::BTreeMap;

use dbt_adapter_core::AdapterType;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

/// Stand-in for `statement`, which would otherwise reach the adapter. Echoes its
/// body so the caller can see which branches ran.
const STATEMENT_STUB: &str = r#"
{% macro statement(name=none, fetch_result=False, auto_begin=True, language='sql') %}
  [stmt]{{ caller() }}[/stmt]
{%- endmacro %}
"#;

/// `render()` is a runtime builtin the hooks macro calls on each hook's SQL.
const RENDER_STUB: &str = r#"
{% macro render(sql) %}{{ sql }}{% endmacro %}
"#;

/// One environment serving Snowflake (the target default) plus `lake_compute`.
///
/// Loads the real internal packages, so this also exercises
/// `construct_internal_packages` over the union of both adapters' chains --
/// `dbt-adapters`, `dbt-snowflake`, `dbt-lakecompute`, `dbt-duckdb`.
fn snowflake_plus_lake_compute() -> MacroTestHarness {
    MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .with_extra_adapters([AdapterType::LakeCompute])
        .load_all_macros()
        .with_macro("dbt", "statement", STATEMENT_STUB)
        .with_macro("dbt", "render", RENDER_STUB)
        .build()
        .expect("harness should build")
}

/// A single out-of-transaction hook: the input that makes the two `run_hooks`
/// implementations diverge.
///
/// `TARGET_PACKAGE_NAME` is load-bearing. Unprefixed lookup tries the *current*
/// package first, and that key defaults to `"dbt"` when absent -- which would
/// find `dbt-adapters`' definition before the dialect-selected namespace is ever
/// consulted, for every dialect. A model body always carries its own package
/// name here, so the test must too or it proves nothing.
fn hooks_ctx() -> BTreeMap<String, Value> {
    let hook = BTreeMap::from([
        ("sql".to_string(), Value::from("select 1")),
        ("transaction".to_string(), Value::from(false)),
    ]);
    BTreeMap::from([
        (
            "hooks".to_string(),
            Value::from_serialize(vec![Value::from_serialize(hook)]),
        ),
        (
            "TARGET_PACKAGE_NAME".to_string(),
            Value::from("test_project"),
        ),
    ])
}

const TEMPLATE: &str = "{{ run_hooks(hooks, inside_transaction=False) }}";

#[test]
fn lake_compute_node_gets_duckdbs_run_hooks_override() {
    let harness = snowflake_plus_lake_compute();
    let rendered = harness
        .render_for(AdapterType::LakeCompute, TEMPLATE, hooks_ctx())
        .expect("render should succeed");

    assert!(
        rendered.contains("select 1"),
        "the hook itself should still run, got: {rendered:?}"
    );
    assert!(
        !rendered.contains("commit;"),
        "an `lake_compute` node must get DuckDB's `run_hooks`, which drops the extra \
         `commit;`. Getting `dbt-adapters`' version means the inheritance chain \
         is not being walked. Rendered: {rendered:?}"
    );
}

#[test]
fn snowflake_node_gets_dbt_adapters_run_hooks() {
    let harness = snowflake_plus_lake_compute();
    let rendered = harness
        .render(TEMPLATE, hooks_ctx())
        .expect("render should succeed");

    assert!(
        rendered.contains("commit;"),
        "a Snowflake node must get `dbt-adapters`' `run_hooks`; DuckDB's must not \
         leak across chains just because `lake_compute` is also declared. Rendered: {rendered:?}"
    );
}

/// The same environment, rendered both ways, must disagree. Guards against a
/// future change that makes both nodes resolve to one definition — which would
/// leave both assertions above passing for the wrong reason if the shared
/// definition happened to match.
#[test]
fn the_two_dialects_resolve_to_different_definitions() {
    let harness = snowflake_plus_lake_compute();
    let as_lake_compute = harness
        .render_for(AdapterType::LakeCompute, TEMPLATE, hooks_ctx())
        .expect("render should succeed");
    let as_snowflake = harness
        .render(TEMPLATE, hooks_ctx())
        .expect("render should succeed");

    assert_ne!(
        as_lake_compute.split_whitespace().collect::<Vec<_>>(),
        as_snowflake.split_whitespace().collect::<Vec<_>>(),
        "`lake_compute` and Snowflake must resolve `run_hooks` to different definitions"
    );
}
