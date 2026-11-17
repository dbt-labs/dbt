//! Local Arrow runtime for adapters that execute Python outside the warehouse.
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

use adbc_core::options::{OptionDatabase, OptionValue};
use dbt_common::{AdapterError, AdapterErrorKind, AdapterResult};
use serde::{Deserialize, Serialize};

use crate::{AdapterEngine, AdapterResponse};

#[derive(Serialize, Deserialize)]
pub(crate) struct CsvLoad {
    pub schema: String,
    pub identifier: String,
    pub path: String,
    pub column_types: BTreeMap<String, String>,
    pub limit: Option<usize>,
    pub sample_every: usize,
    pub row_number_column: Option<String>,
    pub timestamp_format: String,
}

fn runtime_error(message: impl Into<String>) -> AdapterError {
    AdapterError::new(AdapterErrorKind::Driver, message)
}

fn execute(
    engine: &dyn AdapterEngine,
    operation: &str,
    parameters: serde_json::Value,
) -> AdapterResult<i64> {
    // Use the same validated options as the native connection, including URI
    // credential precedence; credentials travel through stdin, never argv.
    let builder = dbt_auth::auth_for_backend(engine.backend())
        .configure(engine.get_config())
        .map_err(crate::errors::auth_error_to_adapter_error)?;
    let mut options = BTreeMap::new();
    for (key, value) in builder {
        let name = match key {
            OptionDatabase::Uri => "uri".to_owned(),
            OptionDatabase::Username => "username".to_owned(),
            OptionDatabase::Password => "password".to_owned(),
            OptionDatabase::Other(key) => key,
            _ => return Err(runtime_error("Unsupported local Arrow connection option")),
        };
        if let OptionValue::String(value) = value {
            options.insert(name, value);
        }
    }
    let driver = engine
        .get_config()
        .get_str("driver")
        .ok_or_else(|| runtime_error("Missing Redis driver path"))?;
    let flight = engine.get_config().get_str("transport") == Some("flightsql");
    let init_sql = if flight {
        crate::engine::redis::session_sql(engine.get_config())
    } else {
        Vec::new()
    };
    let entrypoint = if flight {
        "AdbcDriverFlightSQLInit"
    } else {
        "AdbcDriverInit"
    };
    let request = serde_json::json!({"operation": operation, "driver": driver, "entrypoint": entrypoint, "init_sql": init_sql, "options": options, "parameters": parameters});
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(include_str!("runtime.py"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| {
            runtime_error(
                "Local Arrow runtime requires python3 with pyarrow and adbc-driver-manager on PATH",
            )
        })?;
    child
        .stdin
        .take()
        .ok_or_else(|| runtime_error("Missing Python runtime stdin"))?
        .write_all(
            serde_json::to_string(&request)
                .map_err(|e| runtime_error(e.to_string()))?
                .as_bytes(),
        )
        .map_err(|e| runtime_error(e.to_string()))?;
    let output = child
        .wait_with_output()
        .map_err(|e| runtime_error(e.to_string()))?;
    if !output.status.success() {
        let mut message = String::from_utf8_lossy(&output.stderr).into_owned();
        for (key, value) in &options {
            if (key == "uri"
                || key == "username"
                || key.contains("password")
                || key.contains("secret"))
                && !value.is_empty()
            {
                message = message.replace(value, "[redacted]");
            }
        }
        return Err(runtime_error(format!(
            "Local Arrow runtime failed: {message}"
        )));
    }
    let rows: i64 = serde_json::from_slice(&output.stdout)
        .map_err(|_| runtime_error("Local Arrow runtime returned invalid row count"))?;
    Ok(rows)
}

pub(crate) fn submit_python_job(
    engine: &dyn AdapterEngine,
    compiled_code: &str,
) -> AdapterResult<AdapterResponse> {
    let rows = execute(engine, "model", serde_json::json!({"code": compiled_code}))?;
    Ok(AdapterResponse::new()
        .with_message("OK")
        .with_rows_affected(rows))
}

pub(crate) fn load_csv_file(engine: &dyn AdapterEngine, mut load: CsvLoad) -> AdapterResult<i64> {
    if load.sample_every == 0
        || load.column_types.is_empty()
        || load
            .row_number_column
            .as_ref()
            .is_some_and(|name| load.column_types.contains_key(name))
    {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "CSV sample_every must be positive, column_types must not be empty, and row_number_column must be distinct",
        ));
    }
    if load.limit == Some(0) {
        load.limit = None;
    }
    use dbt_adapter_sql::ident::quote_identifier;
    let quote = |name: &str| quote_identifier(name, engine.adapter_type());
    let casts = load
        .column_types
        .iter()
        .map(|(name, dtype)| format!("cast(null as {dtype}) as {}", quote(name)))
        .collect::<Vec<_>>()
        .join(", ");
    let probe_sql = format!("select {casts} where false");
    let create_schema_sql = format!("create schema if not exists {}", quote(&load.schema));
    let drop_table_sql = format!(
        "drop table if exists {}.{} cascade",
        quote(&load.schema),
        quote(&load.identifier)
    );
    let mut parameters = serde_json::to_value(load).map_err(|e| runtime_error(e.to_string()))?;
    parameters["probe_sql"] = probe_sql.into();
    parameters["create_schema_sql"] = create_schema_sql.into();
    parameters["drop_table_sql"] = drop_table_sql.into();
    execute(engine, "csv", parameters)
}
