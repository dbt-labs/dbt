use super::*;

#[test]
fn enforced_contract_is_not_asserted_under_clone() {
    for (command, asserted) in [("run", true), ("clone", false)] {
        let harness = build_view_harness(ADAPTER);
        harness.mock().on("get_relation", |_| Ok(Value::from(())));
        let ctx = harness
            .materialization_context("my_view", "select * from prod_schema.my_view")
            .config(Value::from_dyn_object(view_config([(
                "contract",
                Value::from_serialize(BTreeMap::from([("enforced", true)])),
            )])))
            .with(
                "flags",
                Value::from_serialize(BTreeMap::from([("WHICH", command)])),
            )
            .build();
        let result = render_view(&harness, ADAPTER, ctx);
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
