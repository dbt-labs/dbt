use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, default_mock_config};

#[test]
fn python_table_tmp_relation_type_is_allowed() {
    let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let config = default_mock_config();
    config.on("get", |args| {
        let key = args.first().and_then(|v| v.as_str());
        let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
        match key {
            Some("tmp_relation_type") => Ok(Value::from("table")),
            _ => Ok(default),
        }
    });

    let ctx = BTreeMap::from([("config".to_string(), Value::from_dyn_object(config))]);

    let rendered = harness
        .render(
            "{{ dbt_snowflake_get_tmp_relation_type('default', none, 'python') }}",
            ctx,
        )
        .expect("table is valid for Python tmp_relation_type");

    assert_eq!(rendered.trim(), "table");
}

fn render_python_table(temporary: bool, is_transient: bool) -> String {
    let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let catalog_relation = Arc::new(MockJinjaObject::new());
    catalog_relation.set_attr("catalog_type", Value::from("INFO_SCHEMA"));
    catalog_relation.set_attr("is_transient", Value::from(is_transient));
    harness.mock().on("build_catalog_relation", move |_| {
        Ok(Value::from_dyn_object(catalog_relation.clone()))
    });

    let ctx = harness
        .materialization_context(
            "orders",
            "def model(dbt, session):\n    return session.table('orders')",
        )
        .with("temporary", Value::from(temporary))
        .build();

    harness
        .render(
            "{{ snowflake__create_table_as(temporary, this, compiled_code, 'python') }}",
            ctx,
        )
        .expect("Python table macro should render")
}

#[test]
fn python_incremental_staging_table_is_temporary() {
    let rendered = render_python_table(true, true);

    assert!(
        rendered.contains("table_type='temporary'"),
        "Python incremental staging tables should be temporary, got:\n{rendered}"
    );
    assert!(!rendered.contains("table_type='transient'"));
}

#[test]
fn python_table_preserves_transient_config() {
    let rendered = render_python_table(false, true);

    assert!(
        rendered.contains("table_type='transient'"),
        "Python table models should preserve transient configuration, got:\n{rendered}"
    );
    assert!(!rendered.contains("table_type='temporary'"));
}

#[test]
fn alter_relation_comment_uses_iceberg_syntax_after_incorporate() {
    use dbt_adapter::relation::{Relation, RelationObject};
    use dbt_schemas::dbt_types::RelationType;
    use dbt_schemas::schemas::relations::base::{BaseRelation, TableFormat};

    let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let iceberg: Arc<dyn BaseRelation> = Arc::new(
        Relation::new(
            AdapterType::Snowflake,
            "TEST_DB".to_string(),
            "TEST_SCHEMA".to_string(),
            "my_iceberg_table".to_string(),
        )
        .with_relation_type(Some(RelationType::Table))
        .with_table_format(TableFormat::Iceberg),
    );

    // Mirrors `target_relation.incorporate(type='table')` in the incremental
    // materialization, called right before `persist_docs` (fs#14268).
    let incorporated = iceberg
        .incorporate(None, Some(RelationType::Table), None)
        .expect("incorporate should succeed");

    let ctx = BTreeMap::from([
        (
            "relation".to_string(),
            RelationObject::new(incorporated).into_value(),
        ),
        ("relation_comment".to_string(), Value::from("a comment")),
    ]);

    let rendered = harness
        .render(
            "{{ snowflake__alter_relation_comment(relation, relation_comment) }}",
            ctx,
        )
        .expect("alter_relation_comment should render");

    assert!(
        rendered.contains("alter iceberg table"),
        "expected iceberg ALTER syntax after incorporate, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("comment on table"),
        "must not fall back to plain COMMENT ON TABLE for an Iceberg relation, got:\n{rendered}"
    );
}
