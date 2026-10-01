use adbc_core::error::{Error as AdbcError, Result as AdbcResult, Status as AdbcStatus};
use adbc_core::options::{OptionStatement, OptionValue};
use arrow::array::{RecordBatch, RecordBatchReader};
use arrow::record_batch::RecordBatchIterator;
use arrow_schema::{ArrowError, Schema};
use dbt_adbc::{Connection, Statement};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::error::{RecordReplayError, to_adbc_error};
use crate::naming::{
    compute_file_name, compute_file_name_for_get_objects, compute_file_name_for_table_schema,
};
use crate::storage::StorageType;
use crate::storage::sqlite::SqliteHandler;
use crate::{RecordingContext, SharedConfig};

pub struct ReplayConnection {
    recordings_path: PathBuf,
    config: SharedConfig,
    ctx: RecordingContext,
    generation: u64,
}

impl ReplayConnection {
    pub fn new(recordings_path: PathBuf, config: SharedConfig, generation: u64) -> Self {
        Self {
            recordings_path,
            config,
            ctx: RecordingContext::default(),
            generation,
        }
    }

    pub fn set_recording_context(&mut self, ctx: RecordingContext) {
        self.ctx = ctx;
    }
}

impl fmt::Debug for ReplayConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ReplayConnection")
    }
}

impl Connection for ReplayConnection {
    fn new_statement(&mut self) -> AdbcResult<Box<dyn Statement>> {
        let stmt = ReplayStatement::new(
            self.recordings_path.clone(),
            self.config.clone(),
            Default::default(),
        );
        Ok(Box::new(stmt))
    }

    fn cancel(&mut self) -> AdbcResult<()> {
        unimplemented!("ADBC connection cancellation in replay engine")
    }

    fn commit(&mut self) -> AdbcResult<()> {
        unimplemented!("ADBC connection commit in replay engine")
    }

    fn rollback(&mut self) -> AdbcResult<()> {
        unimplemented!("ADBC connection rollback in replay engine")
    }

    #[allow(deprecated)]
    fn get_objects<'a>(
        &'a self,
        _depth: adbc_core::options::ObjectDepth,
        catalog: Option<&'a str>,
        db_schema: Option<&'a str>,
        table_name: Option<&'a str>,
        table_type: Option<Vec<&'a str>>,
        column_name: Option<&'a str>,
    ) -> AdbcResult<Box<dyn RecordBatchReader + Send + 'a>> {
        let path = self.recordings_path.clone();
        // Normalize so a randomized __dbt_tmp suffix matches the key stored at record time.
        let normalized_table_name = table_name.map(|t| self.config.normalize_sql(t));
        let unique_id = compute_file_name_for_get_objects(
            &path,
            self.ctx.node_id.as_deref(),
            catalog,
            db_schema,
            normalized_table_name.as_deref(),
            table_type.as_deref(),
            column_name,
        );

        let storage_type = crate::storage::detect_storage_type(&path, &unique_id);

        match storage_type {
            StorageType::Sqlite => {
                let sqlite_handler = SqliteHandler::new(&path);
                let entry = sqlite_handler
                    .read_objects(&unique_id)
                    .map_err(|e| to_adbc_error(e, Some(&path)))?;

                if let Some(msg) = entry.error {
                    return Err(AdbcError::with_message_and_status(
                        msg,
                        AdbcStatus::Internal,
                    ));
                }

                let (schema, batches) = entry.data.ok_or_else(|| {
                    to_adbc_error(
                        RecordReplayError("Missing data in recording".to_string()),
                        Some(&path),
                    )
                })?;
                let batches_iter = batches
                    .into_iter()
                    .map(|batch| -> Result<RecordBatch, ArrowError> { Ok(batch) });
                Ok(Box::new(RecordBatchIterator::new(batches_iter, schema)))
            }
            StorageType::FileArrowIpc | StorageType::FileParquet => {
                Err(AdbcError::with_message_and_status(
                    "get_objects replay is only supported with SQLite storage",
                    AdbcStatus::Internal,
                ))
            }
        }
    }

    #[allow(deprecated)]
    fn get_table_schema(
        &self,
        catalog: Option<&str>,
        db_schema: Option<&str>,
        table_name: &str,
    ) -> AdbcResult<Schema> {
        let path = self.recordings_path.clone();
        // Same as get_objects: keep the hash key stable despite tmp-suffix churn.
        let normalized_table_name = self.config.normalize_sql(table_name);
        let unique_id = compute_file_name_for_table_schema(
            &path,
            self.ctx.node_id.as_deref(),
            catalog,
            db_schema,
            &normalized_table_name,
        );

        let storage_type = crate::storage::detect_storage_type(&path, &unique_id);

        match storage_type {
            StorageType::Sqlite => {
                let sqlite_handler = SqliteHandler::new(&path);
                let entry = sqlite_handler
                    .read_schema(&unique_id)
                    .map_err(|e| to_adbc_error(e, Some(&path)))?;

                if let Some(msg) = entry.error {
                    return Err(AdbcError::with_message_and_status(
                        msg,
                        AdbcStatus::Internal,
                    ));
                }

                entry
                    .data
                    .map(|(schema, _batches)| schema.as_ref().clone())
                    .ok_or_else(|| {
                        to_adbc_error(
                            RecordReplayError("Missing data in recording".to_string()),
                            Some(&path),
                        )
                    })
            }
            StorageType::FileArrowIpc | StorageType::FileParquet => {
                replay_file_schema(&path, &unique_id, storage_type)
            }
        }
    }

    fn update_node_id(&mut self, node_id: Option<String>) {
        self.ctx.node_id = node_id;
    }

    fn fingerprint(&self) -> u64 {
        self.generation
    }
}

