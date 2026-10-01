use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use dbt_adapter::relation::RelationObject;
use dbt_adapter_core::AdapterType;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::serde_utils::convert_yml_to_value_map;
use dbt_jinja_utils::{mock_object::MockJinjaObject, phases::run::RunConfig};
use dbt_schemas::{
    dbt_types::RelationType,
    schemas::project::{ModelConfig, ProjectModelConfig, ResolvableConfig},
};
use minijinja::Value;
use minijinja::value::mutable_map::MutableMap;

use crate::macro_test_harness::MacroTestHarness;

fn build_harness() -> MacroTestHarness {
    let harness = MacroTestHarness::for_adapter(AdapterType::Databricks)
        .load_all_macros()
        .with_stub_functions()
        .with_behavior_flag("use_materialization_v2", false)
        .build()
        .expect("harness should build");

    harness.mock().on("clean_sql", |args| {
        Ok(args.first().cloned().unwrap_or(Value::UNDEFINED))
    });
    harness.mock().on("is_uniform", |_| Ok(Value::from(false)));
    harness
}

fn omitted_policy_runtime_config() -> RunConfig {
    let unresolved_config: ProjectModelConfig =
        dbt_yaml::from_str("__additional_properties__: {}\n")
            .expect("deserialize project model config with omitted policy");
    assert!(
        unresolved_config.on_configuration_change.is_none(),
        "test input must omit on_configuration_change"
    );
    let mut source_config: ModelConfig = unresolved_config.into();
    source_config.apply_resolve_defaults((StaticAnalysisKind::default(), None, None));
    let serialized =
        dbt_yaml::to_value(&source_config).expect("serialize runtime model config as YAML");
    let mut model_config = convert_yml_to_value_map(serialized);
    assert_eq!(
        model_config
            .get("on_configuration_change")
            .and_then(Value::as_str),
        Some("apply"),
        "runtime model config must expose the dbt-core default through config.get",
    );
    model_config.insert("full_refresh".to_string(), Value::from(false));

    RunConfig {
        model_config,
        model: Arc::new(MutableMap::new()),
        model_compiled_path: PathBuf::new(),
        valid_keys: HashSet::new(),
    }
}

#[test]
fn omitted_model_config_resolves_to_apply_for_existing_relation() {
    let harness = build_harness();
    let change_set = Arc::new(MockJinjaObject::new());
    change_set.set_attr("requires_full_refresh", Value::from(false));
    change_set.set_attr(
        "changes",
        Value::from_serialize(BTreeMap::from([
            (
                "partition_by",
                Value::from_serialize(BTreeMap::from([("partition_by", Value::UNDEFINED)])),
            ),
            (
                "tblproperties",
                Value::from_serialize(BTreeMap::from([("tblproperties", Value::UNDEFINED)])),
            ),
            (
                "comment",
                Value::from_serialize(BTreeMap::from([(
                    "comment",
                    Value::from("updated comment"),
                )])),
            ),
            ("refresh", Value::UNDEFINED),
            ("tags", Value::UNDEFINED),
        ])),
    );
    let changes = Value::from_dyn_object(change_set);
    let model_config = Arc::new(MockJinjaObject::new());
    model_config.on("get_changeset", move |_| Ok(changes.clone()));
    let model_config = Value::from_dyn_object(model_config);
    harness
        .mock()
        .on("get_config_from_model", move |_| Ok(model_config.clone()));
    harness
        .mock()
        .on("get_relation_config", |_| Ok(Value::UNDEFINED));

    let existing = harness.relation(
        "TEST_DB",
        "TEST_SCHEMA",
        "streaming_probe",
        Some(RelationType::StreamingTable),
    );
    let target = harness.relation(
        "TEST_DB",
        "TEST_SCHEMA",
        "streaming_probe",
        Some(RelationType::StreamingTable),
    );
    let ctx = harness
        .materialization_context("streaming_probe", "SELECT 1")
        .relation_type(RelationType::StreamingTable)
        .config(Value::from_object(omitted_policy_runtime_config()))
        .with("existing", RelationObject::new(existing).into_value())
        .with("target", RelationObject::new(target).into_value())
        .build();

    let result = harness
        .render(
            "{% set r = streaming_table_get_build_sql(existing, target) %}{{ r }}",
            ctx,
        )
        .expect("resolved omitted policy should exercise the existing-relation apply branch");

    assert!(
        result
            .to_uppercase()
            .contains("CREATE OR REFRESH STREAMING TABLE"),
        "expected the apply path to render a streaming-table update, got: {result}",
    );
}

#[test]
fn auto_refreshed_streaming_table_no_op_does_not_issue_manual_refresh() {
    let harness = build_harness();
    let model_config = Arc::new(MockJinjaObject::new());
    model_config.on("get_changeset", |_| Ok(Value::from(())));
    model_config.set_attr(
        "refresh",
        Value::from_serialize(BTreeMap::from([("auto_refreshed", Value::from(true))])),
    );
    let model_config = Value::from_dyn_object(model_config);
    harness
        .mock()
        .on("get_config_from_model", move |_| Ok(model_config.clone()));
    harness
        .mock()
        .on("get_relation_config", |_| Ok(Value::UNDEFINED));

    let existing = harness.relation(
        "TEST_DB",
        "TEST_SCHEMA",
        "streaming_probe",
        Some(RelationType::StreamingTable),
    );
    let target = harness.relation(
        "TEST_DB",
        "TEST_SCHEMA",
        "streaming_probe",
        Some(RelationType::StreamingTable),
    );
    let ctx = harness
        .materialization_context("streaming_probe", "SELECT 1")
        .relation_type(RelationType::StreamingTable)
        .config(Value::from_object(omitted_policy_runtime_config()))
        .with("existing", RelationObject::new(existing).into_value())
        .with("target", RelationObject::new(target).into_value())
        .build();

    let result = harness
        .render(
            "{% set r = streaming_table_get_build_sql(existing, target) %}{{ r }}",
            ctx,
        )
        .expect("auto-refreshed streaming table should render the no-op path");

    assert!(
        result.trim().is_empty(),
        "expected no refresh SQL, got: {result}"
    );
}
