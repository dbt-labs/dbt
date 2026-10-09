use std::collections::BTreeMap;

use dbt_adapter_core::AdapterType;
use minijinja::Value;

use crate::macro_test_harness::{MacroTestHarness, default_mock_config};

fn resolve(project_flag: bool, model_setting: Option<Value>) -> String {
    let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .load_all_macros()
        .with_stub_functions()
        .with_behavior_flag("use_materialization_v2", project_flag)
        .build()
        .expect("harness should build");

    let config = default_mock_config();
    config.on("get", move |args| {
        let key = args.first().and_then(Value::as_str);
        let default = args.get(1).cloned().unwrap_or(Value::UNDEFINED);
        match (key, &model_setting) {
            (Some("use_materialization_v2"), Some(value)) => Ok(value.clone()),
            _ => Ok(default),
        }
    });

    harness
        .render(
            "{{ use_materialization_v2() | tojson }}",
            BTreeMap::from([("config".to_string(), Value::from_dyn_object(config))]),
        )
        .expect("resolver should render")
        .trim()
        .to_string()
}

#[test]
fn unset_model_setting_falls_back_to_project_flag() {
    assert_eq!(resolve(false, None), "false");
    assert_eq!(resolve(true, None), "true");
}

#[test]
fn model_setting_overrides_project_flag() {
    assert_eq!(resolve(false, Some(Value::from(true))), "true");
    assert_eq!(resolve(true, Some(Value::from(false))), "false");
}
