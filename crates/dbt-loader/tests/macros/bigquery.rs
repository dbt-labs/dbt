use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::flags::Flags;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use dbt_schemas::dbt_types::RelationType;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

fn build_harness() -> MacroTestHarness {
    MacroTestHarness::for_adapter(AdapterType::Bigquery)
        .load_all_macros()
        .with_stub_functions()
        .build()
        .expect("harness should build")
}

fn base_relation_ctx(harness: &MacroTestHarness) -> BTreeMap<String, Value> {
    let relation = harness.relation(
        "test-db",
        "test_schema",
        "member_snapshot",
        Some(RelationType::Table),
    );
    BTreeMap::from([
        (
            "base_relation".to_string(),
            RelationObject::new(relation).into_value(),
        ),
        // `make_temp_relation` (dbt-adapters/macros/adapters/relation.sql) reads
        // `model.batch` before dispatching; a real dbt `model` always has this key
        // (None for non-microbatch models), so mirror that shape here.
        (
            "model".to_string(),
            Value::from_serialize(BTreeMap::from([("batch", Value::from(()))])),
        ),
    ])
}

#[test]
fn make_temp_relation_appends_unique_suffix() {
    // dbt Core's dbt-bigquery adapter appends a `strftime("%H%M%S%f")` suffix to
    // `__dbt_tmp` temp relations (via `bigquery__make_relation_with_suffix`) to avoid
    // collisions across concurrent runs. Fusion previously fell back to the generic
    // `default__make_temp_relation`, which reused the bare `__dbt_tmp` identifier and
    // caused a conformance SQL mismatch against dbt Core (#8247).
    let harness = build_harness();
    let ctx = base_relation_ctx(&harness);

    let rendered = harness
        .render("{{ make_temp_relation(base_relation).identifier }}", ctx)
        .expect("render should succeed");
    let rendered = rendered.trim();

    let suffix = rendered
        .strip_prefix("member_snapshot__dbt_tmp")
        .unwrap_or_else(|| panic!("expected __dbt_tmp-prefixed identifier, got: {rendered:?}"));
    assert!(
        !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()),
        "BigQuery temp relation should append an all-digit timestamp suffix, got: {rendered:?}"
    );
}

#[test]
fn make_intermediate_relation_does_not_append_suffix() {
    // Unlike `make_temp_relation`, the intermediate/backup relation paths
    // (`dstring=False`) must keep the bare suffix untouched.
    let harness = build_harness();
    let ctx = base_relation_ctx(&harness);

    let rendered = harness
        .render(
            "{{ make_intermediate_relation(base_relation).identifier }}",
            ctx,
        )
        .expect("render should succeed");

    assert_eq!(rendered.trim(), "member_snapshot__dbt_tmp");
}

// ---------------------------------------------------------------------------
// bigquery__generate_schema_name (LRC 4-part namespace)
// ---------------------------------------------------------------------------
const BQ_GENERATE_SCHEMA_NAME: &str = "{{ bigquery__generate_schema_name('staging', node) }}";
const DEFAULT__GENERATE_SCHEMA_NAME: &str = r#"
{% macro default__generate_schema_name(custom_schema_name, node) -%}
    custom_{{ custom_schema_name | trim }}
{%- endmacro %}
"#;

fn schema_name_harness(use_catalogs_v2: bool, override_default: bool) -> MacroTestHarness {
    let mut project_flags = BTreeMap::new();
    if use_catalogs_v2 {
        project_flags.insert("use_catalogs_v2".to_string(), Value::from(true));
    }

    let mut builder = MacroTestHarness::for_adapter(AdapterType::Bigquery)
        .load_all_macros()
        .with_stub_functions()
        .with_global(
            "target",
            Value::from_serialize(BTreeMap::from([("schema", "jaffle_shop")])),
        )
        .with_global(
            "flags",
            Value::from_object(Flags::from_project_flags(project_flags)),
        );

    if override_default {
        builder = builder.with_macro(
            "test_project",
            "default__generate_schema_name",
            DEFAULT__GENERATE_SCHEMA_NAME,
        );
    }

    builder.build().expect("harness should build")
}

fn schema_name_ctx(catalog_name: Option<&'static str>) -> BTreeMap<String, Value> {
    let config = Arc::new(MockJinjaObject::new());
    config.on("get", move |args| {
        Ok(match args.first().and_then(|v| v.as_str()) {
            Some("catalog_name") => catalog_name.map(Value::from).unwrap_or(Value::UNDEFINED),
            _ => Value::UNDEFINED,
        })
    });

    let node = Arc::new(MockJinjaObject::new());
    node.set_attr("config", Value::from_dyn_object(config));

    BTreeMap::from([
        (
            "TARGET_PACKAGE_NAME".to_string(),
            Value::from("test_project"),
        ),
        ("node".to_string(), Value::from_dyn_object(node)),
    ])
}

fn with_lakehouse_catalog(harness: &MacroTestHarness, lakehouse_catalog: &'static str) {
    harness.mock().on("build_catalog_relation", move |_| {
        Ok(Value::from_serialize(BTreeMap::from([(
            "lakehouse_catalog",
            lakehouse_catalog,
        )])))
    });
}

#[test]
fn generate_schema_name_without_catalogs_v2_matches_default() {
    let harness = schema_name_harness(false, false);
    let bigquery = harness
        .render(BQ_GENERATE_SCHEMA_NAME, schema_name_ctx(None))
        .expect("render should succeed");
    let default = harness
        .render(
            "{{ default__generate_schema_name('staging', node) }}",
            schema_name_ctx(None),
        )
        .expect("render should succeed");

    assert_eq!(bigquery.trim(), default.trim());
    assert_eq!(bigquery.trim(), "jaffle_shop_staging");
}

#[test]
fn generate_schema_name_with_catalogs_v2_prefixes_lakehouse_catalog() {
    let harness = schema_name_harness(true, false);
    with_lakehouse_catalog(&harness, "sales_catalog");

    let rendered = harness
        .render(BQ_GENERATE_SCHEMA_NAME, schema_name_ctx(Some("BQ")))
        .expect("render should succeed");

    assert_eq!(rendered.trim(), "sales_catalog.jaffle_shop_staging");
}

#[test]
fn generate_schema_name_with_catalogs_v2_but_no_lrc_matches_default() {
    let harness = schema_name_harness(true, false);

    let rendered = harness
        .render(BQ_GENERATE_SCHEMA_NAME, schema_name_ctx(None))
        .expect("render should succeed");

    assert_eq!(rendered.trim(), "jaffle_shop_staging");
}

#[test]
fn generate_schema_name_composes_on_a_projects_own_default() {
    let off = schema_name_harness(false, true);
    assert_eq!(
        off.render(BQ_GENERATE_SCHEMA_NAME, schema_name_ctx(None))
            .expect("render should succeed")
            .trim(),
        "custom_staging"
    );

    let on = schema_name_harness(true, true);
    with_lakehouse_catalog(&on, "sales_catalog");
    assert_eq!(
        on.render(BQ_GENERATE_SCHEMA_NAME, schema_name_ctx(Some("BQ")))
            .expect("render should succeed")
            .trim(),
        "sales_catalog.custom_staging"
    );
}
