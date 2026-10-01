use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, assert_executed_contains, default_mock_config};

fn mv_macro_name(adapter_type: AdapterType) -> &'static str {
    match adapter_type {
        AdapterType::Databricks => "materialization_materialized_view_databricks",
        other => panic!("unsupported adapter for materialized view test: {other:?}"),
    }
}

fn alter_mv_macro_name(adapter_type: AdapterType) -> &'static str {
    match adapter_type {
        AdapterType::Databricks => "databricks__get_alter_materialized_view_as_sql",
        other => panic!("unsupported adapter for MV alter test: {other:?}"),
    }
}

fn render_mv(
    harness: &MacroTestHarness,
    adapter_type: AdapterType,
    ctx: BTreeMap<String, Value>,
) -> dbt_common::FsResult<String> {
    let call = format!("{{{{ {}() }}}}", mv_macro_name(adapter_type));
    harness.render(&call, ctx)
}

fn mv_config() -> Arc<MockJinjaObject> {
    let mock = default_mock_config();
    mock.on("get", |args| {
        let key = args.first().and_then(|v| v.as_str());
        let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
        match key {
            Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                "enforced".to_string(),
                Value::from(false),
            )]))),
            Some("full_refresh") => Ok(Value::from(false)),
            Some("on_configuration_change") => Ok(Value::from("apply")),
            _ => Ok(default),
        }
    });
    mock
}

/// Build a mock `configuration_changes` value
fn config_changes_mock(requires_full_refresh: bool, changes: BTreeMap<&str, Value>) -> Value {
    let mock = Arc::new(MockJinjaObject::new());
    mock.set_attr("requires_full_refresh", Value::from(requires_full_refresh));
    mock.set_attr("changes", Value::from_serialize(changes));
    Value::from_dyn_object(mock)
}

mod databricks {
    use super::*;

    const ADAPTER: AdapterType = AdapterType::Databricks;

    fn build_harness() -> MacroTestHarness {
        let harness = MacroTestHarness::for_adapter(ADAPTER)
            .load_all_macros()
            .with_stub_functions()
            .with_behavior_flag("use_materialization_v2", false)
            .build()
            .expect("harness should build");

        let mock = harness.mock();
        mock.on("clean_sql", |args| {
            Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
        });
        mock.on("get_column_tags_from_model", |_| Ok(Value::UNDEFINED));
        mock.on("drop_relation", |_| Ok(Value::UNDEFINED));
        mock.on("rename_relation", |_| Ok(Value::UNDEFINED));
        mock.on("commit", |_| Ok(Value::UNDEFINED));
        mock.on("resolve_file_format", |_| Ok(Value::from("delta")));
        mock.on("is_uniform", |_| Ok(Value::from(false)));
        mock.on("has_dbr_capability", |_| Ok(Value::from(false)));
        mock.on("get_relation_config", |_| Ok(Value::UNDEFINED));
        mock.on("get_columns_in_relation", |_| {
            Ok(Value::from(Vec::<Value>::new()))
        });
        mock.on("parse_columns_and_constraints", |_| {
            Ok(Value::from(vec![
                Value::from(Vec::<Value>::new()),
                Value::UNDEFINED,
            ]))
        });

        let refresh = Value::from_serialize(BTreeMap::from([
            ("cron", Value::UNDEFINED),
            ("time_zone_value", Value::UNDEFINED),
        ]));
        let model_config = Arc::new(MockJinjaObject::new());
        model_config.set_attr(
            "partitioned_by",
            Value::from_serialize(BTreeMap::from([("partition_by", Value::UNDEFINED)])),
        );
        model_config.set_attr(
            "tblproperties",
            Value::from_serialize(BTreeMap::from([("tblproperties", Value::UNDEFINED)])),
        );
        model_config.set_attr(
            "comment",
            Value::from_serialize(BTreeMap::from([("comment", Value::UNDEFINED)])),
        );
        model_config.set_attr("refresh", refresh);
        let model_config_val = Value::from_dyn_object(model_config);
        mock.on("get_config_from_model", move |_| {
            Ok(model_config_val.clone())
        });

        harness
    }

