use std::sync::Arc;

use arrow_schema::{DataType, Field, TimeUnit};
use dbt_adapter::sql_types::{
    DefaultTypeOps, SINGLESTORE_METADATA_SQL_TYPE_KEY, TypeOps, get_field_sql_type_metadata_key,
};
use dbt_adapter_core::AdapterType::SingleStore;

#[test]
fn test_singlestore_type_formatting_and_parsing() {
    let type_ops = DefaultTypeOps::new(SingleStore);

    // Layer B: Type formatter testing
    let format = |dt: &DataType| -> String {
        let mut out = String::new();
        type_ops
            .format_arrow_type_as_sql(dt, true, &mut out)
            .unwrap();
        out
    };

    assert_eq!(format(&DataType::Float32), "FLOAT");
    assert_eq!(format(&DataType::Float64), "DOUBLE");
    assert_eq!(format(&DataType::Int8), "TINYINT");
    assert_eq!(format(&DataType::Int16), "SMALLINT");
    assert_eq!(format(&DataType::Int32), "INT");
    assert_eq!(format(&DataType::Int64), "BIGINT");
    assert_eq!(format(&DataType::UInt8), "TINYINT UNSIGNED");
    assert_eq!(format(&DataType::UInt16), "SMALLINT UNSIGNED");
    assert_eq!(format(&DataType::UInt32), "INT UNSIGNED");
    assert_eq!(format(&DataType::UInt64), "BIGINT UNSIGNED");
    assert_eq!(format(&DataType::Utf8), "TEXT");
    assert_eq!(format(&DataType::LargeUtf8), "TEXT");
    assert_eq!(format(&DataType::Binary), "BLOB");
    assert_eq!(format(&DataType::Date32), "DATE");
    assert_eq!(format(&DataType::Time64(TimeUnit::Microsecond)), "TIME(6)");
    assert_eq!(
        format(&DataType::Timestamp(TimeUnit::Microsecond, None)),
        "DATETIME(6)"
    );
    assert_eq!(format(&DataType::Decimal128(18, 4)), "DECIMAL(18, 4)");

    // SingleStore VECTOR formatting
    let vector_f32 =
        DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, false)), 1536);
    assert_eq!(format(&vector_f32), "VECTOR(1536, F32)");

    let vector_f64 =
        DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float64, false)), 768);
    assert_eq!(format(&vector_f64), "VECTOR(768, F64)");

    let vector_i8 =
        DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Int8, false)), 128);
    assert_eq!(format(&vector_i8), "VECTOR(128, I8)");

    // Metadata key
    assert_eq!(
        get_field_sql_type_metadata_key(SingleStore),
        SINGLESTORE_METADATA_SQL_TYPE_KEY
    );

    // Layer A: Type parser testing via TypeOps
    let (dt, nullable) = type_ops.parse_into_nullable_arrow_type("signed").unwrap();
    assert!(nullable);
    assert_eq!(dt, DataType::Int64);

    let (dt, _) = type_ops.parse_into_nullable_arrow_type("unsigned").unwrap();
    assert_eq!(dt, DataType::UInt64);

    let (dt, _) = type_ops
        .parse_into_nullable_arrow_type("int(11) unsigned")
        .unwrap();
    assert_eq!(dt, DataType::UInt32);

    let (dt, _) = type_ops
        .parse_into_nullable_arrow_type("vector(1536, f32)")
        .unwrap();
    assert_eq!(dt, vector_f32);
}
