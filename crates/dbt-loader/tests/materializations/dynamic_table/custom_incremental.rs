//! Custom incremental dynamic tables (`refresh_mode='CUSTOM_INCREMENTAL'`): a dynamic table whose
//! refresh is one DML statement, `create dynamic table X (<cols>) ... refresh using (<dml>)`.

use super::*;

const CI_TEMP_RELATION: &str = "TEST_DB.TEST_SCHEMA.MY_DT__dbt_tmp";
const CI_COLUMNS_DDL: &str = r#"("ID" NUMBER(38,0), "AMOUNT" NUMBER(38,2), "IS_DELETED" BOOLEAN)"#;
const CI_UPDATE_SET: &str = r#"update set "ID" = DBT_INTERNAL_SOURCE."ID", "AMOUNT" = DBT_INTERNAL_SOURCE."AMOUNT", "IS_DELETED" = DBT_INTERNAL_SOURCE."IS_DELETED""#;
const CI_INSERT: &str = r#"insert ("ID", "AMOUNT", "IS_DELETED") values (DBT_INTERNAL_SOURCE."ID", DBT_INTERNAL_SOURCE."AMOUNT", DBT_INTERNAL_SOURCE."IS_DELETED")"#;

/// A merge CI DT with a refreshing schedule.
fn ci_local_config() -> LocalConfig {
    LocalConfig {
        refresh_mode: Some("CUSTOM_INCREMENTAL".to_string()),
        incremental_strategy: Some("merge"),
        unique_key: Some(Value::from("id")),
        raw_code: Some(CI_RAW_CODE),
        ..base_local_config()
    }
}

/// The readback of a CI DT already matching [`ci_local_config`].
fn ci_matching_remote_state() -> RemoteState {
    RemoteState {
        refresh_mode: Some("CUSTOM_INCREMENTAL".to_string()),
        ..matching_remote_state()
    }
}

fn render_ci(
    fixture: &Fixture,
    local: &LocalConfig,
    full_refresh: bool,
) -> dbt_common::FsResult<String> {
    render_sql(fixture, local, CI_SQL, full_refresh, "apply")
}

/// The discovery statement: the model rendered with `changes()` starting now, run zero-row into
/// a temporary table.
fn ci_discovery_sql() -> String {
    format!(
        "create or replace temporary table {CI_TEMP_RELATION} as ( select * from ( select o.id, o.amount, o.METADATA$ACTION = 'DELETE' as is_deleted from RAW.PUBLIC.ORDERS changes(information => DEFAULT) at(offset => 0) as o ) limit 0 ) ;"
    )
}

/// The stored merge body for [`ci_local_config`].
fn ci_merge_body(sql: &str) -> String {
    format!(
        "merge into SELF as DBT_INTERNAL_DEST using ( {sql} ) as DBT_INTERNAL_SOURCE on DBT_INTERNAL_DEST.id = DBT_INTERNAL_SOURCE.id when matched then {CI_UPDATE_SET} when not matched then {CI_INSERT}"
    )
}

fn ci_ddl(verb: &str, body: &str) -> String {
    format!(
        "{verb} dynamic table {RENDERED_RELATION} {CI_COLUMNS_DDL} target_lag = '1 minute' warehouse = WH refresh_mode = CUSTOM_INCREMENTAL initialize = ON_CREATE scheduler = 'ENABLE' refresh using ( {body} )"
    )
}

/// `refresh using (...)`, through its closing paren, as rendered: no whitespace normalization.
fn raw_refresh_using_body(ddl: &str) -> String {
    let start = ddl
        .find("refresh using (")
        .expect("a CI DT DDL has a refresh using body");
    let end = ddl.rfind(')').expect("the refresh using body is closed");
    ddl[start..=end].to_string()
}

