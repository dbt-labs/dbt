use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, assert_executed_contains, default_mock_config};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn view_macro_name(adapter_type: AdapterType) -> &'static str {
    match adapter_type {
        AdapterType::Snowflake => "materialization_view_snowflake",
        AdapterType::Databricks => "materialization_view_databricks",
        AdapterType::Bigquery => "materialization_view_bigquery",
        AdapterType::Redshift => "materialization_view_redshift",
        AdapterType::Postgres => "materialization_view_default",
        other => panic!("unsupported adapter for view materialization test: {other:?}"),
    }
}

fn build_view_harness(adapter_type: AdapterType) -> MacroTestHarness {
    let harness = MacroTestHarness::for_adapter(adapter_type)
        .load_all_macros()
        .with_stub_functions()
        .with_behavior_flag("use_materialization_v2", false)
        .build()
        .expect("harness should build");

    let mock = harness.mock();
    mock.on("rename_relation", |_| Ok(Value::UNDEFINED));
    mock.on("drop_relation", |_| Ok(Value::UNDEFINED));
    mock.on("commit", |_| Ok(Value::UNDEFINED));
    mock.on("clean_sql", |args| {
        Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
    });
    mock.on("get_column_tags_from_model", |_| Ok(Value::UNDEFINED));
    mock.on("get_view_options", |_| {
        Ok(Value::from(BTreeMap::<String, Value>::new()))
    });
    mock.on("resolve_file_format", |_| Ok(Value::from("delta")));
    mock.on("is_uniform", |_| Ok(Value::from(false)));
    mock.on("has_dbr_capability", |_| Ok(Value::from(false)));

    harness
}

fn render_view(
    harness: &MacroTestHarness,
    adapter_type: AdapterType,
    ctx: BTreeMap<String, Value>,
) -> dbt_common::FsResult<String> {
    let call = format!("{{{{ {}() }}}}", view_macro_name(adapter_type));
    harness.render(&call, ctx)
}

fn full_refresh_mock_config() -> Arc<MockJinjaObject> {
    let mock = default_mock_config();
    mock.on("get", |args| {
        let key = args.first().and_then(|v| v.as_str());
        let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
        match key {
            Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                "enforced".to_string(),
                Value::from(false),
            )]))),
            Some("full_refresh") => Ok(Value::from(true)),
            _ => Ok(default),
        }
    });
    mock
}

fn v2_update_via_alter_config(full_refresh: bool) -> Arc<MockJinjaObject> {
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
            Some("view_update_via_alter") => Ok(Value::from(true)),
            _ => Ok(default),
        }
    });
    mock
}

// Scenarios:
// 1. Nothing already exists under the target namespace
// 2. A view already exists under the target namespace
// 3. A table already exists under the target namespace

/// Shorthand: build harness, mock get_relation → None, render, assert success.
fn run_no_existing(adapter_type: AdapterType) -> MacroTestHarness {
    let harness = build_view_harness(adapter_type);
    harness.mock().on("get_relation", |_| Ok(Value::from(())));
    let ctx = harness
        .materialization_context("my_view", "SELECT id, name FROM source_table")
        .build();
    render_view(&harness, adapter_type, ctx)
        .unwrap_or_else(|e| panic!("{adapter_type:?} view materialization failed: {e:?}"));
    harness
}

/// Shorthand: build harness, mock get_relation → existing view, render, assert success.
fn run_existing_view(adapter_type: AdapterType) -> MacroTestHarness {
    let harness = build_view_harness(adapter_type);
    let existing = harness.relation(
        "TEST_DB",
        "TEST_SCHEMA",
        "my_view",
        Some(RelationType::View),
    );
    harness.mock().on("get_relation", move |_| {
        Ok(RelationObject::new(Arc::clone(&existing)).into_value())
    });
    let ctx = harness
        .materialization_context("my_view", "SELECT id, name FROM source_table")
        .build();
    render_view(&harness, adapter_type, ctx)
        .unwrap_or_else(|e| panic!("{adapter_type:?} view materialization failed: {e:?}"));
    harness
}

