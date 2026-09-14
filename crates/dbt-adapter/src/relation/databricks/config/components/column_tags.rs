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
use dbt_yaml::Value as YmlValue;
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
        diff_fn: changed_column_tags_diff,
        to_jinja_fn: to_jinja,
        value: tags,
    }
}

/// Set-only column-tag diff matching dbt-databricks `ColumnTagsConfig.get_diff`.
///
/// Reference: https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/column_tags.py
fn changed_column_tags_diff(
    desired_state: &IndexMap<String, IndexMap<String, String>>,
    current_state: &IndexMap<String, IndexMap<String, String>>,
) -> Option<IndexMap<String, IndexMap<String, String>>> {
    // Identifiers are case-insensitive, so compare column names after lowercasing.
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
                    if let YmlValue::String(value_str, _) = tag_value {
                        column_tag_map.insert(tag_name.clone(), value_str.clone());
                    }
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

    fn tags(entries: &[(&str, &[(&str, &str)])]) -> IndexMap<String, IndexMap<String, String>> {
        entries
            .iter()
            .map(|(col, col_tags)| {
                (
                    (*col).to_string(),
                    col_tags
                        .iter()
                        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn test_get_diff_column_tags() {
        // Desired col1 replaces tags (no remote-only keys); col2 is new.
        let remote = tags(&[("col1", &[("old_tag", "old_value")])]);
        let desired = tags(&[
            ("col1", &[("new_tag", "new_value")]),
            ("col2", &[("col2_tag", "col2_value")]),
        ]);

        let diff = changed_column_tags_diff(&desired, &remote).unwrap();

        assert_eq!(
            diff,
            tags(&[
                ("col1", &[("new_tag", "new_value")]),
                ("col2", &[("col2_tag", "col2_value")]),
            ])
        );
        assert!(!diff.get("col1").unwrap().contains_key("old_tag"));
    }

    #[test]
    fn test_get_diff_empty_desired_is_none() {
        // No changeset → apply emits no ALTER; remote-only tags are left as-is.
        let remote = tags(&[("col1", &[("tag_a", "value_a"), ("tag_b", "value_b")])]);
        let desired = IndexMap::new();
        assert!(changed_column_tags_diff(&desired, &remote).is_none());
    }

    #[test]
    fn test_get_diff_desired_with_empty_remote() {
        let desired = tags(&[("col1", &[("tag_a", "value_a"), ("tag_b", "value_b")])]);
        let remote = IndexMap::new();
        let diff = changed_column_tags_diff(&desired, &remote).unwrap();
        assert_eq!(diff, desired);
    }

    #[test]
    fn test_get_diff_mixed_omits_remote_only_and_uses_desired_maps() {
        // col1 differs (desired map, not union with tag_d); col2 is new; col3 remote-only omitted.
        let desired = tags(&[
            ("col1", &[("tag_a", "new_value"), ("tag_b", "value_b")]),
            ("col2", &[("tag_c", "value_c")]),
        ]);
        let remote = tags(&[
            ("col1", &[("tag_a", "old_value"), ("tag_d", "value_d")]),
            ("col3", &[("tag_e", "value_e")]),
        ]);

        let diff = changed_column_tags_diff(&desired, &remote).unwrap();
        assert_eq!(
            diff,
            tags(&[
                ("col1", &[("tag_a", "new_value"), ("tag_b", "value_b")]),
                ("col2", &[("tag_c", "value_c")]),
            ])
        );
        assert!(!diff.contains_key("col3"));
    }

    #[test]
    fn test_get_diff_no_change() {
        let column_tags = tags(&[("col1", &[("tag1", "value1")])]);
        assert!(changed_column_tags_diff(&column_tags, &column_tags).is_none());
    }

    #[test]
    fn test_get_diff_case_insensitive_column_names_no_change() {
        let desired = tags(&[
            ("account_id", &[("pii", "true")]),
            ("user_name", &[("pii", "true")]),
        ]);
        let remote = tags(&[
            ("Account_ID", &[("pii", "true")]),
            ("User_Name", &[("pii", "true")]),
        ]);
        assert!(changed_column_tags_diff(&desired, &remote).is_none());
    }

    #[test]
    fn test_get_diff_case_insensitive_with_actual_change() {
        let desired = tags(&[
            ("account_id", &[("pii", "false")]),
            ("user_name", &[("pii", "true")]),
        ]);
        let remote = tags(&[
            ("Account_ID", &[("pii", "true")]),
            ("User_Name", &[("pii", "true")]),
        ]);

        let diff = changed_column_tags_diff(&desired, &remote).unwrap();
        assert_eq!(diff, tags(&[("account_id", &[("pii", "false")])]));
    }

    #[test]
    fn test_get_diff_omits_unchanged_sibling_column() {
        let remote = tags(&[("col1", &[("a", "1")]), ("col2", &[("c", "3")])]);
        let desired = tags(&[("col1", &[("a", "1"), ("b", "2")]), ("col2", &[("c", "3")])]);

        let diff = changed_column_tags_diff(&desired, &remote).unwrap();
        assert_eq!(diff, tags(&[("col1", &[("a", "1"), ("b", "2")])]));
        assert!(!diff.contains_key("col2"));
    }
}
