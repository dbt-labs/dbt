use dbt_common::{AdapterError, AdapterResult};
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::Value;

use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
};
use crate::relation::snowflake::config::SnowflakeDescribeResults;
use crate::value::none_value;

pub(crate) const TYPE_NAME: &str = "refresh_warehouse";

/// Component for the raw `refresh_warehouse` config value the user wrote.
///
/// This component exists for symmetry with the other warehouse fields so custom macros
/// can read `dynamic_table.refresh_warehouse` and branch on whether the user opted in.
///
/// It deliberately does not participate in changeset diffing (`diff_fn = immutable`):
/// the effective `WAREHOUSE =` value lives on the `snowflake_warehouse` component,
/// whose `from_local_config` already folds `refresh_warehouse → snowflake_warehouse`.
/// Letting this component also diff would emit a second, redundant ALTER for the same
/// logical change. Snowflake's DESCRIBE output does not expose a separate
/// `refresh_warehouse` column either — there is no usable current state to diff against.
pub(crate) type RefreshWarehouse = SimpleComponentConfigImpl<Option<String>>;

fn to_jinja(v: &Option<String>) -> Value {
    v.as_ref().map(Value::from).unwrap_or_else(none_value)
}

fn new_component(refresh_warehouse: Option<String>) -> RefreshWarehouse {
    RefreshWarehouse {
        type_name: TYPE_NAME,
        diff_fn: diff::immutable,
        to_jinja_fn: to_jinja,
        value: refresh_warehouse,
    }
}

fn from_remote_state(_results: &SnowflakeDescribeResults) -> AdapterResult<RefreshWarehouse> {
    // Snowflake's DESCRIBE output does not expose `refresh_warehouse` separately;
    // the existing table's `WAREHOUSE =` parameter is reported via the `warehouse`
    // column, which the `snowflake_warehouse` component already reads.
    Ok(new_component(None))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<RefreshWarehouse> {
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
        .expect("relation config needs to be Snowflake model");
    Ok(new_component(snowflake_config.refresh_warehouse.clone()))
}

impl_loader!(RefreshWarehouse, SnowflakeDescribeResults);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    #[test]
    fn from_remote_state_is_always_none() {
        // Whatever the remote state, refresh_warehouse is not reported by DESCRIBE.
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("ANY_WH"),
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_state_none() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("MY_WH"),
            refresh_warehouse: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_state_some() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("MY_LARGE_WH"),
            refresh_warehouse: Some("MY_SMALL_REFRESH_WH"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value.as_deref(), Some("MY_SMALL_REFRESH_WH"));
    }

    #[test]
    fn diff_is_always_none_even_when_changed() {
        // Even if old and new differ, the changeset must not include refresh_warehouse —
        // the effective change rides on the `snowflake_warehouse` component.
        let old = new_component(Some("OLD_WH".into()));
        let new = new_component(Some("NEW_WH".into()));
        assert!(ComponentConfig::diff_from(&new, Some(&old)).is_none());
    }
}
