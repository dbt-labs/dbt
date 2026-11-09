use std::collections::BTreeMap;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, default_mock_config};

#[test]
fn test_singlestore_cast_macro() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let rendered = harness
        .render("{{ cast('x', 'int') }}", BTreeMap::<String, Value>::new())
        .expect("cast should render");
    assert_eq!(rendered.trim(), "(x :> int)");

    let rendered = harness
        .render(
            "{{ cast('null', 'decimal(15,2)') }}",
            BTreeMap::<String, Value>::new(),
        )
        .expect("cast should render");
    assert_eq!(rendered.trim(), "(null :> decimal(15,2))");
}

#[test]
fn test_singlestore_get_empty_schema_sql_uses_singlestore_cast() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let columns = BTreeMap::from([
        (
            "id".to_string(),
            BTreeMap::from([
                ("name".to_string(), "id".to_string()),
                ("data_type".to_string(), "bigint".to_string()),
            ]),
        ),
        (
            "amount".to_string(),
            BTreeMap::from([
                ("name".to_string(), "amount".to_string()),
                ("data_type".to_string(), "decimal(15,2)".to_string()),
            ]),
        ),
    ]);

    let ctx = BTreeMap::from([("columns".to_string(), Value::from_serialize(columns))]);

    let rendered = harness
        .render("{{ get_empty_schema_sql(columns) }}", ctx)
        .expect("get_empty_schema_sql should render");
    let normalized = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(normalized.contains("(null :> bigint) as id"));
    assert!(normalized.contains("(null :> decimal(15,2)) as amount"));
}

#[test]
fn test_singlestore_create_view_renders() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    let relation = harness.relation("my_db", "my_schema", "test_view", Some(RelationType::View));
    let rel_val = RelationObject::new(relation).into_value();

    let sql = "select * from (\n-- EPHEMERAL-SELECT-WRAPPER-START\nselect 1\n-- EPHEMERAL-SELECT-WRAPPER-END\n) as t";
    let ctx = BTreeMap::from([
        ("sql".to_string(), Value::from(sql)),
        ("relation".to_string(), rel_val),
    ]);

    let rendered = harness
        .render("{{ create_view_as(relation, sql) }}", ctx)
        .expect("create_view_as should render");
    assert!(rendered.contains("create view `my_db`.`test_view` as"));
    assert!(rendered.contains("-- EPHEMERAL-SELECT-WRAPPER-START"));
    assert!(rendered.contains("-- EPHEMERAL-SELECT-WRAPPER-END"));
}

#[test]
fn test_singlestore_delete_insert_quotes_keys() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    harness
        .mock()
        .on("quote", |args| Ok(Value::from(format!("`{}`", args[0]))));

    let target = harness.relation("my_db", "my_schema", "test_tbl", Some(RelationType::Table));
    let source = harness.relation(
        "my_db",
        "my_schema",
        "test_tbl__dbt_tmp",
        Some(RelationType::Table),
    );
    let target_val = RelationObject::new(target).into_value();
    let source_val = RelationObject::new(source).into_value();

    let dest_columns = vec![
        BTreeMap::from([("name".to_string(), Value::from("order_id"))]),
        BTreeMap::from([("name".to_string(), Value::from("user_id"))]),
        BTreeMap::from([("name".to_string(), Value::from("amount"))]),
    ];

    // 1. Composite key
    let ctx = BTreeMap::from([
        ("target".to_string(), target_val.clone()),
        ("source".to_string(), source_val.clone()),
        (
            "unique_key".to_string(),
            Value::from(vec!["order_id", "user_id"]),
        ),
        (
            "dest_columns".to_string(),
            Value::from_serialize(dest_columns.clone()),
        ),
        (
            "incremental_predicates".to_string(),
            Value::from(Vec::<String>::new()),
        ),
    ]);

    let rendered = harness
        .render("{{ singlestore__get_delete_insert_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) }}", ctx)
        .expect("should render delete_insert with composite key");

    assert!(rendered.contains("delete `test_tbl` from `my_db`.`test_tbl`"));
    assert!(
        rendered.contains("`my_db`.`test_tbl__dbt_tmp`.`order_id` = `my_db`.`test_tbl`.`order_id`")
    );
    assert!(
        rendered.contains("`my_db`.`test_tbl__dbt_tmp`.`user_id` = `my_db`.`test_tbl`.`user_id`")
    );
    assert!(rendered.contains("insert into `my_db`.`test_tbl` (`order_id`, `user_id`, `amount`)"));

    // 2. Single key string
    let ctx = BTreeMap::from([
        ("target".to_string(), target_val),
        ("source".to_string(), source_val),
        ("unique_key".to_string(), Value::from("order_id")),
        (
            "dest_columns".to_string(),
            Value::from_serialize(dest_columns),
        ),
        (
            "incremental_predicates".to_string(),
            Value::from(Vec::<String>::new()),
        ),
    ]);

    let rendered = harness
        .render("{{ singlestore__get_delete_insert_merge_sql(target, source, unique_key, dest_columns, incremental_predicates) }}", ctx)
        .expect("should render delete_insert with single key");

    assert!(rendered.contains("delete from `my_db`.`test_tbl`"));
    assert!(rendered.contains("`order_id`) in ("));
    assert!(rendered.contains("select (`order_id`)"));
    assert!(rendered.contains("from `my_db`.`test_tbl__dbt_tmp`"));
}

