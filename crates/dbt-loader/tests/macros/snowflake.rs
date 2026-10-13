use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, default_mock_config};

#[test]
fn snapshot_check_cols_keeps_exact_case_matching() {
    let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let rendered = harness
        .render(
            "{{ 'found' if adapter.dispatch('snapshot_check_column_exists', 'dbt')('dice_change_hash', ['DICE_CHANGE_HASH']) else 'missing' }}|{{ 'found' if adapter.dispatch('snapshot_check_column_exists', 'dbt')('DICE_CHANGE_HASH', ['DICE_CHANGE_HASH']) else 'missing' }}",
            BTreeMap::<String, Value>::new(),
        )
        .expect("snapshot column check should render");

    assert_eq!(rendered.trim(), "missing|found");
}

#[test]
fn python_table_tmp_relation_type_is_allowed() {
    let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let config = default_mock_config();
    config.on("get", |args| {
        let key = args.first().and_then(|v| v.as_str());
        let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
        match key {
            Some("tmp_relation_type") => Ok(Value::from("table")),
            _ => Ok(default),
        }
    });

    let ctx = BTreeMap::from([("config".to_string(), Value::from_dyn_object(config))]);

    let rendered = harness
        .render(
            "{{ dbt_snowflake_get_tmp_relation_type('default', none, 'python') }}",
            ctx,
        )
        .expect("table is valid for Python tmp_relation_type");

    assert_eq!(rendered.trim(), "table");
}

fn render_python_table(temporary: bool, is_transient: bool) -> String {
    let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let catalog_relation = Arc::new(MockJinjaObject::new());
    catalog_relation.set_attr("catalog_type", Value::from("INFO_SCHEMA"));
    catalog_relation.set_attr("is_transient", Value::from(is_transient));
    harness.mock().on("build_catalog_relation", move |_| {
        Ok(Value::from_dyn_object(catalog_relation.clone()))
    });

    let ctx = harness
        .materialization_context(
            "orders",
            "def model(dbt, session):\n    return session.table('orders')",
        )
        .with("temporary", Value::from(temporary))
        .build();

    harness
        .render(
            "{{ snowflake__create_table_as(temporary, this, compiled_code, 'python') }}",
            ctx,
        )
        .expect("Python table macro should render")
}

#[test]
fn python_incremental_staging_table_is_temporary() {
    let rendered = render_python_table(true, true);

    assert!(
        rendered.contains("table_type='temporary'"),
        "Python incremental staging tables should be temporary, got:\n{rendered}"
    );
    assert!(!rendered.contains("table_type='transient'"));
}

#[test]
fn python_table_preserves_transient_config() {
    let rendered = render_python_table(false, true);

    assert!(
        rendered.contains("table_type='transient'"),
        "Python table models should preserve transient configuration, got:\n{rendered}"
    );
    assert!(!rendered.contains("table_type='temporary'"));
}

/// Render `snowflake__create_csv_table` for a seed whose catalog relation
/// reports `is_transient`, mirroring `render_python_table`.
fn render_seed_create_csv_table(is_transient: bool) -> String {
    let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .load_all_macros()
        // `create_csv_table` wraps its DDL in `statement('_')`, which calls
        // `store_result`; register the standard stubs so the render succeeds.
        .with_stub_functions()
        .build()
        .expect("harness should build");

    let catalog_relation = Arc::new(MockJinjaObject::new());
    catalog_relation.set_attr("catalog_type", Value::from("INFO_SCHEMA"));
    catalog_relation.set_attr("is_transient", Value::from(is_transient));
    harness.mock().on("build_catalog_relation", move |_| {
        Ok(Value::from_dyn_object(catalog_relation.clone()))
    });

    // Stub the adapter column typing/quoting the DDL body calls back into.
    harness
        .mock()
        .on("convert_type", |_| Ok(Value::from("integer")));
    harness.mock().on("quote_seed_column", |args| {
        Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
    });

    // Minimal agate_table stand-in: a single `id` column, no rows.
    let agate_table = Arc::new(MockJinjaObject::new());
    agate_table.set_attr("column_names", Value::from(vec![Value::from("id")]));

    let ctx = harness
        .materialization_context("my_seed", "")
        .relation_type(dbt_schemas::dbt_types::RelationType::Table)
        .with("agate_table", Value::from_dyn_object(agate_table))
        .build();

    harness
        .render("{{ snowflake__create_csv_table(model, agate_table) }}", ctx)
        .expect("snowflake__create_csv_table should render")
}

/// CORE-903: a default-format seed is created TRANSIENT (is_transient defaults
/// to true), matching table behavior. Pre-fix it emitted a plain CREATE TABLE.
#[test]
fn seed_create_csv_table_is_transient_by_default() {
    let rendered = render_seed_create_csv_table(true);
    let lower = rendered.to_lowercase();

    assert!(
        lower.contains("create transient table"),
        "seed DDL should be TRANSIENT when the catalog relation is transient, got:\n{rendered}"
    );
}

