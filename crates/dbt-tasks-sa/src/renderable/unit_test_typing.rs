use arrow_schema::{DataType, Field, FieldRef, TimeUnit};
use std::ops::Deref;
use std::sync::{Arc, LazyLock};

// --- TimePrecision (inlined from sdf-functions-sdk::datetime) ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TimePrecision(u8);

impl TimePrecision {
    pub fn new_valid(valid_precision: u8) -> Self {
        assert!(
            valid_precision <= 9,
            "Time precision must be between 0 and 9: {valid_precision}"
        );
        TimePrecision(valid_precision)
    }

    pub fn value(&self) -> u8 {
        self.0
    }

    pub fn suitable_time_unit(&self) -> TimeUnit {
        match self.value() {
            0 => TimeUnit::Second,
            1..=3 => TimeUnit::Millisecond,
            4..=9 => TimeUnit::Microsecond,
            _ => unreachable!(),
        }
    }
}

// --- IsTimestamp (inlined from sdf-functions-sdk::snowflake::timestamp) ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsTimestamp {
    No,
    Yes(TimePrecision),
}

impl IsTimestamp {
    pub fn is_yes(&self) -> bool {
        matches!(self, IsTimestamp::Yes(_))
    }
}

// --- timestamp type checkers (inlined from sdf-functions-sdk::snowflake::timestamp) ---

fn is_snowflake_timestamp_ntz_type(data_type: &DataType) -> IsTimestamp {
    match data_type {
        DataType::FixedSizeList(field, 1) if field.name().starts_with("timestamp_ntz:") => {
            IsTimestamp::Yes(TimePrecision::new_valid(
                field
                    .name()
                    .strip_prefix("timestamp_ntz:")
                    .expect("string prefix checked")
                    .parse::<u8>()
                    .expect("invalid serialized timestamp precision"),
            ))
        }
        _ => IsTimestamp::No,
    }
}

fn is_snowflake_timestamp_ltz_type(data_type: &DataType) -> IsTimestamp {
    match data_type {
        DataType::FixedSizeList(field, 1) if field.name().starts_with("timestamp_ltz:") => {
            IsTimestamp::Yes(TimePrecision::new_valid(
                field
                    .name()
                    .strip_prefix("timestamp_ltz:")
                    .expect("string prefix checked")
                    .parse::<u8>()
                    .expect("invalid serialized timestamp precision"),
            ))
        }
        _ => IsTimestamp::No,
    }
}

fn is_snowflake_timestamp_tz_type(data_type: &DataType) -> IsTimestamp {
    match data_type {
        DataType::FixedSizeList(field, 1) if field.name().starts_with("timestamp_tz:") => {
            IsTimestamp::Yes(TimePrecision::new_valid(
                field
                    .name()
                    .strip_prefix("timestamp_tz:")
                    .expect("string prefix checked")
                    .parse::<u8>()
                    .expect("invalid serialized timestamp precision"),
            ))
        }
        _ => IsTimestamp::No,
    }
}

// --- variant/object helpers (inlined from sdf-functions-sdk::snowflake::variant) ---

fn snowflake_variant_fixed_size_list_field() -> FieldRef {
    static VARIANT_FIELD: LazyLock<FieldRef> =
        LazyLock::new(|| Arc::new(Field::new("variant", DataType::Binary, true)));
    Arc::clone(VARIANT_FIELD.deref())
}

fn snowflake_object_fixed_size_list_field() -> FieldRef {
    static OBJECT_FIELD: LazyLock<FieldRef> =
        LazyLock::new(|| Arc::new(Field::new("object", DataType::Binary, true)));
    Arc::clone(OBJECT_FIELD.deref())
}

fn snowflake_variant_type() -> DataType {
    DataType::FixedSizeList(snowflake_variant_fixed_size_list_field(), 1)
}

fn snowflake_object_type() -> DataType {
    DataType::FixedSizeList(snowflake_object_fixed_size_list_field(), 1)
}

// --- BigqueryTyping ---

#[non_exhaustive]
pub struct BigqueryTyping {}

impl BigqueryTyping {
    pub fn numeric() -> DataType {
        DataType::Decimal128(38, 9)
    }

