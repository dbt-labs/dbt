//! Tests covering `ResolveCore` end-to-end:
//!
//! 1. The typed ctx serializes to exactly the same key set today's hand-built
//!    parse-env `BTreeMap` did in `phases/parse/init.rs` — 11 keys in a fixed
//!    order.
//! 2. Object identity for `var` (a `JinjaObject<ConfiguredVar>`) survives the
//!    serde round-trip — the registered global is still a `ConfiguredVar`,
//!    not a data-shaped value.
//! 3. The `ResolveCore` JsonSchema has stable shape (snapshot test).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::TimeZone;
use chrono_tz::Tz;
use dbt_jinja_ctx::{GlobalCore, JinjaObject, ResolveCore, register_globals_from_serialize};
use dbt_jinja_vars::ConfiguredVar;
use minijinja::Environment;
use minijinja::Value as MinijinjaValue;
use minijinja_contrib::modules::py_datetime::datetime::PyDateTime;
use minijinja_contrib::modules::pytz::PytzTimezone;

fn fixture_resolve_core() -> ResolveCore {
    let run_started_at = Tz::UTC.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();

    let mut target_inner: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    target_inner.insert("name".to_string(), MinijinjaValue::from("dev"));
    target_inner.insert("schema".to_string(), MinijinjaValue::from("public"));
    let target = Arc::new(target_inner);

    let mut flags_inner: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    flags_inner.insert("FULL_REFRESH".to_string(), MinijinjaValue::from(false));

    let mut invocation_args_dict: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    invocation_args_dict.insert("debug".to_string(), MinijinjaValue::from(false));

    ResolveCore {
        global_core: GlobalCore {
            run_started_at: JinjaObject::new(PyDateTime::new_aware(
                run_started_at,
                Some(PytzTimezone::new(Tz::UTC)),
            )),
            target: target.clone(),
            flags: MinijinjaValue::from_serialize(flags_inner),
        },
        project_name: "my_project".to_string(),
        env: target,
        invocation_args_dict,
        invocation_id: "11111111-2222-3333-4444-555555555555".to_string(),
        var: JinjaObject::new(ConfiguredVar::new(BTreeMap::new(), BTreeMap::new())),
        database: Some("analytics".to_string()),
        schema: Some("public".to_string()),
        write: MinijinjaValue::NONE,
    }
}

fn ctx_keys(ctx: &ResolveCore) -> Vec<String> {
    let value = MinijinjaValue::from_serialize(ctx);
    let object = value
        .as_object()
        .expect("ResolveCore must serialize to a map-shaped value");
    let mut keys: Vec<String> = object
        .try_iter_pairs()
        .expect("ResolveCore must serialize to a map-shaped value")
        .filter_map(|(k, _)| k.as_str().map(|s| s.to_string()))
        .collect();
    keys.sort();
    keys
}

#[test]
fn resolve_core_serializes_to_exactly_eleven_keys() {
    let ctx = fixture_resolve_core();
    let keys = ctx_keys(&ctx);
    let expected = [
        "database",
        "env",
        "flags",
        "invocation_args_dict",
        "invocation_id",
        "project_name",
        "run_started_at",
        "schema",
        "target",
        "var",
        "write",
    ];
    let expected: Vec<String> = expected.iter().map(|&s| s.to_string()).collect();
    assert_eq!(
        keys, expected,
        "parse env globals must be exactly the 11 keys today's BTreeMap registered"
    );
}

#[test]
fn var_preserves_configured_var_object_identity() {
    let ctx = fixture_resolve_core();
    let mut env = Environment::new();
    register_globals_from_serialize(&mut env, &ctx);

    let value = env
        .get_global("var")
        .expect("var must be registered as a global");
    let object = value.as_object().expect("var must round-trip as an Object");
    assert!(
        object.downcast::<ConfiguredVar>().is_some(),
        "var lost its ConfiguredVar identity through serde — \
         JinjaObject<T> smuggle path is broken for ConfiguredVar"
    );
}

#[test]
fn target_and_env_alias_to_the_same_payload() {
    let ctx = fixture_resolve_core();
    let mut env = Environment::new();
    register_globals_from_serialize(&mut env, &ctx);

    let target = env.get_global("target").expect("target must be registered");
    let env_global = env.get_global("env").expect("env must be registered");

    let target_name = target
        .as_object()
        .and_then(|o| o.get_value(&MinijinjaValue::from("name")))
        .and_then(|v| v.as_str().map(|s| s.to_string()));
    let env_name = env_global
        .as_object()
        .and_then(|o| o.get_value(&MinijinjaValue::from("name")))
        .and_then(|v| v.as_str().map(|s| s.to_string()));
    assert_eq!(target_name, env_name);
    assert_eq!(target_name.as_deref(), Some("dev"));
}

#[test]
fn write_global_is_none() {
    let ctx = fixture_resolve_core();
    let mut env = Environment::new();
    register_globals_from_serialize(&mut env, &ctx);
    let write = env.get_global("write").expect("write must be registered");
    assert!(write.is_none(), "write global must be Value::NONE at parse");
}

#[test]
fn resolve_core_json_schema_snapshot() {
    let schema = schemars::schema_for!(ResolveCore);
    insta::assert_json_snapshot!("resolve_core_schema", schema);
}