#[test]
fn test_singlestore_merge_missing_unique_key_errors() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    let target = harness.relation("my_db", "my_schema", "test_tbl", Some(RelationType::Table));
    let source = harness.relation(
        "my_db",
        "my_schema",
        "test_tbl__dbt_tmp",
        Some(RelationType::Table),
    );
    let target_val = RelationObject::new(target).into_value();
    let source_val = RelationObject::new(source).into_value();

    let dest_columns = vec![BTreeMap::from([(
        "name".to_string(),
        Value::from("order_id"),
    )])];

    let arg_dict = BTreeMap::from([
        ("target_relation".to_string(), target_val),
        ("temp_relation".to_string(), source_val),
        ("unique_key".to_string(), Value::from(())),
        (
            "dest_columns".to_string(),
            Value::from_serialize(dest_columns),
        ),
    ]);

    let ctx = BTreeMap::from([("arg_dict".to_string(), Value::from_serialize(arg_dict))]);

    let res = harness.render(
        "{{ singlestore__get_incremental_merge_sql(arg_dict) }}",
        ctx,
    );
    assert!(res.is_err());
    let err_msg = res.unwrap_err().to_string();
    assert!(err_msg.contains("requires a 'unique_key' configuration"));
}

#[test]
fn test_singlestore_merge_no_matching_index_in_statistics_errors() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .with_stub_functions()
        .with_global("execute", Value::from(true))
        .build()
        .expect("harness should build");

    let target = harness.relation("my_db", "my_schema", "test_tbl", Some(RelationType::Table));
    let source = harness.relation(
        "my_db",
        "my_schema",
        "test_tbl__dbt_tmp",
        Some(RelationType::Table),
    );
    let target_val = RelationObject::new(target).into_value();
    let source_val = RelationObject::new(source).into_value();

    let dest_columns = vec![BTreeMap::from([(
        "name".to_string(),
        Value::from("order_id"),
    )])];

    let arg_dict = BTreeMap::from([
        ("target_relation".to_string(), target_val),
        ("temp_relation".to_string(), source_val),
        ("unique_key".to_string(), Value::from("order_id")),
        (
            "dest_columns".to_string(),
            Value::from_serialize(dest_columns),
        ),
    ]);

    let ctx = BTreeMap::from([("arg_dict".to_string(), Value::from_serialize(arg_dict))]);

    let res = harness.render(
        "{{ singlestore__get_incremental_merge_sql(arg_dict) }}",
        ctx,
    );
    assert!(res.is_err());
    let err_msg = res.unwrap_err().to_string();
    assert!(err_msg.contains("Found no matching unique index in information_schema.statistics"));
}

#[test]
fn test_singlestore_snapshot_string_as_time() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let rendered = harness
        .render(
            "{{ snapshot_string_as_time('2026-11-02 17:00:00') }}",
            BTreeMap::<String, Value>::new(),
        )
        .expect("snapshot_string_as_time should render");
    assert_eq!(rendered.trim(), "'2026-11-02 17:00:00' :> datetime");
}