/// Shorthand: build harness, mock get_relation → existing table, render with
/// full_refresh config, assert success.
fn run_existing_table(adapter_type: AdapterType) -> MacroTestHarness {
    let harness = build_view_harness(adapter_type);
    let existing = harness.relation(
        "TEST_DB",
        "TEST_SCHEMA",
        "my_view",
        Some(RelationType::Table),
    );
    harness.mock().on("get_relation", move |_| {
        Ok(RelationObject::new(Arc::clone(&existing)).into_value())
    });
    harness.mock().on("drop_relation", |_| Ok(Value::from(())));
    let ctx = harness
        .materialization_context("my_view", "SELECT id, name FROM source_table")
        .config(Value::from_dyn_object(full_refresh_mock_config()))
        .build();
    render_view(&harness, adapter_type, ctx)
        .unwrap_or_else(|e| panic!("{adapter_type:?} view materialization failed: {e:?}"));
    harness
}

mod snowflake {
    use super::*;
    const ADAPTER: AdapterType = AdapterType::Snowflake;

    #[test]
    fn no_existing_relation() {
        let h = run_no_existing(ADAPTER);
        h.mock().observed_calls().assert_not_called("drop_relation");
        assert_executed_contains(h.mock(), "create or replace");
    }

    #[test]
    fn existing_view_replaced_without_drop() {
        let h = run_existing_view(ADAPTER);
        h.mock().observed_calls().assert_not_called("drop_relation");
        assert_executed_contains(h.mock(), "create or replace");
    }

    #[test]
    fn existing_table_dropped_before_create() {
        let h = run_existing_table(ADAPTER);
        h.mock().observed_calls().assert_called("drop_relation");
        assert_executed_contains(h.mock(), "create or replace");
    }
}

mod databricks {
    use super::*;
    const ADAPTER: AdapterType = AdapterType::Databricks;

    // -- use_materialization_v2 = false, default --------------------

    #[test]
    fn no_existing_relation() {
        let h = run_no_existing(ADAPTER);
        h.mock().observed_calls().assert_not_called("drop_relation");
        assert_executed_contains(h.mock(), "create or replace");
    }

    #[test]
    fn existing_view_replaced_without_drop() {
        let h = run_existing_view(ADAPTER);
        h.mock().observed_calls().assert_not_called("drop_relation");
        assert_executed_contains(h.mock(), "create or replace");
    }

    #[test]
    fn existing_table_dropped_before_create() {
        let h = run_existing_table(ADAPTER);
        h.mock().observed_calls().assert_called("drop_relation");
        assert_executed_contains(h.mock(), "create or replace");
    }

    // -- use_materialization_v2 = true ----------------------------------

    #[test]
    fn v2_no_existing_relation() {
        let harness = build_view_harness(ADAPTER);
        harness.set_behavior_flags([("use_materialization_v2", true)]);
        harness.mock().on("get_relation", |_| Ok(Value::from(())));

        let ctx = harness
            .materialization_context("my_view", "SELECT id, name FROM source_table")
            .build();
        render_view(&harness, ADAPTER, ctx).expect("v2 materialization should succeed");

        harness
            .mock()
            .observed_calls()
            .assert_not_called("drop_relation");
        assert_executed_contains(harness.mock(), "create or replace");
    }

    #[test]
    fn v2_existing_view_replaced() {
        let harness = build_view_harness(ADAPTER);
        harness.set_behavior_flags([("use_materialization_v2", true)]);
        let existing = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "my_view",
            Some(RelationType::View),
        );
        harness.mock().on("get_relation", move |_| {
            Ok(RelationObject::new(Arc::clone(&existing)).into_value())
        });

        let ctx = harness
            .materialization_context("my_view", "SELECT id, name FROM source_table")
            .build();
        render_view(&harness, ADAPTER, ctx).expect("v2 materialization should succeed");

