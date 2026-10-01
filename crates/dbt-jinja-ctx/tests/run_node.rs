//! Tests covering `RunNodeCtx` end-to-end:
//!
//! 1. The typed ctx serializes to exactly the same key set today's
//!    `build_run_node_context<S>` BTreeMap produces — 18 base keys plus the
//!    optional `pre_hooks` / `post_hooks` / `load_agate_table` (skipped via
//!    `#[serde(skip_serializing_if = "Option::is_none")]` when absent).
//! 2. `Option<MinijinjaValue>` fields with `None` value MUST be omitted
//!    from the registered map (matching today's "only insert if present"
//!    semantic).
//! 3. `model` / `builtins` downcast contracts hold (same as compile-node).
//! 4. The `RunNodeCtx` JsonSchema has stable shape (snapshot test).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use dbt_jinja_ctx::{
    CompileBaseCtx, DbtNamespace, JinjaObject, LazyModelWrapper, MacroLookupContext, RunNodeCtx,
    to_jinja_btreemap, to_model_context_map,
};
use indexmap::IndexMap;
use minijinja::Value as MinijinjaValue;
use minijinja::machinery::Span;

fn fixture_run_node_ctx(
    pre_hooks: Option<MinijinjaValue>,
    post_hooks: Option<MinijinjaValue>,
    load_agate_table: Option<MinijinjaValue>,
) -> RunNodeCtx {
    let mut model_inner: IndexMap<String, MinijinjaValue> = IndexMap::new();
    model_inner.insert(
        "name".to_string(),
        MinijinjaValue::from("dbt_columns".to_string()),
    );

    let shared_model_map = to_model_context_map(model_inner);

    let mut builtins_inner: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    builtins_inner.insert("ref".to_string(), MinijinjaValue::from("ref-fn-stub"));
    builtins_inner.insert("config".to_string(), MinijinjaValue::from("config-stub"));

    RunNodeCtx {
        base: None,
        this: MinijinjaValue::from("this-stub"),
        database: "analytics".to_string(),
        schema: "public".to_string(),
        identifier: "dbt_columns".to_string(),
        pre_hooks,
        post_hooks,
        config: MinijinjaValue::from("run-config-stub"),
        model: JinjaObject::new(LazyModelWrapper::new(
            shared_model_map.clone(),
            PathBuf::from("/tmp/nonexistent.sql"),
        )),
        node: JinjaObject::new(LazyModelWrapper::new(
            shared_model_map,
            PathBuf::from("/tmp/nonexistent.sql"),
        )),
        connection_name: String::new(),
        store_result: MinijinjaValue::from("store-result-stub"),
        load_result: MinijinjaValue::from("load-result-stub"),
        store_raw_result: MinijinjaValue::from("store-raw-result-stub"),
        submit_python_job: MinijinjaValue::from("submit-python-job-stub"),
        context: JinjaObject::new(MacroLookupContext::new(
            "my_project".to_string(),
            None,
            BTreeSet::new(),
        )),
        write: MinijinjaValue::from("write-stub"),
        load_agate_table,
        builtins: MinijinjaValue::from_object(builtins_inner),
        target_package_name: "my_project".to_string(),
        current_path: "run/dbt_columns.sql".to_string(),
        current_span: MinijinjaValue::from_serialize(Span::default()),
    }
}

#[test]
fn run_node_ctx_serializes_to_expected_keys_minimal() {
    let ctx = fixture_run_node_ctx(None, None, None);
    let registered = to_jinja_btreemap(&ctx);
    let mut keys: Vec<&str> = registered.keys().map(|s| s.as_str()).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "TARGET_PACKAGE_NAME",
            "__minijinja_current_path",
            "__minijinja_current_span",
            "builtins",
            "config",
            "connection_name",
            "context",
            "database",
            "identifier",
            "load_result",
            "model",
            "node",
            "schema",
            "store_raw_result",
            "store_result",
            "submit_python_job",
            "this",
            "write",
        ],
        "run-node ctx must omit pre_hooks/post_hooks/load_agate_table when None \
         (matches the 'insert only if present' semantic of the BTreeMap-based \
         build_run_node_context)"
    );
}

#[test]
fn run_node_ctx_serializes_with_optionals() {
    let ctx = fixture_run_node_ctx(
        Some(MinijinjaValue::from("pre-hooks-stub")),
        Some(MinijinjaValue::from("post-hooks-stub")),
        Some(MinijinjaValue::from("load-agate-table-stub")),
    );
    let registered = to_jinja_btreemap(&ctx);
    assert!(registered.contains_key("pre_hooks"));
    assert!(registered.contains_key("post_hooks"));
    assert!(registered.contains_key("load_agate_table"));
}

