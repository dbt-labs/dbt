use dbt_common::current_function_name;
use dbt_yaml::{DbtSchema, UntaggedEnumDeserialize};
use minijinja::value::Enumerator;
use minijinja::{Error as MinijinjaError, ErrorKind as MinijinjaErrorKind, State};
use minijinja::{
    arg_utils::ArgParser,
    listener::RenderingEventListener,
    value::{Object, Value as MinijinjaValue},
};
use serde::{Deserialize, Serialize};
use strum::{AsRefStr, Display, EnumIter, EnumString, IntoEnumIterator};

use std::convert::AsRef;
use std::{rc::Rc, sync::Arc};

/// reference: https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-bigquery/src/dbt/adapters/bigquery/relation_configs/_partition.py#L12-L13
#[derive(Debug, Clone, Serialize, Deserialize, Eq, DbtSchema)]
pub struct BigqueryPartitionConfig {
    pub field: String,
    #[serde(default = "BigqueryPartitionConfig::default_data_type")]
    pub data_type: String,
    #[serde(flatten)]
    pub __inner__: BigqueryPartitionConfigInner,
    #[serde(
        default,
        deserialize_with = "crate::schemas::serde::yaml_11_bool_default"
    )]
    pub copy_partitions: bool,
}

/// reference: https://github.com/dbt-labs/dbt-adapters/blob/c16cc7047e8678f8bb88ae294f43da2c68e9f5cc/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L503
impl PartialEq for BigqueryPartitionConfig {
    fn eq(&self, other: &Self) -> bool {
        // Both are partitioned, check details
        match (&self.__inner__, &other.__inner__) {
            (
                BigqueryPartitionConfigInner::Time(self_time_part),
                BigqueryPartitionConfigInner::Time(other_time_part),
            ) => {
                if self_time_part.granularity.to_lowercase()
                    != other_time_part.granularity.to_lowercase()
                {
                    return false;
                }

                let self_field_str = &self.field;
                let other_config_field_str = &other.field;

                let bq_self_field_was_none = self_field_str.is_empty();
                let part1 = if bq_self_field_was_none {
                    false
                } else {
                    self_field_str.to_lowercase() == other_config_field_str.to_lowercase()
                };
                let part2 = other.time_ingestion_partitioning() && !bq_self_field_was_none;

                part1 || part2
            }
            (
                BigqueryPartitionConfigInner::Range(self_range_part),
                BigqueryPartitionConfigInner::Range(other_range_part),
            ) => {
                self.field.to_lowercase() == other.field.to_lowercase()
                    && self_range_part.range == other_range_part.range
            }
            _ => false, // Mismatch: one is Time, other is Range
        }
    }
}

/// Enum representing all field names in BigqueryPartitionConfig
#[derive(Debug, Clone, EnumString, Display, EnumIter, AsRefStr)]
#[strum(serialize_all = "snake_case")]
enum PartitionConfigField {
    Field,
    DataType,
    TimeIngestionPartitioning,
    CopyPartitions,
    // Flattened field for TimeConfig
    Granularity,
    // Flattened field for RangeConfig
    Range,
}

#[derive(Debug, Clone, Serialize, UntaggedEnumDeserialize, PartialEq, Eq, DbtSchema)]
#[serde(untagged)]
pub enum BigqueryPartitionConfigInner {
    Range(RangeConfig),
    Time(TimeConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
pub struct TimeConfig {
    #[serde(default = "BigqueryPartitionConfig::default_granularity")]
    pub granularity: String,
    /// When this is true, the [`BigqueryPartitionConfig::field`] will be used as the `_PARTITIONTIME` pseudo column
    /// _PARTITIONTIME: https://cloud.google.com/bigquery/docs/partitioned-tables#ingestion_time
    /// https://docs.getdbt.com/reference/resource-configs/bigquery-configs#partitioning-by-an-ingestion-date-or-timestamp
    #[serde(
        default,
        deserialize_with = "crate::schemas::serde::yaml_11_bool_default"
    )]
    pub time_ingestion_partitioning: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
pub struct RangeConfig {
    pub range: Range,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, DbtSchema)]
pub struct Range {
    pub start: i64,
    pub end: i64,
    pub interval: i64,
}

