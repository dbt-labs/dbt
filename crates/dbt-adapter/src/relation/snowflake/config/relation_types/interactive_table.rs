//! Reduced config-component loader for Snowflake interactive tables.
//!
//! `SHOW INTERACTIVE TABLES` only exposes `cluster_by`, `target_lag`, `refresh_warehouse`,
//! and `initialization_warehouse` — unlike dynamic tables, there is no corresponding column
//! for `scheduler`, `refresh_mode`, `initialize`, `transient`, `immutable_where`,
//! `row_access_policy`, or `table_tag`, so those components are intentionally excluded here.
//! `transient` is instead rejected up front as an invalid config
//! (see `components::interactive_table_warehouse::validate_not_transient`), since
//! `TRANSIENT INTERACTIVE TABLE` has no valid DDL.

use crate::AdapterType;
use crate::relation::config_v2::ComponentConfigChange;
use crate::relation::config_v2::{
    ComponentConfigLoader, RelationConfigLoader, SimpleComponentConfigImpl,
};
use crate::relation::snowflake::config::{SnowflakeDescribeResults, components};
use indexmap::IndexMap;

/// Whether this changeset requires a full rebuild rather than an in-place `ALTER`.
///
/// - `cluster_by`: alterable on dynamic tables, but not on interactive ones.
/// - `target_lag` `Some -> None`: Snowflake rejects `UNSET TARGET_LAG` with `001422`; a static
///   interactive table is a plain `TABLE` and cannot be altered into a dynamic one.
///
/// The `None -> Some` direction is NOT detectable here — the diff carries only the desired
/// value, so a newly-set lag is indistinguishable from a changed one. `target_lag_newly_set`
/// in `relation_impl.rs` handles it, where both `RelationConfig` sides are available.
fn requires_full_refresh(components: &IndexMap<&'static str, ComponentConfigChange>) -> bool {
    if components.contains_key(components::cluster_by::TYPE_NAME) {
        return true;
    }

    if let Some(ComponentConfigChange::Some(change)) =
        components.get(components::target_lag::TYPE_NAME)
        && let Some(target_lag) = change
            .as_any()
            .downcast_ref::<SimpleComponentConfigImpl<Option<String>>>()
        && target_lag.value.is_none()
    {
        return true;
    }

    false
}

/// Create a `RelationConfigLoader` for Snowflake interactive tables.
pub(crate) fn new_loader() -> RelationConfigLoader<'static, SnowflakeDescribeResults> {
    let loaders: [Box<dyn ComponentConfigLoader<SnowflakeDescribeResults>>; 4] = [
        Box::new(components::ClusterByLoader),
        // Scheduler-free variant: interactive tables don't support `scheduler`.
        Box::new(components::TargetLagWithoutSchedulerLoader),
        Box::new(components::InteractiveTableWarehouseLoader),
        Box::new(components::InteractiveTableInitializationWarehouseLoader),
    ];

    RelationConfigLoader::new(AdapterType::Snowflake, loaders, requires_full_refresh)
}

#[cfg(test)]
mod tests {
    use super::requires_full_refresh;
    use crate::relation::config_v2::{ComponentConfigChange, RelationConfig};
    use crate::relation::snowflake::config::{components, test_helpers};
    use dbt_schemas::schemas::common::ClusterConfig;
    use indexmap::IndexMap;

    #[test]
    fn cluster_by_change_triggers_full_refresh() {
        let changes = IndexMap::from_iter([(
            components::cluster_by::TYPE_NAME,
            ComponentConfigChange::Drop,
        )]);
        assert!(requires_full_refresh(&changes));
    }

    #[test]
    fn alterable_changes_do_not_trigger_full_refresh() {
        let changes = IndexMap::from_iter([
            (
                components::target_lag::TYPE_NAME,
                ComponentConfigChange::Drop,
            ),
            (
                components::interactive_table_warehouse::TYPE_NAME,
                ComponentConfigChange::Drop,
            ),
            (
                components::snowflake_initialization_warehouse::TYPE_NAME,
                ComponentConfigChange::Drop,
            ),
        ]);
        assert!(!requires_full_refresh(&changes));
    }

