use super::*;

fn view_config(overrides: impl IntoIterator<Item = (&'static str, Value)>) -> Arc<MockJinjaObject> {
    let mut settings = BTreeMap::from([(
        "contract",
        Value::from_serialize(BTreeMap::from([("enforced", false)])),
    )]);
    settings.extend(overrides);
    let mock = default_mock_config();
    mock.on("get", move |args| {
        let key = args.first().and_then(Value::as_str).unwrap_or_default();
        Ok(settings
            .get(key)
            .cloned()
            .or_else(|| args.get(1).cloned())
            .unwrap_or(Value::UNDEFINED))
    });
    mock
}

fn row_filter_config() -> Arc<MockJinjaObject> {
    view_config([
        ("view_update_via_alter", Value::from(true)),
        (
            "row_filter",
            Value::from_serialize(BTreeMap::from([
                ("function", Value::from("filter_id")),
                ("columns", Value::from_serialize(["id"])),
            ])),
        ),
    ])
}

fn model_with_masked_column() -> Value {
    Value::from_serialize(BTreeMap::from([
        ("alias", Value::from("my_view")),
        ("unique_id", Value::from("model.test_project.my_view")),
        (
            "columns",
            Value::from_serialize(BTreeMap::from([(
                "msg",
                BTreeMap::from([("column_mask", BTreeMap::from([("function", "mask_msg")]))]),
            )])),
        ),
    ]))
}

#[test]
fn v2_update_via_alter_rejects_unsupported_configs_without_changes() {
    let cases = [
        (
            "row filter",
            Value::from_dyn_object(row_filter_config()),
            None,
            "Row filters are not supported for views.",
        ),
        (
            "column mask",
            Value::from_dyn_object(update_via_alter_config(false, None)),
            Some(model_with_masked_column()),
            "Column masks are not supported for views.",
        ),
    ];
    for (case, config, model, expected) in cases {
        let harness = existing_view_alter_harness(true);
        let mut builder = harness
            .materialization_context("my_view", "SELECT id, msg FROM source_table")
            .config(config);
        if let Some(model) = model {
            builder = builder.with("model", model);
        }
        let error = render_view(&harness, ADAPTER, builder.build())
            .expect_err("an existing view with no changes must still be validated");
        assert!(
            error.to_string().contains(expected),
            "{case}: unexpected error {error}"
        );
    }
}

fn assert_invalid_contract_rejected(changeset: Value) {
    let harness = existing_view_alter_harness(true);
    let model_config = Arc::new(MockJinjaObject::new());
    model_config.on("get_changeset", move |_| Ok(changeset.clone()));
    harness.mock().on("get_config_from_model", move |_| {
        Ok(Value::from_dyn_object(Arc::clone(&model_config)))
    });
    let config = view_config([
        ("view_update_via_alter", Value::from(true)),
        (
            "contract",
            Value::from_serialize(BTreeMap::from([("enforced", true)])),
        ),
    ]);
    let ctx = harness
        .materialization_context("my_view", "SELECT id, msg FROM source_table")
        .config(Value::from_dyn_object(config))
        .with(
            "flags",
            Value::from_serialize(BTreeMap::from([("WHICH", "run")])),
        )
        .build();
    let error = render_view(&harness, ADAPTER, ctx)
        .expect_err("an existing view must reject an enforced contract without declared columns");
    assert!(
        error.to_string().contains("enforced contract"),
        "unexpected error: {error}"
    );
    assert!(executed_sql(harness.mock()).is_empty());
}

#[test]
fn v2_update_via_alter_rejects_an_invalid_contract_without_changes() {
    assert_invalid_contract_rejected(Value::UNDEFINED);
}

#[test]
fn v2_update_via_alter_rejects_an_invalid_contract_with_query_changes() {
    assert_invalid_contract_rejected(Value::from_serialize(BTreeMap::from([
        ("requires_full_refresh", Value::from(false)),
        (
            "changes",
            Value::from_serialize(BTreeMap::from([(
                "query",
                BTreeMap::from([("query", "select 2 as id")]),
            )])),
        ),
    ])));
}