/// CORE-903: an explicit `transient: false` seed is a plain permanent table.
#[test]
fn seed_create_csv_table_honors_transient_false() {
    let rendered = render_seed_create_csv_table(false);
    let lower = rendered.to_lowercase();

    assert!(
        lower.contains("create table"),
        "seed DDL should be a plain CREATE TABLE when not transient, got:\n{rendered}"
    );
    assert!(
        !lower.contains("transient"),
        "explicit transient: false must not emit the transient keyword, got:\n{rendered}"
    );
}

#[test]
fn alter_relation_comment_uses_iceberg_syntax_after_incorporate() {
    use dbt_adapter::relation::{Relation, RelationObject};
    use dbt_schemas::dbt_types::RelationType;
    use dbt_schemas::schemas::relations::base::{BaseRelation, TableFormat};

    let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
        .load_all_macros()
        .build()
        .expect("harness should build");

    let iceberg: Arc<dyn BaseRelation> = Arc::new(
        Relation::new(
            AdapterType::Snowflake,
            "TEST_DB".to_string(),
            "TEST_SCHEMA".to_string(),
            "my_iceberg_table".to_string(),
        )
        .with_relation_type(Some(RelationType::Table))
        .with_table_format(TableFormat::Iceberg),
    );

    // Mirrors `target_relation.incorporate(type='table')` in the incremental
    // materialization, called right before `persist_docs` (fs#14268).
    let incorporated = iceberg
        .incorporate(None, Some(RelationType::Table), None)
        .expect("incorporate should succeed");

    let ctx = BTreeMap::from([
        (
            "relation".to_string(),
            RelationObject::new(incorporated).into_value(),
        ),
        ("relation_comment".to_string(), Value::from("a comment")),
    ]);

    let rendered = harness
        .render(
            "{{ snowflake__alter_relation_comment(relation, relation_comment) }}",
            ctx,
        )
        .expect("alter_relation_comment should render");

    assert!(
        rendered.contains("alter iceberg table"),
        "expected iceberg ALTER syntax after incorporate, got:\n{rendered}"
    );
    assert!(
        !rendered.contains("comment on table"),
        "must not fall back to plain COMMENT ON TABLE for an Iceberg relation, got:\n{rendered}"
    );
}

mod cld_base_location {
    use super::*;

    const V2_BASE_LOCATION: &str = "s3://bucket/warehouse/TEST_SCHEMA/orders";
    const V1_BASE_LOCATION: &str = "_dbt/TEST_SCHEMA/orders";

    #[derive(Clone, Copy)]
    enum CldMacro {
        Rest,
        Glue,
    }

    struct Runtime {
        dbt_version: &'static str,
        use_catalogs_v2: Option<bool>,
    }

    const V2: Runtime = Runtime {
        dbt_version: "2.0.0",
        use_catalogs_v2: Some(true),
    };
    const FUSION_V1: Runtime = Runtime {
        dbt_version: "2.0.0",
        use_catalogs_v2: Some(false),
    };
    const CORE_1X: Runtime = Runtime {
        dbt_version: "1.10.0",
        use_catalogs_v2: None,
    };

    fn catalog_relation(
        catalog_database: Value,
        catalog_linked_database: Value,
        external_volume: Value,
        base_location: Value,
    ) -> Value {
        let linked_catalog_provider = Arc::new(MockJinjaObject::new());
        linked_catalog_provider.set_attr("is_glue", Value::from(false));

        let is_cld = !catalog_linked_database.is_none() || !catalog_database.is_undefined();
        let relation = Arc::new(MockJinjaObject::new());
        relation.set_attr(
            "linked_catalog_provider",
            Value::from_dyn_object(linked_catalog_provider),
        );
        relation.on("has_catalog_linked_database", move |_| {
            Ok(Value::from(is_cld))
        });
        relation.set_attr("catalog_name", Value::from("REST"));
        relation.set_attr("catalog_database", catalog_database);
        relation.set_attr("catalog_linked_database", catalog_linked_database);
        relation.set_attr(
            "catalog_linked_database_type",
            if is_cld {
                Value::from("unity")
            } else {
                Value::from(())
            },
        );
        relation.set_attr("external_volume", external_volume);
        relation.set_attr("base_location", base_location);
        for attr in [
            "iceberg_version",
            "target_file_size",
            "auto_refresh",
            "max_data_extension_time_in_days",
        ] {
            relation.set_attr(attr, Value::from(()));
        }
        Value::from_dyn_object(relation)
    }

    fn v2_cld(base_location: Value) -> Value {
        catalog_relation(
            Value::from("MY_CLD"),
            Value::from(()),
            Value::from(()),
            base_location,
        )
    }

    fn fusion_v1_cld() -> Value {
        catalog_relation(
            Value::UNDEFINED,
            Value::from("MY_CLD"),
            Value::from(()),
            Value::from(V1_BASE_LOCATION),
        )
    }