impl BigqueryPartitionConfig {
    const PARTITION_DATE: &str = "_PARTITIONDATE";
    pub const PARTITION_TIME: &str = "_PARTITIONTIME";

    pub fn time_ingestion_partitioning(&self) -> bool {
        match &self.__inner__ {
            BigqueryPartitionConfigInner::Time(TimeConfig {
                time_ingestion_partitioning,
                ..
            }) => *time_ingestion_partitioning,
            BigqueryPartitionConfigInner::Range(_) => false,
        }
    }

    pub fn granularity(&self) -> Result<String, MinijinjaError> {
        match &self.__inner__ {
            BigqueryPartitionConfigInner::Time(TimeConfig { granularity, .. }) => {
                Ok(granularity.to_string())
            }
            BigqueryPartitionConfigInner::Range(_) => Err(MinijinjaError::new(
                MinijinjaErrorKind::InvalidArgument,
                "RangeConfig does not have a granularity",
            )),
        }
    }

    pub fn range(&self) -> Result<Range, MinijinjaError> {
        match &self.__inner__ {
            BigqueryPartitionConfigInner::Range(RangeConfig { range }) => Ok(range.clone()),
            BigqueryPartitionConfigInner::Time(_) => Err(MinijinjaError::new(
                MinijinjaErrorKind::InvalidArgument,
                "TimeConfig does not have a range",
            )),
        }
    }

    pub fn default_data_type() -> String {
        "date".to_string()
    }

    pub fn default_granularity() -> String {
        "day".to_string()
    }

    /// Return the data type of partitions for replacement.
    /// When time_ingestion_partitioning is enabled, the data type supported are date & timestamp.
    pub fn data_type_for_partition(&self) -> Result<MinijinjaValue, MinijinjaError> {
        let data_type = if !self.time_ingestion_partitioning() || self.data_type == "date" {
            self.data_type.as_str()
        } else {
            "timestamp"
        };
        Ok(MinijinjaValue::from(data_type))
    }

    pub fn reject_partition_field_column(
        &self,
        args: &[MinijinjaValue],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        let mut parser = ArgParser::new(args, None);
        parser.check_num_args(current_function_name!(), 0, 1)?;

        let columns = parser.get::<MinijinjaValue>("columns")?;
        if let Ok(iter) = columns.try_iter() {
            let columns = iter
                .filter(|c| {
                    let name = c
                        .get_attr("name")
                        .expect("column must have a name attribute");
                    !name
                        .as_str()
                        .expect("name attribute must be a string")
                        .eq_ignore_ascii_case(self.field.as_str())
                })
                .collect::<Vec<_>>();
            Ok(MinijinjaValue::from(columns))
        } else {
            Err(MinijinjaError::new(
                MinijinjaErrorKind::InvalidArgument,
                "columns must be a list of Column",
            ))
        }
    }

    /// Return true if the data type should be truncated instead of cast to the data type
    pub fn data_type_should_be_truncated(&self) -> bool {
        !(self.data_type == "int64"
            || (self.data_type == "date"
                && match &self.__inner__ {
                    BigqueryPartitionConfigInner::Time(TimeConfig { granularity, .. }) => {
                        granularity == "day"
                    }
                    BigqueryPartitionConfigInner::Range(_) => {
                        unreachable!("when data_type is date, inner must be a TimeConfig")
                    }
                }))
    }

    /// Return the time partitioning field name based on the data type.
    /// The default is _PARTITIONTIME, but for date it is _PARTITIONDATE
    pub fn time_partitioning_field(&self) -> Result<MinijinjaValue, MinijinjaError> {
        let field = if self.data_type == "date" {
            Self::PARTITION_DATE
        } else {
            Self::PARTITION_TIME
        };
        Ok(MinijinjaValue::from(field))
    }

    /// Return the insertable time partitioning field name based on the data type.
    /// Practically, only _PARTITIONTIME works so far.
    pub fn insertable_time_partitioning_field(&self) -> Result<MinijinjaValue, MinijinjaError> {
        Ok(MinijinjaValue::from(Self::PARTITION_TIME))
    }

