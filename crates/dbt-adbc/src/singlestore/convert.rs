use super::sqlx::mysql::MySqlRow;
use super::sqlx::{Column, Row, TypeInfo};
use adbc_core::error::{Error, Result, Status};
use arrow_array::builder::*;
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema};
use std::sync::Arc;

pub(crate) enum ColumnBuilder {
    Bool(BooleanBuilder),
    Int8(Int8Builder),
    Int16(Int16Builder),
    Int32(Int32Builder),
    Int64(Int64Builder),
    UInt8(UInt8Builder),
    UInt16(UInt16Builder),
    UInt32(UInt32Builder),
    UInt64(UInt64Builder),
    Float32(Float32Builder),
    Float64(Float64Builder),
    String(StringBuilder),
}

impl ColumnBuilder {
    pub(crate) fn from_type_name(type_name: &str) -> (Self, DataType) {
        let upper = type_name.to_ascii_uppercase();
        match upper.as_str() {
            "BOOLEAN" => (
                ColumnBuilder::Bool(BooleanBuilder::new()),
                DataType::Boolean,
            ),
            "TINYINT" => (ColumnBuilder::Int8(Int8Builder::new()), DataType::Int8),
            "SMALLINT" => (ColumnBuilder::Int16(Int16Builder::new()), DataType::Int16),
            "INT" | "INTEGER" | "MEDIUMINT" => {
                (ColumnBuilder::Int32(Int32Builder::new()), DataType::Int32)
            }
            "BIGINT" => (ColumnBuilder::Int64(Int64Builder::new()), DataType::Int64),
            "TINYINT UNSIGNED" => (ColumnBuilder::UInt8(UInt8Builder::new()), DataType::UInt8),
            "SMALLINT UNSIGNED" => (
                ColumnBuilder::UInt16(UInt16Builder::new()),
                DataType::UInt16,
            ),
            "INT UNSIGNED" | "INTEGER UNSIGNED" | "MEDIUMINT UNSIGNED" => (
                ColumnBuilder::UInt32(UInt32Builder::new()),
                DataType::UInt32,
            ),
            "BIGINT UNSIGNED" => (
                ColumnBuilder::UInt64(UInt64Builder::new()),
                DataType::UInt64,
            ),
            "FLOAT" => (
                ColumnBuilder::Float32(Float32Builder::new()),
                DataType::Float32,
            ),
            "DOUBLE" => (
                ColumnBuilder::Float64(Float64Builder::new()),
                DataType::Float64,
            ),
            _ => (ColumnBuilder::String(StringBuilder::new()), DataType::Utf8),
        }
    }

    fn append_from_row(&mut self, row: &MySqlRow, idx: usize) {
        match self {
            ColumnBuilder::Bool(b) => {
                if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(idx) {
                    b.append_value(v);
                } else if let Ok(Some(v)) = row.try_get::<Option<i8>, _>(idx) {
                    b.append_value(v != 0);
                } else {
                    b.append_null();
                }
            }
            ColumnBuilder::Int8(b) => {
                if let Ok(Some(v)) = row.try_get::<Option<i8>, _>(idx) {
                    b.append_value(v);
                } else {
                    b.append_null();
                }
            }
            ColumnBuilder::Int16(b) => {
                if let Ok(Some(v)) = row.try_get::<Option<i16>, _>(idx) {
                    b.append_value(v);
                } else {
                    b.append_null();
                }
            }
            ColumnBuilder::Int32(b) => {
                if let Ok(Some(v)) = row.try_get::<Option<i32>, _>(idx) {
                    b.append_value(v);
                } else {
                    b.append_null();
                }
            }
            ColumnBuilder::Int64(b) => {
                if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(idx) {
                    b.append_value(v);
                } else {
                    b.append_null();
                }
            }
            ColumnBuilder::UInt8(b) => {
                if let Ok(Some(v)) = row.try_get::<Option<u8>, _>(idx) {
                    b.append_value(v);
                } else {
                    b.append_null();
                }
            }
            ColumnBuilder::UInt16(b) => {
                if let Ok(Some(v)) = row.try_get::<Option<u16>, _>(idx) {
                    b.append_value(v);
                } else {
                    b.append_null();
                }
            }
            ColumnBuilder::UInt32(b) => {
                if let Ok(Some(v)) = row.try_get::<Option<u32>, _>(idx) {
                    b.append_value(v);
                } else {
                    b.append_null();
                }
            }
            ColumnBuilder::UInt64(b) => {
                if let Ok(Some(v)) = row.try_get::<Option<u64>, _>(idx) {
                    b.append_value(v);
                } else {
                    b.append_null();
                }
            }
            ColumnBuilder::Float32(b) => {
                if let Ok(Some(v)) = row.try_get::<Option<f32>, _>(idx) {
                    b.append_value(v);
                } else {
                    b.append_null();
                }
            }
            ColumnBuilder::Float64(b) => {
                if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(idx) {
                    b.append_value(v);
                } else {
                    b.append_null();
                }
            }
            ColumnBuilder::String(b) => {
                if let Some(val) = get_as_string(row, idx) {
                    b.append_value(val);
                } else {
                    b.append_null();
                }
            }
        }
    }

