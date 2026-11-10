//! Tests for `dbt-snowflake/macros/materializations/dynamic_table.sql`.
//!
//! The materialization is driven end to end: `relation.from_config(config.model)` and
//! `relation.dynamic_table_config_changeset(...)` are the real implementations, so `config.model`
//! here is a `DbtModel` serialized exactly the way the run context serializes it, and the mocked
//! `describe_relation` returns a readback shaped like `SHOW DYNAMIC TABLES`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_agate::AgateTable;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::project::{ModelConfig, WarehouseSpecificNodeConfig};
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

    let model = DbtModel {
        deprecated_config: ModelConfig {
            __warehouse_specific_config__: wh_config,
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

/// Build a harness whose `describe_relation` returns `remote` for a dynamic-table relation.
fn fixture(existing: Option<RelationType>, remote: RemoteState) -> Fixture {
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
