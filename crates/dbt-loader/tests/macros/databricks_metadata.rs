use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

fn build_harness() -> MacroTestHarness {
    let sql =
        include_str!("../../src/dbt_macro_assets/dbt-databricks/macros/adapters/metadata.sql");
    MacroTestHarness::for_adapter(AdapterType::Databricks)
        .with_macro_at_path(
            "dbt_databricks",
            "describe_table_extended_as_json_sql",
            sql,
            "dbt_macro_assets/dbt-databricks/macros/adapters/metadata.sql",
        )
        .build()
        .expect("harness should build")
}

#[test]
fn test_describe_table_extended_as_json_sql() {
    let harness = build_harness();
    let relation = harness.relation(
        "some_database",
        "some_schema",
        "some_table",
        Some(RelationType::Table),
    );

    let rendered = harness
        .render(
            "{{ describe_table_extended_as_json_sql(relation) }}",
            [(
                "relation".to_string(),
                RelationObject::new(relation).into_value(),
            )]
            .into_iter()
            .collect::<std::collections::BTreeMap<String, Value>>(),
        )
        .expect("render should succeed");

    assert_eq!(
        rendered.trim(),
        "DESCRIBE TABLE EXTENDED `some_database`.`some_schema`.`some_table` AS JSON"
    );
}