        harness
            .mock()
            .observed_calls()
            .assert_not_called("drop_relation");
        assert_executed_contains(harness.mock(), "create");
    }

    #[test]
    fn v2_full_refresh_replaces_view_when_update_via_alter_is_enabled() {
        let mut harness = build_view_harness(ADAPTER);
        harness
            .env_mut()
            .env
            .add_function("store_raw_result", |_kwargs: minijinja::value::Kwargs| {
                Ok(Value::UNDEFINED)
            });
        harness.set_behavior_flags([("use_materialization_v2", true)]);
        let existing = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "my_view",
            Some(RelationType::View),
        );
        harness.mock().on("get_relation", move |_| {
            Ok(RelationObject::new(Arc::clone(&existing)).into_value())
        });
        harness
            .mock()
            .on("get_relation_config", |_| Ok(Value::UNDEFINED));
        let model_config = Arc::new(MockJinjaObject::new());
        model_config.on("get_changeset", |_| Ok(Value::UNDEFINED));
        harness.mock().on("get_config_from_model", move |_| {
            Ok(Value::from_dyn_object(Arc::clone(&model_config)))
        });

        let ctx = harness
            .materialization_context("my_view", "SELECT id, name FROM source_table")
            .config(Value::from_dyn_object(v2_update_via_alter_config(true)))
            .build();
        render_view(&harness, ADAPTER, ctx).expect("v2 full refresh should replace the view");

        assert_executed_contains(harness.mock(), "create or replace");
    }

    #[test]
    fn v2_update_via_alter_rejects_hive_metastore_views() {
        let harness = build_view_harness(ADAPTER);
        harness.set_behavior_flags([("use_materialization_v2", true)]);
        let existing = harness.relation(
            "hive_metastore",
            "TEST_SCHEMA",
            "my_view",
            Some(RelationType::View),
        );
        harness.mock().on("get_relation", move |_| {
            Ok(RelationObject::new(Arc::clone(&existing)).into_value())
        });

        let ctx = harness
            .materialization_context("my_view", "SELECT id, name FROM source_table")
            .config(Value::from_dyn_object(v2_update_via_alter_config(false)))
            .build();
        let error = render_view(&harness, ADAPTER, ctx)
            .expect_err("Hive Metastore views must reject update-via-alter");

        assert!(
            error
                .to_string()
                .contains("Cannot update a view in the Hive metastore via ALTER VIEW")
        );
    }
}

mod bigquery {
    use super::*;
    const ADAPTER: AdapterType = AdapterType::Bigquery;

    #[test]
    fn no_existing_relation() {
        let h = run_no_existing(ADAPTER);
        h.mock().observed_calls().assert_not_called("drop_relation");
        assert_executed_contains(h.mock(), "create or replace");
    }

    #[test]
    fn existing_view_replaced_without_drop() {
        let h = run_existing_view(ADAPTER);
        h.mock().observed_calls().assert_not_called("drop_relation");
        assert_executed_contains(h.mock(), "create or replace");
    }

    #[test]
    fn existing_table_dropped_before_create() {
        let h = run_existing_table(ADAPTER);
        h.mock().observed_calls().assert_called("drop_relation");
        assert_executed_contains(h.mock(), "create or replace");
    }
}

mod redshift {
    use super::*;
    const ADAPTER: AdapterType = AdapterType::Redshift;

    #[test]
    fn no_existing_relation() {
        let h = run_no_existing(ADAPTER);
        h.mock().observed_calls().assert_called("rename_relation");
        assert_executed_contains(h.mock(), "create");
    }

    #[test]
    fn existing_view_swapped_via_rename() {
        let h = run_existing_view(ADAPTER);
        h.mock().observed_calls().assert_called("rename_relation");
        assert_executed_contains(h.mock(), "create");
    }

    #[test]
    fn existing_table_swapped_via_rename() {
        let h = run_existing_table(ADAPTER);
        h.mock().observed_calls().assert_called("rename_relation");
        assert_executed_contains(h.mock(), "create");
    }
}

mod postgres {
    use super::*;
    const ADAPTER: AdapterType = AdapterType::Postgres;

    #[test]
    fn no_existing_relation() {
        let h = run_no_existing(ADAPTER);
        h.mock().observed_calls().assert_called("rename_relation");
        assert_executed_contains(h.mock(), "create");
    }

    #[test]
    fn existing_view_swapped_via_rename() {
        let h = run_existing_view(ADAPTER);
        h.mock().observed_calls().assert_called("rename_relation");
        assert_executed_contains(h.mock(), "create");
    }

    #[test]
    fn existing_table_swapped_via_rename() {
        let h = run_existing_table(ADAPTER);
        h.mock().observed_calls().assert_called("rename_relation");
        assert_executed_contains(h.mock(), "create");
    }
}
