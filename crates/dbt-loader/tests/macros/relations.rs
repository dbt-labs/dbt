use std::collections::BTreeMap;

use dbt_adapter::catalog_relation::CatalogRelation;
use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::dbt_catalogs_v2::CatalogType;
use dbt_schemas::schemas::project::{ModelConfig, ProjectModelConfig};
use dbt_schemas::schemas::relations::base::TableFormat;
use indexmap::IndexMap;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, default_mock_config};

mod databricks {
    use super::*;

    fn render_primary_key_constraint(expression: Option<&str>) -> String {
        let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
            .load_all_macros()
            .with_stub_functions()
            .build()
            .expect("constraint harness should build");
        harness.mock().on("quote", |args| {
            let identifier = args
                .first()
                .and_then(Value::as_str)
                .expect("quote expects a string identifier");
            Ok(Value::from(format!("`{identifier}`")))
        });

        let relation = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "constrained_model",
            Some(RelationType::Table),
        );
        let mut constraint = BTreeMap::from([
            ("type".to_string(), Value::from("primary_key")),
            ("columns".to_string(), Value::from(vec![Value::from("id")])),
        ]);
        if let Some(expression) = expression {
            constraint.insert("expression".to_string(), Value::from(expression));
        }

        let columns = BTreeMap::from([(
            "id".to_string(),
            BTreeMap::from([("name".to_string(), "id".to_string())]),
        )]);
        let ctx = BTreeMap::from([
            (
                "relation".to_string(),
                RelationObject::new(relation).into_value(),
            ),
            ("constraint".to_string(), Value::from_serialize(constraint)),
            (
                "model".to_string(),
                Value::from_serialize(BTreeMap::from([("columns", columns)])),
            ),
        ]);

