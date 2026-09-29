use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;
use minijinja::dispatch_object::DispatchObject;

use crate::macro_test_harness::{
    MacroTestHarness, assert_executed_contains, default_mock_config, executed_sql,
};

fn incremental_macro_name(adapter_type: AdapterType) -> &'static str {
    match adapter_type {
        AdapterType::Databricks => "materialization_incremental_databricks",
        other => panic!("unsupported adapter for incremental materialization test: {other:?}"),
    }
}

fn render_incremental(
    harness: &MacroTestHarness,
    adapter_type: AdapterType,
    ctx: BTreeMap<String, Value>,
) -> dbt_common::FsResult<String> {
    let call = format!("{{{{ {}() }}}}", incremental_macro_name(adapter_type));
    harness.render(&call, ctx)
}

fn incremental_model(alias: &str, sql: &str) -> Value {
    Value::from_serialize(BTreeMap::from([
        ("name", Value::from(alias)),
        ("alias", Value::from(alias)),
        (
            "unique_id",
            Value::from(format!("model.test_project.{alias}")),
        ),
        ("columns", Value::from(BTreeMap::<String, Value>::new())),
        (
            "config",
            Value::from_serialize(BTreeMap::from([(
                "materialized",
                Value::from("incremental"),
            )])),
        ),
        ("language", Value::from("sql")),
        ("compiled_code", Value::from(sql)),
    ]))
}

fn incremental_config_with(strategy: Option<&str>, full_refresh: bool) -> Arc<MockJinjaObject> {
    incremental_config_with_options(strategy, full_refresh, false, true)
}

fn incremental_config_with_contract(
    strategy: Option<&str>,
    full_refresh: bool,
    contract_enforced: bool,
) -> Arc<MockJinjaObject> {
    incremental_config_with_options(strategy, full_refresh, contract_enforced, true)
}

fn incremental_config_with_options(
    strategy: Option<&str>,
    full_refresh: bool,
    contract_enforced: bool,
    apply_config_changes: bool,
) -> Arc<MockJinjaObject> {
    let mock = default_mock_config();
    mock.set_attr("materialized", Value::from("incremental"));
    let strategy = strategy.map(str::to_owned);
    mock.on("get", move |args| {
        let key = args.first().and_then(|v| v.as_str());
        let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
        match key {
            Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                "enforced".to_string(),
                Value::from(contract_enforced),
            )]))),
            Some("full_refresh") => Ok(Value::from(full_refresh)),
            Some("incremental_apply_config_changes") => Ok(Value::from(apply_config_changes)),
            Some("incremental_strategy") => Ok(strategy
                .clone()
                .map(Value::from)
                .unwrap_or(Value::UNDEFINED)),
            Some("on_schema_change") => Ok(Value::from("ignore")),
            _ => Ok(default),
        }
    });
    mock
}

fn delete_insert_config() -> Arc<MockJinjaObject> {
    let mock = incremental_config();
    mock.on("get", |args| {
        let key = args.first().and_then(|v| v.as_str());
        let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
        match key {
            Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                "enforced".to_string(),
                Value::from(false),
            )]))),
            Some("full_refresh") => Ok(Value::from(false)),
            Some("incremental_strategy") => Ok(Value::from("delete+insert")),
            Some("incremental_predicates") | Some("predicates") => Ok(Value::from(())),
            Some("unique_key") => Ok(Value::from("id")),
            Some("on_schema_change") => Ok(Value::from("ignore")),
            _ => Ok(default),
        }
    });
    mock.on("require", |args| {
        match args.first().and_then(|v| v.as_str()) {
            Some("unique_key") => Ok(Value::from("id")),
            _ => Ok(Value::UNDEFINED),
        }
    });
    mock
}

fn incremental_config() -> Arc<MockJinjaObject> {
    incremental_config_with(None, false)
}

fn incremental_ctx(harness: &MacroTestHarness) -> BTreeMap<String, Value> {
    incremental_ctx_with_config(harness, incremental_config())
}

fn incremental_ctx_with_config(
    harness: &MacroTestHarness,
    config: Arc<MockJinjaObject>,
) -> BTreeMap<String, Value> {
    harness
        .materialization_context("my_incr", "SELECT id, name FROM source")
        .relation_type(RelationType::Table)
        .config(Value::from_dyn_object(config))
        .with(
            "model",
            incremental_model("my_incr", "SELECT id, name FROM source"),
        )
        .build()
}

