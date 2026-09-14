//! https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/tags.py

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

pub(crate) const TYPE_NAME: &str = "tags";

// TODO(serramatutu): reuse this for `tags` and `labels` in other warehouses
/// Component for Databricks tags.
pub type RelationTags = SimpleComponentConfigImpl<IndexMap<String, String>>;

fn set_only_diff(
    desired_state: &IndexMap<String, String>,
    current_state: &IndexMap<String, String>,
) -> Option<IndexMap<String, String>> {
    let has_diff = desired_state
        .iter()
        .any(|(name, value)| current_state.get(name) != Some(value));

    if has_diff {
        Some(desired_state.clone())
    } else {
        None
    }
}

pub(crate) fn stringify_tag_value(value: &YmlValue) -> String {
    match value {
        YmlValue::Null(_) => String::new(),
        YmlValue::Bool(value, _) => if *value { "True" } else { "False" }.to_string(),
        YmlValue::Number(value, _) => value.to_string(),
        YmlValue::String(value, _) => value.clone(),
        YmlValue::Tagged(value, _) => stringify_tag_value(&value.value),
        value => dbt_yaml::to_string(value)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

fn to_jinja(v: &IndexMap<String, String>) -> Value {
    Value::from(ValueMap::from([(
        Value::from("set_tags"),
        Value::from_serialize(v),
    )]))
}

fn new_component(tags: IndexMap<String, String>) -> RelationTags {
    RelationTags {
        type_name: TYPE_NAME,
        diff_fn: set_only_diff,
        to_jinja_fn: to_jinja,
        value: tags,
    }
}

fn from_remote_state(results: &DatabricksRelationMetadata) -> AdapterResult<RelationTags> {
    let Some(remote_tags) = results.get(&DatabricksRelationMetadataKey::InfoSchemaRelationTags)
    else {
        return Ok(new_component(IndexMap::new()));
    };

    let mut tags = IndexMap::new();

    for row in remote_tags.rows() {
        if let (Ok(tag_name_val), Ok(tag_value_val)) =
            (row.get_item(&Value::from(0)), row.get_item(&Value::from(1)))
            && let Some(tag_name) = tag_name_val.as_str()
        {
            tags.insert(
                tag_name.to_string(),
                tag_value_val.as_str().unwrap_or_default().to_string(),
            );
        }
    }

    Ok(new_component(tags))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<RelationTags> {
    let Some(model) = relation_config.as_any().downcast_ref::<DbtModel>() else {
        return Ok(new_component(IndexMap::new()));
    };

    let mut tags = IndexMap::new();

    if let Some(databricks_attr) = &model.__adapter_attr__.databricks_attr
        && let Some(tags_map) = &databricks_attr.databricks_tags
    {
        for (key, value) in tags_map {
            tags.insert(key.clone(), stringify_tag_value(value));
        }
    }

    Ok(new_component(tags))
}

impl_loader!(RelationTags, DatabricksRelationMetadata);

impl RelationTagsLoader {
    /// `None` means the desired config is unknown, so fetch. Empty desired tags means skip.
    pub(crate) fn requires_server_metadata_for_diff(model_config: Option<&RelationConfig>) -> bool {
        model_config
            .and_then(|config| config.get(TYPE_NAME))
            .and_then(|component| component.as_any().downcast_ref::<RelationTags>())
            .is_none_or(|tags| !tags.value.is_empty())
    }

    pub fn new_component_type_erased(tags: IndexMap<String, String>) -> Box<dyn ComponentConfig> {
        Box::new(new_component(tags))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::config_v2::ComponentConfig;

    #[test]
    fn test_scalar_values_use_python_stringification() {
        let integer = dbt_yaml::from_str("1").unwrap();
        let null = dbt_yaml::from_str("null").unwrap();
        let boolean = dbt_yaml::from_str("false").unwrap();

        assert_eq!(stringify_tag_value(&integer), "1");
        assert_eq!(stringify_tag_value(&null), "");
        assert_eq!(stringify_tag_value(&boolean), "False");
    }

    #[test]
    fn test_get_diff_add_or_update() {
        let mut old_tags = IndexMap::new();
        old_tags.insert("a".to_string(), "1".to_string());
        old_tags.insert("b".to_string(), "2".to_string());

        let mut new_tags = IndexMap::new();
        new_tags.insert("b".to_string(), "3".to_string());
        new_tags.insert("c".to_string(), "4".to_string());

        let old_config = new_component(old_tags);
        let new_config = new_component(new_tags);

        let diff = RelationTags::diff_from(&new_config, Some(&old_config)).unwrap();
        let diff = diff.as_any().downcast_ref::<RelationTags>().unwrap();

        assert_eq!(diff.value.get("b"), Some(&"3".to_string()));
        assert_eq!(diff.value.get("c"), Some(&"4".to_string()));
    }

    #[test]
    fn test_get_diff_no_change() {
        let mut tags = IndexMap::new();
        tags.insert("a".to_string(), "1".to_string());
        tags.insert("b".to_string(), "2".to_string());

        let config = new_component(tags);
        let diff = RelationTags::diff_from(&config, Some(&config));

        assert!(diff.is_none());
    }

    #[test]
    fn test_get_diff_ignores_unconfigured_existing_tags() {
        let desired = new_component(IndexMap::from([(
            "managed".to_string(),
            "true".to_string(),
        )]));
        let existing = new_component(IndexMap::from([
            ("managed".to_string(), "true".to_string()),
            ("external".to_string(), "preserved".to_string()),
        ]));

        assert!(RelationTags::diff_from(&desired, Some(&existing)).is_none());
    }
}
