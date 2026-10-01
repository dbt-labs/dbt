use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
};

use crate::value::none_value;
use arrow_schema::Schema;
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::value::Value;

pub(crate) const TYPE_NAME: &str = "description";

/// Component for BigQuery relation description.
pub(crate) type Description = SimpleComponentConfigImpl<String>;

fn new_component(description: String) -> Description {
    Description {
        type_name: TYPE_NAME,
        diff_fn: diff::desired_state,
        to_jinja_fn: |v| {
            if v.is_empty() {
                none_value()
            } else {
                let escaped = v.replace("\n", "\\n");
                Value::from(format!("\"\"\"{escaped}\"\"\""))
            }
        },
        value: description,
    }
}

fn from_remote_state(schema: &Schema) -> AdapterResult<Description> {
    Ok(new_component(
        schema
            .metadata
            .get("Description")
            .map(|s| s.to_owned())
            .unwrap_or_else(|| "".to_owned()),
    ))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<Description> {
    let config = match relation_config.as_any().downcast_ref::<DbtModel>() {
        None => "".to_string(),
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
            .description
            .as_ref()
            .map(|k| k.to_string())
            .unwrap_or_else(|| "".to_string()),
    };
    Ok(new_component(config))
}

impl_loader!(Description, Schema);

impl DescriptionLoader {
    pub fn new_component_type_erased(description: String) -> Box<dyn ComponentConfig> {
        Box::new(new_component(description))
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
    fn from_remote_state_empty() {
        let driver_data = make_driver_data(TestTableConfig {
            description: "",
            ..Default::default()
        });
        let loaded = from_remote_state(&driver_data).unwrap();
        assert!(loaded.value.is_empty());
    }

    #[test]
    fn from_remote_state_some() {
        let driver_data = make_driver_data(TestTableConfig {
            description: "hello world",
            ..Default::default()
        });
        let loaded = from_remote_state(&driver_data).unwrap();
        assert_eq!(&loaded.value, "hello world");
    }

    #[test]
    fn from_local_config_not_configured() {
        let local_data = make_local_config(Default::default());
        let loaded = from_local_config(&local_data).unwrap();
        assert!(loaded.value.is_empty());
    }

    #[test]
    fn from_local_config_some() {
        let local_data = make_local_config(TestTableConfig {
            description: "hello world",
            ..Default::default()
        });
        let loaded = from_local_config(&local_data).unwrap();
        assert_eq!(&loaded.value, "hello world");
    }
}
