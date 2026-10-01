use arrow_array::RecordBatch;
use dbt_adapter_core::AdapterType;
use dbt_adbc as adbc;
use dbt_agate::AgateTable;
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Enumerator, Object, ValueMap};
use minijinja::{State, Value};
use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use std::rc::Rc;
use std::sync::Arc;

use crate::engine::AdapterEngine;
use crate::record_batch::{RecordBatchExt, SNOWFLAKE_DML_COLUMNS, SchemaExt};
use crate::value::none_value;

const KEY_MESSAGE: &str = "message";
const KEY_MESSAGE_ALIAS: &str = "_message";
const KEY_CODE: &str = "code";
const KEY_ROWS_AFFECTED: &str = "rows_affected";
const KEY_QUERY_ID: &str = "query_id";
const KEY_BYTES_PROCESSED: &str = "bytes_processed";
const KEY_BYTES_BILLED: &str = "bytes_billed";
const KEY_SLOT_MS: &str = "slot_ms";
const KEY_LOCATION: &str = "location";
const KEY_PROJECT_ID: &str = "project_id";
const KEY_JOB_ID: &str = "job_id";
/// Non-fatal backend warning text (e.g. a LakeCompute export-limit
/// truncation notice), kept distinct from `message` (which always carries
/// a status/row-count summary) so callers can detect "was there a warning"
/// without parsing prose.
const KEY_WARNING: &str = "warning";

const KNOWN_KEYS: &[&str] = &[KEY_MESSAGE, KEY_CODE, KEY_ROWS_AFFECTED, KEY_QUERY_ID];

/// Response from adapter statement execution.
///
/// A transparent map keyed by string values. Well-known keys (`message`,
/// `code`, `rows_affected`, `query_id`) are exposed via accessors with defaults
/// matching the historical field layout; arbitrary extra keys are preserved.
#[derive(Debug, Clone, PartialEq)]
pub struct AdapterResponse(ValueMap);

