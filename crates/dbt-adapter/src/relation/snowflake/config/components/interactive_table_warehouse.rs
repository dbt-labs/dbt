//! The gated build/refresh-warehouse component for interactive tables.
//!
//! Also hosts `validate_not_transient` and `should_warn_initialization_warehouse_inert`:
//! `transient` and `snowflake_initialization_warehouse` are shared, unforked components that
//! dynamic tables also use, and this is the only new component always loaded on the
//! interactive path, so it is the one safe place a per-build check always runs.

use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::{AdapterError, AdapterResult, ErrorCode};
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use minijinja::Value;

use super::{non_blank, snowflake_attr};
use crate::relation::snowflake::config::{
    SnowflakeDescribeResults, get_string_by_name_from_record_batch,
};
use crate::{
    relation::config_v2::{
        ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
    },
    value::none_value,
};

pub(crate) const TYPE_NAME: &str = "snowflake_warehouse";

/// Component for the warehouse Snowflake uses to build/refresh an *interactive* table.
///
/// This is deliberately a separate component from `components::snowflake_warehouse`
/// (the dynamic-table one), even though both produce the same user-facing
/// `snowflake_warehouse` key: a dynamic table always requires a warehouse, but an
/// interactive table with no `target_lag` is fully static and needs none at all.
pub(crate) type InteractiveTableWarehouse = SimpleComponentConfigImpl<Option<String>>;

fn to_jinja(v: &Option<String>) -> Value {
    v.as_ref().map(Value::from).unwrap_or_else(none_value)
}

// Warehouse names are case-insensitive, and a quoted config value's delimiters are stripped
// before comparing -- see `super::warehouse_names_match`.
fn diff_warehouse(desired: &Option<String>, current: &Option<String>) -> Option<Option<String>> {
    diff::optional_by(desired, current, super::warehouse_names_match)
}

fn new_component(warehouse: Option<String>) -> InteractiveTableWarehouse {
    InteractiveTableWarehouse {
        type_name: TYPE_NAME,
        diff_fn: diff_warehouse,
        to_jinja_fn: to_jinja,
        value: warehouse,
    }
}

fn from_remote_state(
    results: &SnowflakeDescribeResults,
) -> AdapterResult<InteractiveTableWarehouse> {
    let batch = &results.record_batch;
    // `SHOW INTERACTIVE TABLES` names this column `refresh_warehouse`, not `warehouse` as
    // `SHOW DYNAMIC TABLES` does. A static interactive table (no target_lag) may have no
    // warehouse at all, so NULL/empty is tolerated as unset rather than erroring — which
    // also means a wrong column name here fails silently. See
    // `from_remote_state_ignores_the_dynamic_table_warehouse_column`.
    let warehouse = match get_string_by_name_from_record_batch(batch, "refresh_warehouse") {
        Ok(s) if !s.is_empty() => Some(s),
        _ => None,
    };
    Ok(new_component(warehouse))
}

/// `transient=true` has no valid DDL for interactive tables — `TRANSIENT INTERACTIVE TABLE`
/// is a Snowflake syntax error (001003) — so reject it up front.
fn validate_not_transient(transient: Option<bool>) -> AdapterResult<()> {
    if transient == Some(true) {
        return Err(AdapterError::new(
            dbt_common::AdapterErrorKind::Configuration,
            "Invalid interactive table config: `transient=true` is not supported. \
             `TRANSIENT INTERACTIVE TABLE` is a Snowflake syntax error.",
        ));
    }
    Ok(())
}

/// An interactive table only needs a build/refresh warehouse when it has a `target_lag`
/// (i.e. it self-refreshes on a schedule).
fn required_warehouse_missing(target_lag: &Option<String>, warehouse: &Option<String>) -> bool {
    target_lag.is_some() && warehouse.is_none()
}

