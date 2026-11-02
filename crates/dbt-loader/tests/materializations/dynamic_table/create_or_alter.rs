//! `snowflake_dynamic_table_create_or_alter`: with the gate on, an existing native dynamic table
//! syncs its definition in place via `create or alter`, so SQL edits deploy without --full-refresh.

use super::*;

fn create_or_alter_options() -> FixtureOptions {
    FixtureOptions {
        create_or_alter: true,
        ..Default::default()
    }
}

/// The `create or alter` DDL for [`base_local_config`] (refreshing, native, no extra attributes).
fn base_create_or_alter_sql() -> String {
    format!(
        "create or alter dynamic table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH refresh_mode = AUTO scheduler = 'ENABLE' as ( {MODEL_SQL} )"
    )
}

#[test]
fn create_or_alter_gate_off_keeps_sql_only_edit_a_noop() {
    // Control: with the gate off an unchanged config is a skip, whatever the model SQL says.
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        FixtureOptions::default(),
    );
    render_ok(&fixture, &base_local_config(), false, "apply");

    assert!(
        fixture.executed().is_empty(),
        "gate off must not build, got: {:?}",
        fixture.executed()
    );
    assert_eq!(fixture.raw_results(), vec!["main=skip".to_string()]);
}

#[test]
fn create_or_alter_gate_off_still_alters_config_changes() {
    // The legacy path is untouched when the gate is off (e.g. Iceberg, or the flag disabled).
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            target_lag: Some("1 minute".to_string()),
            warehouse: Some("WH".to_string()),
            ..Default::default()
        },
        FixtureOptions::default(),
    );
    let local = LocalConfig {
        target_lag: Some("2 minutes".to_string()),
        snowflake_warehouse: Some("WH".to_string()),
        ..Default::default()
    };
    render_ok(&fixture, &local, false, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "alter dynamic table {RENDERED_RELATION} set target_lag = '2 minutes'"
        )]
    );
}

#[test]
fn create_or_alter_deploys_the_definition_when_nothing_else_changed() {
    // The changeset cannot see the query, so "no config changes" means "query-only edit (or no
    // change)": the definition is still synced in place rather than skipped.
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        create_or_alter_options(),
    );
    render_ok(&fixture, &base_local_config(), false, "apply");

    assert_eq!(fixture.executed(), vec![base_create_or_alter_sql()]);
    assert!(
        fixture.raw_results().is_empty(),
        "an in-place sync is a real build, not a skip: {:?}",
        fixture.raw_results()
    );
    assert_eq!(fixture.describe_calls(), 1);
}

/// The text after `as` of a rendered statement, whitespace included.
fn query_block(sql: &str) -> String {
    let start = sql.find("as (").expect("statement has a query block");
    sql[start..].trim_end().to_string()
}

#[test]
fn create_or_alter_query_text_matches_create_and_replace_byte_for_byte() {
    // Snowflake compares the stored query text literally, whitespace included. If the text sent by
    // CREATE OR ALTER differed from the one CREATE / CREATE OR REPLACE stored, the first sync
    // would look like a definition change and reinitialize an incremental dynamic table.
    let create = fixture_with(None, RemoteState::default(), create_or_alter_options());
    render_ok(&create, &base_local_config(), false, "apply");

    let replace = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        create_or_alter_options(),
    );
    render_ok(&replace, &base_local_config(), true, "apply");

    let sync = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        create_or_alter_options(),
    );
    render_ok(&sync, &base_local_config(), false, "apply");

    let create_block = query_block(&create.executed_raw()[0]);
    assert_eq!(query_block(&replace.executed_raw()[0]), create_block);
    assert_eq!(query_block(&sync.executed_raw()[0]), create_block);
}

#[test]
fn create_or_alter_does_not_apply_on_first_build_or_full_refresh() {
    // No existing relation: plain create. --full-refresh: create or replace.
    let first = fixture_with(None, RemoteState::default(), create_or_alter_options());
    render_ok(&first, &base_local_config(), false, "apply");
    assert!(first.executed()[0].starts_with(&format!("create dynamic table {RENDERED_RELATION}")));

    let refresh = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        create_or_alter_options(),
    );
    render_ok(&refresh, &base_local_config(), true, "apply");
    assert!(
        refresh.executed()[0].starts_with(&format!(
            "create or replace dynamic table {RENDERED_RELATION}"
        )),
        "got: {:?}",
        refresh.executed()
    );
    assert_eq!(refresh.describe_calls(), 0);
}

