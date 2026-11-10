use arrow::array::RecordBatch;
use arrow::ipc::reader::StreamReader as ArrowStreamReader;
use arrow::ipc::writer::StreamWriter as ArrowStreamWriter;
use arrow_schema::Schema;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use rusqlite::params;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{RECORDS_NAME, fix_decimal_precision_in_schema};
use crate::error::RecordReplayError;

#[derive(Debug)]
pub(crate) struct RecordingEntry {
    pub sql: Option<String>,
    pub error: Option<String>,
    pub data: Option<(Arc<Schema>, Vec<RecordBatch>)>,
    /// Read-only statement options captured off the live statement after the
    /// recorded execute, keyed by ADBC option name. Replayed back by
    /// `ReplayStatement::get_option_string`, which otherwise has no way to
    /// reproduce a value that only ever existed on the driver.
    pub options: BTreeMap<String, String>,
}

fn decode_arrow_ipc(
    data_base64: &str,
) -> Result<(Arc<Schema>, Vec<RecordBatch>), RecordReplayError> {
    let bytes = BASE64_STANDARD
        .decode(data_base64)
        .map_err(|e| RecordReplayError(format!("Could not decode base64: {e}")))?;
    let reader = ArrowStreamReader::try_new(Cursor::new(bytes), None)?;
    let schema = reader.schema();
    let batches = reader
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| RecordReplayError(format!("Could not collect record batches: {e}")))?;
    Ok((schema, batches))
}

/// Decode the `options_json` column. A malformed or absent value is treated as
/// "no options recorded" rather than an error: every recording made before
/// options were captured has this column NULL, and a replay of one must keep
/// working exactly as it did before.
fn decode_options(options_json: Option<String>) -> BTreeMap<String, String> {
    options_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default()
}

impl RecordingEntry {
    fn try_from_sqlite_row(
        sql: Option<String>,
        data_base64: Option<String>,
        error: Option<String>,
        options_json: Option<String>,
    ) -> Result<Self, RecordReplayError> {
        Ok(Self {
            sql,
            error,
            data: data_base64
                .map(|data| decode_arrow_ipc(&data))
                .transpose()?,
            options: decode_options(options_json),
        })
    }
}

/// Serialize recorded statement options for the `options_json` column. `None`
/// (SQL NULL) when nothing was captured, so a recording that observed no
/// options is byte-identical to one made before this column existed.
fn encode_options(options: &BTreeMap<String, String>) -> Result<Option<String>, RecordReplayError> {
    if options.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(options)
        .map(Some)
        .map_err(|e| RecordReplayError(format!("Could not encode statement options: {e}")))
}

pub(crate) struct SqliteHandler {
    db_path: PathBuf,
}

impl SqliteHandler {
    pub fn new(recordings_dir: &Path) -> Self {
        Self {
            db_path: recordings_dir.join(RECORDS_NAME),
        }
    }

    #[cfg(test)]
    pub fn exists(&self) -> bool {
        self.db_path.exists()
    }