#[test]
fn test_singlestore_get_dbt_valid_to_current() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let strategy = BTreeMap::from([("updated_at".to_string(), Value::from("_updated_at"))]);
    let columns = BTreeMap::from([("dbt_valid_to".to_string(), Value::from("dbt_valid_to"))]);
    let ctx = BTreeMap::from([
        ("strategy".to_string(), Value::from_serialize(strategy)),
        ("columns".to_string(), Value::from_serialize(columns)),
    ]);

    let rendered = harness
        .render(
            "{{ singlestore__get_dbt_valid_to_current(strategy, columns) }}",
            ctx,
        )
        .expect("singlestore__get_dbt_valid_to_current should render");
    let normalized = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(normalized.contains(":> datetime) :> datetime) as dbt_valid_to"));
}

/// Context for the snapshot macros: a timestamp strategy with
/// `hard_deletes='new_record'`, so every SingleStore-specific branch renders.
fn snapshot_ctx(harness: &MacroTestHarness) -> BTreeMap<String, Value> {
    let strategy = BTreeMap::from([
        ("unique_key".to_string(), Value::from("id")),
        ("updated_at".to_string(), Value::from("_updated_at")),
        ("scd_id".to_string(), Value::from("md5(id)")),
        ("row_changed".to_string(), Value::from("true")),
        ("hard_deletes".to_string(), Value::from("new_record")),
    ]);
    let target = harness.relation("my_db", "my_db", "snap", Some(RelationType::Table));
    BTreeMap::from([
        ("strategy".to_string(), Value::from_serialize(strategy)),
        ("source_sql".to_string(), Value::from("select 1 as id")),
        (
            "target_relation".to_string(),
            RelationObject::new(target).into_value(),
        ),
        (
            "config".to_string(),
            Value::from_dyn_object(default_mock_config()),
        ),
    ])
}

#[test]
fn test_singlestore_snapshot_staging_table() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    let rendered = harness
        .render(
            "{{ snapshot_staging_table(strategy, source_sql, target_relation) }}",
            snapshot_ctx(&harness),
        )
        .expect("snapshot_staging_table should render");

    // MySQL-family engines only start a comment at `--` followed by whitespace.
    assert!(!rendered.contains("--avoid"), "{rendered}");
    assert_eq!(rendered.matches("-- avoid").count(), 2, "{rendered}");
    let normalized = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized.contains(":> datetime) :> datetime) as dbt_valid_to"),
        "{normalized}"
    );
}

#[test]
fn test_singlestore_build_snapshot_table_casts_valid_to() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    let rendered = harness
        .render(
            "{{ build_snapshot_table(strategy, source_sql) }}",
            snapshot_ctx(&harness),
        )
        .expect("build_snapshot_table should render");
    let normalized = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized.contains(":> datetime) :> datetime) as dbt_valid_to"),
        "{normalized}"
    );
    assert!(
        normalized.contains("'False' as dbt_is_deleted"),
        "{normalized}"
    );
}

#[test]
fn test_singlestore_alter_relation_comment() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let table = harness.relation("my_db", "my_schema", "test_tbl", Some(RelationType::Table));
    let view = harness.relation("my_db", "my_schema", "test_view", Some(RelationType::View));

    let table_val = RelationObject::new(table).into_value();
    let view_val = RelationObject::new(view).into_value();

    // 1. Table relation comment
    let ctx = BTreeMap::from([
        ("relation".to_string(), table_val.clone()),
        ("comment".to_string(), Value::from("Orders table summary")),
    ]);
    let rendered = harness
        .render(
            "{{ singlestore__alter_relation_comment(relation, comment) }}",
            ctx,
        )
        .expect("alter_relation_comment should render for table");
    assert_eq!(
        rendered.trim(),
        "alter table `my_db`.`test_tbl` comment = 'Orders table summary'"
    );

    // 2. Table relation comment with escaping (' and \)
    let ctx = BTreeMap::from([
        ("relation".to_string(), table_val),
        (
            "comment".to_string(),
            Value::from("It's a table with trailing\\"),
        ),
    ]);
    let rendered = harness
        .render(
            "{{ singlestore__alter_relation_comment(relation, comment) }}",
            ctx,
        )
        .expect("alter_relation_comment should escape correctly");
    assert_eq!(
        rendered.trim(),
        "alter table `my_db`.`test_tbl` comment = 'It''s a table with trailing\\\\'"
    );

    // 3. View relation comment should safely render empty (SingleStore Error 1347: not a BASE TABLE)
    let ctx = BTreeMap::from([
        ("relation".to_string(), view_val.clone()),
        ("comment".to_string(), Value::from("View comment")),
    ]);
    let rendered = harness
        .render(
            "{{ singlestore__alter_relation_comment(relation, comment) }}",
            ctx,
        )
        .expect("alter_relation_comment should render empty for view");
    assert_eq!(rendered.trim(), "");

    // 4. View column comment should safely render empty
    let columns = BTreeMap::from([(
        "id".to_string(),
        BTreeMap::from([("description".to_string(), "id column")]),
    )]);
    let ctx = BTreeMap::from([
        ("relation".to_string(), view_val),
        ("columns".to_string(), Value::from_serialize(columns)),
    ]);
    let rendered = harness
        .render(
            "{{ singlestore__alter_column_comment(relation, columns) }}",
            ctx,
        )
        .expect("alter_column_comment should render empty for view");
    assert_eq!(rendered.trim(), "");
}

