use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::{
    MacroTestHarness, assert_executed_contains, default_mock_config, executed_sql,
};

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

fn update_via_alter_config(
    full_refresh: bool,
    use_materialization_v2: Option<bool>,
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
            Some("view_update_via_alter") => Ok(Value::from(true)),
            Some("use_materialization_v2") => {
                Ok(use_materialization_v2.map(Value::from).unwrap_or(default))
            }
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
    fn existing_table_replaced_after_view_is_staged() {
        let h = run_existing_table(ADAPTER);
        let executed = executed_sql(h.mock());
        let first = executed
            .first()
            .expect("view materialization should execute SQL");
        assert!(
            first.contains("create or replace view") && first.contains("my_view__dbt_stg"),
            "view must be created at the staging relation before the table is touched: {executed:?}"
        );
        h.mock().observed_calls().assert_called("rename_relation");
        assert_executed_contains(h.mock(), "rename to");
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

    fn existing_view_alter_harness(project_flag: bool) -> MacroTestHarness {
        let mut harness = build_view_harness(ADAPTER);
        harness
            .env_mut()
            .env
            .add_function("store_raw_result", |_kwargs: minijinja::value::Kwargs| {
                Ok(Value::UNDEFINED)
            });
        harness.set_behavior_flags([("use_materialization_v2", project_flag)]);
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
        harness
    }

    #[test]
    fn v2_full_refresh_replaces_view_when_update_via_alter_is_enabled() {
        let harness = existing_view_alter_harness(true);

        let ctx = harness
            .materialization_context("my_view", "SELECT id, name FROM source_table")
            .config(Value::from_dyn_object(update_via_alter_config(true, None)))
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
            .config(Value::from_dyn_object(update_via_alter_config(false, None)))
            .build();
        let error = render_view(&harness, ADAPTER, ctx)
            .expect_err("Hive Metastore views must reject update-via-alter");

        assert!(
            error
                .to_string()
                .contains("Cannot update a view in the Hive metastore via ALTER VIEW")
        );
    }

    #[test]
    fn v2_update_via_alter_replaces_metric_view_with_view() {
        // Converting a metric view into an ordinary view must replace it, never
        // take the in-place ALTER path, even with view_update_via_alter enabled:
        // the relation types differ, so the type gate short-circuits ALTER.
        let harness = build_view_harness(ADAPTER);
        harness.set_behavior_flags([("use_materialization_v2", true)]);
        let existing = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "my_view",
            Some(RelationType::MetricView),
        );
        harness.mock().on("get_relation", move |_| {
            Ok(RelationObject::new(Arc::clone(&existing)).into_value())
        });

        let ctx = harness
            .materialization_context("my_view", "SELECT id, name FROM source_table")
            .config(Value::from_dyn_object(update_via_alter_config(false, None)))
            .build();
        render_view(&harness, ADAPTER, ctx)
            .expect("metric_view to view must replace instead of ALTER");

        // A recreate proves replacement; a broken gate would ALTER and skip it.
        assert_executed_contains(harness.mock(), "create or replace view");
    }

    // -- model-level use_materialization_v2 overrides the project flag ------

    fn render_existing_view_with_model_setting(
        project_flag: bool,
        model_setting: bool,
    ) -> MacroTestHarness {
        let harness = existing_view_alter_harness(project_flag);
        let ctx = harness
            .materialization_context("my_view", "SELECT id, name FROM source_table")
            .config(Value::from_dyn_object(update_via_alter_config(
                false,
                Some(model_setting),
            )))
            .build();
        render_view(&harness, ADAPTER, ctx).expect("view materialization should succeed");
        harness
    }

    #[test]
    fn model_setting_opts_into_v2_when_project_flag_is_off() {
        let h = render_existing_view_with_model_setting(false, true);
        // Only the v2 path diffs model config against the existing view.
        h.mock()
            .observed_calls()
            .assert_called("get_config_from_model");
    }

    #[test]
    fn model_setting_opts_out_of_v2_when_project_flag_is_on() {
        let h = render_existing_view_with_model_setting(true, false);
        h.mock()
            .observed_calls()
            .assert_not_called("get_config_from_model");
        assert_executed_contains(h.mock(), "create or replace view");
    }

    // -- ALTER path validation and column comments ---------------------------

    fn update_via_alter_with_row_filter_config() -> Arc<MockJinjaObject> {
        let mock = default_mock_config();
        mock.on("get", |args| {
            let key = args.first().and_then(|v| v.as_str());
            let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
            match key {
                Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                    "enforced".to_string(),
                    Value::from(false),
                )]))),
                Some("view_update_via_alter") => Ok(Value::from(true)),
                Some("row_filter") => Ok(Value::from_serialize(BTreeMap::from([
                    ("function", Value::from("filter_id")),
                    ("columns", Value::from_serialize(["id"])),
                ]))),
                _ => Ok(default),
            }
        });
        mock
    }

    fn model_with_masked_column() -> Value {
        Value::from_serialize(BTreeMap::from([
            ("alias", Value::from("my_view")),
            ("unique_id", Value::from("model.test_project.my_view")),
            (
                "columns",
                Value::from_serialize(BTreeMap::from([(
                    "msg",
                    BTreeMap::from([("column_mask", BTreeMap::from([("function", "mask_msg")]))]),
                )])),
            ),
        ]))
    }

    #[test]
    fn v2_update_via_alter_rejects_unsupported_configs_without_changes() {
        let cases = [
            (
                "row filter",
                Value::from_dyn_object(update_via_alter_with_row_filter_config()),
                None,
                "Row filters are not supported for views.",
            ),
            (
                "column mask",
                Value::from_dyn_object(update_via_alter_config(false, None)),
                Some(model_with_masked_column()),
                "Column masks are not supported for views.",
            ),
        ];
        for (case, config, model, expected) in cases {
            let harness = existing_view_alter_harness(true);
            let mut builder = harness
                .materialization_context("my_view", "SELECT id, msg FROM source_table")
                .config(config);
            if let Some(model) = model {
                builder = builder.with("model", model);
            }
            let error = render_view(&harness, ADAPTER, builder.build())
                .expect_err("an existing view with no changes must still be validated");
            assert!(
                error.to_string().contains(expected),
                "{case}: unexpected error {error}"
            );
        }
    }

    fn contract_enforced_config() -> Arc<MockJinjaObject> {
        let mock = default_mock_config();
        mock.on("get", |args| {
            let key = args.first().and_then(|v| v.as_str());
            let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
            match key {
                Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                    "enforced".to_string(),
                    Value::from(true),
                )]))),
                _ => Ok(default),
            }
        });
        mock
    }

    #[test]
    fn enforced_contract_is_not_asserted_under_clone() {
        for (command, asserted) in [("run", true), ("clone", false)] {
            let harness = build_view_harness(ADAPTER);
            harness.mock().on("get_relation", |_| Ok(Value::from(())));
            let ctx = harness
                .materialization_context("my_view", "select * from prod_schema.my_view")
                .config(Value::from_dyn_object(contract_enforced_config()))
                .with(
                    "flags",
                    Value::from_serialize(BTreeMap::from([("WHICH", command)])),
                )
                .build();
            let result = render_view(&harness, ADAPTER, ctx);
            if asserted {
                let error = result.expect_err("run must assert the enforced contract");
                assert!(
                    error.to_string().contains("enforced contract"),
                    "{command}: unexpected error {error}"
                );
            } else {
                result.unwrap_or_else(|e| panic!("{command}: clone must skip the contract: {e}"));
                assert_executed_contains(harness.mock(), "create or replace view");
            }
        }
    }

    #[test]
    fn alter_view_query_change_reapplies_post_alter_comments_only() {
        let harness = build_view_harness(ADAPTER);
        harness
            .mock()
            .on("resolve_file_format", |_| Ok(Value::from("parquet")));
        harness.mock().on("has_feature", |_| Ok(Value::from(true)));
        harness.mock().on("quote", |args| {
            let name = args.first().and_then(|v| v.as_str()).unwrap_or_default();
            Ok(Value::from(format!("`{name}`")))
        });
        harness.mock().on("get_columns_in_relation", |_| {
            Ok(Value::from(Vec::<Value>::new()))
        });
        let id_column = BTreeMap::from([(
            "id",
            BTreeMap::from([("name", "id"), ("description", "Updated id")]),
        )]);
        let persisted_columns = id_column.clone();
        harness.mock().on("get_persist_doc_columns", move |_| {
            Ok(Value::from_serialize(&persisted_columns))
        });
        let config = default_mock_config();
        config.on("persist_column_docs", |_| Ok(Value::from(true)));
        let changes = Value::from_serialize(BTreeMap::from([
            (
                "query",
                Value::from_serialize(BTreeMap::from([("query", "select 1 as id")])),
            ),
            (
                "column_comments",
                Value::from_serialize(BTreeMap::from([(
                    "comments",
                    BTreeMap::from([("msg", "Stale msg")]),
                )])),
            ),
        ]));
        let model = Value::from_serialize(BTreeMap::from([
            ("alias", Value::from("my_view")),
            ("unique_id", Value::from("model.test_project.my_view")),
            ("columns", Value::from_serialize(&id_column)),
        ]));
        let ctx = harness
            .materialization_context("my_view", "select 1 as id")
            .config(Value::from_dyn_object(config))
            .with("model", model)
            .with("changes", changes)
            .build();
        harness
            .render("{{ alter_view(this, changes) }}", ctx)
            .expect("alter_view should render");

        let executed = executed_sql(harness.mock());
        assert!(
            executed
                .iter()
                .any(|sql| sql.contains("COMMENT ON COLUMN") && sql.contains("Updated id")),
            "post-ALTER comment must be written regardless of file_format: {executed:?}"
        );
        assert!(
            !executed.iter().any(|sql| sql.contains("Stale msg")),
            "the pre-ALTER comment diff must not run after a query change: {executed:?}"
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
