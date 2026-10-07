use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_jinja_ctx::MacroLookupContext;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, default_mock_config, executed_sql};

const SNAPSHOT_SQL: &str = "SELECT 1 AS id, current_timestamp() AS updated_at";

fn snapshot_macro_name(adapter_type: AdapterType) -> &'static str {
    match adapter_type {
        AdapterType::Bigquery => "materialization_snapshot_default",
        other => panic!("unsupported adapter for snapshot materialization test: {other:?}"),
    }
}

fn snapshot_model() -> Value {
    Value::from_serialize(BTreeMap::from([
        ("name", Value::from("my_snapshot")),
        ("alias", Value::from("my_snapshot")),
        ("database", Value::from("TEST_DB")),
        ("schema", Value::from("TEST_SCHEMA")),
        (
            "unique_id",
            Value::from("snapshot.test_project.my_snapshot"),
        ),
        ("resource_type", Value::from("snapshot")),
        ("columns", Value::from(BTreeMap::<String, Value>::new())),
        ("config", Value::from(BTreeMap::<String, Value>::new())),
        ("compiled_code", Value::from(SNAPSHOT_SQL)),
    ]))
}

fn snapshot_config() -> Arc<MockJinjaObject> {
    let mock = default_mock_config();
    mock.on("get", |args| {
        let key = args.first().and_then(|v| v.as_str());
        let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
        match key {
            Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                "enforced".to_string(),
                Value::from(false),
            )]))),
            Some("strategy") => Ok(Value::from("timestamp")),
            Some("unique_key") => Ok(Value::from("id")),
            Some("updated_at") => Ok(Value::from("updated_at")),
            _ => Ok(default),
        }
    });
    mock
}

fn build_harness(adapter_type: AdapterType) -> MacroTestHarness {
    let harness = MacroTestHarness::for_adapter(adapter_type)
        .load_all_macros()
        .with_stub_functions()
        .with_behavior_flag("use_catalogs_v2", false)
        .build()
        .expect("harness should build");

    let mock = harness.mock();
    mock.on("parse_partition_by", |_| Ok(Value::from(())));
    mock.on("build_catalog_relation", |_| {
        Ok(Value::from_serialize(BTreeMap::from([(
            "table_format",
            "default",
        )])))
    });
    mock.on("get_table_options", |_| {
        Ok(Value::from(BTreeMap::<String, Value>::new()))
    });
    mock.on("commit", |_| Ok(Value::UNDEFINED));
    mock.on("get_hard_deletes_behavior", |_| Ok(Value::from("ignore")));
    mock.on("get_column_schema_from_query", |_| {
        Ok(Value::from_serialize(vec![BTreeMap::from([
            ("column", "dbt_snapshot_time"),
            ("dtype", "TIMESTAMP"),
        ])]))
    });

    harness
}

/// Renders the snapshot materialization for a target the relation cache does
/// not know about and returns the DDL that built it.
fn render_first_build(adapter_type: AdapterType) -> String {
    let harness = build_harness(adapter_type);
    harness.mock().on("get_relation", |_| Ok(Value::from(())));

    let ctx = harness
        .materialization_context("my_snapshot", SNAPSHOT_SQL)
        .relation_type(RelationType::Table)
        .config(Value::from_dyn_object(snapshot_config()))
        // `strategy_dispatch` looks the strategy macro up through `context`.
        .with(
            "context",
            Value::from_object(MacroLookupContext::new(
                "test_project".to_string(),
                None,
                BTreeSet::from(["test_project".to_string()]),
            )),
        )
        .with("model", snapshot_model())
        .build();

    let call = format!("{{{{ {}() }}}}", snapshot_macro_name(adapter_type));
    harness
        .render(&call, ctx)
        .unwrap_or_else(|e| panic!("{adapter_type:?} snapshot materialization failed: {e:?}"));

    executed_sql(harness.mock())
        .into_iter()
        .find(|sql| sql.contains("my_snapshot") && sql.contains("create"))
        .unwrap_or_else(|| panic!("no create statement for the snapshot target was executed"))
}

mod bigquery {
    use super::*;

    #[test]
    fn first_build_creates_table_without_replace() {
        let sql = render_first_build(AdapterType::Bigquery);
        assert!(
            sql.contains("create table `TEST_DB`.`TEST_SCHEMA`.`my_snapshot`"),
            "{sql}"
        );
        assert!(!sql.contains("or replace"), "{sql}");
    }

    /// Renders `bigquery__create_table_as` directly for the snapshot target.
    fn render_create_table_as(temporary: bool, model: Option<Value>) -> String {
        let harness = build_harness(AdapterType::Bigquery);
        let target = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "my_snapshot",
            Some(RelationType::Table),
        );
        let mut ctx = harness
            .materialization_context("my_snapshot", SNAPSHOT_SQL)
            .with("target", RelationObject::new(target).into_value())
            .with("temporary", Value::from(temporary))
            .build();
        match model {
            Some(model) => ctx.insert("model".to_string(), model),
            None => ctx.remove("model"),
        };

