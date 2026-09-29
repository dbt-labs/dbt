use dbt_common::{AdapterError, AdapterResult};
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::Value;

use crate::relation::snowflake::config::{
    SnowflakeDescribeResults, get_string_by_name_from_record_batch,
};
use crate::{
    relation::config_v2::{
        ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
    },
    value::none_value,
};

pub(crate) const TYPE_NAME: &str = "immutable_where";

/// Component for Snowflake dynamic table immutable where clause.
pub(crate) type ImmutableWhere = SimpleComponentConfigImpl<Option<String>>;

fn to_jinja(v: &Option<String>) -> Value {
    v.as_ref().map(Value::from).unwrap_or_else(none_value)
}

fn new_component(immutable_where: Option<String>) -> ImmutableWhere {
    ImmutableWhere {
        type_name: TYPE_NAME,
        diff_fn: diff::desired_state,
        to_jinja_fn: to_jinja,
        value: immutable_where,
    }
}

fn from_remote_state(results: &SnowflakeDescribeResults) -> AdapterResult<ImmutableWhere> {
    let batch = &results.record_batch;
    // Snowflake returns "IMMUTABLE WHERE (expr)" — strip prefix/suffix to get bare expression.
    let immutable_where = get_string_by_name_from_record_batch(batch, "immutable_where")
        .ok()
        .and_then(|v| {
            let v = v.trim().to_string();
            if v.is_empty() {
                None
            } else {
                let stripped = v
                    .strip_prefix("IMMUTABLE WHERE (")
                    .and_then(|s| s.strip_suffix(')'))
                    .map(|s| s.to_string())
                    .unwrap_or(v);
                Some(stripped)
            }
        });
    Ok(new_component(immutable_where))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<ImmutableWhere> {
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
    Ok(new_component(snowflake_config.immutable_where.clone()))
}

impl_loader!(ImmutableWhere, SnowflakeDescribeResults);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    #[test]
    fn from_remote_state_none() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            immutable_where: None,
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_remote_state_some() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            immutable_where: Some("IMMUTABLE WHERE (id < 100)"),
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_some());
        assert_eq!(loaded.value.unwrap(), "id < 100")
    }

    #[test]
    fn from_local_state_none() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            immutable_where: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_state_some() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            immutable_where: Some("id < 100"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_some());
        assert_eq!(loaded.value.unwrap(), "id < 100")
    }
}