    /// Render the partition expression
    pub fn render(&self, alias: Option<String>) -> Result<MinijinjaValue, MinijinjaError> {
        let column = if !self.time_ingestion_partitioning() {
            self.field.to_owned()
        } else {
            self.time_partitioning_field()?
                .as_str()
                .expect("time_partitioning_field must be a string")
                .to_owned()
        };

        let column = if let Some(alias) = &alias {
            format!("{alias}.{column}")
        } else {
            column
        };

        let result = if self.data_type_should_be_truncated() {
            format!(
                "{}_trunc({}, {})",
                self.data_type,
                column,
                self.granularity()?
            )
        } else {
            column
        };

        Ok(MinijinjaValue::from(result))
    }

    fn get_alias(&self, args: &[MinijinjaValue]) -> Result<Option<String>, MinijinjaError> {
        let mut parser = ArgParser::new(args, None);
        parser.check_num_args(current_function_name!(), 0, 1)?;

        let alias = parser
            .get_optional::<MinijinjaValue>("alias")
            .map(|a| {
                a.as_str().map(String::from).ok_or_else(|| {
                    MinijinjaError::new(
                        MinijinjaErrorKind::InvalidArgument,
                        "alias must be a string",
                    )
                })
            })
            .transpose()?;

        Ok(alias)
    }

    // Because this method will be used under a `impl Object` trait block,
    // and a method named `render` with a default impl already exists there,
    // a different name must be chosen here to make sure self.render resolves to the correct method
    pub fn render_(&self, args: &[MinijinjaValue]) -> Result<MinijinjaValue, MinijinjaError> {
        let alias = self.get_alias(args)?;

        self.render(alias)
    }

    /// Wrap the partitioning column when time involved to ensure it is properly cast to matching time
    pub fn render_wrapped(
        &self,
        args: &[MinijinjaValue],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        let alias = self.get_alias(args)?;

        // The partition key for RANGE [start, end) partitioned tables is always "start",
        if self.data_type == "int64" {
            if let Ok(range) = self.range() {
                let column = self.render(alias)?;
                let column = column.as_str().unwrap();
                return Ok(MinijinjaValue::from(format!(
                    "({column} - MOD({column} - {}, {}))",
                    range.start, range.interval
                )));
            }
        }

        if (self.data_type == "date"
            || self.data_type == "timestamp"
            || self.data_type == "datetime")
            && !self.data_type_should_be_truncated()
            && !(self.time_ingestion_partitioning() && self.data_type == "date")
        {
            Ok(MinijinjaValue::from(format!(
                "{}({})",
                self.data_type,
                self.render(alias)?.as_str().unwrap()
            )))
        } else {
            self.render(alias)
        }
    }
}

impl Object for BigqueryPartitionConfig {
    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        let key_str = key.as_str()?;
        let field = PartitionConfigField::try_from(key_str).ok()?;

