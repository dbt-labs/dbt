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

pub(crate) const TYPE_NAME: &str = "table_tag";

/// Component for Snowflake dynamic table tag.
pub(crate) type TableTag = SimpleComponentConfigImpl<Option<String>>;

fn to_jinja(v: &Option<String>) -> Value {
    v.as_ref().map(Value::from).unwrap_or_else(none_value)
}

fn new_component(table_tag: Option<String>) -> TableTag {
    TableTag {
        type_name: TYPE_NAME,
        diff_fn: diff::immutable,
        to_jinja_fn: to_jinja,
        value: table_tag,
    }
}

fn from_remote_state(_results: &SnowflakeDescribeResults) -> AdapterResult<TableTag> {
    // In Core, the table tag is extracted here but never accessed
    // https://github.com/dbt-labs/dbt-adapters/blob/cb1b4a0b0758fd307dc21583bb3acfc78397a077/dbt-snowflake/src/dbt/adapters/snowflake/relation_configs/dynamic_table.py#L236
    Ok(new_component(None))
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<TableTag> {
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
    Ok(new_component(snowflake_config.table_tag.clone()))
}

impl_loader!(TableTag, SnowflakeDescribeResults);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    // Table tag isn't returned from the remote state

    #[test]
    fn from_local_state_none() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            table_tag: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_state_some() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            table_tag: Some("cool_tag = cool value"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_some());
        assert_eq!(loaded.value.unwrap(), "cool_tag = cool value");
    }
}
