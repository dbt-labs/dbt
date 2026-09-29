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

pub(crate) const TYPE_NAME: &str = "snowflake_initialization_warehouse";

/// Component for Snowflake dynamic table initialization warehouse
pub(crate) type SnowflakeInitializationWarehouse = SimpleComponentConfigImpl<Option<String>>;

fn to_jinja(v: &Option<String>) -> Value {
    v.as_ref().map(Value::from).unwrap_or_else(none_value)
}

// Warehouse names are case-insensitive identifiers and `SHOW` reports them uppercased, so a
// lowercase name in the model config would otherwise diff forever against its own readback.
// A quoted config value's delimiters are stripped before comparing -- see
// `super::warehouse_names_match`.
fn diff_initialization_warehouse(
    desired: &Option<String>,
    current: &Option<String>,
) -> Option<Option<String>> {
    diff::optional_by(desired, current, super::warehouse_names_match)
}

pub(super) fn new_component(
    initialization_warehouse: Option<String>,
) -> SnowflakeInitializationWarehouse {
    SnowflakeInitializationWarehouse {
        type_name: TYPE_NAME,
        diff_fn: diff_initialization_warehouse,
        to_jinja_fn: to_jinja,
        value: initialization_warehouse,
    }
}

/// `"NONE"`/`""` are Snowflake's readback spellings for *absence*, not values, so both sides
/// fold to `None` at LOAD time — unlike `target_lag`/`cluster_by`, whose exact text must reach
/// the DDL and so normalize only inside their diff fn. `create.sql` omits a Jinja-none clause
/// and `alter.sql` keys its `unset` branch off a falsy `.context`; a stored `"NONE"` breaks both.
pub(super) fn normalize_unset(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("NONE"))
}