        harness
            .render(
                "{{ get_constraint_sql(relation, constraint, model) | join('\\n') }}",
                ctx,
            )
            .expect("primary-key constraint should render")
    }

    fn generated_constraint_name(sql: &str) -> &str {
        sql.split_once("add constraint ")
            .and_then(|(_, suffix)| suffix.split_once(" primary key"))
            .map(|(name, _)| name)
            .expect("rendered SQL should contain a primary-key constraint name")
    }

    #[test]
    fn primary_key_expression_is_rendered_and_changes_generated_name() {
        let without_expression = render_primary_key_constraint(None);
        let with_expression = render_primary_key_constraint(Some("timeseries"));

        assert!(
            with_expression.contains("primary key(`id`) timeseries;"),
            "primary-key expression should be appended to the DDL: {with_expression}"
        );
        assert_eq!(
            generated_constraint_name(&without_expression),
            "f82f3cec0489fd1683773f6573fdf556"
        );
        assert_eq!(
            generated_constraint_name(&with_expression),
            "c5a2df3443aff7d5b8c474aea39c6265",
            "the generated name must match the current-v1 hash input"
        );
    }

    fn constraint_config(persist_constraints: bool) -> Value {
        let config = default_mock_config();
        config.on("get", move |args| {
            let key = args.first().and_then(Value::as_str);
            let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
            match key {
                Some("persist_constraints") => Ok(Value::from(persist_constraints)),
                Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                    "enforced".to_string(),
                    Value::from(false),
                )]))),
                _ => Ok(default),
            }
        });
        Value::from_dyn_object(config)
    }

    fn render_legacy_constraint_macro(
        expression: &str,
        context: BTreeMap<String, Value>,
    ) -> dbt_common::FsResult<String> {
        MacroTestHarness::for_adapter(AdapterType::Databricks)
            .load_all_macros()
            .with_stub_functions()
            .build()
            .expect("constraint harness should build")
            .render(expression, context)
    }

    #[test]
    fn legacy_constraint_metadata_respects_persist_constraints_and_validates_shape() {
        let model = Value::from_serialize(BTreeMap::from([(
            "meta".to_string(),
            BTreeMap::from([(
                "constraints".to_string(),
                vec![BTreeMap::from([
                    ("name".to_string(), "positive_id".to_string()),
                    ("condition".to_string(), "id > 0".to_string()),
                ])],
            )]),
        )]));

        let enabled = render_legacy_constraint_macro(
            "{{ get_model_constraints(model) | tojson }}",
            BTreeMap::from([
                ("config".to_string(), constraint_config(true)),
                ("model".to_string(), model.clone()),
            ]),
        )
        .expect("valid legacy model constraint should render");
        let disabled = render_legacy_constraint_macro(
            "{{ get_model_constraints(model) | tojson }}",
            BTreeMap::from([
                ("config".to_string(), constraint_config(false)),
                ("model".to_string(), model),
            ]),
        )
        .expect("disabled legacy constraints should render an empty list");

        assert!(
            enabled.contains("positive_id") && enabled.contains("expression"),
            "enabled legacy constraints should be translated: {enabled:?}"
        );
        assert_eq!(disabled.trim(), "[]");

        let invalid_model = Value::from_serialize(BTreeMap::from([(
            "meta".to_string(),
            BTreeMap::from([(
                "constraints".to_string(),
                vec![BTreeMap::from([(
                    "condition".to_string(),
                    "id > 0".to_string(),
                )])],
            )]),
        )]));
        let error = render_legacy_constraint_macro(
            "{{ get_model_constraints(model) | tojson }}",
            BTreeMap::from([
                ("config".to_string(), constraint_config(true)),
                ("model".to_string(), invalid_model),
            ]),
        )
        .expect_err("legacy check constraints without a name must fail");
        assert!(error.to_string().contains("Invalid check constraint name"));

        let invalid_column = Value::from_serialize(BTreeMap::from([
            ("name".to_string(), Value::from("id")),
            (
                "meta".to_string(),
                Value::from_serialize(BTreeMap::from([(
                    "constraint".to_string(),
                    "invalid".to_string(),
                )])),
            ),
        ]));
        let error = render_legacy_constraint_macro(
            "{{ get_column_constraints(column) | tojson }}",
            BTreeMap::from([
                ("config".to_string(), constraint_config(true)),
                ("column".to_string(), invalid_column.clone()),
            ]),
        )
        .expect_err("unsupported legacy column constraints must fail");
        assert!(
            error
                .to_string()
                .contains("Invalid constraint for column id. Only `not_null` is supported.")
        );

        let disabled = render_legacy_constraint_macro(
            "{{ get_column_constraints(column) | tojson }}",
            BTreeMap::from([
                ("config".to_string(), constraint_config(false)),
                ("column".to_string(), invalid_column),
            ]),
        )
        .expect("disabled legacy column metadata must not be validated");
        assert_eq!(disabled.trim(), "[]");
    }
    const STATEMENT_STUB: &str = r#"
{% macro statement(name=none, fetch_result=False, auto_begin=True, language='sql') -%}
  [statement:{{ name }}]{{ caller() }}[/statement]
{%- endmacro %}
"#;

    fn build_comment_clause_harness() -> MacroTestHarness {
        let databricks_comment_sql =
            include_str!("../../src/dbt_macro_assets/dbt-databricks/macros/relations/comment.sql");

        let dispatching_comment_clause = r#"
{% macro comment_clause() -%}
  {{ adapter.dispatch('comment_clause', 'dbt')() }}
{%- endmacro %}
"#;

        MacroTestHarness::for_adapter(AdapterType::Databricks)
            .with_macro("dbt", "comment_clause", dispatching_comment_clause)
            .with_macro_at_path(
                "dbt_databricks",
                "databricks__comment_clause",
                databricks_comment_sql,
                "dbt_macro_assets/dbt-databricks/macros/relations/comment.sql",
            )
            .build()
            .expect("harness should build")
    }

    #[test]
    fn tblproperties_clause_preserves_project_declaration_order() {
        let databricks_tblproperties_sql = include_str!(
            "../../src/dbt_macro_assets/dbt-databricks/macros/relations/tblproperties.sql"
        );
        let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
            .with_macro_at_path(
                "dbt_databricks",
                "databricks__tblproperties_clause",
                databricks_tblproperties_sql,
                "dbt-databricks/macros/relations/tblproperties.sql",
            )
            .build()
            .expect("tblproperties harness should build");
        harness.mock().on("is_uniform", |_| Ok(Value::from(false)));

        let project_config: ProjectModelConfig = dbt_yaml::from_str(
            r#"
+tblproperties:
  zeta: last
  alpha: first
  middle: center
__additional_properties__: {}
"#,
        )
        .expect("project config should parse");
        let model_config: ModelConfig = project_config.into();
        let tblproperties = model_config
            .__warehouse_specific_config__
            .tblproperties
            .expect("tblproperties should be present");
        let config = IndexMap::from([(
            "tblproperties".to_string(),
            Value::from_serialize(tblproperties),
        )]);

        let rendered = harness
            .render(
                "{{ databricks__tblproperties_clause() }}",
                BTreeMap::from([("config".to_string(), Value::from_serialize(config))]),
            )
            .expect("tblproperties clause should render");

        let zeta = rendered.find("'zeta' = 'last'").expect("zeta property");
        let alpha = rendered.find("'alpha' = 'first'").expect("alpha property");
        let middle = rendered
            .find("'middle' = 'center'")
            .expect("middle property");
        assert!(
            zeta < alpha && alpha < middle,
            "rendered clause: {rendered}"
        );
    }

    // `databricks__create_table_as` calls these clause helpers (defined in other asset files)
    // unconditionally. Each must resolve, but the only thing under test here is the
    // create/replace branch, so we register them as no-ops.
    const CLAUSE_STUBS: [&str; 8] = [
        "file_format_clause",
        "partition_cols",
        "get_create_row_filter_clause",
        "liquid_clustered_cols",
        "clustered_cols",
        "location_clause",
        "comment_clause",
        "tblproperties_clause",
    ];

    fn build_create_table_harness() -> MacroTestHarness {
        let databricks_create_table_sql = include_str!(
            "../../src/dbt_macro_assets/dbt-databricks/macros/relations/table/create.sql"
        );

        let mut builder = MacroTestHarness::for_adapter(AdapterType::Databricks)
            .with_macro_at_path(
                "dbt_databricks",
                "databricks__create_table_as",
                databricks_create_table_sql,
                "dbt-databricks/macros/relations/table/create.sql",
            );
        for name in CLAUSE_STUBS {
            builder = builder.with_macro(
                "dbt_databricks",
                name,
                &format!("{{% macro {name}() %}}{{% endmacro %}}"),
            );
        }
        builder.build().expect("create table harness should build")
    }

    fn ctx_for(description: &str) -> BTreeMap<String, Value> {
        BTreeMap::from([
            (
                "config".to_string(),
                Value::from_serialize(BTreeMap::from([(
                    "persist_docs".to_string(),
                    BTreeMap::from([("relation".to_string(), true)]),
                )])),
            ),
            (
                "model".to_string(),
                Value::from_serialize(BTreeMap::from([(
                    "description".to_string(),
                    description.to_string(),
                )])),
            ),
        ])
    }

    fn build_location_clause_harness() -> MacroTestHarness {
        let databricks_location_sql =
            include_str!("../../src/dbt_macro_assets/dbt-databricks/macros/relations/location.sql");

        MacroTestHarness::for_adapter(AdapterType::Databricks)
            .with_macro(
                "dbt",
                "is_incremental",
                "{% macro is_incremental() %}{{ return(false) }}{% endmacro %}",
            )
            .with_macro_at_path(
                "dbt_databricks",
                "location_clause",
                databricks_location_sql,
                "dbt_macro_assets/dbt-databricks/macros/relations/location.sql",
            )
            .build()
            .expect("location clause harness should build")
    }

    fn build_optimize_harness() -> MacroTestHarness {
        let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
            .load_all_macros()
            .with_macro("dbt", "statement", STATEMENT_STUB)
            .build()
            .expect("optimize harness should build");

        harness
            .mock()
            .on("resolve_file_format", |_| Ok(Value::from("delta")));

        harness
    }

    fn render_optimize(
        config_values: BTreeMap<String, Value>,
        vars: BTreeMap<String, bool>,
    ) -> String {
        let mut harness = build_optimize_harness();
        let config = default_mock_config();
        config.on("get", move |args| {
            let key = args.first().and_then(Value::as_str);
            let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
            Ok(key
                .and_then(|key| config_values.get(key).cloned())
                .unwrap_or(default))
        });

        harness
            .env_mut()
            .env
            .add_function("var", move |name: Value, default: Option<Value>| {
                let value = name
                    .as_str()
                    .and_then(|name| vars.get(name))
                    .copied()
                    .map(Value::from);
                Ok(value.unwrap_or_else(|| default.unwrap_or(Value::UNDEFINED)))
            });

        let relation = RelationObject::new(harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "target",
            Some(RelationType::Table),
        ))
        .into_value();
        harness
            .render(
                "{{ optimize(relation) }}",
                BTreeMap::from([
                    ("config".to_string(), Value::from_dyn_object(config)),
                    ("relation".to_string(), relation),
                ]),
            )
            .expect("optimize should render")
    }

    #[test]
    fn optimize_macro_skips_configured_clustering() {
        for (cluster_key, cluster_value) in [
            ("zorder", Value::from("id")),
            (
                "liquid_clustered_by",
                Value::from_serialize(vec!["id".to_string()]),
            ),
            ("auto_liquid_cluster", Value::from(true)),
        ] {
            let rendered = render_optimize(
                BTreeMap::from([
                    (cluster_key.to_string(), cluster_value),
                    ("skip_optimize".to_string(), Value::from(true)),
                ]),
                BTreeMap::new(),
            );
            assert!(
                !rendered.to_lowercase().contains("optimize"),
                "skip_optimize must suppress post-materialization OPTIMIZE for {cluster_key}, got: {rendered:?}"
            );
        }
    }

    #[test]
    fn optimize_macro_preserves_existing_guards() {
        let cluster = BTreeMap::from([(String::from("zorder"), Value::from("id"))]);

        let rendered = render_optimize(cluster.clone(), BTreeMap::new());
        assert!(
            rendered.to_lowercase().contains("optimize"),
            "absent skip_optimize must preserve optimize, got: {rendered:?}"
        );

        let rendered = render_optimize(
            BTreeMap::from([
                (String::from("zorder"), Value::from("id")),
                (String::from("skip_optimize"), Value::from(false)),
            ]),
            BTreeMap::new(),
        );
        assert!(
            rendered.to_lowercase().contains("optimize"),
            "false skip_optimize must preserve optimize, got: {rendered:?}"
        );

        for variable in ["DATABRICKS_SKIP_OPTIMIZE", "databricks_skip_optimize"] {
            let rendered = render_optimize(
                cluster.clone(),
                BTreeMap::from([(variable.to_string(), true)]),
            );
            assert!(
                !rendered.to_lowercase().contains("optimize"),
                "{variable} must still suppress optimize, got: {rendered:?}"
            );
        }
    }

    fn render_location_clause(external_volume: Option<&str>) -> String {
        let harness = build_location_clause_harness();
        let relation = Value::from_object(CatalogRelation {
            external_volume: external_volume.map(str::to_string),
            ..CatalogRelation::default_catalog_relation_databricks()
        });

        harness
            .render(
                "{{ location_clause(relation) }}",
                BTreeMap::from([("relation".to_string(), relation)]),
            )
            .expect("render should succeed")
    }

    #[test]
    fn location_clause_renders_location_from_relation() {
        let rendered = render_location_clause(Some("s3://bucket/root/a"));
        assert!(
            rendered.contains("location 's3://bucket/root/a'"),
            "expected a location clause, got: {rendered:?}"
        );
    }

    #[test]
    fn location_clause_renders_nothing_without_location() {
        let rendered = render_location_clause(None);
        assert!(
            !rendered.to_lowercase().contains("location"),
            "expected no location clause, got: {rendered:?}"
        );
    }

    #[test]
    fn comment_clause_does_not_render_empty_comment() {
        let harness = build_comment_clause_harness();
        let rendered = harness
            .render("{{ comment_clause() }}", ctx_for(""))
            .expect("render should succeed");

        assert_eq!(rendered.trim(), "");
        assert!(
            !rendered.contains("comment ''"),
            "Should never render an empty comment clause, got: {rendered:?}"
        );
    }

    #[test]
    fn comment_clause_renders_non_empty_comment() {
        let harness = build_comment_clause_harness();
        let rendered = harness
            .render("{{ comment_clause() }}", ctx_for("hello"))
            .expect("render should succeed");

        assert!(
            rendered.contains("comment 'hello'"),
            "Expected non-empty comment clause, got: {rendered:?}"
        );
    }

    /// Render `databricks__create_table_as` for a Databricks relation with the given catalog
    /// shape (the #10647 area), toggling the `use_catalogs_v2` behavior flag.
    fn render_databricks_create_table(
        use_catalogs_v2: bool,
        catalog_type: &str,
        table_format: &str,
        file_format: Option<&str>,
    ) -> String {
        let harness = build_create_table_harness();

        if use_catalogs_v2 {
            enable_catalogs_v2();
        }

        harness.mock().set_attr(
            "behavior",
            Value::from_serialize(BTreeMap::from([(
                "use_catalogs_v2",
                BTreeMap::from([("no_warn", use_catalogs_v2)]),
            )])),
        );

        let relation = Value::from_object(CatalogRelation {
            catalog_type: if catalog_type.eq_ignore_ascii_case("hive_metastore") {
                CatalogType::HiveMetastore
            } else {
                CatalogType::Unity
            },
            table_format: if table_format.eq_ignore_ascii_case("iceberg") {
                TableFormat::Iceberg
            } else {
                TableFormat::Default
            },
            file_format: file_format.map(str::to_string),
            ..CatalogRelation::default_catalog_relation_databricks()
        });
        harness
            .mock()
            .on("build_catalog_relation", move |_| Ok(relation.clone()));

        let ctx = harness
            .materialization_context("customers", "select 1")
            .relation_type(RelationType::Table)
            .with("dbt_version", Value::from("2.0.0"))
            .build();

        harness
            .render(
                "{{ databricks__create_table_as(false, this, 'select 1') }}",
                ctx,
            )
            .expect("render should succeed")
    }

    fn enable_catalogs_v2() {
        let catalogs = dbt_yaml::from_str("catalogs: []\n").expect("valid catalogs.yml v2");
        let project_flags =
            dbt_yaml::from_str("use_catalogs_v2: true\n").expect("valid project flags");
        dbt_adapter::load_catalogs::do_load_catalogs(
            catalogs,
            std::path::Path::new("catalogs.yml"),
            Some(&project_flags),
        )
        .expect("catalogs.yml v2 should load");
    }

    #[test]
    fn managed_iceberg_uses_create_or_replace_under_catalogs_v2() {
        let rendered = render_databricks_create_table(true, "unity", "iceberg", Some("parquet"));
        assert!(
            rendered.to_lowercase().contains("create or replace table"),
            "managed iceberg under catalogs v2 must use `create or replace table`, got:\n{rendered}"
        );
    }

    #[test]
    fn non_replaceable_relation_keeps_plain_create_under_catalogs_v2() {
        let rendered = render_databricks_create_table(true, "unity", "default", Some("parquet"));
        let lower = rendered.to_lowercase();
        assert!(
            !lower.contains("create or replace table") && lower.contains("create table"),
            "non-replaceable relation under catalogs v2 must keep `create table`, got:\n{rendered}"
        );
    }

    #[test]
    fn managed_iceberg_keeps_v1_behavior_without_catalogs_v2() {
        let rendered = render_databricks_create_table(false, "unity", "iceberg", Some("parquet"));
        let lower = rendered.to_lowercase();
        assert!(
            !lower.contains("create or replace table") && lower.contains("create table"),
            "managed iceberg without catalogs v2 must keep `create table`, got:\n{rendered}"
        );
    }

    fn build_file_format_harness() -> MacroTestHarness {
        let file_format_sql = include_str!(
            "../../src/dbt_macro_assets/dbt-databricks/macros/relations/file_format.sql"
        );

        MacroTestHarness::for_adapter(AdapterType::Databricks)
            .with_macro_at_path(
                "dbt_databricks",
                "file_format_clause",
                file_format_sql,
                "dbt-databricks/macros/relations/file_format.sql",
            )
            .build()
            .expect("file format harness should build")
    }

    fn render_file_format_clause(
        use_catalogs_v2: bool,
        use_managed_iceberg: bool,
        relation: CatalogRelation,
    ) -> String {
        let harness = build_file_format_harness();

        if use_catalogs_v2 {
            enable_catalogs_v2();
        }

        harness.mock().set_attr(
            "behavior",
            Value::from_serialize(serde_json::json!({
                "use_catalogs_v2": { "no_warn": use_catalogs_v2 },
                "use_managed_iceberg": use_managed_iceberg,
            })),
        );

        let ctx = BTreeMap::from([
            ("dbt_version".to_string(), Value::from("2.0.0")),
            ("relation".to_string(), Value::from_object(relation)),
        ]);

        harness
            .render("{{ file_format_clause(relation) }}", ctx)
            .expect("render should succeed")
    }

    #[test]
    fn iceberg_without_catalogs_yml_renders_uniform_delta_v1_by_default() {
        let relation = CatalogRelation::default_catalog_relation_databricks()
            .with_table_format(TableFormat::Iceberg)
            .with_adapter_property("use_uniform", "false");
        let rendered = render_file_format_clause(false, false, relation);
        let lower = rendered.to_lowercase();
        assert!(
            lower.contains("using delta") && !lower.contains("using iceberg"),
            "table_format=iceberg without catalogs.yml must default to UniForm (`using delta`) in v1, unchanged from today, got:\n{rendered}"
        );
    }

    #[test]
    fn iceberg_renders_managed_iceberg_when_use_managed_iceberg_enabled_v1() {
        let relation = CatalogRelation::default_catalog_relation_databricks()
            .with_table_format(TableFormat::Iceberg)
            .with_adapter_property("use_uniform", "false");
        let rendered = render_file_format_clause(false, true, relation);
        let lower = rendered.to_lowercase();
        assert!(
            lower.contains("using iceberg") && !lower.contains("using delta"),
            "table_format=iceberg with use_managed_iceberg: true must opt into managed Iceberg (`using iceberg`), got:\n{rendered}"
        );
    }

    #[test]
    fn iceberg_without_catalog_name_renders_uniform_delta_explicit_opt_in_v2() {
        let relation = CatalogRelation::default_catalog_relation_databricks()
            .with_table_format(TableFormat::Iceberg)
            .with_adapter_property("use_uniform", "true");
        let rendered = render_file_format_clause(true, true, relation);
        let lower = rendered.to_lowercase();
        assert!(
            lower.contains("using delta") && !lower.contains("using iceberg"),
            "table_format=iceberg with explicit use_uniform:true (v2) must render UniForm (`using delta`), got:\n{rendered}"
        );
    }

    #[test]
    fn iceberg_without_catalogs_yml_renders_managed_iceberg_even_with_use_catalogs_v2_flag() {
        let relation = CatalogRelation::default_catalog_relation_databricks()
            .with_table_format(TableFormat::Iceberg)
            .with_adapter_property("use_uniform", "false");
        let rendered = render_file_format_clause(true, true, relation);
        let lower = rendered.to_lowercase();
        assert!(
            lower.contains("using iceberg") && !lower.contains("using delta"),
            "table_format=iceberg with use_catalogs_v2=true but no catalogs.yml must still default to managed Iceberg (`using iceberg`), got:\n{rendered}"
        );
    }

    fn render_constraint_changeset(contract_enforced: bool) -> String {
        let alter_sql = include_str!(
            "../../src/dbt_macro_assets/dbt-databricks/macros/relations/table/alter.sql"
        );
        let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
            .with_macro_at_path(
                "dbt_databricks",
                "apply_config_changeset",
                alter_sql,
                "dbt-databricks/macros/relations/table/alter.sql",
            )
            .with_macro(
                "test_project",
                "apply_constraints",
                "{% macro apply_constraints(relation, constraints) %}APPLY_CONSTRAINTS_SENTINEL{% endmacro %}",
            )
            .with_stub_functions()
            .build()
            .expect("constraint changeset harness should build");
        let config = default_mock_config();
        config.on("get", move |args| {
            let key = args.first().and_then(Value::as_str);
            let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
            match key {
                Some("contract") => Ok(Value::from_serialize(BTreeMap::from([(
                    "enforced",
                    contract_enforced,
                )]))),
                _ => Ok(default),
            }
        });
        let changes = Value::from_serialize(BTreeMap::from([(
            "changes",
            BTreeMap::from([("constraints", Value::from(true))]),
        )]));
        let ctx = harness
            .materialization_context("constraint_table", "select 1")
            .relation_type(RelationType::Table)
            .config(Value::from_dyn_object(config))
            .with("configuration_changes", changes)
            .build();

        harness
            .render(
                "{{ apply_config_changeset(this, model, configuration_changes) }}",
                ctx,
            )
            .expect("constraint changeset should render")
    }

    #[test]
    fn table_alter_skips_constraint_reconciliation_when_contract_is_unenforced() {
        let rendered = render_constraint_changeset(false);
        assert!(
            !rendered.contains("APPLY_CONSTRAINTS_SENTINEL"),
            "unenforced contract must not apply constraints, got: {rendered}"
        );
    }

    #[test]
    fn table_alter_reconciles_constraints_when_contract_is_enforced() {
        let rendered = render_constraint_changeset(true);
        assert!(
            rendered.contains("APPLY_CONSTRAINTS_SENTINEL"),
            "enforced contract must apply constraints, got: {rendered}"
        );
    }
}

