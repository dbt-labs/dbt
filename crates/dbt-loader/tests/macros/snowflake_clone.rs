//! Tests for `dbt-snowflake/macros/materializations/clone.sql`.
//!
//! `dbt clone` copies the source relation's kind into the DDL: cloning an Iceberg
//! table requires `create or replace iceberg table ... clone ...` (Iceberg tables
//! cannot be transient, and the plain form fails on Iceberg sources), while every
//! other source keeps the transient default. The source's Iceberg-ness is read off
//! the relation returned by `load_cached_relation` at clone time.

use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::{Relation, RelationObject};
use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::relations::SNOWFLAKE_RESOLVED_QUOTING;
use dbt_schemas::schemas::relations::base::TableFormat;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

fn build_harness() -> MacroTestHarness {
    MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("clone harness should build")
}

/// A Snowflake relation with the given table format, shaped the way
/// `adapter.get_relation` returns cached relations to Jinja.
fn source_relation(table_format: TableFormat) -> Value {
    let relation = Relation::new(
        AdapterType::Snowflake,
        "TEST_DB".to_string(),
        "OTHER_SCHEMA".to_string(),
        "MY_TABLE".to_string(),
    )
    .with_relation_type(Some(RelationType::Table))
    .with_quoting(SNOWFLAKE_RESOLVED_QUOTING)
    .with_table_format(table_format);
    RelationObject::new(Arc::from(relation)).into_value()
}

fn render_clone(harness: &MacroTestHarness, defer_relation: Value) -> String {
    let target = harness.relation(
        "TEST_DB",
        "TEST_SCHEMA",
        "MY_TABLE",
        Some(RelationType::Table),
    );
    let ctx = BTreeMap::from([
        (
            "this_relation".to_string(),
            RelationObject::new(target).into_value(),
        ),
        ("defer_relation".to_string(), defer_relation),
    ]);
    let rendered = harness
        .render(
            "{{ snowflake__create_or_replace_clone(this_relation, defer_relation) }}",
            ctx,
        )
        .expect("clone macro should render");
    // Collapse Jinja's intra-clause whitespace so assertions can match phrases
    // that span template lines.
    rendered.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn iceberg_source_clones_as_iceberg_table() {
    let harness = build_harness();
    let source = source_relation(TableFormat::Iceberg);
    let mock_source = source.clone();
    harness
        .mock()
        .on("get_relation", move |_| Ok(mock_source.clone()));

    let rendered = render_clone(&harness, source);
    let lower = rendered.to_lowercase();
    assert!(
        lower.contains("create or replace iceberg table"),
        "cloning an Iceberg source must emit `create or replace iceberg table`, got:\n{rendered}"
    );
    assert!(
        !lower.contains("transient"),
        "Iceberg tables cannot be transient, got:\n{rendered}"
    );
}

#[test]
fn regular_source_keeps_transient_default() {
    let harness = build_harness();
    let source = source_relation(TableFormat::Default);
    let mock_source = source.clone();
    harness
        .mock()
        .on("get_relation", move |_| Ok(mock_source.clone()));

    let rendered = render_clone(&harness, source);
    let lower = rendered.to_lowercase();
    assert!(
        lower.contains("create or replace transient table"),
        "a regular source keeps the transient default, got:\n{rendered}"
    );
    assert!(
        !lower.contains("iceberg"),
        "a non-Iceberg source must not render the iceberg keyword, got:\n{rendered}"
    );
}

#[test]
fn uncached_source_defaults_to_non_iceberg_clone() {
    let harness = build_harness();
    // `load_cached_relation` finds nothing: get_relation resolves to none.
    harness.mock().on("get_relation", |_| Ok(Value::from(())));

    let source = source_relation(TableFormat::Default);
    let rendered = render_clone(&harness, source);
    let lower = rendered.to_lowercase();
    assert!(
        lower.contains("create or replace transient table"),
        "an unresolvable source must keep the transient default, got:\n{rendered}"
    );
    assert!(
        !lower.contains("iceberg"),
        "an unresolvable source must not render the iceberg keyword, got:\n{rendered}"
    );
}
