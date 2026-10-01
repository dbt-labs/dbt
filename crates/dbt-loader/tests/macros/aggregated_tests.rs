use std::collections::HashMap;

use dbt_adapter_core::AdapterType;
use dbt_jinja_ctx::DbtNamespace;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

/// Render the shared batched-`unique` macro for `adapter_type`, lowercased.
///
/// The per-adapter variation lives entirely in the `aggregated_unique_field`
/// dispatch, which `for_adapter` drives, so rendering the default body directly
/// is enough.
fn render_batched_unique(adapter_type: AdapterType) -> String {
    let harness = MacroTestHarness::for_adapter(adapter_type)
        .load_all_macros()
        .with_global(
            "aggregated_test_skip_column_names",
            Value::from(Vec::<String>::new()),
        )
        // The harness does not wire package namespaces, and the macro reaches
        // dispatch through `dbt.`-qualified calls. `DbtNamespace` is the same
        // object a real render gets, so lookup stays genuine.
        .with_global("dbt", Value::from_object(DbtNamespace::new("dbt")))
        .build()
        .expect("harness should build");

    harness
        .render(
            "{{ default__test_aggregated_unique('my_model', ['tax_rate']) }}",
            HashMap::<String, Value>::new(),
        )
        .expect("batched unique macro should render")
        .to_lowercase()
}

#[test]
fn snowflake_batched_unique_uses_plain_cast() {
    let rendered = render_batched_unique(AdapterType::Snowflake);
    assert!(
        rendered.contains("cast(tax_rate as"),
        "expected a plain cast of the unique_field column, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("try_cast") && !rendered.contains("safe_cast"),
        "Snowflake try_cast accepts only a string source expression, so the \
         batched unique projection must not use safe_cast; got:\n{rendered}"
    );
}

#[test]
fn bigquery_batched_unique_uses_safe_cast() {
    let rendered = render_batched_unique(AdapterType::Bigquery);
    assert!(
        rendered.contains("safe_cast(tax_rate as"),
        "BigQuery SAFE_CAST yields NULL instead of erroring on an unconvertible \
         value, so the batched unique projection must keep it; got:\n{rendered}"
    );
}

#[test]
fn duckdb_batched_unique_uses_plain_cast() {
    let rendered = render_batched_unique(AdapterType::DuckDB);
    assert!(
        rendered.contains("cast(tax_rate as"),
        "expected default__safe_cast to render a plain cast, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("try_cast"),
        "expected no try_cast on the default path, got:\n{rendered}"
    );
}

/// GizmoSQL inherits DuckDB's macro chain, so it must render exactly as DuckDB does.
#[test]
fn gizmosql_batched_unique_matches_duckdb() {
    let rendered = render_batched_unique(AdapterType::GizmoSQL);
    assert_eq!(
        rendered.split_whitespace().collect::<Vec<_>>(),
        render_batched_unique(AdapterType::DuckDB)
            .split_whitespace()
            .collect::<Vec<_>>(),
        "GizmoSQL should resolve the batched unique test through DuckDB's chain, got:\n{rendered}"
    );
}