/// Isolated tests for `relations/interactive_table/*.sql`.
///
/// These drive the inner macros, which take the resolved configuration as a parameter, rather than
/// the public `snowflake__get_{create,replace}_interactive_table_*` entry points that resolve it
/// through `relation.from_config(config.model)`. Standing the configuration in with a mock keeps
/// each DDL assertion independent of config resolution, and the mock panics on any attribute it was
/// not given, so it also pins which configuration keys the DDL is allowed to read.
mod snowflake_interactive_table {
    use std::sync::Arc;

    use dbt_adapter::relation::RelationObject;
    use dbt_jinja_utils::mock_object::MockJinjaObject;

    use super::*;

    const CREATE_PATH: &str = "dbt-snowflake/macros/relations/interactive_table/create.sql";
    const REPLACE_PATH: &str = "dbt-snowflake/macros/relations/interactive_table/replace.sql";
    const ALTER_PATH: &str = "dbt-snowflake/macros/relations/interactive_table/alter.sql";
    const DROP_PATH: &str = "dbt-snowflake/macros/relations/interactive_table/drop.sql";
    const RENAME_PATH: &str = "dbt-snowflake/macros/relations/interactive_table/rename.sql";
    const OPTIONAL_PATH: &str = "dbt-snowflake/macros/utils/optional.sql";
    const SHARED_ALTER_PATH: &str = "dbt-snowflake/macros/relations/target_lag_warehouse_alter.sql";

    const CREATE_SQL: &str = include_str!(
        "../../src/dbt_macro_assets/dbt-snowflake/macros/relations/interactive_table/create.sql"
    );
    const REPLACE_SQL: &str = include_str!(
        "../../src/dbt_macro_assets/dbt-snowflake/macros/relations/interactive_table/replace.sql"
    );
    const ALTER_SQL: &str = include_str!(
        "../../src/dbt_macro_assets/dbt-snowflake/macros/relations/interactive_table/alter.sql"
    );
    const DROP_SQL: &str = include_str!(
        "../../src/dbt_macro_assets/dbt-snowflake/macros/relations/interactive_table/drop.sql"
    );
    const RENAME_SQL: &str = include_str!(
        "../../src/dbt_macro_assets/dbt-snowflake/macros/relations/interactive_table/rename.sql"
    );
    const OPTIONAL_SQL: &str =
        include_str!("../../src/dbt_macro_assets/dbt-snowflake/macros/utils/optional.sql");
    const SHARED_ALTER_SQL: &str = include_str!(
        "../../src/dbt_macro_assets/dbt-snowflake/macros/relations/target_lag_warehouse_alter.sql"
    );

    /// `get_replace_sql` is the shared dispatcher, defined outside this macro set, so the alter
    /// tests substitute a marker they can assert on.
    const REPLACE_MARKER: &str = "dispatched-to-get-replace-sql";

    const RENDERED_RELATION: &str = "TEST_DB.TEST_SCHEMA.MY_IT";

