use std::collections::BTreeMap;
use std::path::Path;

use super::*;
use crate::adapter::Adapter;
use crate::adapter::adapter_impl::AdapterImpl;
use crate::sql_types::DefaultTypeOps;
use crate::stmt_splitter::DefaultStmtSplitter;
use dbt_adapter_core::AdapterType;

use dbt_common::cancellation::never_cancels;
use dbt_schemas::schemas::relations::{DEFAULT_DBT_QUOTING, DEFAULT_RESOLVED_QUOTING};
use indexmap::IndexMap;

/// Helper to call [Adapter::call_method_impl] with jinja-valued arguments.
fn dispatch_test(
    adapter: &Arc<Adapter>,
    name: &str,
    args: &[Value],
) -> Result<Value, minijinja::Error> {
    let env = minijinja::Environment::new();
    let state = State::new_for_env(&env);
    adapter.call_method_impl(&state, name, args, &[])
}

/// As `dispatch_test`, but with a `dialect` global standing in for the render
/// context a node that selected an adapter via `+adapter` would carry.
fn dispatch_test_with_dialect(
    adapter: &Arc<Adapter>,
    dialect: &str,
    name: &str,
    args: &[Value],
) -> Result<Value, minijinja::Error> {
    let mut env = minijinja::Environment::new();
    env.add_global(minijinja::constants::DIALECT, Value::from(dialect));
    let state = State::new_for_env(&env);
    adapter.call_method_impl(&state, name, args, &[])
}

/// Minimal listener that opts into introspective-hole rendering, standing in
/// for `dbt_jinja_utils::listener::SymbolicRenderingEventListener` (which
/// this crate doesn't depend on): `Adapter::call_method`'s Parse-mode
/// taint-wrapping is gated on a listener like this being present, so tests
/// asserting a result *is* tainted need one, matching how only
/// `JinjaRenderMode::Symbolic` behaves in production.
#[derive(Debug)]
struct TaintGateListener;

impl RenderingEventListener for TaintGateListener {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "TaintGateListener"
    }

    fn on_macro_start(&self, _file_path: Option<&Path>, _line: &u32, _col: &u32, _offset: &u32) {}

    fn on_macro_stop(&self, _file_path: Option<&Path>, _line: &u32, _col: &u32, _offset: &u32) {}

    fn on_malicious_return(&self, _location: &minijinja::CodeLocation) {}

    fn on_function_start(&self) {}

    fn on_function_end(&self) {}

    fn wants_introspective_holes(&self) -> bool {
        true
    }
}

/// Create a Typed-phase DuckDB adapter backed by MockEngine.
fn make_duckdb_adapter() -> Arc<Adapter> {
    make_mock_adapter(AdapterType::DuckDB)
}

/// Create a parse-phase DuckDB adapter (returns defaults, no real execution).
fn make_duckdb_parse_adapter() -> Arc<Adapter> {
    let adapter = Adapter::new_parse_phase_adapter(
        AdapterType::DuckDB,
        dbt_yaml::Mapping::new(),
        DEFAULT_DBT_QUOTING,
        Arc::new(DefaultTypeOps::new(AdapterType::DuckDB)),
        None,
    );
    Arc::new(adapter)
}

/// Helper to build a minijinja dict Value from key-value pairs.
fn dict(pairs: &[(&str, &str)]) -> Value {
    let map: IndexMap<String, Value> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), Value::from(*v)))
        .collect();
    Value::from(map)
}

// -- external_root tests --------------------------------------------------

#[test]
fn test_external_root_default() {
    let adapter = make_duckdb_adapter();
    let result = dispatch_test(&adapter, "external_root", &[]).unwrap();
    assert_eq!(result.as_str().unwrap(), ".");
}

// TODO: test external_root with custom config once MockAdapter supports custom AdapterConfig

// -- external_write_options tests (ported from dbt-duckdb test_external_utils.py) --

