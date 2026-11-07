//! Tests for `dbt-snowflake/macros/materializations/dynamic_table.sql`.
//!
//! The materialization is driven end to end: `relation.from_config(config.model)` and
//! `relation.dynamic_table_config_changeset(...)` are the real implementations, so `config.model`
//! here is a `DbtModel` serialized exactly the way the run context serializes it, and the mocked
//! `describe_relation` returns a readback shaped like `SHOW DYNAMIC TABLES`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use dbt_adapter::Column;
use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_agate::AgateTable;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::jinja_node::LazyNodeContext;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::project::{ModelConfig, WarehouseSpecificNodeConfig};
use dbt_schemas::schemas::serialization_utils::SerializationMode;
use dbt_schemas::schemas::{AdapterAttr, CommonAttributes, DbtModel, NodeBaseAttributes};
use dbt_yaml::Spanned;
use minijinja::Value;
use minijinja::value::{Kwargs, ValueMap};

use crate::macro_test_harness::{MacroTestHarness, default_mock_config, executed_sql};

const ADAPTER: AdapterType = AdapterType::Snowflake;

const MATERIALIZATION: &str = "{{ materialization_dynamic_table_snowflake() }}";

const DATABASE: &str = "TEST_DB";
const SCHEMA: &str = "TEST_SCHEMA";
const IDENTIFIER: &str = "MY_DT";
const RENDERED_RELATION: &str = "TEST_DB.TEST_SCHEMA.MY_DT";
const MODEL_SQL: &str = "select 1 as id";

/// A CI DT model's template, and its compiled SQL as the run context passes it in `sql`.
const CI_RAW_CODE: &str = "select o.id, o.amount, o.METADATA$ACTION = 'DELETE' as is_deleted from {{ changes('RAW.PUBLIC.ORDERS') }} as o";
const CI_SQL: &str = "select o.id, o.amount, o.METADATA$ACTION = 'DELETE' as is_deleted from RAW.PUBLIC.ORDERS changes(information => DEFAULT) as o";
/// What CI DT column discovery reads back from the temporary relation.
const CI_COLUMNS: &[(&str, &str)] = &[
    ("ID", "NUMBER(38,0)"),
    ("AMOUNT", "NUMBER(38,2)"),
    ("IS_DELETED", "BOOLEAN"),
];

/// The columns `describe_relation` returns for a dynamic table, spelled as `SHOW DYNAMIC TABLES`
/// reports them (`adapter_impl.rs`'s `describe_dynamic_table` selects exactly these 11 columns,
/// in this order). A column this fixture omits reads back as unset rather than erroring, so a
/// name that drifts from the adapter's select list would phantom-diff instead of failing.
#[derive(Default)]
struct RemoteState {
    target_lag: Option<String>,
    warehouse: Option<String>,
    scheduler: Option<String>,
    refresh_mode: Option<String>,
    initialization_warehouse: Option<String>,
    cluster_by: Option<String>,
}

/// The `describe_relation` return value for a dynamic table, under the key the changeset reads it
/// from.
fn describe_result(state: RemoteState) -> Value {
    let batch = arrow_array::record_batch!(
        ("name", Utf8, [IDENTIFIER]),
        ("schema_name", Utf8, [SCHEMA]),
        ("database_name", Utf8, [DATABASE]),
        ("text", Utf8, [""]),
        ("target_lag", Utf8, [state.target_lag]),
        ("scheduler", Utf8, [state.scheduler]),
        ("warehouse", Utf8, [state.warehouse]),
        ("refresh_mode", Utf8, [state.refresh_mode]),
        (
            "initialization_warehouse",
            Utf8,
            [state.initialization_warehouse]
        ),
        ("immutable_where", Utf8, [Option::<String>::None]),
        ("cluster_by", Utf8, [state.cluster_by])
    )
    .expect("readback batch should build");

    Value::from(ValueMap::from_iter([(
        Value::from("dynamic_table"),
        Value::from_object(AgateTable::from_record_batch(Arc::new(batch))),
    )]))
}

