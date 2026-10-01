use dbt_common::{AdapterError, AdapterResult};
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::Value;

use crate::relation::config_v2::diff;
use crate::relation::snowflake::config::SnowflakeDescribeResults;
use crate::{
    relation::config_v2::{
        ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, impl_loader,
    },
    value::none_value,
};

pub(crate) const TYPE_NAME: &str = "row_access_policy";

/// Component for Snowflake dynamic table row access policy.
pub(crate) type RowAccessPolicy = SimpleComponentConfigImpl<Option<String>>;

fn to_jinja(v: &Option<String>) -> Value {
    v.as_ref().map(Value::from).unwrap_or_else(none_value)
}

fn new_component(row_access_policy: Option<String>) -> RowAccessPolicy {
    RowAccessPolicy {
        type_name: TYPE_NAME,
        diff_fn: diff::immutable,
        to_jinja_fn: to_jinja,
        value: row_access_policy,
    }
}

fn from_remote_state(_results: &SnowflakeDescribeResults) -> AdapterResult<RowAccessPolicy> {
    // In Core, the row access policy is extracted here but never accessed
    // https://github.com/dbt-labs/dbt-adapters/blob/cb1b4a0b0758fd307dc21583bb3acfc78397a077/dbt-snowflake/src/dbt/adapters/snowflake/relation_configs/dynamic_table.py#L235
    Ok(new_component(None))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<RowAccessPolicy> {
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
    Ok(new_component(snowflake_config.row_access_policy.clone()))
}

impl_loader!(RowAccessPolicy, SnowflakeDescribeResults);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    // Row access policy isn't returned from the remote state

    #[test]
    fn from_local_state_none() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            row_access_policy: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_state_some() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            row_access_policy: Some("snowflake_bogus_policy"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_some());
        assert_eq!(loaded.value.unwrap(), "snowflake_bogus_policy");
    }
}
