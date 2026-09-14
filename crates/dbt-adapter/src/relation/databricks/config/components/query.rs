//! https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/query.py

use crate::AdapterType;
use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
};
use crate::relation::databricks::config::{
    DatabricksRelationMetadata, DatabricksRelationMetadataKey,
};
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use minijinja::value::{Value, ValueMap};

pub(crate) const TYPE_NAME: &str = "query";

// TODO(serramatutu): reuse this for `query` or `sql` in other warehouses
/// Component for Databricks query.
pub type Query = SimpleComponentConfigImpl<String>;

// `&String` is required by `ToJinjaFn`
#[expect(clippy::ptr_arg)]
fn to_jinja(v: &String) -> Value {
    // `databricks__alter_view` reads `changes.get("query").query`
    Value::from(ValueMap::from([(
        Value::from("query"),
        Value::from(v.clone()),
    )]))
}

fn new_component(query: &str) -> Query {
    Query {
        type_name: TYPE_NAME,
        diff_fn: diff::desired_state,
        to_jinja_fn: to_jinja,
        value: dbt_adapter_sql::statements::clean_sql(query, AdapterType::Databricks),
    }
}

fn from_remote_state(results: &DatabricksRelationMetadata) -> AdapterResult<Query> {
    let Some(views) = results.get(&DatabricksRelationMetadataKey::InfoSchemaViews) else {
        return Ok(new_component(""));
    };

    let Some(row) = views.rows().into_iter().next() else {
        return Ok(new_component(""));
    };

    let view_definition = row
        .get_attr("view_definition")
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let view_definition = view_definition.trim();

    // `CREATE VIEW` / `ALTER VIEW` submit the query wrapped in parentheses, and Databricks stores
    // the definition as submitted, so unwrap it before comparing against the model's own SQL.
    let view_definition = view_definition
        .strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
        .unwrap_or(view_definition);

    Ok(new_component(view_definition))
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<Query> {
    // Without the compiled SQL there is nothing to compare the applied view definition against.
    // Failing here is deliberate: reporting "no change" would silently leave the view pointing at
    // a stale definition, which is exactly what `view_update_via_alter` exists to prevent.
    let Some(compiled_code) = relation_config.compiled_code() else {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "Cannot compile model `{}` with no SQL query",
                relation_config.name()
            ),
        ));
    };

    Ok(new_component(compiled_code))
}

impl_loader!(Query, DatabricksRelationMetadata);

impl QueryLoader {
    pub fn new_component_type_erased(query: &str) -> Box<dyn ComponentConfig> {
        Box::new(new_component(query))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::databricks::describe_json::*;
    use crate::metadata::databricks::*;
    use crate::relation::databricks::config::test_helpers;
    use arrow::array::{ArrayRef, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use dbt_agate::AgateTable;
    use indexmap::IndexMap;
    use std::sync::Arc;

    /// Mimics `SELECT * FROM system.information_schema.views`, which is where the applied view
    /// definition comes from.
    fn create_mock_info_schema_views(view_definitions: &[&str]) -> AgateTable {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "view_definition",
            DataType::Utf8,
            true,
        )]));
        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(StringArray::from(view_definitions.to_vec())) as ArrayRef],
        )
        .unwrap();

        AgateTable::from_record_batch(Arc::new(batch))
    }

    fn from_remote_state_with(view_definitions: &[&str]) -> Query {
        let results = IndexMap::from([(
            DatabricksRelationMetadataKey::InfoSchemaViews,
            create_mock_info_schema_views(view_definitions),
        )]);
        from_remote_state(&results).unwrap()
    }

    #[test]
    fn test_from_remote_state_unwraps_the_stored_view_definition() {
        // Databricks stores the definition as submitted, i.e. wrapped in the parentheses that
        // `CREATE VIEW` / `ALTER VIEW` emit around the model's SQL.
        let config = from_remote_state_with(&["(\n    select 1 as id\n  )"]);

        assert_eq!(config.value, "select 1 as id");
    }

    #[test]
    fn test_from_remote_state_leaves_an_unwrapped_definition_alone() {
        let config = from_remote_state_with(&["  select 1 as id;  "]);

        assert_eq!(config.value, "select 1 as id");
    }

    #[test]
    fn test_from_remote_state_without_info_schema_views() {
        let config = from_remote_state(&IndexMap::new()).unwrap();

        assert_eq!(config.value, "");
    }

    #[test]
    fn test_from_remote_state_without_rows() {
        let config = from_remote_state_with(&[]);

        assert_eq!(config.value, "");
    }

    #[test]
    fn test_as_json_path_produces_no_false_diff_vs_info_schema_path() {
        let info_schema_config = from_remote_state_with(&["(\n    select 1 as id\n  )"]);

        let row = Some(ViewDescriptionRow {
            view_definition: "(\n    select 1 as id\n  )".to_string(),
        });
        let as_json_results = IndexMap::from([(
            DatabricksRelationMetadataKey::InfoSchemaViews,
            view_description_to_agate(&row).unwrap(),
        )]);
        let as_json_config = from_remote_state(&as_json_results).unwrap();

        assert_eq!(as_json_config.value, info_schema_config.value);
    }

    #[test]
    fn test_from_local_config_reads_the_compiled_code() {
        let model = test_helpers::create_mock_dbt_model(test_helpers::TestModelConfig {
            query: Some("select 1 as id".to_string()),
            ..Default::default()
        });
        let config = from_local_config(&model).unwrap();

        assert_eq!(config.value, "select 1 as id");
    }

    #[test]
    fn test_from_local_config_without_compiled_code_is_an_error() {
        let model = test_helpers::create_mock_dbt_model(test_helpers::TestModelConfig {
            query: None,
            ..Default::default()
        });
        let err = from_local_config(&model).unwrap_err();

        assert!(
            err.to_string().contains("with no SQL query"),
            "unexpected error: {err}"
        );
    }
}