impl AdapterResponse {
    /// Start building an empty response.
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        let m = message.into();
        self.0
            .insert(Value::from(KEY_MESSAGE), Value::from(m.clone()));
        self.0
            .insert(Value::from(KEY_MESSAGE_ALIAS), Value::from(m));
        self
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.0
            .insert(Value::from(KEY_CODE), Value::from(code.into()));
        self
    }

    pub fn with_rows_affected(mut self, rows_affected: i64) -> Self {
        self.0
            .insert(Value::from(KEY_ROWS_AFFECTED), Value::from(rows_affected));
        self
    }

    pub fn with_query_id(mut self, query_id: impl Into<String>) -> Self {
        self.0
            .insert(Value::from(KEY_QUERY_ID), Value::from(query_id.into()));
        self
    }

    /// Insert an arbitrary key/value pair.
    pub fn with(mut self, key: impl Into<Value>, value: impl Into<Value>) -> Self {
        self.0.insert(key.into(), value.into());
        self
    }

    fn get_str(&self, key: &str) -> Option<String> {
        self.0
            .get(&Value::from(key))
            .and_then(|v| v.as_str().map(String::from))
    }

    pub fn message(&self) -> String {
        self.get_str(KEY_MESSAGE).unwrap_or_default()
    }

    pub fn code(&self) -> String {
        self.get_str(KEY_CODE).unwrap_or_default()
    }

    pub fn query_id(&self) -> Option<String> {
        self.get_str(KEY_QUERY_ID)
    }

    /// Non-fatal backend warning text, if the executed statement's response
    /// carried one (e.g. a LakeCompute export-limit truncation notice).
    pub fn warning(&self) -> Option<String> {
        self.get_str(KEY_WARNING)
    }

    /// Rows affected, clamping the `-1` unknown sentinel (and any negative) to 0.
    pub fn rows_affected(&self) -> u64 {
        u64::try_from(self.rows_affected_i64()).unwrap_or(0)
    }

    /// Raw rows-affected value; negative (e.g. `-1`) means unknown.
    pub fn rows_affected_i64(&self) -> i64 {
        self.0
            .get(&Value::from(KEY_ROWS_AFFECTED))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    }

    /// Build the response for an executed statement from its result batch.
    ///
    /// Uses schema metadata to build adapter-specific fields.
    pub fn from_record_batch(batch: &RecordBatch, adapter_type: AdapterType) -> Self {
        let (code, rows) = code_and_rows(adapter_type, batch);

        let message = match adapter_type {
            AdapterType::Bigquery => {
                use adbc::bigquery::schema_metadata::*;
                let bytes_processed = batch
                    .meta_i64(QUERY_TOTAL_BYTES_PROCESSED)
                    .or_else(|| batch.meta_i64(JOB_TOTAL_BYTES_PROCESSED));

                bytes_processed
                    .map(|bp| {
                        if rows == 0 {
                            format!("{code} ({} processed)", format_bytes(bp),)
                        } else {
                            format!(
                                "{code} ({} rows, {} processed)",
                                format_rows_number(rows),
                                format_bytes(bp),
                            )
                        }
                    })
                    .unwrap_or_else(|| format!("{code} ({} rows)", format_rows_number(rows),))
            }
            _ => format!("{code} {rows}"),
        };

        let mut response = Self::new()
            .with_message(message)
            .with_code(code)
            .with_rows_affected(rows);

        let query_id = query_id_from_record_batch(batch, adapter_type);
        if let Some(query_id) = query_id {
            response = response.with_query_id(query_id);
        }

        if adapter_type == AdapterType::LakeCompute
            && let Some(warnings) = batch.meta_string(adbc::lake_compute::schema_metadata::WARNINGS)
            && !warnings.is_empty()
        {
            let message = format!("{} {warnings}", response.message());
            response = response.with_message(message).with(KEY_WARNING, warnings);
        }

        // Add adapter-specific fields to the AdapterResponse
        #[expect(clippy::single_match)]
        match adapter_type {
            AdapterType::Bigquery => {
                use adbc::bigquery::schema_metadata::*;

                let bytes_processed = batch
                    .meta_i64(QUERY_TOTAL_BYTES_PROCESSED)
                    .or_else(|| batch.meta_i64(JOB_TOTAL_BYTES_PROCESSED))
                    .unwrap_or(0);
                response = response.with(KEY_BYTES_PROCESSED, bytes_processed);

                if let Some(bytes_billed) = batch.meta_i64(TOTAL_BYTES_BILLED) {
                    response = response.with(KEY_BYTES_BILLED, bytes_billed);
                }
                if let Some(slot_ms) = batch.meta_i64(SLOT_MILLIS) {
                    response = response.with(KEY_SLOT_MS, slot_ms);
                }
                if let Some(job_id) = batch.meta_string(QUERY_ID) {
                    response = response.with(KEY_JOB_ID, job_id);
                }
            }
            _ => {}
        }

        response
    }

    /// Add the parts of the response that describe the connection rather than the
    /// statement, and so cannot be read off the result batch.
    pub fn with_connection_info(
        mut self,
        adapter_type: AdapterType,
        engine: &dyn AdapterEngine,
    ) -> Self {
        if adapter_type != AdapterType::Bigquery {
            return self;
        }
        if let Some(location) = engine.config(KEY_LOCATION) {
            self = self.with(KEY_LOCATION, location.into_owned());
        }
        // The project jobs are billed to, which is not necessarily the project the
        // relations live in.
        let project_id = engine
            .config("execution_project")
            .or_else(|| engine.config("project"))
            .or_else(|| engine.config("database"));
        if let Some(project_id) = project_id {
            self = self.with(KEY_PROJECT_ID, project_id.into_owned());
        }
        self
    }
}

pub(crate) fn query_id_from_record_batch(
    batch: &RecordBatch,
    adapter_type: AdapterType,
) -> Option<String> {
    match adapter_type {
        AdapterType::Snowflake => batch.meta_string(adbc::snowflake::schema_metadata::QUERY_ID),
        AdapterType::Bigquery => batch.meta_string(adbc::bigquery::schema_metadata::QUERY_ID),
        AdapterType::Databricks => batch.meta_string(adbc::databricks::schema_metadata::QUERY_ID),
        AdapterType::LakeCompute => {
            batch.meta_string(adbc::lake_compute::schema_metadata::QUERY_ID)
        }
        _ => None,
    }
}