#[test]
fn test_external_write_options_csv_inferred() {
    let adapter = make_duckdb_adapter();
    let result = dispatch_test(
        &adapter,
        "external_write_options",
        &[Value::from("/tmp/test.csv"), dict(&[])],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "format csv, header 1");
}

#[test]
fn test_external_write_options_parquet_with_codec() {
    let adapter = make_duckdb_adapter();
    let result = dispatch_test(
        &adapter,
        "external_write_options",
        &[Value::from("./foo.parquet"), dict(&[("codec", "zstd")])],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "codec zstd, format parquet");
}

#[test]
fn test_external_write_options_delimiter_infers_csv() {
    let adapter = make_duckdb_adapter();
    let result = dispatch_test(
        &adapter,
        "external_write_options",
        &[
            Value::from("bar"),
            dict(&[("delimiter", "|"), ("header", "0")]),
        ],
    )
    .unwrap();
    assert_eq!(
        result.as_str().unwrap(),
        "delimiter '|', header 0, format csv"
    );
}

#[test]
fn test_external_write_options_partition_by_single() {
    let adapter = make_duckdb_adapter();
    let result = dispatch_test(
        &adapter,
        "external_write_options",
        &[Value::from("a.parquet"), dict(&[("partition_by", "ds")])],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "partition_by ds, format parquet");
}

#[test]
fn test_external_write_options_partition_by_multi_adds_parens() {
    let adapter = make_duckdb_adapter();
    let result = dispatch_test(
        &adapter,
        "external_write_options",
        &[
            Value::from("b.csv"),
            dict(&[("partition_by", "ds,category")]),
        ],
    )
    .unwrap();
    assert_eq!(
        result.as_str().unwrap(),
        "partition_by (ds,category), format csv, header 1"
    );
}

#[test]
fn test_external_write_options_null_quoted() {
    let adapter = make_duckdb_adapter();
    let result = dispatch_test(
        &adapter,
        "external_write_options",
        &[Value::from("/path/to/c.csv"), dict(&[("null", "\\N")])],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "null '\\N', format csv, header 1");
}

// -- external_read_location tests (ported from dbt-duckdb test_external_utils.py) --

#[test]
fn test_external_read_location_no_partition() {
    let adapter = make_duckdb_adapter();
    let result = dispatch_test(
        &adapter,
        "external_read_location",
        &[
            Value::from("bar"),
            dict(&[("format", "csv"), ("delimiter", "|"), ("header", "0")]),
        ],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "bar");
}

#[test]
fn test_external_read_location_single_partition() {
    let adapter = make_duckdb_adapter();
    let result = dispatch_test(
        &adapter,
        "external_read_location",
        &[
            Value::from("/tmp/a"),
            dict(&[("partition_by", "ds"), ("format", "parquet")]),
        ],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "/tmp/a/*/*.parquet");
}

#[test]
fn test_external_read_location_multi_partition() {
    let adapter = make_duckdb_adapter();
    let result = dispatch_test(
        &adapter,
        "external_read_location",
        &[Value::from("b"), dict(&[("partition_by", "ds,category")])],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "b/*/*/*.parquet");
}

fn make_adapter_with_truthy_nulls(adapter_type: AdapterType) -> Arc<Adapter> {
    let concrete = AdapterImpl::new_mock(
        adapter_type,
        BTreeMap::from([(
            "enable_truthy_nulls_equals_macro".to_string(),
            Value::from(true),
        )]),
        DEFAULT_RESOLVED_QUOTING,
        Arc::new(DefaultTypeOps::new(adapter_type)),
        Arc::new(DefaultStmtSplitter),
    );
    Arc::new(Adapter::new(Arc::new(concrete), None, never_cancels()))
}

/// Create a Typed-phase mock adapter for the given adapter type (no behavior flags).
fn make_mock_adapter(adapter_type: AdapterType) -> Arc<Adapter> {
    let concrete = AdapterImpl::new_mock(
        adapter_type,
        BTreeMap::new(),
        DEFAULT_RESOLVED_QUOTING,
        Arc::new(DefaultTypeOps::new(adapter_type)),
        Arc::new(DefaultStmtSplitter),
    );
    Arc::new(Adapter::new(Arc::new(concrete), None, never_cancels()))
}

