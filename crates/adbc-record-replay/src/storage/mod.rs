pub(crate) mod sqlite;

#[deprecated(note = "File-based storage is legacy. Use SQLite storage for new recordings.")]
pub(crate) mod file;

pub(crate) static RECORDS_NAME: &str = "recordings.db";

use arrow_schema::{DataType, Field, Schema};
use std::sync::Arc;

pub(crate) fn fix_decimal_precision_in_schema(schema: &Schema) -> Schema {
    let fixed_fields: Vec<Field> = schema
        .fields()
        .iter()
        .map(|field| fix_decimal_precision_in_field(field))
        .collect();

    Schema::new(fixed_fields).with_metadata(schema.metadata().clone())
}

fn fix_decimal_precision_in_field(field: &Field) -> Field {
    let fixed_data_type = match field.data_type() {
        DataType::Decimal128(precision, scale) if *precision == 0 => {
            tracing::warn!(
                "Found DECIMAL128 with invalid precision=0, using defaults (38, 9) for field '{}'",
                field.name()
            );
            DataType::Decimal128(38, *scale)
        }
        DataType::Decimal256(precision, scale) if *precision == 0 => {
            tracing::warn!(
                "Found DECIMAL256 with invalid precision=0, using defaults (76, 38) for field '{}'",
                field.name()
            );
            DataType::Decimal256(76, *scale)
        }
        DataType::List(list_field) => {
            let fixed_list_field = Arc::new(fix_decimal_precision_in_field(list_field));
            DataType::List(fixed_list_field)
        }
        DataType::LargeList(list_field) => {
            let fixed_list_field = Arc::new(fix_decimal_precision_in_field(list_field));
            DataType::LargeList(fixed_list_field)
        }
        DataType::Struct(fields) => {
            let fixed_fields: Vec<Arc<Field>> = fields
                .iter()
                .map(|f| Arc::new(fix_decimal_precision_in_field(f)))
                .collect();
            DataType::Struct(fixed_fields.into())
        }
        other => other.clone(),
    };

    Field::new(field.name(), fixed_data_type, field.is_nullable())
        .with_metadata(field.metadata().clone())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StorageType {
    Sqlite,
    #[deprecated(note = "Arrow IPC file storage is legacy. Use SQLite.")]
    FileArrowIpc,
    #[deprecated(note = "Parquet file storage is legacy. Use SQLite.")]
    FileParquet,
}

#[allow(deprecated)]
pub(crate) fn detect_storage_type(path: &std::path::Path, file_name: &str) -> StorageType {
    let db_path = path.join(RECORDS_NAME);
    if db_path.exists() {
        return StorageType::Sqlite;
    }

    let arrow_path = path.join(format!("{file_name}.arrow"));
    if arrow_path.exists() {
        return StorageType::FileArrowIpc;
    }

    StorageType::FileParquet
}