fn rows_affected(batch: &RecordBatch, adapter_type: AdapterType) -> i64 {
    if let Some(rows) = batch.meta_i64(crate::record_batch::ROWS_AFFECTED_META) {
        return rows;
    }
    if batch.num_rows() == 0 {
        return 0;
    }
    if batch.schema().has_dml_columns(adapter_type) {
        return SNOWFLAKE_DML_COLUMNS
            .iter()
            .filter_map(|col| batch.named_value_as_i64(col))
            .sum();
    }
    batch.num_rows() as i64
}

/// How to describe the statement the warehouse ran, and how many rows it touched.
///
/// Warehouses that report the kind of statement they ran are described the way dbt
/// Core describes them; everything else falls back to `SUCCESS` and the row count
/// the batch itself carries. A row count of `None` means the statement has no row
/// count, as opposed to a count of zero.
fn code_and_rows(adapter_type: AdapterType, batch: &RecordBatch) -> (String, i64) {
    let schema = batch.schema();
    let metadata = schema.metadata();
    let batch_rows = batch.num_rows() as i64;
    let fallback = || ("SUCCESS".to_string(), rows_affected(batch, adapter_type));

    match adapter_type {
        AdapterType::Bigquery => {
            use adbc::bigquery::schema_metadata as bq;
            let Some(statement_type) = metadata.get(bq::STATEMENT_TYPE) else {
                return fallback();
            };
            let dml_affected_rows = metadata
                .get(bq::NUM_DML_AFFECTED_ROWS)
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0);
            match statement_type.as_str() {
                "CREATE_VIEW" => ("CREATE VIEW".to_string(), 0),
                // dbt Core reads the row count off the destination table, which
                // costs an extra round trip. An unfetched batch reports zero rows.
                "CREATE_TABLE_AS_SELECT" => ("CREATE TABLE".to_string(), batch_rows),
                "SCRIPT" => ("SCRIPT".to_string(), 0),
                "INSERT" | "DELETE" | "MERGE" | "UPDATE" => {
                    (statement_type.clone(), dml_affected_rows)
                }
                "SELECT" => ("SELECT".to_string(), batch_rows),
                _ => fallback(),
            }
        }
        _ => fallback(),
    }
}

fn format_bytes(num_bytes: i64) -> String {
    // dbt Core renders a falsy byte count as-is rather than as "0.0 Bytes".
    if num_bytes == 0 {
        return "0".to_string();
    }

    let mut num_bytes = num_bytes as f64;
    for unit in ["Bytes", "KiB", "MiB", "GiB", "TiB"] {
        if num_bytes.abs() < 1024.0 {
            return format!("{num_bytes:.1} {unit}");
        }
        num_bytes /= 1024.0;
    }
    format!("{num_bytes:.1} PiB")
}

fn format_rows_number(rows_number: i64) -> String {
    let mut rows_number = rows_number as f64;
    for unit in ["", "k", "m", "b"] {
        if rows_number.abs() < 1000.0 {
            return format!("{rows_number:.1}{unit}");
        }
        rows_number /= 1000.0;
    }
    format!("{rows_number:.1}t")
}

impl Default for AdapterResponse {
    /// Build a response with the canonical field layout, adding defaults for
    /// `message`, `code`, and `rows_affected`.
    fn default() -> AdapterResponse {
        let mut map = ValueMap::default();
        map.insert(Value::from(KEY_MESSAGE), Value::from(String::default()));
        map.insert(Value::from(KEY_CODE), Value::from(String::default()));
        map.insert(Value::from(KEY_ROWS_AFFECTED), Value::from(0i64));
        Self(map)
    }
}

impl Serialize for AdapterResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AdapterResponse {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(AdapterResponse(ValueMap::deserialize(deserializer)?))
    }
}

impl Object for AdapterResponse {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        Some(self.0.get(key).cloned().unwrap_or_else(none_value))
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        let mut keys: Vec<Value> = KNOWN_KEYS.iter().map(|k| Value::from(*k)).collect();
        for key in self.0.keys() {
            let is_known = key.as_str().is_some_and(|s| KNOWN_KEYS.contains(&s));
            if !is_known {
                keys.push(key.clone());
            }
        }
        Enumerator::Iter(Box::new(keys.into_iter()))
    }
}

