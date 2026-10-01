use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use crate::relation::config_v2::{
    ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
};
use crate::value::none_value;

use arrow_schema::Schema;
use dbt_schemas::schemas::{
    DbtModel, InternalDbtNodeAttributes,
    manifest::{
        BigqueryPartitionConfig, BigqueryPartitionConfigInner, Range, RangeConfig, TimeConfig,
    },
};
use minijinja::value::Value;

pub(crate) const TYPE_NAME: &str = "partition";

/// Component for BigQuery partition configuration
pub(crate) type PartitionBy = SimpleComponentConfigImpl<Option<BigqueryPartitionConfig>>;

fn to_jinja(v: &Option<BigqueryPartitionConfig>) -> Value {
    v.clone().map(Value::from_object).unwrap_or_else(none_value)
}

fn new_component(cfg: Option<BigqueryPartitionConfig>) -> PartitionBy {
    PartitionBy {
        type_name: TYPE_NAME,
        diff_fn: diff::desired_state,
        to_jinja_fn: to_jinja,
        value: cfg,
    }
}

fn from_remote_state(schema: &Schema) -> AdapterResult<PartitionBy> {
    let metadata = &schema.metadata;
    let time_partition = if let Some(partition) = metadata.get("TimePartitioning.Field") {
        let field_name = partition.parse::<String>().unwrap();

        let data_type = schema
            .fields()
            .iter()
            .find(|field| field.name().eq_ignore_ascii_case(&field_name))
            .map(|field| {
                dbt_adapter_sql::types::original_type_string(
                    dbt_adapter_core::AdapterType::Bigquery,
                    field,
                )
                .expect("No 'BIGQUERY:type' in field metadata. This is a driver bug.")
                .to_string()
            })
            .expect("BigQuery returned invalid 'TimePartitioning.Field'. This is a driver bug.");

        Some(BigqueryPartitionConfig {
            field: field_name,
            data_type,
            __inner__: BigqueryPartitionConfigInner::Time(TimeConfig {
                granularity: metadata.get("TimePartitioning.Type").unwrap().to_string(),
                time_ingestion_partitioning: false,
            }),
            // TODO(serramatutu): how do we determine the value of this?
            copy_partitions: false,
        })
    } else {
        None
    };

    let range_partition = if let Some(partition) = metadata.get("RangePartitioning.Field") {
        let field = partition.parse::<String>().unwrap();

        Some(BigqueryPartitionConfig {
            field,
            data_type: "int64".to_string(),
            __inner__: BigqueryPartitionConfigInner::Range(RangeConfig {
                range: Range {
                    start: metadata
                        .get("RangePartitioning.Range.Start")
                        .map(|s| {
                            s.parse::<i64>()
                                .expect("Could not parse 'RangePartitioning.Range.Start' as i64")
                        })
                        .unwrap(),
                    end: metadata
                        .get("RangePartitioning.Range.End")
                        .map(|s| {
                            s.parse::<i64>()
                                .expect("Could not parse 'RangePartitioning.Range.End' as i64")
                        })
                        .unwrap(),
                    interval: metadata
                        .get("RangePartitioning.Range.Interval")
                        .map(|s| {
                            s.parse::<i64>()
                                .expect("Could not parse 'RangePartitioning.Range.Interval' as i64")
                        })
                        .unwrap(),
                },
            }),
            // TODO(serramatutu): how do we determine the value of this?
            copy_partitions: false,
        })
    } else {
        None
    };

    let partition_by = time_partition.or(range_partition);

    Ok(new_component(partition_by))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<PartitionBy> {
    let partition_by = match relation_config.as_any().downcast_ref::<DbtModel>() {
        None => None,
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
            .partition_by
            .as_ref()
            .and_then(|pb| pb.as_bigquery()),
    };

    Ok(new_component(partition_by.cloned()))
}

impl_loader!(PartitionBy, Schema);

impl PartitionByLoader {
    pub fn new_component_type_erased(
        partition_by: Option<BigqueryPartitionConfig>,
    ) -> Box<dyn ComponentConfig> {
        Box::new(new_component(partition_by))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::bigquery::config::test_helpers::{
        TestTableConfig, make_driver_data, make_local_config,
    };
    use dbt_adapter_core::AdapterType;

    #[test]
    fn from_remote_state_no_partition() {
        let driver_data = make_driver_data(Default::default());
        let loaded = from_remote_state(&driver_data).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_remote_state_with_time_partition() {
        let pb = BigqueryPartitionConfig {
            field: "my_field".to_string(),
            data_type: "DATETIME".to_string(),
            __inner__: BigqueryPartitionConfigInner::Time(TimeConfig {
                granularity: "DAY".to_string(),
                time_ingestion_partitioning: false,
            }),
            copy_partitions: false,
        };
        let driver_data = make_driver_data(TestTableConfig {
            partition_by: Some(pb.clone()),
            ..Default::default()
        });
        let loaded = from_remote_state(&driver_data).unwrap();
        assert_eq!(loaded.value, Some(pb))
    }

    #[test]
    fn from_remote_state_with_case_insensitive_time_partition_field() {
        let pb = BigqueryPartitionConfig {
            field: "date".to_string(),
            data_type: "DATE".to_string(),
            __inner__: BigqueryPartitionConfigInner::Time(TimeConfig {
                granularity: "DAY".to_string(),
                time_ingestion_partitioning: false,
            }),
            copy_partitions: false,
        };
        let driver_data = make_driver_data(TestTableConfig {
            partition_by: Some(pb.clone()),
            ..Default::default()
        });
        let fields = driver_data
            .fields()
            .iter()
            .map(|field| {
                let field = field.as_ref().clone();
                if field.name() == "date" {
                    std::sync::Arc::new(field.with_name("Date"))
                } else {
                    std::sync::Arc::new(field)
                }
            })
            .collect::<Vec<_>>();
        let driver_data = Schema::new_with_metadata(fields, driver_data.metadata().clone());

        let loaded = from_remote_state(&driver_data).unwrap();
        assert_eq!(loaded.value, Some(pb))
    }

    #[test]
    fn from_remote_state_with_range_partition() {
        let pb = BigqueryPartitionConfig {
            field: "my_field".to_string(),
            data_type: "INT64".to_string(),
            __inner__: BigqueryPartitionConfigInner::Range(RangeConfig {
                range: Range {
                    start: 0,
                    end: 10,
                    interval: 2,
                },
            }),
            copy_partitions: false,
        };
        let driver_data = make_driver_data(TestTableConfig {
            partition_by: Some(pb.clone()),
            ..Default::default()
        });
        let loaded = from_remote_state(&driver_data).unwrap();
        assert_eq!(loaded.value, Some(pb))
    }

    #[test]
    fn from_local_config_no_partition() {
        let local_data = make_local_config(Default::default());
        let loaded = from_local_config(&local_data).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_config_with_time_partition() {
        let pb = BigqueryPartitionConfig {
            field: "my_field".to_string(),
            data_type: "DATETIME".to_string(),
            __inner__: BigqueryPartitionConfigInner::Time(TimeConfig {
                granularity: "DAY".to_string(),
                time_ingestion_partitioning: false,
            }),
            copy_partitions: false,
        };
        let config = make_local_config(TestTableConfig {
            partition_by: Some(pb.clone()),
            ..Default::default()
        });
        let loaded = from_local_config(&config).unwrap();
        assert_eq!(loaded.value, Some(pb))
    }

    #[test]
    fn from_local_config_with_range_partition() {
        let pb = BigqueryPartitionConfig {
            field: "my_field".to_string(),
            data_type: "INT64".to_string(),
            __inner__: BigqueryPartitionConfigInner::Range(RangeConfig {
                range: Range {
                    start: 0,
                    end: 10,
                    interval: 2,
                },
            }),
            copy_partitions: false,
        };
        let config = make_local_config(TestTableConfig {
            partition_by: Some(pb.clone()),
            ..Default::default()
        });
        let loaded = from_local_config(&config).unwrap();
        assert_eq!(loaded.value, Some(pb))
    }
}