#[derive(Default)]
struct LocalConfig {
    target_lag: Option<String>,
    snowflake_warehouse: Option<String>,
    snowflake_initialization_warehouse: Option<String>,
    cluster_by: Option<dbt_schemas::schemas::common::ClusterConfig>,
    refresh_mode: Option<String>,
    incremental_strategy: Option<&'static str>,
    unique_key: Option<Value>,
    delete_condition: Option<&'static str>,
    merge_update_columns: Option<Value>,
    merge_exclude_columns: Option<Value>,
    /// The model's template. When set, `model` is a run-time `LazyNodeContext` carrying it as
    /// `raw_code`, which CI DT column discovery re-renders.
    raw_code: Option<&'static str>,
}

/// The model node behind `config.model` and `model`.
fn model_node(config: &LocalConfig) -> DbtModel {
    let base_attr = NodeBaseAttributes {
        adapter: AdapterType::Snowflake,
        propagate: Vec::new(),
        effective_propagation_target: None,
        unrendered_config: Default::default(),
        database: DATABASE.to_string(),
        schema: SCHEMA.to_string(),
        alias: IDENTIFIER.to_string(),
        relation_name: None,
        quoting: dbt_schemas::schemas::relations::SNOWFLAKE_RESOLVED_QUOTING,
        quoting_ignore_case: false,
        materialized: DbtMaterialization::DynamicTable,
        compute: None,
        static_analysis: Spanned::new(dbt_common::io_args::StaticAnalysisKind::On),
        static_analysis_off_reason: None,
        enabled: true,
        extended_model: false,
        persist_docs: None,
        columns: vec![],
        refs: vec![],
        sources: vec![],
        functions: vec![],
        metrics: vec![],
        depends_on: Default::default(),
    };

    let wh_config = WarehouseSpecificNodeConfig {
        target_lag: config.target_lag.clone(),
        snowflake_warehouse: config.snowflake_warehouse.clone(),
        snowflake_initialization_warehouse: config.snowflake_initialization_warehouse.clone(),
        cluster_by: config.cluster_by.clone(),
        refresh_mode: config.refresh_mode.clone(),
        ..Default::default()
    };
    let adapter_attr = AdapterAttr::from_config_and_dialect(&wh_config, ADAPTER);

    DbtModel {
        deprecated_config: ModelConfig {
            __warehouse_specific_config__: wh_config,
            ..Default::default()
        },
        __common_attr__: CommonAttributes {
            name: IDENTIFIER.to_string(),
            fqn: vec!["test".to_string(), IDENTIFIER.to_string()],
            raw_code: config.raw_code.map(str::to_string),
            ..Default::default()
        },
        __adapter_attr__: adapter_attr,
        __base_attr__: base_attr,
        ..Default::default()
    }
}

/// A serialized model node, built the way the run context builds `config.model`.
fn serialized_model(config: &LocalConfig) -> Value {
    let model = model_node(config);
    Value::from_serialize(dbt_common::serde_utils::convert_yml_to_value_map(
        dbt_schemas::schemas::InternalDbtNode::serialize(&model),
    ))
}

fn dynamic_config(
    local: &LocalConfig,
    full_refresh: bool,
    on_configuration_change: &'static str,
) -> Arc<MockJinjaObject> {
    let mock = default_mock_config();
    let refresh_mode = local.refresh_mode.clone();
    let incremental_strategy = local.incremental_strategy;
    let unique_key = local.unique_key.clone();
    let delete_condition = local.delete_condition;
    let merge_update_columns = local.merge_update_columns.clone();
    let merge_exclude_columns = local.merge_exclude_columns.clone();
    mock.on("get", move |args| {
        let key = args.first().and_then(|v| v.as_str());
        // `default` may arrive positionally or as a keyword (`default=...`), which minijinja
        // passes as a trailing `Kwargs` value.
        let default = args
            .last()
            .and_then(|v| <Kwargs as minijinja::value::ArgType>::from_value(Some(v)).ok())
            .and_then(|kwargs| kwargs.get::<Value>("default").ok())
            .or_else(|| args.get(1).cloned())
            .unwrap_or(Value::UNDEFINED);
        match key {
            Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                "enforced".to_string(),
                Value::from(false),
            )]))),
            Some("full_refresh") => Ok(Value::from(full_refresh)),
            Some("on_configuration_change") => Ok(Value::from(on_configuration_change)),
            Some("refresh_mode") => Ok(refresh_mode.clone().map_or(default, Value::from)),
            Some("incremental_strategy") => Ok(incremental_strategy.map_or(default, Value::from)),
            Some("unique_key") => Ok(unique_key.clone().unwrap_or(default)),
            Some("delete_condition") => Ok(delete_condition.map_or(default, Value::from)),
            Some("merge_update_columns") => Ok(merge_update_columns.clone().unwrap_or(default)),
            Some("merge_exclude_columns") => Ok(merge_exclude_columns.clone().unwrap_or(default)),
            _ => Ok(default),
        }
    });
    mock.set_attr("model", serialized_model(local));
    mock
}

