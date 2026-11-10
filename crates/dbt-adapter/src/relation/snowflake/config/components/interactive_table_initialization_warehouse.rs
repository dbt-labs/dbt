//! Interactive-table variant of the `snowflake_initialization_warehouse` component.
//!
//! Interactive tables need a different local-config rule from dynamic tables, so this module
//! forks only that half. Everything else — the component type, the `NONE`/`""` unset
//! normalization, the case-insensitive diff and the remote readback — is reused from
//! `snowflake_initialization_warehouse`, which keeps dynamic-table behavior unchanged and
//! keeps the subtle normalization documented in exactly one place.
//!
//! The component reports the same `type_name`, so macros read the same key either way.

use dbt_common::AdapterResult;
use dbt_schemas::schemas::InternalDbtNodeAttributes;

use super::snowflake_attr;
use super::snowflake_initialization_warehouse::{
    SnowflakeInitializationWarehouse, from_remote_state, new_component, normalize_unset,
};
// `impl_loader` only reads `TYPE_NAME` from its `#[cfg(test)]` accessor. Reused rather than
// redeclared so both variants report the same component key.
#[cfg(test)]
use super::snowflake_initialization_warehouse::TYPE_NAME;
use crate::relation::config_v2::{ComponentConfig, ComponentConfigLoader, impl_loader};
use crate::relation::snowflake::config::SnowflakeDescribeResults;

/// An initialization warehouse names the warehouse used for a self-refreshing table's initial
/// build, so it is only a valid property of an interactive table that has a `target_lag`;
/// Snowflake rejects it on a static one. `snowflake_initialization_warehouse` can be set
/// project-wide, so the value is dropped rather than carried into a change that would emit an
/// `ALTER` the warehouse cannot accept — the remote side of a static table always reads back as
/// absent, so that change would otherwise be re-emitted on every run.
fn initialization_warehouse_is_inert(
    target_lag: &Option<String>,
    initialization_warehouse: &Option<String>,
) -> bool {
    target_lag.is_none() && initialization_warehouse.is_some()
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<SnowflakeInitializationWarehouse> {
    let snowflake_config = snowflake_attr(relation_config)?;

    let initialization_warehouse =
        normalize_unset(snowflake_config.snowflake_initialization_warehouse.clone());

    // The accompanying user-facing warning is emitted by `interactive_table_warehouse`, which
    // sees both keys on the same load pass.
    if initialization_warehouse_is_inert(&snowflake_config.target_lag, &initialization_warehouse) {
        return Ok(new_component(None));
    }

    Ok(new_component(initialization_warehouse))
}

impl_loader!(
    InteractiveTableInitializationWarehouse,
    SnowflakeDescribeResults
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    #[test]
    fn inert_initialization_warehouse_is_dropped_so_a_static_table_produces_no_change() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_initialization_warehouse: Some("INIT_WH"),
            target_lag: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).expect("an inert value must not error");
        assert_eq!(loaded.value, None);
    }

    #[test]
    fn initialization_warehouse_is_kept_when_target_lag_present() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_initialization_warehouse: Some("INIT_WH"),
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).expect("a lagged table keeps the value");
        assert_eq!(loaded.value, Some("INIT_WH".to_string()));
    }

    #[test]
    fn unset_normalization_is_still_applied() {
        for sentinel in ["NONE", "none", ""] {
            let local_state =
                test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
                    snowflake_initialization_warehouse: Some(sentinel),
                    target_lag: Some("1 hour"),
                    ..Default::default()
                });
            let loaded = from_local_config(&local_state).expect("sentinels must not error");
            assert_eq!(
                loaded.value, None,
                "sentinel {sentinel:?} must fold to None"
            );
        }
    }

    #[test]
    fn inert_only_without_target_lag() {
        assert!(initialization_warehouse_is_inert(
            &None,
            &Some("INIT_WH".to_string())
        ));
        assert!(!initialization_warehouse_is_inert(&None, &None));
        assert!(!initialization_warehouse_is_inert(
            &Some("1 hour".to_string()),
            &Some("INIT_WH".to_string())
        ));
    }
}
