//! Tests covering `ResolveBaseCtx` end-to-end:
//!
//! 1. The typed ctx serializes to the same key set today's hand-built
//!    `build_resolve_context` BTreeMap produces — fifteen fixed keys plus one
//!    top-level entry per `dbt_namespace` (via `#[serde(flatten)]`).
//! 2. The `dbt_namespaces` flatten emits each namespace as its own
//!    top-level Jinja key (not as a nested `dbt_namespaces.foo` path).
//! 3. The `ResolveBaseCtx` JsonSchema has stable shape (snapshot test).

use std::collections::{BTreeMap, BTreeSet};

use dbt_jinja_ctx::{
    DbtNamespace, JinjaObject, MacroLookupContext, ResolveBaseCtx, to_jinja_btreemap,
};
use minijinja::Value as MinijinjaValue;

fn fixture_resolve_base_ctx() -> ResolveBaseCtx {
    let mut macro_dispatch_order: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    macro_dispatch_order.insert(
        "dbt".to_string(),
        MinijinjaValue::from(vec!["dbt".to_string()]),
    );

    let mut dbt_namespaces: BTreeMap<String, JinjaObject<DbtNamespace>> = BTreeMap::new();
    dbt_namespaces.insert(
        "dbt".to_string(),
        JinjaObject::new(DbtNamespace::new("dbt")),
    );
    dbt_namespaces.insert(
        "snowflake".to_string(),
        JinjaObject::new(DbtNamespace::new("snowflake")),
    );

    ResolveBaseCtx {
        doc: MinijinjaValue::from("doc-stub"),
        macro_dispatch_order,
        target_package_name: "my_project".to_string(),
        execute: false,
        context: JinjaObject::new(MacroLookupContext::new(
            "my_project".to_string(),
            None,
            BTreeSet::from(["my_project".to_string()]),
        )),
        node: MinijinjaValue::NONE,
        connection_name: String::new(),
        store_result: MinijinjaValue::from("store-result-stub"),
        load_result: MinijinjaValue::from("load-result-stub"),
        store_raw_result: MinijinjaValue::from("store-raw-result-stub"),
        builtins: MinijinjaValue::from("builtins-stub"),
        ref_fn: MinijinjaValue::from("ref-stub"),
        source: MinijinjaValue::from("source-stub"),
        metric: MinijinjaValue::from("metric-stub"),
        function: MinijinjaValue::from("function-stub"),
        dbt_namespaces,
    }
}

#[test]
fn resolve_base_ctx_serializes_to_expected_keys() {
    let ctx = fixture_resolve_base_ctx();
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
            "doc",
            "execute",
            "function",
            "load_result",
            "metric",
            "node",
            "ref",
            "snowflake",
            "source",
            "store_raw_result",
            "store_result",
        ],
        "resolve-base ctx must produce the fifteen base keys plus one entry per \
         dbt_namespace via #[serde(flatten)]"
    );
}

#[test]
fn context_exposes_root_project_name_at_resolve_base() {
    let registered = to_jinja_btreemap(&fixture_resolve_base_ctx());
    let project_name = registered
        .get("context")
        .expect("context must be present at resolve-base scope")
        .get_attr("project_name")
        .expect("context.project_name must resolve");

    assert_eq!(project_name.as_str(), Some("my_project"));
}

/// Regression: a custom `generate_schema_name` that calls `run_query(...)`
/// unguarded reaches `load_result` through `statement`, and
/// `generate_component_name` evaluates those macros against the bare base ctx.
/// When the three result-store functions were only on `ResolveModelCtx`, the
/// macro raised `UnknownFunction` and `generate_database_and_schema` silently
/// fell back to the profile schema.
#[test]
fn result_store_fns_present_at_resolve_base() {
    let ctx = fixture_resolve_base_ctx();
    let registered = to_jinja_btreemap(&ctx);
    for key in ["store_result", "load_result", "store_raw_result"] {
        assert!(
            registered.contains_key(key),
            "`{key}` must be present at resolve-base scope; \
             generate_*_name macros run against this ctx and dbt-core's \
             MacroContext always exposes it"
        );
    }
}

#[test]
fn dbt_namespaces_flatten_to_top_level_keys() {
    let ctx = fixture_resolve_base_ctx();
    let registered = to_jinja_btreemap(&ctx);

    // The map field name `dbt_namespaces` MUST NOT appear — it's flattened
    // into per-namespace top-level keys.
    assert!(
        !registered.contains_key("dbt_namespaces"),
        "`dbt_namespaces` flatten leaked the field name as a top-level key"
    );

    // Each namespace value must round-trip back to a `DbtNamespace` Object,
    // not a serde-serialized stand-in (the dispatch path on
    // `{{ dbt.macro_name }}` calls `Object::get_property`).
    let dbt_obj = registered
        .get("dbt")
        .and_then(|v| v.as_object())
        .and_then(|o| o.downcast::<DbtNamespace>())
        .expect("`dbt` namespace must downcast to DbtNamespace");
    assert_eq!(dbt_obj.name, "dbt");

    let snowflake_obj = registered
        .get("snowflake")
        .and_then(|v| v.as_object())
        .and_then(|o| o.downcast::<DbtNamespace>())
        .expect("`snowflake` namespace must downcast to DbtNamespace");
    assert_eq!(snowflake_obj.name, "snowflake");
}

#[test]
fn execute_is_false_at_resolve_base() {
    let ctx = fixture_resolve_base_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let execute = registered
        .get("execute")
        .expect("execute must be present at resolve-base scope");
    assert!(
        !execute.is_true(),
        "execute must be false at resolve-base; flips to true at compile/run"
    );
}

#[test]
fn node_is_none_at_resolve_base() {
    let ctx = fixture_resolve_base_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let node = registered
        .get("node")
        .expect("node must be present at resolve-base scope");
    assert!(
        node.is_none(),
        "node must be Value::NONE at base; populated per-model"
    );
}

#[test]
fn resolve_base_ctx_json_schema_snapshot() {
    let schema = schemars::schema_for!(ResolveBaseCtx);
    insta::assert_json_snapshot!("resolve_base_ctx_schema", schema);
}

/// Regression test: minijinja's dispatch lookup
/// (`fs/sa/crates/dbt-jinja/minijinja/src/dispatch_object.rs`) does
/// `order.downcast_object::<Vec<String>>()` to read the per-namespace search
/// order from `MACRO_DISPATCH_ORDER`. The underlying Object type therefore
/// MUST be exactly `Vec<String>` — not the `MutableVec<Value>` that
/// serde-serializing a `Vec<String>` would produce. This test asserts the
/// downcast still works after the typed-ctx round-trip; if it ever flips
/// back to a serde-serialized `Vec<String>` field, this fails fast and
/// the dispatch_logic_test goldie wouldn't have to.
#[test]
fn macro_dispatch_order_values_downcast_to_vec_string() {
    let ctx = fixture_resolve_base_ctx();
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
