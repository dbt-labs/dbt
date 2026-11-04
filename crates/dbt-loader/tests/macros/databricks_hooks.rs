use std::collections::BTreeMap;

use dbt_adapter_core::AdapterType;
use dbt_jinja_ctx::objects::run::HookConfig;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, executed_sql};

#[test]
fn run_hooks_executes_matching_nonempty_hooks_in_order() {
    for inside_transaction in [false, true] {
        let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
            .load_all_macros()
            .with_stub_functions()
            .build()
            .expect("harness should build");
        let hooks = [
            ("select 'outside'", false),
            ("select 'ordinary'", true),
            ("  ", inside_transaction),
            ("select 'last'", inside_transaction),
        ]
        .into_iter()
        .map(|(sql, transaction)| {
            Value::from_object(HookConfig {
                sql: sql.to_string(),
                transaction,
            })
        })
        .collect::<Vec<_>>();
        let ctx = BTreeMap::from([
            ("hooks".to_string(), Value::from(hooks)),
            (
                "inside_transaction".to_string(),
                Value::from(inside_transaction),
            ),
            ("execute".to_string(), Value::from(true)),
            (
                "TARGET_PACKAGE_NAME".to_string(),
                Value::from("test_project"),
            ),
        ]);
        harness
            .render("{{ run_hooks(hooks, inside_transaction) }}", ctx)
            .expect("run_hooks should render");

        let executed = executed_sql(harness.mock())
            .iter()
            .map(|sql| sql.trim().to_string())
            .collect::<Vec<_>>();
        let first = if inside_transaction {
            "select 'ordinary'"
        } else {
            "select 'outside'"
        };
        assert_eq!(executed, [first, "select 'last'"]);
    }
}