#[test]
fn test_singlestore_alter_column_comment_with_defaults() {
    let mut harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    harness.mock().on("quote", |args| {
        let identifier = args
            .first()
            .and_then(Value::as_str)
            .expect("quote expects a string identifier");
        Ok(Value::from(format!("`{identifier}`")))
    });

    let table = harness.relation("my_db", "my_schema", "test_tbl", Some(RelationType::Table));
    let table_val = RelationObject::new(table).into_value();

    let mock_rows = vec![
        vec![
            Value::from("id"),
            Value::from("int(11)"),
            Value::from("not null"),
            Value::from("7"),
            Value::from("int"),
        ],
        vec![
            Value::from("s"),
            Value::from("varchar(10)"),
            Value::from("null"),
            Value::from("x"),
            Value::from("varchar"),
        ],
        vec![
            Value::from("ts"),
            Value::from("timestamp"),
            Value::from("not null"),
            Value::from("CURRENT_TIMESTAMP"),
            Value::from("timestamp"),
        ],
        vec![
            Value::from("no_def"),
            Value::from("varchar(20)"),
            Value::from("null"),
            Value::from(()),
            Value::from("varchar"),
        ],
    ];

    harness
        .env_mut()
        .env
        .add_function("load_result", move |_name: Value| {
            Ok(Value::from_serialize(BTreeMap::from([(
                "table",
                mock_rows.clone(),
            )])))
        });
    harness
        .env_mut()
        .env
        .add_global("execute", Value::from(true));

    let columns = BTreeMap::from([
        (
            "id".to_string(),
            BTreeMap::from([("description".to_string(), "id description")]),
        ),
        (
            "s".to_string(),
            BTreeMap::from([("description".to_string(), "it's with \\ and '")]),
        ),
        (
            "ts".to_string(),
            BTreeMap::from([("description".to_string(), "ts description")]),
        ),
        (
            "no_def".to_string(),
            BTreeMap::from([("description".to_string(), "no default column")]),
        ),
    ]);

    let ctx = BTreeMap::from([
        ("relation".to_string(), table_val),
        ("columns".to_string(), Value::from_serialize(columns)),
    ]);

    let rendered = harness
        .render(
            "{{ singlestore__alter_column_comment(relation, columns) }}",
            ctx,
        )
        .expect("alter_column_comment should render for table with defaults");

    let normalized = rendered.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        normalized
            .contains("modify column `id` int(11) not null default 7 comment 'id description'")
    );
    assert!(normalized.contains(
        "modify column `s` varchar(10) null default 'x' comment 'it''s with \\\\ and '''"
    ));
    assert!(normalized.contains(
        "modify column `ts` timestamp not null default CURRENT_TIMESTAMP comment 'ts description'"
    ));
    assert!(
        normalized.contains("modify column `no_def` varchar(20) null comment 'no default column'")
    );
}

