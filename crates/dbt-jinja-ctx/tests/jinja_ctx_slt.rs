//! SLT-style cross-phase Jinja context tests.
//!
//! Each `#[test]` here builds the typed-ctx fixtures available at the current
//! migration step, then runs an `.slt` file against them via the harness in
//! `common/jinja_ctx_slt.rs`. Phases that haven't landed yet are simply not
//! populated in the fixtures map; rows referencing them are silently skipped
//! until a future migration step fills them in.

#[path = "common/jinja_ctx_slt.rs"]
mod jinja_ctx_slt;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use chrono::TimeZone;
use chrono_tz::Tz;
use dbt_jinja_ctx::{
    DbtNamespace, GlobalCore, JinjaObject, LoadCtx, MacroLookupContext, ParseExecute,
    ResolveBaseCtx, ResolveCore, ResolveModelCtx,
};
use dbt_jinja_vars::ConfiguredVar;
use minijinja::Value as MinijinjaValue;
use minijinja::machinery::Span;
use minijinja_contrib::modules::py_datetime::datetime::PyDateTime;
use minijinja_contrib::modules::pytz::PytzTimezone;

use jinja_ctx_slt::{PhaseFixtures, run_slt};

fn fixture_target() -> Arc<BTreeMap<String, MinijinjaValue>> {
    let mut t: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    t.insert("name".to_string(), MinijinjaValue::from("dev"));
    t.insert("schema".to_string(), MinijinjaValue::from("public"));
    t.insert("database".to_string(), MinijinjaValue::from("analytics"));
    Arc::new(t)
}

fn fixture_load_flags() -> BTreeMap<String, MinijinjaValue> {
    let mut f = BTreeMap::new();
    f.insert("FULL_REFRESH".to_string(), MinijinjaValue::from(false));
    f.insert("STORE_FAILURES".to_string(), MinijinjaValue::from(false));
    f.insert("INTROSPECT".to_string(), MinijinjaValue::from(true));
    f
}

fn fixture_load_ctx() -> LoadCtx {
    let run_started_at = Tz::UTC.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
    LoadCtx::new(run_started_at, fixture_target(), fixture_load_flags())
}

fn fixture_resolve_core() -> ResolveCore {
    let run_started_at = Tz::UTC.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
    let target = fixture_target();
    let mut invocation_args_dict: BTreeMap<String, MinijinjaValue> = BTreeMap::new();
    invocation_args_dict.insert("debug".to_string(), MinijinjaValue::from(false));

    ResolveCore {
        global_core: GlobalCore {
            run_started_at: JinjaObject::new(PyDateTime::new_aware(
                run_started_at,
                Some(PytzTimezone::new(Tz::UTC)),
            )),
            target: target.clone(),
            // Fixture stand-in for the parse-phase `Flags` Object — a bare
            // Map is observationally equivalent at the .slt level (key
            // lookups work the same way through `get_attr`). The real
            // `Flags` Object identity is exercised end-to-end via the
            // `dbt-cli clean_project` goldie.
            flags: MinijinjaValue::from_serialize(fixture_load_flags()),
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

    ResolveModelCtx {
        this: MinijinjaValue::from("this-stub"),
        ref_fn: MinijinjaValue::from("ref-fn-stub"),
        source: MinijinjaValue::from("source-fn-stub"),
        function: MinijinjaValue::from("function-fn-stub"),
        metric: MinijinjaValue::from("metric-fn-stub"),
        config: MinijinjaValue::from("config-stub"),
        model: MinijinjaValue::from_object(model_inner),
        builtins: MinijinjaValue::from_object(builtins_inner),
        graph: MinijinjaValue::UNDEFINED,
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

fn fixtures() -> PhaseFixtures {
    PhaseFixtures {
        load: Some(MinijinjaValue::from_serialize(fixture_load_ctx())),
        resolve_globals: Some(MinijinjaValue::from_serialize(fixture_resolve_core())),
        resolve_base: Some(MinijinjaValue::from_serialize(fixture_resolve_base_ctx())),
        resolve_model: Some(MinijinjaValue::from_serialize(fixture_resolve_model_ctx())),
        ..PhaseFixtures::default()
    }
}

#[test]
fn load_basics_slt() {
    run_slt("tests/data/jinja_ctx_slt/load_basics.slt", &fixtures())
}

#[test]
fn resolve_globals_basics_slt() {
    run_slt(
        "tests/data/jinja_ctx_slt/resolve_globals_basics.slt",
        &fixtures(),
    )
}

#[test]
fn resolve_base_basics_slt() {
    run_slt(
        "tests/data/jinja_ctx_slt/resolve_base_basics.slt",
        &fixtures(),
    );
}

#[test]
fn resolve_model_basics_slt() {
    run_slt(
        "tests/data/jinja_ctx_slt/resolve_model_basics.slt",
        &fixtures(),
    );
}

#[test]
fn cross_phase_keyset_slt() {
    run_slt(
        "tests/data/jinja_ctx_slt/cross_phase_keyset.slt",
        &fixtures(),
    )
}

/// Smoke-test: a deliberately-wrong expectation must surface as a clear
/// `expected ... got ...` panic. Guards the harness against silently passing
/// when its comparator is broken.
#[test]
#[should_panic(expected = "expected `\"not-dev\"`, got `\"dev\"`")]
fn slt_harness_reports_value_mismatch() {
    run_slt(
        "tests/data/jinja_ctx_slt/_negative_value_mismatch.slt",
        &fixtures(),
    )
}
