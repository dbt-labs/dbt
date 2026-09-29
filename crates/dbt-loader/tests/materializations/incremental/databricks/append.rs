use super::*;

const APPEND_SQL: &str = "INSERT INTO target SELECT * FROM source";
const DATABRICKS_STRATEGIES_SQL: &str = include_str!(
    "../../../../src/dbt_macro_assets/dbt-databricks/macros/materializations/incremental/strategies.sql"
);

fn normalize_sql(sql: &str) -> String {
    sql.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn mock_insert_columns(harness: &MacroTestHarness) {
    harness.mock().on("get_columns_in_relation", |_| {
        Ok(Value::from_serialize(vec![
            BTreeMap::from([("name", "id")]),
            BTreeMap::from([("name", "name")]),
        ]))
    });
}

fn insert_context(harness: &MacroTestHarness) -> BTreeMap<String, Value> {
    let target = harness.relation(
        "TEST_DB",
        "TEST_SCHEMA",
        "my_incr",
        Some(RelationType::Table),
    );
    let source = harness.relation(
        "TEST_DB",
        "TEST_SCHEMA",
        "my_incr__dbt_tmp",
        Some(RelationType::Table),
    );
    BTreeMap::from([
        (
            "source".to_string(),
            RelationObject::new(source).into_value(),
        ),
        (
            "target".to_string(),
            RelationObject::new(target).into_value(),
        ),
    ])
}

fn existing_table_harness(materialization_v2: bool) -> MacroTestHarness {
    let harness = build_harness_with_materialization_v2(materialization_v2);
    let existing = harness.relation(
        "TEST_DB",
        "TEST_SCHEMA",
        "my_incr",
        Some(RelationType::Table),
    );
    harness.mock().on("get_relation", move |_| {
        Ok(RelationObject::new(Arc::clone(&existing)).into_value())
    });
    harness
}

fn mock_incremental_strategy(harness: &MacroTestHarness) {
    harness
        .mock()
        .on("get_relation_config", |_| Ok(Value::UNDEFINED));

    let model_config = Arc::new(MockJinjaObject::new());
    model_config.on("get_changeset", |_| Ok(Value::from(())));
    let model_config = Value::from_dyn_object(model_config);
    harness
        .mock()
        .on("get_config_from_model", move |_| Ok(model_config.clone()));
    harness.mock().on("get_incremental_strategy_macro", |args| {
        let selected = args.last().and_then(Value::as_str);
        if selected != Some("append") {
            return Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("expected append strategy, got {selected:?}"),
            ));
        }
        Ok(Value::from_function(
            |_args: &[Value]| -> Result<Value, minijinja::Error> { Ok(Value::from(APPEND_SQL)) },
        ))
    });
}

fn mock_v2_creation(harness: &MacroTestHarness) {
    harness.mock().on("get_columns_in_relation", |_| {
        Ok(Value::from_serialize(Vec::<BTreeMap<String, Value>>::new()))
    });
    harness.mock().on("parse_columns_and_constraints", |_| {
        Ok(Value::from_serialize((
            Vec::<Value>::new(),
            Vec::<Value>::new(),
        )))
    });
}

fn render_append(harness: &MacroTestHarness, full_refresh: bool) {
    let ctx = incremental_ctx_with_config(
        harness,
        incremental_config_with(Some("append"), full_refresh),
    );
    render_incremental(harness, ADAPTER, ctx)
        .unwrap_or_else(|e| panic!("incremental append materialization failed: {e:?}"));
}

fn assert_append_executed(harness: &MacroTestHarness) {
    let sqls = executed_sql(harness.mock());
    assert!(
        sqls.iter()
            .any(|sql| sql.to_lowercase().contains(&APPEND_SQL.to_lowercase())),
        "expected append DML after the temp relation, got: {sqls:?}",
    );
}

fn assert_create_without_incremental_strategy(harness: &MacroTestHarness) {
    assert_executed_contains(harness.mock(), "create");
    harness
        .mock()
        .observed_calls()
        .assert_not_called("get_incremental_strategy_macro");
}

fn assert_existing_table_append(materialization_v2: bool) {
    let harness = existing_table_harness(materialization_v2);
    mock_incremental_strategy(&harness);

    render_append(&harness, false);

    assert_append_executed(&harness);
}

fn assert_existing_table_full_refresh(materialization_v2: bool) {
    let harness = existing_table_harness(materialization_v2);
    if materialization_v2 {
        mock_v2_creation(&harness);
    }

    render_append(&harness, true);

    assert_create_without_incremental_strategy(&harness);
}

#[test]
fn append_strategy_renders_insert_into_target() {
    let harness = MacroTestHarness::for_adapter(ADAPTER)
        .with_macro_at_path(
            "dbt_databricks",
            "databricks__get_incremental_append_sql",
            DATABRICKS_STRATEGIES_SQL,
            "dbt_macro_assets/dbt-databricks/macros/materializations/incremental/strategies.sql",
        )
        .with_macro_at_path(
            "dbt_databricks",
            "get_insert_into_sql",
            DATABRICKS_STRATEGIES_SQL,
            "dbt_macro_assets/dbt-databricks/macros/materializations/incremental/strategies.sql",
        )
        .with_macro_at_path(
            "dbt_databricks",
            "insert_into_sql_impl",
            DATABRICKS_STRATEGIES_SQL,
            "dbt_macro_assets/dbt-databricks/macros/materializations/incremental/strategies.sql",
        )
        .build()
        .expect("Databricks append strategy harness should build");
    harness
        .mock()
        .on("has_dbr_capability", |_| Ok(Value::from(false)));
    mock_insert_columns(&harness);
    let ctx = insert_context(&harness);

    let append_sql = harness
        .render(
            "{{ databricks__get_incremental_append_sql({'temp_relation': source, 'target_relation': target}) }}",
            ctx.clone(),
        )
        .unwrap_or_else(|e| panic!("rendering Databricks append SQL failed: {e:?}"));
    let public_helper_sql = harness
        .render("{{ get_insert_into_sql(source, target) }}", ctx)
        .unwrap_or_else(|e| panic!("rendering public insert SQL helper failed: {e:?}"));
    let normalized = normalize_sql(&append_sql);

    assert!(
        normalized.contains("insert into `test_db`.`test_schema`.`my_incr`"),
        "expected the persistent relation to be the INSERT target, got: {append_sql}",
    );
    assert!(
        normalized.contains("select * from `test_db`.`test_schema`.`my_incr__dbt_tmp`"),
        "expected the temporary relation to be the SELECT source, got: {append_sql}",
    );
    assert!(
        !normalized.contains("create or replace"),
        "got: {append_sql}"
    );
    assert_eq!(normalize_sql(&public_helper_sql), normalized);
}

#[test]
fn existing_table_append_uses_incremental_statement() {
    assert_existing_table_append(false);
}

#[test]
fn existing_table_full_refresh_recreates_instead_of_appending() {
    assert_existing_table_full_refresh(false);
}

#[test]
fn v2_no_existing_relation_creates_table() {
    let harness = build_harness_with_materialization_v2(true);
    harness.mock().on("get_relation", |_| Ok(Value::from(())));
    mock_v2_creation(&harness);

    render_append(&harness, false);

    assert_create_without_incremental_strategy(&harness);
}

#[test]
fn v2_existing_table_append_uses_incremental_statement() {
    assert_existing_table_append(true);
}

#[test]
fn v2_existing_table_full_refresh_recreates_instead_of_appending() {
    assert_existing_table_full_refresh(true);
}
