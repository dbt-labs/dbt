mod trino {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use dbt_adapter::relation::RelationObject;
    use dbt_adapter_core::AdapterType;
    use dbt_jinja_utils::mock_object::MockJinjaObject;
    use dbt_schemas::dbt_types::RelationType;
    use minijinja::Value;
    use minijinja::value::Kwargs;

    use crate::macro_test_harness::{MacroTestHarness, default_mock_config, executed_sql};

    const MODEL: &str = "my_table";
    const SQL: &str = "select 1 as id";

    /// A `config` mock answering `on_table_exists` and `properties`, and the
    /// caller's default for every other key.
    fn table_config(on_table_exists: &'static str, properties: Option<Value>) -> Value {
        let mock = default_mock_config();
        mock.on("get", move |args| {
            let key = args.first().and_then(|v| v.as_str());
            let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
            match key {
                Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                    "enforced".to_string(),
                    Value::from(false),
                )]))),
                Some("on_table_exists") => Ok(Value::from(on_table_exists)),
                Some("properties") => Ok(properties.clone().unwrap_or(default)),
                _ => Ok(default),
            }
        });
        Value::from_dyn_object::<Arc<MockJinjaObject>>(mock)
    }

    /// Builds a harness where `get_relation` finds `my_table` as an existing
    /// table when `existing` is set, and nothing for every other identifier.
    fn build(existing: bool) -> MacroTestHarness {
        let harness = MacroTestHarness::for_adapter(AdapterType::Trino)
            .load_all_macros()
            .with_stub_functions()
            .build()
            .expect("harness should build");

        let existing_table = harness.relation("memory", "t9", MODEL, Some(RelationType::Table));
        harness.mock().on("get_relation", move |args| {
            let identifier = args
                .last()
                .and_then(|v| <Kwargs as minijinja::value::ArgType>::from_value(Some(v)).ok())
                .and_then(|kwargs| kwargs.get::<String>("identifier").ok());
            match identifier.as_deref() {
                Some(MODEL) if existing => {
                    Ok(RelationObject::new(Arc::clone(&existing_table)).into_value())
                }
                _ => Ok(Value::from(())),
            }
        });
        harness
            .mock()
            .on("rename_relation", |_| Ok(Value::UNDEFINED));
        harness.mock().on("drop_relation", |_| Ok(Value::UNDEFINED));
        harness
    }

    fn render(harness: &MacroTestHarness, config: Value) {
        let ctx = harness
            .materialization_context(MODEL, SQL)
            .database("memory")
            .schema("t9")
            .relation_type(RelationType::Table)
            .config(config)
            .build();
        harness
            .render("{{ materialization_table_trino() }}", ctx)
            .unwrap_or_else(|e| panic!("trino table materialization failed: {e:?}"));
    }

    /// `(method, first argument)` for every execute/rename/drop call, in order.
    fn ddl_calls(harness: &MacroTestHarness) -> Vec<(String, String)> {
        harness
            .mock()
            .observed_calls()
            .iter()
            .filter(|c| {
                matches!(
                    c.method.as_str(),
                    "execute" | "rename_relation" | "drop_relation"
                )
            })
            .map(|c| {
                let args = c
                    .args
                    .iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>()
                    .join(" -> ");
                (c.method.clone(), args)
            })
            .collect()
    }

    fn squash(sql: &str) -> String {
        sql.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    const TARGET: &str = r#""memory"."t9"."my_table""#;

    /// A DDL call the materialization is expected to make.
    #[derive(Debug)]
    enum Ddl<'a> {
        /// An executed `create table <relation>`.
        Create(&'a str),
        /// `rename_relation(from, to)`.
        Rename(&'a str, &'a str),
        /// `drop_relation(relation)`.
        Drop(&'a str),
    }

    impl Ddl<'_> {
        /// The index of this call in `calls`, as returned by `ddl_calls`.
        fn index_in(&self, calls: &[(String, String)]) -> usize {
            calls
                .iter()
                .position(|(method, args)| match self {
                    Ddl::Create(relation) => {
                        method == "execute"
                            && squash(args).contains(&format!("create table {relation}"))
                    }
                    Ddl::Rename(from, to) => {
                        method == "rename_relation" && *args == format!("{from} -> {to}")
                    }
                    Ddl::Drop(relation) => method == "drop_relation" && args == relation,
                })
                .unwrap_or_else(|| panic!("expected {self:?}, got {calls:?}"))
        }
    }

    /// Asserts that one executed statement, whitespace squashed, contains `expected`.
    fn assert_executed(harness: &MacroTestHarness, expected: &str) {
        let sqls: Vec<String> = executed_sql(harness.mock())
            .iter()
            .map(|s| squash(s))
            .collect();
        assert!(
            sqls.iter().any(|s| s.contains(expected)),
            "expected `{expected}`, got {sqls:?}"
        );
    }

    /// Asserts that the existing table was neither renamed nor dropped.
    fn assert_no_rename_or_drop(harness: &MacroTestHarness) {
        let calls = harness.mock().observed_calls();
        calls.assert_not_called("rename_relation");
        calls.assert_not_called("drop_relation");
    }

    #[test]
    fn table_on_table_exists_rename() {
        let harness = build(true);
        render(&harness, table_config("rename", None));

        let calls = ddl_calls(&harness);
        let tmp = r#""memory"."t9"."my_table__dbt_tmp""#;
        let backup = r#""memory"."t9"."my_table__dbt_backup""#;
        let order = [
            Ddl::Create(tmp),
            Ddl::Rename(TARGET, backup),
            Ddl::Rename(tmp, TARGET),
            Ddl::Drop(backup),
        ]
        .map(|ddl| ddl.index_in(&calls));
        assert!(
            order.windows(2).all(|pair| pair[0] < pair[1]),
            "create tmp, rename old to backup, rename tmp, drop backup must run in order: {calls:?}"
        );
    }

    #[test]
    fn table_on_table_exists_replace() {
        let harness = build(true);
        render(&harness, table_config("replace", None));

        assert_executed(
            &harness,
            r#"create or replace table "memory"."t9"."my_table""#,
        );
        assert_no_rename_or_drop(&harness);
    }

    #[test]
    fn table_on_table_exists_drop() {
        let harness = build(true);
        render(&harness, table_config("drop", None));

        let calls = ddl_calls(&harness);
        assert!(
            Ddl::Drop(TARGET).index_in(&calls) < Ddl::Create(TARGET).index_in(&calls),
            "drop must precede create: {calls:?}"
        );
        harness
            .mock()
            .observed_calls()
            .assert_not_called("rename_relation");
    }

    #[test]
    fn table_on_table_exists_skip() {
        let harness = build(true);
        render(&harness, table_config("skip", None));

        assert_executed(
            &harness,
            r#"create table if not exists "memory"."t9"."my_table""#,
        );
        assert_no_rename_or_drop(&harness);
    }

    #[test]
    fn table_properties_rendered() {
        let harness = build(false);
        let properties = Value::from_serialize(BTreeMap::from([("format", "'PARQUET'")]));
        render(&harness, table_config("rename", Some(properties)));

        assert_executed(
            &harness,
            r#"create table "memory"."t9"."my_table" WITH (format = 'PARQUET') as ( select 1 as id )"#,
        );
    }

    #[test]
    fn table_without_properties_renders_no_with_clause() {
        let harness = build(false);
        render(&harness, table_config("rename", None));

        assert_executed(
            &harness,
            r#"create table "memory"."t9"."my_table" as ( select 1 as id )"#,
        );
    }

    /// `trino__get_columns_in_relation` and `trino__list_relations_without_caching` read the
    /// catalog's `information_schema` views. `relation.information_schema()` without a view
    /// name renders a trailing `.` in dbt v2, so the macros name the view.
    #[test]
    fn information_schema_queries_name_the_view() {
        let harness = build(true);
        let relation = harness.relation("iceberg", "s", "t", Some(RelationType::Table));
        let ctx = BTreeMap::from([
            (
                "relation".to_string(),
                RelationObject::new(relation).into_value(),
            ),
            ("execute".to_string(), Value::from(true)),
        ]);
        harness
            .render(
                "{% set columns = trino__get_columns_in_relation(relation) %}\
                 {% set relations = trino__list_relations_without_caching(relation) %}",
                ctx,
            )
            .unwrap_or_else(|e| panic!("information_schema macros failed: {e:?}"));
        let sql = executed_sql(harness.mock()).join("\n");
        for expected in [
            "data_type\n",
            "\n    from iceberg.INFORMATION_SCHEMA.columns\n",
            "\n    from iceberg.INFORMATION_SCHEMA.tables t\n",
        ] {
            assert!(sql.contains(expected), "{expected:?} in {sql}");
        }
        assert!(!sql.contains(".."), "{sql}");
    }
}