fn assert_render_error(result: dbt_common::FsResult<String>, fixture: &Fixture, needle: &str) {
    let err = result.expect_err("the materialization must raise");
    assert!(
        format!("{err:?}").contains(needle),
        "error should mention `{needle}`, got: {err:?}"
    );
    assert!(
        fixture.executed().is_empty(),
        "nothing must run when validation fails, got: {:?}",
        fixture.executed()
    );
}

#[test]
fn ci_create_discovers_columns_and_stores_a_merge_body() {
    let fixture = fixture(None, RemoteState::default());
    render_ci(&fixture, &ci_local_config(), false).expect("CI DT should render");

    assert_eq!(
        fixture.executed(),
        vec![ci_discovery_sql(), ci_ddl("create", &ci_merge_body(CI_SQL))],
        "a first build discovers the columns, then creates the CI DT with them"
    );
    fixture
        .harness
        .mock()
        .observed_calls()
        .assert_called("drop_relation");
}

#[test]
fn ci_full_refresh_replaces_with_the_same_body_as_create() {
    let created = fixture(None, RemoteState::default());
    render_ci(&created, &ci_local_config(), false).expect("create should render");
    let replaced = fixture(Some(RelationType::DynamicTable), ci_matching_remote_state());
    render_ci(&replaced, &ci_local_config(), true).expect("full refresh should render");

    assert_eq!(
        replaced.executed(),
        vec![
            ci_discovery_sql(),
            ci_ddl("create or replace", &ci_merge_body(CI_SQL))
        ],
        "--full-refresh rediscovers the columns and replaces the CI DT"
    );
    assert_eq!(
        raw_refresh_using_body(&created.executed_raw()[1]),
        raw_refresh_using_body(&replaced.executed_raw()[1]),
        "create and replace must store a byte-identical refresh body"
    );
}

#[test]
fn ci_composite_key_and_delete_condition() {
    let fixture = fixture(None, RemoteState::default());
    let local = LocalConfig {
        unique_key: Some(Value::from(vec!["id", "amount"])),
        delete_condition: Some("DBT_INTERNAL_SOURCE.is_deleted"),
        ..ci_local_config()
    };
    render_ci(&fixture, &local, false).expect("CI DT should render");

    let body = format!(
        "merge into SELF as DBT_INTERNAL_DEST using ( {CI_SQL} ) as DBT_INTERNAL_SOURCE on DBT_INTERNAL_DEST.id = DBT_INTERNAL_SOURCE.id and DBT_INTERNAL_DEST.amount = DBT_INTERNAL_SOURCE.amount when matched and coalesce(DBT_INTERNAL_SOURCE.is_deleted, false) then delete when matched then {CI_UPDATE_SET} when not matched and not coalesce(DBT_INTERNAL_SOURCE.is_deleted, false) then {CI_INSERT}"
    );
    assert_eq!(fixture.executed()[1], ci_ddl("create", &body));
}

#[test]
fn ci_model_starting_with_a_cte_is_wrapped_in_a_subquery() {
    let fixture = fixture(None, RemoteState::default());
    let sql = "with latest as (select * from RAW.PUBLIC.STOCK changes(information => DEFAULT)) select * from latest";
    render_sql(&fixture, &ci_local_config(), sql, false, "apply").expect("CI DT should render");

    assert_eq!(fixture.executed()[1], ci_ddl("create", &ci_merge_body(sql)));
}

#[test]
fn ci_regular_to_custom_incremental_switch_replaces() {
    let fixture = fixture(
        Some(RelationType::DynamicTable),
        RemoteState {
            refresh_mode: Some("INCREMENTAL".to_string()),
            ..matching_remote_state()
        },
    );
    render_ci(&fixture, &ci_local_config(), false).expect("switch should render");

    assert_eq!(
        fixture.executed(),
        vec![
            ci_discovery_sql(),
            ci_ddl("create or replace", &ci_merge_body(CI_SQL))
        ]
    );
    assert_eq!(
        fixture.describe_calls(),
        1,
        "the switch reuses the changeset's describe"
    );
}

