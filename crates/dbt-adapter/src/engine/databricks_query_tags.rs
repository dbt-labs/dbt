use adbc_core::options::OptionValue;
use dbt_common::{AdapterError, AdapterErrorKind, AdapterResult};
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::{DbtModel, DbtSeed, DbtSnapshot, DbtTest, DbtUnitTest};
use indexmap::IndexMap;
use minijinja::State;
use serde::Deserialize;
use serde_json::Value;

use super::Options;
use crate::AdapterType;

const QUERY_TAG_OPTION_PREFIX: &str = "databricks.query_tag.";
const DBT_CORE_VERSION: &str = "@@dbt_core_version";
const DBT_MODEL_NAME: &str = "@@dbt_model_name";
const DBT_MATERIALIZED: &str = "@@dbt_materialized";
const MAX_AUTOMATIC_VALUE_CHARS: usize = 128;
const RESERVED_KEYS: [&str; 3] = [DBT_CORE_VERSION, DBT_MODEL_NAME, DBT_MATERIALIZED];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct DatabricksQueryTags {
    tags: IndexMap<String, String>,
}

impl DatabricksQueryTags {
    fn from_node(
        model_query_tags: Option<&str>,
        model_name: Option<&str>,
        materialized: Option<&str>,
    ) -> AdapterResult<Self> {
        let model_tags = parse_user_tags(model_query_tags, "Model config")?;

        let mut tags = IndexMap::new();
        if let Some(model_name) = model_name {
            tags.insert(DBT_MODEL_NAME.to_string(), truncate_default(model_name));
        }
        if let Some(materialized) = materialized {
            tags.insert(DBT_MATERIALIZED.to_string(), truncate_default(materialized));
        }
        tags.extend(model_tags);

        Ok(Self { tags })
    }

    pub(super) fn into_statement_options(self) -> Vec<(String, OptionValue)> {
        self.tags
            .into_iter()
            .map(|(key, value)| {
                (
                    format!("{QUERY_TAG_OPTION_PREFIX}{key}"),
                    OptionValue::String(value),
                )
            })
            .collect()
    }
}

pub(super) fn query_tags_from_state(state: Option<&State>) -> AdapterResult<DatabricksQueryTags> {
    let Some(node) = state.and_then(|state| state.lookup("model", &[])) else {
        return Ok(DatabricksQueryTags::default());
    };
    let yaml_node = dbt_yaml::to_value(&node)
        .map_err(|error| AdapterError::new(AdapterErrorKind::Configuration, error.to_string()))?;
    query_tags_from_yaml_node(&yaml_node)
}

pub(crate) fn databricks_statement_options(
    adapter_type: AdapterType,
    state: Option<&State>,
) -> AdapterResult<Options> {
    match (adapter_type, state) {
        (AdapterType::Databricks, Some(state)) => {
            Ok(query_tags_from_state(Some(state))?.into_statement_options())
        }
        _ => Ok(Options::new()),
    }
}

fn query_tags_from_yaml_node(yaml_node: &dbt_yaml::Value) -> AdapterResult<DatabricksQueryTags> {
    macro_rules! tags_for_node {
        ($node_type:ty, $node:ident) => {
            if let Ok($node) = <$node_type>::deserialize(yaml_node) {
                let query_tags = $node
                    .__adapter_attr__
                    .databricks_attr
                    .as_deref()
                    .and_then(|attr| attr.query_tags.as_deref());
                return DatabricksQueryTags::from_node(
                    query_tags,
                    Some(&$node.__common_attr__.name),
                    Some(&$node.__base_attr__.materialized.to_string()),
                );
            }
        };
    }

    // TODO: Remove this path after seeds and unit tests expose typed adapter attributes.
    macro_rules! tags_for_node_deprecated {
        ($node_type:ty, $node:ident, $materialized:expr) => {
            if let Ok($node) = <$node_type>::deserialize(yaml_node) {
                let query_tags = $node
                    .deprecated_config
                    .__warehouse_specific_config__
                    .query_tags
                    .as_deref();
                let materialized: Option<String> = $materialized;
                return DatabricksQueryTags::from_node(
                    query_tags,
                    Some(&$node.__common_attr__.name),
                    materialized.as_deref(),
                );
            }
        };
    }

    tags_for_node!(DbtModel, model);
    tags_for_node_deprecated!(DbtUnitTest, unit_test, None);
    tags_for_node!(DbtTest, test);
    tags_for_node!(DbtSnapshot, snapshot);
    tags_for_node_deprecated!(
        DbtSeed,
        seed,
        Some(
            seed.deprecated_config
                .materialized
                .as_ref()
                .unwrap_or(&DbtMaterialization::Seed)
                .to_string()
        )
    );

    Ok(DatabricksQueryTags::default())
}

