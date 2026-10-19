use std::{collections::BTreeMap, sync::Arc};

use dbt_schemas::{
    jinja_node::LazyNodeContext,
    schemas::{DbtModel, serialization_utils::SerializationMode},
};
use minijinja::{Environment, Value, value::Object};

fn context() -> LazyNodeContext {
    let mut model = DbtModel::default();
    model.__common_attr__.name = "test_model".into();
    model.__common_attr__.description = Some("original".into());
    LazyNodeContext::from_node(Arc::new(model), SerializationMode::OmitNone)
}

#[test]
fn lazy_model_reads_compiled_sql_fresh_and_honors_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compiled.sql");
    let wrapper = Arc::new(context().with_compiled_path(path.clone()));
    assert!(wrapper.get_value(&Value::from("compiled_code")).is_none());
    std::fs::write(&path, "select 1").unwrap();
    assert_eq!(
        wrapper.get_value(&Value::from("compiled_code")),
        Some(Value::from("select 1"))
    );
    std::fs::write(&path, "select 2").unwrap();
    assert_eq!(
        wrapper.get_value(&Value::from("compiled_sql")),
        Some(Value::from("select 2"))
    );
    wrapper.insert(Value::from("compiled_code"), Value::from("batch sql"));
    assert_eq!(
        wrapper.get_value(&Value::from("compiled_code")),
        Some(Value::from("batch sql"))
    );
    assert_eq!(
        wrapper.get_value(&Value::from("compiled_sql")),
        Some(Value::from("select 2"))
    );
}

#[test]
fn lazy_model_disk_fallback_does_not_add_dict_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compiled.sql");
    std::fs::write(&path, "select 1").unwrap();
    let model = Value::from_object(context().with_compiled_path(path));
    let env = Environment::new();
    assert_eq!(env.render_str(
        "{{ model.pop('compiled_code', 'missing') }}|{{ model.compiled_code }}|{{ model.setdefault('compiled_code', 'stored') }}|{{ model.compiled_code }}|{{ model.pop('compiled_code') }}|{{ model.compiled_code }}|{% do model.clear() %}{{ model.compiled_code }}|{{ model.keys() | list }}",
        BTreeMap::from([("model", model)]), &[]).unwrap(),
        "missing|select 1|stored|select 1|stored|select 1|select 1|[]");
}

#[test]
fn lazy_model_ignores_jinja_compiled_sql_writes_like_eager_contexts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("compiled.sql");
    std::fs::write(&path, "select 1").unwrap();
    let fields = Arc::new(context().with_compiled_path(path));
    let model = Value::from_dyn_object(fields.clone());
    let env = Environment::new();
    let template = "{% do model.update({'compiled_code': 'select 2'}) %}{{ model.compiled_code }}|{{ model['compiled_code'] }}|{{ model.get('compiled_code') }}|{{ model.copy().compiled_code }}";
    assert_eq!(
        env.render_str(template, BTreeMap::from([("model", model.clone())]), &[])
            .unwrap(),
        "select 1|select 1|select 1|select 2"
    );
    fields.insert(Value::from("compiled_code"), Value::from("batch sql"));
    assert_eq!(
        env.render_str(
            "{{ model.compiled_code }}",
            BTreeMap::from([("model", model)]),
            &[]
        )
        .unwrap(),
        "batch sql"
    );
}

#[test]
fn lazy_model_preserves_metadata_and_query_identity() {
    let mut model = DbtModel::default();
    model.__common_attr__.unique_id = "model.pkg.test".into();
    model.__common_attr__.name = "test".into();
    model.__common_attr__.package_name = "pkg".into();
    model.__common_attr__.original_file_path = "models/test.sql".into();
    let fields = LazyNodeContext::from_node(Arc::new(model), SerializationMode::OmitNone);
    for value in [
        Value::from_object(fields.alias()),
        Value::from_object(fields),
    ] {
        let env = Environment::new();
        let template = env.template_from_str("").unwrap();
        let state = template
            .eval_to_state(BTreeMap::from([("model", value)]), &[])
            .unwrap();
        assert_eq!(
            crate::utils::node_metadata_from_state(&state),
            Some(("model.pkg.test".into(), "models/test.sql".into()))
        );
        let query = dbt_adapter::query_ctx::query_ctx_from_state(&state).unwrap();
        assert_eq!(query.node_id().map(String::as_str), Some("model.pkg.test"));
        assert_eq!(query.project_name().map(String::as_str), Some("pkg"));
        assert_eq!(query.model_name().map(String::as_str), Some("test"));
    }
}
