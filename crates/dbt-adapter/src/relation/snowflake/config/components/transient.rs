use dbt_common::{AdapterError, AdapterResult};
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::Value;

use crate::relation::snowflake::config::{
    SnowflakeDescribeResults, get_bool_by_name_from_record_batch,
};
use crate::{
    relation::config_v2::{
        ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, impl_loader,
    },
    value::none_value,
};

pub(crate) const TYPE_NAME: &str = "transient";

/// Component for Snowflake dynamic table transient setting.
pub(crate) type Transient = SimpleComponentConfigImpl<Option<bool>>;

fn to_jinja(v: &Option<bool>) -> Value {
    v.map(Value::from).unwrap_or_else(none_value)
}

// Transient is only compared when both sides are explicitly known:
// - new is None when the user omitted transient from their config ("don't care")
// - existing is None when describe_dynamic_table was called without include_transient
fn diff_transient(desired: &Option<bool>, current: &Option<bool>) -> Option<Option<bool>> {
    match (current, desired) {
        (Some(c), Some(d)) if c != d => Some(*desired),
        _ => None,
    }
}

fn new_component(transient: Option<bool>) -> Transient {
    Transient {
        type_name: TYPE_NAME,
        diff_fn: diff_transient,
        to_jinja_fn: to_jinja,
        value: transient,
    }
}

fn from_remote_state(results: &SnowflakeDescribeResults) -> AdapterResult<Transient> {
    let batch = &results.record_batch;
    Ok(new_component(get_bool_by_name_from_record_batch(
        batch,
        "transient",
    )))
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<Transient> {
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
    Ok(new_component(snowflake_config.transient))
}

impl_loader!(Transient, SnowflakeDescribeResults);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    // A table is always either transient or not transient, but if we can't get
    // information from both remote and local we can't perform a diff.
    #[test]
    fn from_remote_state_none() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            transient: None,
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_remote_state_some_transient() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            transient: Some(true),
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_some());
        assert!(loaded.value.unwrap());
    }

    #[test]
    fn from_local_state_none() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            transient: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_state_some_not_transient() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            transient: Some(false),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_some());
        assert!(!loaded.value.unwrap());
    }
}