fn parse_user_tags(
    query_tags: Option<&str>,
    source: &str,
) -> AdapterResult<IndexMap<String, String>> {
    let Some(query_tags) = query_tags.filter(|value| !value.is_empty()) else {
        return Ok(IndexMap::new());
    };

    let object: IndexMap<String, Value> = serde_json::from_str(query_tags).map_err(|error| {
        if error.classify() == serde_json::error::Category::Data {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                "query_tags must be a JSON object (dictionary)",
            )
        } else {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Invalid JSON in query_tags: {error}"),
            )
        }
    })?;

    let mut reserved = object
        .keys()
        .filter(|key| RESERVED_KEYS.contains(&key.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    reserved.sort();
    if !reserved.is_empty() {
        return configuration_error(format!(
            "{source}: Cannot use reserved query tag keys: {}. Reserved keys are: {}",
            reserved.join(", "),
            RESERVED_KEYS.join(", ")
        ));
    }

    object
        .into_iter()
        .map(|(key, value)| {
            let Value::String(value) = value else {
                return configuration_error(format!(
                    "{source}: query_tags values must be strings for key '{key}'. Only string values are supported."
                ));
            };
            Ok((key, value))
        })
        .collect()
}

fn configuration_error<T>(message: impl Into<String>) -> AdapterResult<T> {
    Err(AdapterError::new(
        AdapterErrorKind::Configuration,
        message.into(),
    ))
}

fn truncate_default(value: &str) -> String {
    value.chars().take(MAX_AUTOMATIC_VALUE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        DatabricksQueryTags, QUERY_TAG_OPTION_PREFIX, databricks_statement_options,
        query_tags_from_state, query_tags_from_yaml_node,
    };
    use crate::AdapterType;
    use adbc_core::options::OptionValue;
    use dbt_schemas::schemas::{
        AdapterAttr, DbtModel, DbtSeed, DbtSnapshot, DbtTest, DbtUnitTest,
        common::DbtMaterialization, manifest::DbtOperation, nodes::DatabricksAttr,
    };
    use minijinja::{Environment, State, Value as MinijinjaValue};

    fn string_option<'a>(options: &'a [(String, OptionValue)], name: &str) -> Option<&'a str> {
        options.iter().find_map(|(option_name, value)| {
            if option_name == name
                && let OptionValue::String(value) = value
            {
                Some(value.as_str())
            } else {
                None
            }
        })
    }

    #[test]
    fn emits_only_node_driver_statement_options() {
        let options = DatabricksQueryTags::from_node(
            Some(r#"{"team":"model"}"#),
            Some("orders"),
            Some("incremental"),
        )
        .unwrap()
        .into_statement_options();

        assert_eq!(options.len(), 3);
        assert_eq!(
            options
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            [
                "databricks.query_tag.@@dbt_model_name",
                "databricks.query_tag.@@dbt_materialized",
                "databricks.query_tag.team",
            ]
        );

        assert_eq!(
            string_option(&options, &format!("{QUERY_TAG_OPTION_PREFIX}team")),
            Some("model")
        );
        assert_eq!(
            string_option(
                &options,
                &format!("{QUERY_TAG_OPTION_PREFIX}@@dbt_model_name")
            ),
            Some("orders")
        );
    }

    #[test]
    fn preserves_query_tag_insertion_order() {
        let options = DatabricksQueryTags::from_node(
            Some(r#"{"m_model":"last","a_shared":"model"}"#),
            Some("orders"),
            Some("incremental"),
        )
        .unwrap()
        .into_statement_options();

        let names = options
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "databricks.query_tag.@@dbt_model_name",
                "databricks.query_tag.@@dbt_materialized",
                "databricks.query_tag.m_model",
                "databricks.query_tag.a_shared",
            ]
        );
        assert_eq!(
            string_option(&options, "databricks.query_tag.a_shared"),
            Some("model")
        );
    }

    #[test]
    fn missing_state_and_operations_inherit_database_defaults_without_statement_options() {
        assert!(
            query_tags_from_state(None)
                .unwrap()
                .into_statement_options()
                .is_empty()
        );

        let mut operation = DbtOperation::default();
        operation.__common_attr__.name = "on-run-start-0".to_string();
        let yaml = dbt_yaml::to_value(operation).unwrap();

        let options = query_tags_from_yaml_node(&yaml)
            .unwrap()
            .into_statement_options();
        assert!(options.is_empty());

        let internal = dbt_yaml::from_str("internal: true").unwrap();
        assert!(
            query_tags_from_yaml_node(&internal)
                .unwrap()
                .into_statement_options()
                .is_empty()
        );
    }

    fn tagged_seed() -> DbtSeed {
        let mut seed = DbtSeed::default();
        seed.__common_attr__.name = "orders_seed".to_string();
        seed.__base_attr__.materialized = DbtMaterialization::Table;
        seed.deprecated_config.materialized = Some(DbtMaterialization::Seed);
        seed.deprecated_config
            .__warehouse_specific_config__
            .query_tags = Some(r#"{"team":"resource"}"#.to_string());
        seed
    }

    fn model_global(seed: DbtSeed) -> MinijinjaValue {
        MinijinjaValue::from_serialize(dbt_yaml::to_value(seed).unwrap())
    }

    #[test]
    fn databricks_statement_options_tag_the_node_in_state() {
        let mut env = Environment::new();
        env.add_global("model", model_global(tagged_seed()));
        let state = State::new_for_env(&env);

        let options = databricks_statement_options(AdapterType::Databricks, Some(&state)).unwrap();

        assert_eq!(
            string_option(&options, "databricks.query_tag.@@dbt_model_name"),
            Some("orders_seed")
        );
        assert_eq!(
            string_option(&options, "databricks.query_tag.@@dbt_materialized"),
            Some("seed")
        );
        assert_eq!(
            string_option(&options, "databricks.query_tag.team"),
            Some("resource")
        );
    }

    #[test]
    fn databricks_statement_options_are_empty_without_databricks_or_without_a_node() {
        let mut env = Environment::new();
        env.add_global("model", model_global(tagged_seed()));
        let state = State::new_for_env(&env);

        for adapter_type in [AdapterType::Spark, AdapterType::Snowflake] {
            assert!(
                databricks_statement_options(adapter_type, Some(&state))
                    .unwrap()
                    .is_empty(),
                "{adapter_type} must not receive Databricks query tags"
            );
        }

        assert!(
            databricks_statement_options(AdapterType::Databricks, None)
                .unwrap()
                .is_empty()
        );
    }

    fn databricks_attr(query_tags: &str) -> AdapterAttr {
        AdapterAttr::default().with_databricks_attr(Some(Box::new(DatabricksAttr {
            query_tags: Some(query_tags.to_string()),
            ..Default::default()
        })))
    }

    fn assert_resource_tags(
        yaml: dbt_yaml::Value,
        resource_name: &str,
        materialized: Option<&str>,
    ) {
        let options = query_tags_from_yaml_node(&yaml)
            .unwrap()
            .into_statement_options();

        assert_eq!(
            string_option(&options, "databricks.query_tag.team"),
            Some("resource")
        );
        assert_eq!(
            string_option(&options, "databricks.query_tag.@@dbt_model_name"),
            Some(resource_name)
        );
        assert_eq!(
            string_option(&options, "databricks.query_tag.@@dbt_materialized"),
            materialized
        );
    }

    #[test]
    fn non_model_resources_emit_resource_query_tags() {
        let mut test = DbtTest::default();
        test.__common_attr__.name = "accepted_values_orders".to_string();
        test.__base_attr__.materialized = DbtMaterialization::Test;
        test.__adapter_attr__ = databricks_attr(r#"{"team":"resource"}"#);
        test.deprecated_config
            .__warehouse_specific_config__
            .query_tags = Some(r#"{"team":"deprecated"}"#.to_string());
        assert_resource_tags(
            dbt_yaml::to_value(test).unwrap(),
            "accepted_values_orders",
            Some("test"),
        );

        let mut snapshot = DbtSnapshot::default();
        snapshot.__common_attr__.name = "orders_snapshot".to_string();
        snapshot.__base_attr__.materialized = DbtMaterialization::Snapshot;
        snapshot.__adapter_attr__ = databricks_attr(r#"{"team":"resource"}"#);
        snapshot
            .deprecated_config
            .__warehouse_specific_config__
            .query_tags = Some(r#"{"team":"deprecated"}"#.to_string());
        assert_resource_tags(
            dbt_yaml::to_value(snapshot).unwrap(),
            "orders_snapshot",
            Some("snapshot"),
        );

        assert_resource_tags(
            dbt_yaml::to_value(tagged_seed()).unwrap(),
            "orders_seed",
            Some("seed"),
        );

        let mut unit_test = DbtUnitTest::default();
        unit_test.__common_attr__.name = "orders_unit_test".to_string();
        unit_test
            .deprecated_config
            .__warehouse_specific_config__
            .query_tags = Some(r#"{"team":"resource"}"#.to_string());
        assert_resource_tags(
            dbt_yaml::to_value(unit_test).unwrap(),
            "orders_unit_test",
            None,
        );
    }

    #[test]
    fn model_state_reads_rendered_query_tags() {
        let mut model = DbtModel::default();
        model.__common_attr__.name = "orders".to_string();
        model.__adapter_attr__ = databricks_attr(r#"{"team":"model"}"#);
        let yaml = dbt_yaml::to_value(model).unwrap();

        let options = query_tags_from_yaml_node(&yaml)
            .unwrap()
            .into_statement_options();

        assert_eq!(
            string_option(&options, "databricks.query_tag.team"),
            Some("model")
        );
    }

    #[test]
    fn rejects_invalid_shapes_and_non_string_values() {
        let non_object =
            DatabricksQueryTags::from_node(Some(r#"["not","an","object"]"#), None, None)
                .unwrap_err();
        assert!(non_object.message().contains("must be a JSON object"));

        let non_string =
            DatabricksQueryTags::from_node(Some(r#"{"cost_center":3000}"#), None, None)
                .unwrap_err();
        assert!(non_string.message().contains("values must be strings"));
    }

    #[test]
    fn rejects_reserved_node_query_tag_keys() {
        for source in [
            r#"{"@@dbt_core_version":"override"}"#,
            r#"{"@@dbt_model_name":"override"}"#,
            r#"{"@@dbt_materialized":"override"}"#,
        ] {
            let model_error = DatabricksQueryTags::from_node(Some(source), None, None).unwrap_err();
            assert!(model_error.message().contains("reserved query tag keys"));
        }

        let error = DatabricksQueryTags::from_node(
            Some(r#"{"@@dbt_materialized":"override","@@dbt_core_version":"override"}"#),
            None,
            None,
        )
        .unwrap_err();
        assert!(
            error.message().contains(
                "Cannot use reserved query tag keys: @@dbt_core_version, @@dbt_materialized"
            ),
            "{}",
            error.message()
        );
    }

    #[test]
    fn truncates_automatic_node_values_to_128_characters() {
        let model_name = format!("{}::", "x".repeat(127));
        let options = DatabricksQueryTags::from_node(None, Some(&model_name), Some("table"))
            .unwrap()
            .into_statement_options();

        assert_eq!(
            string_option(&options, "databricks.query_tag.@@dbt_model_name")
                .unwrap()
                .chars()
                .count(),
            128
        );
    }
}
