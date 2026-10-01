//! Tests covering `ResolveModelCtx` end-to-end:
//!
//! 1. The typed ctx serializes to exactly the same key set today's
//!    `build_resolve_model_context<T>` BTreeMap produces — 17 keys including
//!    the underscore-decorated `TARGET_UNIQUE_ID` /
//!    `__minijinja_current_path` / `__minijinja_current_span` constants.
//! 2. `model` and `builtins` (constructed via
//!    `Value::from_object(BTreeMap<String, MinijinjaValue>)`) MUST downcast
//!    back to that exact concrete type. Compile/run-node-context code
//!    relies on this; serde-walking would produce a `MutableMap<Value,
//!    Value>` instead and silently break the downcast — same trap PR 3
//!    caught with `MACRO_DISPATCH_ORDER`'s `Vec<String>` values.
//! 3. The `ResolveModelCtx` JsonSchema has stable shape (snapshot test).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use dbt_jinja_ctx::{
    JinjaObject, MacroLookupContext, ParseExecute, ResolveModelCtx, to_jinja_btreemap,
};
use minijinja::Value as MinijinjaValue;
use minijinja::machinery::Span;
use minijinja::value::mutable_map::MutableMap;

fn fixture_resolve_model_ctx() -> ResolveModelCtx {
    let mut model_inner: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    model_inner.insert(
        "name".to_string(),
        MinijinjaValue::from("dbt_columns".to_string()),
    );
    model_inner.insert(
        "package_name".to_string(),
        MinijinjaValue::from("my_project".to_string()),
    );

    let mut builtins_inner: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    builtins_inner.insert("ref".to_string(), MinijinjaValue::from("ref-fn-stub"));
    builtins_inner.insert("source".to_string(), MinijinjaValue::from("source-fn-stub"));
    builtins_inner.insert("config".to_string(), MinijinjaValue::from("config-stub"));

    ResolveModelCtx {
        this: MinijinjaValue::from("this-stub"),
        ref_fn: MinijinjaValue::from("ref-fn-stub"),
        source: MinijinjaValue::from("source-fn-stub"),
        function: MinijinjaValue::from("function-fn-stub"),
        metric: MinijinjaValue::from("metric-fn-stub"),
        config: MinijinjaValue::from("config-stub"),
        model: MinijinjaValue::from_object(model_inner),
        builtins: MinijinjaValue::from_object(builtins_inner),
        graph: MinijinjaValue::from_dyn_object(Arc::new(MutableMap::new())),
        store_result: MinijinjaValue::from("store-result-stub"),
        load_result: MinijinjaValue::from("load-result-stub"),
        store_raw_result: MinijinjaValue::from("store-raw-result-stub"),
        execute: JinjaObject::new(ParseExecute::new(Arc::new(AtomicBool::new(false)))),
        context: JinjaObject::new(MacroLookupContext::new(
            "my_project".to_string(),
            None,
            BTreeSet::new(),
        )),
        target_unique_id: "my_project.dbt_columns".to_string(),
        current_path: "models/dbt_columns.sql".to_string(),
        current_span: MinijinjaValue::from_serialize(Span::default()),
    }
}

#[test]
fn resolve_model_ctx_serializes_to_seventeen_keys() {
    let ctx = fixture_resolve_model_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let mut keys: Vec<&str> = registered.keys().map(|s| s.as_str()).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "TARGET_UNIQUE_ID",
            "__minijinja_current_path",
            "__minijinja_current_span",
            "builtins",
            "config",
            "context",
            "execute",
            "function",
            "graph",
            "load_result",
            "metric",
            "model",
            "ref",
            "source",
            "store_raw_result",
            "store_result",
            "this",
        ],
        "resolve-model ctx must produce exactly the 17 keys today's \
         build_resolve_model_context<T> BTreeMap registered"
    );
}

/// Regression test: compile/run-node-context code does
/// `.downcast_ref::<BTreeMap<String, MinijinjaValue>>()` on `builtins` to
/// overlay validated ref/source/function/config per-node. The underlying
/// Object type must be `BTreeMap<String, MinijinjaValue>` exactly. Going
/// through serde would produce a `MutableMap<Value, Value>` instead and
/// silently break the downcast (same shape trap PR 3 hit with
/// `MACRO_DISPATCH_ORDER`).
#[test]
fn builtins_downcasts_to_btreemap_string_value() {
    let ctx = fixture_resolve_model_ctx();
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
         compile_node_context.rs:155 / run_node_context.rs:304 rely on this"
    );
    let map = downcast.unwrap();
    assert!(map.contains_key("ref"));
    assert!(map.contains_key("source"));
    assert!(map.contains_key("config"));
}

/// `model` is also constructed via `Value::from_object(BTreeMap<String,
/// MinijinjaValue>)`. While I don't see a downcast against it in current
/// code (downstream reads use `.get_attr("name")`/`.get_attr("config")`
/// which work on any map-shaped Value), preserving the concrete Object type
/// guards against future code that does downcast — and keeps the wire
/// shape byte-identical to the original `from_object(model_map)`.
#[test]
fn model_downcasts_to_btreemap_string_value() {
    let ctx = fixture_resolve_model_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let model = registered.get("model").expect("model must be registered");
    let downcast = model
        .as_object()
        .and_then(|obj| obj.downcast::<BTreeMap<String, MinijinjaValue>>());
    assert!(
        downcast.is_some(),
        "model must downcast to BTreeMap<String, MinijinjaValue> — \
         preserves the same concrete Object type the original \
         from_object(model_map) produced"
    );
}

#[test]
fn target_unique_id_serializes_as_string_value() {
    let ctx = fixture_resolve_model_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let value = registered
        .get("TARGET_UNIQUE_ID")
        .expect("TARGET_UNIQUE_ID must be registered");
    assert_eq!(value.as_str(), Some("my_project.dbt_columns"));
}

/// `graph` must survive registration as the *same* interior-mutable map
/// object, not a serde-rematerialized copy: parse-time macros stash scratch
/// state on it and read it back from a later render. dbt-labs/fs#13454.
#[test]
fn graph_is_a_shared_mutable_mapping_at_resolve_model() {
    let ctx = fixture_resolve_model_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let graph = registered.get("graph").expect("graph must be registered");
    let map = graph
        .as_object()
        .and_then(|obj| obj.downcast_ref::<MutableMap>())
        .expect("graph must register as a MutableMap");
    assert_eq!(
        map.keys().len(),
        0,
        "graph starts empty at parse-model scope"
    );
    map.insert(MinijinjaValue::from("scratch"), MinijinjaValue::from(1));
    assert_eq!(
        registered
            .get("graph")
            .and_then(|g| g.get_attr("scratch").ok())
            .and_then(|v| v.as_i64()),
        Some(1),
        "a write through the registered graph must be visible on re-read"
    );
}

#[test]
fn resolve_model_ctx_json_schema_snapshot() {
    let schema = schemars::schema_for!(ResolveModelCtx);
    insta::assert_json_snapshot!("resolve_model_ctx_schema", schema);
}
