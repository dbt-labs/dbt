//! Managed dynamic Iceberg tables (`BUILT_IN` catalog) under `snowflake_dynamic_table_create_or_alter`.
//!
//! Snowflake lets a managed dynamic Iceberg table change its dynamic-table clauses (warehouse,
//! target lag, refresh mode, query, clustering) like a native one, forbids changing the Iceberg
//! identity clauses (external volume, catalog, base location, Iceberg version), and cannot convert
//! between a native and an Iceberg dynamic table. The Snowflake-side rejections are exercised only
//! against a live account; these tests pin the DDL dbt sends and the routing around it.

use super::*;

fn iceberg_options() -> FixtureOptions {
    FixtureOptions {
        create_or_alter: true,
        iceberg_target: true,
        iceberg_existing: true,
        ..Default::default()
    }
}

/// [`base_local_config`] on a managed Iceberg table.
fn iceberg_local_config() -> LocalConfig {
    LocalConfig {
        iceberg: true,
        ..base_local_config()
    }
}

/// The identity clauses CREATE OR ALTER must restate, as resolved from [`iceberg_local_config`].
const ICEBERG_IDENTITY: &str = "external_volume = 'MY_VOLUME' catalog = 'SNOWFLAKE' base_location = '_dbt/TEST_SCHEMA/MY_DT' iceberg_version = 2";

/// The `create or alter` DDL for [`base_local_config`] on a managed Iceberg table.
fn base_iceberg_create_or_alter_sql() -> String {
    format!(
        "create or alter dynamic iceberg table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH {ICEBERG_IDENTITY} refresh_mode = AUTO scheduler = 'ENABLE' as ( {MODEL_SQL} )"
    )
}

/// Apply `local` to an existing dynamic table that reads back as `remote`.
fn sync(remote: RemoteState, options: FixtureOptions, local: &LocalConfig) -> Fixture {
    let fixture = fixture_with(Some(RelationType::DynamicTable), remote, options);
    render_ok(&fixture, local, false, "apply");
    fixture
}

/// The run executed exactly one statement, and it starts with `prefix`.
fn assert_one_statement_starting_with(fixture: &Fixture, prefix: &str) {
    let executed = fixture.executed();
    assert_eq!(executed.len(), 1, "got: {executed:?}");
    assert!(executed[0].starts_with(prefix), "got: {executed:?}");
}

#[test]
fn create_or_alter_iceberg_deploys_the_definition_and_restates_the_identity() {
    let fixture = sync(
        matching_remote_state(),
        iceberg_options(),
        &iceberg_local_config(),
    );

    assert_eq!(fixture.executed(), vec![base_iceberg_create_or_alter_sql()]);
}

#[test]
fn create_or_alter_iceberg_applies_dynamic_table_attributes_in_place() {
    // Warehouse, target lag and clustering change under the same rules as a native table.
    let remote = RemoteState {
        target_lag: Some("5 minutes".to_string()),
        warehouse: Some("OLD_WH".to_string()),
        ..matching_remote_state()
    };
    let local = LocalConfig {
        cluster_by: Some(dbt_schemas::schemas::common::ClusterConfig::List(vec![
            "id".to_string(),
        ])),
        ..iceberg_local_config()
    };
    let fixture = sync(remote, iceberg_options(), &local);

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create or alter dynamic iceberg table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH {ICEBERG_IDENTITY} refresh_mode = AUTO scheduler = 'ENABLE' cluster by (id) as ( {MODEL_SQL} )"
        )]
    );
}

#[test]
fn create_or_alter_iceberg_refresh_mode_change_routes_to_replace() {
    let remote = RemoteState {
        refresh_mode: Some("INCREMENTAL".to_string()),
        ..matching_remote_state()
    };
    let local = LocalConfig {
        refresh_mode: Some("FULL".to_string()),
        ..iceberg_local_config()
    };
    let fixture = sync(remote, iceberg_options(), &local);

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create or replace dynamic iceberg table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH external_volume = 'MY_VOLUME' catalog = 'snowflake' base_location = '_dbt/TEST_SCHEMA/MY_DT' iceberg_version = 2 refresh_mode = FULL initialize = ON_CREATE scheduler = 'ENABLE' as ( {MODEL_SQL} )"
        )]
    );
}

