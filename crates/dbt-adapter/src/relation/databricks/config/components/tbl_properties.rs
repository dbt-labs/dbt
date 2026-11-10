//! https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/tblproperties.py

use crate::errors::AdapterResult;
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, impl_loader,
};
use crate::relation::databricks::config::{
    DatabricksRelationMetadata, DatabricksRelationMetadataKey,
};
use dbt_schemas::schemas::DbtModel;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_yaml::Value as YmlValue;
use indexmap::IndexMap;
use minijinja::value::{Value, ValueMap};

pub(crate) const TYPE_NAME: &str = "tblproperties";

pub(crate) const PIPELINE_ID_KEY: &str = "pipelines.pipelineId";

/// Component for Databricks table properties.
pub type TblProperties = SimpleComponentConfigImpl<IndexMap<String, String>>;

fn to_jinja(v: &IndexMap<String, String>) -> Value {
    // FIXME: is there a way to ignore a key and serialize into Value without an extra allocation?
    let ignore_pipeline = v
        .iter()
        .filter(|(k, _v)| k.as_str() != PIPELINE_ID_KEY)
        .collect::<IndexMap<_, _>>();

    Value::from(ValueMap::from([
        (
            Value::from("tblproperties"),
            Value::from_serialize(ignore_pipeline),
        ),
        (
            Value::from("pipeline_id"),
            Value::from_serialize(v.get(PIPELINE_ID_KEY)),
        ),
    ]))
}

fn new_component(properties: IndexMap<String, String>) -> TblProperties {
    TblProperties {
        type_name: TYPE_NAME,
        diff_fn: diff,
        to_jinja_fn: to_jinja,
        value: properties,
    }
}

/// Takes the diff between two `TblProperties`, matching the Python dbt-databricks
/// `TblPropertiesConfig.get_diff` semantics:
///
/// ```python
/// def get_diff(self, other):
///     if self.tblproperties.items() - other.tblproperties.items():
///         return self
///     return None
/// ```
///
/// This is an asymmetric, one-directional comparison: a diff is reported only when the
/// *desired* state has a non-pipeline key/value pair that is missing or different in the
/// *current* state. Fusion retains the pipeline ID for rendering but excludes it from comparison
/// because dbt-databricks stores it separately from `tblproperties`. Keys present only in the
/// current state are ignored, and the returned value is the full desired state (matching Python's
/// `get_diff` returning `self`).
fn diff(
    desired_state: &IndexMap<String, String>,
    current_state: &IndexMap<String, String>,
) -> Option<IndexMap<String, String>> {
    let has_diff = desired_state
        .iter()
        .any(|(k, v)| k.as_str() != PIPELINE_ID_KEY && current_state.get(k) != Some(v));

    if has_diff {
        Some(desired_state.clone())
    } else {
        None
    }
}

