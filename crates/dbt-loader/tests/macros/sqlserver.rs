use std::collections::BTreeMap;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

const ADAPTER: AdapterType = AdapterType::SqlServer;

fn build_harness() -> MacroTestHarness {
    // `sqlserver__get_columns_in_query` reads `.table.columns['name'].values()`.
    let columns_in_query = Value::from_serialize(BTreeMap::from([(
        "table",
        BTreeMap::from([(
            "columns",
            BTreeMap::from([("name", BTreeMap::from([("0", "ID"), ("1", "Val")]))]),
        )]),
    )]));
    let harness = MacroTestHarness::for_adapter(ADAPTER)
        .load_all_macros()
        .with_stub_functions()
        .with_global(
            "load_result",
            Value::from_function(move |_name: Value| Ok(columns_in_query.clone())),
        )
        .build()
        .expect("harness should build");
    harness.mock().on("quote", |args| {
        Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
    });
    harness
}

/// dbt-msft/dbt-sqlserver#865: T-SQL can't nest the snapshot's `WITH` in
/// upstream's `select <check_cols> from (<sql>) subq`.
mod snapshot_check_cols {
    use super::*;

    fn render(check_cols: &str) -> Result<String, String> {
        let harness = build_harness();
        let target = harness.relation("TestDB", "dbo", "snap", Some(RelationType::Table));
        harness.mock().on("get_relation", move |_| {
            Ok(RelationObject::new(target.clone()).into_value())
        });
        harness.mock().on("get_columns_in_relation", |_| {
            Ok(Value::from_serialize(vec![
                BTreeMap::from([("name", "ID")]),
                BTreeMap::from([("name", "Val")]),
            ]))
        });
        let node = Value::from_serialize(BTreeMap::from([
            ("database", "TestDB"),
            ("schema", "dbo"),
            ("alias", "snap"),
            (
                "compiled_code",
                "with src as (select 1 as id, 'a' as val) select * from src",
            ),
        ]));
        let template = format!(
            "{{% set r = snapshot_check_all_get_existing_columns(node, true, {check_cols}) %}}{{{{ r[0] }}}}|{{{{ r[1] | join(',') }}}}"
        );
        harness
            .render(
                &template,
                BTreeMap::from([
                    ("node".to_string(), node),
                    ("execute".to_string(), Value::from(true)),
                    // A snapshot renders under its own package. Absent, unprefixed
                    // lookup starts in `dbt` and finds the global macro first.
                    (
                        "TARGET_PACKAGE_NAME".to_string(),
                        Value::from("test_project"),
                    ),
                ]),
            )
            .map_err(|e| format!("{e:?}"))
    }

    #[test]
    fn unknown_check_col_is_a_compiler_error() {
        let err = render("['missing']").expect_err("render should fail");
        assert!(
            err.contains("check_cols column 'missing' is not in the snapshot query"),
            "got: {err}"
        );
    }
}
