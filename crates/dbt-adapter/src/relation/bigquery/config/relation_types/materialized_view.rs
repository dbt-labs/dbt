use crate::AdapterType;
use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use crate::relation::bigquery::config::components;
use crate::relation::config_v2::ComponentConfigChange;
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, RelationConfig, RelationConfigLoader,
};
use arrow_schema::Schema;
use chrono::DateTime;
use dbt_schemas::schemas::manifest::{
    BigqueryPartitionConfig, BigqueryPartitionConfigInner, Range, RangeConfig, TimeConfig,
};
use indexmap::IndexMap;

fn requires_full_refresh(components: &IndexMap<&'static str, ComponentConfigChange>) -> bool {
    const REFRESH_ON: [&str; 2] = [
        components::cluster_by::TYPE_NAME,
        components::partition_by::TYPE_NAME,
    ];

    for name in REFRESH_ON {
        let Some(change) = components.get(name) else {
            continue;
        };

        match change {
            ComponentConfigChange::Some(_) => return true,
            ComponentConfigChange::Drop => return true,
            ComponentConfigChange::None => continue,
        };
    }

    false
}

/// Create a `RelationConfigLoader` for BigQuery materialized views
pub(crate) fn new_loader() -> RelationConfigLoader<'static, Schema> {
    let loaders: [Box<dyn ComponentConfigLoader<Schema>>; 7] = [
        Box::new(components::ClusterByLoader),
        Box::new(components::DescriptionLoader),
        Box::new(components::KmsKeyLoader),
        Box::new(components::LabelsLoader),
        Box::new(components::PartitionByLoader),
        Box::new(components::RefreshLoader),
        Box::new(components::TagsLoader),
    ];

    RelationConfigLoader::new(AdapterType::Bigquery, loaders, requires_full_refresh)
}