impl TryFrom<Value> for AdapterResponse {
    type Error = minijinja::Error;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        if let Some(response) = value.downcast_object::<AdapterResponse>() {
            Ok((*response).clone())
        } else if let Some(message_str) = value.as_str() {
            Ok(AdapterResponse::new().with_message(message_str))
        } else {
            Err(minijinja::Error::new(
                minijinja::ErrorKind::CannotDeserialize,
                "Failed to downcast response",
            ))
        }
    }
}

/// load_result response object
#[derive(Debug)]
pub struct ResultObject {
    pub response: AdapterResponse,
    pub table: Option<AgateTable>,
    #[allow(unused)]
    pub data: Option<Value>,
}

impl ResultObject {
    pub fn new(response: AdapterResponse, table: Option<AgateTable>) -> Self {
        let data = if let Some(table) = &table {
            Some(Value::from_object(table.rows()))
        } else {
            Some(Value::UNDEFINED)
        };
        Self {
            response,
            table,
            data,
        }
    }
}

impl Object for ResultObject {
    fn call_method(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        method: &str,
        _args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        // NOTE: the `keys` method is used by the `stage_external_sources` macro in
        // `dbt-external-table`. Don't delete this unless the external package is fixed.
        if method == "keys" {
            Ok(Value::from_iter(["response", "table", "data"]))
        } else {
            Err(minijinja::Error::new(
                minijinja::ErrorKind::UnknownMethod,
                format!("Unknown method on ResultObject: '{method}'"),
            ))
        }
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "table" => self
                .table
                .as_ref()
                .map(|t| Value::from_object((*t).clone())),
            "data" => self.data.clone(),
            "response" => Some(Value::from_object(self.response.clone())),
            _ => Some(Value::UNDEFINED), // Only return empty at Parsetime TODO fix later
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Str(&["table", "data", "response"])
    }
}

#[cfg(test)]
mod from_record_batch_tests {
    use super::*;
    use arrow::array::{ArrayRef, Decimal128Array, Int64Array};
    use arrow::datatypes::{Field, Schema};
    use std::collections::HashMap;

    /// A batch of `values`, one column per `(name, array)` pair, carrying `metadata`
    /// on its schema.
    fn batch(columns: Vec<(&str, ArrayRef)>, metadata: &[(&str, &str)]) -> RecordBatch {
        let fields: Vec<Field> = columns
            .iter()
            .map(|(name, array)| Field::new(*name, array.data_type().clone(), true))
            .collect();
        let metadata: HashMap<String, String> = metadata
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let schema = Arc::new(Schema::new_with_metadata(fields, metadata));
        let arrays = columns.into_iter().map(|(_, array)| array).collect();
        RecordBatch::try_new(schema, arrays).expect("columns match the derived schema")
    }

    fn int_column(name: &str, values: Vec<Option<i64>>) -> (&str, ArrayRef) {
        (name, Arc::new(Int64Array::from(values)) as ArrayRef)
    }

    fn select_batch(rows: i64, metadata: &[(&str, &str)]) -> RecordBatch {
        batch(
            vec![int_column("id", (0..rows).map(Some).collect())],
            metadata,
        )
    }

    fn dml_batch(inserted: i64, updated: i64, deleted: i64) -> RecordBatch {
        batch(
            vec![
                int_column("number of rows inserted", vec![Some(inserted)]),
                int_column("number of rows updated", vec![Some(updated)]),
                int_column("number of rows deleted", vec![Some(deleted)]),
            ],
            &[],
        )
    }

    fn bigquery(rows: i64, metadata: &[(&str, &str)]) -> AdapterResponse {
        AdapterResponse::from_record_batch(&select_batch(rows, metadata), AdapterType::Bigquery)
    }

    fn json(response: &AdapterResponse) -> serde_json::Value {
        serde_json::to_value(response).expect("response serialises to JSON")
    }

    #[test]
    fn test_snowflake_merge_sums_dml_counts() {
        let batch = dml_batch(100, 50, 10);
        let response = AdapterResponse::from_record_batch(&batch, AdapterType::Snowflake);
        assert_eq!(response.rows_affected_i64(), 160);
        assert_eq!(response.message(), "SUCCESS 160");
        assert_eq!(response.code(), "SUCCESS");

        // The DML columns are Snowflake's convention; elsewhere they are just columns.
        let response = AdapterResponse::from_record_batch(&batch, AdapterType::Bigquery);
        assert_eq!(response.rows_affected_i64(), 1);
    }