    pub fn connect(&self) -> Result<rusqlite::Connection, RecordReplayError> {
        let conn = rusqlite::Connection::open(&self.db_path)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS recordings (
                unique_id TEXT NOT NULL,
                record_type TEXT NOT NULL,
                sql TEXT,
                data_base64 TEXT,
                error TEXT,
                options_json TEXT,
                PRIMARY KEY (unique_id, record_type)
            )",
            [],
        )?;
        Ok(conn)
    }

    /// Whether this recordings.db has the `options_json` column. Recordings
    /// committed before statement options were captured don't, and replaying
    /// one must not write to it -- an `ALTER TABLE` on the read path would
    /// modify a committed fixture just by running the test suite against it.
    fn has_options_column(conn: &rusqlite::Connection) -> Result<bool, RecordReplayError> {
        Ok(conn
            .prepare("SELECT 1 FROM pragma_table_info('recordings') WHERE name = 'options_json'")?
            .exists([])?)
    }

    /// Add `options_json` to a recordings.db that predates it. Only called from
    /// write paths, which are already modifying the file.
    fn add_options_column_if_missing(conn: &rusqlite::Connection) -> Result<(), RecordReplayError> {
        if Self::has_options_column(conn)? {
            return Ok(());
        }
        conn.execute("ALTER TABLE recordings ADD COLUMN options_json TEXT", [])?;
        Ok(())
    }

    /// `options_json` if the column is present, else a NULL placeholder, so the
    /// readers below always see the same column count.
    fn options_column_expr(conn: &rusqlite::Connection) -> Result<&'static str, RecordReplayError> {
        Ok(if Self::has_options_column(conn)? {
            "options_json"
        } else {
            "NULL"
        })
    }

    pub fn encode_arrow_ipc(
        &self,
        batches: &[RecordBatch],
        schema: Arc<Schema>,
    ) -> Result<String, RecordReplayError> {
        let mut buffer = Vec::new();
        {
            let mut writer = ArrowStreamWriter::try_new(&mut buffer, &schema)?;
            for batch in batches {
                writer.write(batch)?;
            }
            writer.finish()?;
        }
        Ok(BASE64_STANDARD.encode(&buffer))
    }

    pub fn write_execute(
        &self,
        unique_id: &str,
        sql: &str,
        batches: &[RecordBatch],
        schema: Arc<Schema>,
        options: &BTreeMap<String, String>,
    ) -> Result<(), RecordReplayError> {
        let conn = self.connect()?;
        Self::add_options_column_if_missing(&conn)?;
        let data_base64 = self.encode_arrow_ipc(batches, schema)?;
        conn.execute(
            "INSERT OR REPLACE INTO recordings
                (unique_id, record_type, sql, data_base64, error, options_json)
             VALUES (?1, 'execute', ?2, ?3, NULL, ?4)",
            params![unique_id, sql, data_base64, encode_options(options)?],
        )?;
        Ok(())
    }

    pub fn write_execute_error(
        &self,
        unique_id: &str,
        sql: &str,
        error_msg: &str,
    ) -> Result<(), RecordReplayError> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT OR REPLACE INTO recordings (unique_id, record_type, sql, data_base64, error)
             VALUES (?1, 'execute', ?2, NULL, ?3)",
            params![unique_id, sql, error_msg],
        )?;
        Ok(())
    }

    pub fn write_schema(&self, unique_id: &str, schema: &Schema) -> Result<(), RecordReplayError> {
        let fixed_schema = fix_decimal_precision_in_schema(schema);
        let schema_ref = Arc::new(fixed_schema);
        let empty_batch = RecordBatch::new_empty(schema_ref.clone());
        let data_base64 = self.encode_arrow_ipc(&[empty_batch], schema_ref)?;

        let conn = self.connect()?;
        conn.execute(
            "INSERT OR REPLACE INTO recordings (unique_id, record_type, sql, data_base64, error)
             VALUES (?1, 'get_table_schema', NULL, ?2, NULL)",
            params![unique_id, data_base64],
        )?;
        Ok(())
    }

    pub fn write_schema_error(
        &self,
        unique_id: &str,
        error_msg: &str,
    ) -> Result<(), RecordReplayError> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT OR REPLACE INTO recordings (unique_id, record_type, sql, data_base64, error)
             VALUES (?1, 'get_table_schema', NULL, NULL, ?2)",
            params![unique_id, error_msg],
        )?;
        Ok(())
    }

    pub fn write_objects(
        &self,
        unique_id: &str,
        batches: &[RecordBatch],
        schema: Arc<Schema>,
    ) -> Result<(), RecordReplayError> {
        let conn = self.connect()?;
        let data_base64 = self.encode_arrow_ipc(batches, schema)?;
        conn.execute(
            "INSERT OR REPLACE INTO recordings (unique_id, record_type, sql, data_base64, error)
             VALUES (?1, 'get_objects', NULL, ?2, NULL)",
            params![unique_id, data_base64],
        )?;
        Ok(())
    }

    pub fn write_objects_error(
        &self,
        unique_id: &str,
        error_msg: &str,
    ) -> Result<(), RecordReplayError> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT OR REPLACE INTO recordings (unique_id, record_type, sql, data_base64, error)
             VALUES (?1, 'get_objects', NULL, NULL, ?2)",
            params![unique_id, error_msg],
        )?;
        Ok(())
    }

    pub fn read_execute(
        &self,
        unique_id: &str,
        replay_sql: &str,
    ) -> Result<RecordingEntry, RecordReplayError> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT sql, data_base64, error, {options} FROM recordings
                 WHERE unique_id = ?1 AND record_type = 'execute'",
            options = Self::options_column_expr(&conn)?
        ))?;
        stmt.query_row(params![unique_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(|e| {
            RecordReplayError(format!(
                "Could not query row for replay sql ({replay_sql}): {e}"
            ))
        })
        .and_then(|(sql, data, err, options)| {
            RecordingEntry::try_from_sqlite_row(sql, data, err, options)
        })
    }

    pub fn read_schema(&self, unique_id: &str) -> Result<RecordingEntry, RecordReplayError> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT sql, data_base64, error, {options} FROM recordings
                 WHERE unique_id = ?1 AND record_type = 'get_table_schema'",
            options = Self::options_column_expr(&conn)?
        ))?;
        stmt.query_row(params![unique_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(|e| RecordReplayError(format!("Could not query row: {e}")))
        .and_then(|(sql, data, err, options)| {
            RecordingEntry::try_from_sqlite_row(sql, data, err, options)
        })
    }

    pub fn read_objects(&self, unique_id: &str) -> Result<RecordingEntry, RecordReplayError> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT sql, data_base64, error, {options} FROM recordings
                 WHERE unique_id = ?1 AND record_type = 'get_objects'",
            options = Self::options_column_expr(&conn)?
        ))?;
        stmt.query_row(params![unique_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map_err(|e| RecordReplayError(format!("Could not query row: {e}")))
        .and_then(|(sql, data, err, options)| {
            RecordingEntry::try_from_sqlite_row(sql, data, err, options)
        })
    }

    pub(crate) fn read_all_rows(&self) -> Result<Vec<RecordingEntry>, RecordReplayError> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT sql, data_base64, error, {options} FROM recordings",
            options = Self::options_column_expr(&conn)?
        ))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|e| RecordReplayError(format!("Could not query row: {e}")))?;

        let mut out = Vec::new();
        for row in rows {
            let (sql, data, err, options) = row?;
            out.push(RecordingEntry::try_from_sqlite_row(
                sql, data, err, options,
            )?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int32Array, StringArray};
    use arrow_schema::{DataType, Field};
    use std::collections::HashMap;

    #[test]
    fn execute_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let handler = SqliteHandler::new(dir.path());

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new("name", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int32Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec![Some("alice"), Some("bob"), None])),
            ],
        )
        .unwrap();

        let unique_id = "test-node-0";
        let sql = "SELECT * FROM users";
        handler
            .write_execute(unique_id, sql, &[batch], schema, &BTreeMap::new())
            .unwrap();

        let entry = handler.read_execute(unique_id, sql).unwrap();
        assert_eq!(entry.sql.as_deref(), Some(sql));
        assert!(entry.error.is_none());
        assert!(entry.data.is_some());

        let (schema, batches) = entry.data.unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 3);
        assert_eq!(schema.fields().len(), 2);
    }

    #[test]
    fn execute_options_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let handler = SqliteHandler::new(dir.path());
        let options = BTreeMap::from([(
            "adbc.dbt.last_warnings".to_string(),
            "results were truncated".to_string(),
        )]);

        handler
            .write_execute(
                "node-0",
                "SELECT 1",
                &[],
                Arc::new(Schema::empty()),
                &options,
            )
            .unwrap();

        let entry = handler.read_execute("node-0", "SELECT 1").unwrap();
        assert_eq!(entry.options, options);
    }

    #[test]
    fn execute_options_absent_reads_as_empty() {
        // A recording written before `options_json` existed: `connect()` adds
        // the column, and the NULL it leaves behind must read as "no options"
        // rather than failing the whole replay.
        let dir = tempfile::tempdir().unwrap();
        let handler = SqliteHandler::new(dir.path());
        let conn = handler.connect().unwrap();
        conn.execute("ALTER TABLE recordings DROP COLUMN options_json", [])
            .unwrap();
        conn.execute(
            "INSERT INTO recordings (unique_id, record_type, sql, data_base64, error)
             VALUES ('node-0', 'execute', 'SELECT 1', NULL, NULL)",
            [],
        )
        .unwrap();
        drop(conn);

        let entry = handler.read_execute("node-0", "SELECT 1").unwrap();
        assert!(entry.options.is_empty());
    }

    #[test]
    fn execute_error_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let handler = SqliteHandler::new(dir.path());

        let unique_id = "test-error-0";
        let sql = "SELECT * FROM nonexistent";
        let error_msg = "Table not found: nonexistent";

        handler
            .write_execute_error(unique_id, sql, error_msg)
            .unwrap();

        let entry = handler.read_execute(unique_id, sql).unwrap();
        assert_eq!(entry.sql.as_deref(), Some(sql));
        assert_eq!(entry.error.as_deref(), Some(error_msg));
        assert!(entry.data.is_none());
    }

    #[test]
    fn schema_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let handler = SqliteHandler::new(dir.path());

        let mut metadata = HashMap::new();
        metadata.insert("custom_key".to_string(), "custom_value".to_string());
        let schema = Schema::new(vec![
            Field::new("col1", DataType::Float64, false),
            Field::new("col2", DataType::Boolean, true),
        ])
        .with_metadata(metadata);

        let unique_id = "node-0.get_table_schema";

        handler.write_schema(unique_id, &schema).unwrap();

        let entry = handler.read_schema(unique_id).unwrap();
        assert!(entry.sql.is_none());
        assert!(entry.error.is_none());
        assert!(entry.data.is_some());

        let (read_schema, _) = entry.data.unwrap();
        assert_eq!(read_schema.fields().len(), 2);
        assert_eq!(read_schema.field(0).name(), "col1");
        assert_eq!(read_schema.field(1).name(), "col2");
        assert_eq!(
            read_schema.metadata().get("custom_key"),
            Some(&"custom_value".to_string())
        );
    }

    #[test]
    fn schema_error_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let handler = SqliteHandler::new(dir.path());

        let unique_id = "node-1.get_table_schema";
        let error_msg = "Schema not found";

        handler.write_schema_error(unique_id, error_msg).unwrap();

        let entry = handler.read_schema(unique_id).unwrap();
        assert_eq!(entry.error.as_deref(), Some(error_msg));
        assert!(entry.data.is_none());
    }

    #[test]
    fn exists() {
        let dir = tempfile::tempdir().unwrap();
        let handler = SqliteHandler::new(dir.path());

        assert!(!handler.exists());
        handler.connect().unwrap();
        assert!(handler.exists());
    }

    #[test]
    fn empty_batches() {
        let dir = tempfile::tempdir().unwrap();
        let handler = SqliteHandler::new(dir.path());

        let schema = Arc::new(Schema::new(vec![Field::new("x", DataType::Int32, false)]));
        let unique_id = "empty-0";
        let sql = "SELECT x FROM empty_table";

        handler
            .write_execute(unique_id, sql, &[], schema, &BTreeMap::new())
            .unwrap();

        let entry = handler.read_execute(unique_id, sql).unwrap();

        let (schema, batches) = entry.data.unwrap();
        assert!(batches.is_empty());
        assert_eq!(schema.fields().len(), 1);
        assert_eq!(schema.field(0).name(), "x");
    }

    #[test]
    #[allow(deprecated)]
    fn detect_storage_types() {
        use crate::storage::{RECORDS_NAME, detect_storage_type};

        let dir = tempfile::tempdir().unwrap();

        assert_eq!(
            detect_storage_type(dir.path(), "test-0"),
            crate::storage::StorageType::FileParquet
        );

        let db_path = dir.path().join(RECORDS_NAME);
        std::fs::write(&db_path, "").unwrap();
        assert_eq!(
            detect_storage_type(dir.path(), "test-0"),
            crate::storage::StorageType::Sqlite
        );

        std::fs::remove_file(&db_path).unwrap();
        let arrow_path = dir.path().join("test-0.arrow");
        std::fs::write(&arrow_path, "").unwrap();
        #[allow(deprecated)]
        {
            assert_eq!(
                detect_storage_type(dir.path(), "test-0"),
                crate::storage::StorageType::FileArrowIpc
            );
        }
    }
}