#[test]
fn builtins_downcasts_to_btreemap_string_value() {
    let ctx = fixture_run_node_ctx(None, None, None);
    let registered = to_jinja_btreemap(&ctx);
    let builtins = registered.get("builtins").unwrap();
    let downcast = builtins
        .as_object()
        .and_then(|obj| obj.downcast::<BTreeMap<String, MinijinjaValue>>());
    assert!(downcast.is_some());
}

/// Base with values that DIFFER from [`fixture_run_node_ctx`] on every key
/// the two structs share, so the shadowing test below can tell which side
/// won. Also carries base-only keys (`execute`, `graph`, `ref`/`source`/
/// `function`, `MACRO_DISPATCH_ORDER`, a `dbt` namespace) that the overlay
/// does not define.
fn fixture_distinguishable_base() -> CompileBaseCtx {
    let mut macro_dispatch_order: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    macro_dispatch_order.insert(
        "dbt".to_string(),
        MinijinjaValue::from(vec!["dbt".to_string()]),
    );

    let mut base_builtins: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    base_builtins.insert("ref".to_string(), MinijinjaValue::from("base-ref"));

    let mut dbt_namespaces: BTreeMap<String, JinjaObject<DbtNamespace>> = BTreeMap::new();
    dbt_namespaces.insert(
        "dbt".to_string(),
        JinjaObject::new(DbtNamespace::new("dbt")),
    );

    CompileBaseCtx {
        macro_dispatch_order,
        ref_fn: MinijinjaValue::from("base-ref-fn"),
        source: MinijinjaValue::from("base-source-fn"),
        function: MinijinjaValue::from("base-function-fn"),
        execute: true,
        builtins: MinijinjaValue::from_object(base_builtins),
        dbt_metadata_envs: MinijinjaValue::from_object(BTreeMap::<String, MinijinjaValue>::new()),
        context: JinjaObject::new(MacroLookupContext::new(
            "base_project".to_string(),
            None,
            Default::default(),
        )),
        graph: MinijinjaValue::from("base-graph"),
        store_result: MinijinjaValue::from("base-store-result"),
        load_result: MinijinjaValue::from("base-load-result"),
        target_package_name: "base_project".to_string(),
        node: MinijinjaValue::from("base-node"),
        connection_name: "base-connection".to_string(),
        dbt_namespaces,
    }
}

/// The composition seam: `base: Some(..)` must flatten the base keys in AND
/// let the per-node overlay fields shadow the base entries they share. This
/// is what lets a typed caller pass `RunNodeCtx` straight to `eval` /
/// `render_named_str` with the same last-write-wins semantic the legacy
/// `base_context.clone(); ctx.extend(overlay)` BTreeMap path produced.
#[test]
fn base_some_flattens_and_is_shadowed_by_per_node_fields() {
    let mut ctx = fixture_run_node_ctx(None, None, None);
    ctx.base = Some(fixture_distinguishable_base());
    let registered = to_jinja_btreemap(&ctx);

    // Base-only keys (the overlay has no such fields) must be present.
    for base_only in [
        "execute",
        "graph",
        "MACRO_DISPATCH_ORDER",
        "ref",
        "source",
        "function",
        "dbt_metadata_envs",
        "dbt", // dbt_namespaces flattened to a top-level key
    ] {
        assert!(
            registered.contains_key(base_only),
            "base-only key `{base_only}` must flatten in from CompileBaseCtx"
        );
    }

    // Shadowed keys: the per-node overlay value must win over the base value.
    assert_eq!(
        registered
            .get("TARGET_PACKAGE_NAME")
            .and_then(|v| v.as_str()),
        Some("my_project"),
        "overlay TARGET_PACKAGE_NAME must shadow base `base_project`"
    );
    assert_eq!(
        registered.get("connection_name").and_then(|v| v.as_str()),
        Some(""),
        "overlay connection_name must shadow base `base-connection`"
    );
    assert_eq!(
        registered.get("store_result").and_then(|v| v.as_str()),
        Some("store-result-stub"),
        "overlay store_result must shadow the base closure"
    );
    // `node` is a `LazyModelWrapper` object on the overlay vs a plain string
    // on the base — the overlay object must win.
    assert!(
        registered.get("node").and_then(|v| v.as_object()).is_some(),
        "overlay `node` (LazyModelWrapper) must shadow the base string node"
    );
}

#[test]
fn run_node_ctx_json_schema_snapshot() {
    let schema = schemars::schema_for!(RunNodeCtx);
    insta::assert_json_snapshot!("run_node_ctx_schema", schema);
}