    fn build_harness() -> MacroTestHarness {
        let replace_stub = format!(
            "{{% macro get_replace_sql(existing_relation, target_relation, sql) %}}{REPLACE_MARKER}{{% endmacro %}}"
        );

        MacroTestHarness::for_adapter(AdapterType::Snowflake)
            .with_stub_functions()
            .with_macro_at_path("dbt_snowflake", "optional", OPTIONAL_SQL, OPTIONAL_PATH)
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__create_interactive_table_sql",
                CREATE_SQL,
                CREATE_PATH,
            )
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__interactive_table_options_sql",
                CREATE_SQL,
                CREATE_PATH,
            )
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__replace_interactive_table_sql",
                REPLACE_SQL,
                REPLACE_PATH,
            )
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__get_alter_interactive_table_as_sql",
                ALTER_SQL,
                ALTER_PATH,
            )
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__target_lag_warehouse_alter_active",
                SHARED_ALTER_SQL,
                SHARED_ALTER_PATH,
            )
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__get_target_lag_warehouse_alter_sql",
                SHARED_ALTER_SQL,
                SHARED_ALTER_PATH,
            )
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__get_drop_interactive_table_sql",
                DROP_SQL,
                DROP_PATH,
            )
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__get_rename_interactive_table_sql",
                RENAME_SQL,
                RENAME_PATH,
            )
            .with_macro("dbt_snowflake", "get_replace_sql", &replace_stub)
            .build()
            .expect("interactive table harness should build")
    }

    fn relation_value(harness: &MacroTestHarness) -> Value {
        let relation = harness.relation(
            "TEST_DB",
            "TEST_SCHEMA",
            "MY_IT",
            Some(RelationType::InteractiveTable),
        );
        RelationObject::new(relation).into_value()
    }

    fn optional_str(value: Option<&str>) -> Value {
        value.map(Value::from).unwrap_or_else(|| Value::from(()))
    }

    /// A resolved interactive table configuration, as `relation.from_config` produces.
    fn interactive_table_config(
        target_lag: Option<&str>,
        snowflake_warehouse: Option<&str>,
        snowflake_initialization_warehouse: Option<&str>,
        cluster_by: Option<&str>,
    ) -> Value {
        let config = Arc::new(MockJinjaObject::new());
        config.set_attr("target_lag", optional_str(target_lag));
        config.set_attr("snowflake_warehouse", optional_str(snowflake_warehouse));
        config.set_attr(
            "snowflake_initialization_warehouse",
            optional_str(snowflake_initialization_warehouse),
        );
        config.set_attr("cluster_by", optional_str(cluster_by));
        Value::from_dyn_object(config)
    }

    /// A changeset entry for a component that changed. `Some(value)` is a new value,
    /// `None` is a clear.
    fn changed_to(value: Option<&str>) -> Value {
        Value::from_serialize(BTreeMap::from([(
            "context".to_string(),
            optional_str(value),
        )]))
    }

    /// A configuration changeset. Each component is either absent (`None`) or an entry built
    /// with [`changed_to`].
    fn changeset(
        requires_full_refresh: bool,
        target_lag: Option<Value>,
        snowflake_warehouse: Option<Value>,
        snowflake_initialization_warehouse: Option<Value>,
    ) -> Value {
        let changes = Arc::new(MockJinjaObject::new());
        changes.set_attr("requires_full_refresh", Value::from(requires_full_refresh));
        changes.set_attr("target_lag", target_lag.unwrap_or_else(|| Value::from(())));
        changes.set_attr(
            "snowflake_warehouse",
            snowflake_warehouse.unwrap_or_else(|| Value::from(())),
        );
        changes.set_attr(
            "snowflake_initialization_warehouse",
            snowflake_initialization_warehouse.unwrap_or_else(|| Value::from(())),
        );
        Value::from_dyn_object(changes)
    }

    fn render_create(harness: &MacroTestHarness, config: Value) -> String {
        let ctx = BTreeMap::from([
            ("relation".to_string(), relation_value(harness)),
            ("interactive_table".to_string(), config),
            ("sql".to_string(), Value::from("select 1 as id")),
        ]);
        harness
            .render(
                "{{ snowflake__create_interactive_table_sql(interactive_table, relation, sql) }}",
                ctx,
            )
            .expect("create should render")
    }

    fn render_replace(harness: &MacroTestHarness, config: Value) -> String {
        let ctx = BTreeMap::from([
            ("relation".to_string(), relation_value(harness)),
            ("interactive_table".to_string(), config),
            ("sql".to_string(), Value::from("select 1 as id")),
        ]);
        harness
            .render(
                "{{ snowflake__replace_interactive_table_sql(interactive_table, relation, sql) }}",
                ctx,
            )
            .expect("replace should render")
    }

    fn render_alter(harness: &MacroTestHarness, configuration_changes: Value) -> String {
        let ctx = BTreeMap::from([
            ("relation".to_string(), relation_value(harness)),
            ("configuration_changes".to_string(), configuration_changes),
            ("sql".to_string(), Value::from("select 1 as id")),
        ]);
        harness
            .render(
                "{{ snowflake__get_alter_interactive_table_as_sql(relation, configuration_changes, relation, sql) }}",
                ctx,
            )
            .expect("alter should render")
    }

    /// The rendered statements of a multi-statement script, split on the separator.
    fn statements(rendered: &str) -> Vec<String> {
        rendered
            .split(';')
            .map(normalized)
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Collapse rendered whitespace to single spaces, so that assertions read as the DDL does.
    ///
    /// Token boundaries survive this, so a clause that was accidentally glued to the one before
    /// it still fails an equality assertion.
    fn normalized(rendered: &str) -> String {
        rendered.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn static_create_omits_target_lag_and_both_warehouses() {
        let harness = build_harness();
        // A warehouse and an initialization warehouse are configured, but no target lag: the
        // gate is the target lag alone, because Snowflake rejects a warehouse without one.
        let rendered = render_create(
            &harness,
            interactive_table_config(None, Some("MY_WH"), Some("INIT_WH"), None),
        );
        assert_eq!(
            normalized(&rendered),
            format!("create interactive table {RENDERED_RELATION} as ( select 1 as id )"),
            "got:\n{rendered}"
        );
        let lower = rendered.to_lowercase();
        assert!(
            !lower.contains("target_lag"),
            "a static interactive table must not set target_lag, got:\n{rendered}"
        );
        // Also covers `initialization_warehouse`, of which this is a substring.
        assert!(
            !lower.contains("warehouse"),
            "a static interactive table must not name a warehouse, got:\n{rendered}"
        );
    }

    #[test]
    fn refreshing_create_emits_every_applicable_clause() {
        let harness = build_harness();
        let rendered = render_create(
            &harness,
            interactive_table_config(
                Some("1 minute"),
                Some("MY_WH"),
                Some("INIT_WH"),
                Some("id, val"),
            ),
        );
        assert_eq!(
            normalized(&rendered),
            format!(
                "create interactive table {RENDERED_RELATION} \
                 cluster by (id, val) \
                 target_lag = '1 minute' \
                 warehouse = MY_WH \
                 initialization_warehouse = INIT_WH \
                 as ( select 1 as id )"
            ),
            "got:\n{rendered}"
        );
    }

    #[test]
    fn initialization_warehouse_is_omitted_when_unset_on_a_refreshing_table() {
        let harness = build_harness();
        let rendered = render_create(
            &harness,
            interactive_table_config(Some("1 minute"), Some("MY_WH"), None, None),
        );
        let lower = rendered.to_lowercase();

        assert!(
            lower.contains("target_lag = '1 minute'") && lower.contains("warehouse = my_wh"),
            "expected the refreshing clauses, got:\n{rendered}"
        );
        assert!(
            !lower.contains("initialization_warehouse"),
            "an unset initialization warehouse must be omitted, got:\n{rendered}"
        );
    }

    #[test]
    fn replace_uses_create_or_replace_interactive_table() {
        let harness = build_harness();
        let rendered = render_replace(
            &harness,
            interactive_table_config(Some("1 minute"), Some("MY_WH"), None, Some("id")),
        );
        assert_eq!(
            normalized(&rendered),
            format!(
                "create or replace interactive table {RENDERED_RELATION} \
                 cluster by (id) \
                 target_lag = '1 minute' \
                 warehouse = MY_WH \
                 as ( select 1 as id )"
            ),
            "got:\n{rendered}"
        );
    }

    #[test]
    fn transient_never_appears_in_create_or_replace() {
        let harness = build_harness();
        let config = || {
            interactive_table_config(
                Some("1 minute"),
                Some("MY_WH"),
                Some("INIT_WH"),
                Some("id, val"),
            )
        };

        for rendered in [
            render_create(&harness, config()),
            render_replace(&harness, config()),
        ] {
            assert!(
                !rendered.to_lowercase().contains("transient"),
                "there is no valid transient interactive table DDL, got:\n{rendered}"
            );
        }
    }

    #[test]
    fn requires_full_refresh_routes_to_replace_rather_than_alter() {
        let harness = build_harness();
        // A component change that is perfectly alterable on its own: only the flag decides.
        let rendered = render_alter(
            &harness,
            changeset(true, Some(changed_to(Some("2 minutes"))), None, None),
        );
        let lower = rendered.to_lowercase();

        assert!(
            lower.contains(REPLACE_MARKER),
            "a full refresh must be dispatched to the replace path, got:\n{rendered}"
        );
        assert!(
            !lower.contains("alter interactive table"),
            "a full refresh must not emit an alter, got:\n{rendered}"
        );
    }

    #[test]
    fn set_changes_render_as_one_alter_set_statement() {
        let harness = build_harness();
        let rendered = render_alter(
            &harness,
            changeset(
                false,
                Some(changed_to(Some("2 minutes"))),
                Some(changed_to(Some("OTHER_WH"))),
                Some(changed_to(Some("INIT_WH"))),
            ),
        );
        // Full equality rather than substrings: each assignment stays a substring of the
        // rendered output even when the whitespace separating it from its neighbour is lost,
        // so only comparing the whole statement catches two clauses glued together.
        assert_eq!(
            statements(&rendered),
            vec![format!(
                "alter interactive table {RENDERED_RELATION} set target_lag = '2 minutes' \
                 warehouse = OTHER_WH initialization_warehouse = INIT_WH"
            )],
            "set changes belong in one well-separated statement, got:\n{rendered}"
        );
        assert!(
            !rendered.to_lowercase().contains("unset"),
            "a set change must not also unset anything, got:\n{rendered}"
        );
    }

    #[test]
    fn cleared_initialization_warehouse_renders_exactly_one_unset_statement() {
        let harness = build_harness();
        let rendered = render_alter(
            &harness,
            changeset(false, None, None, Some(changed_to(None))),
        );
        let lower = rendered.to_lowercase();

        assert_eq!(
            statements(&rendered),
            vec![format!(
                "alter interactive table {RENDERED_RELATION} unset initialization_warehouse"
            )],
            "a clear is exactly one standalone unset statement, got:\n{rendered}"
        );
        // A clear must not open an `alter ... set` with nothing assigned in it. `unset` is
        // removed first so that its own trailing `set` cannot mask a dangling one.
        assert!(
            !lower.replace("unset", "").contains("set"),
            "a clear must not emit a dangling `set`, got:\n{rendered}"
        );
    }

    #[test]
    fn cleared_initialization_warehouse_is_separated_from_a_set_change() {
        let harness = build_harness();
        let rendered = render_alter(
            &harness,
            changeset(
                false,
                Some(changed_to(Some("2 minutes"))),
                None,
                Some(changed_to(None)),
            ),
        );

        let statements = statements(&rendered);
        assert_eq!(
            statements.len(),
            2,
            "a clear alongside another change is two separated statements, got:\n{rendered}"
        );
        assert_eq!(
            statements[0].to_lowercase(),
            format!("alter interactive table {RENDERED_RELATION} set target_lag = '2 minutes'")
                .to_lowercase(),
            "the set statement is malformed, got:\n{rendered}"
        );
        assert_eq!(
            statements[1].to_lowercase(),
            format!("alter interactive table {RENDERED_RELATION} unset initialization_warehouse")
                .to_lowercase(),
            "the unset statement is malformed, got:\n{rendered}"
        );
    }

    #[test]
    fn warehouse_only_change_renders_exactly_one_set_statement() {
        let harness = build_harness();
        // Only `snowflake_warehouse` is present in the changeset: no `target_lag` entry and no
        // initialization-warehouse entry, so `warehouse = ...` is the only clause that could
        // possibly render.
        let rendered = render_alter(
            &harness,
            changeset(false, None, Some(changed_to(Some("OTHER_WH"))), None),
        );
        assert_eq!(
            statements(&rendered),
            vec![format!(
                "alter interactive table {RENDERED_RELATION} set warehouse = OTHER_WH"
            )],
            "a warehouse-only change is exactly one set statement, got:\n{rendered}"
        );
    }

    #[test]
    fn warehouse_change_is_separated_from_a_cleared_initialization_warehouse() {
        let harness = build_harness();
        // `target_lag` is absent here; `snowflake_warehouse` alone drives the set statement,
        // alongside a cleared initialization warehouse driving the unset statement. The two
        // must still come out as separate, well-formed statements.
        let rendered = render_alter(
            &harness,
            changeset(
                false,
                None,
                Some(changed_to(Some("WH_B"))),
                Some(changed_to(None)),
            ),
        );

        let statements = statements(&rendered);
        assert_eq!(
            statements.len(),
            2,
            "a warehouse change alongside a clear is two separated statements, got:\n{rendered}"
        );
        assert_eq!(
            statements[0],
            format!("alter interactive table {RENDERED_RELATION} set warehouse = WH_B"),
            "the set statement is malformed, got:\n{rendered}"
        );
        assert_eq!(
            statements[1],
            format!("alter interactive table {RENDERED_RELATION} unset initialization_warehouse"),
            "the unset statement is malformed, got:\n{rendered}"
        );
    }

    #[test]
    fn drop_uses_a_plain_drop_table() {
        let harness = build_harness();
        let ctx = BTreeMap::from([("relation".to_string(), relation_value(&harness))]);
        let rendered = harness
            .render(
                "{{ snowflake__get_drop_interactive_table_sql(relation) }}",
                ctx,
            )
            .expect("drop should render");

        assert_eq!(
            rendered.split_whitespace().collect::<Vec<_>>().join(" "),
            format!("drop table if exists {RENDERED_RELATION}"),
            "got:\n{rendered}"
        );
    }

    #[test]
    fn rename_uses_a_plain_alter_table_rename() {
        let harness = build_harness();
        let ctx = BTreeMap::from([
            ("relation".to_string(), relation_value(&harness)),
            ("new_name".to_string(), Value::from("MY_IT__BACKUP")),
        ]);
        let rendered = harness
            .render(
                "{{ snowflake__get_rename_interactive_table_sql(relation, new_name) }}",
                ctx,
            )
            .expect("rename should render");
        let lower = rendered.to_lowercase();

        assert!(
            lower.contains(&format!(
                "alter table {} rename to my_it__backup",
                RENDERED_RELATION.to_lowercase()
            )),
            "got:\n{rendered}"
        );
        assert!(
            !lower.contains("alter interactive table"),
            "an interactive table is renamed as a plain table, got:\n{rendered}"
        );
    }
}

/// Tests for the `snowflake__get_{create,replace,drop}_sql` dispatch routers in
/// `relations/{create,replace,drop}.sql`, which route a relation to its dynamic-table,
/// interactive-table, or default DDL path based on `relation.is_dynamic_table` /
/// `relation.is_interactive_table`.
///
/// The branches under test are stubbed with markers rather than the real DDL macros: only the
/// routing decision is in scope here, not the DDL each branch produces (that is covered
/// elsewhere, e.g. `snowflake_interactive_table` above).
mod snowflake_relation_dispatch {
    use dbt_adapter::relation::RelationObject;

    use super::*;

    const CREATE_ROUTER_PATH: &str = "dbt-snowflake/macros/relations/create.sql";
    const REPLACE_ROUTER_PATH: &str = "dbt-snowflake/macros/relations/replace.sql";
    const DROP_ROUTER_PATH: &str = "dbt-snowflake/macros/relations/drop.sql";

    const CREATE_ROUTER_SQL: &str =
        include_str!("../../src/dbt_macro_assets/dbt-snowflake/macros/relations/create.sql");
    const REPLACE_ROUTER_SQL: &str =
        include_str!("../../src/dbt_macro_assets/dbt-snowflake/macros/relations/replace.sql");
    const DROP_ROUTER_SQL: &str =
        include_str!("../../src/dbt_macro_assets/dbt-snowflake/macros/relations/drop.sql");

    const DYNAMIC_MARKER: &str = "dispatched-to-dynamic-table";
    const INTERACTIVE_MARKER: &str = "dispatched-to-interactive-table";
    const DEFAULT_MARKER: &str = "dispatched-to-default";

    fn stub(name: &str, params: &str, marker: &str) -> String {
        format!("{{% macro {name}({params}) %}}{marker}{{% endmacro %}}")
    }

    fn build_create_harness() -> MacroTestHarness {
        MacroTestHarness::for_adapter(AdapterType::Snowflake)
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__get_create_sql",
                CREATE_ROUTER_SQL,
                CREATE_ROUTER_PATH,
            )
            .with_macro(
                "dbt_snowflake",
                "snowflake__get_create_dynamic_table_as_sql",
                &stub(
                    "snowflake__get_create_dynamic_table_as_sql",
                    "relation, sql",
                    DYNAMIC_MARKER,
                ),
            )
            .with_macro(
                "dbt_snowflake",
                "snowflake__get_create_interactive_table_as_sql",
                &stub(
                    "snowflake__get_create_interactive_table_as_sql",
                    "relation, sql",
                    INTERACTIVE_MARKER,
                ),
            )
            .with_macro(
                "dbt_snowflake",
                "default__get_create_sql",
                &stub("default__get_create_sql", "relation, sql", DEFAULT_MARKER),
            )
            .build()
            .expect("create router harness should build")
    }

    fn build_replace_harness() -> MacroTestHarness {
        MacroTestHarness::for_adapter(AdapterType::Snowflake)
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__get_replace_sql",
                REPLACE_ROUTER_SQL,
                REPLACE_ROUTER_PATH,
            )
            .with_macro(
                "dbt_snowflake",
                "snowflake__get_replace_dynamic_table_sql",
                &stub(
                    "snowflake__get_replace_dynamic_table_sql",
                    "relation, sql",
                    DYNAMIC_MARKER,
                ),
            )
            .with_macro(
                "dbt_snowflake",
                "snowflake__get_replace_interactive_table_sql",
                &stub(
                    "snowflake__get_replace_interactive_table_sql",
                    "relation, sql",
                    INTERACTIVE_MARKER,
                ),
            )
            .with_macro(
                "dbt_snowflake",
                "default__get_replace_sql",
                &stub(
                    "default__get_replace_sql",
                    "existing_relation, target_relation, sql",
                    DEFAULT_MARKER,
                ),
            )
            .build()
            .expect("replace router harness should build")
    }

    fn build_drop_harness() -> MacroTestHarness {
        MacroTestHarness::for_adapter(AdapterType::Snowflake)
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__get_drop_sql",
                DROP_ROUTER_SQL,
                DROP_ROUTER_PATH,
            )
            .with_macro(
                "dbt_snowflake",
                "snowflake__get_drop_dynamic_table_sql",
                &stub(
                    "snowflake__get_drop_dynamic_table_sql",
                    "relation",
                    DYNAMIC_MARKER,
                ),
            )
            .with_macro(
                "dbt_snowflake",
                "snowflake__get_drop_interactive_table_sql",
                &stub(
                    "snowflake__get_drop_interactive_table_sql",
                    "relation",
                    INTERACTIVE_MARKER,
                ),
            )
            .with_macro(
                "dbt_snowflake",
                "default__get_drop_sql",
                &stub("default__get_drop_sql", "relation", DEFAULT_MARKER),
            )
            .build()
            .expect("drop router harness should build")
    }

    fn relation_value(harness: &MacroTestHarness, relation_type: RelationType) -> Value {
        let relation = harness.relation("TEST_DB", "TEST_SCHEMA", "MY_REL", Some(relation_type));
        RelationObject::new(relation).into_value()
    }

    const ALL_MARKERS: [&str; 3] = [DYNAMIC_MARKER, INTERACTIVE_MARKER, DEFAULT_MARKER];

    /// Assert the router reached `expected` and none of the other branches. `case` identifies
    /// which row of the caller's table produced `rendered`.
    fn assert_only(rendered: &str, expected: &str, case: &str) {
        assert!(
            rendered.contains(expected),
            "case {case}: expected marker {expected:?}, got:\n{rendered}"
        );
        for other in ALL_MARKERS.iter().filter(|other| **other != expected) {
            assert!(
                !rendered.contains(other),
                "case {case}: did not expect marker {other:?}, got:\n{rendered}"
            );
        }
    }

    #[test]
    fn create_sql_routes_by_relation_type() {
        let harness = build_create_harness();
        for (relation_type, expected) in [
            (RelationType::InteractiveTable, INTERACTIVE_MARKER),
            (RelationType::DynamicTable, DYNAMIC_MARKER),
        ] {
            let ctx = BTreeMap::from([
                (
                    "relation".to_string(),
                    relation_value(&harness, relation_type),
                ),
                ("sql".to_string(), Value::from("select 1")),
            ]);
            let rendered = harness
                .render("{{ snowflake__get_create_sql(relation, sql) }}", ctx)
                .expect("create should render");
            assert_only(&rendered, expected, &format!("{relation_type:?}"));
        }
    }

    #[test]
    fn replace_sql_routes_by_relation_types() {
        let harness = build_replace_harness();
        for (existing, target, expected) in [
            (
                RelationType::InteractiveTable,
                RelationType::InteractiveTable,
                INTERACTIVE_MARKER,
            ),
            // Only one side interactive falls through to the default branch either way round.
            (
                RelationType::InteractiveTable,
                RelationType::Table,
                DEFAULT_MARKER,
            ),
            (
                RelationType::Table,
                RelationType::InteractiveTable,
                DEFAULT_MARKER,
            ),
            (
                RelationType::DynamicTable,
                RelationType::DynamicTable,
                DYNAMIC_MARKER,
            ),
        ] {
            let ctx = BTreeMap::from([
                (
                    "existing_relation".to_string(),
                    relation_value(&harness, existing),
                ),
                (
                    "target_relation".to_string(),
                    relation_value(&harness, target),
                ),
                ("sql".to_string(), Value::from("select 1")),
            ]);
            let rendered = harness
                .render(
                    "{{ snowflake__get_replace_sql(existing_relation, target_relation, sql) }}",
                    ctx,
                )
                .expect("replace should render");
            assert_only(&rendered, expected, &format!("{existing:?} -> {target:?}"));
        }
    }

    #[test]
    fn drop_sql_routes_by_relation_type() {
        let harness = build_drop_harness();
        for (relation_type, expected) in [
            (RelationType::InteractiveTable, INTERACTIVE_MARKER),
            (RelationType::DynamicTable, DYNAMIC_MARKER),
        ] {
            let ctx = BTreeMap::from([(
                "relation".to_string(),
                relation_value(&harness, relation_type),
            )]);
            let rendered = harness
                .render("{{ snowflake__get_drop_sql(relation) }}", ctx)
                .expect("drop should render");
            assert_only(&rendered, expected, &format!("{relation_type:?}"));
        }
    }

    /// Both flags derive from the relation's single `relation_type()`, so at most one can be true —
    /// the create/replace/drop routers rely on that and check `is_interactive_table` first with no
    /// mixed-kind branch. Scope: this covers the `RelationType` derivation only, not Snowflake's
    /// raw `IS_DYNAMIC`/`IS_INTERACTIVE` columns, which a dynamic interactive table reports as
    /// `'YES'` simultaneously.
    #[test]
    fn is_dynamic_table_and_is_interactive_table_are_mutually_exclusive() {
        let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
            .build()
            .expect("harness should build");

        let interactive = relation_value(&harness, RelationType::InteractiveTable);
        let dynamic = relation_value(&harness, RelationType::DynamicTable);

        let ctx = BTreeMap::from([
            ("interactive".to_string(), interactive),
            ("dynamic".to_string(), dynamic),
        ]);
        let rendered = harness
            .render(
                "{{ interactive.is_dynamic_table }}|{{ interactive.is_interactive_table }}|\
                 {{ dynamic.is_dynamic_table }}|{{ dynamic.is_interactive_table }}",
                ctx,
            )
            .expect("render should succeed");

        assert_eq!(
            rendered, "False|True|True|False",
            "an interactive-table relation and a dynamic-table relation must each report \
             exactly one of the two flags as true, got:\n{rendered}"
        );
    }
}

