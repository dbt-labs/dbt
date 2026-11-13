use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::{Relation, RelationObject};
use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

const DROP_PATH: &str = "dbt-snowflake/macros/relations/table/drop.sql";
const DROP_SQL: &str =
    include_str!("../../src/dbt_macro_assets/dbt-snowflake/macros/relations/table/drop.sql");

/// Stub CLD detector: only the string `CLD_MODEL` (standing in for `config.model`) counts as
/// catalog-linked. This mirrors the real failure in fs#16629, where the relation and the
/// bare-database catalog relation never see the CLD but the model config does.
const STUB_IS_CLD: &str = r#"{% macro snowflake__is_catalog_linked_database(relation=none, catalog_relation=none) -%}
    {{ return(relation is string and relation == 'CLD_MODEL') }}
{%- endmacro %}"#;

fn render_drop(model: &str) -> String {
    let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .with_macro(
            "dbt_snowflake",
            "snowflake__is_catalog_linked_database",
            STUB_IS_CLD,
        )
        .with_macro_at_path(
            "dbt_snowflake",
            "snowflake__drop_table",
            DROP_SQL,
            DROP_PATH,
        )
        .build()
        .expect("harness should build");
    // A bare database string resolves to the default catalog relation (no CLD).
    harness
        .mock()
        .on("build_catalog_relation", |_| Ok(Value::from(false)));

    let config = Arc::new(MockJinjaObject::new());
    config.set_attr("model", Value::from(model));
    let relation = RelationObject::new(Arc::new(Relation::new(
        AdapterType::Snowflake,
        "cld".to_string(),
        "schema_a".to_string(),
        "orders__dbt_tmp".to_string(),
    )));
    let ctx = BTreeMap::from([
        ("dbt_version".to_string(), Value::from("2.0.7")),
        ("config".to_string(), Value::from_dyn_object(config)),
        ("relation".to_string(), relation.into_value()),
    ]);
    harness
        .render("{{ snowflake__drop_table(relation) }}", ctx)
        .expect("drop_table should render")
}

#[test]
fn drop_table_omits_cascade_when_model_is_catalog_linked() {
    let sql = render_drop("CLD_MODEL").to_lowercase();
    assert!(sql.contains("drop table if exists"), "got: {sql}");
    assert!(
        !sql.contains("cascade"),
        "CLD drop must not cascade, got: {sql}"
    );
}

#[test]
fn drop_table_keeps_cascade_for_regular_database() {
    let sql = render_drop("PLAIN_MODEL").to_lowercase();
    assert!(
        sql.contains("cascade"),
        "regular drop keeps cascade, got: {sql}"
    );
}
