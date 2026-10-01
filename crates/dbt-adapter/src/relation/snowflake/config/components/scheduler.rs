use dbt_common::{AdapterError, AdapterResult};
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::Value;

use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
};
use crate::relation::snowflake::config::{
    SnowflakeDescribeResults, get_string_by_name_from_record_batch,
};

pub(crate) const TYPE_NAME: &str = "scheduler";

/// Component for Snowflake dynamic table scheduler (ENABLE or DISABLE).
pub(crate) type Scheduler = SimpleComponentConfigImpl<String>;

fn to_jinja(v: &String) -> Value {
    Value::from(v)
}

fn new_component(scheduler: String) -> Scheduler {
    Scheduler {
        type_name: TYPE_NAME,
        diff_fn: diff::desired_state,
        to_jinja_fn: to_jinja,
        value: scheduler,
    }
}

fn from_remote_state(results: &SnowflakeDescribeResults) -> AdapterResult<Scheduler> {
    let batch = &results.record_batch;
    let scheduler = get_string_by_name_from_record_batch(batch, "scheduler")
        .ok()
        .and_then(|s| {
            if s.is_empty() {
                None
            } else {
                Some(s.to_uppercase())
            }
        })
        .unwrap_or_else(|| {
            // Infer from target_lag presence when the scheduler column is absent.
            let has_target_lag = get_string_by_name_from_record_batch(batch, "target_lag")
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            if has_target_lag {
                "ENABLE".to_string()
            } else {
                "DISABLE".to_string()
            }
        });
    Ok(new_component(scheduler))
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<Scheduler> {
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

    // Reference: https://docs.getdbt.com/reference/resource-configs/snowflake-configs?version=2.0#scheduler
    // This is implemented in Core here: https://github.com/dbt-labs/dbt-adapters/blob/cb1b4a0b0758fd307dc21583bb3acfc78397a077/dbt-snowflake/src/dbt/adapters/snowflake/relation_configs/dynamic_table.py#L161-L184
    let scheduler = match snowflake_config.scheduler.as_deref() {
        Some(s) => s.to_uppercase(),
        None if snowflake_config.target_lag.is_some() => "ENABLE".to_string(),
        None => "DISABLE".to_string(),
    };
    Ok(new_component(scheduler))
}

impl_loader!(Scheduler, SnowflakeDescribeResults);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    #[test]
    fn from_remote_state_inferred_enable() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            scheduler: None,
            target_lag: Some("5 hours"),
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert_eq!(loaded.value, "ENABLE");
    }

    #[test]
    fn from_remote_state_inferred_disable() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            scheduler: None,
            target_lag: None,
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert_eq!(loaded.value, "DISABLE");
    }

    #[test]
    fn from_local_state_inferred_enable() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            scheduler: None,
            target_lag: Some("5 hours"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value, "ENABLE");
    }

    #[test]
    fn from_local_state_inferred_disable() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            scheduler: None,
            target_lag: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value, "DISABLE");
    }

    #[test]
    fn from_local_state_some() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            scheduler: Some("ENABLE"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value, "ENABLE");
    }
}
