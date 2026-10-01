//! Tests for `dbt-snowflake/macros/materializations/interactive_table.sql`.
//!
//! The materialization is driven end to end: `relation.from_config(config.model)` and
//! `relation.interactive_table_config_changeset(...)` are the real implementations, so `config.model`
//! here is a `DbtModel` serialized exactly the way the run context serializes it, and the mocked
//! `describe_interactive_table` returns a readback shaped like `SHOW INTERACTIVE TABLES`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_agate::AgateTable;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::{ClusterConfig, DbtMaterialization};
use dbt_schemas::schemas::project::{ModelConfig, WarehouseSpecificNodeConfig};
use dbt_schemas::schemas::{AdapterAttr, CommonAttributes, DbtModel, NodeBaseAttributes};
use dbt_yaml::Spanned;
use minijinja::Value;
use minijinja::value::{Kwargs, ValueMap};

use crate::macro_test_harness::{MacroTestHarness, default_mock_config, executed_sql};

const ADAPTER: AdapterType = AdapterType::Snowflake;

const MATERIALIZATION: &str = "{{ materialization_interactive_table_snowflake() }}";

const DATABASE: &str = "TEST_DB";
const SCHEMA: &str = "TEST_SCHEMA";
const IDENTIFIER: &str = "MY_IT";
const RENDERED_RELATION: &str = "TEST_DB.TEST_SCHEMA.MY_IT";
const MODEL_SQL: &str = "select 1 as id";

/// The columns `describe_interactive_table` returns, spelled as `SHOW INTERACTIVE TABLES`
/// reports them. A column this fixture omits reads back as unset rather than erroring, so a
/// name that drifts from the adapter's select list would phantom-diff instead of failing.
#[derive(Default)]
struct RemoteState {
    target_lag: Option<String>,
    refresh_warehouse: Option<String>,
    initialization_warehouse: Option<String>,
    cluster_by: Option<String>,
}

/// The `describe_interactive_table` return value, under the key the changeset reads it from.
fn describe_result(state: RemoteState) -> Value {
    let batch = arrow_array::record_batch!(
        ("name", Utf8, [IDENTIFIER]),
        ("schema_name", Utf8, [SCHEMA]),
        ("database_name", Utf8, [DATABASE]),
        ("text", Utf8, [""]),
        ("target_lag", Utf8, [state.target_lag]),
        ("refresh_warehouse", Utf8, [state.refresh_warehouse]),
        (
            "initialization_warehouse",
            Utf8,
            [state.initialization_warehouse]
        ),
        ("cluster_by", Utf8, [state.cluster_by])
    )
    .expect("readback batch should build");

    Value::from(ValueMap::from_iter([(
        Value::from("interactive_table"),
        Value::from_object(AgateTable::from_record_batch(Arc::new(batch))),
    )]))
}

#[derive(Default)]
struct LocalConfig {
    target_lag: Option<String>,
    snowflake_warehouse: Option<String>,
    snowflake_initialization_warehouse: Option<String>,
    cluster_by: Option<ClusterConfig>,
}

/// A serialized model node, built the way the run context builds `config.model`: the node's own
/// serialization (which is what carries `resource_type`) converted to a Jinja value map.
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
        materialized: DbtMaterialization::InteractiveTable,
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