    #[test]
    fn static_interactive_table_with_no_warehouse_loads_cleanly() {
        // A static interactive table (no target_lag) needs no warehouse at all.
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: None,
            target_lag: None,
            ..Default::default()
        });
        let desired = loader.from_local_config(&local).unwrap();

        let remote =
            test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState::default());
        let existing = loader.from_remote_state(&remote).unwrap();

        let changes = RelationConfig::diff(&desired, &existing);
        assert!(matches!(
            changes.get(components::interactive_table_warehouse::TYPE_NAME),
            ComponentConfigChange::None
        ));
    }

    #[test]
    fn target_lag_removed_triggers_full_refresh() {
        // dynamic -> static: Snowflake rejects ALTER-ing target_lag away (001422); this
        // direction is detectable from the changeset alone (see `requires_full_refresh`).
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            target_lag: None,
            snowflake_warehouse: None,
            ..Default::default()
        });
        let desired = loader.from_local_config(&local).unwrap();

        let remote = test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState {
            target_lag: Some("1 hour".to_owned()),
            refresh_warehouse: Some("WH".to_owned()),
            ..Default::default()
        });
        let existing = loader.from_remote_state(&remote).unwrap();

        let changes = RelationConfig::diff(&desired, &existing);
        assert!(changes.requires_full_refresh());
    }

    #[test]
    fn target_lag_value_change_does_not_trigger_full_refresh() {
        // Both sides dynamic, value only: an in-place `ALTER SET TARGET_LAG` — the whole
        // reason this diff-then-alter path exists rather than always rebuilding.
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            target_lag: Some("2 hours"),
            snowflake_warehouse: Some("WH"),
            ..Default::default()
        });
        let desired = loader.from_local_config(&local).unwrap();

        let remote = test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState {
            target_lag: Some("1 hour".to_owned()),
            refresh_warehouse: Some("WH".to_owned()),
            ..Default::default()
        });
        let existing = loader.from_remote_state(&remote).unwrap();

        let changes = RelationConfig::diff(&desired, &existing);
        assert!(!changes.requires_full_refresh());
    }

    #[test]
    fn target_lag_added_does_not_trigger_full_refresh_here_by_design() {
        // Not detectable from the changeset alone — see `requires_full_refresh`; the changeset
        // builder forces the refresh instead.
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            target_lag: Some("1 hour"),
            snowflake_warehouse: Some("WH"),
            ..Default::default()
        });
        let desired = loader.from_local_config(&local).unwrap();

        let remote =
            test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState::default());
        let existing = loader.from_remote_state(&remote).unwrap();

        let changes = RelationConfig::diff(&desired, &existing);
        assert!(!changes.requires_full_refresh());
    }

    /// Reading `warehouse` instead of `refresh_warehouse` yields `None` silently,
    /// phantom-diffing the warehouse away.
    #[test]
    fn refresh_warehouse_readback_column_is_read() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let remote = test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState {
            refresh_warehouse: Some("WH".to_owned()),
            target_lag: Some("1 hour".to_owned()),
            ..Default::default()
        });

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        assert!(matches!(
            changes.get(components::interactive_table_warehouse::TYPE_NAME),
            ComponentConfigChange::None
        ));
    }

    #[test]
    fn normalized_target_lag_readback_is_not_a_change() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: Some("60 seconds"),
            ..Default::default()
        });
        let remote = test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState {
            refresh_warehouse: Some("WH".to_owned()),
            target_lag: Some("1 minute".to_owned()),
            ..Default::default()
        });

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        assert!(matches!(
            changes.get(components::target_lag::TYPE_NAME),
            ComponentConfigChange::None
        ));
        assert!(!changes.requires_full_refresh());
    }

    #[test]
    fn downstream_target_lag_readback_is_not_a_change() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: Some("DOWNSTREAM"),
            ..Default::default()
        });
        let remote = test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState {
            refresh_warehouse: Some("WH".to_owned()),
            target_lag: Some("DOWNSTREAM".to_owned()),
            ..Default::default()
        });

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        assert!(matches!(
            changes.get(components::target_lag::TYPE_NAME),
            ComponentConfigChange::None
        ));
    }

    /// `requires_full_refresh` fires on any `cluster_by` change, so a phantom `cluster_by`
    /// diff rebuilds the entire table on every no-op run.
    #[test]
    fn parenthesized_cluster_by_readback_does_not_force_full_refresh() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: Some("1 hour"),
            cluster_by: Some(ClusterConfig::List(vec!["id".to_owned(), "val".to_owned()])),
            ..Default::default()
        });
        let remote = test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState {
            refresh_warehouse: Some("WH".to_owned()),
            target_lag: Some("1 hour".to_owned()),
            cluster_by: Some("(id, val)".to_owned()),
            ..Default::default()
        });

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        assert!(matches!(
            changes.get(components::cluster_by::TYPE_NAME),
            ComponentConfigChange::None
        ));
        assert!(!changes.requires_full_refresh());
    }

    /// Pins that this relation type wires the interactive-specific warehouse loaders, which
    /// drop values a static table cannot hold.
    #[test]
    fn static_table_drops_both_warehouse_keys_so_no_change_is_produced() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            snowflake_initialization_warehouse: Some("INIT_WH"),
            target_lag: None,
            cluster_by: Some(ClusterConfig::String("id".to_owned())),
            ..Default::default()
        });
        let remote = test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState {
            cluster_by: Some("(id)".to_owned()),
            ..Default::default()
        });

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        assert!(
            matches!(
                changes.get(components::interactive_table_warehouse::TYPE_NAME),
                ComponentConfigChange::None
            ),
            "an inert warehouse must not reach the changeset"
        );
        assert!(
            matches!(
                changes.get(components::snowflake_initialization_warehouse::TYPE_NAME),
                ComponentConfigChange::None
            ),
            "an inert initialization warehouse must not reach the changeset"
        );
        assert!(!changes.requires_full_refresh());
    }

    #[test]
    fn single_column_parenthesized_cluster_by_readback_is_not_a_change() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: Some("1 hour"),
            cluster_by: Some(ClusterConfig::String("id".to_owned())),
            ..Default::default()
        });
        let remote = test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState {
            refresh_warehouse: Some("WH".to_owned()),
            target_lag: Some("1 hour".to_owned()),
            cluster_by: Some("(id)".to_owned()),
            ..Default::default()
        });

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        assert!(matches!(
            changes.get(components::cluster_by::TYPE_NAME),
            ComponentConfigChange::None
        ));
    }

    #[test]
    fn genuine_cluster_by_change_still_forces_full_refresh() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: Some("1 hour"),
            cluster_by: Some(ClusterConfig::List(vec![
                "id".to_owned(),
                "other".to_owned(),
            ])),
            ..Default::default()
        });
        let remote = test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState {
            refresh_warehouse: Some("WH".to_owned()),
            target_lag: Some("1 hour".to_owned()),
            cluster_by: Some("(id, val)".to_owned()),
            ..Default::default()
        });

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        assert!(matches!(
            changes.get(components::cluster_by::TYPE_NAME),
            ComponentConfigChange::Some(_)
        ));
        assert!(changes.requires_full_refresh());
    }

    #[test]
    fn case_differing_initialization_warehouse_readback_is_not_a_change() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: Some("1 hour"),
            snowflake_initialization_warehouse: Some("my_init_wh"),
            ..Default::default()
        });
        let remote = test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState {
            refresh_warehouse: Some("WH".to_owned()),
            target_lag: Some("1 hour".to_owned()),
            initialization_warehouse: Some("MY_INIT_WH".to_owned()),
            ..Default::default()
        });

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        assert!(matches!(
            changes.get(components::snowflake_initialization_warehouse::TYPE_NAME),
            ComponentConfigChange::None
        ));
        assert!(!changes.requires_full_refresh());
    }

    #[test]
    fn genuine_initialization_warehouse_change_is_still_detected() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: Some("1 hour"),
            snowflake_initialization_warehouse: Some("other_init_wh"),
            ..Default::default()
        });
        let remote = test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState {
            refresh_warehouse: Some("WH".to_owned()),
            target_lag: Some("1 hour".to_owned()),
            initialization_warehouse: Some("MY_INIT_WH".to_owned()),
            ..Default::default()
        });

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        assert!(matches!(
            changes.get(components::snowflake_initialization_warehouse::TYPE_NAME),
            ComponentConfigChange::Some(_)
        ));
    }

    /// Static interactive tables report NULL for every refresh-related column. All three must
    /// read as "unset", and none may produce a diff against an equally-unset local config.
    #[test]
    fn static_interactive_table_null_readback_columns_are_unset() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: None,
            target_lag: None,
            ..Default::default()
        });
        let remote =
            test_helpers::make_remote_interactive_state(test_helpers::TestRemoteState::default());

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        for type_name in [
            components::target_lag::TYPE_NAME,
            components::interactive_table_warehouse::TYPE_NAME,
            components::snowflake_initialization_warehouse::TYPE_NAME,
        ] {
            assert!(
                matches!(changes.get(type_name), ComponentConfigChange::None),
                "{type_name} phantom-diffed on an all-NULL static readback"
            );
        }
        assert!(!changes.requires_full_refresh());
    }

    /// `scheduler` is a dynamic-table-only key and is deliberately not in the interactive
    /// component set, so the dynamic-table `scheduler` <-> `target_lag` cross-validation must
    /// not reject an interactive-table config.
    #[test]
    fn scheduler_enable_without_target_lag_does_not_error() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            scheduler: Some("ENABLE"),
            target_lag: None,
            snowflake_warehouse: None,
            ..Default::default()
        });
        assert!(loader.from_local_config(&local).is_ok());
    }

    #[test]
    fn scheduler_disable_with_target_lag_does_not_error() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            scheduler: Some("DISABLE"),
            target_lag: Some("1 hour"),
            snowflake_warehouse: Some("WH"),
            ..Default::default()
        });
        assert!(loader.from_local_config(&local).is_ok());
    }

    #[test]
    fn refreshing_interactive_table_requires_warehouse() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: None,
            target_lag: Some("1 hour"),
            ..Default::default()
        });
        let err = loader.from_local_config(&local).unwrap_err();
        assert!(err.message().contains("snowflake_warehouse"));
    }
}