#[test]
fn ci_switch_respects_on_configuration_change() {
    let regular = || RemoteState {
        refresh_mode: Some("INCREMENTAL".to_string()),
        ..matching_remote_state()
    };

    let fixture_continue = fixture(Some(RelationType::DynamicTable), regular());
    render_sql(
        &fixture_continue,
        &ci_local_config(),
        CI_SQL,
        false,
        "continue",
    )
    .expect("`continue` should render");
    assert!(
        fixture_continue.executed().is_empty(),
        "got: {:?}",
        fixture_continue.executed()
    );
    assert_eq!(
        fixture_continue.raw_results(),
        vec!["main=skip".to_string()]
    );

    let fixture_fail = fixture(Some(RelationType::DynamicTable), regular());
    let result = render_sql(&fixture_fail, &ci_local_config(), CI_SQL, false, "fail");
    assert!(result.is_err(), "`fail` must raise, got: {result:?}");
    assert!(fixture_fail.executed().is_empty());

    // CI DT -> regular: the changeset sees no change (desired AUTO), the kind check does.
    let fixture_back = fixture(Some(RelationType::DynamicTable), ci_matching_remote_state());
    let result = render(&fixture_back, &base_local_config(), false, "fail");
    assert!(
        result.is_err(),
        "`fail` must raise on CI DT -> regular, got: {result:?}"
    );
}

#[test]
fn ci_custom_incremental_to_regular_switch_replaces() {
    // The local side leaves refresh_mode unset (AUTO), which the refresh_mode diff treats as
    // matching anything, so only the kind check catches this.
    let fixture = fixture(Some(RelationType::DynamicTable), ci_matching_remote_state());
    render_ok(&fixture, &base_local_config(), false, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create or replace dynamic table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH refresh_mode = AUTO initialize = ON_CREATE scheduler = 'ENABLE' as ( {MODEL_SQL} )"
        )]
    );
    assert_eq!(fixture.describe_calls(), 1);
}

#[test]
fn ci_unchanged_is_a_no_op_without_discovery() {
    let fixture = fixture(Some(RelationType::DynamicTable), ci_matching_remote_state());
    render_ci(&fixture, &ci_local_config(), false).expect("CI DT should render");

    assert!(
        fixture.executed().is_empty(),
        "got: {:?}",
        fixture.executed()
    );
    assert_eq!(fixture.raw_results(), vec!["main=skip".to_string()]);
    fixture
        .harness
        .mock()
        .observed_calls()
        .assert_not_called("get_columns_in_relation");
}

#[test]
fn ci_target_lag_change_is_altered_in_place() {
    // Same as a regular dynamic table: properties ALTER can change are altered, not replaced.
    let fixture = fixture(Some(RelationType::DynamicTable), ci_matching_remote_state());
    let local = LocalConfig {
        target_lag: Some("5 minutes".to_string()),
        ..ci_local_config()
    };
    render_ci(&fixture, &local, false).expect("CI DT should render");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "alter dynamic table {RENDERED_RELATION} set target_lag = '5 minutes'"
        )]
    );
    fixture
        .harness
        .mock()
        .observed_calls()
        .assert_not_called("get_columns_in_relation");
}

#[test]
fn ci_without_target_lag_is_refreshed_after_the_build() {
    let fixture = fixture(None, RemoteState::default());
    let local = LocalConfig {
        target_lag: None,
        ..ci_local_config()
    };
    render_ci(&fixture, &local, false).expect("CI DT should render");

    let executed = fixture.executed();
    assert_eq!(executed.len(), 3, "got: {executed:?}");
    assert!(
        executed[1].contains("scheduler = 'DISABLE' refresh using ("),
        "got: {}",
        executed[1]
    );
    assert_eq!(
        executed[2],
        format!("alter dynamic table {RENDERED_RELATION} refresh")
    );
}

#[test]
fn ci_requires_an_incremental_strategy() {
    let fixture = fixture(None, RemoteState::default());
    let local = LocalConfig {
        incremental_strategy: None,
        ..ci_local_config()
    };
    assert_render_error(
        render_ci(&fixture, &local, false),
        &fixture,
        "no `incremental_strategy`",
    );
}

