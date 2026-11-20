use super::*;
use std::borrow::Cow;

/// Renders an Arrow type as a MariaDB column type, as used in DDL (seeds,
/// contracts). https://mariadb.com/kb/en/data-types/
///
/// Integers keep their width instead of collapsing to `integer` like
/// upstream `convert_integer_type`: MariaDB `INT` is 32-bit, so an
/// `Int64` seed column needs `bigint`.
pub fn try_format_type(datatype: &DataType, out: &mut String) -> AdapterResult<()> {
    let rendered: Cow<str> = match datatype {
        DataType::Null => "text".into(),
        DataType::Boolean => "boolean".into(),
        DataType::Int8 => "tinyint".into(),
        DataType::Int16 => "smallint".into(),
        DataType::Int32 => "int".into(),
        DataType::Int64 => "bigint".into(),
        DataType::UInt8 => "tinyint unsigned".into(),
        DataType::UInt16 => "smallint unsigned".into(),
        DataType::UInt32 => "int unsigned".into(),
        DataType::UInt64 => "bigint unsigned".into(),
        DataType::Float16 | DataType::Float32 => "float".into(),
        DataType::Float64 => "double".into(),
        DataType::Decimal32(precision, scale)
        | DataType::Decimal64(precision, scale)
        | DataType::Decimal128(precision, scale)
        | DataType::Decimal256(precision, scale) => {
            // DECIMAL allows at most 65 digits and a scale of 38.
            let scale = (*scale).clamp(0, 38);
            let precision = (*precision).clamp(1, 65).max(scale as u8);
            format!("decimal({precision},{scale})").into()
        }
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => "text".into(),
        DataType::Binary
        | DataType::LargeBinary
        | DataType::BinaryView
        | DataType::FixedSizeBinary(_) => "blob".into(),
        DataType::Date32 | DataType::Date64 => "date".into(),
        DataType::Timestamp(TimeUnit::Second, _) => "datetime".into(),
        DataType::Timestamp(TimeUnit::Millisecond, _) => "datetime(3)".into(),
        DataType::Timestamp(TimeUnit::Microsecond | TimeUnit::Nanosecond, _) => {
            "datetime(6)".into()
        }
        DataType::Time32(_) | DataType::Time64(_) | DataType::Duration(_) => "time(6)".into(),
        _ => {
            return Err(AdapterError::new(
                AdapterErrorKind::UnsupportedType,
                format!("{datatype} is not convertible to a MariaDB sql type"),
            ));
        }
    };
    out.push_str(&rendered);
    Ok(())
}

/// Renders an Arrow type as a MariaDB `CAST` target. CAST only takes a few
/// types (no `text`, `bigint` or `boolean`): https://mariadb.com/kb/en/cast/
pub fn try_format_cast_target(datatype: &DataType, out: &mut String) -> AdapterResult<()> {
    let target = match datatype {
        DataType::Boolean
        | DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64 => "signed",
        DataType::UInt8 | DataType::UInt16 | DataType::UInt32 | DataType::UInt64 => "unsigned",
        DataType::Null | DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => "char",
        DataType::Binary
        | DataType::LargeBinary
        | DataType::BinaryView
        | DataType::FixedSizeBinary(_) => "binary",
        _ => return try_format_type(datatype, out),
    };
    out.push_str(target);
    Ok(())
}
