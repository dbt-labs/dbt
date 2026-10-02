//! SingleStore SQL type formatting.
//!
//! Provides Arrow DataType → SingleStore SQL type conversion used by
//! `DefaultTypeOps` when the adapter type is `AdapterType::SingleStore`.

use crate::AdapterResult;
use crate::errors::{AdapterError, AdapterErrorKind};
use arrow_schema::{DataType, TimeUnit};

pub fn try_format_type(
    datatype: &DataType,
    nullable: bool,
    out: &mut String,
) -> AdapterResult<()> {
    let mut rendered = String::new();
    match datatype {
        DataType::Null => rendered.push_str("TEXT"),
        DataType::Boolean => rendered.push_str("BOOLEAN"),
        DataType::Int8 => rendered.push_str("TINYINT"),
        DataType::Int16 => rendered.push_str("SMALLINT"),
        DataType::Int32 => rendered.push_str("INT"),
        DataType::Int64 => rendered.push_str("BIGINT"),
        DataType::UInt8 => rendered.push_str("TINYINT UNSIGNED"),
        DataType::UInt16 => rendered.push_str("SMALLINT UNSIGNED"),
        DataType::UInt32 => rendered.push_str("INT UNSIGNED"),
        DataType::UInt64 => rendered.push_str("BIGINT UNSIGNED"),
        DataType::Float16 | DataType::Float32 => rendered.push_str("FLOAT"),
        DataType::Float64 => rendered.push_str("DOUBLE"),
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => rendered.push_str("TEXT"),
        DataType::Binary | DataType::LargeBinary | DataType::BinaryView => {
            rendered.push_str("BLOB")
        }
        DataType::Date32 | DataType::Date64 => rendered.push_str("DATE"),
        DataType::Time32(_) | DataType::Time64(_) => rendered.push_str("TIME(6)"),
        DataType::Timestamp(TimeUnit::Second, _) => rendered.push_str("DATETIME"),
        DataType::Timestamp(TimeUnit::Millisecond, _) => rendered.push_str("DATETIME(3)"),
        DataType::Timestamp(TimeUnit::Microsecond, _)
        | DataType::Timestamp(TimeUnit::Nanosecond, _) => rendered.push_str("DATETIME(6)"),
        DataType::Decimal128(precision, scale) | DataType::Decimal256(precision, scale) => {
            rendered = format!("DECIMAL({precision}, {scale})");
        }
        DataType::FixedSizeList(field, size) => {
            let elem_type = field.data_type();
            let mut elem_str = String::new();
            match elem_type {
                DataType::Float32 => elem_str.push_str("F32"),
                DataType::Float64 => elem_str.push_str("F64"),
                DataType::Int8 => elem_str.push_str("I8"),
                DataType::Int16 => elem_str.push_str("I16"),
                DataType::Int32 => elem_str.push_str("I32"),
                DataType::Int64 => elem_str.push_str("I64"),
                _ => try_format_type(elem_type, false, &mut elem_str)?,
            }
            rendered = format!("VECTOR({size}, {elem_str})");
        }
        DataType::List(_) | DataType::LargeList(_) | DataType::Struct(_) | DataType::Map(..) => {
            rendered.push_str("JSON");
        }
        _ => {
            return Err(AdapterError::new(
                AdapterErrorKind::UnsupportedType,
                format!("{datatype} is not convertible to singlestore sql type"),
            ));
        }
    }

    out.push_str(&rendered);
    if !nullable {
        out.push_str(" NOT NULL");
    }
    Ok(())
}
