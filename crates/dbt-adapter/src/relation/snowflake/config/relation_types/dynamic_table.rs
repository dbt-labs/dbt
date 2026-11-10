//! https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-snowflake/src/dbt/adapters/snowflake/relation_configs/dynamic_table.py

use crate::AdapterType;
use crate::relation::config_v2::ComponentConfigChange;
use crate::relation::config_v2::{ComponentConfigLoader, RelationConfigLoader};
use crate::relation::snowflake::config::{SnowflakeDescribeResults, components};
use indexmap::IndexMap;

fn requires_full_refresh(components: &IndexMap<&'static str, ComponentConfigChange>) -> bool {
    const REFRESH_ON: [&str; 2] = [
        components::transient::TYPE_NAME,
        components::refresh_mode::TYPE_NAME,
    ];
    REFRESH_ON.iter().any(|k| components.contains_key(k))
}

/// Create a `RelationConfigLoader` for Snowflake dynamic tables.
pub(crate) fn new_loader() -> RelationConfigLoader<'static, SnowflakeDescribeResults> {
    let loaders: [Box<dyn ComponentConfigLoader<SnowflakeDescribeResults>>; 13] = [
        Box::new(components::ClusterByLoader),
        Box::new(components::CopyGrantsLoader),
        Box::new(components::ImmutableWhereLoader),
        Box::new(components::InitializeLoader),
        Box::new(components::RefreshModeLoader),
        Box::new(components::RefreshWarehouseLoader),
        Box::new(components::RowAccessPolicyLoader),
        Box::new(components::SchedulerLoader),
        Box::new(components::SnowflakeInitializationWarehouseLoader),
        Box::new(components::SnowflakeWarehouseLoader),
        Box::new(components::TableTagLoader),
        Box::new(components::TargetLagLoader),
        Box::new(components::TransientLoader),
    ];

    RelationConfigLoader::new(AdapterType::Snowflake, loaders, requires_full_refresh)
}

#[cfg(test)]
mod tests {
    use super::requires_full_refresh;
    use crate::relation::config_v2::{
        ComponentConfigChange, RelationConfig, SimpleComponentConfigImpl,
    };
    use crate::relation::snowflake::config::{components, test_helpers};
    use dbt_schemas::schemas::common::ClusterConfig;
    use indexmap::IndexMap;

    #[test]
    fn transient_change_triggers_full_refresh() {
        let changes = IndexMap::from_iter([(
            components::transient::TYPE_NAME,
            ComponentConfigChange::Drop,
        )]);
        assert!(requires_full_refresh(&changes));
    }

    #[test]
    fn refresh_mode_change_triggers_full_refresh() {
        let changes = IndexMap::from_iter([(
            components::refresh_mode::TYPE_NAME,
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
                components::snowflake_warehouse::TYPE_NAME,
                ComponentConfigChange::Drop,
            ),
            (
                components::snowflake_initialization_warehouse::TYPE_NAME,
                ComponentConfigChange::Drop,
            ),
            (
                components::scheduler::TYPE_NAME,
                ComponentConfigChange::Drop,
            ),
            (
                components::immutable_where::TYPE_NAME,
                ComponentConfigChange::Drop,
            ),
            (
                components::cluster_by::TYPE_NAME,
                ComponentConfigChange::Drop,
            ),
            (
                components::refresh_warehouse::TYPE_NAME,
                ComponentConfigChange::Drop,
            ),
        ]);
        assert!(!requires_full_refresh(&changes));
    }