#[test]
fn create_or_alter_transient_change_routes_to_replace() {
    // transient cannot be converted by CREATE OR ALTER: the requires_full_refresh veto rebuilds.
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            transient: Some(true),
            ..matching_remote_state()
        },
        create_or_alter_options(),
    );
    let local = LocalConfig {
        transient: Some(false),
        ..base_local_config()
    };
    render_ok(&fixture, &local, false, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create or replace dynamic table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH refresh_mode = AUTO initialize = ON_CREATE scheduler = 'ENABLE' as ( {MODEL_SQL} )"
        )]
    );
}

#[test]
fn create_or_alter_unchanged_transient_is_preserved_in_place() {
    // A query-only edit on a transient table must not route to replace, and the table must stay
    // transient: the DDL re-declares `transient`.
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            transient: Some(true),
            ..matching_remote_state()
        },
        create_or_alter_options(),
    );
    let local = LocalConfig {
        transient: Some(true),
        ..base_local_config()
    };
    render_ok(&fixture, &local, false, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create or alter transient dynamic table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH refresh_mode = AUTO scheduler = 'ENABLE' as ( {MODEL_SQL} )"
        )]
    );
}

#[test]
fn create_or_alter_refresh_mode_change_routes_to_replace() {
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            refresh_mode: Some("INCREMENTAL".to_string()),
            ..matching_remote_state()
        },
        create_or_alter_options(),
    );
    let local = LocalConfig {
        refresh_mode: Some("FULL".to_string()),
        ..base_local_config()
    };
    render_ok(&fixture, &local, false, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create or replace dynamic table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH refresh_mode = FULL initialize = ON_CREATE scheduler = 'ENABLE' as ( {MODEL_SQL} )"
        )]
    );
}

#[test]
fn create_or_alter_refresh_mode_change_to_auto_is_undetected_and_applied_in_place() {
    // The changeset ignores changes *to* AUTO, so this takes the in-place path (Snowflake resolves
    // AUTO to a concrete mode) rather than the replace route.
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            refresh_mode: Some("INCREMENTAL".to_string()),
            ..matching_remote_state()
        },
        create_or_alter_options(),
    );
    let local = LocalConfig {
        refresh_mode: Some("AUTO".to_string()),
        ..base_local_config()
    };
    render_ok(&fixture, &local, false, "apply");

    assert_eq!(fixture.executed(), vec![base_create_or_alter_sql()]);
}

#[test]
fn create_or_alter_transient_default_drift_is_undetected_and_redeclared() {
    // Known limitation: `transient` unset in config is not compared, so the effective default
    // (here the behavior flag, off) is re-declared against a live transient table -- Snowflake
    // rejects the flip at run time; the recovery is --full-refresh.
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            transient: Some(true),
            ..matching_remote_state()
        },
        create_or_alter_options(),
    );
    render_ok(&fixture, &base_local_config(), false, "apply");

    assert_eq!(fixture.executed(), vec![base_create_or_alter_sql()]);
}

#[test]
fn create_or_alter_applies_a_config_change_together_with_the_query() {
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        create_or_alter_options(),
    );
    let local = LocalConfig {
        cluster_by: Some(dbt_schemas::schemas::common::ClusterConfig::List(vec![
            "id".to_string(),
        ])),
        ..base_local_config()
    };
    render_ok(&fixture, &local, false, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create or alter dynamic table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH refresh_mode = AUTO scheduler = 'ENABLE' cluster by (id) as ( {MODEL_SQL} )"
        )]
    );
}

#[test]
fn create_or_alter_on_configuration_change_fail_raises_on_a_config_change() {
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        create_or_alter_options(),
    );
    let local = LocalConfig {
        cluster_by: Some(dbt_schemas::schemas::common::ClusterConfig::List(vec![
            "id".to_string(),
        ])),
        ..base_local_config()
    };
    let result = render(&fixture, &local, false, "fail");

    assert!(result.is_err(), "`fail` must raise, got: {result:?}");
    assert!(fixture.executed().is_empty());
}

#[test]
fn create_or_alter_on_configuration_change_fail_does_not_block_a_query_only_edit() {
    // `fail` is only about detected config changes: with none, the definition still deploys.
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        create_or_alter_options(),
    );
    render_ok(&fixture, &base_local_config(), false, "fail");

    assert_eq!(fixture.executed(), vec![base_create_or_alter_sql()]);
}

#[test]
fn create_or_alter_on_configuration_change_continue_skips_config_change_and_query() {
    // Known limitation: a run carrying a tracked config change AND a SQL edit skips both.
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        create_or_alter_options(),
    );
    let local = LocalConfig {
        cluster_by: Some(dbt_schemas::schemas::common::ClusterConfig::List(vec![
            "id".to_string(),
        ])),
        ..base_local_config()
    };
    render_ok(&fixture, &local, false, "continue");

    assert!(
        fixture.executed().is_empty(),
        "`continue` must not build, got: {:?}",
        fixture.executed()
    );
    assert_eq!(fixture.raw_results(), vec!["main=skip".to_string()]);
}

