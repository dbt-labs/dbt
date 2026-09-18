use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter_core::AdapterType;
use dbt_common::ErrorCode;
use dbt_jinja_utils::node_resolver::NodeResolver;
use dbt_jinja_utils::phases::compile::{
    DependencyValidationConfig, build_compile_node_context_inner,
};
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::{CommonAttributes, DbtModel, NodeBaseAttributes};
use dbt_schemas::state::{DbtRuntimeConfig, ModelStatus, NodeResolverTracker};
use minijinja::{Environment, Value};

use super::validate_runtime_sql_header;

#[test]
fn wap_rejects_sql_header_added_only_when_execute_is_true() {
    let mut model = DbtModel {
        __common_attr__: CommonAttributes {
            unique_id: "model.pkg.orders".to_string(),
            name: "orders".to_string(),
            package_name: "pkg".to_string(),
            language: Some("sql".to_string()),
            ..Default::default()
        },
        __base_attr__: NodeBaseAttributes {
            database: "DB".to_string(),
            schema: "SCHEMA".to_string(),
            alias: "orders".to_string(),
            materialized: DbtMaterialization::Table,
            ..Default::default()
        },
        ..Default::default()
    };
    model.deprecated_config.wap = Some(true);
    assert!(model.validate_wap_config(AdapterType::Snowflake).is_ok());
    let mut resolver = NodeResolver::default();
    resolver
        .insert_ref(&model, AdapterType::Snowflake, ModelStatus::Enabled, false)
        .unwrap();
    let resolver = Arc::new(resolver);
    let mut env = Environment::new();
    // Exercise the shipped macro and real CompileConfig::set mutation instead
    // of inserting a sql_header entry directly into the result map.
    env.add_template(
        "configs.sql",
        include_str!(
            "../../../dbt-loader/src/dbt_macro_assets/dbt-adapters/macros/materializations/configs.sql"
        ),
    )
    .unwrap();
    let template = "{% from 'configs.sql' import set_sql_header %}\
        {% if execute %}\
        {% call set_sql_header(config) %}set wap_bypass = true;{% endcall %}\
        {% endif %}select 1 as id";

    assert!(model.deprecated_config.sql_header.is_none());
    for execute in [false, true] {
        let base = BTreeMap::from([("execute".to_string(), Value::from(execute))]);
        let (context, config) = build_compile_node_context_inner(
            &model,
            AdapterType::Snowflake,
            &base,
            "pkg",
            resolver.clone(),
            Arc::new(DbtRuntimeConfig::default()),
            DependencyValidationConfig::new_validated(),
        )
        .unwrap();
        let sql = env.render_str(template, &context, &[]).unwrap();
        assert_eq!(sql, "select 1 as id");
        let header = config.get("sql_header").map(|entry| entry.value().clone());
        let result = validate_runtime_sql_header(header.as_ref());
        if execute {
            assert_eq!(header.unwrap().as_str(), Some("set wap_bypass = true;"));
            assert_eq!(result.unwrap_err().code, ErrorCode::InvalidConfig);
        } else {
            assert!(result.is_ok());
        }
    }
}

#[test]
fn wap_runtime_sql_header_allows_absent_or_empty_values() {
    assert!(validate_runtime_sql_header(None).is_ok());
    for header in [Value::from(()), Value::from(""), Value::from(" \n\t")] {
        assert!(validate_runtime_sql_header(Some(&header)).is_ok());
    }
    assert!(validate_runtime_sql_header(Some(&Value::from("select 1"))).is_err());
}
