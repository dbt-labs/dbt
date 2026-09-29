use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter_core::AdapterType;
use dbt_jinja_utils::mock_object::MockJinjaObject;
use minijinja::Value;

use crate::macro_test_harness::MacroTestHarness;

fn render_writer_options(clustered_by: Value, buckets: Option<i64>) -> String {
    let python_macros =
        include_str!("../../src/dbt_macro_assets/dbt-databricks/macros/adapters/python.sql");
    let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .with_macro_at_path(
            "dbt_databricks",
            "py_get_writer_options",
            python_macros,
            "dbt_macro_assets/dbt-databricks/macros/adapters/python.sql",
        )
        .with_macro(
            "dbt",
            "is_incremental",
            "{% macro is_incremental() %}{{ return(false) }}{% endmacro %}",
        )
        .build()
        .expect("Python macro harness should build");

    harness
        .mock()
        .on("resolve_file_format", |_| Ok(Value::from("delta")));

    let config = Arc::new(MockJinjaObject::new());
    config.on("get", move |args| {
        let key = args.first().and_then(Value::as_str);
        Ok(match key {
            Some("clustered_by") => clustered_by.clone(),
            Some("buckets") => buckets.map(Value::from).unwrap_or_else(|| Value::from(())),
            _ => Value::from(()),
        })
    });
    config.set_attr(
        "model",
        Value::from_serialize(BTreeMap::<String, Value>::new()),
    );

    harness
        .render(
            "{{ py_get_writer_options() }}",
            BTreeMap::from([
                ("config".to_string(), Value::from_dyn_object(config)),
                (
                    "model".to_string(),
                    Value::from_serialize(BTreeMap::<String, Value>::new()),
                ),
            ]),
        )
        .expect("writer options should render")
}

#[test]
fn clustered_by_scalar_renders_one_bucket_column() {
    let rendered = render_writer_options(Value::from("name"), Some(2));

    assert_eq!(
        rendered.trim(),
        ".format(\"delta\")\n.bucketBy(2, ['name'])"
    );
}

#[test]
fn clustered_by_array_preserves_bucket_column_order() {
    let rendered = render_writer_options(
        Value::from_serialize(vec!["name".to_string(), "date".to_string()]),
        Some(2),
    );

    assert_eq!(
        rendered.trim(),
        ".format(\"delta\")\n.bucketBy(2, ['name', 'date'])"
    );
}

#[test]
fn clustered_by_without_buckets_omits_bucket_by() {
    let rendered = render_writer_options(
        Value::from_serialize(vec!["name".to_string(), "date".to_string()]),
        None,
    );

    assert_eq!(rendered.trim(), ".format(\"delta\")");
}
