use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::node_resolver::NodeResolver;
use dbt_jinja_utils::phases::compile::{
    DependencyValidationConfig, build_compile_node_context_inner,
};
use dbt_schemas::state::{DbtRuntimeConfig, ModelStatus, NodeResolverTracker};
use minijinja::{Environment, Value};

use super::TableLifecycle;
use super::clone_tests::fixture;
use super::stage_tests::{StageConfig, stage_with_config, submitted_sql};

#[test]
fn wap_executes_runtime_sql_header_with_candidate_this_before_ctas() {
    let (_directory, _io, wap) = fixture(Some(false));
    let model = wap.execution_model().unwrap();
    let mut resolver = NodeResolver::default();
    resolver
        .insert_ref(
            wap.model.as_ref(),
            AdapterType::Snowflake,
            ModelStatus::Enabled,
            false,
        )
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
        {% call set_sql_header(config) %}update {{ this }} set id = id + 1;{% endcall %}\
        {% endif %}select 1 as id";

    assert!(model.deprecated_config.sql_header.is_none());
    for execute in [false, true] {
        let base = BTreeMap::from([("execute".to_string(), Value::from(execute))]);
        let (mut context, config) = build_compile_node_context_inner(
            &model,
            AdapterType::Snowflake,
            &base,
            "wap_test",
            resolver.clone(),
            Arc::new(DbtRuntimeConfig::default()),
            DependencyValidationConfig::new_validated(),
        )
        .unwrap();
        // TaskRunnerCtx applies this invocation-local override after building
        // the compile context; the shared resolver keeps its public identity.
        context.insert(
            "this".to_owned(),
            RelationObject::new(wap.candidate_relation().unwrap().into()).into_value(),
        );
        let sql = env.render_str(template, &context, &[]).unwrap();
        assert_eq!(sql, "select 1 as id");
        let header = config.get("sql_header").map(|entry| entry.value().clone());
        let expected_header =
            "update \"My\"\"Database\".\"My.Schema\".\"__DBT_WAP_TEST\" set id = id + 1;";
        if execute {
            assert_eq!(header.as_ref().unwrap().as_str(), Some(expected_header));
        } else {
            assert!(header.as_ref().is_none_or(Value::is_none));
        }
        let (result, adapter) = stage_with_config(
            true,
            Some(TableLifecycle::Permanent),
            &sql,
            StageConfig {
                runtime_sql_header: header,
                ..Default::default()
            },
        );
        result.unwrap();
        let submitted = submitted_sql(&adapter);
        assert_eq!(submitted.len(), 2);
        let expected_ctas = "create or replace transient table \
            \"My\"\"Database\".\"My.Schema\".\"__DBT_WAP_TEST\" as (select 1 as id ) ;";
        assert_eq!(
            submitted[1],
            if execute {
                format!("{expected_header} {expected_ctas}")
            } else {
                expected_ctas.to_owned()
            }
        );
    }
    assert!(wap.model.deprecated_config.sql_header.is_none());
    assert_eq!(wap.model.__base_attr__.alias, "Orders");
}

#[test]
fn wap_runtime_sql_header_overrides_the_parsed_header() {
    let (result, adapter) = stage_with_config(
        true,
        None,
        "select 1 as id",
        StageConfig {
            sql_header: Some("set wap_parsed_header = 1;".to_owned()),
            runtime_sql_header: Some(Value::from("set wap_runtime_header = 2;")),
            ..Default::default()
        },
    );
    result.unwrap();
    let submitted = submitted_sql(&adapter);
    assert!(submitted[1].starts_with("set wap_runtime_header = 2; create or replace"));
    assert!(!submitted[1].contains("wap_parsed_header"));
}

#[test]
fn wap_runtime_sql_header_allows_absent_or_empty_values() {
    for header in [
        None,
        Some(Value::from(())),
        Some(Value::from("")),
        Some(Value::from(" \n\t")),
    ] {
        let (result, adapter) = stage_with_config(
            true,
            None,
            "select 1 as id",
            StageConfig {
                runtime_sql_header: header,
                ..Default::default()
            },
        );
        result.unwrap();
        assert!(submitted_sql(&adapter)[1].starts_with("create or replace transient table"));
    }
}