#[test]
fn test_render_equals_flag_off_returns_simple_eq() {
    let adapter = make_duckdb_adapter();
    let result = dispatch_test(
        &adapter,
        "render_equals",
        &[Value::from("a"), Value::from("b")],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "(a = b)");
}

#[test]
fn test_render_equals_parse_mode_returns_simple_eq() {
    let adapter = make_duckdb_parse_adapter();
    let result = dispatch_test(
        &adapter,
        "render_equals",
        &[Value::from("a"), Value::from("b")],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "(a = b)");
}

#[test]
fn test_render_equals_flag_on_snowflake_is_not_distinct_from() {
    let adapter = make_adapter_with_truthy_nulls(AdapterType::Snowflake);
    let result = dispatch_test(
        &adapter,
        "render_equals",
        &[Value::from("a"), Value::from("b")],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "(a IS NOT DISTINCT FROM b)");
}

#[test]
fn test_render_equals_flag_on_bigquery_is_not_distinct_from() {
    let adapter = make_adapter_with_truthy_nulls(AdapterType::Bigquery);
    let result = dispatch_test(
        &adapter,
        "render_equals",
        &[Value::from("a"), Value::from("b")],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "(a IS NOT DISTINCT FROM b)");
}

#[test]
fn test_render_equals_flag_on_postgres_is_not_distinct_from() {
    let adapter = make_adapter_with_truthy_nulls(AdapterType::Postgres);
    let result = dispatch_test(
        &adapter,
        "render_equals",
        &[Value::from("a"), Value::from("b")],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "(a IS NOT DISTINCT FROM b)");
}

#[test]
fn test_render_equals_flag_on_redshift_is_not_distinct_from() {
    let adapter = make_adapter_with_truthy_nulls(AdapterType::Redshift);
    let result = dispatch_test(
        &adapter,
        "render_equals",
        &[Value::from("a"), Value::from("b")],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "(a IS NOT DISTINCT FROM b)");
}

#[test]
fn test_render_equals_flag_on_duckdb_is_not_distinct_from() {
    let adapter = make_adapter_with_truthy_nulls(AdapterType::DuckDB);
    let result = dispatch_test(
        &adapter,
        "render_equals",
        &[Value::from("a"), Value::from("b")],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "(a IS NOT DISTINCT FROM b)");
}

#[test]
fn test_render_equals_flag_on_databricks_is_not_distinct_from() {
    let adapter = make_adapter_with_truthy_nulls(AdapterType::Databricks);
    let result = dispatch_test(
        &adapter,
        "render_equals",
        &[Value::from("a"), Value::from("b")],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "(a IS NOT DISTINCT FROM b)");
}

/// `LakeCompute` defines no null-comparison form of its own, so it must answer as
/// DuckDB. It previously fell into the `_` arm and emitted the verbose
/// `case when ... end = 0` form.
#[test]
fn test_render_equals_flag_on_lake_compute_matches_duckdb() {
    let adapter = make_adapter_with_truthy_nulls(AdapterType::LakeCompute);
    let result = dispatch_test(
        &adapter,
        "render_equals",
        &[Value::from("a"), Value::from("b")],
    )
    .unwrap();
    assert_eq!(result.as_str().unwrap(), "(a IS NOT DISTINCT FROM b)");
}

// -- location_exists tests ------------------------------------------------

#[test]
fn test_location_exists_parse_mode_returns_false() {
    let adapter = make_duckdb_parse_adapter();
    let result = dispatch_test(
        &adapter,
        "location_exists",
        &[Value::from("/nonexistent/path")],
    )
    .unwrap();
    assert_eq!(result, Value::from(false));
}

// -- parse-mode arg permissiveness ----------------------------------------
//
// Python `@available.parse_*` decorators short-circuit at parse time without
// inspecting argument types; macros that pass the "wrong" thing should still
// receive the canned value. These tests pin that invariant: at parse time,
// mistyped args do not raise — the Parse arm returns the canned response.