type Recorder = Arc<Mutex<Vec<String>>>;

struct Fixture {
    harness: MacroTestHarness,
    raw_results: Recorder,
}

impl Fixture {
    fn executed(&self) -> Vec<String> {
        self.executed_raw()
            .iter()
            .map(|sql| normalized(&strip_block_comments(sql)))
            .filter(|sql| !sql.is_empty())
            .collect()
    }

    fn executed_raw(&self) -> Vec<String> {
        executed_sql(self.harness.mock())
    }

    fn raw_results(&self) -> Vec<String> {
        self.raw_results.lock().unwrap().clone()
    }

    fn describe_calls(&self) -> usize {
        self.harness
            .mock()
            .observed_calls()
            .to("describe_relation")
            .count()
    }
}

fn normalized(rendered: &str) -> String {
    rendered.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_block_comments(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start..].find("*/") {
            Some(end) => rest = &rest[start + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Build a harness whose `describe_relation` returns `remote` for a dynamic-table relation.
fn fixture(existing: Option<RelationType>, remote: RemoteState) -> Fixture {
    fixture_with_columns(existing, remote, CI_COLUMNS)
}

/// [`fixture`], with `get_columns_in_relation` (CI DT column discovery) returning `columns` as
/// `(name, data type)` pairs.
fn fixture_with_columns(
    existing: Option<RelationType>,
    remote: RemoteState,
    columns: &[(&str, &str)],
) -> Fixture {
    let mut harness = MacroTestHarness::for_adapter(ADAPTER)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    let raw_results: Recorder = Arc::new(Mutex::new(Vec::new()));
    let sink = raw_results.clone();
    harness
        .env_mut()
        .env
        .add_function("store_raw_result", move |kwargs: Kwargs| {
            let name = kwargs.get::<Value>("name").unwrap_or(Value::UNDEFINED);
            let code = kwargs.get::<Value>("code").unwrap_or(Value::UNDEFINED);
            sink.lock().unwrap().push(format!("{name}={code}"));
            Ok(Value::UNDEFINED)
        });

    let logs: Recorder = Arc::new(Mutex::new(Vec::new()));
    harness
        .env_mut()
        .env
        .add_function("log", move |msg: Value| {
            logs.lock().unwrap().push(msg.to_string());
            Ok(Value::UNDEFINED)
        });

    harness.mock().set_attr(
        "behavior",
        Value::from_serialize(BTreeMap::from([
            ("use_catalogs_v2", BTreeMap::from([("no_warn", false)])),
            (
                "snowflake_default_transient_dynamic_tables",
                BTreeMap::from([("no_warn", false)]),
            ),
        ])),
    );

    let catalog_relation = Value::from_object(
        dbt_adapter::catalog_relation::CatalogRelation::default_catalog_relation_snowflake(),
    );
    harness.mock().on("build_catalog_relation", move |_| {
        Ok(catalog_relation.clone())
    });

    let existing_value = match existing {
        Some(relation_type) => {
            let relation = harness.relation(DATABASE, SCHEMA, IDENTIFIER, Some(relation_type));
            RelationObject::new(relation).into_value()
        }
        None => Value::from(()),
    };
    harness
        .mock()
        .on("get_relation", move |_| Ok(existing_value.clone()));

    let described = describe_result(remote);
    harness
        .mock()
        .on("describe_relation", move |_| Ok(described.clone()));

    let columns = Value::from(
        columns
            .iter()
            .map(|(name, data_type)| {
                Value::from_object(Column::new(
                    ADAPTER,
                    name.to_string(),
                    data_type.to_string(),
                    None,
                    None,
                    None,
                ))
            })
            .collect::<Vec<_>>(),
    );
    harness
        .mock()
        .on("get_columns_in_relation", move |_| Ok(columns.clone()));
    harness.mock().on("drop_relation", |_| Ok(Value::UNDEFINED));

    Fixture {
        harness,
        raw_results,
    }
}

fn render(
    fixture: &Fixture,
    local: &LocalConfig,
    full_refresh: bool,
    on_configuration_change: &'static str,
) -> dbt_common::FsResult<String> {
    render_sql(
        fixture,
        local,
        MODEL_SQL,
        full_refresh,
        on_configuration_change,
    )
}

fn render_sql(
    fixture: &Fixture,
    local: &LocalConfig,
    sql: &str,
    full_refresh: bool,
    on_configuration_change: &'static str,
) -> dbt_common::FsResult<String> {
    let mut ctx = fixture
        .harness
        .materialization_context(IDENTIFIER, sql)
        .database(DATABASE)
        .schema(SCHEMA)
        .relation_type(RelationType::DynamicTable)
        .config(Value::from_dyn_object(dynamic_config(
            local,
            full_refresh,
            on_configuration_change,
        )));
    if local.raw_code.is_some() {
        let model =
            LazyNodeContext::from_node(Arc::new(model_node(local)), SerializationMode::OmitNone)
                .with_eager_nested_maps()
                .with_original_view();
        ctx = ctx.with("model", Value::from_object(model));
    }
    fixture.harness.render(MATERIALIZATION, ctx.build())
}

fn render_ok(
    fixture: &Fixture,
    local: &LocalConfig,
    full_refresh: bool,
    on_configuration_change: &'static str,
) {
    render(fixture, local, full_refresh, on_configuration_change)
        .unwrap_or_else(|e| panic!("materialization should render: {e:?}"));
}

/// A refreshing dynamic table, the configuration most of these tests use.
fn base_local_config() -> LocalConfig {
    LocalConfig {
        target_lag: Some("1 minute".to_string()),
        snowflake_warehouse: Some("WH".to_string()),
        ..Default::default()
    }
}

/// The readback of a table already matching [`base_local_config`].
/// `SchedulerLoader::from_remote_state` only uppercases the raw column, so `scheduler` must be the
/// literal `ENABLE`/`DISABLE` the local side infers from `target_lag` — not a running-state word
/// like "ACTIVE" — or every "no changes" test diffs spuriously.
fn matching_remote_state() -> RemoteState {
    RemoteState {
        target_lag: Some("1 minute".to_string()),
        warehouse: Some("WH".to_string()),
        scheduler: Some("ENABLE".to_string()),
        ..Default::default()
    }
}

#[test]
fn no_existing_relation_creates_the_dynamic_table() {
    let fixture = fixture(None, RemoteState::default());
    render_ok(&fixture, &base_local_config(), false, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create dynamic table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH refresh_mode = AUTO initialize = ON_CREATE scheduler = 'ENABLE' as ( {MODEL_SQL} )"
        )],
        "a first build must emit exactly the create DDL"
    );
    assert_eq!(
        fixture.describe_calls(),
        0,
        "there is no current state to read on a first build"
    );
}

#[test]
fn full_refresh_replaces_rather_than_alters() {
    let fixture = fixture(Some(RelationType::DynamicTable), matching_remote_state());
    render_ok(&fixture, &base_local_config(), true, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create or replace dynamic table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH refresh_mode = AUTO initialize = ON_CREATE scheduler = 'ENABLE' as ( {MODEL_SQL} )"
        )],
        "--full-refresh must replace the relation"
    );
    assert_eq!(
        fixture.describe_calls(),
        0,
        "a full refresh must not consult the current state at all"
    );
}

