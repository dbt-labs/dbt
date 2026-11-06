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
        AdapterType::Spark => "materialization_snapshot_spark",
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

fn snapshot_config(unique_key: Value) -> Arc<MockJinjaObject> {
    let mock = default_mock_config();
    mock.set_attr("materialized", Value::from("snapshot"));
    mock.on("get", move |args| {
        let key = args.first().and_then(|v| v.as_str());
        let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
        match key {
            Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                "enforced".to_string(),
                Value::from(false),
            )]))),
            Some("strategy") => Ok(Value::from("timestamp")),
            Some("unique_key") => Ok(unique_key.clone()),
            Some("updated_at") => Ok(Value::from("updated_at")),
            Some("file_format") => Ok(Value::from("delta")),
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

fn render_snapshot(
    harness: &MacroTestHarness,
    adapter_type: AdapterType,
    unique_key: Value,
    case: &str,
) {
    let ctx = harness
        .materialization_context("my_snapshot", SNAPSHOT_SQL)
        .relation_type(RelationType::Table)
        .config(Value::from_dyn_object(snapshot_config(unique_key)))
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
        .unwrap_or_else(|e| panic!("{adapter_type:?}, {case}: snapshot failed: {e:?}"));
}

/// Renders the snapshot materialization for a target the relation cache does
/// not know about and returns the DDL that built it.
fn render_first_build(adapter_type: AdapterType) -> String {
    let harness = build_harness(adapter_type);
    harness.mock().on("get_relation", |_| Ok(Value::from(())));
    render_snapshot(&harness, adapter_type, Value::from("id"), "first build");

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

    const SOURCE_COLUMNS: &[&str] = &[
        "id",
        "tenant_id",
        "updated_at",
        "new_column",
        "dbt_unique_key_business",
        "dbt_scd_id",
        "dbt_updated_at",
        "dbt_valid_from",
        "dbt_valid_to",
    ];

    fn staging_columns(columns: &[&str], helpers: &[&str], uppercase: bool) -> Value {
        Value::from_serialize(
            columns
                .iter()
                .chain(helpers)
                .chain(["dbt_change_type"].iter())
                .map(|name| {
                    let name = if uppercase {
                        name.to_uppercase()
                    } else {
                        name.to_string()
                    };
                    BTreeMap::from([("name", name), ("data_type", "string".to_string())])
                })
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
            .build()
            .expect("harness should build");

        let columns = staging_columns(SOURCE_COLUMNS, helpers, uppercase);
        let missing_columns = staging_columns(
            &["new_column", "dbt_unique_key_business"],
            helpers,
            uppercase,
        );
        let mut existing = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "my_snapshot",
            Some(RelationType::Table),
        );
        Arc::get_mut(&mut existing)
            .expect("relation should not be shared")
            .set_is_delta(Some(true));
        let mock = harness.mock();
        mock.on("get_missing_columns", move |_| Ok(missing_columns.clone()));
        mock.on("get_columns_in_relation", move |_| Ok(columns.clone()));
        mock.on("get_relation", move |_| {
            Ok(RelationObject::new(Arc::clone(&existing)).into_value())
        });
        mock.on("check_schema_exists", |_| Ok(Value::from(true)));
        mock.on("get_hard_deletes_behavior", |_| Ok(Value::from("ignore")));
        for method in [
            "valid_snapshot_target",
            "expand_target_column_types",
            "drop_relation",
            "commit",
        ] {
            mock.on(method, |_| Ok(Value::UNDEFINED));
        }
        mock.on("quote", |args| Ok(Value::from(format!("`{}`", args[0]))));
        harness
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
                let case = format!("unique_key={unique_key}, uppercase={uppercase}");
                let harness = build_harness(&helpers, uppercase);
                render_snapshot(&harness, AdapterType::Spark, unique_key.clone(), &case);
                let mock = harness.mock();
                let sql = executed_sql(mock).join("\n").to_lowercase();
                let normalized_sql = sql.split_whitespace().collect::<Vec<_>>().join(" ");
                assert!(
                    normalized_sql.contains(
                        "alter table `test_db`.`test_schema`.`my_snapshot` add columns ( `new_column` string, `dbt_unique_key_business` string );"
                    ),
                    "{case}: unexpected schema alteration: {sql}"
                );
                let source_columns = mock
                    .observed_calls()
                    .to("quote")
                    .map(|call| call.args[0].to_string().to_lowercase())
                    .collect::<Vec<_>>();
                assert_eq!(
                    source_columns, SOURCE_COLUMNS,
                    "{case}: unexpected merge columns"
                );
            }
        }
    }
}
