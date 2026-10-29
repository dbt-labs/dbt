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

#[test]
fn test_fetch_foreign_key_constraints_sql_scopes_joins_to_constraint_schema() {
    let sql = include_str!(
        "../../src/dbt_macro_assets/dbt-databricks/macros/relations/components/constraints.sql"
    );
    let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .with_macro_at_path(
            "dbt_databricks",
            "fetch_foreign_key_constraints_sql",
            sql,
            "dbt_macro_assets/dbt-databricks/macros/relations/components/constraints.sql",
        )
        .build()
        .expect("harness should build");
    let relation = harness.relation(
        "some_database",
        "some_schema",
        "some_table",
        Some(RelationType::Table),
    );

    let rendered = harness
        .render(
            "{{ fetch_foreign_key_constraints_sql(relation) }}",
            [(
                "relation".to_string(),
                RelationObject::new(relation).into_value(),
            )]
            .into_iter()
            .collect::<std::collections::BTreeMap<String, Value>>(),
        )
        .expect("render should succeed");

    assert_eq!(
        rendered.split_whitespace().collect::<Vec<_>>().join(" "),
        "SELECT kcu.constraint_name, kcu.column_name AS from_column, \
         ukcu.table_catalog AS to_catalog, ukcu.table_schema AS to_schema, \
         ukcu.table_name AS to_table, ukcu.column_name AS to_column \
         FROM `some_database`.information_schema.key_column_usage kcu \
         JOIN `some_database`.information_schema.referential_constraints rc \
         ON kcu.constraint_catalog = rc.constraint_catalog \
         AND kcu.constraint_schema = rc.constraint_schema \
         AND kcu.constraint_name = rc.constraint_name \
         JOIN `some_database`.information_schema.key_column_usage ukcu \
         ON rc.unique_constraint_catalog = ukcu.constraint_catalog \
         AND rc.unique_constraint_schema = ukcu.constraint_schema \
         AND rc.unique_constraint_name = ukcu.constraint_name \
         AND kcu.ordinal_position = ukcu.ordinal_position \
         WHERE kcu.table_catalog = 'some_database' \
         AND kcu.table_schema = 'some_schema' \
         AND kcu.table_name = 'some_table' \
         AND kcu.constraint_name IN ( SELECT constraint_name \
         FROM `some_database`.information_schema.table_constraints \
         WHERE table_catalog = 'some_database' AND table_schema = 'some_schema' \
         AND table_name = 'some_table' AND constraint_type = 'FOREIGN KEY' ) \
         ORDER BY kcu.ordinal_position;"
    );
}
