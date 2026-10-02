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

macro_rules! append_opt {
    ($builder:expr, $row:expr, $idx:expr, $ty:ty) => {
        match $row.try_get::<Option<$ty>, _>($idx) {
            Ok(Some(v)) => $builder.append_value(v),
            _ => $builder.append_null(),
        }
    };
}

fn append_bool(b: &mut BooleanBuilder, row: &MySqlRow, idx: usize) {
    if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(idx) {
        b.append_value(v);
    } else if let Ok(Some(v)) = row.try_get::<Option<i8>, _>(idx) {
        b.append_value(v != 0);
    } else {
        b.append_null();
    }
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
            ColumnBuilder::Bool(b) => append_bool(b, row, idx),
            ColumnBuilder::Int8(b) => append_opt!(b, row, idx, i8),
            ColumnBuilder::Int16(b) => append_opt!(b, row, idx, i16),
            ColumnBuilder::Int32(b) => append_opt!(b, row, idx, i32),
            ColumnBuilder::Int64(b) => append_opt!(b, row, idx, i64),
            ColumnBuilder::UInt8(b) => append_opt!(b, row, idx, u8),
            ColumnBuilder::UInt16(b) => append_opt!(b, row, idx, u16),
            ColumnBuilder::UInt32(b) => append_opt!(b, row, idx, u32),
            ColumnBuilder::UInt64(b) => append_opt!(b, row, idx, u64),
            ColumnBuilder::Float32(b) => append_opt!(b, row, idx, f32),
            ColumnBuilder::Float64(b) => append_opt!(b, row, idx, f64),
            ColumnBuilder::String(b) => match get_as_string(row, idx) {
                Some(val) => b.append_value(val),
                None => b.append_null(),
            },
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

fn try_get_num_or_bool(row: &MySqlRow, idx: usize) -> Option<String> {
    row.try_get::<Option<i64>, _>(idx).ok().flatten().map(|v| v.to_string())
        .or_else(|| row.try_get::<Option<f64>, _>(idx).ok().flatten().map(|v| v.to_string()))
        .or_else(|| row.try_get::<Option<bool>, _>(idx).ok().flatten().map(|v| v.to_string()))
}

fn try_get_temporal(row: &MySqlRow, idx: usize) -> Option<String> {
    row.try_get::<Option<chrono::NaiveDateTime>, _>(idx).ok().flatten().map(|v| v.to_string())
        .or_else(|| row.try_get::<Option<chrono::NaiveDate>, _>(idx).ok().flatten().map(|v| v.to_string()))
        .or_else(|| row.try_get::<Option<chrono::NaiveTime>, _>(idx).ok().flatten().map(|v| v.to_string()))
}

fn get_as_string(row: &MySqlRow, idx: usize) -> Option<String> {
    row.try_get::<Option<String>, _>(idx)
        .ok()
        .flatten()
        .or_else(|| try_get_num_or_bool(row, idx))
        .or_else(|| try_get_temporal(row, idx))
        .or_else(|| {
            row.try_get::<Option<Vec<u8>>, _>(idx)
                .ok()
                .flatten()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        })
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
