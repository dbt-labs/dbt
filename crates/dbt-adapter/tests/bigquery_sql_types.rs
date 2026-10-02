use std::sync::Arc;

use arrow_schema::{DataType, Field, TimeUnit};
use dbt_adapter::sql_types::{DefaultTypeOps, TypeOps};
use dbt_adapter_core::AdapterType::Bigquery;

#[test]
fn test_bigquery_formats_decimal_by_supported_range() {
    let type_ops = DefaultTypeOps::new(Bigquery);

    for (data_type, expected) in [
        (DataType::Decimal128(38, 9), "NUMERIC"),
        (DataType::Decimal128(29, 0), "NUMERIC"),
        (DataType::Decimal128(30, 0), "BIGNUMERIC"),
        (DataType::Decimal128(30, 20), "BIGNUMERIC"),
        (DataType::Decimal256(10, 2), "NUMERIC"),
    ] {
        let mut formatted = String::new();
        type_ops
            .format_arrow_type_as_sql(&data_type, true, &mut formatted)
            .unwrap();
        assert_eq!(formatted, expected, "failed to format {data_type}");
    }
}

#[test]
fn test_bigquery_formats_nested_logical_types() {
    let geography =
        DataType::FixedSizeList(Arc::new(Field::new("geography", DataType::Utf8, true)), 1);
    let json = DataType::FixedSizeList(Arc::new(Field::new("json", DataType::Utf8, true)), 1);
    let data_type = DataType::Struct(
        vec![
            Field::new("location", geography, true),
            Field::new(
                "events",
                DataType::List(Arc::new(Field::new("item", json, true))),
                true,
            ),
            Field::new(
                "created_at",
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                true,
            ),
        ]
        .into(),
    );

    let mut out = String::new();
    DefaultTypeOps::new(Bigquery)
        .format_arrow_type_as_sql(&data_type, true, &mut out)
        .unwrap();
    assert_eq!(
        out,
        "STRUCT<location GEOGRAPHY, events ARRAY<JSON>, created_at TIMESTAMP>"
    );
}
