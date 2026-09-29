//! Read seed files, without a DataFusion `SessionContext`.
//!
//! JSON and physical-types parquet entrypoints block the caller; run them on a blocking
//! pool when inside async code. The view-promoted parquet entrypoint is async (`tokio`).

use arrow::array::RecordBatch;
use arrow::compute::CastOptions;
use arrow_schema::{ArrowError, DataType, Field, Schema};
use datafusion_common::DataFusionError;
use dbt_adapter_core::AdapterType;
use futures::TryStreamExt;
use parquet::arrow::ArrowWriter;
use parquet::arrow::ParquetRecordBatchStreamBuilder;
use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::{path::Path, sync::Arc};

/// Supported on-disk table formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableFormat {
    Parquet,
    Csv,
    Json,
}

/// Strategies for normalizing inferred column names.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum InferColumnNameStrategy {
    Verbatim,
    Uppercase,
    Lowercase,
}

/// Read a JSON seed file.
/// Returns the inferred Arrow schema, and record batches when `load_batches` is true.
/// Blocking.
pub fn read_json_seed(
    path: &Path,
    load_batches: bool,
) -> Result<(Arc<Schema>, Option<Vec<RecordBatch>>), DataFusionError> {
    let file = File::open(path).map_err(|e| DataFusionError::External(Box::new(e)))?;
    let mut reader = BufReader::new(file);
    let (schema, _) = arrow::json::reader::infer_json_schema(&mut reader, None)
        .map_err(|e| DataFusionError::External(Box::new(e)))?;
    let schema = Arc::new(schema);
    if !load_batches {
        return Ok((schema, None));
    }
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|e| DataFusionError::External(Box::new(e)))?;
    let json_reader = arrow::json::reader::ReaderBuilder::new(schema.clone())
        .build(reader)
        .map_err(|e| DataFusionError::External(Box::new(e)))?;
    let batches: Vec<RecordBatch> = json_reader
        .collect::<Result<_, _>>()
        .map_err(|e| DataFusionError::External(Box::new(e)))?;
    Ok((schema, Some(batches)))
}

/// Open `path` as a tokio file and build a parquet stream reader.
async fn open_parquet_stream(
    path: &Path,
) -> Result<ParquetRecordBatchStreamBuilder<tokio::fs::File>, DataFusionError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| DataFusionError::External(Box::new(e)))?;
    ParquetRecordBatchStreamBuilder::new(file)
        .await
        .map_err(|e| DataFusionError::External(Box::new(e)))
}

/// Read a Parquet seed file with view-type promotion (Utf8/LargeUtf8 → Utf8View,
/// Binary/LargeBinary → BinaryView). Matches what DataFusion's `ParquetFormat::infer_schema`
/// produces under the default session config.
/// Returns the listing schema, and decoded view-promoted batches when `load_batches` is true.
pub async fn read_parquet_seed_view(
    path: &Path,
    load_batches: bool,
) -> Result<(Arc<Schema>, Option<Vec<RecordBatch>>), DataFusionError> {
    let builder = open_parquet_stream(path).await?;
    let listing_schema = Arc::new(transform_schema_to_view(builder.schema().as_ref()));
    if !load_batches {
        return Ok((listing_schema, None));
    }
    let stream = builder
        .build()
        .map_err(|e| DataFusionError::External(Box::new(e)))?;
    let raw_batches: Vec<RecordBatch> = stream
        .try_collect()
        .await
        .map_err(|e| DataFusionError::External(Box::new(e)))?;
    let batches = raw_batches
        .into_iter()
        .map(|batch| promote_batch_to_view(batch, &listing_schema))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| DataFusionError::External(Box::new(e)))?;
    Ok((listing_schema, Some(batches)))
}

/// Read a Parquet seed file with physical Arrow types (no view promotion).
/// Returns the footer-derived schema and all decoded batches.
/// Blocking.
pub fn read_parquet_seed_physical(
    path: &Path,
) -> Result<(Arc<Schema>, Vec<RecordBatch>), DataFusionError> {
    let file = File::open(path).map_err(|e| DataFusionError::External(Box::new(e)))?;
    let builder = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| DataFusionError::External(Box::new(e)))?;
    let schema = builder.schema().clone();
    let reader = builder
        .build()
        .map_err(|e| DataFusionError::External(Box::new(e)))?;
    let batches: Vec<RecordBatch> = reader
        .collect::<Result<_, _>>()
        .map_err(|e| DataFusionError::External(Box::new(e)))?;
    Ok((schema, batches))
}

/// Build a zero-row parquet file in memory that preserves the given schema.
/// Used by `--empty` to ship a schema-only seed payload to the db-runner.
pub fn write_empty_parquet_bytes(schema: &Arc<Schema>) -> Result<Vec<u8>, String> {
    let empty_batch = RecordBatch::new_empty(schema.clone());
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut writer = ArrowWriter::try_new(&mut buf, schema.clone(), None)
            .map_err(|e| format!("ArrowWriter init failed: {e}"))?;
        writer
            .write(&empty_batch)
            .map_err(|e| format!("ArrowWriter write failed: {e}"))?;
        writer
            .close()
            .map_err(|e| format!("ArrowWriter close failed: {e}"))?;
    }
    Ok(buf)
}

