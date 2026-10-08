use std::fs::File;
use std::sync::Arc;

use arrow::array::RecordBatch;
use arrow::compute::concat_batches;
use arrow::csv::ReaderBuilder;
use arrow_schema::{DataType, Field, FieldRef, Schema};
use dbt_adapter_core::AdapterType;
use dbt_adapter_sql::types::metadata_sql_type_key;
use indexmap::IndexMap;

use crate::AdapterResult;
use crate::errors::{AdapterError, AdapterErrorKind, arrow_error_to_adapter_error};

pub(crate) fn ingest_schema_with_column_overrides(
    arrow_schema: &Schema,
    column_overrides: &IndexMap<String, String>,
    adapter_type: AdapterType,
) -> AdapterResult<Schema> {
    let sql_type_key = metadata_sql_type_key(adapter_type);

    let new_fields = arrow_schema
        .fields()
        .iter()
        .map(|field_ref| {
            let Some(data_type) = column_overrides.get(field_ref.name()) else {
                return Ok(Arc::clone(field_ref));
            };

            let mut metadata = field_ref.metadata().clone();
            metadata.insert(sql_type_key.to_string(), data_type.to_string());

            let field = field_ref.as_ref().clone().with_metadata(metadata);

            Ok(Arc::new(field))
        })
        .collect::<AdapterResult<Vec<FieldRef>>>()?;

    Ok(Schema::new(new_fields))
}

/// Read a seed CSV with every column as text, so the database casts each
/// value to the target column type exactly as `COPY ... FROM` would.
///
/// Only empty values become NULL. Agate's parse also nulls case-insensitive
/// `"null"`, which would change seed data that a `COPY` load preserves.
pub(crate) fn read_seed_csv_as_text(
    path: &str,
    delimiter: &str,
    column_names: &[String],
) -> AdapterResult<RecordBatch> {
    let &[delimiter] = delimiter.as_bytes() else {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!("seed delimiter must be a single byte, got {delimiter:?}"),
        ));
    };
    let schema = Arc::new(Schema::new(
        column_names
            .iter()
            .map(|name| Field::new(name, DataType::Utf8, true))
            .collect::<Vec<_>>(),
    ));
    let file = File::open(path).map_err(|e| {
        AdapterError::new(
            AdapterErrorKind::Io,
            format!("failed to open seed file {path}: {e}"),
        )
    })?;
    let batches = ReaderBuilder::new(Arc::clone(&schema))
        .with_header(true)
        .with_delimiter(delimiter)
        .build(file)
        .and_then(|reader| reader.collect::<Result<Vec<_>, _>>())
        .map_err(|e| {
            AdapterError::new(
                AdapterErrorKind::Io,
                format!("failed to read seed file {path}: {e}"),
            )
        })?;
    concat_batches(&schema, &batches).map_err(arrow_error_to_adapter_error)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use arrow_schema::{DataType, Field};

    use super::*;

    #[test]
    fn test_bigquery_ingest_schema_with_column_overrides() {
        let original_metadata = HashMap::from([("existing".to_string(), "value".to_string())]);
        let arrow_schema = Schema::new(vec![
            Field::new("id", DataType::Utf8, true).with_metadata(original_metadata),
            Field::new("name", DataType::Utf8, false),
        ]);
        let column_overrides = IndexMap::from([("id".to_string(), "int64".to_string())]);

        let ingest_schema = ingest_schema_with_column_overrides(
            &arrow_schema,
            &column_overrides,
            AdapterType::Bigquery,
        )
        .expect("ingest schema should apply BigQuery column overrides");

        let id = ingest_schema.field(0);
        assert_eq!(id.name(), "id");
        assert_eq!(id.data_type(), &DataType::Utf8);
        assert!(id.is_nullable());
        assert_eq!(id.metadata().get("existing"), Some(&"value".to_string()));
        assert_eq!(
            id.metadata()
                .get(metadata_sql_type_key(AdapterType::Bigquery)),
            Some(&"int64".to_string())
        );

        let name = ingest_schema.field(1);
        assert_eq!(name.name(), "name");
        assert_eq!(name.data_type(), &DataType::Utf8);
        assert!(!name.is_nullable());
        assert!(name.metadata().is_empty());
    }

    fn read_text(csv: &str, delimiter: &str) -> AdapterResult<RecordBatch> {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut file, csv.as_bytes()).unwrap();
        let names = ["id".to_string(), "label".to_string()];
        read_seed_csv_as_text(file.path().to_str().unwrap(), delimiter, &names)
    }

    fn labels(batch: &RecordBatch) -> Vec<Option<String>> {
        let col = batch
            .column(1)
            .as_any()
            .downcast_ref::<arrow::array::StringArray>()
            .unwrap();
        col.iter().map(|v| v.map(str::to_string)).collect()
    }

    #[test]
    fn read_seed_csv_as_text_only_nulls_empty_values() {
        // Matches DuckDB `COPY ... FROM` (CSV): only empty values are NULL;
        // the literal strings "NULL"/"null" and whitespace survive.
        let batch = read_text(
            "id,label\n1,NULL\n2,null\n3,\n4,\"\"\n5,\"a, b\"\n6,  padded  \n",
            ",",
        )
        .unwrap();
        assert_eq!(batch.num_rows(), 6);
        assert_eq!(
            labels(&batch),
            vec![
                Some("NULL".to_string()),
                Some("null".to_string()),
                None,
                None,
                Some("a, b".to_string()),
                Some("  padded  ".to_string()),
            ]
        );
        assert!(
            batch
                .schema()
                .fields()
                .iter()
                .all(|f| f.data_type() == &DataType::Utf8)
        );
    }

    #[test]
    fn read_seed_csv_as_text_honors_delimiter() {
        let batch = read_text("id|label\n1|x,y\n", "|").unwrap();
        assert_eq!(labels(&batch), vec![Some("x,y".to_string())]);
    }

    #[test]
    fn read_seed_csv_as_text_rejects_multibyte_delimiter() {
        let err = read_text("id,label\n1,x\n", "||").unwrap_err();
        assert!(err.to_string().contains("single byte"), "{err}");
    }
}