pub(crate) fn relation_config_from_recorded(
    recorded: Option<&serde_json::Value>,
) -> AdapterResult<RelationConfig> {
    let recorded = recorded.ok_or_else(|| conversion_error("payload is absent"))?;
    let options = recorded
        .get("options")
        .ok_or_else(|| conversion_error("missing 'options'"))?;

    let refresh = components::refresh::Config {
        enable: options
            .get("enable_refresh")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        interval_min: options
            .get("refresh_interval_minutes")
            .and_then(|v| v.as_f64())
            .unwrap_or(30.0),
        max_staleness: options
            .get("max_staleness")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        expiration: options
            .get("expiration_timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.to_utc()),
    };

    let components: Vec<Box<dyn ComponentConfig>> = vec![
        components::DescriptionLoader::new_component_type_erased(
            options
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        ),
        components::LabelsLoader::new_component_type_erased(string_map_from_recorded(
            options, "labels",
        )),
        components::TagsLoader::new_component_type_erased(string_map_from_recorded(
            options, "tags",
        )),
        components::KmsKeyLoader::new_component_type_erased(
            options
                .get("kms_key_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        ),
        components::RefreshLoader::new_component_type_erased(refresh),
        components::ClusterByLoader::new_component_type_erased(cluster_from_recorded(recorded)?),
        components::PartitionByLoader::new_component_type_erased(partition_from_recorded(
            recorded,
        )?),
    ];

    Ok(RelationConfig::new(
        AdapterType::Bigquery,
        components,
        requires_full_refresh,
    ))
}

fn conversion_error(detail: &str) -> AdapterError {
    AdapterError::new(
        AdapterErrorKind::Configuration,
        format!(
            "could not convert recorded describe_relation payload into a BigQuery RelationConfig: {detail}"
        ),
    )
}

fn string_map_from_recorded(options: &serde_json::Value, key: &str) -> IndexMap<String, String> {
    options
        .get(key)
        .and_then(|v| serde_json::from_value::<IndexMap<String, String>>(v.clone()).ok())
        .unwrap_or_default()
}

fn cluster_from_recorded(recorded: &serde_json::Value) -> AdapterResult<Vec<String>> {
    match recorded.get("cluster") {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(cluster) => cluster
            .get("fields")
            .and_then(|f| serde_json::from_value::<Vec<String>>(f.clone()).ok())
            .ok_or_else(|| conversion_error("malformed 'cluster.fields'")),
    }
}

fn partition_from_recorded(
    recorded: &serde_json::Value,
) -> AdapterResult<Option<BigqueryPartitionConfig>> {
    let partition = match recorded.get("partition") {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(partition) => partition,
    };

    let field = partition
        .get("field")
        .and_then(|v| v.as_str())
        .ok_or_else(|| conversion_error("partition missing 'field'"))?
        .to_string();
    let data_type = partition
        .get("data_type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let copy_partitions = partition
        .get("copy_partitions")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let inner = match partition.get("range") {
        Some(range) if !range.is_null() => {
            let get_i64 = |key: &str| -> AdapterResult<i64> {
                range
                    .get(key)
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| conversion_error(&format!("partition range missing '{key}'")))
            };
            BigqueryPartitionConfigInner::Range(RangeConfig {
                range: Range {
                    start: get_i64("start")?,
                    end: get_i64("end")?,
                    interval: get_i64("interval")?,
                },
            })
        }
        _ => BigqueryPartitionConfigInner::Time(TimeConfig {
            granularity: partition
                .get("granularity")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            time_ingestion_partitioning: partition
                .get("time_ingestion_partitioning")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }),
    };

    Ok(Some(BigqueryPartitionConfig {
        field,
        data_type,
        __inner__: inner,
        copy_partitions,
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        cluster_from_recorded, new_loader, partition_from_recorded, relation_config_from_recorded,
        requires_full_refresh,
    };
    use crate::relation::bigquery::config::components;
    use crate::relation::bigquery::config::test_helpers::{TestTableConfig, make_local_config};
    use crate::relation::config_v2::{ComponentConfigChange, RelationConfig};
    use dbt_schemas::schemas::manifest::{
        BigqueryPartitionConfig, BigqueryPartitionConfigInner, TimeConfig,
    };
    use indexmap::IndexMap;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn partition_by_change_triggers_full_refresh() {
        let changes = IndexMap::from_iter([(
            components::partition_by::TYPE_NAME,
            ComponentConfigChange::Drop,
        )]);
        assert!(requires_full_refresh(&changes));
    }

    #[test]
    fn cluster_by_change_triggers_full_refresh() {
        let changes = IndexMap::from_iter([(
            components::cluster_by::TYPE_NAME,
            ComponentConfigChange::Drop,
        )]);
        assert!(requires_full_refresh(&changes));
    }

    #[test]
    fn option_changes_do_not_trigger_full_refresh() {
        let changes = IndexMap::from_iter([
            (
                components::description::TYPE_NAME,
                ComponentConfigChange::Drop,
            ),
            (components::kms_key::TYPE_NAME, ComponentConfigChange::Drop),
            (components::labels::TYPE_NAME, ComponentConfigChange::Drop),
            (components::refresh::TYPE_NAME, ComponentConfigChange::Drop),
            (components::tags::TYPE_NAME, ComponentConfigChange::Drop),
        ]);
        assert!(!requires_full_refresh(&changes));
    }

    #[test]
    fn reconstruct_cluster_config() {
        let desired = new_loader()
            .from_local_config(&make_local_config(TestTableConfig {
                cluster_by: &["id", "name"],
                ..Default::default()
            }))
            .unwrap();

        let recorded = json!({
            "options": {},
            "partition": null,
            "cluster": { "fields": ["id", "name"] },
        });
        let current = relation_config_from_recorded(Some(&recorded)).unwrap();

        assert!(RelationConfig::diff(&desired, &current).is_empty());
    }

    #[test]
    fn reconstruct_time_partition_config() {
        let partition_by = BigqueryPartitionConfig {
            field: "created_at".to_string(),
            data_type: "date".to_string(),
            __inner__: BigqueryPartitionConfigInner::Time(TimeConfig {
                granularity: "day".to_string(),
                time_ingestion_partitioning: false,
            }),
            copy_partitions: false,
        };
        let desired = new_loader()
            .from_local_config(&make_local_config(TestTableConfig {
                partition_by: Some(partition_by),
                ..Default::default()
            }))
            .unwrap();

        let recorded = json!({
            "options": {},
            "partition": {
                "field": "created_at",
                "data_type": "date",
                "granularity": "day",
                "range": null,
                "time_ingestion_partitioning": false,
                "copy_partitions": false,
            },
            "cluster": null,
        });
        let current = relation_config_from_recorded(Some(&recorded)).unwrap();

        assert!(RelationConfig::diff(&desired, &current).is_empty());
    }

    #[test]
    fn reconstruct_range_partition_config() {
        let recorded = json!({
            "partition": {
                "field": "id",
                "data_type": "int64",
                "range": { "start": 0, "end": 100, "interval": 10 },
                "copy_partitions": false,
            },
        });
        let partition = partition_from_recorded(&recorded).unwrap().unwrap();
        assert_eq!(partition.field, "id");
        match partition.__inner__ {
            BigqueryPartitionConfigInner::Range(range) => {
                assert_eq!(range.range.start, 0);
                assert_eq!(range.range.end, 100);
                assert_eq!(range.range.interval, 10);
            }
            other => panic!("expected a range partition, got {other:?}"),
        }
    }

    #[test]
    fn reconstruct_errors_on_absent_payload() {
        assert!(relation_config_from_recorded(None).is_err());
    }

    #[test]
    fn reconstruct_errors_on_missing_options() {
        assert!(relation_config_from_recorded(Some(&json!({}))).is_err());
    }

    #[test]
    fn reconstruct_errors_on_malformed_cluster() {
        let recorded = json!({ "options": {}, "cluster": { "fields": "not-an-array" } });
        assert!(relation_config_from_recorded(Some(&recorded)).is_err());
    }

    #[test]
    fn reconstruct_errors_on_malformed_partition_by() {
        let recorded = json!({ "options": {}, "partition": { "field": "id", "data_type": "int64", "range": { "bogus": "field" }, "copy_partitions": false, } });
        assert!(relation_config_from_recorded(Some(&recorded)).is_err());
    }

    #[test]
    fn reconstruct_defaults_on_empty_cluster() {
        assert!(cluster_from_recorded(&json!({})).unwrap().is_empty());
        assert!(
            cluster_from_recorded(&json!({ "cluster": null }))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn reconstruct_options_config() {
        let desired = new_loader()
            .from_local_config(&make_local_config(TestTableConfig {
                description: "a non-default description",
                labels: HashMap::from([("env", "test"), ("team", "data")]),
                tags: HashMap::from([("test/resource_tag", "test1")]),
                kms_key: "my-kms-key",
                ..Default::default()
            }))
            .unwrap();

        let recorded = json!({
            "options": {
                "description": "a non-default description",
                "labels": { "env": "test", "team": "data" },
                "tags": { "test/resource_tag": "test1" },
                "kms_key_name": "my-kms-key",
            },
            "partition": null,
            "cluster": null,
        });
        let current = relation_config_from_recorded(Some(&recorded)).unwrap();

        assert!(RelationConfig::diff(&desired, &current).is_empty());
    }

    #[test]
    fn reconstruct_refresh_options_match_local_config() {
        let desired = new_loader()
            .from_local_config(&make_local_config(TestTableConfig {
                enable_refresh: Some(false),
                refresh_interval_minutes: 60.0,
                max_staleness: "INTERVAL 8 HOUR",
                ..Default::default()
            }))
            .unwrap();

        let recorded = json!({
            "options": {
                "enable_refresh": false,
                "refresh_interval_minutes": 60.0,
                "max_staleness": "INTERVAL 8 HOUR",
            },
            "partition": null,
            "cluster": null,
        });
        let current = relation_config_from_recorded(Some(&recorded)).unwrap();

        assert!(RelationConfig::diff(&desired, &current).is_empty());
    }

    // Both Fusion and Core allow you to configure both a range and a granularity
    // at the same time. If a range is configured, the granularity key gets ignored.
    #[test]
    fn reconstruct_partition_config_prefers_range() {
        let recorded = json!({
            "partition": {
                "field": "id",
                "data_type": "int64",
                "range": { "start": 0, "end": 100, "interval": 10 },
                "granularity": "day",
                "copy_partitions": false,
            },
        });
        let partition = partition_from_recorded(&recorded).unwrap().unwrap();
        assert_eq!(partition.field, "id");
        match partition.__inner__ {
            BigqueryPartitionConfigInner::Range(range) => {
                assert_eq!(range.range.start, 0);
                assert_eq!(range.range.end, 100);
                assert_eq!(range.range.interval, 10);
            }
            other => panic!("expected a range partition, got {other:?}"),
        }
    }
}
