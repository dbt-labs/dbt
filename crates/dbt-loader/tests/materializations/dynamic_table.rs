//! Tests for `dbt-snowflake/macros/materializations/dynamic_table.sql`.
//!
//! The materialization is driven end to end: `relation.from_config(config.model)` and
//! `relation.dynamic_table_config_changeset(...)` are the real implementations, so `config.model`
//! here is a `DbtModel` serialized exactly the way the run context serializes it, and the mocked
//! `describe_relation` returns a readback shaped like `SHOW DYNAMIC TABLES`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use dbt_adapter::relation::{RelationObject, create_relation};
use dbt_adapter_core::AdapterType;
use dbt_agate::AgateTable;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::dbt_catalogs::TableFormat;
use dbt_schemas::schemas::project::{ModelConfig, WarehouseSpecificNodeConfig};
use dbt_schemas::schemas::relations::SNOWFLAKE_RESOLVED_QUOTING;
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
}

/// A serialized model node, built the way the run context builds `config.model`.
fn serialized_model(config: &LocalConfig) -> Value {
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

    let wh_config = WarehouseSpecificNodeConfig {
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
    };
    let adapter_attr = AdapterAttr::from_config_and_dialect(&wh_config, ADAPTER);

    let model = DbtModel {
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
            ..Default::default()
        },
        __adapter_attr__: adapter_attr,
        __base_attr__: base_attr,
        ..Default::default()
    };

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
    /// What `adapter.dynamic_table_create_or_alter_enabled(config.model)` returns. The gate itself
    /// (behavior flag + native catalog) is tested in `dbt-adapter`; here it is a given.
    create_or_alter: bool,
    /// The `snowflake_default_transient_dynamic_tables` behavior flag.
    default_transient: bool,
    /// The configured catalog is a managed Iceberg one (`BUILT_IN`) instead of the native default.
    iceberg_target: bool,
    /// The live relation is a dynamic Iceberg table (`SHOW OBJECTS` reports `is_iceberg`).
    iceberg_existing: bool,
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
                BTreeMap::from([("no_warn", options.default_transient)]),
            ),
        ])),
    );

    let create_or_alter = options.create_or_alter;
    harness
        .mock()
        .on("dynamic_table_create_or_alter_enabled", move |_| {
            Ok(Value::from(create_or_alter))
        });

    let iceberg_target = options.iceberg_target;
    let native_catalog_relation = Value::from_object(
        dbt_adapter::catalog_relation::CatalogRelation::default_catalog_relation_snowflake(),
    );
    harness.mock().on("build_catalog_relation", move |args| {
        if !iceberg_target {
            return Ok(native_catalog_relation.clone());
        }
        // Resolve the managed Iceberg catalog from the model config exactly as the adapter does.
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

    let existing_value = match existing {
        Some(relation_type) => {
            let mut relation = create_relation(
                ADAPTER,
                DATABASE.to_string(),
                SCHEMA.to_string(),
                Some(IDENTIFIER.to_string()),
                Some(relation_type),
                SNOWFLAKE_RESOLVED_QUOTING,
            )
            .expect("create_relation");
            if options.iceberg_existing {
                relation.set_table_format(Some(TableFormat::Iceberg));
            }
            RelationObject::new(Arc::from(relation)).into_value()
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
    let ctx = fixture
        .harness
        .materialization_context(IDENTIFIER, MODEL_SQL)
        .database(DATABASE)
        .schema(SCHEMA)
        .relation_type(RelationType::DynamicTable)
        .config(Value::from_dyn_object(dynamic_config(
            local,
            full_refresh,
            on_configuration_change,
        )))
        .build();
    fixture.harness.render(MATERIALIZATION, ctx)
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

// ---------------------------------------------------------------------------------------------
// `snowflake_dynamic_table_create_or_alter`: with the gate on, an existing native dynamic table
// syncs its definition in place via `create or alter`, so SQL edits deploy without --full-refresh.
// ---------------------------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------------------------
// Managed dynamic Iceberg tables (`BUILT_IN` catalog) under `snowflake_dynamic_table_create_or_alter`.
//
// Snowflake lets a managed dynamic Iceberg table change its dynamic-table clauses (warehouse,
// target lag, refresh mode, query, clustering) like a native one, forbids changing the Iceberg
// identity clauses (external volume, catalog, base location, Iceberg version), and cannot convert
// between a native and an Iceberg dynamic table. The Snowflake-side rejections are exercised only
// against a live account; these tests pin the DDL dbt sends and the routing around it.
// ---------------------------------------------------------------------------------------------

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

#[test]
fn create_or_alter_iceberg_deploys_the_definition_and_restates_the_identity() {
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        iceberg_options(),
    );
    render_ok(&fixture, &iceberg_local_config(), false, "apply");

    assert_eq!(fixture.executed(), vec![base_iceberg_create_or_alter_sql()]);
}

#[test]
fn create_or_alter_iceberg_applies_dynamic_table_attributes_in_place() {
    // Warehouse, target lag and clustering change under the same rules as a native table.
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            target_lag: Some("5 minutes".to_string()),
            warehouse: Some("OLD_WH".to_string()),
            ..matching_remote_state()
        },
        iceberg_options(),
    );
    let local = LocalConfig {
        cluster_by: Some(dbt_schemas::schemas::common::ClusterConfig::List(vec![
            "id".to_string(),
        ])),
        ..iceberg_local_config()
    };
    render_ok(&fixture, &local, false, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create or alter dynamic iceberg table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH {ICEBERG_IDENTITY} refresh_mode = AUTO scheduler = 'ENABLE' cluster by (id) as ( {MODEL_SQL} )"
        )]
    );
}