#[test]
fn create_or_alter_rejects_an_unknown_on_configuration_change_value() {
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        create_or_alter_options(),
    );
    let local = LocalConfig {
        cluster_by: Some(dbt_schemas::schemas::common::ClusterConfig::List(vec![
            "id".to_string(),
        ])),
        ..base_local_config()
    };
    assert!(render(&fixture, &local, false, "bogus").is_err());
    assert!(fixture.executed().is_empty());
}

#[test]
fn create_or_alter_transient_comes_from_the_default_flag_when_unset() {
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            transient: Some(true),
            ..matching_remote_state()
        },
        FixtureOptions {
            create_or_alter: true,
            default_transient: true,
            ..Default::default()
        },
    );
    render_ok(&fixture, &base_local_config(), false, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create or alter transient dynamic table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH refresh_mode = AUTO scheduler = 'ENABLE' as ( {MODEL_SQL} )"
        )]
    );
}

#[test]
fn create_or_alter_explicit_transient_overrides_the_default_flag() {
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            transient: Some(false),
            ..matching_remote_state()
        },
        FixtureOptions {
            create_or_alter: true,
            default_transient: true,
            ..Default::default()
        },
    );
    let local = LocalConfig {
        transient: Some(false),
        ..base_local_config()
    };
    render_ok(&fixture, &local, false, "apply");

    assert_eq!(fixture.executed(), vec![base_create_or_alter_sql()]);
}

#[test]
fn create_or_alter_ddl_emits_the_declarative_attributes() {
    // initialization_warehouse, refresh_mode, scheduler, cluster_by and immutable_where are part of
    // the re-declared end state.
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            initialization_warehouse: Some("INIT_WH".to_string()),
            cluster_by: Some("LINEAR(id)".to_string()),
            refresh_mode: Some("FULL".to_string()),
            ..matching_remote_state()
        },
        create_or_alter_options(),
    );
    let local = LocalConfig {
        snowflake_initialization_warehouse: Some("INIT_WH".to_string()),
        cluster_by: Some(dbt_schemas::schemas::common::ClusterConfig::List(vec![
            "id".to_string(),
        ])),
        refresh_mode: Some("FULL".to_string()),
        immutable_where: Some("id < 10".to_string()),
        ..base_local_config()
    };
    let result = render(&fixture, &local, false, "apply");
    result.unwrap_or_else(|e| panic!("materialization should render: {e:?}"));

    let executed = fixture.executed();
    assert_eq!(executed.len(), 1, "got: {executed:?}");
    assert_eq!(
        executed[0],
        format!(
            "create or alter dynamic table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH initialization_warehouse = INIT_WH refresh_mode = FULL scheduler = 'ENABLE' cluster by (id) immutable where (id < 10) as ( {MODEL_SQL} )"
        )
    );
}

#[test]
fn create_or_alter_ddl_omits_create_only_and_governance_clauses() {
    // `initialize` is create-time-only, `copy grants` applies to a replaced object, and Snowflake
    // rejects policy / tag clauses in CREATE OR ALTER (error 001506).
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        create_or_alter_options(),
    );
    let local = LocalConfig {
        initialize: Some("ON_SCHEDULE".to_string()),
        copy_grants: Some(true),
        table_tag: Some("my_tag = 'v1'".to_string()),
        row_access_policy: Some("my_policy on (id)".to_string()),
        ..base_local_config()
    };
    render_ok(&fixture, &local, false, "apply");

    let executed = fixture.executed();
    assert_eq!(executed, vec![base_create_or_alter_sql()]);
    let sql = executed[0].to_lowercase();
    for forbidden in ["initialize", "copy grants", "tag", "row access policy"] {
        assert!(!sql.contains(forbidden), "`{forbidden}` leaked into: {sql}");
    }
}

#[test]
fn create_or_alter_without_target_lag_declares_a_disabled_scheduler() {
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            scheduler: Some("DISABLE".to_string()),
            warehouse: Some("WH".to_string()),
            ..Default::default()
        },
        create_or_alter_options(),
    );
    let local = LocalConfig {
        snowflake_warehouse: Some("WH".to_string()),
        ..Default::default()
    };
    render_ok(&fixture, &local, false, "apply");

    let executed = fixture.executed();
    assert_eq!(
        executed[0],
        format!(
            "create or alter dynamic table {RENDERED_RELATION} warehouse = WH refresh_mode = AUTO scheduler = 'DISABLE' as ( {MODEL_SQL} )"
        ),
        "got: {executed:?}"
    );
}
