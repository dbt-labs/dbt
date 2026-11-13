use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::macro_test_harness::{MacroTestHarness, assert_executed_contains, default_mock_config};

#[test]
fn databricks_metric_view_rejects_an_empty_yaml_definition() {
    let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    harness.mock().on("get_relation", |_| Ok(().into()));

    let ctx = harness
        .materialization_context("order_metrics", "")
        .relation_type(RelationType::MetricView)
        .build();

    let error = harness
        .render("{{ materialization_metric_view_databricks() }}", ctx)
        .expect_err("an empty metric-view definition should fail");

    assert!(
        error
            .to_string()
            .contains("Cannot compile metric view order_metrics with no YAML definition"),
        "unexpected error: {error}"
    );
}

fn assert_incompatible_relation_uses_safe_replacement(existing_type: RelationType) {
    let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    harness.mock().on("clean_sql", |args| {
        Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
    });
    harness
        .mock()
        .on("resolve_file_format", |_| Ok(Value::from("delta")));
    harness
        .mock()
        .on("rename_relation", |_| Ok(Value::UNDEFINED));
    harness.mock().on("drop_relation", |_| Ok(Value::UNDEFINED));

    let existing = harness.relation(
        "TEST_DB",
        "TEST_SCHEMA",
        "order_metrics",
        Some(existing_type),
    );
    harness.mock().on("get_relation", move |_| {
        Ok(RelationObject::new(Arc::clone(&existing)).into_value())
    });

    let ctx = harness
        .materialization_context(
            "order_metrics",
            "version: 1.1\nsource: `main`.`default`.`source_orders`\nmeasures:\n  - name: total_orders\n    expr: count(1)",
        )
        .relation_type(RelationType::MetricView)
        .config({
            let config = default_mock_config();
            config.set_attr("materialized", Value::from("metric_view"));
            Value::from_dyn_object(config)
        })
        .build();

    harness
        .render("{{ materialization_metric_view_databricks() }}", ctx)
        .expect("incompatible relation should be replaced safely");

    harness
        .mock()
        .observed_calls()
        .assert_called("rename_relation");
    assert_executed_contains(harness.mock(), "with metrics");
    // The `databricks__yaml_quote_backtick_values` helper quotes the backticked `source:` value.
    assert_executed_contains(
        harness.mock(),
        "source: \"`main`.`default`.`source_orders`\"",
    );
}

#[test]
fn databricks_metric_view_replaces_an_existing_table() {
    assert_incompatible_relation_uses_safe_replacement(RelationType::Table);
}

#[test]
fn databricks_metric_view_replaces_an_existing_view() {
    assert_incompatible_relation_uses_safe_replacement(RelationType::View);
}

#[test]
fn databricks_yaml_quote_backtick_values_only_quotes_bare_source_identifiers() {
    let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    let quote = |yaml_body: &str| {
        harness
            .render(
                "{{ databricks__yaml_quote_backtick_values(yaml_body) }}",
                BTreeMap::from([("yaml_body".to_string(), Value::from(yaml_body))]),
            )
            .expect("helper should render")
    };

    let yaml = concat!(
        "source: `catalog`.`schema`.`orders`\n",
        "joins:\n",
        "  - name: customers\n",
        "    source :   `catalog`.`schema`.`customers` # customer source\n",
        "    on: `orders`.`customer_id` = `customers`.`id`\n",
        "data_source: `catalog`.`schema`.`ignored`\n",
    );
    let expected = concat!(
        "source: \"`catalog`.`schema`.`orders`\"\n",
        "joins:\n",
        "  - name: customers\n",
        "    source :   \"`catalog`.`schema`.`customers`\" # customer source\n",
        "    on: `orders`.`customer_id` = `customers`.`id`\n",
        "data_source: `catalog`.`schema`.`ignored`\n",
    );
    assert_eq!(quote(yaml), expected);

    // Already-quoted values and bodies without backticks are left alone.
    assert_eq!(
        quote("source: \"`catalog`.`schema`.`orders`\"\n"),
        "source: \"`catalog`.`schema`.`orders`\"\n"
    );
    assert_eq!(quote("source: orders\n"), "source: orders\n");
}