#[test]
fn ci_rejects_strategies_other_than_merge() {
    for strategy in ["append", "delete+insert", "microbatch"] {
        let fixture = fixture(None, RemoteState::default());
        let local = LocalConfig {
            incremental_strategy: Some(strategy),
            ..ci_local_config()
        };
        assert_render_error(
            render_ci(&fixture, &local, false),
            &fixture,
            "unsupported incremental_strategy",
        );
    }
}

#[test]
fn ci_merge_requires_a_unique_key() {
    let fixture = fixture(None, RemoteState::default());
    let local = LocalConfig {
        unique_key: None,
        ..ci_local_config()
    };
    assert_render_error(
        render_ci(&fixture, &local, false),
        &fixture,
        "requires `unique_key`",
    );
}

#[test]
fn ci_delete_condition_cannot_reference_the_target_row() {
    let fixture = fixture(None, RemoteState::default());
    let local = LocalConfig {
        delete_condition: Some("dbt_internal_dest.is_deleted"),
        ..ci_local_config()
    };
    assert_render_error(
        render_ci(&fixture, &local, false),
        &fixture,
        "not DBT_INTERNAL_DEST",
    );
}

#[test]
fn ci_raw_changes_clause_is_rejected_with_a_hint() {
    let fixture = fixture(None, RemoteState::default());
    let local = LocalConfig {
        raw_code: Some("select * from RAW.PUBLIC.ORDERS CHANGES ( INFORMATION => DEFAULT )"),
        ..ci_local_config()
    };
    assert_render_error(
        render_ci(&fixture, &local, false),
        &fixture,
        "{{ changes(ref(",
    );
}

#[test]
fn ci_metadata_column_in_the_output_is_rejected() {
    let fixture = fixture_with(
        None,
        RemoteState::default(),
        FixtureOptions {
            columns: Some(&[("ID", "NUMBER(38,0)"), ("METADATA$ACTION", "VARCHAR(6)")]),
            ..Default::default()
        },
    );
    let err = render_ci(&fixture, &ci_local_config(), false).expect_err("must raise");
    assert!(
        format!("{err:?}").contains("METADATA$ACTION"),
        "got: {err:?}"
    );
    assert_eq!(
        fixture.executed(),
        vec![ci_discovery_sql()],
        "only discovery runs before the error"
    );
}

#[test]
fn incremental_strategy_without_custom_incremental_builds_a_regular_dynamic_table() {
    let fixture = fixture(None, RemoteState::default());
    let local = LocalConfig {
        incremental_strategy: Some("merge"),
        ..base_local_config()
    };
    render_ok(&fixture, &local, false, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create dynamic table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH refresh_mode = AUTO initialize = ON_CREATE scheduler = 'ENABLE' as ( {MODEL_SQL} )"
        )]
    );
}

#[test]
fn ci_cannot_be_an_iceberg_table() {
    let fixture = fixture(None, RemoteState::default());
    let built_in = Value::from_serialize(BTreeMap::from([("catalog_type", "BUILT_IN")]));
    fixture
        .harness
        .mock()
        .on("build_catalog_relation", move |_| Ok(built_in.clone()));
    assert_render_error(
        render_ci(&fixture, &ci_local_config(), false),
        &fixture,
        "dynamic Iceberg tables",
    );
}

#[test]
fn ci_strategy_is_case_insensitive() {
    let fixture = fixture(None, RemoteState::default());
    let local = LocalConfig {
        incremental_strategy: Some("MERGE"),
        ..ci_local_config()
    };
    render_ci(&fixture, &local, false).expect("'MERGE' should be accepted");
    assert_eq!(
        fixture.executed()[1],
        ci_ddl("create", &ci_merge_body(CI_SQL))
    );
}

