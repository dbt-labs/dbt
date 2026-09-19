//! https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/column_tags.py

use crate::errors::AdapterResult;
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, RelationConfig, SimpleComponentConfigImpl, impl_loader,
};
use crate::relation::databricks::config::{
    DatabricksRelationMetadata, DatabricksRelationMetadataKey,
};
use dbt_schemas::schemas::DbtModel;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use indexmap::IndexMap;
use minijinja::value::{Value, ValueMap};

pub(crate) const TYPE_NAME: &str = "column_tags";

/// Component for Databricks column tags: column name -> (tag key -> value).
pub type ColumnTags = SimpleComponentConfigImpl<IndexMap<String, IndexMap<String, String>>>;

fn to_jinja(v: &IndexMap<String, IndexMap<String, String>>) -> Value {
    Value::from(ValueMap::from([(
        Value::from("set_column_tags"),
        Value::from_serialize(v),
    )]))
}

fn new_component(tags: IndexMap<String, IndexMap<String, String>>) -> ColumnTags {
    ColumnTags {
        type_name: TYPE_NAME,
        diff_fn: merge_tags_diff,
        to_jinja_fn: to_jinja,
        value: tags,
    }
}

fn merge_tags_diff(
    desired_state: &IndexMap<String, IndexMap<String, String>>,
    current_state: &IndexMap<String, IndexMap<String, String>>,
) -> Option<IndexMap<String, IndexMap<String, String>>> {
    let current_by_lower = current_state
        .iter()
        .map(|(column_name, tags)| (column_name.to_lowercase(), tags))
        .collect::<IndexMap<_, _>>();
    let changed = desired_state
        .iter()
        .filter(|(column_name, tags)| {
            current_by_lower
                .get(&column_name.to_lowercase())
                .is_none_or(|current| current != tags)
        })
        .map(|(column_name, tags)| (column_name.clone(), tags.clone()))
        .collect::<IndexMap<_, _>>();

    if changed.is_empty() {
        None
    } else {
        Some(changed)
    }
}

fn from_remote_state(results: &DatabricksRelationMetadata) -> AdapterResult<ColumnTags> {
    let mut column_tags: IndexMap<String, IndexMap<String, String>> = IndexMap::new();
    if let Some(column_tags_table) =
        results.get(&DatabricksRelationMetadataKey::InfoSchemaColumnTags)
    {
        for row in column_tags_table.rows() {
            if let (Ok(column_name_val), Ok(tag_name_val), Ok(tag_value_val)) = (
                row.get_item(&Value::from(0)),
                row.get_item(&Value::from(1)),
                row.get_item(&Value::from(2)),
            ) && let (Some(column_name), Some(tag_name), Some(tag_value)) = (
                column_name_val.as_str(),
                tag_name_val.as_str(),
                tag_value_val.as_str(),
            ) {
                column_tags
                    .entry(column_name.to_string())
                    .or_default()
                    .insert(tag_name.to_string(), tag_value.to_string());
            }
        }
    }

    Ok(new_component(column_tags))
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<ColumnTags> {
    let mut column_tags = IndexMap::new();

    if let Some(model) = relation_config.as_any().downcast_ref::<DbtModel>() {
        for column in &model.__base_attr__.columns {
            if let Some(column_databricks_tags) = &column.databricks_tags {
                let mut column_tag_map = IndexMap::new();
                for (tag_name, tag_value) in column_databricks_tags {
                    column_tag_map
                        .insert(tag_name.clone(), super::relation_tags::stringify_tag_value(tag_value));
                }
                if !column_tag_map.is_empty() {
                    column_tags.insert(column.name.clone(), column_tag_map);
                }
            }
        }
    }

    Ok(new_component(column_tags))
}

impl_loader!(ColumnTags, DatabricksRelationMetadata);

impl ColumnTagsLoader {
    /// Column tags are set-only: diffs add or update desired tags and never unset existing tags.
    /// When model config is available, fetch current tags only when column tags are configured.
    pub(crate) fn requires_server_metadata_for_diff(model_config: Option<&RelationConfig>) -> bool {
        model_config
            .and_then(|config| config.get(TYPE_NAME))
            .and_then(|component| component.as_any().downcast_ref::<ColumnTags>())
            .is_none_or(|tags| !tags.value.is_empty())
    }

    pub fn new_component_type_erased(
        tags: IndexMap<String, IndexMap<String, String>>,
    ) -> Box<dyn ComponentConfig> {
        Box::new(new_component(tags))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_diff_column_tags() {
        let mut old_column_tags = IndexMap::new();
        let mut old_col1_tags = IndexMap::new();
        old_col1_tags.insert("old_tag".to_string(), "old_value".to_string());
        old_column_tags.insert("col1".to_string(), old_col1_tags);

        let mut new_column_tags = IndexMap::new();
        let mut new_col1_tags = IndexMap::new();
        new_col1_tags.insert("new_tag".to_string(), "new_value".to_string());
        new_column_tags.insert("col1".to_string(), new_col1_tags);

        let mut new_col2_tags = IndexMap::new();
        new_col2_tags.insert("col2_tag".to_string(), "col2_value".to_string());
        new_column_tags.insert("col2".to_string(), new_col2_tags);

        let diff = merge_tags_diff(&new_column_tags, &old_column_tags).unwrap();

        let col1_tags = diff.get("col1").unwrap();
        assert_eq!(col1_tags.get("new_tag"), Some(&"new_value".to_string()));
        assert_eq!(col1_tags.len(), 1);

        let col2_tags = diff.get("col2").unwrap();
        assert_eq!(col2_tags.get("col2_tag"), Some(&"col2_value".to_string()));
    }

    #[test]
    fn test_get_diff_no_change() {
        let mut column_tags = IndexMap::new();
        let mut col_tags = IndexMap::new();
        col_tags.insert("tag1".to_string(), "value1".to_string());
        column_tags.insert("col1".to_string(), col_tags);

        let diff = merge_tags_diff(&column_tags, &column_tags);
        assert!(diff.is_none());
    }

    #[test]
    fn test_get_diff_matches_column_names_case_insensitively() {
        let desired = IndexMap::from([(
            "account_id".to_string(),
            IndexMap::from([("pii".to_string(), "true".to_string())]),
        )]);
        let existing = IndexMap::from([(
            "Account_ID".to_string(),
            IndexMap::from([("pii".to_string(), "true".to_string())]),
        )]);

        assert!(merge_tags_diff(&desired, &existing).is_none());
    }

    #[test]
    fn test_get_diff_emits_only_changed_desired_columns() {
        let desired = IndexMap::from([
            (
                "account_id".to_string(),
                IndexMap::from([("pii".to_string(), "false".to_string())]),
            ),
            (
                "user_name".to_string(),
                IndexMap::from([("pii".to_string(), "true".to_string())]),
            ),
        ]);
        let existing = IndexMap::from([
            (
                "Account_ID".to_string(),
                IndexMap::from([("pii".to_string(), "true".to_string())]),
            ),
            (
                "User_Name".to_string(),
                IndexMap::from([("pii".to_string(), "true".to_string())]),
            ),
            (
                "unmanaged".to_string(),
                IndexMap::from([("external".to_string(), "preserved".to_string())]),
            ),
        ]);

        assert_eq!(
            merge_tags_diff(&desired, &existing),
            Some(IndexMap::from([(
                "account_id".to_string(),
                IndexMap::from([("pii".to_string(), "false".to_string())]),
            )]))
        );
    }
}