#[test]
fn test_singlestore_generate_alias_name_s10_prefix() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .build()
        .expect("harness should build");

    // Case 1: Standard model in default database/schema: no prefix
    let node_standard = BTreeMap::from([
        ("name".to_string(), Value::from("orders")),
        ("database".to_string(), Value::from("dbt_ss_oss")),
        ("schema".to_string(), Value::from("dbt_ss_oss")),
    ]);
    let target = BTreeMap::from([
        ("database".to_string(), Value::from("dbt_ss_oss")),
        ("schema".to_string(), Value::from("dbt_ss_oss")),
    ]);
    let ctx = BTreeMap::from([
        ("node".to_string(), Value::from_serialize(node_standard)),
        ("target".to_string(), Value::from_serialize(target.clone())),
    ]);
    let rendered = harness
        .render("{{ singlestore__generate_alias_name(node=node) }}", ctx)
        .expect("should render alias");
    assert_eq!(rendered.trim(), "orders");

    // Case 2 (S10 bug): Custom database dbt_ss_oss_aux, inheriting default schema dbt_ss_oss: no prefix!
    let node_aux = BTreeMap::from([
        ("name".to_string(), Value::from("mrt_custom_database")),
        ("database".to_string(), Value::from("dbt_ss_oss_aux")),
        ("schema".to_string(), Value::from("dbt_ss_oss")),
    ]);
    let ctx = BTreeMap::from([
        ("node".to_string(), Value::from_serialize(node_aux)),
        ("target".to_string(), Value::from_serialize(target.clone())),
    ]);
    let rendered = harness
        .render("{{ singlestore__generate_alias_name(node=node) }}", ctx)
        .expect("should render alias");
    assert_eq!(rendered.trim(), "mrt_custom_database");

    // Case 3: Explicit custom schema `audit` (even without node.config.schema set): prefixed with schema!
    let node_custom_schema = BTreeMap::from([
        ("name".to_string(), Value::from("mrt_custom_schema")),
        ("database".to_string(), Value::from("dbt_ss_oss")),
        ("schema".to_string(), Value::from("dbt_ss_oss_audit")),
    ]);
    let ctx = BTreeMap::from([
        (
            "node".to_string(),
            Value::from_serialize(node_custom_schema.clone()),
        ),
        ("target".to_string(), Value::from_serialize(target)),
    ]);
    let rendered = harness
        .render("{{ generate_alias_name(none, node) }}", ctx.clone())
        .expect("should render alias");
    assert_eq!(rendered.trim(), "dbt_ss_oss_audit_mrt_custom_schema");

    // Case 4: Project-level generate_alias_name override is honoured without forced schema prefix
    let override_harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .with_root_package("override_project")
        .load_all_macros()
        .with_macro(
            "override_project",
            "generate_alias_name",
            "{% macro generate_alias_name(custom_alias_name=none, node=none) %}custom_{{ node.name }}{% endmacro %}",
        )
        .build()
        .expect("override harness should build");
    let rendered_override = dbt_jinja_utils::utils::generate_component_name_value(
        override_harness.env(),
        "alias",
        "override_project",
        "override_project",
        &ctx,
        None,
        Value::from_serialize(&node_custom_schema),
    )
    .expect("should render overridden alias");
    assert_eq!(rendered_override.trim(), "custom_mrt_custom_schema");
}

#[test]
fn test_singlestore_rename_relation_no_implicit_drop() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    let from = harness.relation("my_db", "my_db", "src_table", Some(RelationType::Table));
    let to = harness.relation("my_db", "my_db", "dest_table", Some(RelationType::Table));

    let from_val = RelationObject::new(from).into_value();
    let to_val = RelationObject::new(to).into_value();

    let ctx = BTreeMap::from([
        ("from_relation".to_string(), from_val),
        ("to_relation".to_string(), to_val),
        ("execute".to_string(), Value::from(true)),
    ]);

    harness
        .render(
            "{{ singlestore__rename_relation(from_relation, to_relation) }}",
            ctx,
        )
        .expect("singlestore__rename_relation should render");

    let calls = harness.mock().observed_calls();
    let execute_sqls: Vec<String> = calls
        .iter()
        .filter(|c| c.method == "execute")
        .map(|c| format!("{:?}", c.args))
        .collect();

    // Must contain alter table rename
    assert!(
        execute_sqls
            .iter()
            .any(|sql| sql
                .contains("alter table `my_db`.`src_table` rename to `my_db`.`dest_table`")),
        "expected alter table rename in execute calls: {execute_sqls:?}"
    );
    // Must NOT contain implicit drop
    assert!(
        !execute_sqls
            .iter()
            .any(|sql| sql.to_lowercase().contains("drop")),
        "rename must not execute implicit drop: {execute_sqls:?}"
    );
}

