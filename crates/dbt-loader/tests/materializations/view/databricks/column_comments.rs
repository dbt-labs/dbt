use super::*;

#[test]
fn alter_view_query_change_reapplies_post_alter_comments_only() {
    let harness = build_view_harness(ADAPTER);
    harness
        .mock()
        .on("resolve_file_format", |_| Ok(Value::from("parquet")));
    harness.mock().on("has_feature", |_| Ok(Value::from(true)));
    harness.mock().on("quote", |args| {
        let name = args.first().and_then(|v| v.as_str()).unwrap_or_default();
        Ok(Value::from(format!("`{name}`")))
    });
    harness.mock().on("get_columns_in_relation", |_| {
        Ok(Value::from(Vec::<Value>::new()))
    });
    let id_column = BTreeMap::from([(
        "id",
        BTreeMap::from([("name", "id"), ("description", "Updated id")]),
    )]);
    let persisted_columns = id_column.clone();
    harness.mock().on("get_persist_doc_columns", move |_| {
        Ok(Value::from_serialize(&persisted_columns))
    });
    let config = default_mock_config();
    config.on("persist_column_docs", |_| Ok(Value::from(true)));
    let changes = Value::from_serialize(BTreeMap::from([
        (
            "query",
            Value::from_serialize(BTreeMap::from([("query", "select 1 as id")])),
        ),
        (
            "column_comments",
            Value::from_serialize(BTreeMap::from([(
                "comments",
                BTreeMap::from([("msg", "Stale msg")]),
            )])),
        ),
    ]));
    let model = Value::from_serialize(BTreeMap::from([
        ("alias", Value::from("my_view")),
        ("unique_id", Value::from("model.test_project.my_view")),
        ("columns", Value::from_serialize(&id_column)),
    ]));
    let ctx = harness
        .materialization_context("my_view", "select 1 as id")
        .config(Value::from_dyn_object(config))
        .with("model", model)
        .with("changes", changes)
        .build();
    harness
        .render("{{ alter_view(this, changes) }}", ctx)
        .expect("alter_view should render");

    let executed = executed_sql(harness.mock());
    assert!(
        executed
            .iter()
            .any(|sql| sql.contains("COMMENT ON COLUMN") && sql.contains("Updated id")),
        "post-ALTER comment must be written regardless of file_format: {executed:?}"
    );
    assert!(
        !executed.iter().any(|sql| sql.contains("Stale msg")),
        "the pre-ALTER comment diff must not run after a query change: {executed:?}"
    );
}