#[test]
fn test_parse_mode_accepts_mistyped_args_drop_relation() {
    let adapter = make_duckdb_parse_adapter();
    // drop_relation expects a BaseRelation; passing an integer would error at
    // dispatch time pre-refactor. Parse mode must now ignore arg types.
    let result = dispatch_test(&adapter, "drop_relation", &[Value::from(42)]).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_parse_mode_accepts_mistyped_args_check_schema_exists() {
    let adapter = make_duckdb_parse_adapter();
    // check_schema_exists expects two strings; passing an int + list should not
    // error at parse time — Parse arm returns the canned `true`.
    let result = dispatch_test(
        &adapter,
        "check_schema_exists",
        &[Value::from(42), Value::from(vec![Value::from("oops")])],
    )
    .unwrap();
    assert_eq!(result, Value::from(true));
}

#[test]
fn test_parse_mode_accepts_mistyped_args_list_relations_without_caching() {
    let adapter = make_duckdb_parse_adapter();
    // list_relations_without_caching expects a BaseRelation; pass a string instead.
    let result = dispatch_test(
        &adapter,
        "list_relations_without_caching",
        &[Value::from("oops")],
    )
    .unwrap();
    // Parse-mode returns an empty list
    assert!(result.try_iter().unwrap().next().is_none());
}

#[dbt_runtime::worker_test]
fn test_get_relation_dispatch_spark_absent_database() {
    // Exercises the full `"get_relation"` arm of `call_method_impl` (arg parsing + per-adapter
    // database resolution + handoff to `get_relation`) for the absent-database (`none`) case,
    // covering every branch of the inline resolution. An absent database is tolerated for every
    // adapter, matching dbt-core: `adapter.get_relation(database=none, ...)` returns a relation
    // rather than raising (verified against dbt-core / dbt-duckdb, which returns `None`).
    let args = [
        Value::from(()), // database: none
        Value::from("my_schema"),
        Value::from("my_table"),
    ];

    // Spark: no catalog -> resolves to the empty default and a relation is still returned.
    let result = dispatch_test(
        &make_mock_adapter(AdapterType::Spark),
        "get_relation",
        &args,
    )
    .unwrap();
    assert!(!result.is_none() && !result.is_undefined());

    // Databricks: substitutes its default catalog -> a relation is returned.
    let result = dispatch_test(
        &make_mock_adapter(AdapterType::Databricks),
        "get_relation",
        &args,
    )
    .unwrap();
    assert!(!result.is_none() && !result.is_undefined());

    // Every other adapter tolerates an absent database (defaults to `""`) rather than erroring,
    // matching dbt-core's duck-typed `get_relation`.
    let result = dispatch_test(
        &make_mock_adapter(AdapterType::DuckDB),
        "get_relation",
        &args,
    )
    .unwrap();
    assert!(!result.is_none() && !result.is_undefined());
}

// -- introspective taint wiring --------------------------------------------
//
// `Adapter::call_method` (the `Object` trait method, as opposed to
// `call_method_impl` which `dispatch_test` above calls directly and which
// bypasses this wrapping) taints the return value of every method in
// `INTROSPECTIVE_METHODS` when running in `Parse` mode. This is what lets
// `JinjaRenderMode::Symbolic` hole-punch introspective results instead of
// silently rendering the Parse-mode stub as if it were real.

fn call_method_test(
    adapter: &Arc<Adapter>,
    name: &str,
    args: &[Value],
) -> Result<Value, minijinja::Error> {
    let env = minijinja::Environment::new();
    let state = State::new_for_env(&env);
    let listener: Rc<dyn RenderingEventListener> = Rc::new(TaintGateListener);
    adapter.call_method(&state, name, args, &[listener])
}

#[test]
fn test_parse_mode_execute_result_is_tainted() {
    let adapter = make_duckdb_parse_adapter();
    let result = call_method_test(&adapter, "execute", &[Value::from("select 1")]).unwrap();
    assert!(result.is_introspective_stub());
}

#[test]
fn test_parse_mode_execute_result_is_not_tainted_without_an_opted_in_listener() {
    // Regression test: taint-wrapping is gated on a listener actually
    // wanting introspective holes (only `JinjaRenderMode::Symbolic`'s does).
    // Wrapping unconditionally changes the value's `ValueRepr` from
    // whatever primitive it really is (e.g. `None`) to `Object` for *every*
    // render mode, which silently broke plain `{% if not
    // adapter.get_relation(...) %}`/`is none`-style checks that never touch
    // taint at all -- `Value::is_none()` can't see through an `Object`
    // wrapper no matter what `IntrospectiveValue` overrides.
    let adapter = make_duckdb_parse_adapter();
    let env = minijinja::Environment::new();
    let state = State::new_for_env(&env);
    let result = adapter
        .call_method(&state, "execute", &[Value::from("select 1")], &[])
        .unwrap();
    assert!(!result.is_introspective_stub());
}

#[test]
fn test_parse_mode_get_columns_in_relation_result_is_tainted() {
    let adapter = make_duckdb_parse_adapter();
    let relation = dispatch_test(
        &adapter,
        "get_relation",
        &[
            Value::from("db"),
            Value::from("schema"),
            Value::from("my_table"),
        ],
    )
    .unwrap();
    let result = call_method_test(&adapter, "get_columns_in_relation", &[relation]).unwrap();
    assert!(result.is_introspective_stub());
}

#[test]
fn test_parse_mode_get_columns_in_relation_accepts_string() {
    let adapter = make_duckdb_parse_adapter();
    let result = call_method_test(
        &adapter,
        "get_columns_in_relation",
        &[Value::from("relation_name")],
    )
    .unwrap();
    assert!(result.is_introspective_stub());
}

#[test]
fn test_runtime_get_columns_in_relation_rejects_string() {
    let adapter = make_duckdb_adapter();
    let err = dispatch_test(
        &adapter,
        "get_columns_in_relation",
        &[Value::from("relation_name")],
    )
    .unwrap_err();
    assert_eq!(err.detail(), Some("relation must be an object"));
}

#[test]
fn test_parse_mode_get_relation_result_is_tainted() {
    let adapter = make_duckdb_parse_adapter();
    let result = call_method_test(
        &adapter,
        "get_relation",
        &[
            Value::from("db"),
            Value::from("schema"),
            Value::from("my_table"),
        ],
    )
    .unwrap();
    assert!(result.is_introspective_stub());
}

#[test]
fn test_parse_mode_non_introspective_method_is_not_tainted() {
    let adapter = make_duckdb_parse_adapter();
    let result = call_method_test(
        &adapter,
        "check_schema_exists",
        &[Value::from("db"), Value::from("schema")],
    )
    .unwrap();
    assert!(!result.is_introspective_stub());
}

#[dbt_runtime::worker_test]
fn test_typed_mode_execute_result_is_not_tainted() {
    let adapter = make_duckdb_adapter();
    let result = call_method_test(&adapter, "execute", &[Value::from("select 1")]).unwrap();
    assert!(!result.is_introspective_stub());
}

/// `adapter.type()` must report the adapter the *node* runs on. Model bodies
/// branch on it, so a node that selected a `lakecompute` adapter seeing the
/// target's default would take the wrong branch.
#[test]
fn adapter_type_follows_the_nodes_selected_dialect() {
    let adapter = make_duckdb_adapter();

    // No selection: the adapter's own type.
    let default = dispatch_test(&adapter, "type", &[]).unwrap();
    assert_eq!(default.as_str().unwrap(), "duckdb");

    // Selection: the node's dialect wins.
    let selected = dispatch_test_with_dialect(&adapter, "lakecompute", "type", &[]).unwrap();
    assert_eq!(selected.as_str().unwrap(), "lakecompute");
}

/// An unparseable or absent dialect must fall back to the adapter's own type
/// rather than erroring or silently reporting something else.
#[test]
fn adapter_type_falls_back_on_an_unknown_dialect() {
    let adapter = make_duckdb_adapter();

    let result = dispatch_test_with_dialect(&adapter, "not_an_adapter", "type", &[]).unwrap();
    assert_eq!(result.as_str().unwrap(), "duckdb");
}

#[test]
fn test_statement_macro_does_not_crash_in_symbolic_lint_mode() {
    // Regression test: `JinjaRenderMode::Symbolic` forces `execute = true` so
    // macros guarded by `{% if not execute %}`/`{% if execute %}` take their
    // real (introspective) branch instead of a hardcoded default -- relying
    // on `Adapter::call_method`'s Parse-mode taint-wrapping (see
    // `test_parse_mode_execute_result_is_tainted`) plus tuple-unpack taint
    // propagation (`IntrospectiveValue::unpack`) to keep that branch safe.
    // dbt's own `statement()` macro is the canonical case this must handle:
    // `{% set res, table = adapter.execute(...) %}` inside a real
    // `{% if execute %}` block, invoked via `{% call statement() %}`.
    // Before the taint/unpack fix, this failed with "cannot unpack: sequence
    // of wrong length (expected 2, got 1)".
    let adapter = make_duckdb_parse_adapter();
    let env = minijinja::Environment::new();
    let listener: Rc<dyn RenderingEventListener> = Rc::new(TaintGateListener);
    let source = "\
{%- macro statement(name=None, fetch_result=False, auto_begin=True, language='sql') -%}
  {%- if execute -%}
    {%- set compiled_code = caller() -%}
    {%- if language == 'sql' -%}
      {%- set res, table = adapter.execute(compiled_code, auto_begin=auto_begin, fetch=fetch_result) -%}
    {%- endif -%}
  {%- endif -%}
{%- endmacro -%}
{%- call statement('run_query_statement', fetch_result=true) -%}
select 1
{%- endcall -%}";
    let result = env.render_str(
        source,
        minijinja::context! {
            execute => true,
            adapter => Value::from_object(adapter.as_ref().clone()),
        },
        &[listener],
    );

    assert!(result.is_ok(), "{:?}", result.err());
}

#[test]
fn test_parse_mode_call_with_tainted_argument_short_circuits_instead_of_erroring() {
    // Regression test: `quote()` isn't itself introspective (it's a pure
    // string transform, not in `INTROSPECTIVE_METHODS`), but it's commonly
    // called with an identifier drawn from an already-tainted result (e.g. a
    // column name from `get_columns_in_relation()`). Before this fix,
    // `call_method_impl`'s `ArgsIter::next_arg::<&str>()` hard-failed with
    // "argument 'identifier' to quote() has incompatible type
    // IntrospectiveValue; value is not a string" because the tainted arg is
    // an `Object`, not a real `&str`, and `Adapter::call_method` called the
    // real impl regardless of argument taint.
    let adapter = make_duckdb_parse_adapter();
    let tainted_relation = call_method_test(
        &adapter,
        "get_relation",
        &[
            Value::from("db"),
            Value::from("schema"),
            Value::from("my_table"),
        ],
    )
    .unwrap();
    assert!(tainted_relation.is_introspective_stub());

    let result = call_method_test(&adapter, "quote", &[tainted_relation]).unwrap();
    assert!(result.is_introspective_stub());
}

#[test]
fn test_check_schema_exists_tolerates_none_database() {
    let adapter = make_duckdb_adapter();
    let err = dispatch_test(
        &adapter,
        "check_schema_exists",
        &[Value::from(()), Value::from("main")],
    )
    .unwrap_err();
    let message = err.to_string();
    assert!(
        !message.contains("incompatible type"),
        "database=none should not fail argument type conversion, got: {message}"
    );
    assert!(
        message.contains("template not found") || message.contains("check_schema_exists"),
        "expected a macro-lookup failure past arg parsing, got: {message}"
    );
}
