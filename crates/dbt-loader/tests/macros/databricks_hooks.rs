use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use dbt_adapter_core::AdapterType;
use dbt_jinja_ctx::objects::run::HookConfig;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, executed_sql};

const ORDINARY: &str = "insert into audit values ('ordinary')";
const OUTSIDE: &str = "insert into audit values ('outside')";

fn hook(sql: &str, transaction: bool) -> Value {
    Value::from_object(HookConfig {
        sql: sql.to_string(),
        transaction,
    })
}

/// `TARGET_PACKAGE_NAME` makes unprefixed `run_hooks` resolve the way it does
/// for a model; without it lookup falls back to `dbt-adapters`' definition.
fn hooks_ctx(pre_hooks: Vec<Value>, post_hooks: Vec<Value>) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("pre_hooks".to_string(), Value::from(pre_hooks)),
        ("post_hooks".to_string(), Value::from(post_hooks)),
        ("execute".to_string(), Value::from(true)),
        (
            "TARGET_PACKAGE_NAME".to_string(),
            Value::from("test_project"),
        ),
    ])
}

/// Builds a harness with every Databricks macro loaded and a `render` that
/// records the hook SQL it is asked to render.
fn build_harness(flag_enabled: bool) -> (MacroTestHarness, Arc<Mutex<Vec<String>>>) {
    let mut harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .load_all_macros()
        .with_stub_functions()
        .with_behavior_flag("use_non_transactional_hooks", flag_enabled)
        .build()
        .expect("harness should build");

    let rendered = Arc::new(Mutex::new(Vec::new()));
    let recorded = rendered.clone();
    harness
        .env_mut()
        .env
        .add_function("render", move |sql: String| {
            recorded.lock().unwrap().push(sql.clone());
            Ok(Value::from(sql))
        });
    (harness, rendered)
}

fn executed(harness: &MacroTestHarness) -> Vec<String> {
    executed_sql(harness.mock())
        .iter()
        .map(|sql| sql.trim().to_string())
        .collect()
}

fn render_hooks(harness: &MacroTestHarness, ctx: BTreeMap<String, Value>) {
    harness
        .render("{{ run_pre_hooks() }}{{ run_post_hooks() }}", ctx)
        .expect("hooks should render");
}

#[test]
fn outside_hooks_are_skipped_without_rendering_when_flag_disabled() {
    let (harness, rendered) = build_harness(false);
    render_hooks(
        &harness,
        hooks_ctx(
            vec![hook(OUTSIDE, false), hook(ORDINARY, true)],
            vec![hook(ORDINARY, true), hook(OUTSIDE, false)],
        ),
    );

    assert_eq!(executed(&harness), [ORDINARY, ORDINARY]);
    assert_eq!(*rendered.lock().unwrap(), [ORDINARY, ORDINARY]);
}

#[test]
fn outside_hooks_run_without_commit_when_flag_enabled() {
    let (harness, _) = build_harness(true);
    render_hooks(
        &harness,
        hooks_ctx(
            vec![hook(ORDINARY, true), hook(OUTSIDE, false)],
            vec![hook(OUTSIDE, false), hook(ORDINARY, true)],
        ),
    );

    // Outside hooks run before ordinary pre-hooks and after ordinary post-hooks.
    assert_eq!(executed(&harness), [OUTSIDE, ORDINARY, ORDINARY, OUTSIDE]);
}

#[test]
fn ordinary_hooks_run_when_flag_disabled() {
    let (harness, _) = build_harness(false);
    render_hooks(
        &harness,
        hooks_ctx(vec![hook(ORDINARY, true)], vec![hook(ORDINARY, true)]),
    );

    assert_eq!(executed(&harness), [ORDINARY, ORDINARY]);
}

#[test]
fn empty_rendered_hook_is_not_executed() {
    let (harness, rendered) = build_harness(false);
    render_hooks(&harness, hooks_ctx(vec![hook("  ", true)], vec![]));

    assert!(executed(&harness).is_empty());
    assert_eq!(*rendered.lock().unwrap(), ["  "]);
}