        harness
            .render(
                "{{ create_table_as(temporary, target, 'select 1 as id') }}",
                ctx,
            )
            .expect("render should succeed")
    }

    #[test]
    fn snapshot_staging_table_still_replaces() {
        let sql = render_create_table_as(true, Some(snapshot_model()));
        assert!(sql.contains("create or replace table"), "{sql}");
    }

    #[test]
    fn create_table_as_without_model_still_replaces() {
        let sql = render_create_table_as(false, None);
        assert!(sql.contains("create or replace table"), "{sql}");
    }
}

mod spark {
    use super::*;

    fn helper_columns(helpers: &[&str], uppercase: bool) -> Value {
        let mut names = vec!["new_column".to_string(), "dbt_change_type".to_string()];
        names.extend(helpers.iter().map(|name| (*name).to_string()));
        if uppercase {
            names = names.into_iter().map(|name| name.to_uppercase()).collect();
        }
        Value::from_serialize(
            names
                .iter()
                .map(|name| BTreeMap::from([("name", name.as_str()), ("data_type", "string")]))
                .collect::<Vec<_>>(),
        )
    }

    fn build_harness(helpers: &[&str], uppercase: bool) -> MacroTestHarness {
        let harness = MacroTestHarness::for_adapter(AdapterType::Spark)
            .load_all_macros()
            .with_stub_functions()
            .with_macro(
                "test_project",
                "spark_build_snapshot_staging_table",
                r#"{% macro spark_build_snapshot_staging_table(strategy, sql, target_relation) %}
                            {{ return('snapshot_staging') }}
                        {% endmacro %}"#,
            )
            .with_macro(
                "test_project",
                "spark__snapshot_merge_sql",
                r#"{% macro spark__snapshot_merge_sql(target, source, insert_cols) %}
                            {% do adapter.record_insert_columns(insert_cols) %}
                            select 1
                        {% endmacro %}"#,
            )
            .build()
            .expect("harness should build");

        let columns = helper_columns(helpers, uppercase);
        let missing_columns = columns.clone();
        let mock = harness.mock();
        mock.on("get_missing_columns", move |_| Ok(missing_columns.clone()));
        mock.on("get_columns_in_relation", move |_| Ok(columns.clone()));
        mock.on("get_relation", |_| {
            Ok(Value::from_serialize(BTreeMap::from([
                ("is_table", true),
                ("is_delta", true),
            ])))
        });
        mock.on("check_schema_exists", |_| Ok(Value::from(true)));
        mock.on("get_hard_deletes_behavior", |_| Ok(Value::from("ignore")));
        for method in [
            "valid_snapshot_target",
            "expand_target_column_types",
            "record_insert_columns",
            "drop_relation",
            "commit",
        ] {
            mock.on(method, |_| Ok(Value::UNDEFINED));
        }
        mock.on("quote", |args| Ok(Value::from(format!("`{}`", args[0]))));
        harness
    }

    fn render_snapshot(harness: &MacroTestHarness, unique_key: Value) {
        let config = snapshot_config();
        config.set_attr("materialized", Value::from("snapshot"));
        config.on("get", move |args| {
            match args.first().and_then(|value| value.as_str()) {
                Some("strategy") => Ok(Value::from("timestamp")),
                Some("unique_key") => Ok(unique_key.clone()),
                Some("updated_at") => Ok(Value::from("updated_at")),
                Some("file_format") => Ok(Value::from("delta")),
                _ => Ok(args.get(1).cloned().unwrap_or(Value::UNDEFINED)),
            }
        });
        let ctx = harness
            .materialization_context("my_snapshot", SNAPSHOT_SQL)
            .relation_type(RelationType::Table)
            .config(Value::from_dyn_object(config))
            .with("model", snapshot_model())
            .with(
                "context",
                Value::from_object(MacroLookupContext::new(
                    "test_project".to_string(),
                    None,
                    BTreeSet::from(["test_project".to_string()]),
                )),
            )
            .build();
        harness
            .render("{{ materialization_snapshot_spark() }}", ctx)
            .expect("Spark snapshot should render");
    }

    #[test]
    fn snapshot_excludes_key_helpers_from_schema_and_merge_columns() {
        for (unique_key, helpers) in [
            (Value::from("id"), vec!["dbt_unique_key"]),
            (Value::from(vec!["id"]), vec!["dbt_unique_key_1"]),
            (
                Value::from(vec!["id", "tenant_id"]),
                vec!["dbt_unique_key_1", "dbt_unique_key_2"],
            ),
        ] {
            for uppercase in [false, true] {
                let harness = build_harness(&helpers, uppercase);
                render_snapshot(&harness, unique_key.clone());
                let mock = harness.mock();
                let sql = executed_sql(mock).join("\n").to_lowercase();
                assert!(sql.contains("add columns"), "{sql}");
                assert!(sql.contains("`new_column` string"), "{sql}");
                assert!(!sql.contains("dbt_unique_key"), "{sql}");
                assert!(!sql.contains("dbt_change_type"), "{sql}");

                let calls = mock.observed_calls();
                let insert_calls = calls.to("record_insert_columns").collect::<Vec<_>>();
                assert_eq!(insert_calls.len(), 1);
                let insert_columns = insert_calls[0].args[0]
                    .try_iter()
                    .expect("insert columns should be a list")
                    .map(|value| value.to_string().to_lowercase())
                    .collect::<Vec<_>>();
                assert_eq!(insert_columns, vec!["`new_column`"]);
            }
        }
    }
}