#[test]
fn test_singlestore_temp_and_backup_relations_unique_per_invocation() {
    let harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let rel = harness.relation("my_db", "my_db", "orders", Some(RelationType::Table));
    let rel_val = RelationObject::new(rel).into_value();

    let ctx = BTreeMap::from([
        ("base_relation".to_string(), rel_val),
        (
            "invocation_id".to_string(),
            Value::from("7411707d-4ada-4517-9314-7189a774b241"),
        ),
    ]);

    let rendered_temp = harness
        .render(
            "{{ singlestore__make_temp_relation(base_relation).identifier }}",
            ctx.clone(),
        )
        .expect("singlestore__make_temp_relation should render");
    assert_eq!(rendered_temp.trim(), "orders__dbt_tmp_89a774b241");

    let rendered_intermediate = harness
        .render(
            "{{ singlestore__make_intermediate_relation(base_relation).identifier }}",
            ctx.clone(),
        )
        .expect("singlestore__make_intermediate_relation should render");
    assert_eq!(rendered_intermediate.trim(), "orders__dbt_tmp_89a774b241");

    let rendered_backup = harness
        .render(
            "{{ singlestore__make_backup_relation(base_relation, 'table').identifier }}",
            ctx,
        )
        .expect("singlestore__make_backup_relation should render");
    assert_eq!(rendered_backup.trim(), "orders__dbt_backup_89a774b241");
}

#[test]
fn test_singlestore_drop_stale_temp_and_backup_relations() {
    let mut harness = MacroTestHarness::for_adapter(AdapterType::SingleStore)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build");

    // Mock load_result returning 2 aged leftover tables that matched the query
    let mock_rows = vec![
        vec![Value::from("orders__dbt_tmp_0123456789")],
        vec![Value::from("orders__dbt_backup_abcdef0123")],
    ];

    harness
        .env_mut()
        .env
        .add_function("load_result", move |_name: Value| {
            Ok(Value::from_serialize(BTreeMap::from([(
                "table",
                mock_rows.clone(),
            )])))
        });
    harness
        .env_mut()
        .env
        .add_global("execute", Value::from(true));

    harness
        .mock()
        .on("drop_relation", |_args| Ok(Value::from(())));

    let rel = harness.relation("my_db", "my_db", "orders", Some(RelationType::Table));
    let rel_val = RelationObject::new(rel).into_value();

    let ctx = BTreeMap::from([("base_relation".to_string(), rel_val)]);

    harness
        .render(
            "{{ singlestore__drop_stale_temp_and_backup_relations(base_relation) }}",
            ctx,
        )
        .expect("singlestore__drop_stale_temp_and_backup_relations should render");

    let calls = harness.mock().observed_calls();

    // 1. Check the query statement executed to fetch stale tables
    let all_calls: Vec<(String, String)> = calls
        .iter()
        .map(|c| (c.method.to_string(), format!("{:?}", c.args)))
        .collect();

    assert!(
        all_calls.iter().any(|(method, args)| {
            method == "execute"
                && args.contains("table_schema = 'my_db'")
                && args.contains("table_name regexp '^orders__dbt_(tmp|backup)_[0-9a-f]{10}$'")
                && args.contains("create_time < now() - interval 24 hour")
        }),
        "expected stale tables query with schema, regexp and 24h cutoff in calls: {all_calls:?}"
    );

    // 2. Check that aged leftovers returned were dropped via drop_relation
    assert!(
        all_calls.iter().any(|(method, args)| {
            method == "drop_relation" && args.contains("orders__dbt_tmp_0123456789")
        }),
        "expected drop of aged temp relation: {all_calls:?}"
    );
    assert!(
        all_calls.iter().any(|(method, args)| {
            method == "drop_relation" && args.contains("orders__dbt_backup_abcdef0123")
        }),
        "expected drop of aged backup relation: {all_calls:?}"
    );
}