#[test]
fn no_changes_records_a_skip() {
    let fixture = fixture(Some(RelationType::DynamicTable), matching_remote_state());
    render_ok(&fixture, &base_local_config(), false, "apply");

    assert!(
        fixture.executed().is_empty(),
        "an unchanged relation must not be rebuilt or altered, got: {:?}",
        fixture.executed()
    );
    assert_eq!(
        fixture.raw_results(),
        vec!["main=skip".to_string()],
        "the no-op path records a skip rather than running a build statement"
    );
    assert_eq!(fixture.describe_calls(), 1);
}

#[test]
fn target_lag_change_is_altered_in_place() {
    let fixture = fixture(
        Some(RelationType::DynamicTable),
        RemoteState {
            target_lag: Some("1 minute".to_string()),
            warehouse: Some("WH".to_string()),
            ..Default::default()
        },
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
        )],
        "an alterable change must be applied in place"
    );
    assert_eq!(fixture.describe_calls(), 1);
}

#[test]
fn refresh_mode_change_is_replaced_not_altered() {
    // `refresh_mode`'s diff always treats a desired value of `AUTO` (the local-side default when
    // unset) as matching, regardless of the remote value — see
    // `components::refresh_mode::diff_refresh_mode`. A real diff requires the local side to pin
    // an explicit, non-`AUTO` value that disagrees with the remote readback.
    let fixture = fixture(
        Some(RelationType::DynamicTable),
        RemoteState {
            target_lag: Some("1 minute".to_string()),
            warehouse: Some("WH".to_string()),
            refresh_mode: Some("INCREMENTAL".to_string()),
            ..Default::default()
        },
    );
    let local = LocalConfig {
        target_lag: Some("1 minute".to_string()),
        snowflake_warehouse: Some("WH".to_string()),
        refresh_mode: Some("FULL".to_string()),
        ..Default::default()
    };
    render_ok(&fixture, &local, false, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create or replace dynamic table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH refresh_mode = FULL initialize = ON_CREATE scheduler = 'ENABLE' as ( {MODEL_SQL} )"
        )],
        "a refresh_mode change must rebuild the relation, not be altered in place"
    );
}