#[test]
fn create_or_alter_iceberg_refresh_mode_change_routes_to_replace() {
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            refresh_mode: Some("INCREMENTAL".to_string()),
            ..matching_remote_state()
        },
        iceberg_options(),
    );
    let local = LocalConfig {
        refresh_mode: Some("FULL".to_string()),
        ..iceberg_local_config()
    };
    render_ok(&fixture, &local, false, "apply");

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
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        FixtureOptions {
            iceberg_existing: false,
            ..iceberg_options()
        },
    );
    render_ok(&fixture, &iceberg_local_config(), false, "apply");

    let executed = fixture.executed();
    assert_eq!(executed.len(), 1, "got: {executed:?}");
    assert!(
        executed[0].starts_with("create or replace dynamic iceberg table"),
        "got: {executed:?}"
    );
}

#[test]
fn create_or_alter_iceberg_to_native_conversion_routes_to_replace() {
    // ... and cannot convert a dynamic Iceberg table to a regular one either.
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        FixtureOptions {
            iceberg_target: false,
            ..iceberg_options()
        },
    );
    render_ok(&fixture, &iceberg_local_config(), false, "apply");

    let executed = fixture.executed();
    assert_eq!(executed.len(), 1, "got: {executed:?}");
    assert!(
        executed[0].starts_with("create or replace dynamic table"),
        "got: {executed:?}"
    );
}

#[test]
fn create_or_alter_iceberg_does_not_apply_on_first_build_or_full_refresh() {
    let first_build = fixture_with(None, RemoteState::default(), iceberg_options());
    render_ok(&first_build, &iceberg_local_config(), false, "apply");
    assert!(
        first_build.executed()[0].starts_with("create dynamic iceberg table"),
        "got: {:?}",
        first_build.executed()
    );

    let full_refresh = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        iceberg_options(),
    );
    render_ok(&full_refresh, &iceberg_local_config(), true, "apply");
    assert!(
        full_refresh.executed()[0].starts_with("create or replace dynamic iceberg table"),
        "got: {:?}",
        full_refresh.executed()
    );
}

#[test]
fn create_or_alter_iceberg_gate_off_keeps_sql_only_edit_a_noop() {
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        FixtureOptions {
            create_or_alter: false,
            ..iceberg_options()
        },
    );
    render_ok(&fixture, &iceberg_local_config(), false, "apply");

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
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        matching_remote_state(),
        FixtureOptions {
            default_transient: true,
            ..iceberg_options()
        },
    );
    let local = LocalConfig {
        initialize: Some("ON_SCHEDULE".to_string()),
        copy_grants: Some(true),
        table_tag: Some("my_tag = 'v1'".to_string()),
        row_access_policy: Some("my_policy on (id)".to_string()),
        ..iceberg_local_config()
    };
    render_ok(&fixture, &local, false, "apply");

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
    let fixture = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            scheduler: Some("DISABLE".to_string()),
            warehouse: Some("WH".to_string()),
            ..Default::default()
        },
        iceberg_options(),
    );
    let local = LocalConfig {
        snowflake_warehouse: Some("WH".to_string()),
        iceberg: true,
        ..Default::default()
    };
    render_ok(&fixture, &local, false, "apply");

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
    let changed = RemoteState {
        target_lag: Some("5 minutes".to_string()),
        ..matching_remote_state()
    };

    let continued = fixture_with(
        Some(RelationType::DynamicTable),
        RemoteState {
            target_lag: changed.target_lag.clone(),
            ..matching_remote_state()
        },
        iceberg_options(),
    );
    render_ok(&continued, &iceberg_local_config(), false, "continue");
    assert!(continued.executed().is_empty());

    let failing = fixture_with(Some(RelationType::DynamicTable), changed, iceberg_options());
    assert!(render(&failing, &iceberg_local_config(), false, "fail").is_err());
}
