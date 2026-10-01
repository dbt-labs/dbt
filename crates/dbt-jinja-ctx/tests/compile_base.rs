//! Tests covering `CompileBaseCtx` end-to-end:
//!
//! 1. The typed ctx serializes to the same key set today's hand-built
//!    `build_operation_context_btreemap` BTreeMap produces — 14 fixed keys
//!    plus one top-level entry per `dbt_namespace` (via `#[serde(flatten)]`).
//! 2. `MACRO_DISPATCH_ORDER` per-namespace values must downcast to
//!    `Vec<String>` (the type minijinja's dispatch lookup expects).
//! 3. `builtins` must downcast to `BTreeMap<String, MinijinjaValue>` —
//!    `compile_node_context_inner` reads this back via downcast on every
//!    per-node pass.
//! 4. The `CompileBaseCtx` JsonSchema has stable shape (snapshot test).

use std::collections::BTreeMap;

use dbt_jinja_ctx::{
    CompileBaseCtx, DbtNamespace, JinjaObject, MacroLookupContext, to_jinja_btreemap,
};
use minijinja::Value as MinijinjaValue;

fn fixture_compile_base_ctx() -> CompileBaseCtx {
    let mut macro_dispatch_order: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    macro_dispatch_order.insert(
        "dbt".to_string(),
        MinijinjaValue::from(vec!["dbt".to_string()]),
    );

    let mut builtins_inner: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    builtins_inner.insert("ref".to_string(), MinijinjaValue::from("ref-fn-stub"));
    builtins_inner.insert("source".to_string(), MinijinjaValue::from("source-fn-stub"));
    builtins_inner.insert(
        "function".to_string(),
        MinijinjaValue::from("function-fn-stub"),
    );

    let mut dbt_namespaces: BTreeMap<String, JinjaObject<DbtNamespace>> = BTreeMap::new();
    dbt_namespaces.insert(
        "dbt".to_string(),
        JinjaObject::new(DbtNamespace::new("dbt")),
    );

    CompileBaseCtx {
        macro_dispatch_order,
        ref_fn: MinijinjaValue::from("ref-fn-stub"),
        source: MinijinjaValue::from("source-fn-stub"),
        function: MinijinjaValue::from("function-fn-stub"),
        execute: true,
        builtins: MinijinjaValue::from_object(builtins_inner),
        dbt_metadata_envs: MinijinjaValue::from_object(BTreeMap::<String, MinijinjaValue>::new()),
        context: JinjaObject::new(MacroLookupContext::new(
            "my_project".to_string(),
            None,
            Default::default(),
        )),
        graph: MinijinjaValue::UNDEFINED,
        store_result: MinijinjaValue::from("store-result-stub"),
        load_result: MinijinjaValue::from("load-result-stub"),
        target_package_name: "my_project".to_string(),
        node: MinijinjaValue::NONE,
        connection_name: String::new(),
        dbt_namespaces,
    }
}

#[test]
fn compile_base_ctx_serializes_to_expected_keys() {
    let ctx = fixture_compile_base_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let mut keys: Vec<&str> = registered.keys().map(|s| s.as_str()).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "MACRO_DISPATCH_ORDER",
            "TARGET_PACKAGE_NAME",
            "builtins",
            "connection_name",
            "context",
            "dbt",
            "dbt_metadata_envs",
            "execute",
            "function",
            "graph",
            "load_result",
            "node",
            "ref",
            "source",
            "store_result",
        ],
        "compile-base ctx must produce 14 fixed keys plus one entry per \
         dbt_namespace via #[serde(flatten)]"
    );
}

#[test]
fn execute_is_true_at_compile_base() {
    let ctx = fixture_compile_base_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let execute = registered
        .get("execute")
        .expect("execute must be present at compile-base scope");
    assert!(
        execute.is_true(),
        "execute must be true at compile/run scope (flipped from false at parse)"
    );
}

/// Regression test: minijinja's dispatch lookup
/// (`fs/sa/crates/dbt-jinja/minijinja/src/dispatch_object.rs`) does
/// `order.downcast_object::<Vec<String>>()` to read the per-namespace
/// search order from `MACRO_DISPATCH_ORDER`. The underlying Object type
/// MUST be `Vec<String>` exactly — same downcast contract as
/// `ResolveBaseCtx` enforces.
#[test]
fn macro_dispatch_order_values_downcast_to_vec_string() {
    let ctx = fixture_compile_base_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let order_value = registered
        .get("MACRO_DISPATCH_ORDER")
        .expect("MACRO_DISPATCH_ORDER must be registered");
    let order_obj = order_value
        .as_object()
        .expect("MACRO_DISPATCH_ORDER must be an Object map");
    let dbt_order = order_obj
        .get_value(&MinijinjaValue::from("dbt"))
        .expect("dbt namespace must have an entry in MACRO_DISPATCH_ORDER");
    let downcast = dbt_order
        .as_object()
        .and_then(|obj| obj.downcast::<Vec<String>>());
    assert!(
        downcast.is_some(),
        "MACRO_DISPATCH_ORDER per-namespace value must downcast to \
         `Vec<String>` (the type minijinja's dispatch lookup expects)"
    );
    assert_eq!(downcast.unwrap().as_ref(), &vec!["dbt".to_string()]);
}

/// Regression test: `compile_node_context_inner` reads the base
/// `builtins` back via
/// `.downcast_ref::<BTreeMap<String, MinijinjaValue>>()` on every
/// per-node pass to overlay the validated ref/source/function/config.
/// The underlying Object MUST be that exact concrete type. Going through
/// serde would produce a `MutableMap<Value, Value>` instead and silently
/// break the downcast.
#[test]
fn builtins_downcasts_to_btreemap_string_value() {
    let ctx = fixture_compile_base_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let builtins = registered
        .get("builtins")
        .expect("builtins must be registered");
    let downcast = builtins
        .as_object()
        .and_then(|obj| obj.downcast::<BTreeMap<String, MinijinjaValue>>());
    assert!(
        downcast.is_some(),
        "builtins must downcast to BTreeMap<String, MinijinjaValue> — \
         compile_node_context_inner / build_run_node_context rely on this"
    );
    let map = downcast.unwrap();
    assert!(map.contains_key("ref"));
    assert!(map.contains_key("source"));
    assert!(map.contains_key("function"));
}

#[test]
fn dbt_namespaces_flatten_to_top_level_keys() {
    let ctx = fixture_compile_base_ctx();
    let registered = to_jinja_btreemap(&ctx);
    assert!(
        !registered.contains_key("dbt_namespaces"),
        "`dbt_namespaces` flatten leaked the field name as a top-level key"
    );
    let dbt_obj = registered
        .get("dbt")
        .and_then(|v| v.as_object())
        .and_then(|o| o.downcast::<DbtNamespace>())
        .expect("`dbt` namespace must downcast to DbtNamespace");
    assert_eq!(dbt_obj.name, "dbt");
}

#[test]
fn compile_base_ctx_json_schema_snapshot() {
    let schema = schemars::schema_for!(CompileBaseCtx);
    insta::assert_json_snapshot!("compile_base_ctx_schema", schema);
}
