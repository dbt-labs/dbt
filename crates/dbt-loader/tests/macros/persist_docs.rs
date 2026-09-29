use std::collections::BTreeMap;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, resolved_quoting_for};

mod databricks {
    use super::*;

    fn build_comment_on_column_harness() -> MacroTestHarness {
        let databricks_persist_docs_sql = include_str!(
            "../../src/dbt_macro_assets/dbt-databricks/macros/adapters/persist_docs.sql"
        );

        // Until Fusion's macro assets implement `comment_on_column_sql`, define a fallback to ensure
        // the test fails via assertion (wrong SQL) rather than panicking (missing macro).
        let fallback = r#"
{% macro comment_on_column_sql(column_path, escaped_comment) -%}
  alter table {{ column_path.rsplit('.', 1)[0] }} change column {{ column_path.rsplit('.', 1)[1] }} comment '{{ escaped_comment }}'
{%- endmacro %}
"#;
        let combined = format!("{fallback}\n{databricks_persist_docs_sql}");

        let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
            .with_root_package("test_package".to_string())
            .with_macro_at_path(
                "dbt_databricks",
                "comment_on_column_sql",
                &combined,
                "dbt_macro_assets/dbt-databricks/macros/adapters/persist_docs.sql",
            )
            .build()
            .expect("harness should build");

        harness.mock().on("has_feature", |_| Ok(Value::from(false)));

        harness
    }

    #[test]
    fn comment_on_column_sql_uses_legacy_alter_table_syntax() {
        let harness = build_comment_on_column_harness();
        let ctx = BTreeMap::from([
            (
                "column_path".to_string(),
                Value::from("`dbt`.`dbt_entities`.`ent_shopify_inventory_quantity`.`id`"),
            ),
            (
                "escaped_comment".to_string(),
                Value::from("Primary key for the inventory quantity record."),
            ),
        ]);

        let rendered = harness
            .render(
                "{{ comment_on_column_sql(column_path, escaped_comment) }}",
                ctx,
            )
            .expect("render should succeed");

        assert_eq!(
            rendered.trim(),
            "ALTER TABLE `dbt`.`dbt_entities`.`ent_shopify_inventory_quantity` ALTER COLUMN `id` COMMENT 'Primary key for the inventory quantity record.'"
        );
    }

    #[test]
    fn get_persist_docs_column_list_skips_column_missing_from_query() {
        let view_create_sql = include_str!(
            "../../src/dbt_macro_assets/dbt-databricks/macros/relations/view/create.sql"
        );
        let generic_persist_docs_sql = include_str!(
            "../../src/dbt_macro_assets/dbt-adapters/macros/adapters/persist_docs.sql"
        );

        let combined = format!("{view_create_sql}\n{generic_persist_docs_sql}");

        let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
            .with_root_package("test_package".to_string())
            .with_macro_at_path(
                "dbt_databricks",
                "get_persist_docs_column_list",
                &combined,
                "dbt_macro_assets/dbt-databricks/macros/relations/view/create.sql",
            )
            .build()
            .expect("harness should build");

        harness.mock().on("quote", |args| {
            let name = args.first().and_then(|v| v.as_str()).unwrap_or_default();
            Ok(Value::from(format!("`{name}`")))
        });

        let relation = harness.relation(
            "test_db",
            "test_schema",
            "customers",
            Some(RelationType::View),
        );
        let ctx = BTreeMap::from([
            (
                "relation".to_string(),
                RelationObject::new(relation).into_value(),
            ),
            (
                "model_columns".to_string(),
                Value::from_serialize(BTreeMap::from([
                    (
                        "id",
                        BTreeMap::from([("description", Value::from("Primary key"))]),
                    ),
                    (
                        "email",
                        BTreeMap::from([("description", Value::from("Email address"))]),
                    ),
                ])),
            ),
            (
                "query_columns".to_string(),
                Value::from(vec![Value::from("id")]),
            ),
        ]);

        let rendered = harness
            .render(
                "{{ get_persist_docs_column_list(relation, model_columns, query_columns) }}",
                ctx,
            )
            .expect("render should succeed");

        assert_eq!(rendered.trim(), "`id` comment 'Primary key'");
    }
}

mod snowflake {
    use super::*;

    use std::sync::Arc;