pub(crate) struct ReplayStatement {
    recordings_path: PathBuf,
    config: SharedConfig,
    ctx: RecordingContext,
    sql: Option<String>,
    /// Statement options captured off the live driver when this recording was
    /// made, populated by the last `execute`/`execute_update` and served by
    /// `get_option_string`. Empty until then, and for recordings made before
    /// options were captured.
    recorded_options: BTreeMap<String, String>,
}

impl ReplayStatement {
    pub(crate) fn new(
        recordings_path: PathBuf,
        config: SharedConfig,
        ctx: RecordingContext,
    ) -> Self {
        Self {
            recordings_path,
            config,
            ctx,
            sql: None,
            recorded_options: BTreeMap::new(),
        }
    }
}

impl Statement for ReplayStatement {
    fn bind(&mut self, _batch: RecordBatch) -> AdbcResult<()> {
        todo!("ReplayStatement::bind")
    }

    fn bind_stream(&mut self, _reader: Box<dyn RecordBatchReader + Send>) -> AdbcResult<()> {
        todo!("ReplayStatement::bind_stream")
    }

    #[allow(deprecated)]
    fn execute<'a>(&'a mut self) -> AdbcResult<Box<dyn RecordBatchReader + Send + 'a>> {
        let replay_sql = match &self.sql {
            Some(sql) => sql,
            None => "none",
        };

        let path = self.recordings_path.clone();
        let unique_id = compute_file_name(
            &path,
            self.ctx.node_id.as_ref(),
            Some(replay_sql),
            self.ctx.metadata,
        )?;

        let storage_type = crate::storage::detect_storage_type(&path, &unique_id);

        match storage_type {
            StorageType::Sqlite => {
                let sqlite_handler = SqliteHandler::new(&path);
                let entry = sqlite_handler
                    .read_execute(&unique_id, replay_sql)
                    .map_err(|e| to_adbc_error(e, Some(&path)))?;

                let record_sql = entry.sql.as_deref().unwrap_or("none");
                if self.config.normalize_sql(record_sql) != self.config.normalize_sql(replay_sql) {
                    panic!(
                        "Recorded query ({record_sql}) and actual query ({replay_sql}) do not match (unique_id: {unique_id})"
                    );
                }

                if let Some(msg) = entry.error {
                    return Err(AdbcError::with_message_and_status(
                        msg,
                        AdbcStatus::Internal,
                    ));
                }

                self.recorded_options = entry.options;

                entry
                    .data
                    .map(|(schema, batches)| {
                        Box::new(RecordBatchIterator::new(
                            batches.into_iter().map(Ok),
                            schema,
                        )) as Box<dyn RecordBatchReader + Send>
                    })
                    .ok_or_else(|| {
                        to_adbc_error(
                            RecordReplayError("Missing data in recording".to_string()),
                            Some(&path),
                        )
                    })
            }
            StorageType::FileArrowIpc | StorageType::FileParquet => {
                replay_file_execute(&path, &unique_id, replay_sql, storage_type, &self.config)
            }
        }
    }

    #[allow(deprecated)]
    fn execute_update(&mut self) -> AdbcResult<Option<i64>> {
        let replay_sql = match &self.sql {
            Some(sql) => sql,
            None => "none",
        };

        let path = self.recordings_path.clone();
        let unique_id = compute_file_name(
            &path,
            self.ctx.node_id.as_ref(),
            Some(replay_sql),
            self.ctx.metadata,
        )?;

        let storage_type = crate::storage::detect_storage_type(&path, &unique_id);

        match storage_type {
            StorageType::Sqlite => {
                let sqlite_handler = SqliteHandler::new(&path);
                let entry = sqlite_handler
                    .read_execute(&unique_id, replay_sql)
                    .map_err(|e| to_adbc_error(e, Some(&path)))?;

                let record_sql = entry.sql.as_deref().unwrap_or("none");
                if self.config.normalize_sql(record_sql) != self.config.normalize_sql(replay_sql) {
                    panic!(
                        "Recorded query ({record_sql}) and actual query ({replay_sql}) do not match (unique_id: {unique_id})"
                    );
                }

                if let Some(msg) = entry.error {
                    return Err(AdbcError::with_message_and_status(
                        msg,
                        AdbcStatus::Internal,
                    ));
                }

                self.recorded_options = entry.options;

                Ok(None)
            }
            StorageType::FileArrowIpc | StorageType::FileParquet => Ok(None),
        }
    }

    fn execute_schema(&mut self) -> AdbcResult<Schema> {
        todo!("ReplayStatement::execute_schema")
    }

    fn execute_partitions(&mut self) -> AdbcResult<adbc_core::PartitionedResult> {
        todo!("ReplayStatement::execute_partitions")
    }

    fn get_parameter_schema(&self) -> AdbcResult<Schema> {
        todo!("ReplayStatement::get_parameter_schema")
    }

    fn prepare(&mut self) -> AdbcResult<()> {
        todo!("ReplayStatement::prepare")
    }

    fn set_sql_query(&mut self, sql: &str) -> AdbcResult<()> {
        self.sql = Some(sql.to_string());
        Ok(())
    }

    fn set_substrait_plan(&mut self, _plan: &[u8]) -> AdbcResult<()> {
        unimplemented!("ReplayStatement::set_substrait_plan")
    }

    fn cancel(&mut self) -> AdbcResult<()> {
        todo!("ReplayStatement::cancel")
    }

    fn set_option(&mut self, key: OptionStatement, value: OptionValue) -> AdbcResult<()> {
        if let OptionStatement::Other(ref name) = key {
            self.ctx.absorb_option(name, &value);
        }
        Ok(())
    }

    fn get_option_string(&self, key: OptionStatement) -> AdbcResult<String> {
        // Serve back what the live driver reported for this option when the
        // recording was made. Anything not captured keeps the historical
        // behavior of reporting an empty value rather than an error, so replays
        // that never depended on an option are unaffected.
        if let OptionStatement::Other(ref name) = key
            && let Some(value) = self.recorded_options.get(name)
        {
            return Ok(value.clone());
        }
        Ok(String::new())
    }
}