        match field {
            PartitionConfigField::Field => Some(MinijinjaValue::from(self.field.clone())),
            PartitionConfigField::DataType => Some(MinijinjaValue::from(self.data_type.clone())),
            PartitionConfigField::Granularity => self.granularity().map(MinijinjaValue::from).ok(),
            PartitionConfigField::TimeIngestionPartitioning => {
                Some(MinijinjaValue::from(self.time_ingestion_partitioning()))
            }
            PartitionConfigField::CopyPartitions => {
                Some(MinijinjaValue::from(self.copy_partitions))
            }
            PartitionConfigField::Range => self.range().map(MinijinjaValue::from_serialize).ok(),
        }
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        args: &[MinijinjaValue],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        match name {
            "data_type_for_partition" => self.data_type_for_partition(),
            "reject_partition_field_column" => self.reject_partition_field_column(args),
            "time_partitioning_field" => self.time_partitioning_field(),
            "render" => self.render_(args),
            "render_wrapped" => self.render_wrapped(args),
            "insertable_time_partitioning_field" => self.insertable_time_partitioning_field(),
            _ => Err(MinijinjaError::new(
                MinijinjaErrorKind::UnknownMethod,
                format!("Unknown method on PartitionConfig object: '{name}'"),
            )),
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        let fields = PartitionConfigField::iter()
            .map(|f| MinijinjaValue::from(f.as_ref()))
            .collect::<Vec<_>>();
        Enumerator::Iter(Box::new(fields.into_iter()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq, DbtSchema)]
pub struct GrantAccessToTarget {
    pub dataset: Option<String>,
    pub project: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_time_partition_config() {
        let json = dbt_yaml::from_str(
            r#"
            field: created_at
            data_type: timestamp
            granularity: hour
        "#,
        )
        .unwrap();

        let config: BigqueryPartitionConfig = dbt_yaml::from_value(json).unwrap();
        assert!(matches!(
            config.__inner__,
            BigqueryPartitionConfigInner::Time(_)
        ));
    }

    /// The `partition_by` booleans hit the same YAML 1.1 divergence as `contract.enforced`, and
    /// `time_ingestion_partitioning` fails through the untagged enum.
    #[test]
    fn test_partition_config_resolves_yaml_11_boolean_tokens() {
        for (token, expected) in [("no", false), ("off", false), ("yes", true), ("on", true)] {
            let json = dbt_yaml::from_str(&format!(
                "field: created_at\ndata_type: timestamp\ncopy_partitions: {token}\ntime_ingestion_partitioning: {token}\n"
            ))
            .unwrap();
            let config: BigqueryPartitionConfig = dbt_yaml::from_value(json).unwrap();
            assert_eq!(config.copy_partitions, expected, "token: {token}");
            assert_eq!(
                config.time_ingestion_partitioning(),
                expected,
                "token: {token}"
            );
        }

        // Asserted on `copy_partitions`, which errors precisely; a bad
        // `time_ingestion_partitioning` errors too, but through the untagged-enum wrapper.
        let json = dbt_yaml::from_str("field: created_at\ncopy_partitions: maybe\n").unwrap();
        let result: Result<BigqueryPartitionConfig, _> = dbt_yaml::from_value(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_range_partition_config() {
        let json = dbt_yaml::from_str(
            r#"
            field: "user_id"
            data_type: "int64"
            range:
                start: 0
                end: 100
                interval: 10
        "#,
        )
        .unwrap();

        let config: BigqueryPartitionConfig = dbt_yaml::from_value(json).unwrap();
        assert!(matches!(
            config.__inner__,
            BigqueryPartitionConfigInner::Range(_)
        ));
        assert!(!config.time_ingestion_partitioning());
        assert!(!config.copy_partitions);
    }

    #[test]
    fn test_deserialize_with_defaults() {
        let json = dbt_yaml::from_str(
            r#"
            field: created_at
        "#,
        )
        .unwrap();

        let config: BigqueryPartitionConfig = dbt_yaml::from_value(json).unwrap();
        assert_eq!(config.field, "created_at");
        assert_eq!(config.data_type, "date"); // default
        assert!(
            matches!(config.__inner__, BigqueryPartitionConfigInner::Time(TimeConfig { ref granularity, .. }) if granularity == "day")
        ); // default
        assert!(!config.time_ingestion_partitioning()); // default
        assert!(!config.copy_partitions); // default
    }

    #[test]
    fn test_partition_config_field_enum_covers_all_fields_time_config() {
        // Create a sample config with time partitioning
        let config = BigqueryPartitionConfig {
            field: "test_field".to_string(),
            data_type: "date".to_string(),
            __inner__: BigqueryPartitionConfigInner::Time(TimeConfig {
                granularity: "day".to_string(),
                time_ingestion_partitioning: false,
            }),
            copy_partitions: false,
        };

        // Serialize to JSON to get all field names
        let json_value = dbt_yaml::to_value(&config).unwrap();
        let json_object = json_value.as_mapping().unwrap();

        // Test that all JSON fields can be parsed by our enum (except flattened fields)
        for field_name in json_object.keys() {
            assert!(
                PartitionConfigField::try_from(
                    field_name.as_str().expect("field_name should be a string")
                )
                .is_ok(),
                "Field '{field_name:?}' should be parseable by PartitionConfigField enum"
            );
        }

        // Test that a bogus field is rejected
        assert!(PartitionConfigField::try_from("invalid_field").is_err());
    }

    #[test]
    fn test_partition_config_field_enum_covers_all_fields_range_config() {
        // Create a sample config with range partitioning
        let config = BigqueryPartitionConfig {
            field: "user_id".to_string(),
            data_type: "int64".to_string(),
            __inner__: BigqueryPartitionConfigInner::Range(RangeConfig {
                range: Range {
                    start: 0,
                    end: 100,
                    interval: 10,
                },
            }),
            copy_partitions: true,
        };

        // Serialize to JSON to get all field names
        let json_value = dbt_yaml::to_value(&config).unwrap();
        let json_object = json_value.as_mapping().unwrap();

        // Test that all JSON fields can be parsed by our enum (except nested range fields)
        for field_name in json_object.keys() {
            assert!(
                PartitionConfigField::try_from(
                    field_name.as_str().expect("field_name should be a string")
                )
                .is_ok(),
                "Field '{field_name:?}' should be parseable by PartitionConfigField enum"
            );
        }
    }

    #[test]
    fn test_get_value_returns_correct_values() {
        // Test with time config
        let time_config = Arc::new(BigqueryPartitionConfig {
            field: "created_at".to_string(),
            data_type: "timestamp".to_string(),
            __inner__: BigqueryPartitionConfigInner::Time(TimeConfig {
                granularity: "hour".to_string(),
                time_ingestion_partitioning: true,
            }),
            copy_partitions: false,
        });

        // Test field values
        assert_eq!(
            time_config
                .get_value(&MinijinjaValue::from("field"))
                .unwrap(),
            MinijinjaValue::from("created_at")
        );
        assert_eq!(
            time_config
                .get_value(&MinijinjaValue::from("data_type"))
                .unwrap(),
            MinijinjaValue::from("timestamp")
        );
        assert_eq!(
            time_config
                .get_value(&MinijinjaValue::from("granularity"))
                .unwrap(),
            MinijinjaValue::from("hour")
        );
        assert_eq!(
            time_config
                .get_value(&MinijinjaValue::from("time_ingestion_partitioning"))
                .unwrap(),
            MinijinjaValue::from(true)
        );
        assert_eq!(
            time_config
                .get_value(&MinijinjaValue::from("copy_partitions"))
                .unwrap(),
            MinijinjaValue::from(false)
        );

        // Test with range config (granularity should return None)
        let range_config = Arc::new(BigqueryPartitionConfig {
            field: "user_id".to_string(),
            data_type: "int64".to_string(),
            __inner__: BigqueryPartitionConfigInner::Range(RangeConfig {
                range: Range {
                    start: 0,
                    end: 100,
                    interval: 10,
                },
            }),
            copy_partitions: true,
        });

        assert_eq!(
            range_config
                .get_value(&MinijinjaValue::from("field"))
                .unwrap(),
            MinijinjaValue::from("user_id")
        );
        assert_eq!(
            range_config
                .get_value(&MinijinjaValue::from("data_type"))
                .unwrap(),
            MinijinjaValue::from("int64")
        );
        // granularity should return None for range config
        assert!(
            range_config
                .get_value(&MinijinjaValue::from("granularity"))
                .is_none()
        );
        assert_eq!(
            range_config
                .get_value(&MinijinjaValue::from("copy_partitions"))
                .unwrap(),
            MinijinjaValue::from(true)
        );

        // Test that invalid fields return None
        assert!(
            time_config
                .get_value(&MinijinjaValue::from("invalid_field"))
                .is_none()
        );
        assert!(
            range_config
                .get_value(&MinijinjaValue::from("invalid_field"))
                .is_none()
        );
    }

    #[test]
    fn test_deserialize_inner() {
        let inner_time_json = dbt_yaml::from_str(
            r#"
                {
                    "granularity": "day",
                    "time_ingestion_partitioning": false
                }
            "#,
        )
        .unwrap();
        let inner_time_config: BigqueryPartitionConfigInner =
            dbt_yaml::from_value(inner_time_json).unwrap();
        assert!(
            matches!(inner_time_config, BigqueryPartitionConfigInner::Time(TimeConfig { granularity, time_ingestion_partitioning: false }) if granularity == "day")
        );

        let inner_range_json = dbt_yaml::from_str(
            r#"
            {
                "range":
                {
                    "start": 0,
                    "end": 2,
                    "interval": 1
                }
            }
            "#,
        )
        .unwrap();
        let inner_range_config: BigqueryPartitionConfigInner =
            dbt_yaml::from_value(inner_range_json).unwrap();
        assert!(matches!(
            inner_range_config,
            BigqueryPartitionConfigInner::Range(RangeConfig {
                range: Range {
                    start: 0,
                    end: 2,
                    interval: 1,
                }
            })
        ));
    }

    fn time_config(
        field: &str,
        data_type: &str,
        granularity: &str,
        time_ingestion_partitioning: bool,
    ) -> BigqueryPartitionConfig {
        BigqueryPartitionConfig {
            field: field.to_string(),
            data_type: data_type.to_string(),
            __inner__: BigqueryPartitionConfigInner::Time(TimeConfig {
                granularity: granularity.to_string(),
                time_ingestion_partitioning,
            }),
            copy_partitions: false,
        }
    }

    fn range_config(field: &str, start: i64, end: i64, interval: i64) -> BigqueryPartitionConfig {
        BigqueryPartitionConfig {
            field: field.to_string(),
            data_type: "int64".to_string(),
            __inner__: BigqueryPartitionConfigInner::Range(RangeConfig {
                range: Range {
                    start,
                    end,
                    interval,
                },
            }),
            copy_partitions: false,
        }
    }

    fn rendered_wrapped(config: &BigqueryPartitionConfig, alias: Option<&str>) -> String {
        let args: Vec<MinijinjaValue> = match alias {
            Some(a) => vec![MinijinjaValue::from(a)],
            None => vec![],
        };
        config
            .render_wrapped(&args)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn test_render_wrapped_range_int64() {
        let config = range_config("user_id", 0, 100, 10);
        assert_eq!(
            rendered_wrapped(&config, None),
            "(user_id - MOD(user_id - 0, 10))"
        );
    }

    #[test]
    fn test_render_wrapped_range_int64_with_alias() {
        let config = range_config("user_id", 5, 100, 10);
        assert_eq!(
            rendered_wrapped(&config, Some("t")),
            "(t.user_id - MOD(t.user_id - 5, 10))"
        );
    }

    #[test]
    fn test_render_wrapped_date_day_wraps_in_cast() {
        // date with day granularity is not truncated, so it is wrapped in date(...)
        let config = time_config("created_at", "date", "day", false);
        assert_eq!(rendered_wrapped(&config, None), "date(created_at)");
    }

    #[test]
    fn test_render_wrapped_date_day_with_alias() {
        let config = time_config("created_at", "date", "day", false);
        assert_eq!(rendered_wrapped(&config, Some("t")), "date(t.created_at)");
    }

    #[test]
    fn test_render_wrapped_date_month_is_truncated() {
        // date with non-day granularity is truncated, not wrapped
        let config = time_config("created_at", "date", "month", false);
        assert_eq!(
            rendered_wrapped(&config, None),
            "date_trunc(created_at, month)"
        );
    }

    #[test]
    fn test_render_wrapped_timestamp_is_truncated() {
        // timestamp is always truncated, never wrapped in a cast
        let config = time_config("created_at", "timestamp", "hour", false);
        assert_eq!(
            rendered_wrapped(&config, None),
            "timestamp_trunc(created_at, hour)"
        );
    }

    #[test]
    fn test_render_wrapped_datetime_is_truncated() {
        let config = time_config("created_at", "datetime", "day", false);
        assert_eq!(
            rendered_wrapped(&config, None),
            "datetime_trunc(created_at, day)"
        );
    }

    #[test]
    fn test_render_wrapped_date_time_ingestion_uses_partition_date() {
        // ingestion-time date partitioning renders the pseudo column without a cast
        let config = time_config("created_at", "date", "day", true);
        assert_eq!(rendered_wrapped(&config, None), "_PARTITIONDATE");
    }

    #[test]
    fn test_render_wrapped_timestamp_time_ingestion_uses_partition_time() {
        let config = time_config("created_at", "timestamp", "hour", true);
        assert_eq!(
            rendered_wrapped(&config, None),
            "timestamp_trunc(_PARTITIONTIME, hour)"
        );
    }

    #[test]
    fn test_serde_json_round_trip() {
        let original = BigqueryPartitionConfig {
            field: "valid_starting_at_week".to_string(),
            data_type: "date".to_string(),
            __inner__: BigqueryPartitionConfigInner::Time(TimeConfig {
                granularity: "day".to_string(),
                time_ingestion_partitioning: false,
            }),
            copy_partitions: false,
        };

        let json = serde_json::to_string(&original).unwrap();
        println!("{json}");

        let value = dbt_yaml::from_str(&json).unwrap();
        let roundtripped: BigqueryPartitionConfig = dbt_yaml::from_value(value).unwrap();

        assert_eq!(original, roundtripped);
    }
}
