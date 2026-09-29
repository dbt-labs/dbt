use dbt_common::{AdapterError, AdapterResult};
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::Value;

use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
};
use crate::relation::snowflake::config::SnowflakeDescribeResults;

pub(crate) const TYPE_NAME: &str = "initialize";

const DEFAULT_INITIALIZE: &str = "ON_CREATE";

/// Component for Snowflake dynamic table initialize setting.
pub(crate) type Initialize = SimpleComponentConfigImpl<String>;

fn to_jinja(v: &String) -> Value {
    Value::from(v)
}

fn new_component(initialize: String) -> Initialize {
    Initialize {
        type_name: TYPE_NAME,
        diff_fn: diff::immutable,
        to_jinja_fn: to_jinja,
        value: initialize,
    }
}

fn from_remote_state(_results: &SnowflakeDescribeResults) -> AdapterResult<Initialize> {
    // We don't get initialize since that's a one-time scheduler attribute, not a DT attribute
    Ok(new_component(DEFAULT_INITIALIZE.to_string()))
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<Initialize> {
    let snowflake_config = relation_config
        .as_any()
        .downcast_ref::<DbtModel>()
        .ok_or_else(|| {
            AdapterError::new(
                dbt_common::AdapterErrorKind::UnexpectedResult,
                "relation config needs to be a model",
            )
        })?
        .__adapter_attr__
        .snowflake_attr
        .as_ref()
        .ok_or_else(|| {
            AdapterError::new(
                dbt_common::AdapterErrorKind::Configuration,
                "relation config needs to be Snowflake model",
            )
        })?;
    let initialize = match snowflake_config.initialize.as_deref() {
        Some(value) => value.to_uppercase(),
        None => DEFAULT_INITIALIZE.to_string(),
    };
    Ok(new_component(initialize))
}

impl_loader!(Initialize, SnowflakeDescribeResults);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    // Initialize isn't returned from the remote state

    #[test]
    fn from_local_state_some() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            initialize: "ON_CREATE",
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value, "ON_CREATE")
    }
}
