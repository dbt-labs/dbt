use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
};

use crate::value::none_value;
use arrow_schema::Schema;
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::value::Value;
use serde_json;

use indexmap::IndexMap;

pub(crate) const TYPE_NAME: &str = "tags";

/// Component for BigQuery relation tags
pub(crate) type Tags = SimpleComponentConfigImpl<IndexMap<String, String>>;

fn new_component(tags: IndexMap<String, String>) -> Tags {
    Tags {
        type_name: TYPE_NAME,
        diff_fn: diff::desired_state,
        to_jinja_fn: |v| {
            if v.is_empty() {
                none_value()
            } else {
                Value::from_serialize(Vec::from_iter(v.iter()))
            }
        },
        value: tags,
    }
}

fn from_remote_state(schema: &Schema) -> AdapterResult<Tags> {
    Ok(new_component(
        schema
            .metadata
            .get("ResourceTags")
            .map(|tags_json| {
                if tags_json.is_empty() {
                    IndexMap::new()
                } else {
                    // SAFETY: this assumes the driver will never return
                    // stuff that is not JSON-encoded
                    serde_json::from_str(tags_json).unwrap_or_default()
                }
            })
            .unwrap_or_default(),
    ))
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<Tags> {
    let config = match relation_config.as_any().downcast_ref::<DbtModel>() {
        None => IndexMap::default(),
        Some(model) => model
            .__adapter_attr__
            .bigquery_attr
            .as_ref()
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "relation config needs to be BigQuery model".to_string(),
                )
            })?
            .resource_tags
            .as_ref()
            .map(|t| IndexMap::from_iter(t.iter().map(|(k, v)| (k.to_string(), v.to_string()))))
            .unwrap_or_default(),
    };
    Ok(new_component(config))
}

impl_loader!(Tags, Schema);

impl TagsLoader {
    pub fn new_component_type_erased(tags: IndexMap<String, String>) -> Box<dyn ComponentConfig> {
        Box::new(new_component(tags))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::bigquery::config::test_helpers::{
        TestTableConfig, make_driver_data, make_local_config,
    };
    use std::collections::HashMap;

    #[test]
    fn from_remote_state_no_tags() {
        let driver_data = make_driver_data(Default::default());
        let loaded = from_remote_state(&driver_data).unwrap();
        assert!(loaded.value.is_empty());
    }

    #[test]
    fn from_remote_state_with_tags() {
        let driver_data = make_driver_data(TestTableConfig {
            tags: HashMap::from([("tag_a", "val_a"), ("tag_b", "val_b")]),
            ..Default::default()
        });
        let loaded = from_remote_state(&driver_data).unwrap();
        assert_eq!(loaded.value.len(), 2);
        assert_eq!(loaded.value.get("tag_a"), Some(&"val_a".to_string()));
        assert_eq!(loaded.value.get("tag_b"), Some(&"val_b".to_string()));
    }

    #[test]
    fn from_local_config_no_tags() {
        let local_data = make_local_config(Default::default());
        let loaded = from_local_config(&local_data).unwrap();
        assert!(loaded.value.is_empty());
    }

    #[test]
    fn from_local_config_with_tags() {
        let local_data = make_local_config(TestTableConfig {
            tags: HashMap::from([("tag_a", "val_a"), ("tag_b", "val_b")]),
            ..Default::default()
        });
        let loaded = from_local_config(&local_data).unwrap();
        assert_eq!(loaded.value.len(), 2);
        assert_eq!(loaded.value.get("tag_a"), Some(&"val_a".to_string()));
        assert_eq!(loaded.value.get("tag_b"), Some(&"val_b".to_string()));
    }
}
