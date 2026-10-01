use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
};
use crate::value::none_value;

use arrow_schema::Schema;
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::value::{Value, ValueMap};
use serde_json;

pub(crate) const TYPE_NAME: &str = "cluster";

/// Component for BigQuery cluster by columns.
pub(crate) type ClusterBy = SimpleComponentConfigImpl<Vec<String>>;

fn to_jinja(v: &Vec<String>) -> Value {
    if v.is_empty() {
        none_value()
    } else {
        ValueMap::from([("fields".into(), Value::from_serialize(v))]).into()
    }
}

// BigQuery limits the number of `cluster_by` columns to 4, so efficiency isn't
// really a huge concern here.
#[allow(clippy::ptr_arg)]
fn diff_cluster_by(desired: &Vec<String>, current: &Vec<String>) -> Option<Vec<String>> {
    if desired.len() != current.len() {
        return Some(desired.clone());
    }

    let mut desired_sorted = desired.iter().collect::<Vec<&String>>();
    let mut current_sorted = current.iter().collect::<Vec<&String>>();

    desired_sorted.sort_unstable();
    current_sorted.sort_unstable();

    (desired_sorted != current_sorted).then(|| desired.clone())
}

fn new_component(columns: Vec<String>) -> ClusterBy {
    ClusterBy {
        type_name: TYPE_NAME,
        diff_fn: diff_cluster_by,
        to_jinja_fn: to_jinja,
        value: columns,
    }
}

fn from_remote_state(schema: &Schema) -> AdapterResult<ClusterBy> {
    let columns = schema
        .metadata
        .get("Clustering.Fields")
        .map(|value_json| {
            if value_json.is_empty() {
                Vec::default()
            } else {
                serde_json::from_str(value_json).unwrap_or_default()
            }
        })
        .unwrap_or_default();

    Ok(new_component(columns))
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<ClusterBy> {
    let config = match relation_config.as_any().downcast_ref::<DbtModel>() {
        None => Vec::new(),
        Some(model) => model
            .__adapter_attr__
            .bigquery_attr
            .as_ref()
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "relation config needs to be BigQuery model".to_string(),
                )
            })?
            .cluster_by
            .as_ref()
            .map(|cb| cb.fields().iter().map(|f| (*f).to_string()).collect())
            .unwrap_or_default(),
    };
    Ok(new_component(config))
}

impl_loader!(ClusterBy, Schema);

impl ClusterByLoader {
    pub fn new_component_type_erased(columns: Vec<String>) -> Box<dyn ComponentConfig> {
        Box::new(new_component(columns))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::bigquery::config::test_helpers::{
        TestTableConfig, make_driver_data, make_local_config,
    };

    #[test]
    fn from_remote_state_no_cluster_by() {
        let driver_data = make_driver_data(Default::default());
        let loaded = from_remote_state(&driver_data).unwrap();
        assert!(loaded.value.is_empty());
    }

    #[test]
    fn from_remote_state_with_cluster_by() {
        let driver_data = make_driver_data(TestTableConfig {
            cluster_by: &["a", "b", "c"],
            ..Default::default()
        });
        let loaded = from_remote_state(&driver_data).unwrap();
        assert_eq!(loaded.value.len(), 3);
        assert_eq!(loaded.value[0], "a");
        assert_eq!(loaded.value[1], "b");
        assert_eq!(loaded.value[2], "c");
    }

    #[test]
    fn from_local_config_no_cluster_by() {
        let local_data = make_local_config(Default::default());
        let loaded = from_local_config(&local_data).unwrap();
        assert!(loaded.value.is_empty());
    }

    #[test]
    fn from_local_config_with_cluster_by() {
        let local_data = make_local_config(TestTableConfig {
            cluster_by: &["a", "b", "c"],
            ..Default::default()
        });
        let loaded = from_local_config(&local_data).unwrap();
        assert_eq!(loaded.value.len(), 3);
        assert_eq!(loaded.value[0], "a");
        assert_eq!(loaded.value[1], "b");
        assert_eq!(loaded.value[2], "c");
    }

    #[test]
    fn same_fields_different_order_no_change() {
        let desired = vec![
            "portal".to_string(),
            "cu_name".to_string(),
            "rt_name".to_string(),
        ];
        let current = vec![
            "rt_name".to_string(),
            "portal".to_string(),
            "cu_name".to_string(),
        ];
        assert!(diff_cluster_by(&desired, &current).is_none());
    }

    #[test]
    fn different_fields_detected_as_change() {
        let desired = vec!["id".to_string(), "value".to_string()];
        let current = vec!["id".to_string(), "name".to_string()];
        assert!(diff_cluster_by(&desired, &current).is_some());
    }

    #[test]
    fn cluster_added_detected_as_change() {
        let desired = vec!["id".to_string()];
        let current = vec![];
        assert!(diff_cluster_by(&desired, &current).is_some());
    }

    #[test]
    fn cluster_removed_detected_as_change() {
        let desired = vec![];
        let current = vec!["id".to_string()];
        assert!(diff_cluster_by(&desired, &current).is_some());
    }

    #[test]
    fn both_none_no_change() {
        let desired = vec![];
        let current = vec![];
        assert!(diff_cluster_by(&desired, &current).is_none());
    }
}
