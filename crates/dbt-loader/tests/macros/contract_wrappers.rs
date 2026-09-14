//! Tests for the macros that splice a node body into a wrapping query:
//! `get_empty_subquery_sql` (`dbt-adapters/macros/adapters/columns.sql`) and
//! `get_select_subquery` (`dbt-adapters/macros/relations/table/create.sql`).
//!
//! Both are reached when `contract.enforced` is set, and a terminal `;` in the
//! body would otherwise land inside their parentheses. Normalization lives in
//! the dispatcher, so these render through it rather than calling a `*__`
//! implementation directly -- that is what proves the adapter overrides
//! (`bigquery__`, `fabric__`, `exasol__`) are covered too.

use std::collections::BTreeMap;

use dbt_adapter_core::AdapterType;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

/// Adapters shipping their own `get_empty_subquery_sql` / `get_select_subquery`,
/// plus representatives that fall through to the `default__` implementations.
const ADAPTERS: &[AdapterType] = &[
    // default__
    AdapterType::Snowflake,
    AdapterType::Databricks,
    AdapterType::DuckDB,
    // overrides
    AdapterType::Bigquery,
    AdapterType::Fabric,
    AdapterType::Exasol,
];

const EMPTY_SUBQUERY: &str = "{{ get_empty_subquery_sql(select_sql) }}";
const SELECT_SUBQUERY: &str = "{{ get_select_subquery(select_sql) }}";

fn build_harness(adapter_type: AdapterType) -> MacroTestHarness {
    let harness = MacroTestHarness::for_adapter(adapter_type)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    // `strip_trailing_statement_terminator` comes from the harness, wired to the
    // real dialect-aware normalization.
    let mock = harness.mock().clone();
    // `bigquery__get_column_names` nests the model's columns, then renders each
    // one through the adapter.
    mock.on("nest_column_data_types", |args| {
        Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
    });
    mock.on("get_struct_select_expression", |args| {
        Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
    });

    harness
}

fn ctx(sql: &str) -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("select_sql".to_string(), Value::from(sql)),
        (
            "model".to_string(),
            Value::from_serialize(BTreeMap::from([(
                "columns",
                BTreeMap::from([("id", BTreeMap::from([("name", "id")]))]),
            )])),
        ),
    ])
}

fn render(harness: &MacroTestHarness, template: &str, sql: &str) -> String {
    harness
        .render(template, ctx(sql))
        .expect("render should succeed")
}

/// Every shape of trailing terminator must be gone from the wrapper, including
/// the ones a plain suffix trim would miss.
#[test]
fn wrappers_drop_trailing_terminators() {
    let bodies = [
        "select cast(1 as int) as id;",
        "select cast(1 as int) as id;;",
        "select cast(1 as int) as id; -- trailing",
        "select cast(1 as int) as id;\n-- trailing\n",
        "select cast(1 as int) as id;   \n",
    ];

    for adapter_type in ADAPTERS {
        let harness = build_harness(*adapter_type);
        for template in [EMPTY_SUBQUERY, SELECT_SUBQUERY] {
            for body in bodies {
                let rendered = render(&harness, template, body);

                assert!(
                    !rendered.contains(';'),
                    "{adapter_type:?} {template} left a semicolon inside the wrapper \
                     for {body:?}: {rendered}"
                );
                assert!(
                    rendered.contains("select cast(1 as int) as id"),
                    "{adapter_type:?} {template} dropped the body for {body:?}: {rendered}"
                );
            }
        }
    }
}

/// A body with a terminator must wrap exactly like the same body without one,
/// so the fix cannot be observed as a formatting change.
#[test]
fn wrappers_normalize_to_the_terminator_free_rendering() {
    for adapter_type in ADAPTERS {
        let harness = build_harness(*adapter_type);
        for template in [EMPTY_SUBQUERY, SELECT_SUBQUERY] {
            assert_eq!(
                render(&harness, template, "select 1 as id;"),
                render(&harness, template, "select 1 as id"),
                "{adapter_type:?} {template} rendered a terminated body differently"
            );
        }
    }
}

/// Without a terminator there is nothing to drop, so the body must be spliced in
/// verbatim -- including its surrounding whitespace.
#[test]
fn wrappers_preserve_bodies_without_a_terminator() {
    for adapter_type in ADAPTERS {
        let harness = build_harness(*adapter_type);
        for template in [EMPTY_SUBQUERY, SELECT_SUBQUERY] {
            let rendered = render(&harness, template, "\n  select 1 as id\n");

            assert!(
                rendered.contains("\n  select 1 as id\n"),
                "{adapter_type:?} {template} reformatted the body: {rendered}"
            );
        }
    }
}

/// A `;` inside a string literal is not a terminator and must survive.
#[test]
fn wrappers_keep_semicolons_inside_the_body() {
    for adapter_type in ADAPTERS {
        let harness = build_harness(*adapter_type);
        for template in [EMPTY_SUBQUERY, SELECT_SUBQUERY] {
            let rendered = render(&harness, template, "select 'a;b' as id");

            assert!(
                rendered.contains("select 'a;b' as id"),
                "{adapter_type:?} {template} mangled a quoted semicolon: {rendered}"
            );
        }
    }
}

/// A multi-statement body is not a stray terminator, so it is left alone for the
/// warehouse to reject.
#[test]
fn wrappers_leave_multi_statement_bodies_alone() {
    for adapter_type in ADAPTERS {
        let harness = build_harness(*adapter_type);
        for template in [EMPTY_SUBQUERY, SELECT_SUBQUERY] {
            let rendered = render(&harness, template, "select 1 as id; select 2 as id;");

            assert!(
                rendered.contains("select 1 as id; select 2 as id;"),
                "{adapter_type:?} {template} rewrote a multi-statement body: {rendered}"
            );
        }
    }
}

/// Normalizing at the dispatcher must not shadow the adapter's own wrapper.
#[test]
fn dispatch_still_reaches_adapter_overrides() {
    // (adapter, marker unique to that adapter's `get_empty_subquery_sql`)
    let cases = [
        (AdapterType::Bigquery, "current_timestamp()"),
        (AdapterType::Exasol, ") dbt_sbq"),
        (AdapterType::Snowflake, ") as __dbt_sbq"),
    ];

    for (adapter_type, marker) in cases {
        let harness = build_harness(adapter_type);
        let rendered = render(&harness, EMPTY_SUBQUERY, "select 1 as id;");

        assert!(
            rendered.contains(marker),
            "{adapter_type:?} did not reach its own wrapper (expected {marker:?}): {rendered}"
        );
        assert!(
            !rendered.contains(';'),
            "{adapter_type:?} left a semicolon inside the wrapper: {rendered}"
        );
    }
}

/// Normalizing before dispatch must not disturb the header the implementation
/// emits ahead of the wrapper.
#[test]
fn empty_subquery_sql_keeps_the_sql_header() {
    // `fabric__get_empty_subquery_sql` ignores `select_sql_header` outright, so
    // it has no header to preserve.
    let adapters = ADAPTERS.iter().filter(|a| **a != AdapterType::Fabric);

    for adapter_type in adapters {
        let harness = build_harness(*adapter_type);
        let rendered = harness
            .render(
                "{{ get_empty_subquery_sql(select_sql, 'set x = 1') }}",
                ctx("select 1 as id;"),
            )
            .expect("render should succeed");

        assert!(
            rendered.contains("set x = 1"),
            "{adapter_type:?} dropped the sql header: {rendered}"
        );
        assert!(
            rendered.contains("select 1 as id"),
            "{adapter_type:?} dropped the body: {rendered}"
        );
    }
}