#[test]
fn ci_ephemeral_upstream_is_rejected_before_discovery_runs() {
    let fixture = fixture(None, RemoteState::default());
    let local = LocalConfig {
        raw_code: Some(
            "select o.* from {{ changes('RAW.PUBLIC.ORDERS', 'default') }} as o join __dbt__cte__dim as d on o.id = d.id",
        ),
        ..ci_local_config()
    };
    assert_render_error(
        render_ci(&fixture, &local, false),
        &fixture,
        "refs an ephemeral model",
    );
}

#[test]
fn ci_merge_update_columns_narrow_the_update() {
    let fixture = fixture(None, RemoteState::default());
    let local = LocalConfig {
        merge_update_columns: Some(Value::from(vec!["amount"])),
        ..ci_local_config()
    };
    render_ci(&fixture, &local, false).expect("CI DT should render");

    let body = format!(
        "merge into SELF as DBT_INTERNAL_DEST using ( {CI_SQL} ) as DBT_INTERNAL_SOURCE on DBT_INTERNAL_DEST.id = DBT_INTERNAL_SOURCE.id when matched then update set amount = DBT_INTERNAL_SOURCE.amount when not matched then {CI_INSERT}"
    );
    assert_eq!(fixture.executed()[1], ci_ddl("create", &body));
}

#[test]
fn ci_merge_exclude_columns_are_not_updated() {
    let fixture = fixture(None, RemoteState::default());
    let local = LocalConfig {
        merge_exclude_columns: Some(Value::from(vec!["is_deleted"])),
        ..ci_local_config()
    };
    render_ci(&fixture, &local, false).expect("CI DT should render");

    let body = format!(
        r#"merge into SELF as DBT_INTERNAL_DEST using ( {CI_SQL} ) as DBT_INTERNAL_SOURCE on DBT_INTERNAL_DEST.id = DBT_INTERNAL_SOURCE.id when matched then update set "ID" = DBT_INTERNAL_SOURCE."ID", "AMOUNT" = DBT_INTERNAL_SOURCE."AMOUNT" when not matched then {CI_INSERT}"#
    );
    assert_eq!(fixture.executed()[1], ci_ddl("create", &body));
}

#[test]
fn ci_merge_update_and_exclude_columns_together_raise() {
    let fixture = fixture(None, RemoteState::default());
    let local = LocalConfig {
        merge_update_columns: Some(Value::from(vec!["amount"])),
        merge_exclude_columns: Some(Value::from(vec!["is_deleted"])),
        ..ci_local_config()
    };
    let err = render_ci(&fixture, &local, false).expect_err("must raise");
    assert!(
        format!("{err:?}").contains("merge_update_columns and merge_exclude_columns"),
        "got: {err:?}"
    );
}

#[test]
fn ci_model_referencing_itself_is_rejected() {
    let fixture = fixture(None, RemoteState::default());
    let sql = format!(
        "select o.* from RAW.PUBLIC.ORDERS changes(information => DEFAULT) as o where o.id not in (select id from {RENDERED_RELATION})"
    );
    let result = render_sql(&fixture, &ci_local_config(), &sql, false, "apply");
    assert_render_error(result, &fixture, "references itself");
}

#[test]
fn ci_model_reading_a_relation_whose_name_extends_its_own_is_allowed() {
    let fixture = fixture(None, RemoteState::default());
    let sql =
        format!("select o.* from {RENDERED_RELATION}_RAW changes(information => DEFAULT) as o");
    render_sql(&fixture, &ci_local_config(), &sql, false, "apply")
        .expect("MY_DT_RAW is not a reference to MY_DT");
}

#[test]
fn ci_rejects_immutable_where() {
    // Snowflake: "Unsupported feature Immutability constraint on Dynamic Table with REFRESH USING".
    let fixture = fixture(None, RemoteState::default());
    let local = LocalConfig {
        immutable_where: Some("id < 100".to_string()),
        ..ci_local_config()
    };
    assert_render_error(
        render_ci(&fixture, &local, false),
        &fixture,
        "does not support on a custom incremental dynamic table",
    );
}
