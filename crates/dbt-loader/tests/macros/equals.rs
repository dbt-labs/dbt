use std::collections::HashMap;

use dbt_adapter_core::AdapterType;
use dbt_common::behavior_flags::{Behavior, BehaviorFlag};
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

const FLAG_NAME: &str = "enable_truthy_nulls_equals_macro";

const DISPATCHING_EQUALS: &str = r#"
{% macro equals(expr1, expr2) %}
    {{ return(adapter.dispatch('equals', 'dbt')(expr1, expr2)) }}
{%- endmacro %}
"#;

const DEFAULT_EQUALS_SQL: &str =
    include_str!("../../src/dbt_macro_assets/dbt-adapters/macros/utils/equals.sql");

/// Build a harness whose mock adapter returns a fixed string from `render_equals`.
fn build_harness(
    adapter_type: AdapterType,
    render_equals_result: &'static str,
) -> MacroTestHarness {
    let harness = MacroTestHarness::for_adapter(adapter_type)
        .with_macro("dbt", "equals", DISPATCHING_EQUALS)
        .with_macro_at_path(
            "dbt",
            "default__equals",
            DEFAULT_EQUALS_SQL,
            "dbt_macro_assets/dbt-adapters/macros/utils/equals.sql",
        )
        .build()
        .expect("harness should build");

    harness.mock().on("render_equals", move |_args| {
        Ok(Value::from(render_equals_result))
    });

    harness
}

fn render_equals(harness: &MacroTestHarness) -> String {
    harness
        .render("{{ equals('a', 'b') }}", HashMap::<String, Value>::new())
        .expect("render should succeed")
}

#[test]
fn dispatch_passes_through_to_render_equals() {
    let harness = build_harness(AdapterType::Snowflake, "(a IS NOT DISTINCT FROM b)");
    let rendered = render_equals(&harness);
    let normalized = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(
        normalized, "(a IS NOT DISTINCT FROM b)",
        "expected result from adapter.render_equals to be returned, got: {rendered:?}"
    );
}

#[test]
fn dispatch_passes_through_simple_equality() {
    let harness = build_harness(AdapterType::Postgres, "(a = b)");
    let rendered = render_equals(&harness);
    let normalized = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(
        normalized, "(a = b)",
        "expected result from adapter.render_equals to be returned, got: {rendered:?}"
    );
}

mod bigquery_merge {
    use super::*;

    const BQ_MERGE_SQL: &str = include_str!(
        "../../src/dbt_macro_assets/dbt-bigquery/macros/materializations/incremental_strategy/merge.sql"
    );

    fn build_merge_harness(truthy_nulls: bool) -> MacroTestHarness {
        let harness = MacroTestHarness::for_adapter(AdapterType::Bigquery)
            .with_macro_at_path(
                "dbt_bigquery",
                "bigquery__get_merge_unique_key_match",
                BQ_MERGE_SQL,
                "dbt_macro_assets/dbt-bigquery/macros/materializations/incremental_strategy/merge.sql",
            )
            .build()
            .expect("harness should build");

        let overrides = std::collections::BTreeMap::from([(FLAG_NAME.to_string(), truthy_nulls)]);
        let behavior = Behavior::new(
            vec![BehaviorFlag::new(FLAG_NAME, false, None, None, None)],
            &overrides,
        );
        harness
            .mock()
            .set_attr("behavior", Value::from_object(behavior));

        harness
    }

    fn render(harness: &MacroTestHarness) -> String {
        harness
            .render(
                "{{ bigquery__get_merge_unique_key_match('DBT_INTERNAL_SOURCE.id', 'DBT_INTERNAL_DEST.id') }}",
                HashMap::<String, Value>::new(),
            )
            .expect("render should succeed")
    }

    #[test]
    fn truthy_nulls_uses_legacy_null_safe_predicate() {
        let harness = build_merge_harness(true);
        let rendered = render(&harness);
        assert!(
            rendered.contains("DBT_INTERNAL_SOURCE.id is null and DBT_INTERNAL_DEST.id is null")
                && rendered.contains("DBT_INTERNAL_SOURCE.id = DBT_INTERNAL_DEST.id"),
            "expected legacy `(... is null and ... is null) or (... = ...)` predicate \
             so BigQuery's partition-pruning analyzer still recognises the merge ON clause; got:\n{rendered}"
        );
        assert!(
            !rendered.to_uppercase().contains("IS NOT DISTINCT FROM"),
            "BigQuery merge must NOT use IS NOT DISTINCT FROM (breaks require_partition_filter); got:\n{rendered}"
        );
    }

    #[test]
    fn truthy_nulls_disabled_uses_simple_equality() {
        let harness = build_merge_harness(false);
        let rendered = render(&harness);
        assert!(
            rendered.contains("DBT_INTERNAL_SOURCE.id = DBT_INTERNAL_DEST.id"),
            "expected simple `=` predicate, got:\n{rendered}"
        );
        assert!(
            !rendered.contains("is null and"),
            "expected no null-safe expansion when flag is off, got:\n{rendered}"
        );
    }
}