    #[test]
    fn refresh_warehouse_differs_from_existing_triggers_snowflake_warehouse_change() {
        let loader = super::new_loader();
        // local: split — DDL on EXEC_WH, self-refresh on REFRESH_WH
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("EXEC_WH"),
            refresh_warehouse: Some("REFRESH_WH"),
            target_lag: Some("1 hour"),
            initialize: "on_create",
            ..Default::default()
        });
        // remote: existing dynamic table still has WAREHOUSE = EXEC_WH
        let remote = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("EXEC_WH"),
            target_lag: Some("1 hour"),
            ..Default::default()
        });

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        // (1) snowflake_warehouse change carries the refresh warehouse as desired
        let change = changes.get(components::snowflake_warehouse::TYPE_NAME);
        let cfg = match change {
            ComponentConfigChange::Some(c) => c,
            other => panic!(
                "expected snowflake_warehouse component change, got {:?}",
                other
            ),
        };
        let desired_value: &SimpleComponentConfigImpl<String> = cfg
            .as_any()
            .downcast_ref()
            .expect("snowflake_warehouse component should be SimpleComponentConfigImpl<String>");
        assert_eq!(desired_value.value, "REFRESH_WH");

        // (2) refresh_warehouse component is absent from the changeset
        assert!(matches!(
            changes.get(components::refresh_warehouse::TYPE_NAME),
            ComponentConfigChange::None
        ));
    }

    #[test]
    fn refresh_warehouse_matches_existing_no_change() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("EXEC_WH"),
            refresh_warehouse: Some("REFRESH_WH"),
            target_lag: Some("1 hour"),
            initialize: "on_create",
            ..Default::default()
        });
        // remote already on REFRESH_WH — matches desired effective WAREHOUSE
        let remote = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("REFRESH_WH"),
            target_lag: Some("1 hour"),
            ..Default::default()
        });

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        assert!(matches!(
            changes.get(components::snowflake_warehouse::TYPE_NAME),
            ComponentConfigChange::None
        ));
        assert!(matches!(
            changes.get(components::refresh_warehouse::TYPE_NAME),
            ComponentConfigChange::None
        ));
    }

    /// Snowflake normalizes `target_lag` units on readback, so a dynamic table configured
    /// with `'60 seconds'` reads back as `'1 minute'`. Comparing raw strings emitted a no-op
    /// `ALTER ... SET TARGET_LAG` on every single run.
    #[test]
    fn normalized_target_lag_readback_is_not_a_change() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: Some("60 seconds"),
            initialize: "on_create",
            ..Default::default()
        });
        let remote = test_helpers::make_remote_state(test_helpers::TestRemoteState {
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
    }

    #[test]
    fn genuine_target_lag_change_is_still_detected() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: Some("120 seconds"),
            initialize: "on_create",
            ..Default::default()
        });
        let remote = test_helpers::make_remote_state(test_helpers::TestRemoteState {
            refresh_warehouse: Some("WH".to_owned()),
            target_lag: Some("1 minute".to_owned()),
            ..Default::default()
        });

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        assert!(matches!(
            changes.get(components::target_lag::TYPE_NAME),
            ComponentConfigChange::Some(_)
        ));
    }

    /// `SHOW` reports `cluster_by` parenthesized, so a dynamic table configured with
    /// `["id", "val"]` reads back as `(id, val)`.
    #[test]
    fn parenthesized_cluster_by_readback_is_not_a_change() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            cluster_by: Some(ClusterConfig::List(vec!["id".to_owned(), "val".to_owned()])),
            initialize: "on_create",
            ..Default::default()
        });
        let remote = test_helpers::make_remote_state(test_helpers::TestRemoteState {
            refresh_warehouse: Some("WH".to_owned()),
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
    }

    #[test]
    fn genuine_cluster_by_change_is_still_detected() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            cluster_by: Some(ClusterConfig::List(vec![
                "id".to_owned(),
                "other".to_owned(),
            ])),
            initialize: "on_create",
            ..Default::default()
        });
        let remote = test_helpers::make_remote_state(test_helpers::TestRemoteState {
            refresh_warehouse: Some("WH".to_owned()),
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
    }

    /// Warehouse names are case-insensitive identifiers and `SHOW` reports them uppercased,
    /// so a lowercase `snowflake_initialization_warehouse` in the model config must not read
    /// as a change against the uppercased readback.
    #[test]
    fn case_differing_initialization_warehouse_readback_is_not_a_change() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: Some("1 hour"),
            snowflake_initialization_warehouse: Some("my_init_wh"),
            initialize: "on_create",
            ..Default::default()
        });
        let remote = test_helpers::make_remote_state(test_helpers::TestRemoteState {
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
    }

    #[test]
    fn genuine_initialization_warehouse_change_is_still_detected() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            target_lag: Some("1 hour"),
            snowflake_initialization_warehouse: Some("other_init_wh"),
            initialize: "on_create",
            ..Default::default()
        });
        let remote = test_helpers::make_remote_state(test_helpers::TestRemoteState {
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

    /// Clustering key order is part of the clustering definition, so reordering is a real
    /// change even though the key *set* is identical.
    #[test]
    fn reordered_cluster_by_is_a_change() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("WH"),
            cluster_by: Some(ClusterConfig::List(vec!["val".to_owned(), "id".to_owned()])),
            initialize: "on_create",
            ..Default::default()
        });
        let remote = test_helpers::make_remote_state(test_helpers::TestRemoteState {
            refresh_warehouse: Some("WH".to_owned()),
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
    }

    #[test]
    fn refresh_warehouse_case_insensitive_no_change() {
        let loader = super::new_loader();
        let local = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("EXEC_WH"),
            refresh_warehouse: Some("refresh_wh"), // lowercase desired
            target_lag: Some("1 hour"),
            initialize: "on_create",
            ..Default::default()
        });
        let remote = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            snowflake_warehouse: Some("REFRESH_WH"), // uppercase from Snowflake
            target_lag: Some("1 hour"),
            ..Default::default()
        });

        let desired = loader.from_local_config(&local).unwrap();
        let existing = loader.from_remote_state(&remote).unwrap();
        let changes = RelationConfig::diff(&desired, &existing);

        assert!(matches!(
            changes.get(components::snowflake_warehouse::TYPE_NAME),
            ComponentConfigChange::None
        ));
        assert!(matches!(
            changes.get(components::refresh_warehouse::TYPE_NAME),
            ComponentConfigChange::None
        ));
    }
}
