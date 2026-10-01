//! Legacy file-based storage (Arrow IPC / Parquet).
//!
//! Deprecated: all new recordings use SQLite. This module exists only to
//! read recordings created before the SQLite migration.

use arrow::array::{RecordBatch, RecordBatchIterator, RecordBatchReader};
use arrow::ipc::reader::FileReader as ArrowFileReader;
use arrow_schema::{ArrowError, Field, Schema};
use std::collections::HashMap;
use std::fs::{self, File, metadata};
use std::path::Path;
use std::sync::Arc;

use crate::error::RecordReplayError;

#[deprecated(note = "File-based storage is legacy. Use SQLite storage for new recordings.")]
#[derive(Clone, Copy)]
pub(crate) enum FileFormat {
    Parquet,
    ArrowIPC,
}

#[allow(deprecated)]
impl FileFormat {
    pub fn extension(&self) -> &str {
        match self {
            FileFormat::Parquet => "parquet",
            FileFormat::ArrowIPC => "arrow",
        }
    }
}

#[deprecated(note = "File-based storage is legacy. Use SQLite storage for new recordings.")]
#[allow(deprecated)]
pub(crate) struct FileHandler {
    format: FileFormat,
}

#[allow(deprecated)]
impl FileHandler {
    pub fn new_for_replay(format: FileFormat) -> Self {
        Self { format }
    }

    pub fn read_error(
        &self,
        base_path: &Path,
        file_name: &str,
    ) -> Result<Option<String>, RecordReplayError> {
        let err_path = base_path.join(format!("{file_name}.err"));
        if err_path.exists() {
            let msg = fs::read_to_string(err_path)?;
            Ok(Some(msg))
        } else {
            Ok(None)
        }
    }

    pub fn read_sql(&self, base_path: &Path, file_name: &str) -> Result<String, RecordReplayError> {
        let sql_path = base_path.join(format!("{file_name}.sql"));
        Ok(fs::read_to_string(sql_path)?)
    }

    pub fn read_schema(
        &self,
        base_path: &Path,
        file_name: &str,
    ) -> Result<Schema, RecordReplayError> {
        let data_path = base_path.join(format!("{file_name}.{}", self.format.extension()));

        match self.format {
            FileFormat::ArrowIPC => {
                let file = File::open(data_path)?;
                let reader = ArrowFileReader::try_new(file, None)?;
                Ok(reader.schema().as_ref().clone())
            }
            FileFormat::Parquet => {
                let file = File::open(&data_path)?;
                let builder =
                    parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file)?;
                let reader = builder.build()?;
                let schema = reader.schema().as_ref().clone();

                let metadata_path = base_path.join(format!("{file_name}.metadata.json"));
                let metadata_json = fs::read_to_string(metadata_path)?;
                let metadata: HashMap<String, String> = serde_json::from_str(&metadata_json)?;

                let mut schema_builder = arrow_schema::SchemaBuilder::from(schema.fields());
                for (key, value) in metadata {
                    schema_builder.metadata_mut().insert(key, value);
                }

                Ok(schema_builder.finish())
            }
        }
    }

    pub fn read_batches<'a>(
        &self,
        path: &Path,
    ) -> Result<Box<dyn RecordBatchReader + Send + 'a>, RecordReplayError> {
        let file_metadata = metadata(path)?;

        if file_metadata.len() == 0 {
            let schema = Arc::new(Schema::new(Vec::<Field>::new()));
            let batch = RecordBatch::new_empty(schema.clone());
            let results = vec![batch]
                .into_iter()
                .map(|batch| -> Result<RecordBatch, ArrowError> { Ok(batch) });
            let iterator = RecordBatchIterator::new(results, schema);
            return Ok(Box::new(iterator));
        }

        match self.format {
            FileFormat::ArrowIPC => {
                let file = File::open(path)?;
                let reader = ArrowFileReader::try_new(file, None)?;
                Ok(Box::new(reader))
            }
            FileFormat::Parquet => {
                let file = File::open(path)?;
                let builder =
                    parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file)?;
                let reader = builder.build()?;
                Ok(Box::new(reader))
            }
        }
    }
}
