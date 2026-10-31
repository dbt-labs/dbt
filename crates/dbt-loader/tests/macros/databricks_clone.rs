use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::RecordBatch;
use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_agate::AgateTable;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::{executed_sql, MacroTestHarness};

const POLICIES_SQL: &str = "SELECT 1 \
    FROM `Main`.`information_schema`.`row_filters` \
    WHERE table_catalog = 'main' AND table_schema = 'prod' AND table_name = 'orders' \
    UNION ALL \
    SELECT 1 \
    FROM `Main`.`information_schema`.`column_masks` \
    WHERE table_catalog = 'main' AND table_schema = 'prod' AND table_name = 'orders'";

/// `describe_as_json` and `capability` drive the lookup choice; `json_metadata` and
/// `policy_rows` are what the DESCRIBE and the information_schema query return.
struct Lookup {
    describe_as_json: bool,
    capability: bool,
    json_metadata: &'static str,
    policy_rows: usize,
}

fn table(batch: RecordBatch) -> Value {
    Value::from_serialize(BTreeMap::from([(
        "table",
        Value::from_object(AgateTable::from_record_batch(Arc::new(batch))),
    )]))
}

fn build_harness(lookup: Lookup) -> MacroTestHarness {
    let mut harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .load_all_macros()
        .with_stub_functions()
        .with_global("execute", Value::from(true))
        .build()
        .expect("harness should build");

    harness.mock().set_attr(
        "behavior",
        Value::from_serialize(BTreeMap::from([(
            "use_describe_as_json_for_relation_metadata",
            BTreeMap::from([("no_warn", lookup.describe_as_json)]),
        )])),
    );
    let capability = lookup.capability;
    harness.mock().on("has_dbr_capability", move |args| {
        Ok(Value::from(
            args.first().and_then(Value::as_str) == Some("describe_table_extended_as_json")
                && capability,
        ))
    });

    let json_metadata = lookup.json_metadata;
    let policy_rows = lookup.policy_rows;
    harness
        .env_mut()
        .env
        .add_function("load_result", move |name: Value| {
            let batch = match name.as_str() {
                Some("describe_table_extended_as_json") => {
                    arrow_array::record_batch!(("json_metadata", Utf8, [json_metadata]))
                }
                Some("clone_source_policies") => {
                    arrow_array::record_batch!(("1", Int32, vec![1; policy_rows]))
                }
                other => panic!("unexpected load_result({other:?})"),
            }
            .expect("result batch should build");
            Ok(table(batch))
        });
    harness
}

fn relation_value(
    harness: &MacroTestHarness,
    database: &str,
    schema: &str,
    identifier: &str,
) -> Value {
    RelationObject::new(harness.relation(database, schema, identifier, Some(RelationType::Table)))
        .into_value()
}

fn has_policies(
    harness: &MacroTestHarness,
    database: &str,
    schema: &str,
    identifier: &str,
) -> bool {
    let rendered = harness
        .render(
            "{{ 'yes' if clone_source_has_policies(relation) else 'no' }}",
            BTreeMap::from([(
                "relation".to_string(),
                relation_value(harness, database, schema, identifier),
            )]),
        )
        .expect("clone_source_has_policies should render");
    match rendered.trim() {
        "yes" => true,
        "no" => false,
        other => panic!("unexpected render {other:?}"),
    }
}

fn normalized(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn executed(harness: &MacroTestHarness) -> Vec<String> {
    executed_sql(harness.mock())
        .iter()
        .map(|s| normalized(s))
        .collect()
}

fn queried_information_schema(harness: &MacroTestHarness) -> bool {
    executed(harness)
        .iter()
        .any(|sql| sql.contains("information_schema"))
}

#[test]
fn hive_metastore_source_skips_the_lookup() {
    let harness = build_harness(Lookup {
        describe_as_json: true,
        capability: true,
        json_metadata: r#"{"row_filter": {"function_name": "f"}}"#,
        policy_rows: 1,
    });
    assert!(!has_policies(&harness, "hive_metastore", "prod", "orders"));
    assert!(executed_sql(harness.mock()).is_empty());
}

#[test]
fn describe_as_json_lookup_detects_row_filters_and_column_masks() {
    let cases = [
        (r#"{"row_filter": {"function_name": "f"}}"#, true),
        (r#"{"column_masks": [{"column_name": "c"}]}"#, true),
        ("{}", false),
        (r#"{"row_filter": null, "column_masks": []}"#, false),
    ];
    for (json_metadata, expected) in cases {
        let harness = build_harness(Lookup {
            describe_as_json: true,
            capability: true,
            json_metadata,
            // Opposite of `expected` so an information_schema fallback cannot
            // accidentally satisfy the boolean.
            policy_rows: usize::from(!expected),
        });
        assert_eq!(
            has_policies(&harness, "main", "prod", "orders"),
            expected,
            "{json_metadata}"
        );
        assert!(!queried_information_schema(&harness), "{json_metadata}");
    }
}

#[test]
fn lookup_falls_back_to_information_schema() {
    for (describe_as_json, capability) in [(false, true), (true, false)] {
        for (policy_rows, expected) in [(0, false), (1, true), (2, true)] {
            let harness = build_harness(Lookup {
                describe_as_json,
                capability,
                json_metadata: r#"{"row_filter": {"function_name": "f"}}"#,
                policy_rows,
            });
            let case =
                format!("flag={describe_as_json} capability={capability} rows={policy_rows}");
            assert_eq!(
                has_policies(&harness, "Main", "Prod", "Orders"),
                expected,
                "{case}"
            );
            assert_eq!(executed(&harness), [POLICIES_SQL], "{case}");
        }
    }
}