#[test]
fn create_or_alter_native_to_iceberg_conversion_routes_to_replace() {
    // CREATE OR ALTER cannot convert a regular dynamic table to a dynamic Iceberg table.
    let options = FixtureOptions {
        iceberg_existing: false,
        ..iceberg_options()
    };
    let fixture = sync(matching_remote_state(), options, &iceberg_local_config());

    assert_one_statement_starting_with(&fixture, "create or replace dynamic iceberg table");
}

#[test]
fn create_or_alter_iceberg_to_native_conversion_routes_to_replace() {
    // ... and cannot convert a dynamic Iceberg table to a regular one either.
    let options = FixtureOptions {
        iceberg_target: false,
        ..iceberg_options()
    };
    let fixture = sync(matching_remote_state(), options, &iceberg_local_config());

    assert_one_statement_starting_with(&fixture, "create or replace dynamic table");
}

#[test]
fn create_or_alter_iceberg_does_not_apply_on_first_build_or_full_refresh() {
    let first_build = fixture_with(None, RemoteState::default(), iceberg_options());
    render_ok(&first_build, &iceberg_local_config(), false, "apply");
    assert_one_statement_starting_with(&first_build, "create dynamic iceberg table");

    let full_refresh = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        iceberg_options(),
    );
    render_ok(&full_refresh, &iceberg_local_config(), true, "apply");
    assert_one_statement_starting_with(&full_refresh, "create or replace dynamic iceberg table");
}

#[test]
fn create_or_alter_iceberg_gate_off_keeps_sql_only_edit_a_noop() {
    let options = FixtureOptions {
        create_or_alter: false,
        ..iceberg_options()
    };
    let fixture = sync(matching_remote_state(), options, &iceberg_local_config());

    assert!(
        fixture.executed().is_empty(),
        "gate off must not build, got: {:?}",
        fixture.executed()
    );
    assert_eq!(fixture.raw_results(), vec!["main=skip".to_string()]);
}

#[test]
fn create_or_alter_iceberg_omits_transient_and_governance_clauses() {
    // Iceberg targets reject transient conversion, so the default-transient behavior flag must not
    // reach the DDL; the create-only and governance clauses are omitted as for native tables.
    let options = FixtureOptions {
        default_transient: true,
        ..iceberg_options()
    };
    let local = LocalConfig {
        initialize: Some("ON_SCHEDULE".to_string()),
        copy_grants: Some(true),
        table_tag: Some("my_tag = 'v1'".to_string()),
        row_access_policy: Some("my_policy on (id)".to_string()),
        ..iceberg_local_config()
    };
    let fixture = sync(matching_remote_state(), options, &local);

    let executed = fixture.executed();
    assert_eq!(executed, vec![base_iceberg_create_or_alter_sql()]);
    let sql = executed[0].to_lowercase();
    for forbidden in [
        "transient",
        "initialize",
        "copy grants",
        "tag",
        "row access policy",
    ] {
        assert!(!sql.contains(forbidden), "`{forbidden}` leaked into: {sql}");
    }
}

#[test]
fn create_or_alter_iceberg_without_target_lag_declares_a_disabled_scheduler() {
    let remote = RemoteState {
        scheduler: Some("DISABLE".to_string()),
        warehouse: Some("WH".to_string()),
        ..Default::default()
    };
    let local = LocalConfig {
        snowflake_warehouse: Some("WH".to_string()),
        iceberg: true,
        ..Default::default()
    };
    let fixture = sync(remote, iceberg_options(), &local);

    assert_eq!(
        fixture.executed(),
        vec![
            format!(
                "create or alter dynamic iceberg table {RENDERED_RELATION} warehouse = WH {ICEBERG_IDENTITY} refresh_mode = AUTO scheduler = 'DISABLE' as ( {MODEL_SQL} )"
            ),
            format!("alter dynamic table {RENDERED_RELATION} refresh"),
        ]
    );
}

#[test]
fn create_or_alter_iceberg_honors_on_configuration_change() {
    let drifted = || RemoteState {
        target_lag: Some("5 minutes".to_string()),
        ..matching_remote_state()
    };

    let continued = fixture_with(
        Some(RelationType::DynamicTable),
        drifted(),
        iceberg_options(),
    );
    render_ok(&continued, &iceberg_local_config(), false, "continue");
    assert!(continued.executed().is_empty());

    let failing = fixture_with(
        Some(RelationType::DynamicTable),
        drifted(),
        iceberg_options(),
    );
    assert!(render(&failing, &iceberg_local_config(), false, "fail").is_err());
}