mod databricks {
    use super::*;
    const ADAPTER: AdapterType = AdapterType::Databricks;

    fn build_harness() -> MacroTestHarness {
        build_harness_with_materialization_v2(false)
    }

    fn build_harness_with_materialization_v2(enabled: bool) -> MacroTestHarness {
        let mut harness = MacroTestHarness::for_adapter(ADAPTER)
            .load_all_macros()
            .with_stub_functions()
            .with_macro(
                "test_project",
                "apply_constraints",
                "{% macro apply_constraints(relation, constraints) %}{% do adapter.execute('APPLY_CONSTRAINTS_SENTINEL') %}{% endmacro %}",
            )
            .with_behavior_flag("use_materialization_v2", enabled)
            .with_behavior_flag("use_catalogs_v2", false)
            .with_behavior_flag("use_managed_iceberg", false)
            .build()
            .expect("harness should build");

        harness
            .env_mut()
            .env
            .add_function("var", |_name: Value, default: Option<Value>| {
                Ok(default.unwrap_or(Value::UNDEFINED))
            });

        let mock = harness.mock();
        mock.on("clean_sql", |args| {
            Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
        });
        mock.on("get_column_tags_from_model", |_| Ok(Value::UNDEFINED));
        mock.on("drop_relation", |_| Ok(Value::UNDEFINED));
        mock.on("commit", |_| Ok(Value::UNDEFINED));
        mock.on("resolve_file_format", |_| Ok(Value::from("delta")));
        mock.on("is_uniform", |_| Ok(Value::from(false)));
        mock.on("has_dbr_capability", |_| Ok(Value::from(false)));
        mock.on("is_cluster", |_| Ok(Value::from(false)));
        mock.on("optimize", |_| Ok(Value::UNDEFINED));
        mock.on("quote", |args| {
            let identifier = args
                .first()
                .and_then(Value::as_str)
                .expect("quote receives an identifier");
            Ok(Value::from(format!("`{identifier}`")))
        });
        mock.on("valid_incremental_strategies", |_| {
            Ok(Value::from(vec![
                Value::from("append"),
                Value::from("merge"),
                Value::from("insert_overwrite"),
                Value::from("replace_where"),
                Value::from("delete+insert"),
            ]))
        });

        let catalog_val = Value::from_serialize(BTreeMap::from([
            ("file_format".to_string(), Value::from("delta")),
            ("table_format".to_string(), Value::from("delta")),
        ]));
        mock.on("build_catalog_relation", move |_| Ok(catalog_val.clone()));

        harness
    }

    fn assert_ordered_delete_insert_statements(sqls: &[String]) {
        let delete_index = sqls
            .iter()
            .position(|sql| sql.to_lowercase().contains("delete from"))
            .expect("DELETE statement");
        let insert_index = sqls
            .iter()
            .position(|sql| sql.to_lowercase().contains("insert into"))
            .expect("INSERT statement");

        assert!(
            delete_index < insert_index,
            "DELETE must execute before INSERT: {sqls:?}"
        );
        assert!(
            sqls[delete_index].contains(".`id` IN (SELECT `id` FROM"),
            "unique key must be quoted in DELETE: {:?}",
            sqls[delete_index]
        );
        assert!(
            sqls[insert_index].contains("select `id`, `name`"),
            "INSERT must select quoted target columns: {:?}",
            sqls[insert_index]
        );
    }

    #[test]
    fn no_existing_relation_creates_table() {
        let harness = build_harness();
        harness.mock().on("get_relation", |_| Ok(Value::from(())));

        let ctx = incremental_ctx(&harness);
        render_incremental(&harness, ADAPTER, ctx)
            .unwrap_or_else(|e| panic!("incremental materialization failed: {e:?}"));

        harness
            .mock()
            .observed_calls()
            .assert_not_called("drop_relation");

        assert_executed_contains(harness.mock(), "create");
    }

    #[test]
    fn existing_view_dropped_and_recreated() {
        let harness = build_harness();

        let existing = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "my_incr",
            Some(RelationType::View),
        );
        harness.mock().on("get_relation", move |_| {
            Ok(RelationObject::new(Arc::clone(&existing)).into_value())
        });

