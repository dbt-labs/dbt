//! Tests covering `LoadCtx` end-to-end:
//!
//! 1. The typed ctx serializes to exactly the same key set today's hand-built
//!    `BTreeMap` does in `phases/load/init.rs` — `run_started_at` + `target` +
//!    `flags`.
//! 2. The Object identity of `run_started_at` (a `JinjaObject<PyDateTime>`)
//!    survives the serde round-trip — i.e. the registered global is still a
//!    `PyDateTime`-backed object, not a data-shaped value.
//! 3. The `LoadCtx` JsonSchema has stable shape (snapshot test).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::TimeZone;
use chrono_tz::Tz;
use dbt_jinja_ctx::{LoadCtx, register_globals_from_serialize};
use minijinja::Environment;
use minijinja::Value as MinijinjaValue;
use minijinja_contrib::modules::py_datetime::datetime::PyDateTime;

fn fixture_load_ctx() -> LoadCtx {
    let run_started_at = Tz::UTC.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();

    let mut target_inner: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    target_inner.insert("name".to_string(), MinijinjaValue::from("dev"));
    target_inner.insert("schema".to_string(), MinijinjaValue::from("public"));
    let target = Arc::new(target_inner);

    let mut flags = BTreeMap::new();
    flags.insert("FULL_REFRESH".to_string(), MinijinjaValue::from(false));

    LoadCtx::new(run_started_at, target, flags)
}

fn ctx_keys(ctx: &LoadCtx) -> Vec<String> {
    let value = MinijinjaValue::from_serialize(ctx);
    let object = value
        .as_object()
        .expect("LoadCtx must serialize to a map-shaped value");
    let mut keys: Vec<String> = object
        .try_iter_pairs()
        .expect("LoadCtx must serialize to a map-shaped value")
        .filter_map(|(k, _)| k.as_str().map(|s| s.to_string()))
        .collect();
    keys.sort();
    keys
}

#[test]
fn load_ctx_serializes_to_exactly_three_keys() {
    let ctx = fixture_load_ctx();
    assert_eq!(
        ctx_keys(&ctx),
        vec![
            "flags".to_string(),
            "run_started_at".to_string(),
            "target".to_string(),
        ],
        "load env globals must be exactly run_started_at + target + flags"
    );
}

#[test]
fn run_started_at_preserves_pydatetime_object_identity() {
    let ctx = fixture_load_ctx();
    let mut env = Environment::new();
    register_globals_from_serialize(&mut env, &ctx);

    let value = env
        .get_global("run_started_at")
        .expect("run_started_at must be registered");

    let object = value
        .as_object()
        .expect("run_started_at must round-trip as an Object");
    assert!(
        object.downcast::<PyDateTime>().is_some(),
        "run_started_at lost its PyDateTime identity through serde — \
         JinjaObject<T> smuggle path is broken"
    );
}

#[test]
fn target_and_flags_survive_registration_as_expected_values() {
    let ctx = fixture_load_ctx();
    let mut env = Environment::new();
    register_globals_from_serialize(&mut env, &ctx);

    let target = env.get_global("target").expect("target must be registered");
    let target_obj = target.as_object().expect("target must be an object");
    let name = target_obj
        .get_value(&MinijinjaValue::from("name"))
        .and_then(|v| v.as_str().map(|s| s.to_string()));
    assert_eq!(name.as_deref(), Some("dev"));

    let flags = env.get_global("flags").expect("flags must be registered");
    let flags_obj = flags.as_object().expect("flags must be an object");
    let full_refresh = flags_obj
        .get_value(&MinijinjaValue::from("FULL_REFRESH"))
        .map(|v| v.is_true());
    assert_eq!(full_refresh, Some(false));
}

#[test]
fn load_ctx_json_schema_snapshot() {
    let schema = schemars::schema_for!(LoadCtx);
    insta::assert_json_snapshot!("load_ctx_schema", schema);
}