    #[test]
    fn test_snowflake_merge_decimal128_high_precision() {
        let decimal = |value: i128| {
            Arc::new(
                Decimal128Array::from(vec![value])
                    .with_precision_and_scale(38, 0)
                    .expect("38 digits with no scale is a valid decimal"),
            ) as ArrayRef
        };
        let batch = batch(
            vec![
                ("number of rows inserted", decimal(200)),
                ("number of rows updated", decimal(75)),
                ("number of rows deleted", decimal(25)),
            ],
            &[],
        );
        let response = AdapterResponse::from_record_batch(&batch, AdapterType::Snowflake);
        assert_eq!(response.rows_affected_i64(), 300);
    }

    #[test]
    fn test_snowflake_insert_only_partial_dml_columns() {
        let batch = batch(
            vec![int_column("number of rows inserted", vec![Some(42)])],
            &[],
        );
        let response = AdapterResponse::from_record_batch(&batch, AdapterType::Snowflake);
        assert_eq!(response.rows_affected_i64(), 42);
    }

    #[test]
    fn test_snowflake_null_dml_values_treated_as_zero() {
        let batch = batch(
            vec![
                int_column("number of rows inserted", vec![Some(50)]),
                int_column("number of rows updated", vec![None]),
                int_column("number of rows deleted", vec![None]),
            ],
            &[],
        );
        let response = AdapterResponse::from_record_batch(&batch, AdapterType::Snowflake);
        assert_eq!(response.rows_affected_i64(), 50);
    }

    #[test]
    fn test_empty_batch_reports_no_rows() {
        let batch = RecordBatch::new_empty(dml_batch(1, 1, 1).schema());
        let response = AdapterResponse::from_record_batch(&batch, AdapterType::Snowflake);
        assert_eq!(response.rows_affected_i64(), 0);
        assert_eq!(response.message(), "SUCCESS 0");
    }

    #[test]
    fn test_alt_warning_metadata_surfaces_in_message_and_warning_key() {
        let batch = select_batch(
            3,
            &[(
                adbc::lake_compute::schema_metadata::WARNINGS,
                "lakecompute_information_schema.jobs: matched 50001 rows, which exceeds the 50000-row export limit",
            )],
        );
        let response = AdapterResponse::from_record_batch(&batch, AdapterType::LakeCompute);
        assert_eq!(
            response.warning().as_deref(),
            Some(
                "lakecompute_information_schema.jobs: matched 50001 rows, which exceeds the 50000-row export limit"
            )
        );
        assert!(
            response
                .message()
                .contains("lakecompute_information_schema.jobs: matched 50001 rows"),
            "expected warning text to be folded into the message, got: {:?}",
            response.message()
        );
    }

    #[test]
    fn test_alt_without_warning_metadata_has_no_warning() {
        let batch = select_batch(3, &[]);
        let response = AdapterResponse::from_record_batch(&batch, AdapterType::LakeCompute);
        assert_eq!(response.warning(), None);
    }

    #[test]
    fn test_select_uses_num_rows() {
        let response =
            AdapterResponse::from_record_batch(&select_batch(3, &[]), AdapterType::Snowflake);
        assert_eq!(response.rows_affected_i64(), 3);
    }

    #[test]
    fn test_query_id_is_read_per_adapter() {
        let metadata = [
            ("SNOWFLAKE_QUERY_ID", "snowflake-id"),
            ("BIGQUERY:query_id", "bigquery-id"),
            ("DATABRICKS_QUERY_ID", "databricks-id"),
        ];
        let batch = select_batch(1, &metadata);
        for (adapter_type, expected) in [
            (AdapterType::Snowflake, Some("snowflake-id")),
            (AdapterType::Bigquery, Some("bigquery-id")),
            (AdapterType::Databricks, Some("databricks-id")),
            (AdapterType::Postgres, None),
        ] {
            let response = AdapterResponse::from_record_batch(&batch, adapter_type);
            assert_eq!(response.query_id().as_deref(), expected);
        }
    }

