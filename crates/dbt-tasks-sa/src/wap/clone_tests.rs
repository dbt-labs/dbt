//! Offline rendering checks for the actual Snowflake macro and runtime config.
//! Warehouse atomicity, privileges, and storage behavior require live acceptance tests.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_common::io_args::IoArgs;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::phases::run::{RunConfig, build_run_node_context};
use dbt_schemas::schemas::DbtModel;
use dbt_schemas::schemas::common::{DbtMaterialization, ResolvedQuoting};
use dbt_tasks_core::wap::WapModel;
use dbt_telemetry::ExecutionPhase;
use minijinja::{Environment, Value};

const CLONE_MACRO: &str = include_str!(
    "../../../dbt-loader/src/dbt_macro_assets/dbt-snowflake/macros/materializations/clone.sql"
);

fn fixture(copy_grants: Option<bool>) -> (tempfile::TempDir, IoArgs, WapModel) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("models")).unwrap();
    std::fs::write(directory.path().join("models/orders.sql"), "select 1 as id").unwrap();
    let io = IoArgs {
        in_dir: directory.path().to_path_buf(),
        out_dir: directory.path().join("target"),
        ..Default::default()
    };
    let mut model = DbtModel::default();
    model.__common_attr__.unique_id = "model.wap_test.orders".to_owned();
    model.__common_attr__.name = "orders".to_owned();
    model.__common_attr__.package_name = "wap_test".to_owned();
    model.__common_attr__.language = Some("sql".to_owned());
    model.__common_attr__.path = "orders.sql".into();
    model.__common_attr__.original_file_path = "models/orders.sql".into();
    model.__base_attr__.adapter = AdapterType::Snowflake;
    model.__base_attr__.materialized = DbtMaterialization::Table;
    model.__base_attr__.database = "My\"Database".to_owned();
    model.__base_attr__.schema = "My.Schema".to_owned();
    model.__base_attr__.alias = "Orders".to_owned();
    model.__base_attr__.quoting = ResolvedQuoting::trues();
    model.deprecated_config.wap = Some(true);
    model.deprecated_config.materialized = Some(DbtMaterialization::Table);
    model.deprecated_config.alias = Some("Orders".to_owned());
    model
        .deprecated_config
        .__warehouse_specific_config__
        .copy_grants = copy_grants;
    model.deprecated_config.grants =
        serde_json::from_value(serde_json::json!({"select": ["READER"]})).unwrap();
    let wap = WapModel {
        model: Arc::new(model),
        candidate_identifier: "__DBT_WAP_TEST".to_owned(),
        audit_ids: BTreeSet::new(),
    };
    (directory, io, wap)
}

fn runtime_context(model: &DbtModel, io: &IoArgs) -> BTreeMap<String, Value> {
    let (context, _) = build_run_node_context(
        model,
        &model.deprecated_config,
        AdapterType::Snowflake,
        None,
        &BTreeMap::new(),
        io,
        ExecutionPhase::Run,
        None,
        BTreeSet::new(),
    );
    assert!(
        context["config"]
            .downcast_object_ref::<RunConfig>()
            .is_some()
    );
    context
}

fn render_clone(wap: &WapModel, io: &IoArgs, transient: bool, target_exists: bool) -> String {
    let model = super::publication_model(&wap.model, transient, target_exists);
    let mut context = runtime_context(&model, io);
    context.insert(
        "__wap_candidate".to_owned(),
        RelationObject::new(wap.candidate_relation().unwrap().into()).into_value(),
    );
    let template = format!(
        "{CLONE_MACRO}\n{{{{ snowflake__create_or_replace_clone(this, __wap_candidate) }}}}"
    );
    let rendered = JinjaEnv::new(Environment::new())
        .render_str(&template, &context, &[])
        .unwrap();
    rendered.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn actual_clone_macro_uses_resolved_lifecycle_and_public_destination() {
    let (_directory, io, mut wap) = fixture(Some(false));
    for transient in [false, true] {
        // The catalog's resolved lifecycle must win over the original model option.
        Arc::make_mut(&mut wap.model)
            .deprecated_config
            .__warehouse_specific_config__
            .transient = Some(!transient);
        let rendered = render_clone(&wap, &io, transient, true);
        let lifecycle = if transient { "transient " } else { "" };
        assert_eq!(
            rendered,
            format!(
                "create or replace {lifecycle}table \"My\"\"Database\".\"My.Schema\".\"Orders\" \
                 clone \"My\"\"Database\".\"My.Schema\".\"__DBT_WAP_TEST\""
            )
        );
        assert_eq!(wap.model.__base_attr__.alias, "Orders");
        assert_eq!(
            wap.model
                .deprecated_config
                .__warehouse_specific_config__
                .transient,
            Some(!transient),
            "publication must leave the canonical model unchanged"
        );
    }
}

#[test]
fn actual_clone_macro_copies_grants_only_for_configured_replacements() {
    for configured in [None, Some(false), Some(true)] {
        let (_directory, io, wap) = fixture(configured);
        for target_exists in [false, true] {
            let rendered = render_clone(&wap, &io, true, target_exists);
            assert_eq!(
                rendered.ends_with(" copy grants"),
                target_exists && configured == Some(true),
                "copy_grants={configured:?}, target_exists={target_exists}: {rendered}"
            );
            assert!(!rendered.contains("copy tags"));
            assert_eq!(
                wap.model
                    .deprecated_config
                    .__warehouse_specific_config__
                    .copy_grants,
                configured,
                "publication must leave the canonical model unchanged"
            );
        }
    }
}

#[test]
fn candidate_runtime_config_suppresses_public_grants_and_keeps_candidate_identity() {
    let (_directory, io, wap) = fixture(Some(true));
    let candidate = wap.execution_model().unwrap();
    let context = runtime_context(&candidate, &io);
    let canonical_context = runtime_context(&wap.model, &io);
    let environment = Environment::new();
    for expression in ["config.get('grants')", "config.get('copy_grants')"] {
        let expression = environment.compile_expression(expression).unwrap();
        assert!(!expression.eval(&context, &[]).unwrap().is_true());
        assert!(expression.eval(&canonical_context, &[]).unwrap().is_true());
    }
    assert_eq!(
        environment
            .compile_expression("this")
            .unwrap()
            .eval(&context, &[])
            .unwrap()
            .to_string(),
        "\"My\"\"Database\".\"My.Schema\".\"__DBT_WAP_TEST\""
    );
    assert_eq!(
        environment
            .compile_expression("model.alias")
            .unwrap()
            .eval(&context, &[])
            .unwrap()
            .as_str(),
        Some("__DBT_WAP_TEST")
    );
}