/// Tests for the relation-type keyword chosen by three `adapters.sql` macros —
/// `snowflake__alter_relation_comment`, `snowflake__alter_column_comment`, and
/// `snowflake__alter_relation_add_remove_columns` — each of which interpolates a DDL keyword
/// (`table`, `dynamic table`, or the raw `relation.type`) ahead of the relation name. All three
/// fall back to `relation.type` for a kind they don't special-case, and `relation.type` for an
/// interactive table is the string `interactive_table` (with an underscore), which is not valid
/// Snowflake DDL syntax.
///
/// These tests load every real macro the adapter ships with — `run_query`/`statement` for the
/// add/remove-columns macro, `get_column_comment_sql` for the column-comment macro — because the
/// object under test is the fully assembled DDL string, not an isolated branch.
mod snowflake_alter_relation_type_keyword {
    use dbt_adapter::relation::RelationObject;
    use dbt_common::FsResult;

    use crate::macro_test_harness::executed_sql;

    use super::*;

    fn build_harness() -> MacroTestHarness {
        let mut harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
            .load_all_macros()
            .with_stub_functions()
            .build()
            .expect("harness should build");

        harness
            .env_mut()
            .env
            .add_function("load_result", |_name: Value| {
                Ok(Value::from_serialize(BTreeMap::from([(
                    "table",
                    Vec::<Vec<Value>>::new(),
                )])))
            });
        harness
            .env_mut()
            .env
            .add_global("execute", Value::from(true));
        harness.mock().on("quote", |args| {
            Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
        });

        harness
    }