    #[test]
    fn test_no_statistics_shows_rows() {
        let response = bigquery(1, &[]);
        assert_eq!(response.message(), "SUCCESS (1.0 rows)");
        assert_eq!(response.code(), "SUCCESS");
        let json = json(&response);
        assert_eq!(json["bytes_processed"], 0);
    }

    #[test]
    fn test_bigquery_select_reports_rows_and_statistics() {
        let response = bigquery(
            2,
            &[
                ("BIGQUERY:query_id", "job-abc"),
                ("BIGQUERY:Statistics:Query:StatementType", "SELECT"),
                ("BIGQUERY:Statistics:Query:TotalBytesProcessed", "1536"),
                ("BIGQUERY:Statistics:Query:TotalBytesBilled", "10485760"),
                ("BIGQUERY:Statistics:Query:SlotMillis", "7167"),
            ],
        );

        assert_eq!(response.message(), "SELECT (2.0 rows, 1.5 KiB processed)");
        assert_eq!(response.code(), "SELECT");
        assert_eq!(response.rows_affected_i64(), 2);

        let json = json(&response);
        assert_eq!(json["bytes_processed"], 1536);
        assert_eq!(json["bytes_billed"], 10485760i64);
        assert_eq!(json["slot_ms"], 7167);
        assert_eq!(json["job_id"], "job-abc");
        assert_eq!(json["query_id"], "job-abc");
    }

    #[test]
    fn test_bigquery_merge_uses_dml_affected_rows() {
        let response = bigquery(
            0,
            &[
                ("BIGQUERY:Statistics:Query:StatementType", "MERGE"),
                ("BIGQUERY:Statistics:Query:NumDMLAffectedRows", "1500"),
                ("BIGQUERY:Statistics:Query:TotalBytesProcessed", "0"),
            ],
        );

        assert_eq!(response.message(), "MERGE (1.5k rows, 0 processed)");
        assert_eq!(response.code(), "MERGE");
        assert_eq!(response.rows_affected_i64(), 1500);
    }

    #[test]
    fn test_bigquery_create_table_as_select_uses_batch_rows() {
        let response = bigquery(
            2,
            &[
                (
                    "BIGQUERY:Statistics:Query:StatementType",
                    "CREATE_TABLE_AS_SELECT",
                ),
                ("BIGQUERY:Statistics:Query:TotalBytesProcessed", "0"),
            ],
        );

        assert_eq!(response.message(), "CREATE TABLE (2.0 rows, 0 processed)");
        assert_eq!(response.code(), "CREATE TABLE");
    }

    #[test]
    fn test_bigquery_bytes_processed_falls_back_to_job_statistics() {
        let response = bigquery(
            1,
            &[
                ("BIGQUERY:Statistics:Query:StatementType", "SELECT"),
                ("BIGQUERY:Statistics:TotalBytesProcessed", "1024"),
            ],
        );
        assert_eq!(json(&response)["bytes_processed"], 1024);
    }

    #[test]
    fn test_statistics_are_bigquery_only() {
        let metadata = [
            ("BIGQUERY:Statistics:Query:StatementType", "SELECT"),
            ("BIGQUERY:Statistics:Query:SlotMillis", "42"),
        ];
        let response =
            AdapterResponse::from_record_batch(&select_batch(1, &metadata), AdapterType::Snowflake);
        assert_eq!(response.message(), "SUCCESS 1");
        assert_eq!(json(&response).get("slot_ms"), None);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0");
        assert_eq!(format_bytes(512), "512.0 Bytes");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(format_bytes(3 * 1024_i64.pow(5)), "3.0 PiB");
        assert_eq!(format_bytes(2048 * 1024_i64.pow(5)), "2048.0 PiB");
    }

