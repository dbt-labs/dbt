//! Tests for `dbt-snowflake/macros/utils/changes.sql`, the change source of a custom incremental
//! dynamic table.

use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::{RelationObject, create_relation};
use dbt_adapter_core::AdapterType;
use dbt_schemas::filter::RunFilter;
use dbt_schemas::schemas::relations::SNOWFLAKE_RESOLVED_QUOTING;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

const CHANGES_PATH: &str = "dbt-snowflake/macros/utils/changes.sql";
const CHANGES_SQL: &str =
    include_str!("../../src/dbt_macro_assets/dbt-snowflake/macros/utils/changes.sql");

fn harness() -> MacroTestHarness {
    MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .with_macro_at_path("dbt_snowflake", "changes", CHANGES_SQL, CHANGES_PATH)
        .build()
        .expect("harness should build")
}

/// Render `template` with `ctx` as the context.
fn render_with(template: &str, ctx: BTreeMap<String, Value>) -> Result<String, String> {
    harness()
        .render(template, ctx)
        .map(|rendered| rendered.trim().to_string())
        .map_err(|e| format!("{e:?}"))
}

/// Render `template` with `model` holding `model_keys`, as the run context's `model` does.
fn render(template: &str, model_keys: &[(&str, Value)]) -> Result<String, String> {
    let model: BTreeMap<String, Value> = model_keys
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect();
    render_with(
        template,
        BTreeMap::from([("model".to_string(), Value::from_serialize(model))]),
    )
}

/// The `RAW.PUBLIC.ORDERS` relation `source()` returns, with `--empty`'s run filter when `empty`.
fn orders_relation(empty: bool) -> Value {
    let relation = Arc::from(
        create_relation(
            AdapterType::Snowflake,
            "RAW".to_string(),
            "PUBLIC".to_string(),
            Some("ORDERS".to_string()),
            None,
            SNOWFLAKE_RESOLVED_QUOTING,
        )
        .expect("create_relation"),
    );
    let object = if empty {
        RelationObject::new_with_filter(
            relation,
            RunFilter {
                empty: true,
                sample: None,
            },
            None,
        )
    } else {
        RelationObject::new(relation)
    };
    object.into_value()
}

/// The context of a `dbt run`, `--empty` when `empty`: `orders` is the source relation.
fn run_context(empty: bool) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("orders".to_string(), orders_relation(empty)),
        (
            "flags".to_string(),
            Value::from_serialize(BTreeMap::from([("EMPTY", empty)])),
        ),
    ])
}

#[test]
fn default_mode_renders_unbounded_changes() {
    assert_eq!(
        render("{{ changes('DB.SC.ORDERS', 'default') }}", &[]).unwrap(),
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
fn mode_is_case_insensitive() {
    assert_eq!(
        render("{{ changes('DB.SC.ORDERS', 'Append_Only') }}", &[]).unwrap(),
        "DB.SC.ORDERS changes(information => APPEND_ONLY)"
    );
}

#[test]
fn mode_is_required() {
    let err = render("{{ changes('DB.SC.ORDERS') }}", &[]).unwrap_err();
    assert!(err.contains("`mode` is required"), "got: {err}");
}

#[test]
fn unknown_mode_raises() {
    let err = render("{{ changes('DB.SC.ORDERS', mode='latest') }}", &[]).unwrap_err();
    assert!(err.contains("unsupported mode 'latest'"), "got: {err}");
}

#[test]
fn a_relation_renders_its_name() {
    assert_eq!(
        render_with("{{ changes(orders, 'default') }}", run_context(false)).unwrap(),
        "RAW.PUBLIC.ORDERS changes(information => DEFAULT)"
    );
}

#[test]
fn empty_run_filters_after_changes() {
    // Under --empty the relation itself renders as a zero-row subquery, which CHANGES can't follow.
    assert_eq!(
        render_with("{{ orders }}", run_context(true)).unwrap(),
        "(select * from RAW.PUBLIC.ORDERS where false limit 0)"
    );
    assert_eq!(
        render_with("{{ changes(orders, 'default') }} as o", run_context(true)).unwrap(),
        "(select * from RAW.PUBLIC.ORDERS changes(information => DEFAULT) limit 0) as o"
    );
}

#[test]
fn renders_without_a_model_in_context() {
    assert_eq!(
        render_with("{{ changes('DB.SC.ORDERS', 'default') }}", BTreeMap::new()).unwrap(),
        "DB.SC.ORDERS changes(information => DEFAULT)"
    );
}

#[test]
fn resolves_as_dbt_changes() {
    // A project's own `changes` macro takes precedence; `dbt.changes(...)` reaches the built-in.
    let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .load_all_macros()
        .build()
        .expect("harness should build");
    // The run context's `dbt` global, which resolves dbt's and the adapter's internal macros.
    let ctx = BTreeMap::from([(
        "dbt".to_string(),
        Value::from_object(dbt_jinja_ctx::DbtNamespace::new("dbt")),
    )]);
    let rendered = harness
        .render("{{ dbt.changes('DB.SC.ORDERS', 'default') }}", ctx)
        .map_err(|e| format!("{e:?}"));
    assert_eq!(
        rendered.as_deref().map(str::trim),
        Ok("DB.SC.ORDERS changes(information => DEFAULT)")
    );
}