    fn relation_value(harness: &MacroTestHarness, relation_type: RelationType) -> Value {
        let relation = harness.relation("TEST_DB", "TEST_SCHEMA", "MY_REL", Some(relation_type));
        RelationObject::new(relation).into_value()
    }

    fn column(name: &str, data_type: &str) -> Value {
        Value::from_serialize(BTreeMap::from([("name", name), ("data_type", data_type)]))
    }

    fn normalized(rendered: &str) -> String {
        rendered.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    mod alter_relation_comment {
        use super::*;

        fn render(harness: &MacroTestHarness, relation_type: RelationType) -> String {
            let ctx = BTreeMap::from([
                (
                    "relation".to_string(),
                    relation_value(harness, relation_type),
                ),
                (
                    "relation_comment".to_string(),
                    Value::from("a relation comment"),
                ),
            ]);
            harness
                .render(
                    "{{ snowflake__alter_relation_comment(relation, relation_comment) }}",
                    ctx,
                )
                .expect("render should succeed")
        }

        #[test]
        fn uses_table_for_an_interactive_relation() {
            let harness = build_harness();
            let rendered = render(&harness, RelationType::InteractiveTable);
            assert_eq!(
                normalized(&rendered),
                "comment on table TEST_DB.TEST_SCHEMA.MY_REL IS $$a relation comment$$;",
                "got:\n{rendered}"
            );
            assert!(
                !rendered.to_lowercase().contains("interactive_table"),
                "must not interpolate the underscore relation-type string into DDL, got:\n{rendered}"
            );
        }

        #[test]
        fn still_uses_dynamic_table_for_a_dynamic_relation() {
            let harness = build_harness();
            let rendered = render(&harness, RelationType::DynamicTable);
            assert_eq!(
                normalized(&rendered),
                "comment on dynamic table TEST_DB.TEST_SCHEMA.MY_REL IS $$a relation comment$$;",
                "got:\n{rendered}"
            );
        }

        #[test]
        fn still_uses_relation_type_for_a_plain_table() {
            let harness = build_harness();
            let rendered = render(&harness, RelationType::Table);
            assert_eq!(
                normalized(&rendered),
                "comment on table TEST_DB.TEST_SCHEMA.MY_REL IS $$a relation comment$$;",
                "got:\n{rendered}"
            );
        }
    }

    mod alter_column_comment {
        use super::*;

        fn render(harness: &MacroTestHarness, relation_type: RelationType) -> String {
            harness.mock().on("get_columns_in_relation", |_| {
                Ok(Value::from_serialize(vec![BTreeMap::from([(
                    "name", "ID",
                )])]))
            });
            let column_dict = Value::from_serialize(BTreeMap::from([(
                "ID".to_string(),
                BTreeMap::from([("description".to_string(), "the identifier".to_string())]),
            )]));
            let ctx = BTreeMap::from([
                (
                    "relation".to_string(),
                    relation_value(harness, relation_type),
                ),
                ("column_dict".to_string(), column_dict),
                (
                    "config".to_string(),
                    Value::from_dyn_object(default_mock_config()),
                ),
            ]);
            harness
                .render(
                    "{{ snowflake__alter_column_comment(relation, column_dict) }}",
                    ctx,
                )
                .expect("render should succeed")
        }

        #[test]
        fn uses_table_for_an_interactive_relation() {
            let harness = build_harness();
            let rendered = render(&harness, RelationType::InteractiveTable);
            assert_eq!(
                normalized(&rendered),
                "alter table TEST_DB.TEST_SCHEMA.MY_REL alter ID COMMENT $$the identifier$$;",
                "got:\n{rendered}"
            );
        }

        #[test]
        fn still_uses_table_for_a_dynamic_relation() {
            let harness = build_harness();
            let rendered = render(&harness, RelationType::DynamicTable);
            assert_eq!(
                normalized(&rendered),
                "alter table TEST_DB.TEST_SCHEMA.MY_REL alter ID COMMENT $$the identifier$$;",
                "got:\n{rendered}"
            );
        }

        #[test]
        fn still_uses_relation_type_for_a_plain_table() {
            let harness = build_harness();
            let rendered = render(&harness, RelationType::Table);
            assert_eq!(
                normalized(&rendered),
                "alter table TEST_DB.TEST_SCHEMA.MY_REL alter ID COMMENT $$the identifier$$;",
                "got:\n{rendered}"
            );
        }
    }

    /// Reachable with an interactive relation today — not via the interactive-table
    /// materialization (which never calls `process_schema_changes`), but via Snowflake's
    /// `incremental`: a model previously built as an interactive table, rebuilt as `incremental`
    /// without `--full-refresh`, with `on_schema_change: sync_all_columns` and a dropped column.
    mod alter_relation_add_remove_columns {
        use super::*;

        fn render(
            harness: &MacroTestHarness,
            relation_type: RelationType,
            add_columns: Value,
            remove_columns: Value,
        ) -> FsResult<String> {
            let ctx = BTreeMap::from([
                (
                    "relation".to_string(),
                    relation_value(harness, relation_type),
                ),
                ("add_columns".to_string(), add_columns),
                ("remove_columns".to_string(), remove_columns),
            ]);
            harness.render(
                "{{ snowflake__alter_relation_add_remove_columns(relation, add_columns, remove_columns) }}",
                ctx,
            )
        }

        #[test]
        fn add_columns_uses_table_for_an_interactive_relation() {
            let harness = build_harness();
            render(
                &harness,
                RelationType::InteractiveTable,
                Value::from(vec![column("NEW_COL", "VARCHAR(16777216)")]),
                Value::from(Vec::<Value>::new()),
            )
            .expect("render should succeed");

            let executed = executed_sql(harness.mock());
            assert_eq!(
                executed.len(),
                1,
                "exactly one add-column statement should run, got: {executed:?}"
            );
            assert_eq!(
                normalized(&executed[0]),
                "alter table TEST_DB.TEST_SCHEMA.MY_REL add column NEW_COL VARCHAR(16777216)",
                "got:\n{}",
                executed[0]
            );
        }

        #[test]
        fn rejects_remove_columns_for_an_interactive_relation() {
            let harness = build_harness();
            let err = render(
                &harness,
                RelationType::InteractiveTable,
                Value::from(vec![column("NEW_COL", "VARCHAR(16777216)")]),
                Value::from(vec![column("OLD_COL", "VARCHAR(16777216)")]),
            )
            .expect_err("removing a column from an interactive table has no valid DDL");

            let message = err.to_string();
            assert!(
                message.contains("cannot be removed"),
                "error must name the column-removal limitation, got: {message}"
            );
            assert!(
                message.contains("TEST_DB.TEST_SCHEMA.MY_REL"),
                "error must name the relation, got: {message}"
            );
            assert!(
                executed_sql(harness.mock()).is_empty(),
                "no DDL should run once the removal is rejected"
            );
        }

        #[test]
        fn allows_empty_or_absent_remove_columns_for_an_interactive_relation() {
            let harness = build_harness();
            for remove_columns in [Value::from(Vec::<Value>::new()), Value::from(())] {
                render(
                    &harness,
                    RelationType::InteractiveTable,
                    Value::from(vec![column("NEW_COL", "VARCHAR(16777216)")]),
                    remove_columns,
                )
                .expect("an interactive table with nothing to remove must not raise");
            }
        }

        #[test]
        fn still_uses_dynamic_table_for_a_dynamic_relation() {
            let harness = build_harness();
            render(
                &harness,
                RelationType::DynamicTable,
                Value::from(vec![column("NEW_COL", "VARCHAR(16777216)")]),
                Value::from(vec![column("OLD_COL", "VARCHAR(16777216)")]),
            )
            .expect("a dynamic table must still be able to add and remove columns");

            let executed = executed_sql(harness.mock());
            assert_eq!(
                executed.len(),
                2,
                "both an add and a drop statement should run, got: {executed:?}"
            );
            assert_eq!(
                normalized(&executed[0]),
                "alter dynamic table TEST_DB.TEST_SCHEMA.MY_REL add column NEW_COL VARCHAR(16777216)",
                "got:\n{}",
                executed[0]
            );
            assert_eq!(
                normalized(&executed[1]),
                "alter dynamic table TEST_DB.TEST_SCHEMA.MY_REL drop column OLD_COL",
                "got:\n{}",
                executed[1]
            );
        }

        #[test]
        fn still_uses_relation_type_for_a_plain_table() {
            let harness = build_harness();
            render(
                &harness,
                RelationType::Table,
                Value::from(vec![column("NEW_COL", "VARCHAR(16777216)")]),
                Value::from(vec![column("OLD_COL", "VARCHAR(16777216)")]),
            )
            .expect("a plain table must still be able to add and remove columns");

            let executed = executed_sql(harness.mock());
            assert_eq!(
                executed.len(),
                2,
                "both an add and a drop statement should run, got: {executed:?}"
            );
            assert_eq!(
                normalized(&executed[0]),
                "alter table TEST_DB.TEST_SCHEMA.MY_REL add column NEW_COL VARCHAR(16777216)",
                "got:\n{}",
                executed[0]
            );
            assert_eq!(
                normalized(&executed[1]),
                "alter table TEST_DB.TEST_SCHEMA.MY_REL drop column OLD_COL",
                "got:\n{}",
                executed[1]
            );
        }
    }
}

/// Compared on normalized output: dispatching wraps the delegated call in block tags, so the raw
/// strings differ by inert whitespace.
mod snowflake_rename_dispatch {
    use dbt_adapter::relation::RelationObject;