    use dbt_adapter::catalog_relation::CatalogRelation;
    use dbt_adapter::relation::{Relation, RelationObject};
    use dbt_schemas::dbt_types::RelationType;
    use dbt_schemas::schemas::dbt_catalogs_v2::CatalogType;
    use dbt_schemas::schemas::relations::base::{BaseRelation, TableFormat};

    use crate::macro_test_harness::default_mock_config;

    const COLUMN: &str = "GEOGRAPHIC_MARKET";
    const DESCRIPTION: &str = "Market the customer belongs to";

    /// v1 spells the linked signal as `adapter_properties.catalog_linked_database`; catalogs
    /// v2 promotes it to the `catalog_database` field (behind `use_catalogs_v2`). The
    /// Snowflake-managed catalog may also set `catalog_database` under v2 — `ManagedIcebergV2`
    /// exists so that case isn't mistaken for catalog-linked.
    #[derive(Clone, Copy)]
    enum Catalog {
        LinkedV1,
        LinkedV2,
        ManagedIcebergV1,
        ManagedIcebergV2,
        NonIceberg,
    }

    impl Catalog {
        fn table_format(self) -> TableFormat {
            match self {
                Catalog::NonIceberg => TableFormat::Default,
                _ => TableFormat::Iceberg,
            }
        }

        fn catalog_relation(self) -> Value {
            let iceberg = || CatalogRelation {
                table_format: TableFormat::Iceberg,
                ..CatalogRelation::default_catalog_relation_snowflake()
            };
            match self {
                Catalog::LinkedV1 => Value::from_object(CatalogRelation {
                    catalog_type: CatalogType::IcebergRest,
                    adapter_properties: BTreeMap::from([(
                        "catalog_linked_database".to_string(),
                        "MY_LINKED_DB".to_string(),
                    )]),
                    ..iceberg()
                }),
                Catalog::LinkedV2 => Value::from_object(CatalogRelation {
                    catalog_type: CatalogType::IcebergRest,
                    catalog_database: Some("MY_LINKED_DB".to_string()),
                    ..iceberg()
                }),
                Catalog::ManagedIcebergV1 => Value::from_object(CatalogRelation {
                    catalog_type: CatalogType::SnowflakeBuiltIn,
                    external_volume: Some("MY_VOLUME".to_string()),
                    ..iceberg()
                }),
                Catalog::ManagedIcebergV2 => Value::from_object(CatalogRelation {
                    catalog_type: CatalogType::SnowflakeBuiltIn,
                    external_volume: Some("MY_VOLUME".to_string()),
                    catalog_database: Some("ANALYTICS_ICEBERG".to_string()),
                    ..iceberg()
                }),
                Catalog::NonIceberg => {
                    Value::from_object(CatalogRelation::default_catalog_relation_snowflake())
                }
            }
        }

        fn use_catalogs_v2(self) -> bool {
            matches!(self, Catalog::LinkedV2 | Catalog::ManagedIcebergV2)
        }
    }

    /// `warehouse_column` is the name `describe table` reports, which need not match the
    /// case the column is documented under — BigLake folds identifiers to lowercase.
    fn build_harness(warehouse_column: &'static str) -> MacroTestHarness {
        let snowflake_adapters_sql =
            include_str!("../../src/dbt_macro_assets/dbt-snowflake/macros/adapters.sql");

        let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__alter_column_comment",
                snowflake_adapters_sql,
                "dbt_macro_assets/dbt-snowflake/macros/adapters.sql",
            )
            // Registered separately so it's also callable on its own.
            .with_macro_at_path(
                "dbt_snowflake",
                "snowflake__is_catalog_linked_database",
                snowflake_adapters_sql,
                "dbt_macro_assets/dbt-snowflake/macros/adapters.sql",
            )
            .build()
            .expect("harness should build");

        harness.mock().on("get_columns_in_relation", move |_| {
            Ok(Value::from_serialize(vec![BTreeMap::from([(
                "name",
                warehouse_column,
            )])]))
        });
        harness.mock().on("quote", |args| {
            let identifier = args.first().and_then(|v| v.as_str()).unwrap_or_default();
            Ok(Value::from(format!("\"{identifier}\"")))
        });

