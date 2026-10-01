//! Tests covering `CompileNodeCtx` end-to-end:
//!
//! 1. The typed ctx serializes to exactly the same key set today's
//!    `build_compile_node_context_inner<T>` BTreeMap produces — 18 keys
//!    including the underscore-decorated `TARGET_PACKAGE_NAME` /
//!    `TARGET_UNIQUE_ID` / `__minijinja_current_path` /
//!    `__minijinja_current_span` / `__dbt_current_execution_phase__`
//!    constants.
//! 2. `model` and `builtins` (constructed via
//!    `Value::from_object(BTreeMap<String, MinijinjaValue>)`) MUST downcast
//!    back to that exact concrete type — same downcast contract as
//!    `ResolveModelCtx`.
//! 3. The `CompileNodeCtx` JsonSchema has stable shape (snapshot test).

use std::collections::{BTreeMap, BTreeSet};

use dbt_jinja_ctx::{CompileNodeCtx, JinjaObject, MacroLookupContext, to_jinja_btreemap};
use minijinja::Value as MinijinjaValue;
use minijinja::machinery::Span;

fn fixture_compile_node_ctx() -> CompileNodeCtx {
    let mut model_inner: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    model_inner.insert(
        "name".to_string(),
        MinijinjaValue::from("dbt_columns".to_string()),
    );

    let mut builtins_inner: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    builtins_inner.insert("ref".to_string(), MinijinjaValue::from("ref-fn-stub"));
    builtins_inner.insert("config".to_string(), MinijinjaValue::from("config-stub"));

    CompileNodeCtx {
        base: None,
        this: MinijinjaValue::from("this-stub"),
        database: "analytics".to_string(),
        schema: "public".to_string(),
        identifier: "dbt_columns".to_string(),
        config: MinijinjaValue::from("compile-config-stub"),
        ref_fn: MinijinjaValue::from("ref-fn-stub"),
        source: MinijinjaValue::from("source-fn-stub"),
        function: MinijinjaValue::from("function-fn-stub"),
        builtins: MinijinjaValue::from_object(builtins_inner),
        model: MinijinjaValue::from_object(model_inner),
        store_result: MinijinjaValue::from("store-result-stub"),
        load_result: MinijinjaValue::from("load-result-stub"),
        store_raw_result: MinijinjaValue::from("store-raw-result-stub"),
        target_package_name: "my_project".to_string(),
        target_unique_id: "my_project.dbt_columns".to_string(),
        context: JinjaObject::new(MacroLookupContext::new(
            "my_project".to_string(),
            None,
            BTreeSet::new(),
        )),
        current_path: "models/dbt_columns.sql".to_string(),
        current_span: MinijinjaValue::from_serialize(Span::default()),
        current_execution_phase: "render".to_string(),
    }
}

#[test]
fn compile_node_ctx_serializes_to_expected_keys() {
    let ctx = fixture_compile_node_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let mut keys: Vec<&str> = registered.keys().map(|s| s.as_str()).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "TARGET_PACKAGE_NAME",
            "TARGET_UNIQUE_ID",
            "__dbt_current_execution_phase__",
            "__minijinja_current_path",
            "__minijinja_current_span",
            "builtins",
            "config",
            "context",
            "database",
            "function",
            "identifier",
            "load_result",
            "model",
            "ref",
            "schema",
            "source",
            "store_raw_result",
            "store_result",
            "this",
        ],
        "compile-node ctx must produce exactly the 19 keys today's \
         build_compile_node_context_inner overlay produced"
    );
}

#[test]
fn builtins_downcasts_to_btreemap_string_value() {
    let ctx = fixture_compile_node_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let builtins = registered.get("builtins").unwrap();
    let downcast = builtins
        .as_object()
        .and_then(|obj| obj.downcast::<BTreeMap<String, MinijinjaValue>>());
    assert!(downcast.is_some());
}

#[test]
fn model_downcasts_to_btreemap_string_value() {
    let ctx = fixture_compile_node_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let model = registered.get("model").unwrap();
    let downcast = model
        .as_object()
        .and_then(|obj| obj.downcast::<BTreeMap<String, MinijinjaValue>>());
    assert!(downcast.is_some());
}

#[test]
fn current_execution_phase_is_render() {
    let ctx = fixture_compile_node_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let phase = registered.get("__dbt_current_execution_phase__").unwrap();
    assert_eq!(phase.as_str(), Some("render"));
}

#[test]
fn compile_node_ctx_json_schema_snapshot() {
    let schema = schemars::schema_for!(CompileNodeCtx);
    insta::assert_json_snapshot!("compile_node_ctx_schema", schema);
}
