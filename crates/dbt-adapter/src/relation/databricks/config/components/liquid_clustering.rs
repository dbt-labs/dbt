//! https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/liquid_clustering.py

use dbt_schemas::schemas::DbtModel;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use minijinja::Value;
use serde::Serialize;

use crate::errors::AdapterResult;
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
};
use crate::relation::databricks::config::{
    DatabricksRelationMetadata, DatabricksRelationMetadataKey,
};

pub(crate) const TYPE_NAME: &str = "liquid_clustering";

const CLUSTER_BY_AUTO_KEY: &str = "clusterByAuto";
const CLUSTERING_COLUMNS_KEY: &str = "clusteringColumns";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Config {
    pub auto_cluster: bool,
    pub cluster_by: Vec<String>,
}

/// Component for Databricks liquid clustering
pub(crate) type LiquidClustering = SimpleComponentConfigImpl<Config>;

fn new_component(auto_cluster: bool, cluster_by: Vec<String>) -> LiquidClustering {
    LiquidClustering {
        type_name: TYPE_NAME,
        diff_fn: diff::desired_state,
        to_jinja_fn: |v| Value::from_serialize(v),
        value: Config {
            auto_cluster,
            cluster_by,
        },
    }
}

// Reference: https://github.com/databricks/dbt-databricks/blob/45351e11517d3f37c5ac7a736b5fcba453d3f368/dbt/adapters/databricks/relation_configs/liquid_clustering.py#L68-L74
fn extract_cluster_by(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<Vec<String>>>(raw)
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect()
}

// Reference: https://github.com/databricks/dbt-databricks/blob/45351e11517d3f37c5ac7a736b5fcba453d3f368/dbt/adapters/databricks/relation_configs/liquid_clustering.py#L25-L35
fn from_remote_state(results: &DatabricksRelationMetadata) -> AdapterResult<LiquidClustering> {
    let Some(table) = results.get(&DatabricksRelationMetadataKey::ShowTblProperties) else {
        return Ok(new_component(false, Vec::new()));
    };

    let mut auto_cluster = false;
    let mut cluster_by = Vec::new();
    for row in table.rows() {
        if let (Ok(key_val), Ok(value_val)) =
            (row.get_item(&Value::from(0)), row.get_item(&Value::from(1)))
            && let (Some(key_str), Some(value_str)) = (key_val.as_str(), value_val.as_str())
        {
            match key_str {
                CLUSTER_BY_AUTO_KEY => auto_cluster = value_str == "true",
                CLUSTERING_COLUMNS_KEY => cluster_by = extract_cluster_by(value_str),
                _ => {}
            }
        }
    }

    Ok(new_component(auto_cluster, cluster_by))
}

// Reference: https://github.com/databricks/dbt-databricks/blob/45351e11517d3f37c5ac7a736b5fcba453d3f368/dbt/adapters/databricks/relation_configs/liquid_clustering.py#L37-L50
fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<LiquidClustering> {
    let Some(databricks_attr) = relation_config
        .as_any()
        .downcast_ref::<DbtModel>()
        .and_then(|model| model.__adapter_attr__.databricks_attr.as_ref())
    else {
        return Ok(new_component(false, Vec::new()));
    };

    let auto_cluster = databricks_attr.auto_liquid_cluster.unwrap_or(false);
    let cluster_by = databricks_attr
        .liquid_clustered_by
        .as_ref()
        .map(|c| c.to_strings())
        .unwrap_or_default();

    // TODO: managed iceberg partition_by fallback
    // https://github.com/databricks/dbt-databricks/blob/45351e11517d3f37c5ac7a736b5fcba453d3f368/dbt/adapters/databricks/relation_configs/liquid_clustering.py#L45-L50
    Ok(new_component(auto_cluster, cluster_by))
}

impl_loader!(LiquidClustering, DatabricksRelationMetadata);

impl LiquidClusteringLoader {
    pub fn new_component_type_erased(
        auto_cluster: bool,
        cluster_by: Vec<String>,
    ) -> Box<dyn ComponentConfig> {
        Box::new(new_component(auto_cluster, cluster_by))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::databricks::config::test_helpers;
    use dbt_agate::AgateTable;
    use indexmap::IndexMap;
    use std::sync::Arc;

    fn mock_show_tblproperties(rows: &[(&str, &str)]) -> AgateTable {
        use arrow::array::StringArray;
        use arrow::record_batch::RecordBatch;
        use arrow_schema::{DataType, Field, Schema};

        let keys: Vec<&str> = rows.iter().map(|(k, _)| *k).collect();
        let values: Vec<&str> = rows.iter().map(|(_, v)| *v).collect();
        let schema = Arc::new(Schema::new(vec![
            Field::new("key", DataType::Utf8, true),
            Field::new("value", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(keys)),
                Arc::new(StringArray::from(values)),
            ],
        )
        .unwrap();
        AgateTable::from_record_batch(Arc::new(batch))
    }

    fn mock_model(auto_cluster: bool, cluster_by: &[&str]) -> DbtModel {
        test_helpers::create_mock_dbt_model(test_helpers::TestModelConfig {
            auto_cluster,
            cluster_by: cluster_by.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        })
    }

    #[test]
    fn test_extract_cluster_by() {
        assert!(extract_cluster_by("").is_empty());
        assert!(extract_cluster_by("[]").is_empty());
        assert_eq!(extract_cluster_by(r#"[["col1"]]"#), vec!["col1"]);
        assert_eq!(
            extract_cluster_by(r#"[["col1"],["col2"]]"#),
            vec!["col1", "col2"]
        );
    }

    #[test]
    fn test_from_remote_state() {
        let table = mock_show_tblproperties(&[
            ("clusterByAuto", "false"),
            ("clusteringColumns", r#"[["country"],["logdate"]]"#),
        ]);
        let results = IndexMap::from([(DatabricksRelationMetadataKey::ShowTblProperties, table)]);
        let config = from_remote_state(&results).unwrap();

        assert!(!config.value.auto_cluster);
        assert_eq!(config.value.cluster_by, vec!["country", "logdate"]);
    }

    #[test]
    fn test_from_remote_state_auto() {
        let table = mock_show_tblproperties(&[("clusterByAuto", "true")]);
        let results = IndexMap::from([(DatabricksRelationMetadataKey::ShowTblProperties, table)]);
        let config = from_remote_state(&results).unwrap();

        assert!(config.value.auto_cluster);
        assert!(config.value.cluster_by.is_empty());
    }

    #[test]
    fn test_from_remote_state_missing() {
        let results = IndexMap::new();
        let config = from_remote_state(&results).unwrap();

        assert!(!config.value.auto_cluster);
        assert!(config.value.cluster_by.is_empty());
    }

    #[test]
    fn test_from_local_config() {
        let config = from_local_config(&mock_model(false, &["country", "logdate"])).unwrap();
        assert_eq!(config.value.cluster_by, vec!["country", "logdate"]);
        assert!(!config.value.auto_cluster);
    }

    #[test]
    fn test_from_local_config_none() {
        let config = from_local_config(&mock_model(false, &[])).unwrap();
        assert!(config.value.cluster_by.is_empty());
    }

    #[test]
    fn test_diff_is_case_sensitive() {
        let desired = new_component(false, vec!["country".to_string()]);
        let current = new_component(false, vec!["COUNTRY".to_string()]);
        assert!(desired.diff_from(Some(&current)).is_some());

        let same = new_component(false, vec!["country".to_string()]);
        assert!(desired.diff_from(Some(&same)).is_none());
    }
}