fn interactive_config(
    local: &LocalConfig,
    full_refresh: bool,
    on_configuration_change: &'static str,
) -> Arc<MockJinjaObject> {
    let mock = default_mock_config();
    mock.on("get", move |args| {
        let key = args.first().and_then(|v| v.as_str());
        let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
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
    /// What `store_raw_result` recorded, as `name=code`.
    raw_results: Recorder,
    /// What `log()` recorded. The shared relation dispatchers announce which operation they are
    /// applying, which is how a routing decision is observed without depending on the SQL the
    /// dispatcher goes on to produce.
    logs: Recorder,
}

impl Fixture {
    /// The executed statements, whitespace-normalized and with block comments removed. Several of
    /// the relation macros carry a `/* ... */` docstring into the statement they emit; dropping it
    /// lets the assertions below compare whole statements against the DDL as written.
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

    fn logged(&self, needle: &str) -> bool {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains(needle))
    }

    fn describe_calls(&self) -> usize {
        self.harness
            .mock()
            .observed_calls()
            .to("describe_relation")
            .count()
    }
}

/// Collapse rendered whitespace to single spaces, so assertions read as the DDL does. Token
/// boundaries survive, so a clause accidentally glued to the one before it still fails.
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
    let log_sink = logs.clone();
    harness
        .env_mut()
        .env
        .add_function("log", move |msg: Value| {
            log_sink.lock().unwrap().push(msg.to_string());
            Ok(Value::UNDEFINED)
        });

    harness.mock().set_attr(
        "behavior",
        Value::from_serialize(BTreeMap::from([(
            "use_catalogs_v2",
            BTreeMap::from([("no_warn", false)]),
        )])),
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
        logs,
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
        .relation_type(RelationType::InteractiveTable)
        .config(Value::from_dyn_object(interactive_config(
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

/// A refreshing interactive table, the configuration most of these tests use.
fn dynamic_local_config() -> LocalConfig {
    LocalConfig {
        target_lag: Some("1 minute".to_string()),
        snowflake_warehouse: Some("WH".to_string()),
        ..Default::default()
    }
}

/// The readback of a table already matching [`dynamic_local_config`].
fn matching_remote_state() -> RemoteState {
    RemoteState {
        target_lag: Some("1 minute".to_string()),
        refresh_warehouse: Some("WH".to_string()),
        ..Default::default()
    }
}

#[test]
fn no_existing_relation_creates_the_interactive_table() {
    let fixture = fixture(None, RemoteState::default());
    render_ok(&fixture, &dynamic_local_config(), false, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create interactive table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH as ( {MODEL_SQL} )"
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
    let fixture = fixture(
        Some(RelationType::InteractiveTable),
        matching_remote_state(),
    );
    render_ok(&fixture, &dynamic_local_config(), true, "apply");

    assert_eq!(
        fixture.executed(),
        vec![format!(
            "create or replace interactive table {RENDERED_RELATION} target_lag = '1 minute' warehouse = WH as ( {MODEL_SQL} )"
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
fn existing_relation_of_another_type_is_replaced() {
    // Both the intermediate and the final target are interactive tables, so the in-place rename
    // still goes through the plain-table `alter table ... rename to ...` form.
    let fixture = fixture(Some(RelationType::Table), matching_remote_state());
    render_ok(&fixture, &dynamic_local_config(), false, "apply");

    let backup = format!("{RENDERED_RELATION}__dbt_backup");
    let intermediate = format!("{RENDERED_RELATION}__dbt_tmp");
    assert_eq!(
        fixture.executed(),
        vec![format!(
            "-- get the standard intermediate name \
             -- drop any pre-existing intermediate \
             drop table if exists {intermediate} ; \
             create interactive table {intermediate} target_lag = '1 minute' warehouse = WH as ( {MODEL_SQL} ) ; \
             -- get the standard backup name \
             -- drop any pre-existing backup \
             drop table if exists {backup} cascade ; \
             -- use `render` to ensure that the fully qualified name is used \
             alter table {RENDERED_RELATION} rename to {backup} ; \
             -- get the standard intermediate name \
             -- use `render` to ensure that the fully qualified name is used \
             alter table {intermediate} rename to {RENDERED_RELATION} ; \
             -- get the standard backup name \
             drop table if exists {backup} cascade"
        )],
        "the cross-type replace swap must fully render now that the rename router exists"
    );

    assert!(
        fixture.logged("Applying REPLACE to:"),
        "a non-interactive existing relation must go to the replace path"
    );
    assert!(
        !fixture.logged("Applying ALTER to:"),
        "a non-interactive existing relation must not be altered"
    );
    assert_eq!(
        fixture.describe_calls(),
        0,
        "replacing must not depend on reading interactive-table state"
    );
    assert!(
        !fixture
            .executed()
            .iter()
            .any(|sql| sql.contains("alter interactive table")),
        "no in-place alter may be issued, got: {:?}",
        fixture.executed()
    );
}

#[test]
fn existing_interactive_table_with_changes_is_altered() {
    let fixture = fixture(
        Some(RelationType::InteractiveTable),
        RemoteState {
            target_lag: Some("1 minute".to_string()),
            refresh_warehouse: Some("WH".to_string()),
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
            "alter interactive table {RENDERED_RELATION} set target_lag = '2 minutes'"
        )],
        "an alterable change must be applied in place"
    );
    assert_eq!(fixture.describe_calls(), 1);
}

#[test]
fn cluster_by_change_is_replaced_not_altered() {
    // Snowflake rejects `alter ... cluster by` on an interactive table, so the changeset marks
    // this as needing a full rebuild. The materialization must not second-guess that flag.
    let fixture = fixture(
        Some(RelationType::InteractiveTable),
        RemoteState {
            target_lag: Some("1 minute".to_string()),
            refresh_warehouse: Some("WH".to_string()),
            cluster_by: Some("(id, val)".to_string()),
            ..Default::default()
        },
    );
    let local = LocalConfig {
        target_lag: Some("1 minute".to_string()),
        snowflake_warehouse: Some("WH".to_string()),
        cluster_by: Some(ClusterConfig::List(vec![
            "id".to_string(),
            "other".to_string(),
        ])),
        ..Default::default()
    };
    render_ok(&fixture, &local, false, "apply");

    let executed = fixture.executed();
    assert!(
        executed
            .iter()
            .any(|sql| sql.contains("create or replace interactive table")),
        "a cluster_by change must rebuild the relation, got: {executed:?}"
    );
    assert!(
        !executed
            .iter()
            .any(|sql| sql.starts_with("alter interactive table")),
        "a cluster_by change must not be altered in place, got: {executed:?}"
    );
}

#[test]
fn newly_set_target_lag_is_replaced_not_altered() {
    // A static interactive table cannot be turned into a refreshing one in place (001420). The
    // force reaches the macros only through the changeset, so this pins the pass-through.
    let fixture = fixture(Some(RelationType::InteractiveTable), RemoteState::default());
    render_ok(&fixture, &dynamic_local_config(), false, "apply");

    let executed = fixture.executed();
    assert!(
        executed
            .iter()
            .any(|sql| sql.contains("create or replace interactive table")),
        "adding target_lag must rebuild the relation, got: {executed:?}"
    );
    assert!(
        !executed
            .iter()
            .any(|sql| sql.starts_with("alter interactive table")),
        "adding target_lag must not be altered in place, got: {executed:?}"
    );
}

#[test]
fn on_configuration_change_fail_raises() {
    let fixture = fixture(
        Some(RelationType::InteractiveTable),
        RemoteState {
            target_lag: Some("1 minute".to_string()),
            refresh_warehouse: Some("WH".to_string()),
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
        Some(RelationType::InteractiveTable),
        RemoteState {
            target_lag: Some("1 minute".to_string()),
            refresh_warehouse: Some("WH".to_string()),
            ..Default::default()
        },
    );
    let local = LocalConfig {
        target_lag: Some("2 minutes".to_string()),
        snowflake_warehouse: Some("WH".to_string()),
        ..Default::default()
    };

    // Unlike `fail`, this renders successfully; unlike `apply`, it emits no statement.
    render_ok(&fixture, &local, false, "continue");
    assert!(
        fixture.executed().is_empty(),
        "`continue` must not build, got: {:?}",
        fixture.executed()
    );
    assert_eq!(fixture.raw_results(), vec!["main=skip".to_string()]);
}

#[test]
fn no_changes_records_a_skip() {
    let fixture = fixture(
        Some(RelationType::InteractiveTable),
        matching_remote_state(),
    );
    render_ok(&fixture, &dynamic_local_config(), false, "apply");

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

/// `transient interactive table` is not valid DDL, and there is no condition under which an
/// interactive table should be refreshed by dbt. Neither word may appear on any path that emits
/// SQL. The no-op path is absent because it executes nothing at all; see
/// [`no_changes_records_a_skip`].
#[test]
fn no_path_emits_transient_or_refresh() {
    let paths: Vec<(&str, Option<RelationType>, RemoteState, LocalConfig, bool)> = vec![
        (
            "create",
            None,
            RemoteState::default(),
            LocalConfig {
                snowflake_initialization_warehouse: Some("INIT_WH".to_string()),
                ..dynamic_local_config()
            },
            false,
        ),
        (
            "full refresh",
            Some(RelationType::InteractiveTable),
            matching_remote_state(),
            dynamic_local_config(),
            true,
        ),
        (
            "alter",
            Some(RelationType::InteractiveTable),
            matching_remote_state(),
            LocalConfig {
                target_lag: Some("2 minutes".to_string()),
                snowflake_warehouse: Some("WH".to_string()),
                ..Default::default()
            },
            false,
        ),
    ];

    for (label, existing, remote, local, full_refresh) in paths {
        let fixture = fixture(existing, remote);
        render_ok(&fixture, &local, full_refresh, "apply");
        let executed = fixture.executed_raw();
        assert!(
            !executed.is_empty(),
            "the {label} path executed nothing, so this assertion would be vacuous"
        );
        for sql in &executed {
            let lower = sql.to_lowercase();
            assert!(
                !lower.contains("transient"),
                "the {label} path emitted `transient`: {sql}"
            );
            assert!(
                !lower.contains("refresh"),
                "the {label} path emitted `refresh`: {sql}"
            );
        }
    }
}
