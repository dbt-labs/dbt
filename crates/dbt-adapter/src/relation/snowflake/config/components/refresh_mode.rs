use dbt_common::{AdapterError, AdapterResult};
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::Value;

use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
};
use crate::relation::snowflake::config::{
    SnowflakeDescribeResults, get_string_by_name_from_record_batch,
};

pub(crate) const TYPE_NAME: &str = "refresh_mode";

/// Component for Snowflake dynamic table refresh mode.
/// Stored as an uppercase string (e.g. "AUTO", "FULL", "INCREMENTAL").
pub(crate) type RefreshMode = SimpleComponentConfigImpl<String>;

fn to_jinja(v: &String) -> Value {
    Value::from(v)
}

// Reference: https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-snowflake/src/dbt/adapters/snowflake/relation.py#L137-L144
fn diff_refresh_mode(desired: &String, current: &String) -> Option<String> {
    if desired.eq_ignore_ascii_case("AUTO") {
        return None;
    }
    diff::desired_state(desired, current)
}

fn new_component(refresh_mode: String) -> RefreshMode {
    RefreshMode {
        type_name: TYPE_NAME,
        diff_fn: diff_refresh_mode,
        to_jinja_fn: to_jinja,
        value: refresh_mode,
    }
}

fn from_remote_state(results: &SnowflakeDescribeResults) -> AdapterResult<RefreshMode> {
    let batch = &results.record_batch;
    let refresh_mode = get_string_by_name_from_record_batch(batch, "refresh_mode")
        .map_err(|e| AdapterError::new(dbt_common::AdapterErrorKind::UnexpectedResult, e))?
        .to_uppercase();
    Ok(new_component(refresh_mode))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<RefreshMode> {
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
    let refresh_mode = match &snowflake_config.refresh_mode {
        None => "AUTO".to_string(),
        Some(s) => s.to_uppercase(),
    };
    Ok(new_component(refresh_mode))
}

impl_loader!(RefreshMode, SnowflakeDescribeResults);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    #[test]
    fn from_remote_state_auto() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            refresh_mode: Some("AUTO"),
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert_eq!(loaded.value, "AUTO");
    }

    #[test]
    fn from_local_state_default_auto() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            refresh_mode: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value, "AUTO");
    }

    #[test]
    fn from_local_state_some() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            refresh_mode: Some("FULL"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value, "FULL");
    }
}
