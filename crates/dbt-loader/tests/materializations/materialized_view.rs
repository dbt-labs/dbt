use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_common::FsResult;
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
) -> FsResult<String> {
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

    fn build_harness_with_strategy(
        strategy: &'static str,
        reason: Option<&'static str>,
    ) -> MacroTestHarness {
        let reason = reason.map_or_else(|| "none".to_string(), |r| format!("\"{r}\""));
        build_harness_with_decision(&format!("{{'strategy': '{strategy}', 'reason': {reason}}}"))
    }

    fn build_harness_with_decision(decision: &str) -> MacroTestHarness {
        let harness = MacroTestHarness::for_adapter(ADAPTER)
            .load_all_macros()
            .with_stub_functions()
            .with_macro(
                "test_project",
                "databricks__get_mv_replace_strategy",
                &format!(
                    "{{% macro databricks__get_mv_replace_strategy(desired, existing) %}}{{{{ return({decision}) }}}}{{% endmacro %}}"
                ),
            )
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
        try_render_alter(harness, changes)
            .unwrap_or_else(|e| panic!("alter MV macro failed: {e:?}"))
    }

    fn try_render_alter(harness: &MacroTestHarness, changes: Value) -> FsResult<String> {
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
        harness.render(&call, ctx)
    }

    #[test]
    fn no_existing_relation_creates_mv() {
        let harness = build_harness_with_strategy("replace", None);
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
    fn alter_full_refresh_replace_strategy_uses_create_or_replace() {
        let h = build_harness_with_strategy("replace", None);
        let changes =
            config_changes_mock(true, BTreeMap::from([("tags", Value::from("some_tag"))]));
        let result = render_alter(&h, changes);

        let lower = result.to_lowercase();

        h.mock().observed_calls().assert_not_called("drop_relation");
        assert!(
            !lower.contains("drop materialized view"),
            "Expected no DROP when the strategy is replace, got: {result}",
        );
        assert!(
            lower.contains("create or replace"),
            "Expected CREATE OR REPLACE when the strategy is replace, got: {result}",
        );
    }

    #[test]
    fn alter_full_refresh_drop_strategy_uses_drop_and_create() {
        let h = build_harness_with_strategy("drop", None);
        let changes =
            config_changes_mock(true, BTreeMap::from([("tags", Value::from("some_tag"))]));
        let lower = render_alter(&h, changes).to_lowercase();

        assert!(lower.contains("drop materialized view"), "got: {lower}");
        assert!(lower.contains("create or replace"), "got: {lower}");
    }

    #[test]
    fn alter_full_refresh_refuse_strategy_raises_without_drop() {
        let h = build_harness_with_strategy("refuse", Some("location 's3://b/a' would be reused"));
        let changes =
            config_changes_mock(true, BTreeMap::from([("tags", Value::from("some_tag"))]));
        let err = try_render_alter(&h, changes).expect_err("refuse should raise");

        assert!(
            format!("{err:?}").contains("location 's3://b/a' would be reused"),
            "got: {err:?}"
        );
        h.mock().observed_calls().assert_not_called("drop_relation");
    }

    #[test]
    fn alter_full_refresh_unknown_strategy_raises_without_drop() {
        for decision in ["{}", "{'strategy': none}", "{'strategy': 'Drop'}"] {
            let h = build_harness_with_decision(decision);
            let changes =
                config_changes_mock(true, BTreeMap::from([("tags", Value::from("some_tag"))]));
            let err = try_render_alter(&h, changes)
                .expect_err(&format!("decision {decision} should raise"));

            assert!(
                format!("{err:?}").contains("unrecognised replace strategy"),
                "decision {decision}: {err:?}"
            );
            h.mock().observed_calls().assert_not_called("drop_relation");
        }
    }

    #[test]
    fn alter_refresh_schedule_without_full_refresh() {
        let h = build_harness_with_strategy("replace", None);
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
        let h = build_harness_with_strategy("replace", None);
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

    mod replace_strategy {
        use super::*;

        const LOCATION: &str = "delta.universalFormat.compatibility.location";

        fn config(partitioned_by: Value, tblproperties: Value) -> Value {
            Value::from_serialize(BTreeMap::from([
                ("partitioned_by", partitioned_by),
                ("tblproperties", tblproperties),
            ]))
        }

        fn field(name: &str, value: Value) -> Value {
            Value::from_serialize(BTreeMap::from([(name, value)]))
        }

        fn mv_with(parts: &[&str], properties: &[(&str, &str)]) -> Value {
            config(
                field("partition_by", Value::from_serialize(parts)),
                field(
                    "tblproperties",
                    Value::from_serialize(properties.iter().copied().collect::<BTreeMap<_, _>>()),
                ),
            )
        }

        fn mv(parts: &[&str], location: Option<&str>) -> Value {
            mv_with(
                parts,
                &location
                    .map(|l| (LOCATION, l))
                    .into_iter()
                    .collect::<Vec<_>>(),
            )
        }

        struct Decide(MacroTestHarness);

        impl Decide {
            fn new() -> Self {
                Self(
                    MacroTestHarness::for_adapter(ADAPTER)
                        .load_all_macros()
                        .with_stub_functions()
                        .build()
                        .expect("harness should build"),
                )
            }

            /// Returns `(strategy, reason)` for rebuilding `existing` into `desired`.
            fn run(&self, existing: Value, desired: Value) -> (String, String) {
                let out = self
                    .0
                    .render(
                        "{% set d = databricks__get_mv_replace_strategy(desired, existing) %}{{ d.strategy }}|{{ d.reason }}",
                        BTreeMap::from([("desired", desired), ("existing", existing)]),
                    )
                    .unwrap_or_else(|e| panic!("strategy macro failed: {e:?}"));
                let (strategy, reason) = out.split_once('|').expect("strategy|reason");
                (strategy.trim().to_string(), reason.to_string())
            }

            fn assert(
                &self,
                existing: Value,
                desired: Value,
                strategy: &str,
                reason_contains: &str,
            ) {
                let (got, reason) = self.run(existing.clone(), desired.clone());
                assert_eq!(
                    got, strategy,
                    "existing={existing:?} desired={desired:?} reason={reason}"
                );
                assert!(
                    reason.contains(reason_contains),
                    "reason {reason:?} should contain {reason_contains:?}"
                );
            }
        }

        #[test]
        fn no_location() {
            let d = Decide::new();
            d.assert(mv(&[], None), mv(&[], None), "replace", "");
            d.assert(mv(&[], None), mv(&["id"], None), "replace", "");
            d.assert(mv(&["id"], None), mv(&["user_id"], None), "replace", "");
            d.assert(mv(&["id"], None), mv(&[], None), "drop", "");
        }

        #[test]
        fn same_location() {
            let (d, a) = (Decide::new(), Some("s3://b/a"));
            d.assert(mv(&[], a), mv(&[], a), "replace", "");
            d.assert(
                mv_with(&[], &[(LOCATION, "s3://b/a"), ("dbt.test", "a")]),
                mv_with(
                    &[],
                    &[
                        (LOCATION, "s3://b/a"),
                        ("dbt.test", "b"),
                        ("delta.enableChangeDataFeed", "false"),
                    ],
                ),
                "replace",
                "",
            );
            d.assert(
                mv(&[], a),
                mv(&["id"], a),
                "refuse",
                "changing partition_by",
            );
            d.assert(
                mv(&["id"], a),
                mv(&["user_id"], a),
                "refuse",
                "changing partition_by",
            );
            d.assert(
                mv(&["id"], a),
                mv(&[], a),
                "refuse",
                "removing partition_by",
            );
            d.assert(
                mv(&["id"], a),
                mv(&[], a),
                "refuse",
                "doesn't overlap 's3://b/a'",
            );
            d.assert(mv(&["ID"], a), mv(&["`id`"], a), "replace", "");
        }

        #[test]
        fn location_added() {
            let (d, a) = (Decide::new(), Some("s3://b/a"));
            d.assert(mv(&[], None), mv(&[], a), "replace", "");
            d.assert(mv(&["id"], None), mv(&["id"], a), "replace", "");
            d.assert(mv(&[], None), mv(&["id"], a), "replace", "");
            d.assert(mv(&["id"], None), mv(&["name"], a), "replace", "");
            d.assert(
                mv(&["id"], None),
                mv(&[], a),
                "refuse",
                "removing partition_by",
            );
        }

        #[test]
        fn location_deleted() {
            let (d, a) = (Decide::new(), Some("s3://b/a"));
            d.assert(mv(&[], a), mv(&[], None), "replace", "");
            d.assert(mv(&[], a), mv(&["id"], None), "replace", "");
            d.assert(mv(&["id"], a), mv(&["name"], None), "replace", "");
            d.assert(mv(&["id"], a), mv(&[], None), "drop", "");
        }

        #[test]
        fn location_changed_always_refuses() {
            let d = Decide::new();
            for (from, to) in [
                ("s3://b/c04a", "s3://b/c04b"),
                ("s3://b/mv", "s3://other/mv"),
                ("s3://b/p", "s3://b/p/child"),
                ("s3://b/p/child", "s3://b/p"),
                ("s3://b/p", "s3://b/p/"),
            ] {
                for (from_parts, to_parts) in [(&[][..], &[][..]), (&["id"][..], &[][..])] {
                    d.assert(
                        mv(from_parts, Some(from)),
                        mv(to_parts, Some(to)),
                        "refuse",
                        &format!("from '{from}' to '{to}'"),
                    );
                }
            }
        }

        #[test]
        fn empty_location_refuses() {
            let (d, a) = (Decide::new(), Some("s3://b/a"));
            for empty in ["", "  "] {
                d.assert(mv(&["id"], a), mv(&[], Some(empty)), "refuse", "desired");
                d.assert(
                    mv(&["id"], Some(empty)),
                    mv(&[], None),
                    "refuse",
                    "existing",
                );
            }
        }

        #[test]
        fn identical_location_with_trailing_slash_is_unchanged() {
            let (d, p) = (Decide::new(), Some("s3://b/p/"));
            d.assert(
                mv(&["id"], p),
                mv(&[], p),
                "refuse",
                "removing partition_by",
            );
            d.assert(mv(&[], p), mv(&[], p), "replace", "");
        }

        #[test]
        fn unreadable_inputs_refuse() {
            let d = Decide::new();
            let good = || mv(&[], None);
            let props = || {
                field(
                    "tblproperties",
                    Value::from_serialize(BTreeMap::<String, String>::new()),
                )
            };
            let parts = || field("partition_by", Value::from_serialize(Vec::<String>::new()));
            let malformed = [
                (
                    "missing partitioned_by",
                    Value::from_serialize(BTreeMap::from([("tblproperties", props())])),
                    "partitioned_by",
                ),
                (
                    "missing tblproperties",
                    Value::from_serialize(BTreeMap::from([("partitioned_by", parts())])),
                    "tblproperties",
                ),
                (
                    "none component",
                    config(Value::from(()), props()),
                    "partitioned_by",
                ),
                (
                    "component not a mapping",
                    config(Value::from("id"), props()),
                    "partitioned_by",
                ),
                (
                    "missing partition_by field",
                    config(field("other", Value::from(1)), props()),
                    "partitioned_by",
                ),
                (
                    "none partition_by field",
                    config(field("partition_by", Value::from(())), props()),
                    "partitioned_by",
                ),
                (
                    "string partition_by",
                    config(field("partition_by", Value::from("id")), props()),
                    "partitioned_by",
                ),
                (
                    "non-string partition column",
                    config(field("partition_by", Value::from_serialize([1])), props()),
                    "partitioned_by",
                ),
                (
                    "missing tblproperties field",
                    config(parts(), field("other", Value::from(1))),
                    "tblproperties",
                ),
                (
                    "tblproperties not a mapping",
                    config(parts(), field("tblproperties", Value::from("x"))),
                    "tblproperties",
                ),
                (
                    "non-string property value",
                    config(
                        parts(),
                        field(
                            "tblproperties",
                            Value::from_serialize(BTreeMap::from([("k", 1)])),
                        ),
                    ),
                    "tblproperties",
                ),
                ("none config", Value::from(()), "partitioned_by"),
            ];
            for (label, bad, component) in malformed {
                for (side, existing, desired) in [
                    ("desired", good(), bad.clone()),
                    ("existing", bad.clone(), good()),
                ] {
                    let (strategy, reason) = d.run(existing, desired);
                    assert_eq!(strategy, "refuse", "{label} on the {side} side: {reason}");
                    assert!(
                        reason.contains(&format!("could not read the {side} {component}")),
                        "{label} on the {side} side: {reason}"
                    );
                }
            }
        }
    }
}
