use super::*;

fn render_new_view_under(
    command: &str,
    config: Arc<MockJinjaObject>,
    model: Value,
) -> (MacroTestHarness, dbt_common::FsResult<String>) {
    let harness = build_view_harness(ADAPTER);
    harness.mock().on("get_relation", |_| Ok(Value::from(())));
    let ctx = harness
        .materialization_context("my_view", "select * from prod_schema.my_view")
        .config(Value::from_dyn_object(config))
        .with("model", model)
        .with("flags", flags_for(command))
        .build();
    let result = render_view(&harness, ADAPTER, ctx);
    (harness, result)
}

#[test]
fn enforced_contract_is_not_asserted_under_clone() {
    for (command, asserted) in [("run", true), ("clone", false)] {
        let (harness, result) = render_new_view_under(
            command,
            view_config([(
                "contract",
                Value::from_serialize(BTreeMap::from([("enforced", true)])),
            )]),
            governed_model("view", false),
        );
        if asserted {
            let error = result.expect_err("run must assert the enforced contract");
            assert!(
                error.to_string().contains("enforced contract"),
                "{command}: unexpected error {error}"
            );
        } else {
            result.unwrap_or_else(|e| panic!("{command}: clone must skip the contract: {e}"));
            assert_executed_contains(harness.mock(), "create or replace view");
        }
    }
}

fn governed_model(materialized: &str, masked: bool) -> Value {
    let columns = if masked {
        BTreeMap::from([(
            "msg",
            BTreeMap::from([("column_mask", BTreeMap::from([("function", "mask_msg")]))]),
        )])
    } else {
        BTreeMap::new()
    };
    Value::from_serialize(BTreeMap::from([
        ("alias", Value::from("my_view")),
        ("unique_id", Value::from("model.test_project.my_view")),
        ("columns", Value::from_serialize(columns)),
        (
            "config",
            Value::from_serialize(BTreeMap::from([("materialized", materialized)])),
        ),
    ]))
}

#[test]
fn clone_skips_policy_rejection_only_for_materialized_views_and_streaming_tables() {
    let cases = [
        ("run", "view", true),
        ("run", "materialized_view", true),
        ("clone", "view", true),
        ("clone", "materialized_view", false),
        ("clone", "streaming_table", false),
    ];
    for (command, materialized, rejected) in cases {
        for (policy, masked, message) in [
            (
                "column mask",
                true,
                "Column masks are not supported for views.",
            ),
            (
                "row filter",
                false,
                "Row filters are not supported for views.",
            ),
        ] {
            let config = if masked {
                default_mock_config()
            } else {
                row_filter_config()
            };
            let (harness, result) =
                render_new_view_under(command, config, governed_model(materialized, masked));
            let case = format!("{command}/{materialized}/{policy}");
            if rejected {
                let error = result.expect_err(&format!("{case}: must be rejected"));
                assert!(
                    error.to_string().contains(message),
                    "{case}: unexpected error {error}"
                );
            } else {
                result.unwrap_or_else(|e| panic!("{case}: must clone as a view: {e}"));
                assert_executed_contains(harness.mock(), "create or replace view");
            }
        }
    }
}