        let ctx = incremental_ctx(&harness);
        render_incremental(&harness, ADAPTER, ctx)
            .unwrap_or_else(|e| panic!("incremental with existing view failed: {e:?}"));

        harness
            .mock()
            .observed_calls()
            .assert_called("drop_relation");
        assert_executed_contains(harness.mock(), "create");
    }

    #[test]
    fn existing_table_incremental_merge() {
        let harness = build_harness();

        let existing = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "my_incr",
            Some(RelationType::Table),
        );
        harness.mock().on("get_relation", move |_| {
            Ok(RelationObject::new(Arc::clone(&existing)).into_value())
        });
        harness
            .mock()
            .on("get_relation_config", |_| Ok(Value::UNDEFINED));

        let model_config = Arc::new(MockJinjaObject::new());
        model_config.on("get_changeset", |_| Ok(Value::from(())));
        let model_config_val = Value::from_dyn_object(model_config);
        harness.mock().on("get_config_from_model", move |_| {
            Ok(model_config_val.clone())
        });

        harness.mock().on("get_incremental_strategy_macro", |_| {
            Ok(Value::from_function(
                |_args: &[Value]| -> Result<Value, minijinja::Error> {
                    Ok(Value::from("SELECT 1 /* incremental merge */"))
                },
            ))
        });

        let ctx = incremental_ctx(&harness);
        render_incremental(&harness, ADAPTER, ctx)
            .unwrap_or_else(|e| panic!("incremental merge failed: {e:?}"));

        let sqls = executed_sql(harness.mock());
        assert!(
            sqls.len() >= 2,
            "Expected at least 2 SQL statements (temp table + merge), got: {sqls:?}",
        );
    }

    #[test]
    fn legacy_incremental_false_skips_relation_config_lookup() {
        let harness = build_harness();

        let existing = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "my_incr",
            Some(RelationType::Table),
        );
        harness.mock().on("get_relation", move |_| {
            Ok(RelationObject::new(Arc::clone(&existing)).into_value())
        });
        harness
            .mock()
            .on("get_relation_config", |_| Ok(Value::UNDEFINED));

        let model_config = Arc::new(MockJinjaObject::new());
        model_config.on("get_changeset", |_| Ok(Value::from(())));
        let model_config_value = Value::from_dyn_object(Arc::clone(&model_config));
        harness.mock().on("get_config_from_model", move |_| {
            Ok(model_config_value.clone())
        });
        harness.mock().on("get_incremental_strategy_macro", |_| {
            Ok(Value::from_function(
                |_args: &[Value]| -> Result<Value, minijinja::Error> {
                    Ok(Value::from("SELECT 1 /* incremental merge */"))
                },
            ))
        });

        let config = incremental_config_with_options(None, false, false, false);
        let ctx = incremental_ctx_with_config(&harness, config);
        render_incremental(&harness, ADAPTER, ctx)
            .unwrap_or_else(|e| panic!("incremental merge failed: {e:?}"));

        harness
            .mock()
            .observed_calls()
            .assert_not_called("get_relation_config");
        harness
            .mock()
            .observed_calls()
            .assert_not_called("get_config_from_model");
        model_config
            .observed_calls()
            .assert_not_called("get_changeset");
    }

    #[test]
    fn existing_table_incremental_delete_insert_executes_ordered_statements() {
        let harness = build_harness();

        let existing = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "my_incr",
            Some(RelationType::Table),
        );
        harness.mock().on("get_relation", move |_| {
            Ok(RelationObject::new(Arc::clone(&existing)).into_value())
        });
        harness
            .mock()
            .on("get_relation_config", |_| Ok(Value::UNDEFINED));

        let model_config = Arc::new(MockJinjaObject::new());
        model_config.on("get_changeset", |_| Ok(Value::from(())));
        let model_config_val = Value::from_dyn_object(model_config);
        harness.mock().on("get_config_from_model", move |_| {
            Ok(model_config_val.clone())
        });

        harness.mock().on("get_columns_in_relation", |_| {
            Ok(Value::from_serialize(vec![
                BTreeMap::from([("quoted", "`id`")]),
                BTreeMap::from([("quoted", "`name`")]),
            ]))
        });
        harness.mock().on("get_incremental_strategy_macro", |args| {
            let strategy = args
                .get(1)
                .and_then(|v| v.as_str())
                .expect("strategy argument should be passed")
                .replace('+', "_");
            Ok(Value::from_object(DispatchObject {
                macro_name: format!("get_incremental_{strategy}_sql"),
                package_name: None,
                strict: false,
                auto_execute: false,
                context: None,
            }))
        });

        let ctx = harness
            .materialization_context("my_incr", "SELECT id, name FROM source")
            .relation_type(RelationType::Table)
            .config(Value::from_dyn_object(delete_insert_config()))
            .with(
                "model",
                incremental_model("my_incr", "SELECT id, name FROM source"),
            )
            .build();
        render_incremental(&harness, ADAPTER, ctx)
            .unwrap_or_else(|e| panic!("incremental delete+insert failed: {e:?}"));

        let sqls = executed_sql(harness.mock());
        assert_ordered_delete_insert_statements(&sqls);
    }

    fn render_incremental_constraint_changeset(contract_enforced: bool) -> MacroTestHarness {
        let harness = build_harness();
        let existing = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "my_incr",
            Some(RelationType::Table),
        );
        harness.mock().on("get_relation", move |_| {
            Ok(RelationObject::new(Arc::clone(&existing)).into_value())
        });
        harness
            .mock()
            .on("get_relation_config", |_| Ok(Value::UNDEFINED));

        let changeset = Value::from_serialize(BTreeMap::from([(
            "changes",
            BTreeMap::from([("constraints", Value::from(true))]),
        )]));
        let model_config = Arc::new(MockJinjaObject::new());
        model_config.on("get_changeset", move |_| Ok(changeset.clone()));
        let model_config = Value::from_dyn_object(model_config);
        harness
            .mock()
            .on("get_config_from_model", move |_| Ok(model_config.clone()));
        harness.mock().on("get_incremental_strategy_macro", |_| {
            Ok(Value::from_function(
                |_args: &[Value]| -> Result<Value, minijinja::Error> {
                    Ok(Value::from("SELECT 1 /* incremental merge */"))
                },
            ))
        });

        let ctx = incremental_ctx_with_config(
            &harness,
            incremental_config_with_contract(None, false, contract_enforced),
        );
        render_incremental(&harness, ADAPTER, ctx)
            .unwrap_or_else(|e| panic!("constraint reconciliation render failed: {e:?}"));
        harness
    }

    #[test]
    fn incremental_skips_constraint_reconciliation_when_contract_is_unenforced() {
        let harness = render_incremental_constraint_changeset(false);
        let sqls = executed_sql(harness.mock());
        assert!(
            sqls.iter()
                .all(|sql| !sql.contains("APPLY_CONSTRAINTS_SENTINEL")),
            "unenforced contract must not apply constraints, got: {sqls:?}"
        );
    }

    #[test]
    fn incremental_reconciles_constraints_when_contract_is_enforced() {
        let harness = render_incremental_constraint_changeset(true);
        assert_executed_contains(harness.mock(), "APPLY_CONSTRAINTS_SENTINEL");
    }

    mod append;
}

