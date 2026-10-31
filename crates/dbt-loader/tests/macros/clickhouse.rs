use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;
use minijinja::value::Kwargs;

use crate::macro_test_harness::MacroTestHarness;

/// `cluster` mirrors the profile key; the adapter hands it back double-quoted.
fn build_harness(cluster: Option<&'static str>) -> MacroTestHarness {
    let harness = MacroTestHarness::for_adapter(AdapterType::ClickHouse)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");
    harness.mock().on("get_clickhouse_cluster_name", move |_| {
        Ok(match cluster {
            Some(cluster) => Value::from(format!("\"{cluster}\"")),
            None => Value::from(()),
        })
    });
    harness
        .mock()
        .on("should_on_cluster", |_| Ok(Value::from(true)));
    harness
}

fn normalize(rendered: &str) -> String {
    rendered.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn relation_value(harness: &MacroTestHarness, schema: &str, identifier: &str) -> Value {
    let relation = harness.relation("my_db", schema, identifier, Some(RelationType::Table));
    RelationObject::new(relation).into_value()
}

/// A `config` whose `get` answers `materialized`/`engine` and otherwise returns the
/// caller's default (positional or `default=`).
fn config_value(materialized: &'static str, engine: Option<&'static str>) -> Value {
    let mock = Arc::new(MockJinjaObject::new());
    mock.on("get", move |args| {
        let key = args.first().and_then(|v| v.as_str()).unwrap_or("");
        let default = args
            .iter()
            .skip(1)
            .find_map(|v| match Kwargs::try_from(v.clone()) {
                Ok(kwargs) => kwargs.get::<Value>("default").ok(),
                Err(_) => Some(v.clone()),
            })
            .unwrap_or(Value::UNDEFINED);
        Ok(match (key, engine) {
            ("materialized", _) => Value::from(materialized),
            ("engine", Some(engine)) => Value::from(engine),
            _ => default,
        })
    });
    Value::from_dyn_object(mock)
}

mod grants {
    use super::*;

    #[test]
    fn show_grant_sql_reads_system_grants_for_the_relation() {
        let harness = build_harness(None);
        let ctx = BTreeMap::from([(
            "relation".to_string(),
            relation_value(&harness, "my_schema", "my_model"),
        )]);

        let rendered = harness
            .render("{{ clickhouse__get_show_grant_sql(relation) }}", ctx)
            .expect("render should succeed");

        assert_eq!(
            normalize(&rendered),
            "SELECT toString(access_type) as privilege_type, COALESCE(user_name, role_name) as grantee \
             from system.grants where table = 'my_model' AND database = 'my_schema'"
        );
    }

    #[test]
    fn grant_and_revoke_sql_target_the_rendered_relation() {
        let harness = build_harness(None);
        let ctx = BTreeMap::from([(
            "relation".to_string(),
            relation_value(&harness, "my_schema", "my_model"),
        )]);
        let relation = harness
            .render("{{ relation }}", ctx.clone())
            .expect("render should succeed");

        let grant = harness
            .render(
                "{{ clickhouse__get_grant_sql(relation, 'select', ['alice', 'bob']) }}",
                ctx.clone(),
            )
            .expect("render should succeed");
        assert_eq!(
            normalize(&grant),
            format!("grant select on {relation} to alice, bob")
        );

        let revoke = harness
            .render(
                "{{ clickhouse__get_revoke_sql(relation, 'insert', ['alice']) }}",
                ctx,
            )
            .expect("render should succeed");
        assert_eq!(
            normalize(&revoke),
            format!("revoke insert on {relation} from alice")
        );
    }

    #[test]
    fn grant_and_revoke_sql_place_on_cluster_before_the_privilege() {
        let harness = build_harness(Some("test_shard"));
        let ctx = BTreeMap::from([(
            "relation".to_string(),
            relation_value(&harness, "my_schema", "my_model"),
        )]);
        let relation = harness
            .render("{{ relation }}", ctx.clone())
            .expect("render should succeed");

        let grant = harness
            .render(
                "{{ clickhouse__get_grant_sql(relation, 'select', ['alice']) }}",
                ctx.clone(),
            )
            .expect("render should succeed");
        assert_eq!(
            normalize(&grant),
            format!("grant ON CLUSTER \"test_shard\" select on {relation} to alice")
        );

        let revoke = harness
            .render(
                "{{ clickhouse__get_revoke_sql(relation, 'select', ['alice']) }}",
                ctx,
            )
            .expect("render should succeed");
        assert_eq!(
            normalize(&revoke),
            format!("revoke ON CLUSTER \"test_shard\" select on {relation} from alice")
        );
    }
}

mod clone {
    use super::*;

    fn can_clone_table(materialized: &'static str, engine: Option<&'static str>) -> String {
        let harness = build_harness(None);
        let ctx = BTreeMap::from([("config".to_string(), config_value(materialized, engine))]);
        harness
            .render("{{ clickhouse__can_clone_table() }}", ctx)
            .expect("render should succeed")
            .trim()
            .to_string()
    }

    #[test]
    fn can_clone_table_for_mergetree_family_engines() {
        assert_eq!(can_clone_table("table", None), "True");
        assert_eq!(can_clone_table("table", Some("MergeTree()")), "True");
        assert_eq!(
            can_clone_table("incremental", Some("ReplacingMergeTree(ver)")),
            "True"
        );
    }

    #[test]
    fn cannot_clone_table_for_other_engines() {
        assert_eq!(can_clone_table("table", Some("Memory")), "False");
        assert_eq!(can_clone_table("table", Some("Log")), "False");
    }

    #[test]
    fn cannot_clone_distributed_materializations() {
        assert_eq!(can_clone_table("distributed_table", None), "False");
        assert_eq!(
            can_clone_table("distributed_incremental", Some("MergeTree()")),
            "False"
        );
    }

    #[test]
    fn create_or_replace_clone_uses_clone_as() {
        let harness = build_harness(None);
        let ctx = BTreeMap::from([
            (
                "this_relation".to_string(),
                relation_value(&harness, "dev_schema", "orders"),
            ),
            (
                "defer_relation".to_string(),
                relation_value(&harness, "prod_schema", "orders"),
            ),
        ]);
        let this_relation = harness
            .render("{{ this_relation }}", ctx.clone())
            .expect("render should succeed");
        let defer_relation = harness
            .render("{{ defer_relation }}", ctx.clone())
            .expect("render should succeed");

        let rendered = harness
            .render(
                "{{ clickhouse__create_or_replace_clone(this_relation, defer_relation) }}",
                ctx,
            )
            .expect("render should succeed");

        assert_eq!(
            normalize(&rendered),
            format!("CREATE OR REPLACE TABLE {this_relation} CLONE AS {defer_relation}")
        );
    }

    #[test]
    fn create_or_replace_clone_places_on_cluster_before_clone_as() {
        let harness = build_harness(Some("test_shard"));
        let ctx = BTreeMap::from([
            (
                "this_relation".to_string(),
                relation_value(&harness, "dev_schema", "orders"),
            ),
            (
                "defer_relation".to_string(),
                relation_value(&harness, "prod_schema", "orders"),
            ),
        ]);
        let this_relation = harness
            .render("{{ this_relation }}", ctx.clone())
            .expect("render should succeed");
        let defer_relation = harness
            .render("{{ defer_relation }}", ctx.clone())
            .expect("render should succeed");

        let rendered = harness
            .render(
                "{{ clickhouse__create_or_replace_clone(this_relation, defer_relation) }}",
                ctx,
            )
            .expect("render should succeed");

        assert_eq!(
            normalize(&rendered),
            format!(
                "CREATE OR REPLACE TABLE {this_relation} ON CLUSTER \"test_shard\" CLONE AS {defer_relation}"
            )
        );
    }
}
