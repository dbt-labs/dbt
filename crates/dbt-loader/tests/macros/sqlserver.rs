use std::collections::BTreeMap;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, default_mock_config, executed_sql};

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

#[test]
fn array_append_uses_json_modify() {
    let harness = build_harness();
    let sql = harness
        .render(
            "{{ array_append(array_construct([1, 2]), 3) }}",
            BTreeMap::<String, Value>::new(),
        )
        .expect("render should succeed");
    assert_eq!(sql.trim(), "JSON_MODIFY(JSON_ARRAY(1 , 2), 'append $', 3)");
}

/// dbt-msft/dbt-sqlserver#836: the four-step rewrite autocommits each step, so
/// a failed run leaves `<col>__dbt_alter` behind and the next `ADD` fails.
#[test]
fn alter_column_type_drops_leftover_tmp_column_before_add() {
    let harness = build_harness();
    let relation = harness.relation("TestDB", "dbo", "my_tbl", Some(RelationType::Table));
    let ctx = BTreeMap::from([
        (
            "relation".to_string(),
            RelationObject::new(relation).into_value(),
        ),
        (
            "config".to_string(),
            Value::from_dyn_object(default_mock_config()),
        ),
        ("execute".to_string(), Value::from(true)),
    ]);
    harness
        .render(
            "{% do alter_column_type(relation, 'c', 'varchar(9)') %}",
            ctx,
        )
        .expect("render should succeed");

    let sqls = executed_sql(harness.mock());
    assert_eq!(sqls.len(), 5, "got: {sqls:?}");
    let drop_leftover = sqls[0].split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        drop_leftover.contains("if col_length('\"dbo\".\"my_tbl\"', 'c__dbt_alter') is not null")
            && drop_leftover.contains("and col_length('\"dbo\".\"my_tbl\"', 'c') is not null")
            && drop_leftover.ends_with("drop column \"c__dbt_alter\";"),
        "got: {drop_leftover}"
    );
    assert!(
        sqls[1].contains("add \"c__dbt_alter\" varchar(9)"),
        "got: {sqls:?}"
    );
}

/// dbt-msft/dbt-sqlserver#865: T-SQL can't nest the snapshot's `WITH` in
/// upstream's `select <check_cols> from (<sql>) subq`.
mod snapshot_check_cols {
    use super::*;

    fn render(check_cols: &str) -> (Result<String, String>, Vec<String>) {
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
        let out = harness
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
            .map_err(|e| format!("{e:?}"));
        (out, executed_sql(harness.mock()))
    }

    #[test]
    fn reads_columns_without_nesting_the_query() {
        let (out, sqls) = render("['val']");
        // `Val` keeps the query's casing, so it matches the existing column.
        assert_eq!(out.expect("render should succeed"), "False|Val");
        assert_eq!(sqls.len(), 1, "got: {sqls:?}");
        assert!(
            sqls[0].contains("sp_describe_first_result_set"),
            "got: {sqls:?}"
        );
        assert!(!sqls[0].contains("subq"), "got: {sqls:?}");
    }

    #[test]
    fn unknown_check_col_is_a_compiler_error() {
        let (out, _) = render("['missing']");
        let err = out.expect_err("render should fail");
        assert!(
            err.contains("check_cols column 'missing' is not in the snapshot query"),
            "got: {err}"
        );
    }
}