mod databricks_strategies {
    use super::*;

    fn build_harness(
        use_replace_on_for_insert_overwrite: bool,
    ) -> (MacroTestHarness, Arc<MockJinjaObject>) {
        let exceptions = Arc::new(MockJinjaObject::new());
        exceptions.on("warn", |_| Ok(Value::UNDEFINED));

        let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
            .load_all_macros()
            .with_stub_functions()
            .with_behavior_flag(
                "use_replace_on_for_insert_overwrite",
                use_replace_on_for_insert_overwrite,
            )
            .with_global(
                "exceptions",
                Value::from_dyn_object(Arc::clone(&exceptions)),
            )
            .build()
            .expect("harness should build");

        harness.mock().on("quote", |args| {
            let identifier = args
                .first()
                .and_then(Value::as_str)
                .expect("quote requires an identifier");
            Ok(Value::from(format!("`{}`", identifier.replace('`', "``"))))
        });

        (harness, exceptions)
    }

    fn relation_value(harness: &MacroTestHarness, identifier: &str) -> Value {
        RelationObject::new(harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            identifier,
            Some(RelationType::Table),
        ))
        .into_value()
    }

    fn normalize_sql(sql: &str) -> String {
        sql.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    }

    #[test]
    fn replace_where_emits_by_name_only_when_by_name_plus_replace_where_is_supported() {
        for (has_combination_capability, expected_by_name) in [(true, true), (false, false)] {
            let (harness, _) = build_harness(false);
            harness.mock().on("has_dbr_capability", move |args| {
                let capability = args.first().and_then(Value::as_str);
                Ok(Value::from(match capability {
                    Some("insert_by_name") => true,
                    Some("insert_by_name_replace_where") => has_combination_capability,
                    _ => false,
                }))
            });

            let args = Value::from_serialize(BTreeMap::from([
                (
                    "target_relation".to_string(),
                    relation_value(&harness, "target_table"),
                ),
                (
                    "temp_relation".to_string(),
                    relation_value(&harness, "temp_table"),
                ),
                ("incremental_predicates".to_string(), Value::from("id >= 2")),
            ]));

            let sql = harness
                .render(
                    "{{ get_replace_where_sql(args) }}",
                    BTreeMap::from([("args", args)]),
                )
                .expect("replace_where SQL should render");
            assert_eq!(
                normalize_sql(&sql).contains(" by name replace where "),
                expected_by_name,
                "unexpected replace_where SQL: {sql}",
            );
        }
    }

    #[test]
    fn insert_overwrite_warns_only_when_an_old_cluster_cannot_honor_opt_in() {
        const FALLBACK_WARNING: &str = "insert_overwrite: use_replace_on_for_insert_overwrite is enabled but this cluster's DBR version does not support REPLACE ON (requires DBR 17.1+). Falling back to legacy INSERT OVERWRITE.";

        // partition_by is required so the REPLACE ON path emits `replace on` instead of
        // falling back to INSERT OVERWRITE when no replace columns are configured.
        for (is_cluster, has_replace_on, behavior_enabled, expect_warning, expect_replace_on) in [
            (true, false, false, false, false),
            (true, false, true, true, false),
            (true, true, true, false, true),
            (false, true, true, false, true),
            (false, false, false, false, false),
        ] {
            let (harness, exceptions) = build_harness(behavior_enabled);
            harness
                .mock()
                .on("is_cluster", move |_| Ok(Value::from(is_cluster)));
            harness.mock().on("has_dbr_capability", move |args| {
                let capability = args.first().and_then(Value::as_str);
                Ok(Value::from(match capability {
                    Some("replace_on") => has_replace_on,
                    Some("insert_by_name") => true,
                    _ => false,
                }))
            });
            harness.mock().on("get_columns_in_relation", |_| {
                Ok(Value::from_serialize(vec![BTreeMap::from([(
                    "name",
                    Value::from("dt"),
                )])]))
            });

            let config = default_mock_config();
            config.on("get", |args| {
                let key = args.first().and_then(Value::as_str);
                let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
                Ok(match key {
                    Some("partition_by") => Value::from(vec![Value::from("dt")]),
                    Some("liquid_clustered_by") => Value::UNDEFINED,
                    _ => default,
                })
            });

            let sql = harness
                .render(
                    "{{ get_insert_overwrite_sql('source_table', 'target_table') }}",
                    BTreeMap::from([("config", Value::from_dyn_object(config))]),
                )
                .expect("insert_overwrite SQL should render");
            let sql = normalize_sql(&sql);
            assert_eq!(
                sql.contains("replace on"),
                expect_replace_on,
                "unexpected REPLACE ON usage: {sql}",
            );
            assert_eq!(
                sql.contains("insert overwrite"),
                !expect_replace_on,
                "unexpected INSERT OVERWRITE usage: {sql}",
            );

            let warnings: Vec<String> = exceptions
                .observed_calls()
                .to("warn")
                .filter_map(|call| call.args.first().and_then(Value::as_str).map(str::to_owned))
                .collect();
            if expect_warning {
                assert_eq!(warnings, [FALLBACK_WARNING], "unexpected warning for {sql}");
            } else {
                assert!(
                    warnings.is_empty(),
                    "expected no warnings, got {warnings:?} for {sql}",
                );
            }
        }
    }
}

