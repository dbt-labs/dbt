use dbt_common::{AdapterError, AdapterResult};
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::Value;

use super::non_blank;
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, impl_loader,
};
use crate::relation::snowflake::config::{
    SnowflakeDescribeResults, get_string_by_name_from_record_batch,
};

pub(crate) const TYPE_NAME: &str = "snowflake_warehouse";

/// Component for the warehouse stamped into a Snowflake dynamic table's `WAREHOUSE =`
/// clause — i.e. the warehouse Snowflake uses to perform the table's self-refresh.
///
/// The local-config loader picks `refresh_warehouse` first and falls back to
/// `snowflake_warehouse`, so the user can split the DDL-execution warehouse from
/// the self-refresh warehouse:
///
/// * `snowflake_warehouse` — still drives DDL execution via the pre-model
///   `USE WAREHOUSE` hook in materialize.rs (unchanged).
/// * `refresh_warehouse` — when set, becomes the value of this component, i.e.
///   what Snowflake stamps into the table's `WAREHOUSE =` clause.
///
/// Reference: <https://github.com/dbt-labs/dbt-adapters/pull/1788>
pub(crate) type SnowflakeWarehouse = SimpleComponentConfigImpl<String>;

fn to_jinja(v: &String) -> Value {
    Value::from(v)
}

// Snowflake warehouse names are case-insensitive, and a quoted config value's delimiters
// are stripped before comparing -- see `super::warehouse_names_match`.
// The type restriction of `diff_fn` requires that this function take in exactly
// &String, &String, so the clippy warning is incorrect.
// The `Option<String>` form of this comparison lives in `config_v2::diff::optional_by`; change
// one, check both.
#[allow(clippy::ptr_arg)]
fn diff_warehouse(desired: &String, current: &String) -> Option<String> {
    if super::warehouse_names_match(desired, current) {
        None
    } else {
        Some(desired.clone())
    }
}

fn new_component(warehouse: String) -> SnowflakeWarehouse {
    SnowflakeWarehouse {
        type_name: TYPE_NAME,
        diff_fn: diff_warehouse,
        to_jinja_fn: to_jinja,
        value: warehouse,
    }
}

fn from_remote_state(results: &SnowflakeDescribeResults) -> AdapterResult<SnowflakeWarehouse> {
    let batch = &results.record_batch;
    let warehouse = get_string_by_name_from_record_batch(batch, "warehouse")
        .map_err(|e| AdapterError::new(dbt_common::AdapterErrorKind::UnexpectedResult, e))?;
    Ok(new_component(warehouse))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<SnowflakeWarehouse> {
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
    let warehouse = non_blank(snowflake_config.refresh_warehouse.clone())
        .or_else(|| non_blank(snowflake_config.snowflake_warehouse.clone()))
        .ok_or_else(|| {
            AdapterError::new(
                dbt_common::AdapterErrorKind::Configuration,
                "Failed to get required field snowflake_warehouse from dynamic_table config.",
            )
        })?;
    Ok(new_component(warehouse))
}

impl_loader!(SnowflakeWarehouse, SnowflakeDescribeResults);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    #[test]
    fn from_remote_state_with_warehouse() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("warehouse"),
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert_eq!(loaded.value, "warehouse");
    }

    #[test]
    fn from_local_state_with_warehouse() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("warehouse"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value, "warehouse");
    }

    #[test]
    fn from_local_state_falls_back_to_snowflake_warehouse() {
        // refresh_warehouse not set → effective WAREHOUSE = is snowflake_warehouse
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("MY_EXECUTION_WH"),
            refresh_warehouse: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value, "MY_EXECUTION_WH");
    }

    #[test]
    fn from_local_state_prefers_refresh_warehouse() {
        // refresh_warehouse set → it becomes the effective WAREHOUSE = value;
        // snowflake_warehouse still drives DDL execution via the pre-model hook
        // (which reads SnowflakeAttr.snowflake_warehouse directly, not this component).
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("MY_EXECUTION_WH"),
            refresh_warehouse: Some("MY_SMALL_REFRESH_WH"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value, "MY_SMALL_REFRESH_WH");
    }

    #[test]
    fn from_local_state_blank_refresh_warehouse_falls_back_to_snowflake_warehouse() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("MY_EXECUTION_WH"),
            refresh_warehouse: Some(""),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value, "MY_EXECUTION_WH");
    }

    #[test]
    fn from_local_state_whitespace_only_refresh_warehouse_falls_back_to_snowflake_warehouse() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("MY_EXECUTION_WH"),
            refresh_warehouse: Some("   "),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value, "MY_EXECUTION_WH");
    }

    #[test]
    fn from_local_state_errors_when_both_warehouses_blank() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some(""),
            refresh_warehouse: Some("  "),
            ..Default::default()
        });
        let err = from_local_config(&local_state).unwrap_err();
        assert!(err.message().contains("snowflake_warehouse"));
    }

    #[test]
    fn diff_case_insensitive_no_change() {
        assert!(diff_warehouse(&"wh".to_string(), &"WH".to_string()).is_none());
    }

    #[test]
    fn diff_quoted_config_matches_unquoted_readback_of_the_same_warehouse() {
        // `SHOW` never echoes the quote delimiters a config value can carry -- only the
        // resolved name -- so a quoted config value must not diff forever against its own
        // readback.
        assert!(diff_warehouse(&"\"Init_WH\"".to_string(), &"Init_WH".to_string()).is_none());
    }
}
