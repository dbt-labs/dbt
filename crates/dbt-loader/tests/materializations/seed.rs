mod trino {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use dbt_adapter_core::AdapterType;
    use dbt_jinja_utils::mock_object::MockJinjaObject;
    use dbt_schemas::dbt_types::RelationType;
    use minijinja::Value;

    use crate::macro_test_harness::MacroTestHarness;

    /// `types[i]` is what `adapter.convert_type` infers for column `i`.
    fn build(types: &'static [&'static str]) -> MacroTestHarness {
        let harness = MacroTestHarness::for_adapter(AdapterType::Trino)
            .load_all_macros()
            .with_stub_functions()
            // A v2 Trino target: profile fields only, no v1-only target fields (for
            // example `prepared_statements_enabled`).
            .with_global(
                "target",
                Value::from_serialize(BTreeMap::from([
                    ("type", "trino"),
                    ("database", "memory"),
                    ("schema", "t9"),
                ])),
            )
            .build()
            .expect("harness should build");
        let mock = harness.mock();
        mock.on("add_query", |_| Ok(Value::UNDEFINED));
        mock.on("convert_type", move |args| {
            let idx = args.get(1).and_then(|v| v.as_i64()).unwrap_or_default();
            Ok(Value::from(types[idx as usize]))
        });
        mock.on("quote_seed_column", |args| {
            Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
        });
        harness
    }

    fn agate_table(rows: usize) -> Value {
        let rows: Vec<Vec<Value>> = (0..rows)
            .map(|i| vec![Value::from(i as i64), Value::from(format!("name_{i}"))])
            .collect();
        Value::from_serialize(BTreeMap::from([
            ("column_names", Value::from_serialize(["id", "name"])),
            ("rows", Value::from_serialize(rows)),
        ]))
    }

    fn seed_config() -> Value {
        let config = Arc::new(MockJinjaObject::new());
        config.on("get", |args| {
            Ok(args.get(1).cloned().unwrap_or(Value::UNDEFINED))
        });
        Value::from_dyn_object(config)
    }

    #[test]
    fn seed_batches_of_1000() {
        let harness = build(&["BIGINT", "VARCHAR"]);
        let model = Value::from_serialize(BTreeMap::from([
            ("alias", Value::from("my_seed")),
            ("config", Value::from(BTreeMap::<String, Value>::new())),
        ]));
        let ctx = harness
            .materialization_context("my_seed", "")
            .database("memory")
            .schema("t9")
            .relation_type(RelationType::Table)
            .config(seed_config())
            .with("model", model)
            .with("agate_table", agate_table(2500))
            .build();

        let rendered = harness
            .render("{{ load_csv_rows(model, agate_table) }}", ctx)
            .unwrap_or_else(|e| panic!("trino load_csv_rows failed: {e:?}"));

        let calls = harness.mock().observed_calls();
        let batches: Vec<_> = calls.to("add_query").collect();
        assert_eq!(batches.len(), 3, "2500 rows must load in 3 batches");

        // Both columns go through bindings, so a full batch has two `%s` per row.
        let sql = batches[0].args[0].to_string();
        assert!(
            sql.contains(r#"insert into "memory"."t9"."my_seed" (id, name) values"#),
            "unexpected insert: {}",
            &sql[..sql.len().min(200)]
        );
        assert_eq!(
            sql.matches("%s").count(),
            2000,
            "first batch holds 1000 rows"
        );
        let last = batches[2].args[0].to_string();
        assert_eq!(
            last.matches("%s").count(),
            1000,
            "last batch holds 500 rows"
        );
        assert!(
            rendered.contains("insert into"),
            "returns the first batch SQL"
        );
    }

    /// v2 seed rows carry dates and datetimes as objects, not strings. Trino
    /// does not coerce a varchar literal into a date or timestamp column, so
    /// they must render as typed literals rather than as quoted bindings.
    #[test]
    fn seed_date_and_timestamp_values_render_typed_literals() {
        let harness = build(&["BIGINT", "DATE", "TIMESTAMP(6)", "BOOLEAN"]);
        let model = Value::from_serialize(BTreeMap::from([
            ("alias", Value::from("my_seed")),
            ("config", Value::from(BTreeMap::<String, Value>::new())),
        ]));
        let ctx = harness
            .materialization_context("my_seed", "")
            .database("memory")
            .schema("t9")
            .relation_type(RelationType::Table)
            .config(seed_config())
            .with("model", model)
            .build();

        harness
            .render(
                r#"{%- set agate_table = {
                    'column_names': ['id', 'd', 'ts', 'flag'],
                    'rows': [[1, modules.datetime.date(2024, 1, 31),
                              modules.datetime.datetime(2024, 1, 31, 10, 5, 7), true]]
                } -%}
                {{ load_csv_rows(model, agate_table) }}"#,
                ctx,
            )
            .unwrap_or_else(|e| panic!("trino load_csv_rows failed: {e:?}"));

        let calls = harness.mock().observed_calls();
        let sql = calls.to("add_query").next().expect("one insert batch").args[0].to_string();
        let values = sql
            .split("values")
            .nth(1)
            .unwrap_or_default()
            .trim()
            .to_string();
        assert_eq!(
            values, "(%s,DATE '2024-01-31',TIMESTAMP '2024-01-31 10:05:07',%s)",
            "dates and timestamps inline as typed literals; numbers and booleans stay bindings"
        );
    }
}