mod spark {
    use super::*;
    const ADAPTER: AdapterType = AdapterType::Spark;

    fn build_harness() -> MacroTestHarness {
        let harness = MacroTestHarness::for_adapter(ADAPTER)
            .load_all_macros()
            .with_stub_functions()
            .build()
            .expect("harness should build");

        // `spark__get_merge_sql` reads the destination columns, but they are only
        // used when `merge_update_columns`/`merge_exclude_columns` are set.
        harness.mock().on("get_columns_in_relation", |_| {
            Ok(Value::from(Vec::<Value>::new()))
        });

        harness
    }

    fn merge_ctx(harness: &MacroTestHarness, unique_key: Value) -> BTreeMap<String, Value> {
        let target = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "orders_incremental",
            Some(RelationType::Table),
        );
        let source = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "orders_incremental__dbt_tmp",
            Some(RelationType::Table),
        );

        BTreeMap::from([
            (
                "config".to_string(),
                Value::from_dyn_object(default_mock_config()),
            ),
            ("sql_header".to_string(), Value::from(())),
            (
                "source".to_string(),
                RelationObject::new(source).into_value(),
            ),
            (
                "target".to_string(),
                RelationObject::new(target).into_value(),
            ),
            ("unique_key".to_string(), unique_key),
        ])
    }

    #[test]
    fn get_incremental_sql_merge_with_unique_key() {
        let harness = build_harness();
        let ctx = merge_ctx(&harness, Value::from("order_id"));

        let sql = harness
            .render(
                "{{ dbt_spark_get_incremental_sql('merge', source, target, none, unique_key, none) }}",
                ctx,
            )
            .unwrap_or_else(|e| panic!("rendering spark merge sql failed: {e:?}"));

        let lower = sql.to_lowercase();
        assert!(lower.contains("merge into"), "got: {sql}");
        assert!(
            sql.contains("DBT_INTERNAL_SOURCE.order_id = DBT_INTERNAL_DEST.order_id"),
            "got: {sql}",
        );
        assert!(lower.contains("when matched then update set"), "got: {sql}");
        assert!(
            lower.contains("when not matched then insert *"),
            "got: {sql}"
        );
    }

    #[test]
    fn get_incremental_sql_merge_without_unique_key_matches_on_false() {
        let harness = build_harness();
        let ctx = merge_ctx(&harness, Value::from(()));

        let sql = harness
            .render(
                "{{ dbt_spark_get_incremental_sql('merge', source, target, none, unique_key, none) }}",
                ctx,
            )
            .unwrap_or_else(|e| panic!("rendering spark merge sql failed: {e:?}"));

        let lower = sql.to_lowercase();
        assert!(lower.contains("merge into"), "got: {sql}");
        assert!(lower.contains("on false"), "got: {sql}");
    }

    #[test]
    fn get_incremental_sql_insert_overwrite() {
        let harness = build_harness();
        harness.mock().on("get_columns_in_relation", |_| {
            Ok(Value::from_serialize(vec![
                BTreeMap::from([("quoted", "`order_id`")]),
                BTreeMap::from([("quoted", "`order_date`")]),
            ]))
        });

        let target = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "orders_incremental",
            Some(RelationType::Table),
        );
        let source = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "orders_incremental__dbt_tmp",
            Some(RelationType::Table),
        );
        let existing = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "orders_incremental",
            Some(RelationType::Table),
        );

        let ctx = BTreeMap::from([
            (
                "config".to_string(),
                Value::from_dyn_object(default_mock_config()),
            ),
            (
                "source".to_string(),
                RelationObject::new(source).into_value(),
            ),
            (
                "target".to_string(),
                RelationObject::new(target).into_value(),
            ),
            (
                "existing".to_string(),
                RelationObject::new(existing).into_value(),
            ),
        ]);

        let sql = harness
            .render(
                "{{ dbt_spark_get_incremental_sql('insert_overwrite', source, target, existing, none, none) }}",
                ctx,
            )
            .unwrap_or_else(|e| panic!("rendering spark insert_overwrite sql failed: {e:?}"));

        let lower = sql.to_lowercase();
        assert!(lower.contains("insert overwrite table"), "got: {sql}");
        assert!(sql.contains("`order_id`, `order_date`"), "got: {sql}");
        assert!(lower.contains("select"), "got: {sql}");
    }
}
