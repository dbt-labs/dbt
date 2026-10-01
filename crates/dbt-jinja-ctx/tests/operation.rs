//! Tests covering `OperationCtx` end-to-end:
//!
//! 1. The typed ctx serializes to the historical
//!    `build_operation_context_btreemap` BTreeMap shape (14 base keys +
//!    `config: DummyConfig` + per-namespace flatten).
//! 2. The `config` slot downcasts to `DummyConfig` after the serde
//!    round-trip — this pins the operation-scope contract that REPL /
//!    `run-operation` / pre-compile macro evaluation rely on.
//! 3. The `OperationCtx` JsonSchema has stable shape (snapshot test).

use std::collections::BTreeMap;

use dbt_jinja_ctx::{
    CompileBaseCtx, DbtNamespace, DummyConfig, JinjaObject, MacroLookupContext, OperationCtx,
    to_jinja_btreemap,
};
use minijinja::Value as MinijinjaValue;

fn fixture_operation_ctx() -> OperationCtx {
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

    OperationCtx {
        base: CompileBaseCtx {
            macro_dispatch_order,
            ref_fn: MinijinjaValue::from("ref-fn-stub"),
            source: MinijinjaValue::from("source-fn-stub"),
            function: MinijinjaValue::from("function-fn-stub"),
            execute: true,
            builtins: MinijinjaValue::from_object(builtins_inner),
            dbt_metadata_envs: MinijinjaValue::from_object(
                BTreeMap::<String, MinijinjaValue>::new(),
            ),
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
        },
        config: JinjaObject::new(DummyConfig),
    }
}

#[test]
fn operation_ctx_serializes_to_expected_keys() {
    let ctx = fixture_operation_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let mut keys: Vec<&str> = registered.keys().map(|s| s.as_str()).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "MACRO_DISPATCH_ORDER",
            "TARGET_PACKAGE_NAME",
            "builtins",
            "config",
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
        "OperationCtx must produce all CompileBaseCtx keys plus `config` \
         (the DummyConfig overlay) — this is the shape today's \
         `build_operation_context_btreemap` BTreeMap exposes"
    );
}

/// Regression test: the operation-scope `config` MUST round-trip back to
/// a `DummyConfig` Object (not a serde-serialized stand-in) so REPL /
/// `run-operation` / pre-compile macro paths see the documented no-op
/// `call(...)` and `call_method("get", ...)` behaviour.
#[test]
fn config_downcasts_to_dummy_config() {
    let ctx = fixture_operation_ctx();
    let registered = to_jinja_btreemap(&ctx);
    let config = registered.get("config").expect("config must be registered");
    let downcast = config
        .as_object()
        .and_then(|obj| obj.downcast::<DummyConfig>());
    assert!(
        downcast.is_some(),
        "OperationCtx.config must round-trip to a `DummyConfig` Object"
    );
}

#[test]
fn operation_ctx_json_schema_snapshot() {
    let schema = schemars::schema_for!(OperationCtx);
    insta::assert_json_snapshot!("operation_ctx_schema", schema);
}
