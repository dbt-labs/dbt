use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::catalog_relation::CatalogRelation;
use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, assert_executed_contains};

fn build_harness() -> MacroTestHarness {
    let mut harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    harness
        .env_mut()
        .env
        .add_global("execute", Value::from(true));

    harness
}

fn build_spark_harness() -> MacroTestHarness {
    MacroTestHarness::for_adapter(AdapterType::Spark)
        .load_all_macros()
        .build()
        .expect("Spark harness should build")
}

#[test]
fn create_schema_accepts_string_argument() {
    let harness = build_harness();
    let ctx = BTreeMap::from([("relation".to_string(), Value::from("func"))]);

    harness
        .render("{{ spark__create_schema(relation) }}", ctx)
        .expect("create_schema should accept a string argument");

    assert_executed_contains(harness.mock(), "create schema if not exists func");
}

#[test]
fn drop_schema_accepts_string_argument() {
    let harness = build_harness();
    let ctx = BTreeMap::from([("relation".to_string(), Value::from("func"))]);

    harness
        .render("{{ spark__drop_schema(relation) }}", ctx)
        .expect("drop_schema should accept a string argument");

    assert_executed_contains(harness.mock(), "drop schema if exists func cascade");
}

fn render_target_platform(target: BTreeMap<&str, &str>) -> String {
    let harness = build_harness();
    let ctx = BTreeMap::from([("target".to_string(), Value::from_serialize(target))]);

    harness
        .render("{{ spark__target_platform() }}", ctx)
        .expect("target_platform should render")
}

#[test]
fn target_platform_is_fabric_for_lakehouse_target() {
    let target = BTreeMap::from([("method", "livy"), ("platform_hint", "fabric_lakehouse")]);

    assert_eq!(render_target_platform(target), "fabric_lakehouse");
}

#[test]
fn target_platform_is_vanilla_for_fabric_hint_on_other_method() {
    let target = BTreeMap::from([
        ("method", "spark-connect"),
        ("platform_hint", "fabric_lakehouse"),
    ]);

    assert_eq!(render_target_platform(target), "vanilla");
}

#[test]
fn target_platform_is_vanilla_otherwise() {
    let target = BTreeMap::from([("host", "spark.example.com")]);

    assert_eq!(render_target_platform(target), "vanilla");
}

#[test]
fn list_schemas_sql_uses_selected_catalog() {
    let harness = build_spark_harness();
    let rendered = harness
        .render(
            "{{ spark__list_schemas_sql('routed_catalog') }}",
            BTreeMap::<String, Value>::new(),
        )
        .expect("render should succeed");

    assert_eq!(rendered.trim(), "show schemas in routed_catalog");
}

#[test]
fn list_schemas_sql_without_catalog_shows_databases() {
    let harness = build_spark_harness();
    for database in ["''", "'``'", "none"] {
        let rendered = harness
            .render(
                &format!("{{{{ spark__list_schemas_sql({database}) }}}}"),
                BTreeMap::<String, Value>::new(),
            )
            .expect("render should succeed");

        assert_eq!(rendered.trim(), "show databases", "database = {database}");
    }
}

#[test]
fn relation_listing_sql_keeps_catalog_and_schema() {
    let harness = build_spark_harness();
    let relation = harness.relation(
        "routed_catalog",
        "analytics",
        "events",
        Some(RelationType::Table),
    );
    let context = BTreeMap::from([(
        "schema_relation".to_string(),
        RelationObject::new(Arc::clone(&relation)).into_value(),
    )]);

    let rendered = harness
        .render(
            "{{ spark__list_relations_without_caching_sql(schema_relation) }}",
            context.clone(),
        )
        .expect("render should succeed");
    assert_eq!(
        rendered.trim(),
        "show table extended in `routed_catalog`.`analytics` like '*'"
    );

    let rendered = harness
        .render(
            "{{ spark__list_relations_show_tables_without_caching_sql(schema_relation) }}",
            context,
        )
        .expect("render should succeed");
    assert_eq!(
        rendered.trim(),
        "show tables in `routed_catalog`.`analytics` like '*'"
    );
}

#[test]
fn generate_database_name_uses_preconfigured_catalog_name() {
    let harness = build_spark_harness();
    let catalog_relation = Value::from_object(CatalogRelation {
        catalog_name: Some("logical_rest".to_string()),
        catalog_database: Some("spark_catalog".to_string()),
        file_format: Some("iceberg".to_string()),
        ..CatalogRelation::default_catalog_relation_spark()
    });
    harness.mock().on("build_catalog_relation", move |_| {
        Ok(catalog_relation.clone())
    });

    let node = BTreeMap::from([(
        "config",
        Value::from_serialize(BTreeMap::from([("catalog_name", "logical_rest")])),
    )]);
    let context = BTreeMap::from([("node", Value::from_serialize(node))]);

    let rendered = harness
        .render("{{ spark__generate_database_name(none, node) }}", context)
        .expect("render should succeed");

    assert_eq!(rendered.trim(), "spark_catalog");
}