    pub fn is_geography(data_type: &DataType) -> bool {
        matches!(data_type, DataType::FixedSizeList(field, 1) if field.name() == "geography")
    }

    pub fn is_json(data_type: &DataType) -> bool {
        matches!(data_type, DataType::FixedSizeList(field, 1) if field.name() == "json")
    }
}

// --- SnowflakeTyping ---

#[non_exhaustive]
pub struct SnowflakeTyping {}

impl SnowflakeTyping {
    pub fn is_geography(data_type: &DataType) -> bool {
        matches!(data_type, DataType::FixedSizeList(field, 1) if field.name() == "geography")
    }

    pub fn is_geometry(data_type: &DataType) -> bool {
        matches!(data_type, DataType::FixedSizeList(field, 1) if field.name() == "geometry")
    }

    pub fn is_any_timestamp(data_type: &DataType) -> IsTimestamp {
        if let yes @ IsTimestamp::Yes(_) = is_snowflake_timestamp_ntz_type(data_type) {
            return yes;
        }
        if let yes @ IsTimestamp::Yes(_) = is_snowflake_timestamp_ltz_type(data_type) {
            return yes;
        }
        if let yes @ IsTimestamp::Yes(_) = is_snowflake_timestamp_tz_type(data_type) {
            return yes;
        }
        IsTimestamp::No
    }

    pub fn is_time(data_type: &DataType) -> IsTimestamp {
        match data_type {
            DataType::FixedSizeList(field, 1) if field.name().starts_with("time:") => {
                IsTimestamp::Yes(TimePrecision::new_valid(
                    field
                        .name()
                        .strip_prefix("time:")
                        .expect("string prefix checked")
                        .parse::<u8>()
                        .expect("invalid serialized time precision"),
                ))
            }
            _ => IsTimestamp::No,
        }
    }

    pub fn is_variant(data_type: &DataType) -> bool {
        matches!(data_type, DataType::FixedSizeList(field, 1) if field.name() == snowflake_variant_fixed_size_list_field().name())
    }

    pub fn variant() -> DataType {
        snowflake_variant_type()
    }

    pub fn is_object(data_type: &DataType) -> bool {
        matches!(data_type, DataType::FixedSizeList(field, 1) if field.name() == snowflake_object_fixed_size_list_field().name())
    }

    pub fn object() -> DataType {
        snowflake_object_type()
    }

    pub fn is_semi_structured_array(data_type: &DataType) -> bool {
        matches!(data_type, DataType::List(field) if field.name() == "item" && Self::is_variant(field.data_type()))
    }
}

// --- DatabricksTyping ---

#[non_exhaustive]
pub struct DatabricksTyping {}

impl DatabricksTyping {
    pub fn is_timestamp_ntz(data_type: &DataType) -> bool {
        matches!(data_type, DataType::FixedSizeList(field, 1) if field.name() == "timestamp_ntz")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_databricks_is_timestamp_ntz_matches_bare_name() {
        let field = Arc::new(Field::new(
            "timestamp_ntz",
            DataType::Timestamp(TimeUnit::Nanosecond, None),
            true,
        ));
        let data_type = DataType::FixedSizeList(field, 1);
        assert!(DatabricksTyping::is_timestamp_ntz(&data_type));
    }

    #[test]
    fn test_databricks_is_timestamp_ntz_rejects_plain_timestamp() {
        let data_type = DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()));
        assert!(!DatabricksTyping::is_timestamp_ntz(&data_type));
    }

    #[test]
    fn test_databricks_is_timestamp_ntz_rejects_other_distinct_types() {
        let field = Arc::new(Field::new("geography", DataType::Utf8, true));
        let data_type = DataType::FixedSizeList(field, 1);
        assert!(!DatabricksTyping::is_timestamp_ntz(&data_type));
    }

    #[test]
    fn test_databricks_is_timestamp_ntz_rejects_non_length_one_list() {
        let field = Arc::new(Field::new(
            "timestamp_ntz",
            DataType::Timestamp(TimeUnit::Nanosecond, None),
            true,
        ));
        let data_type = DataType::FixedSizeList(field, 2);
        assert!(!DatabricksTyping::is_timestamp_ntz(&data_type));
    }
}