    fn render(cld_macro: CldMacro, runtime: Runtime, catalog_relation: Value) -> String {
        let harness = MacroTestHarness::for_adapter(AdapterType::Snowflake)
            .load_all_macros()
            .with_macro(
                "test_project",
                "make_glue_compatible_relation",
                "{% macro make_glue_compatible_relation(relation) %}{{ return(relation) }}{% endmacro %}",
            )
            .with_macro(
                "test_project",
                "get_partition_by_keys",
                "{% macro get_partition_by_keys(config) %}{{ return([]) }}{% endmacro %}",
            )
            .build()
            .expect("harness should build");
        if let Some(use_catalogs_v2) = runtime.use_catalogs_v2 {
            harness.mock().set_attr(
                "behavior",
                Value::from_serialize(BTreeMap::from([(
                    "use_catalogs_v2",
                    BTreeMap::from([("no_warn", use_catalogs_v2)]),
                )])),
            );
        }
        harness.mock().on("get_relation", |_| Ok(Value::from(())));
        harness.mock().on("get_column_schema_from_query", |_| {
            Ok(Value::from_serialize(vec![BTreeMap::from([
                ("name", "ID"),
                ("data_type", "NUMBER"),
            ])]))
        });
        harness.mock().on("quote", |args| {
            Ok(Value::from(format!(
                "\"{}\"",
                args.first().and_then(Value::as_str).unwrap_or_default()
            )))
        });
        let relation = catalog_relation.clone();
        harness
            .mock()
            .on("build_catalog_relation", move |_| Ok(relation.clone()));

        let ctx = harness
            .materialization_context("orders", "select 1 as id")
            .with("dbt_version", Value::from(runtime.dbt_version))
            .with("catalog_relation", catalog_relation)
            .build();
        let template = match cld_macro {
            CldMacro::Rest => "{{ snowflake__create_table_iceberg_rest_sql(this, compiled_code) }}",
            CldMacro::Glue => {
                "{{ snowflake__create_table_iceberg_rest_with_glue(this, compiled_code, catalog_relation) }}"
            }
        };
        harness
            .render(template, ctx)
            .expect("CLD macro should render")
    }

    #[test]
    fn v2_cld_renders_configured_base_location() {
        let clause = format!("base_location = '{V2_BASE_LOCATION}'");
        for cld_macro in [CldMacro::Rest, CldMacro::Glue] {
            let rendered = render(cld_macro, V2, v2_cld(Value::from(V2_BASE_LOCATION)));
            assert!(
                rendered.lines().any(|line| line.trim() == clause),
                "expected BASE_LOCATION on its own line in v2 CLD DDL, got:\n{rendered}"
            );
            assert!(
                !rendered.contains("external_volume") && !rendered.contains("catalog ="),
                "a CLD must never emit EXTERNAL_VOLUME or CATALOG, got:\n{rendered}"
            );
        }
    }

    #[test]
    fn v2_cld_without_base_location_omits_clause() {
        for cld_macro in [CldMacro::Rest, CldMacro::Glue] {
            let rendered = render(cld_macro, V2, v2_cld(Value::from(())));
            assert!(
                !rendered.contains("base_location"),
                "unexpected DDL:\n{rendered}"
            );
        }
    }

    #[test]
    fn fusion_v1_cld_does_not_render_synthesized_base_location() {
        for cld_macro in [CldMacro::Rest, CldMacro::Glue] {
            let rendered = render(cld_macro, FUSION_V1, fusion_v1_cld());
            assert!(
                !rendered.contains("base_location"),
                "a v1 CLD must keep its existing DDL, got:\n{rendered}"
            );
        }
    }

    #[test]
    fn core_1x_cld_does_not_read_base_location() {
        let core_cld = catalog_relation(
            Value::UNDEFINED,
            Value::from("MY_CLD"),
            Value::from(()),
            Value::UNDEFINED,
        );
        for cld_macro in [CldMacro::Rest, CldMacro::Glue] {
            let rendered = render(cld_macro, CORE_1X, core_cld.clone());
            assert!(
                !rendered.contains("base_location")
                    && !rendered.to_lowercase().contains("undefined"),
                "unexpected DDL:\n{rendered}"
            );
        }
    }

    #[test]
    fn v1_rest_catalog_without_cld_keeps_existing_clauses() {
        let rest = catalog_relation(
            Value::UNDEFINED,
            Value::from(()),
            Value::from("MY_VOLUME"),
            Value::from(V1_BASE_LOCATION),
        );
        let rendered = render(CldMacro::Rest, FUSION_V1, rest);
        for clause in [
            "external_volume = 'MY_VOLUME'".to_string(),
            format!("base_location = '{V1_BASE_LOCATION}'"),
        ] {
            assert!(
                rendered.lines().any(|line| line.trim() == clause),
                "expected `{clause}` on its own line, got:\n{rendered}"
            );
        }
        assert!(
            rendered.contains("catalog = 'REST'"),
            "expected `catalog = 'REST'`, got:\n{rendered}"
        );
        assert_eq!(
            rendered.matches("base_location").count(),
            1,
            "BASE_LOCATION must render exactly once, got:\n{rendered}"
        );
    }
}
