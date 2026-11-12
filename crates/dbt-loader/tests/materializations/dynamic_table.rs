//! Tests for `dbt-snowflake/macros/materializations/dynamic_table.sql`.
//!
//! The materialization is driven end to end: `relation.from_config(config.model)` and
//! `relation.dynamic_table_config_changeset(...)` are the real implementations, so `config.model`
//! here is a `DbtModel` serialized exactly the way the run context serializes it, and the mocked
//! `describe_relation` returns a readback shaped like `SHOW DYNAMIC TABLES`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use dbt_adapter::Column;
use dbt_adapter::relation::{RelationObject, create_relation};
use dbt_adapter_core::AdapterType;
use dbt_agate::AgateTable;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::jinja_node::LazyNodeContext;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::dbt_catalogs::TableFormat;
use dbt_schemas::schemas::project::{
    LazyWarehouseSpecificNodeConfig, ModelConfig, WarehouseSpecificNodeConfig,
};
use dbt_schemas::schemas::relations::SNOWFLAKE_RESOLVED_QUOTING;
use dbt_schemas::schemas::serialization_utils::SerializationMode;
use dbt_schemas::schemas::{
    AdapterAttr, CommonAttributes, DbtModel, DbtModelAttr, NodeBaseAttributes,
};
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
    transient: Option<bool>,
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
        ("cluster_by", Utf8, [state.cluster_by]),
        ("transient", Boolean, [state.transient])
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
    transient: Option<bool>,
    scheduler: Option<String>,
    immutable_where: Option<String>,
    initialize: Option<String>,
    copy_grants: Option<bool>,
    table_tag: Option<String>,
    row_access_policy: Option<String>,
    /// Configure a managed Iceberg table (`table_format`, `external_volume`, `base_location_root`,
    /// `iceberg_version`); the catalog relation is then resolved from this config for real.
    iceberg: bool,
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
        quoting: SNOWFLAKE_RESOLVED_QUOTING,
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

    let wh_config: LazyWarehouseSpecificNodeConfig = WarehouseSpecificNodeConfig {
        target_lag: config.target_lag.clone(),
        snowflake_warehouse: config.snowflake_warehouse.clone(),
        snowflake_initialization_warehouse: config.snowflake_initialization_warehouse.clone(),
        cluster_by: config.cluster_by.clone(),
        refresh_mode: config.refresh_mode.clone(),
        transient: config.transient,
        scheduler: config.scheduler.clone(),
        immutable_where: config.immutable_where.clone(),
        initialize: config.initialize.clone(),
        copy_grants: config.copy_grants,
        table_tag: config.table_tag.clone(),
        row_access_policy: config.row_access_policy.clone(),
        external_volume: config.iceberg.then(|| "MY_VOLUME".to_string()),
        base_location_root: config.iceberg.then(|| "_dbt".to_string()),
        iceberg_version: config.iceberg.then_some(2),
        ..Default::default()
    }
    .into();
    let adapter_attr = AdapterAttr::from_lazy_config_and_dialect(&wh_config, ADAPTER);

    DbtModel {
        deprecated_config: ModelConfig {
            __warehouse_specific_config__: wh_config,
            ..Default::default()
        },
        __model_attr__: DbtModelAttr {
            table_format: config.iceberg.then(|| "iceberg".to_string()),
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
    let immutable_where = local.immutable_where.clone();
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
            Some("immutable_where") => Ok(immutable_where.clone().map_or(default, Value::from)),
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

/// Knobs for the adapter-level collaborators the materialization consults.
#[derive(Default)]
struct FixtureOptions {
    /// The `snowflake_dynamic_table_create_or_alter` behavior flag. The materialization's gate
    /// (`snowflake__dynamic_table_create_or_alter_enabled`) combines it with the resolved catalog.
    create_or_alter: bool,
    /// The `snowflake_default_transient_dynamic_tables` behavior flag.
    default_transient: bool,
    /// The configured catalog is a managed Iceberg one (`BUILT_IN`) instead of the native default.
    iceberg_target: bool,
    /// The live relation is a dynamic Iceberg table (`SHOW OBJECTS` reports `is_iceberg`).
    iceberg_existing: bool,
    /// What CI DT column discovery reads back, as `(name, data type)` pairs; [`CI_COLUMNS`] when
    /// unset.
    columns: Option<&'static [(&'static str, &'static str)]>,
}

/// Build a harness whose `describe_relation` returns `remote` for a dynamic-table relation.
fn fixture(existing: Option<RelationType>, remote: RemoteState) -> Fixture {
    fixture_with(existing, remote, FixtureOptions::default())
}

fn fixture_with(
    existing: Option<RelationType>,
    remote: RemoteState,
    options: FixtureOptions,
) -> Fixture {
    let mut harness = MacroTestHarness::for_adapter(ADAPTER)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    let raw_results = install_recorders(&mut harness);
    install_behavior(&harness, &options);
    install_catalog_relation(&harness, options.iceberg_target);

    let existing_value = existing_relation_value(existing, options.iceberg_existing);
    harness
        .mock()
        .on("get_relation", move |_| Ok(existing_value.clone()));

    let described = describe_result(remote);
    harness
        .mock()
        .on("describe_relation", move |_| Ok(described.clone()));
    install_column_discovery(&harness, options.columns.unwrap_or(CI_COLUMNS));

    Fixture {
        harness,
        raw_results,
    }
}

/// `get_columns_in_relation` (CI DT column discovery) returns `columns`; the temporary relation
/// is dropped through `drop_relation`.
fn install_column_discovery(harness: &MacroTestHarness, columns: &[(&str, &str)]) {
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
}

/// Capture `store_raw_result` calls (returned) and swallow `log`.
fn install_recorders(harness: &mut MacroTestHarness) -> Recorder {
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
    raw_results
}

/// The behavior flags the materialization reads.
fn install_behavior(harness: &MacroTestHarness, options: &FixtureOptions) {
    harness.mock().set_attr(
        "behavior",
        Value::from_serialize(BTreeMap::from([
            ("use_catalogs_v2", BTreeMap::from([("no_warn", false)])),
            (
                "snowflake_default_transient_dynamic_tables",
                BTreeMap::from([("no_warn", options.default_transient)]),
            ),
            (
                "snowflake_dynamic_table_create_or_alter",
                BTreeMap::from([("no_warn", options.create_or_alter)]),
            ),
        ])),
    );
}

/// `build_catalog_relation`: the default native catalog, or (for an Iceberg target) the managed
/// Iceberg catalog resolved from the model config exactly as the adapter does.
fn install_catalog_relation(harness: &MacroTestHarness, iceberg_target: bool) {
    let native_catalog_relation = Value::from_object(
        dbt_adapter::catalog_relation::CatalogRelation::default_catalog_relation_snowflake(),
    );
    harness.mock().on("build_catalog_relation", move |args| {
        if !iceberg_target {
            return Ok(native_catalog_relation.clone());
        }
        let model = args
            .first()
            .expect("build_catalog_relation takes the model");
        let catalog_relation =
            dbt_adapter::catalog_relation::CatalogRelation::from_model_config_and_catalogs(
                ADAPTER, model, None,
            )
            .expect("the managed Iceberg model config should resolve");
        Ok(Value::from_object(catalog_relation))
    });
}

/// The relation `get_relation` returns: none, or an existing dynamic table (native or Iceberg).
fn existing_relation_value(existing: Option<RelationType>, iceberg: bool) -> Value {
    let Some(relation_type) = existing else {
        return Value::from(());
    };
    let mut relation = create_relation(
        ADAPTER,
        DATABASE.to_string(),
        SCHEMA.to_string(),
        Some(IDENTIFIER.to_string()),
        Some(relation_type),
        SNOWFLAKE_RESOLVED_QUOTING,
    )
    .expect("create_relation");
    if iceberg {
        relation.set_table_format(Some(TableFormat::Iceberg));
    }
    RelationObject::new(Arc::from(relation)).into_value()
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

/// [`render`] with `sql` as the compiled model SQL. A `local.raw_code` makes `model` a run-time
/// `LazyNodeContext` carrying it.
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

mod create_or_alter;
mod custom_incremental;
mod iceberg;