/// A static interactive table (no `target_lag`) cannot hold a warehouse, so a project-wide
/// `snowflake_warehouse` is dropped here rather than diffed into a rejected `ALTER`.
///
/// Dropping stays silent on purpose: `snowflake_warehouse` still selects the warehouse the
/// build runs on via the preceding `use warehouse`, so warning "no effect" would be wrong.
fn warehouse_is_inert(target_lag: &Option<String>, warehouse: &Option<String>) -> bool {
    target_lag.is_none() && warehouse.is_some()
}

/// Inert rather than fatal: the value is dropped before any DDL (see
/// `interactive_table_initialization_warehouse`), so warn instead of erroring.
fn should_warn_initialization_warehouse_inert(
    target_lag: &Option<String>,
    initialization_warehouse: &Option<String>,
) -> bool {
    target_lag.is_none() && initialization_warehouse.is_some()
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<InteractiveTableWarehouse> {
    let snowflake_config = snowflake_attr(relation_config)?;

    validate_not_transient(snowflake_config.transient)?;

    let warehouse = non_blank(snowflake_config.refresh_warehouse.clone())
        .or_else(|| non_blank(snowflake_config.snowflake_warehouse.clone()));

    if required_warehouse_missing(&snowflake_config.target_lag, &warehouse) {
        return Err(AdapterError::new(
            dbt_common::AdapterErrorKind::Configuration,
            "Failed to get required field snowflake_warehouse from interactive_table config.",
        ));
    }

    let warehouse = if warehouse_is_inert(&snowflake_config.target_lag, &warehouse) {
        None
    } else {
        warehouse
    };

    if should_warn_initialization_warehouse_inert(
        &snowflake_config.target_lag,
        &snowflake_config.snowflake_initialization_warehouse,
    ) {
        emit_warn_log_message(
            ErrorCode::InvalidConfig,
            "snowflake_initialization_warehouse is ignored on an interactive table without \
             target_lag; it only applies when the table self-refreshes.",
        );
    }

    Ok(new_component(warehouse))
}

impl_loader!(InteractiveTableWarehouse, SnowflakeDescribeResults);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    #[test]
    fn from_remote_state_with_warehouse() {
        let remote_state =
            test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState {
                refresh_warehouse: Some("warehouse".to_owned()),
                ..Default::default()
            });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert_eq!(loaded.value.as_deref(), Some("warehouse"));
    }

    #[test]
    fn from_remote_state_ignores_the_dynamic_table_warehouse_column() {
        // Only the dynamic-table spelling is present, so this must read as unset.
        let remote_state = test_helpers::make_remote_state(test_helpers::TestRemoteState {
            refresh_warehouse: Some("warehouse".to_owned()),
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_remote_state_null_warehouse_is_unset() {
        // Static interactive tables may have no warehouse at all.
        let remote_state =
            test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState::default());
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_remote_state_empty_warehouse_is_unset() {
        let remote_state =
            test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState {
                refresh_warehouse: Some(String::new()),
                ..Default::default()
            });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn gate_does_not_error_when_target_lag_absent_and_warehouse_absent() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: None,
            refresh_warehouse: None,
            target_lag: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn gate_errors_when_target_lag_present_and_warehouse_absent() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: None,
            refresh_warehouse: None,
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let err = from_local_config(&local_state).unwrap_err();
        assert!(err.message().contains("snowflake_warehouse"));
    }

    #[test]
    fn gate_ok_when_target_lag_present_and_warehouse_present() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("MY_EXECUTION_WH"),
            refresh_warehouse: None,
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value.as_deref(), Some("MY_EXECUTION_WH"));
    }

    #[test]
    fn gate_prefers_refresh_warehouse_over_snowflake_warehouse() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("MY_EXECUTION_WH"),
            refresh_warehouse: Some("MY_SMALL_REFRESH_WH"),
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value.as_deref(), Some("MY_SMALL_REFRESH_WH"));
    }

    #[test]
    fn gate_blank_refresh_warehouse_falls_back_to_snowflake_warehouse() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("MY_EXECUTION_WH"),
            refresh_warehouse: Some(""),
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value.as_deref(), Some("MY_EXECUTION_WH"));
    }

    #[test]
    fn gate_whitespace_only_refresh_warehouse_falls_back_to_snowflake_warehouse() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("MY_EXECUTION_WH"),
            refresh_warehouse: Some("   "),
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert_eq!(loaded.value.as_deref(), Some("MY_EXECUTION_WH"));
    }

    #[test]
    fn gate_errors_when_target_lag_present_and_both_warehouses_blank() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some(""),
            refresh_warehouse: Some("  "),
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let err = from_local_config(&local_state).unwrap_err();
        assert!(err.message().contains("snowflake_warehouse"));
    }

    #[test]
    fn transient_true_errors() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            transient: Some(true),
            ..Default::default()
        });
        let err = from_local_config(&local_state).unwrap_err();
        assert!(err.message().contains("transient"));
    }

    #[test]
    fn transient_false_does_not_error() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            transient: Some(false),
            snowflake_warehouse: None,
            target_lag: None,
            ..Default::default()
        });
        assert!(from_local_config(&local_state).is_ok());
    }

    #[test]
    fn diff_case_insensitive_no_change() {
        assert!(diff_warehouse(&Some("wh".to_string()), &Some("WH".to_string())).is_none());
    }

    #[test]
    fn diff_quoted_config_matches_unquoted_readback_of_the_same_warehouse() {
        // `SHOW` never echoes the quote delimiters a config value can carry -- only the
        // resolved name -- so a quoted config value must not diff forever against its own
        // readback.
        assert!(
            diff_warehouse(
                &Some("\"Init_WH\"".to_string()),
                &Some("Init_WH".to_string())
            )
            .is_none()
        );
    }

    #[test]
    fn diff_none_to_none_no_change() {
        assert!(diff_warehouse(&None, &None).is_none());
    }

    #[test]
    fn diff_none_to_some_is_change() {
        let diff = diff_warehouse(&Some("wh".to_string()), &None);
        assert_eq!(diff, Some(Some("wh".to_string())));
    }

    #[test]
    fn diff_some_to_none_is_change() {
        let diff = diff_warehouse(&None, &Some("wh".to_string()));
        assert_eq!(diff, Some(None));
    }

    #[test]
    fn warn_inert_init_warehouse_when_target_lag_absent() {
        assert!(should_warn_initialization_warehouse_inert(
            &None,
            &Some("INIT_WH".to_string())
        ));
    }

    #[test]
    fn no_warn_when_init_warehouse_absent() {
        assert!(!should_warn_initialization_warehouse_inert(&None, &None));
    }

    #[test]
    fn no_warn_when_target_lag_present() {
        assert!(!should_warn_initialization_warehouse_inert(
            &Some("1 hour".to_string()),
            &Some("INIT_WH".to_string())
        ));
    }

    #[test]
    fn inert_warehouse_is_dropped_so_a_static_table_produces_no_change() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).expect("an inert warehouse must not error");
        assert_eq!(loaded.value, None);
        assert!(diff_warehouse(&loaded.value, &None).is_none());
    }

    #[test]
    fn warehouse_is_kept_when_target_lag_present() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).expect("a lagged table keeps its warehouse");
        assert_eq!(loaded.value, Some("WH".to_string()));
    }

    #[test]
    fn warehouse_inert_only_without_target_lag() {
        assert!(warehouse_is_inert(&None, &Some("WH".to_string())));
        assert!(!warehouse_is_inert(&None, &None));
        assert!(!warehouse_is_inert(
            &Some("1 hour".to_string()),
            &Some("WH".to_string())
        ));
    }

    #[test]
    fn from_local_config_does_not_error_when_init_warehouse_inert() {
        // Warn-not-error: an inert `snowflake_initialization_warehouse` must not fail the build.
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_initialization_warehouse: Some("INIT_WH"),
            target_lag: None,
            snowflake_warehouse: None,
            ..Default::default()
        });
        assert!(from_local_config(&local_state).is_ok());
    }
}
