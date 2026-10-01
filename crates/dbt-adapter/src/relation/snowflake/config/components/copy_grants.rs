use dbt_common::{AdapterError, AdapterResult};
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::Value;

use crate::{
    relation::config_v2::{
        ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
    },
    value::none_value,
};

use crate::relation::snowflake::config::SnowflakeDescribeResults;

pub(crate) const TYPE_NAME: &str = "copy_grants";

/// Component for Snowflake dynamic table `copy_grants` setting.
pub(crate) type CopyGrants = SimpleComponentConfigImpl<Option<bool>>;

fn to_jinja(v: &Option<bool>) -> Value {
    v.map(Value::from).unwrap_or_else(none_value)
}

// `COPY GRANTS` only exists as a clause in DDL and is not queryable from the remote state,
// so the state is immutable.
fn new_component(copy_grants: Option<bool>) -> CopyGrants {
    CopyGrants {
        type_name: TYPE_NAME,
        diff_fn: diff::immutable,
        to_jinja_fn: to_jinja,
        value: copy_grants,
    }
}

fn from_remote_state(_results: &SnowflakeDescribeResults) -> AdapterResult<CopyGrants> {
    Ok(new_component(None))
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<CopyGrants> {
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
    Ok(new_component(snowflake_config.copy_grants))
}

impl_loader!(CopyGrants, SnowflakeDescribeResults);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    #[test]
    fn from_remote_state_always_none() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_state_none() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_state_some_copy_grants() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            copy_grants: Some(true),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_some());
        assert!(loaded.value.unwrap());
    }

    #[test]
    fn never_diffs_even_when_both_sides_known() {
        assert!(diff::immutable(&Some(true), &Some(false)).is_none());
    }
}