    fn finish(self) -> ArrayRef {
        match self {
            ColumnBuilder::Bool(mut b) => Arc::new(b.finish()),
            ColumnBuilder::Int8(mut b) => Arc::new(b.finish()),
            ColumnBuilder::Int16(mut b) => Arc::new(b.finish()),
            ColumnBuilder::Int32(mut b) => Arc::new(b.finish()),
            ColumnBuilder::Int64(mut b) => Arc::new(b.finish()),
            ColumnBuilder::UInt8(mut b) => Arc::new(b.finish()),
            ColumnBuilder::UInt16(mut b) => Arc::new(b.finish()),
            ColumnBuilder::UInt32(mut b) => Arc::new(b.finish()),
            ColumnBuilder::UInt64(mut b) => Arc::new(b.finish()),
            ColumnBuilder::Float32(mut b) => Arc::new(b.finish()),
            ColumnBuilder::Float64(mut b) => Arc::new(b.finish()),
            ColumnBuilder::String(mut b) => Arc::new(b.finish()),
        }
    }
}

fn get_as_string(row: &MySqlRow, idx: usize) -> Option<String> {
    if let Ok(val) = row.try_get::<Option<String>, _>(idx) {
        return val;
    }
    if let Ok(val) = row.try_get::<Option<i64>, _>(idx) {
        return val.map(|v| v.to_string());
    }
    if let Ok(val) = row.try_get::<Option<f64>, _>(idx) {
        return val.map(|v| v.to_string());
    }
    if let Ok(val) = row.try_get::<Option<bool>, _>(idx) {
        return val.map(|v| v.to_string());
    }
    if let Ok(val) = row.try_get::<Option<chrono::NaiveDateTime>, _>(idx) {
        return val.map(|v| v.to_string());
    }
    if let Ok(val) = row.try_get::<Option<chrono::NaiveDate>, _>(idx) {
        return val.map(|v| v.to_string());
    }
    if let Ok(val) = row.try_get::<Option<chrono::NaiveTime>, _>(idx) {
        return val.map(|v| v.to_string());
    }
    if let Ok(val) = row.try_get::<Option<Vec<u8>>, _>(idx) {
        return val.map(|bytes: Vec<u8>| String::from_utf8_lossy(&bytes).into_owned());
    }
    None
}

pub fn mysql_rows_to_record_batch(rows: &[MySqlRow]) -> Result<RecordBatch> {
    if rows.is_empty() {
        let schema = Arc::new(Schema::empty());
        return Ok(RecordBatch::new_empty(schema));
    }

    let first = &rows[0];
    let columns = first.columns();
    let mut fields = Vec::with_capacity(columns.len());
    let mut builders = Vec::with_capacity(columns.len());

    for col in columns {
        let col_name = col.name();
        let type_name = col.type_info().name();
        let (builder, data_type) = ColumnBuilder::from_type_name(type_name);
        fields.push(Field::new(col_name, data_type, true));
        builders.push(builder);
    }

    for row in rows {
        for (idx, builder) in builders.iter_mut().enumerate() {
            builder.append_from_row(row, idx);
        }
    }

    let arrays: Vec<ArrayRef> = builders.into_iter().map(|b| b.finish()).collect();
    let schema = Arc::new(Schema::new(fields));

    RecordBatch::try_new(schema, arrays).map_err(|e| {
        Error::with_message_and_status(
            format!("Failed to convert MySQL rows to RecordBatch: {e}"),
            Status::Internal,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_rows_to_record_batch() {
        let rows: Vec<MySqlRow> = vec![];
        let batch = mysql_rows_to_record_batch(&rows).unwrap();
        assert_eq!(batch.num_rows(), 0);
        assert_eq!(batch.num_columns(), 0);
    }

    #[test]
    fn test_column_builder_type_mapping() {
        let (_, dt) = ColumnBuilder::from_type_name("BOOLEAN");
        assert_eq!(dt, DataType::Boolean);

        let (_, dt) = ColumnBuilder::from_type_name("BIGINT");
        assert_eq!(dt, DataType::Int64);

        let (_, dt) = ColumnBuilder::from_type_name("INT");
        assert_eq!(dt, DataType::Int32);

        let (_, dt) = ColumnBuilder::from_type_name("VARCHAR");
        assert_eq!(dt, DataType::Utf8);

        let (_, dt) = ColumnBuilder::from_type_name("DOUBLE");
        assert_eq!(dt, DataType::Float64);
    }
}
