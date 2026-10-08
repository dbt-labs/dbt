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
    let diff: IndexMap<String, String> = desired_state
        .iter()
        .filter(|(name, value)| current_state.get(*name) != Some(*value))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();

    if diff.is_empty() { None } else { Some(diff) }
}

pub(super) fn tag_value_to_string(value: &YmlValue) -> String {
    match value {
        YmlValue::Null(_) => String::new(),
        YmlValue::Timestamp(timestamp, _) => timestamp.to_string(),
        YmlValue::Number(number, _) if number.is_f64() => float_tag_value_to_string(number),
        _ => Value::from_serialize(value).to_string(),
    }
}

fn float_tag_value_to_string(number: &dbt_yaml::Number) -> String {
    // YAML's shortest formatter preserves ties-to-even; Python differs at 10^-5 and exponents.
    let value = number.to_string();
    match value.as_str() {
        ".nan" => return "nan".to_string(),
        ".inf" => return "inf".to_string(),
        "-.inf" => return "-inf".to_string(),
        _ => {}
    }
    let (sign, unsigned) = match value.strip_prefix('-') {
        Some(unsigned) => ("-", unsigned),
        None => ("", value.as_str()),
    };
    if let Some(digits) = unsigned.strip_prefix("0.0000") {
        let (first, rest) = digits.split_at(1);
        let mantissa = if rest.is_empty() {
            first.to_string()
        } else {
            format!("{first}.{rest}")
        };
        return format!("{sign}{mantissa}e-05");
    }
    match value.split_once('e') {
        Some((mantissa, exponent)) => {
            let (sign, digits) = match exponent.strip_prefix('-') {
                Some(digits) => ("-", digits),
                None => ("+", exponent),
            };
            format!("{mantissa}e{sign}{digits:0>2}")
        }
        None => value,
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
            && let (Some(tag_name), Some(tag_value)) =
                (tag_name_val.as_str(), tag_value_val.as_str())
        {
            tags.insert(tag_name.to_string(), tag_value.to_string());
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
            tags.insert(key.clone(), tag_value_to_string(value));
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
        assert!(!diff.value.contains_key("a"));
    }

    #[test]
    fn test_get_diff_omits_unchanged_desired_keys() {
        let old_config = new_component(IndexMap::from([
            ("stable".to_string(), "1".to_string()),
            ("moved".to_string(), "old".to_string()),
            ("remote_only".to_string(), "x".to_string()),
        ]));
        let new_config = new_component(IndexMap::from([
            ("stable".to_string(), "1".to_string()),
            ("moved".to_string(), "new".to_string()),
        ]));

        let diff = RelationTags::diff_from(&new_config, Some(&old_config)).unwrap();
        let diff = diff.as_any().downcast_ref::<RelationTags>().unwrap();

        assert_eq!(
            diff.value,
            IndexMap::from([("moved".to_string(), "new".to_string())])
        );
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
    fn test_get_diff_empty_desired_does_not_unset_remote_tags() {
        let desired = new_component(IndexMap::new());
        let existing = new_component(IndexMap::from([("tag".to_string(), "value".to_string())]));

        assert!(RelationTags::diff_from(&desired, Some(&existing)).is_none());
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

    #[test]
    fn test_tag_value_to_string() {
        for (input, expected) in [
            ("null", ""),
            ("0", "0"),
            ("false", "False"),
            ("value", "value"),
        ] {
            let value: YmlValue = dbt_yaml::from_str(input).unwrap();
            assert_eq!(tag_value_to_string(&value), expected, "{input}");
        }
    }
}