// ---------------------------------------------------------------------------
// Deprecated file-based replay helpers
// ---------------------------------------------------------------------------

#[allow(deprecated)]
fn replay_file_schema(
    path: &Path,
    unique_id: &str,
    storage_type: StorageType,
) -> AdbcResult<Schema> {
    use crate::storage::file::{FileFormat, FileHandler};

    let handler = if storage_type == StorageType::FileArrowIpc {
        FileHandler::new_for_replay(FileFormat::ArrowIPC)
    } else {
        FileHandler::new_for_replay(FileFormat::Parquet)
    };

    if let Some(msg) = handler
        .read_error(path, unique_id)
        .map_err(|e| to_adbc_error(e, Some(path)))?
    {
        return Err(AdbcError::with_message_and_status(
            msg,
            AdbcStatus::Internal,
        ));
    }

    let schema = handler
        .read_schema(path, unique_id)
        .map_err(|e| to_adbc_error(e, Some(path)))?;
    Ok(schema)
}

#[allow(deprecated)]
fn replay_file_execute<'a>(
    path: &Path,
    unique_id: &str,
    replay_sql: &str,
    storage_type: StorageType,
    config: &SharedConfig,
) -> AdbcResult<Box<dyn RecordBatchReader + Send + 'a>> {
    use crate::storage::file::{FileFormat, FileHandler};

    let (data_path, handler) = if storage_type == StorageType::FileArrowIpc {
        (
            path.join(format!("{unique_id}.arrow")),
            FileHandler::new_for_replay(FileFormat::ArrowIPC),
        )
    } else {
        (
            path.join(format!("{unique_id}.parquet")),
            FileHandler::new_for_replay(FileFormat::Parquet),
        )
    };

    let sql_path = path.join(format!("{unique_id}.sql"));

    if !sql_path.exists() {
        panic!(
            "Missing query file ({:?}) during replay. Query: {}",
            &sql_path, replay_sql,
        );
    }

    let record_sql = handler
        .read_sql(path, unique_id)
        .map_err(|e| to_adbc_error(e, Some(path)))?;
    if config.normalize_sql(&record_sql) != config.normalize_sql(replay_sql) {
        panic!(
            "Recorded query ({record_sql}) and actual query ({replay_sql}) do not match ({sql_path:?})"
        );
    }

    if let Some(msg) = handler
        .read_error(path, unique_id)
        .map_err(|e| to_adbc_error(e, Some(path)))?
    {
        return Err(AdbcError::with_message_and_status(
            msg,
            AdbcStatus::Internal,
        ));
    }

    let reader = handler
        .read_batches(&data_path)
        .map_err(|e| to_adbc_error(e, Some(&data_path)))?;
    Ok(reader)
}