pub(super) fn from_remote_state(
    results: &SnowflakeDescribeResults,
) -> AdapterResult<SnowflakeInitializationWarehouse> {
    let batch = &results.record_batch;
    // Some Snowflake accounts don't support the initialization_warehouse field, so this column might not be present.
    let initialization_warehouse =
        get_string_by_name_from_record_batch(batch, "initialization_warehouse").ok();
    Ok(new_component(normalize_unset(initialization_warehouse)))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<SnowflakeInitializationWarehouse> {
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
    Ok(new_component(normalize_unset(
        snowflake_config.snowflake_initialization_warehouse.clone(),
    )))
}

impl_loader!(SnowflakeInitializationWarehouse, SnowflakeDescribeResults);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    #[test]
    fn from_remote_state_some() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            snowflake_initialization_warehouse: Some("warehouse"),
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_some());
        assert_eq!(loaded.value.unwrap(), "warehouse");
    }

    #[test]
    fn from_remote_state_snowflake_none() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            snowflake_initialization_warehouse: Some("NONE"),
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_remote_state_none() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            snowflake_initialization_warehouse: None,
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_state_some() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_initialization_warehouse: Some("warehouse"),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_some());
        assert_eq!(loaded.value.unwrap(), "warehouse");
    }

    #[test]
    fn from_local_state_none() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_initialization_warehouse: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_state_snowflake_none_is_unset() {
        // Normalized on the local side too, so a literal "NONE" in the model config can't
        // diff forever against a remote that reads back as unset.
        for literal in ["NONE", "none", ""] {
            let local_state =
                test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
                    snowflake_initialization_warehouse: Some(literal),
                    ..Default::default()
                });
            let loaded = from_local_config(&local_state).unwrap();
            assert!(loaded.value.is_none(), "{literal:?} should read as unset");
        }
    }

    /// Pins the `unset` branch selected at `alter.sql:40-44`.
    #[test]
    fn clearing_renders_as_jinja_none_so_the_unset_branch_fires() {
        let existing = new_component(Some("WH1".to_owned()));

        // The idiom the golden recording exercises: the config key is removed entirely.
        let cleared_by_omission = new_component(normalize_unset(None));
        let change = ComponentConfig::diff_from(&cleared_by_omission, Some(&existing))
            .expect("clearing must be detected as a change");
        assert!(
            change.to_jinja().is_none(),
            "a cleared warehouse must render as Jinja none to select the `unset` branch"
        );

        let cleared_by_sentinel = new_component(normalize_unset(Some("NONE".to_owned())));
        let change = ComponentConfig::diff_from(&cleared_by_sentinel, Some(&existing))
            .expect("clearing via the NONE sentinel must be detected as a change");
        assert!(change.to_jinja().is_none());
    }

    #[test]
    fn an_unnormalized_sentinel_would_emit_set_none_instead_of_unset() {
        let existing = new_component(Some("WH1".to_owned()));
        let raw_sentinel = new_component(Some("NONE".to_owned()));

        let change = ComponentConfig::diff_from(&raw_sentinel, Some(&existing))
            .expect("a differing value is still a change");
        assert_eq!(
            change.to_jinja().as_str(),
            Some("NONE"),
            "raw sentinel renders truthy, which is exactly the bug load-time normalization avoids"
        );
    }

    #[test]
    fn setting_a_warehouse_renders_truthy_so_the_set_branch_fires() {
        let existing = new_component(normalize_unset(Some("NONE".to_owned())));
        let desired = new_component(normalize_unset(Some("WH1".to_owned())));
        let change = ComponentConfig::diff_from(&desired, Some(&existing))
            .expect("setting a warehouse must be detected as a change");
        assert!(!change.to_jinja().is_none());
        assert_eq!(change.to_jinja().as_str(), Some("WH1"));
    }

    #[test]
    fn case_differing_warehouse_name_is_not_a_change() {
        // Snowflake identifiers are case-insensitive and `SHOW` reports them uppercased, so
        // a model configured with `my_init_wh` reads back as `MY_INIT_WH`.
        let desired = new_component(Some("my_init_wh".to_owned()));
        let existing = new_component(Some("MY_INIT_WH".to_owned()));
        assert!(ComponentConfig::diff_from(&desired, Some(&existing)).is_none());
    }

    #[test]
    fn quoted_config_matches_unquoted_readback_of_the_same_warehouse() {
        // `SHOW` never echoes the quote delimiters a config value can carry -- only the
        // resolved name -- so a quoted config value must not diff forever against its own
        // readback.
        let desired = new_component(Some("\"Init_WH\"".to_owned()));
        let existing = new_component(Some("Init_WH".to_owned()));
        assert!(ComponentConfig::diff_from(&desired, Some(&existing)).is_none());
    }

    #[test]
    fn genuinely_different_warehouse_name_is_still_a_change() {
        let desired = new_component(Some("OTHER_WH".to_owned()));
        let existing = new_component(Some("MY_INIT_WH".to_owned()));
        assert!(ComponentConfig::diff_from(&desired, Some(&existing)).is_some());
    }

    #[test]
    fn unset_and_set_remain_a_change_in_both_directions() {
        let set = new_component(Some("MY_INIT_WH".to_owned()));
        let unset = new_component(None);
        assert!(ComponentConfig::diff_from(&set, Some(&unset)).is_some());
        assert!(ComponentConfig::diff_from(&unset, Some(&set)).is_some());
    }

    #[test]
    fn unset_normalization_and_case_insensitivity_compose() {
        let none_literal = new_component(normalize_unset(Some("NONE".to_owned())));
        let empty = new_component(normalize_unset(Some(String::new())));
        assert!(ComponentConfig::diff_from(&none_literal, Some(&empty)).is_none());

        let real = new_component(normalize_unset(Some("my_init_wh".to_owned())));
        assert!(ComponentConfig::diff_from(&none_literal, Some(&real)).is_some());
        assert!(ComponentConfig::diff_from(&real, Some(&none_literal)).is_some());
    }

    #[test]
    fn local_none_literal_does_not_diff_against_unset_remote() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_initialization_warehouse: Some("NONE"),
            ..Default::default()
        });
        let remote_state = test_helpers::make_remote_state(test_helpers::TestRemoteState {
            initialization_warehouse: Some("NONE".to_owned()),
            ..Default::default()
        });
        let desired = from_local_config(&local_state).unwrap();
        let existing = from_remote_state(&remote_state).unwrap();
        assert!(ComponentConfig::diff_from(&desired, Some(&existing)).is_none());
    }
}