        harness
    }

    fn squish(sql: &str) -> String {
        sql.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// `is_catalog_linked_database` reads this from process state, not `adapter.behavior` —
    /// safe because nextest runs each test in its own process.
    fn set_use_catalogs_v2(enabled: bool) {
        let flags: dbt_yaml::Value = dbt_yaml::from_str(&format!("use_catalogs_v2: {enabled}\n"))
            .expect("valid project flags");
        dbt_adapter::load_catalogs::set_use_catalogs_v2_from_flags(Some(&flags));
    }

    fn render_alter_column_comment(catalog: Catalog, documented: bool) -> String {
        let column_dict = if documented {
            Value::from_serialize(BTreeMap::from([(
                COLUMN,
                BTreeMap::from([("description", DESCRIPTION)]),
            )]))
        } else {
            Value::from_serialize(BTreeMap::<String, Value>::new())
        };
        render(catalog, COLUMN, column_dict)
    }

    fn render(catalog: Catalog, warehouse_column: &'static str, column_dict: Value) -> String {
        let harness = build_harness(warehouse_column);

        // Always set: the macro builds a catalog relation from config.model even for
        // plain tables.
        let config = default_mock_config();
        config.set_attr(
            "model",
            Value::from_serialize(BTreeMap::from([(
                "config",
                BTreeMap::from([("catalog_name", "my_catalog")]),
            )])),
        );
        let catalog_relation = catalog.catalog_relation();
        harness.mock().on("build_catalog_relation", move |_| {
            Ok(catalog_relation.clone())
        });
        set_use_catalogs_v2(catalog.use_catalogs_v2());

        let relation: Arc<dyn BaseRelation> = Arc::new(
            Relation::new(
                AdapterType::Snowflake,
                Some("MY_DB".to_string()),
                Some("MY_SCHEMA".to_string()),
                Some("MY_TABLE".to_string()),
            )
            .with_relation_type(Some(RelationType::Table))
            .with_quoting(resolved_quoting_for(AdapterType::Snowflake))
            .with_table_format(catalog.table_format()),
        );

        let ctx = BTreeMap::from([
            (
                "relation".to_string(),
                RelationObject::new(relation).into_value(),
            ),
            ("column_dict".to_string(), column_dict),
            ("config".to_string(), Value::from_dyn_object(config)),
            ("dbt_version".to_string(), Value::from("2.0.0")),
        ]);

        harness
            .render(
                "{{ snowflake__alter_column_comment(relation, column_dict) }}",
                ctx,
            )
            .expect("render should succeed")
    }

    /// From https://github.com/dbt-labs/dbt-core/issues/15742
    ///
    /// In a catalog-linked database (Iceberg REST / Glue / BigLake), `alter iceberg table
    /// <rel> alter <col> COMMENT ...` fails with `invalid identifier`. `COMMENT ON COLUMN`
    /// resolves the column there, so the catalog-linked path must use that form.
    #[test]
    fn catalog_linked_database_uses_comment_on_column() {
        let rendered = render_alter_column_comment(Catalog::LinkedV1, true);

        assert_eq!(
            squish(&rendered),
            format!(
                "comment on column MY_DB.MY_SCHEMA.MY_TABLE.\"{COLUMN}\" is $${DESCRIPTION}$$;"
            ),
            "catalog-linked column comments must use `comment on column`, got:\n{rendered}"
        );
    }

    /// Same relation under catalogs v2, where the linked database arrives as
    /// `catalog_database` rather than `adapter_properties.catalog_linked_database`.
    #[test]
    fn catalog_linked_database_v2_uses_comment_on_column() {
        let rendered = render_alter_column_comment(Catalog::LinkedV2, true);

        assert_eq!(
            squish(&rendered),
            format!(
                "comment on column MY_DB.MY_SCHEMA.MY_TABLE.\"{COLUMN}\" is $${DESCRIPTION}$$;"
            ),
            "catalog-linked column comments must use `comment on column` under catalogs v2, \
             got:\n{rendered}"
        );
    }

    fn render_is_catalog_linked_database(catalog: Catalog) -> String {
        let harness = build_harness(COLUMN);
        set_use_catalogs_v2(catalog.use_catalogs_v2());

        let ctx = BTreeMap::from([
            ("catalog_relation".to_string(), catalog.catalog_relation()),
            ("dbt_version".to_string(), Value::from("2.0.0")),
        ]);

        harness
            .render(
                "{{ snowflake__is_catalog_linked_database(catalog_relation=catalog_relation) }}",
                ctx,
            )
            .expect("render should succeed")
    }

    /// Guards every caller of this helper — truncate, drop, merge, insert_overwrite,
    /// incremental — not just `persist_docs`.
    #[test]
    fn is_catalog_linked_database_distinguishes_managed_from_linked() {
        assert_eq!(
            render_is_catalog_linked_database(Catalog::LinkedV1).trim(),
            "True",
            "the v1 catalog_linked_database signal must yield a boolean, not the database name"
        );
        assert_eq!(
            render_is_catalog_linked_database(Catalog::ManagedIcebergV2).trim(),
            "False",
            "a managed catalog that sets catalog_database is not catalog-linked"
        );
        assert_eq!(
            render_is_catalog_linked_database(Catalog::LinkedV2).trim(),
            "True",
            "a linked catalog that sets catalog_database is catalog-linked"
        );
    }

    /// Mirrors the reporter's setup: BigLake folds the column to `geographic_market`
    /// while the model documents `GEOGRAPHIC_MARKET`.
    #[test]
    fn catalog_linked_database_matches_documented_column_case_insensitively() {
        let column_dict = Value::from_serialize(BTreeMap::from([(
            COLUMN,
            BTreeMap::from([("description", DESCRIPTION)]),
        )]));
        let rendered = render(Catalog::LinkedV1, "geographic_market", column_dict);

        assert_eq!(
            squish(&rendered),
            format!(
                "comment on column MY_DB.MY_SCHEMA.MY_TABLE.\"geographic_market\" is $${DESCRIPTION}$$;"
            ),
            "a lowercased warehouse column must still pick up its documented description, \
             got:\n{rendered}"
        );
    }

    /// An undocumented column still gets an empty comment, clearing any stale description —
    /// the same semantics the batched ALTER form has always had.
    #[test]
    fn catalog_linked_database_blanks_undocumented_column() {
        let rendered = render_alter_column_comment(Catalog::LinkedV1, false);

        assert_eq!(
            squish(&rendered),
            format!("comment on column MY_DB.MY_SCHEMA.MY_TABLE.\"{COLUMN}\" is $$$$;"),
            "an undocumented column must be blanked, got:\n{rendered}"
        );
    }

    /// Snowflake-managed Iceberg rejects `comment on column` outright:
    /// "The table X is an Iceberg table. Iceberg tables should use ALTER ICEBERG TABLE
    /// commands." So managed Iceberg must keep the batched ALTER form.
    #[test]
    fn managed_iceberg_keeps_alter_iceberg_table() {
        let rendered = render_alter_column_comment(Catalog::ManagedIcebergV1, true);

        assert_eq!(
            squish(&rendered),
            format!(
                "alter iceberg table MY_DB.MY_SCHEMA.MY_TABLE alter \"{COLUMN}\" COMMENT $${DESCRIPTION}$$;"
            ),
            "managed iceberg column comments must keep the batched alter form, got:\n{rendered}"
        );
    }

    /// Routing this relation to `comment on column` would break a configuration that
    /// works today — the catalog type has to be consulted, not just `catalog_database`.
    #[test]
    fn managed_iceberg_with_catalog_database_keeps_alter_iceberg_table() {
        let rendered = render_alter_column_comment(Catalog::ManagedIcebergV2, true);

        assert_eq!(
            squish(&rendered),
            format!(
                "alter iceberg table MY_DB.MY_SCHEMA.MY_TABLE alter \"{COLUMN}\" COMMENT $${DESCRIPTION}$$;"
            ),
            "managed iceberg with catalog_database must keep the batched alter form, got:\n{rendered}"
        );
    }

    #[test]
    fn non_iceberg_keeps_batched_alter() {
        let rendered = render_alter_column_comment(Catalog::NonIceberg, true);

        assert_eq!(
            squish(&rendered),
            format!(
                "alter table MY_DB.MY_SCHEMA.MY_TABLE alter \"{COLUMN}\" COMMENT $${DESCRIPTION}$$;"
            ),
            "non-iceberg column comments must keep the batched alter form, got:\n{rendered}"
        );
    }
}
