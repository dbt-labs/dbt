use dbt_adapter_core::AdapterType;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

fn build_harness() -> MacroTestHarness {
    let sql = include_str!("../../src/dbt_macro_assets/dbt-databricks/macros/adapters/catalog.sql");
    MacroTestHarness::for_adapter(AdapterType::Databricks)
        .with_macro_at_path(
            "dbt_databricks",
            "databricks__get_catalog_schemas_where_clause_sql",
            sql,
            "dbt_macro_assets/dbt-databricks/macros/adapters/catalog.sql",
        )
        .build()
        .expect("harness should build")
}

#[test]
fn test_get_catalog_schemas_where_clause_filters_on_full_schema_name() {
    let harness = build_harness();

    let rendered = harness
        .render(
            "{{ databricks__get_catalog_schemas_where_clause_sql('E2E', ['silver_sap']) }}",
            std::collections::BTreeMap::<String, Value>::new(),
        )
        .expect("render should succeed");

    let normalized = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(
        normalized,
        "WHERE table_catalog = 'e2e' AND (table_schema = 'silver_sap')"
    );
}