#[test]
fn on_configuration_change_fail_raises() {
    let fixture = fixture(
        Some(RelationType::DynamicTable),
        RemoteState {
            target_lag: Some("1 minute".to_string()),
            warehouse: Some("WH".to_string()),
            ..Default::default()
        },
    );
    let local = LocalConfig {
        target_lag: Some("2 minutes".to_string()),
        snowflake_warehouse: Some("WH".to_string()),
        ..Default::default()
    };

    let result = render(&fixture, &local, false, "fail");
    assert!(
        result.is_err(),
        "`fail` must raise when changes are identified, got: {result:?}"
    );
    assert!(
        fixture.executed().is_empty(),
        "`fail` must not execute anything, got: {:?}",
        fixture.executed()
    );
}

#[test]
fn on_configuration_change_continue_skips_the_build() {
    let fixture = fixture(
        Some(RelationType::DynamicTable),
        RemoteState {
            target_lag: Some("1 minute".to_string()),
            warehouse: Some("WH".to_string()),
            ..Default::default()
        },
    );
    let local = LocalConfig {
        target_lag: Some("2 minutes".to_string()),
        snowflake_warehouse: Some("WH".to_string()),
        ..Default::default()
    };

    render_ok(&fixture, &local, false, "continue");
    assert!(
        fixture.executed().is_empty(),
        "`continue` must not build, got: {:?}",
        fixture.executed()
    );
    assert_eq!(fixture.raw_results(), vec!["main=skip".to_string()]);
}

// ---------------------------------------------------------------------------
// Custom incremental dynamic tables (refresh_mode='CUSTOM_INCREMENTAL')
// ---------------------------------------------------------------------------

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
fn ci_config_change_replaces_rather_than_alters() {
    // Only CREATE OR ALTER changes a CI DT's properties, so until that lands a config change
    // rebuilds it.
    let fixture = fixture(Some(RelationType::DynamicTable), ci_matching_remote_state());
    let local = LocalConfig {
        target_lag: Some("5 minutes".to_string()),
        ..ci_local_config()
    };
    render_ci(&fixture, &local, false).expect("CI DT should render");

    let body = ci_merge_body(CI_SQL);
    assert_eq!(
        fixture.executed(),
        vec![
            ci_discovery_sql(),
            format!(
                "create or replace dynamic table {RENDERED_RELATION} {CI_COLUMNS_DDL} target_lag = '5 minutes' warehouse = WH refresh_mode = CUSTOM_INCREMENTAL initialize = ON_CREATE scheduler = 'ENABLE' refresh using ( {body} )"
            )
        ]
    );
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
    let fixture = fixture_with_columns(
        None,
        RemoteState::default(),
        &[("ID", "NUMBER(38,0)"), ("METADATA$ACTION", "VARCHAR(6)")],
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
            "select o.* from {{ changes('RAW.PUBLIC.ORDERS') }} as o join __dbt__cte__dim as d on o.id = d.id",
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