    #[test]
    fn test_format_rows_number() {
        assert_eq!(format_rows_number(0), "0.0");
        assert_eq!(format_rows_number(2), "2.0");
        assert_eq!(format_rows_number(1500), "1.5k");
        assert_eq!(format_rows_number(2_500_000), "2.5m");
        assert_eq!(format_rows_number(4_000_000_000_000), "4.0t");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja_contrib::testing::jinja_assert;
    use std::collections::BTreeMap;

    fn metadata_to_yaml(resp: &AdapterResponse) -> BTreeMap<String, dbt_yaml::Value> {
        let mut map = BTreeMap::new();
        map.insert(
            "_message".to_string(),
            dbt_yaml::Value::string(resp.message()),
        );
        map.insert("code".to_string(), dbt_yaml::Value::string(resp.code()));
        map.insert(
            "rows_affected".to_string(),
            dbt_yaml::to_value(resp.rows_affected_i64()).expect("i64 serialises to YAML"),
        );
        if let Some(qid) = resp.query_id() {
            map.insert("query_id".to_string(), dbt_yaml::Value::string(qid));
        }
        map
    }

    #[test]
    fn test_to_adapter_response_map_matches_core_format() {
        let resp = AdapterResponse::new()
            .with_message("SUCCESS 42")
            .with_code("SUCCESS")
            .with_rows_affected(42)
            .with_query_id("01c2f954-abc");
        let map = metadata_to_yaml(&resp);

        // Core uses `_message`, not `message`
        assert_eq!(
            map.get("_message").and_then(|v| v.as_str()),
            Some("SUCCESS 42")
        );
        assert_eq!(map.get("code").and_then(|v| v.as_str()), Some("SUCCESS"));
        assert_eq!(map.get("rows_affected").and_then(|v| v.as_i64()), Some(42));
        assert_eq!(
            map.get("query_id").and_then(|v| v.as_str()),
            Some("01c2f954-abc")
        );
        // `message` key should NOT be present (Core uses `_message`)
        assert!(!map.contains_key("message"));
    }

    #[test]
    fn test_to_adapter_response_map_omits_null_query_id() {
        let resp = AdapterResponse::new()
            .with_message("SUCCESS 0")
            .with_code("SUCCESS")
            .with_rows_affected(0);
        let map = metadata_to_yaml(&resp);
        assert!(!map.contains_key("query_id"));
        assert_eq!(map.len(), 3); // _message, code, rows_affected
    }

    #[test]
    fn test_json_roundtrip_preserves_extra_keys() {
        let resp = AdapterResponse::new()
            .with_message("SUCCESS 42")
            .with_code("SUCCESS")
            .with_rows_affected(42)
            .with("foo", "bar");

        let json = serde_json::to_string(&resp).expect("serializes to JSON");
        let roundtripped: AdapterResponse =
            serde_json::from_str(&json).expect("deserializes from JSON");

        assert_eq!(roundtripped, resp);
        assert_eq!(
            roundtripped
                .0
                .get(&Value::from("foo"))
                .and_then(|v| v.as_str()),
            Some("bar")
        );
    }

    fn sample_response() -> AdapterResponse {
        AdapterResponse::new()
            .with_message("SUCCESS 42")
            .with_code("SUCCESS")
            .with_rows_affected(42)
            .with_query_id("01c2f954-abc")
            .with("foo", "bar")
    }

    #[test]
    fn test_jinja_renders_all_keys() {
        jinja_assert(
            sample_response(),
            "{{ obj.message }}|{{ obj.code }}|{{ obj.rows_affected }}|{{ obj.query_id }}|{{ obj.foo }}",
            "SUCCESS 42|SUCCESS|42|01c2f954-abc|bar",
        );
    }

    #[test]
    fn test_jinja_renders_as_dict() {
        jinja_assert(
            sample_response(),
            "{{ obj }}",
            "{'message': 'SUCCESS 42', 'code': 'SUCCESS', 'rows_affected': 42, 'query_id': '01c2f954-abc', '_message': 'SUCCESS 42', 'foo': 'bar'}",
        );
    }

    #[test]
    fn test_default_constructor() {
        jinja_assert(
            AdapterResponse::default(),
            "{{ obj.message }}|{{ obj.code }}|{{ obj.rows_affected }}|{{ obj.query_id }}|{{ obj.foo }}",
            "||0|None|None",
        );
    }

    #[test]
    fn test_jinja_tojson() {
        jinja_assert(
            sample_response(),
            "{{ obj | tojson }}",
            r#"{"_message": "SUCCESS 42", "code": "SUCCESS", "foo": "bar", "message": "SUCCESS 42", "query_id": "01c2f954-abc", "rows_affected": 42}"#,
        );
    }
}