/// Rebuild `batch` under `target_schema`, casting columns where types differ.
fn promote_batch_to_view(
    batch: RecordBatch,
    target_schema: &Arc<Schema>,
) -> Result<RecordBatch, ArrowError> {
    let cast_options = CastOptions::default();
    let columns = batch
        .columns()
        .iter()
        .zip(target_schema.fields().iter())
        .map(|(col, target_field)| {
            if col.data_type() == target_field.data_type() {
                Ok(col.clone())
            } else {
                arrow::compute::cast_with_options(
                    col.as_ref(),
                    target_field.data_type(),
                    &cast_options,
                )
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    RecordBatch::try_new(target_schema.clone(), columns)
}

/// Port of `datafusion::datasource::file_format::parquet::transform_schema_to_view`.
/// Rewrites Utf8/LargeUtf8 as Utf8View and Binary/LargeBinary as BinaryView so
/// our compute-free schema inference agrees with what DataFusion's
/// `ParquetFormat::infer_schema` produces (given `schema_force_view_types=true`
/// in the default session config).
fn transform_schema_to_view(schema: &Schema) -> Schema {
    let fields = schema
        .fields()
        .iter()
        .map(|field| match field.data_type() {
            DataType::Utf8 | DataType::LargeUtf8 => field_with_new_type(field, DataType::Utf8View),
            DataType::Binary | DataType::LargeBinary => {
                field_with_new_type(field, DataType::BinaryView)
            }
            _ => Arc::clone(field),
        })
        .collect::<Vec<_>>();
    Schema::new_with_metadata(fields, schema.metadata().clone())
}

fn field_with_new_type(field: &Arc<Field>, new_type: DataType) -> Arc<Field> {
    Arc::new(field.as_ref().clone().with_data_type(new_type))
}

/// Seed schema with column names normalized per `infer_column_name_strategy`.
pub fn adapt_schema(
    schema: Arc<Schema>,
    infer_column_name_strategy: InferColumnNameStrategy,
) -> Arc<Schema> {
    Arc::new(Schema::new_with_metadata(
        schema
            .fields()
            .iter()
            .map(|field| {
                // dbt always trims the field name
                let field_name = field.name().trim();
                let new_name = match infer_column_name_strategy {
                    InferColumnNameStrategy::Verbatim => field_name.to_string(),
                    InferColumnNameStrategy::Uppercase => field_name.to_uppercase(),
                    InferColumnNameStrategy::Lowercase => field_name.to_lowercase(),
                };
                Arc::new((**field).clone().with_name(new_name))
            })
            .collect::<Vec<_>>(),
        schema.metadata().clone(),
    ))
}
/// Computes the appropriate [`InferColumnNameStrategy`] for seeds given dbt
/// configuration flags and dialect-specific casing rules.
pub fn infer_seed_column_name_strategy(
    quote_columns: bool,
    adapter_type: AdapterType,
) -> InferColumnNameStrategy {
    match (quote_columns, adapter_type) {
        // In Trino, all names are lowercase, even quoted.
        (true, _) => InferColumnNameStrategy::Verbatim,
        (
            false,
            AdapterType::Postgres
            | AdapterType::Salesforce
            | AdapterType::Redshift
            | AdapterType::DuckDB
            | AdapterType::LakeCompute,
        ) => InferColumnNameStrategy::Lowercase,
        (false, AdapterType::Snowflake) => InferColumnNameStrategy::Uppercase,
        (
            false,
            AdapterType::Bigquery
            | AdapterType::Databricks
            | AdapterType::Spark
            | AdapterType::Fabric,
        ) => InferColumnNameStrategy::Verbatim,
        (false, AdapterType::ClickHouse) => InferColumnNameStrategy::Verbatim,
        (false, AdapterType::Exasol) => InferColumnNameStrategy::Uppercase,
        (false, AdapterType::Starburst) => todo!("Starburst"),
        (false, AdapterType::Athena) => todo!("Athena"),
        (false, AdapterType::Trino) => todo!("Trino"),
        (false, AdapterType::Dremio) => todo!("Dremio"),
        (false, AdapterType::Oracle) => todo!("Oracle"),
        (false, AdapterType::Datafusion) => todo!("Datafusion"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{BinaryArray, Int32Array, StringArray};
    use parquet::arrow::ArrowWriter;
    use std::fs::File;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Footer-derived listing schema with [`transform_schema_to_view`] — reference for async listing.
    fn parquet_footer_listing_schema_gold(path: &Path) -> Arc<Schema> {
        let file = File::open(path).unwrap();
        let builder =
            parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
        Arc::new(transform_schema_to_view(builder.schema()))
    }

    /// Physical read + view promotion — reference that view-promoted parquet streaming must match.
    fn parquet_physical_read_promoted_gold(path: &Path) -> (Arc<Schema>, Vec<RecordBatch>) {
        let (physical_schema, raw_batches) = read_parquet_seed_physical(path).unwrap();
        let target_schema = Arc::new(transform_schema_to_view(physical_schema.as_ref()));
        let batches = raw_batches
            .into_iter()
            .map(|batch| promote_batch_to_view(batch, &target_schema))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        (target_schema, batches)
    }

    #[dbt_runtime::test]
    async fn read_parquet_seed_view_matches_physical_read_gold() {
        let physical_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, true),
            Field::new("payload", DataType::Binary, true),
        ]));
        let id = Arc::new(Int32Array::from(vec![1, 2, 3]));
        let name = Arc::new(StringArray::from(vec![Some("alice"), Some("bob"), None]));
        let payload = Arc::new(BinaryArray::from_opt_vec(vec![
            Some(b"a".as_ref()),
            None,
            Some(b"cc".as_ref()),
        ]));
        let batch = RecordBatch::try_new(physical_schema.clone(), vec![id, name, payload]).unwrap();

        let file = NamedTempFile::with_suffix(".parquet").unwrap();
        {
            let mut writer = ArrowWriter::try_new(file.reopen().unwrap(), physical_schema, None)
                .expect("parquet writer");
            writer.write(&batch).expect("parquet write");
            writer.close().expect("parquet close");
        }

        let gold = parquet_physical_read_promoted_gold(file.path());
        let (schema, batches) = read_parquet_seed_view(file.path(), true).await.unwrap();
        let batches = batches.expect("load_batches");
        assert_eq!(schema, gold.0);
        assert_eq!(batches.len(), gold.1.len());
        assert_eq!(schema.field(1).data_type(), &DataType::Utf8View);
        assert_eq!(schema.field(2).data_type(), &DataType::BinaryView);
        for (a, g) in batches.iter().zip(gold.1.iter()) {
            assert_eq!(a.schema(), g.schema());
            assert_eq!(a.num_rows(), g.num_rows());
        }
    }

    #[dbt_runtime::test]
    async fn read_parquet_seed_view_listing_schema_matches_full_read_and_footer_gold() {
        let physical_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            physical_schema.clone(),
            vec![
                Arc::new(Int32Array::from(vec![1, 2])),
                Arc::new(StringArray::from(vec!["a", "b"])),
            ],
        )
        .unwrap();

        let file = NamedTempFile::with_suffix(".parquet").unwrap();
        {
            let mut writer = ArrowWriter::try_new(file.reopen().unwrap(), physical_schema, None)
                .expect("parquet writer");
            writer.write(&batch).expect("parquet write");
            writer.close().expect("parquet close");
        }

        let footer_gold = parquet_footer_listing_schema_gold(file.path());

        let (schema_only, no_batches) = read_parquet_seed_view(file.path(), false).await.unwrap();
        assert!(no_batches.is_none());
        assert_eq!(schema_only, footer_gold);

        let (schema_with_batches, batches) =
            read_parquet_seed_view(file.path(), true).await.unwrap();
        let batches = batches.expect("load_batches");
        assert_eq!(schema_with_batches, footer_gold);
        assert_eq!(schema_with_batches, schema_only);
        let (_gold_schema, gold_batches) = parquet_physical_read_promoted_gold(file.path());
        assert_eq!(batches.len(), gold_batches.len());
    }

    #[test]
    fn read_json_seed_load_batches_false_skips_row_scan() {
        let mut file = NamedTempFile::with_suffix(".json").unwrap();
        writeln!(file, r#"{{"id": 1, "name": "alice"}}"#).unwrap();
        file.flush().unwrap();

        let (_schema, batches) = read_json_seed(file.path(), false).unwrap();
        assert!(batches.is_none());
    }

    #[test]
    fn read_json_seed_full_read_matches_listing_schema_and_row_count() {
        let mut file = NamedTempFile::with_suffix(".json").unwrap();
        // Newline-delimited JSON, the on-disk format dbt seed writes use.
        writeln!(file, r#"{{"id": 1, "name": "alice"}}"#).unwrap();
        writeln!(file, r#"{{"id": 2, "name": "bob"}}"#).unwrap();
        writeln!(file, r#"{{"id": 3, "name": "charlie"}}"#).unwrap();
        file.flush().unwrap();

        let (schema, batches) = read_json_seed(file.path(), true).expect("read json");
        let batches = batches.expect("batches");
        let (listing_schema, _) = read_json_seed(file.path(), false).unwrap();
        assert_eq!(schema, listing_schema);

        let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 3);
        for b in &batches {
            assert_eq!(b.schema(), schema);
        }
    }
}