fn from_remote_state(results: &DatabricksRelationMetadata) -> AdapterResult<TblProperties> {
    let Some(table) = results.get(&DatabricksRelationMetadataKey::ShowTblProperties) else {
        return Ok(new_component(IndexMap::new()));
    };

    let mut tblproperties = IndexMap::new();
    for row in table.rows() {
        if let (Ok(key_val), Ok(value_val)) =
            (row.get_item(&Value::from(0)), row.get_item(&Value::from(1)))
            && let (Some(key_str), Some(value_str)) = (key_val.as_str(), value_val.as_str())
        {
            tblproperties.insert(key_str.to_string(), value_str.to_string());
        }
    }

    Ok(new_component(tblproperties))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<TblProperties> {
    let Some(model) = relation_config.as_any().downcast_ref::<DbtModel>() else {
        return Ok(new_component(IndexMap::new()));
    };

    let mut tblproperties = IndexMap::new();

    if let Some(databricks_attr) = &model.__adapter_attr__.databricks_attr
        && let Some(props_map) = &databricks_attr.tblproperties
    {
        for (key, value) in &props_map.0 {
            if let YmlValue::String(value_str, _) = value {
                tblproperties.insert(key.clone(), value_str.clone());
            }
        }
    }

    let is_iceberg = model
        .deprecated_config
        .table_format
        .as_ref()
        .is_some_and(|s| s.eq_ignore_ascii_case("iceberg"));

    if is_iceberg {
        tblproperties.insert(
            "delta.enableIcebergCompatV2".to_string(),
            "true".to_string(),
        );
        tblproperties.insert(
            "delta.universalFormat.enabledFormats".to_string(),
            "iceberg".to_string(),
        );
    }

    Ok(new_component(tblproperties))
}

impl_loader!(TblProperties, DatabricksRelationMetadata);

impl TblPropertiesLoader {
    pub fn new_component_type_erased(
        properties: IndexMap<String, String>,
    ) -> Box<dyn ComponentConfig> {
        Box::new(new_component(properties))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::databricks::config::test_helpers;
    use dbt_agate::AgateTable;
    use dbt_schemas::schemas::DbtModel;
    use indexmap::IndexMap;
    use std::sync::Arc;

    fn create_mock_show_tblproperties_table(properties: Vec<(&str, &str)>) -> AgateTable {
        use arrow::csv::ReaderBuilder;
        use arrow_schema::{DataType, Field, Schema};
        use std::io;

        let mut csv_data = "key,value\n".to_string();
        for (key, value) in properties {
            csv_data.push_str(&format!("{key},{value}\n"));
        }

        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Utf8, true),
            Field::new("value", DataType::Utf8, true),
        ]));

        let file = io::Cursor::new(csv_data);
        let mut reader = ReaderBuilder::new(schema)
            .with_header(true)
            .build(file)
            .unwrap();
        let batch = reader.next().unwrap().unwrap();
        AgateTable::from_record_batch(Arc::new(batch))
    }

    fn create_mock_dbt_model(
        tblproperties: IndexMap<&str, &str>,
        table_format: Option<&str>,
    ) -> DbtModel {
        let cfg = test_helpers::TestModelConfig {
            tbl_properties: tblproperties
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            table_format: table_format.map(|s| s.to_string()),
            ..Default::default()
        };
        test_helpers::create_mock_dbt_model(cfg)
    }

    #[test]
    fn test_diff_ignores_pipeline_id_changes() {
        let current = IndexMap::from_iter([
            (
                "pipelines.pipelineId".to_string(),
                "pipeline123".to_string(),
            ),
            ("custom.property".to_string(), "value".to_string()),
        ]);
        let desired = IndexMap::from_iter([
            (
                "pipelines.pipelineId".to_string(),
                "pipeline123456".to_string(),
            ),
            ("custom.property".to_string(), "value".to_string()),
        ]);

        assert!(diff(&desired, &current).is_none());
    }

    #[test]
    fn test_diff_empty_and_some_exist() {
        let desired = IndexMap::new();
        let current = IndexMap::from_iter([("prop".to_string(), "1".to_string())]);

        assert!(diff(&desired, &current).is_none());
    }

    #[test]
    fn test_diff_some_new_and_empty_existing() {
        let desired = IndexMap::from_iter([("prop".to_string(), "1".to_string())]);
        let current = IndexMap::new();

        assert_eq!(diff(&desired, &current), Some(desired));
    }

    #[test]
    fn test_diff_mixed_case() {
        let desired = IndexMap::from_iter([
            ("prop".to_string(), "1".to_string()),
            ("other".to_string(), "other".to_string()),
        ]);
        let current = IndexMap::from_iter([
            ("prop".to_string(), "2".to_string()),
            ("c".to_string(), "value".to_string()),
        ]);

        assert_eq!(diff(&desired, &current), Some(desired));
    }

    #[test]
    fn test_diff_retains_already_applied_properties() {
        let desired = IndexMap::from_iter([
            ("a".to_string(), "1".to_string()),
            ("c".to_string(), "1".to_string()),
        ]);
        let current = IndexMap::from_iter([("a".to_string(), "1".to_string())]);

        assert_eq!(diff(&desired, &current), Some(desired));
    }

    #[test]
    fn test_diff_no_changes() {
        let desired = IndexMap::from_iter([("prop".to_string(), "1".to_string())]);
        let current = desired.clone();

        assert!(diff(&desired, &current).is_none());
    }

    #[test]
    fn test_diff_ignores_server_set_properties() {
        let desired = IndexMap::from_iter([("prop".to_string(), "1".to_string())]);
        let current = IndexMap::from_iter([
            ("prop".to_string(), "1".to_string()),
            ("server.property".to_string(), "value".to_string()),
        ]);

        assert!(diff(&desired, &current).is_none());
    }

    #[test]
    fn test_from_remote_state_retains_server_set_properties() {
        let table = create_mock_show_tblproperties_table(vec![
            ("prop", "1"),
            ("delta.enableChangeDataFeed", "true"),
        ]);

        let results = IndexMap::from([(DatabricksRelationMetadataKey::ShowTblProperties, table)]);
        let config = from_remote_state(&results).unwrap();

        let expected = IndexMap::<String, String>::from_iter([
            ("prop".to_string(), "1".to_string()),
            ("delta.enableChangeDataFeed".to_string(), "true".to_string()),
        ]);

        assert_eq!(config.value, expected);
    }

    #[test]
    fn test_to_jinja_separates_pipeline_id_from_tblproperties() {
        let properties = IndexMap::from_iter([
            (PIPELINE_ID_KEY.to_string(), "pipeline123".to_string()),
            ("prop".to_string(), "1".to_string()),
        ]);

        let value = to_jinja(&properties);
        let tblproperties = value.get_attr("tblproperties").unwrap();

        assert_eq!(
            value.get_attr("pipeline_id").unwrap().as_str(),
            Some("pipeline123")
        );
        assert_eq!(
            tblproperties
                .get_item(&Value::from("prop"))
                .unwrap()
                .as_str(),
            Some("1")
        );
        assert!(
            tblproperties
                .get_item(&Value::from(PIPELINE_ID_KEY))
                .unwrap()
                .is_undefined()
        );
    }

    #[test]
    fn test_from_local_config() {
        let props = IndexMap::from_iter([
            ("streaming.checkpointLocation", "/tmp/checkpoint"),
            ("streaming.outputMode", "append"),
            ("custom.property", "test_value"),
        ]);
        let model = create_mock_dbt_model(props, None);
        let config = from_local_config(&model).unwrap();

        assert_eq!(config.value.len(), 3);
        assert_eq!(
            config.value.get("streaming.checkpointLocation"),
            Some(&"/tmp/checkpoint".to_string())
        );
        assert_eq!(
            config.value.get("streaming.outputMode"),
            Some(&"append".to_string())
        );
        assert_eq!(
            config.value.get("custom.property"),
            Some(&"test_value".to_string())
        );
        assert!(!config.value.contains_key(PIPELINE_ID_KEY));
    }

    #[test]
    fn test_from_local_config_iceberg() {
        let props = IndexMap::from_iter([("custom.property", "test_value")]);
        let model = create_mock_dbt_model(props, Some("iceberg"));
        let config = from_local_config(&model).unwrap();

        assert_eq!(config.value.len(), 3); // custom + 2 iceberg properties
        assert_eq!(
            config.value.get("custom.property"),
            Some(&"test_value".to_string())
        );
        assert_eq!(
            config.value.get("delta.enableIcebergCompatV2"),
            Some(&"true".to_string())
        );
        assert_eq!(
            config.value.get("delta.universalFormat.enabledFormats"),
            Some(&"iceberg".to_string())
        );
    }

    #[test]
    fn test_from_local_config_empty() {
        let model = create_mock_dbt_model(IndexMap::new(), None);
        let config = from_local_config(&model).unwrap();

        assert!(config.value.is_empty());
    }
}