    use super::*;

    fn build_harness() -> MacroTestHarness {
        MacroTestHarness::for_adapter(AdapterType::Snowflake)
            .load_all_macros()
            .build()
            .expect("rename router harness should build")
    }

    fn relation_value(harness: &MacroTestHarness, relation_type: RelationType) -> Value {
        let relation = harness.relation("TEST_DB", "TEST_SCHEMA", "MY_REL", Some(relation_type));
        RelationObject::new(relation).into_value()
    }

    fn ctx_for(relation: Value, new_name: &str) -> BTreeMap<String, Value> {
        BTreeMap::from([
            ("relation".to_string(), relation),
            ("new_name".to_string(), Value::from(new_name)),
        ])
    }

    fn normalized(rendered: &str) -> String {
        rendered.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The interactive-table macros carry a `/* ... */` docstring into the statement they emit,
    /// so a whole-statement comparison needs to strip it first.
    fn strip_block_comments(sql: &str) -> String {
        let mut out = String::with_capacity(sql.len());
        let mut rest = sql;
        while let Some(start) = rest.find("/*") {
            out.push_str(&rest[..start]);
            match rest[start..].find("*/") {
                Some(end) => rest = &rest[start + end + 2..],
                None => return out,
            }
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn routes_interactive_relation_to_the_interactive_macro() {
        let harness = build_harness();

        let router_out = harness
            .render(
                "{{ snowflake__get_rename_sql(relation, new_name) }}",
                ctx_for(
                    relation_value(&harness, RelationType::InteractiveTable),
                    "TEST_DB.TEST_SCHEMA.MY_REL__dbt_backup",
                ),
            )
            .expect("the router should render an interactive relation");
        let direct_out = harness
            .render(
                "{{ snowflake__get_rename_interactive_table_sql(relation, new_name) }}",
                ctx_for(
                    relation_value(&harness, RelationType::InteractiveTable),
                    "TEST_DB.TEST_SCHEMA.MY_REL__dbt_backup",
                ),
            )
            .expect("the interactive macro should render directly");

        assert_eq!(
            normalized(&router_out),
            normalized(&direct_out),
            "the router must produce exactly what the interactive macro produces directly"
        );
        assert_eq!(
            normalized(&strip_block_comments(&router_out)),
            "alter table TEST_DB.TEST_SCHEMA.MY_REL rename to TEST_DB.TEST_SCHEMA.MY_REL__dbt_backup",
            "got:\n{router_out}"
        );
    }

    #[test]
    fn routes_view_to_the_default_path_unchanged() {
        let harness = build_harness();

        let router_out = harness
            .render(
                "{{ snowflake__get_rename_sql(relation, new_name) }}",
                ctx_for(
                    relation_value(&harness, RelationType::View),
                    "TEST_DB.TEST_SCHEMA.MY_REL__dbt_backup",
                ),
            )
            .expect("the router should render a view relation");
        let default_out = harness
            .render(
                "{{ default__get_rename_sql(relation, new_name) }}",
                ctx_for(
                    relation_value(&harness, RelationType::View),
                    "TEST_DB.TEST_SCHEMA.MY_REL__dbt_backup",
                ),
            )
            .expect("default__get_rename_sql should render a view relation directly");

        assert_eq!(
            normalized(&router_out),
            normalized(&default_out),
            "a view must render exactly as it does through default__get_rename_sql today"
        );
    }

    #[test]
    fn routes_table_to_the_default_path_unchanged() {
        let harness = build_harness();

        let router_out = harness
            .render(
                "{{ snowflake__get_rename_sql(relation, new_name) }}",
                ctx_for(
                    relation_value(&harness, RelationType::Table),
                    "TEST_DB.TEST_SCHEMA.MY_REL__dbt_backup",
                ),
            )
            .expect("the router should render a table relation");
        let default_out = harness
            .render(
                "{{ default__get_rename_sql(relation, new_name) }}",
                ctx_for(
                    relation_value(&harness, RelationType::Table),
                    "TEST_DB.TEST_SCHEMA.MY_REL__dbt_backup",
                ),
            )
            .expect("default__get_rename_sql should render a table relation directly");

        assert_eq!(
            normalized(&router_out),
            normalized(&default_out),
            "a table must render exactly as it does through default__get_rename_sql today"
        );
    }

    /// The third and last type `default__get_rename_sql` handles itself. Covered for the same reason
    /// as view and table: the router now intercepts every Snowflake rename, so each type the default
    /// used to receive directly has to still come out the same way.
    ///
    /// This one renders `alter dynamic table`, because Snowflake's materialized-view rename macro is
    /// the dynamic-table statement. That reads as a contradiction next to
    /// `dynamic_table_still_raises_exactly_as_today` until you know the two are reached differently:
    /// a `DynamicTable` relation does not satisfy `is_materialized_view`, so it falls past every arm
    /// of `default__get_rename_sql` and raises. Both behaviours are what ships today; this test pins
    /// the pair so a change to either is deliberate.
    #[test]
    fn routes_materialized_view_to_the_default_path_unchanged() {
        let harness = build_harness();

        let router_out = harness
            .render(
                "{{ snowflake__get_rename_sql(relation, new_name) }}",
                ctx_for(
                    relation_value(&harness, RelationType::MaterializedView),
                    "TEST_DB.TEST_SCHEMA.MY_REL__dbt_backup",
                ),
            )
            .expect("the router should render a materialized-view relation");
        let default_out = harness
            .render(
                "{{ default__get_rename_sql(relation, new_name) }}",
                ctx_for(
                    relation_value(&harness, RelationType::MaterializedView),
                    "TEST_DB.TEST_SCHEMA.MY_REL__dbt_backup",
                ),
            )
            .expect("default__get_rename_sql should render a materialized view directly");

        assert_eq!(
            normalized(&router_out),
            normalized(&default_out),
            "a materialized view must render exactly as it does through default__get_rename_sql today"
        );
    }

    #[test]
    fn dynamic_table_still_raises_exactly_as_today() {
        let harness = build_harness();

        let router_err = harness
            .render(
                "{{ snowflake__get_rename_sql(relation, new_name) }}",
                ctx_for(
                    relation_value(&harness, RelationType::DynamicTable),
                    "TEST_DB.TEST_SCHEMA.MY_REL__dbt_backup",
                ),
            )
            .expect_err("a dynamic table is not renameable and the router must not change that");
        let default_err = harness
            .render(
                "{{ default__get_rename_sql(relation, new_name) }}",
                ctx_for(
                    relation_value(&harness, RelationType::DynamicTable),
                    "TEST_DB.TEST_SCHEMA.MY_REL__dbt_backup",
                ),
            )
            .expect_err("default__get_rename_sql must still raise for a dynamic table directly");

        // The router adds one more macro frame to the error's call-stack annotation than calling
        // `default__get_rename_sql` directly, so the two are compared on the raised message
        // itself, not the full annotated string.
        let core_message = "`get_rename_sql` has not been implemented for: dynamic_table";
        assert!(
            router_err.to_string().contains(core_message),
            "got: {router_err}"
        );
        assert!(
            default_err.to_string().contains(core_message),
            "got: {default_err}"
        );
    }
}

/// Pins the `table_type` classification in `catalog.sql`'s `snowflake__get_catalog_tables_sql`.
mod snowflake_catalog {
    use std::sync::Arc;

    use dbt_jinja_utils::mock_object::MockJinjaObject;

    use super::*;

    const CATALOG_PATH: &str = "dbt-snowflake/macros/catalog.sql";
    const CATALOG_SQL: &str =
        include_str!("../../src/dbt_macro_assets/dbt-snowflake/macros/catalog.sql");

    fn build_harness() -> MacroTestHarness {
        MacroTestHarness::for_adapter(AdapterType::Snowflake)
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__get_catalog_tables_sql",
                CATALOG_SQL,
                CATALOG_PATH,
            )
            .build()
            .expect("catalog harness should build")
    }

    fn dbschema(database: &str) -> Value {
        let obj = Arc::new(MockJinjaObject::new());
        obj.set_attr("database", Value::from(database));
        Value::from_dyn_object(obj)
    }

    fn normalized(rendered: &str) -> String {
        rendered.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The `table_type` CASE only ever rewrites a row when `IS_DYNAMIC = 'YES' AND
    /// TABLE_TYPE = 'BASE TABLE'`; every other `table_type` value passes through the `else`
    /// branch unchanged. `information_schema.tables` reports an interactive table's
    /// `TABLE_TYPE` as `'INTERACTIVE TABLE'` regardless of `IS_DYNAMIC`, so the `when` arm can
    /// never fire for it: it always falls through `else` and keeps its own type.
    ///
    /// This pins the exact `case` expression by full equality rather than by its `when` and
    /// `else` arms as separate substrings: two substrings both stay present verbatim (and the
    /// test stays green) even if a new arm is inserted between them, so only comparing the whole
    /// construct catches an addition that would widen or reorder the classification.
    #[test]
    fn table_type_case_passes_interactive_tables_through_unmodified() {
        let harness = build_harness();
        let ctx = BTreeMap::from([("dbschema".to_string(), dbschema("TEST_DB"))]);
        let rendered = harness
            .render("{{ snowflake__get_catalog_tables_sql(dbschema) }}", ctx)
            .expect("catalog tables sql should render");
        let lower = normalized(&rendered).to_lowercase();

        let case_start = lower
            .find("case ")
            .expect("a table_type case expression should be present");
        let end_marker = "end as \"table_type\",";
        let case_end = lower[case_start..]
            .find(end_marker)
            .map(|offset| case_start + offset + end_marker.len())
            .expect("the table_type case expression should close");

        assert_eq!(
            &lower[case_start..case_end],
            "case when is_dynamic = 'yes' and table_type = 'base table' then 'dynamic table' \
             else table_type end as \"table_type\",",
            "the table_type case expression must classify only a plain base table with \
             is_dynamic = 'yes' as a dynamic table and pass every other table_type, including \
             'interactive table', through unchanged, got:\n{rendered}"
        );
    }

    /// The `stats:last_modified:include` gate tests the raw `table_type` column (not the
    /// `"table_type"` alias computed above), so it must list every raw value that should get
    /// the stat rather than exclude the ones that shouldn't. Pinning the whole expression by
    /// full equality, rather than checking for `'base table'` and `'interactive table'` as
    /// separate substrings, is what proves both are admitted by the *same* condition and that
    /// the `last_altered is not null` conjunct is still present — a substring check on either
    /// piece alone would stay green even if the other were dropped or the null check removed.
    #[test]
    fn last_modified_include_admits_base_and_interactive_tables() {
        let harness = build_harness();
        let ctx = BTreeMap::from([("dbschema".to_string(), dbschema("TEST_DB"))]);
        let rendered = harness
            .render("{{ snowflake__get_catalog_tables_sql(dbschema) }}", ctx)
            .expect("catalog tables sql should render");
        let lower = normalized(&rendered).to_lowercase();

        let expr_start = lower
            .find("(last_altered is not null")
            .expect("a last_altered null-check should be present");
        let end_marker = "as \"stats:last_modified:include\"";
        let expr_end = lower[expr_start..]
            .find(end_marker)
            .map(|offset| expr_start + offset + end_marker.len())
            .expect("the stats:last_modified:include expression should close");

        assert_eq!(
            &lower[expr_start..expr_end],
            "(last_altered is not null and table_type in ('base table', 'interactive table')) \
             as \"stats:last_modified:include\"",
            "the last_modified stat must be included for both a plain base table and an \
             interactive table, and must still require a non-null last_altered, got:\n{rendered}"
        );
    }

    /// The `clustering_key`, `row_count` and `bytes` stats each gate on their own column's
    /// null-check alone. None of them should pick up a `table_type` test just because the
    /// `last_modified` gate above needs one.
    #[test]
    fn other_stats_include_gates_are_unaffected() {
        let harness = build_harness();
        let ctx = BTreeMap::from([("dbschema".to_string(), dbschema("TEST_DB"))]);
        let rendered = harness
            .render("{{ snowflake__get_catalog_tables_sql(dbschema) }}", ctx)
            .expect("catalog tables sql should render");
        let lower = normalized(&rendered).to_lowercase();

        for (column, label) in [
            ("clustering_key", "stats:clustering_key:include"),
            ("row_count", "stats:row_count:include"),
            ("bytes", "stats:bytes:include"),
        ] {
            let marker = format!("as \"{label}\"");
            let end = lower
                .find(&marker)
                .map(|offset| offset + marker.len())
                .unwrap_or_else(|| panic!("{label} should be present"));
            let expected_tail = format!("({column} is not null) {marker}");
            assert!(
                lower[..end].ends_with(&expected_tail),
                "{label} must gate only on `{column} is not null`, with no table_type test, \
                 got:\n{rendered}"
            );
        }
    }
}