    fn render_alter(harness: &MacroTestHarness, changes: Value) -> String {
        let relation = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "my_mv",
            Some(RelationType::MaterializedView),
        );
        let existing = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "my_mv",
            Some(RelationType::MaterializedView),
        );
        let mut ctx = harness
            .materialization_context("my_mv", "SELECT 1")
            .relation_type(RelationType::MaterializedView)
            .config(Value::from_dyn_object(mv_config()))
            .with("relation", RelationObject::new(relation).into_value())
            .with("changes", changes)
            .with("sql_val", Value::from("SELECT 1"))
            .with("existing", RelationObject::new(existing).into_value())
            .build();
        ctx.insert(
            "model".to_string(),
            Value::from_serialize(BTreeMap::from([
                ("alias", Value::from("my_mv")),
                ("unique_id", Value::from("model.test_project.my_mv")),
                ("columns", Value::from(BTreeMap::<String, Value>::new())),
                ("constraints", Value::from(Vec::<Value>::new())),
            ])),
        );
        let macro_name = alter_mv_macro_name(ADAPTER);
        let call = format!(
            "{{% set r = {macro_name}(relation, changes, sql_val, existing, none, none) %}}{{{{ r }}}}"
        );
        harness
            .render(&call, ctx)
            .unwrap_or_else(|e| panic!("alter MV macro failed: {e:?}"))
    }

    #[test]
    fn no_existing_relation_creates_mv() {
        let harness = build_harness();
        harness.mock().on("get_relation", |_| Ok(Value::from(())));

        let ctx = harness
            .materialization_context("my_mv", "SELECT id, name FROM source")
            .relation_type(RelationType::MaterializedView)
            .config(Value::from_dyn_object(mv_config()))
            .build();

        render_mv(&harness, ADAPTER, ctx)
            .unwrap_or_else(|e| panic!("MV materialization failed: {e:?}"));

        assert_executed_contains(harness.mock(), "create or replace materialized view");
    }

    #[test]
    fn alter_full_refresh_with_partition_by_uses_drop_and_create() {
        let h = build_harness();
        let changes = config_changes_mock(
            true,
            BTreeMap::from([
                ("partition_by", Value::from(true)),
                ("tags", Value::UNDEFINED),
            ]),
        );
        let result = render_alter(&h, changes);
        let lower = result.to_lowercase();

        assert!(
            lower.contains("drop"),
            "Expected DROP SQL from drop_and_create path, got: {result}",
        );
        assert!(
            lower.contains("create"),
            "Expected CREATE SQL from drop_and_create path, got: {result}",
        );
    }

    #[test]
    fn alter_full_refresh_without_partition_by_uses_replace() {
        let h = build_harness();
        let changes =
            config_changes_mock(true, BTreeMap::from([("tags", Value::from("some_tag"))]));
        let result = render_alter(&h, changes);

        h.mock().observed_calls().assert_not_called("drop_relation");
        assert!(
            result.to_lowercase().contains("create or replace"),
            "Expected CREATE OR REPLACE when no partition_by change, got: {result}",
        );
    }

    #[test]
    fn alter_refresh_schedule_without_full_refresh() {
        let h = build_harness();
        let changes = config_changes_mock(
            false,
            BTreeMap::from([
                (
                    "refresh",
                    Value::from_serialize(BTreeMap::from([
                        ("cron", Value::from("0 0 * * *")),
                        ("time_zone_value", Value::from("UTC")),
                        ("is_altered", Value::from(true)),
                    ])),
                ),
                ("tags", Value::UNDEFINED),
            ]),
        );
        let result = render_alter(&h, changes);

        assert!(
            result.to_uppercase().contains("ALTER MATERIALIZED VIEW"),
            "Expected ALTER statement, got: {result}",
        );
    }

    #[test]
    fn alter_no_changes_returns_empty_list() {
        let h = build_harness();
        let changes = config_changes_mock(
            false,
            BTreeMap::from([("refresh", Value::UNDEFINED), ("tags", Value::UNDEFINED)]),
        );
        let result = render_alter(&h, changes);

        assert!(
            !result.contains("ALTER MATERIALIZED VIEW"),
            "Should not ALTER without changes, got: {result}",
        );
        assert!(
            !result.to_lowercase().contains("drop"),
            "Should not DROP without changes, got: {result}",
        );
    }
}
