use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use arrow::array::{Array, Int64Array, StringArray};
use arrow::compute::cast;
use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;
use dbt_adapter_core::AdapterType;
use dbt_adapter_sql::ident::{escape_string_literal, sanitize_identifier};
use dbt_adbc::{Connection, QueryCtx};
use dbt_common::cancellation::CancellationToken;
use dbt_common::{AdapterError, AdapterErrorKind, AdapterResult};
use dbt_schemas::schemas::profiles::{DuckDBLocation, DuckDBPathInfo, DuckDbFlightsConfig};
use dbt_yaml::Value as YmlValue;
use indexmap::IndexMap;
use minijinja::{State, Value};
use regex::Regex;

use crate::adapter::adapter_impl::AdapterImpl;
use crate::macro_exec::execute_macro;
use crate::response::AdapterResponse;

const MAX_SOURCE_BYTES: usize = 200 * 1024;
const MAX_REQUIREMENTS_BYTES: usize = 20 * 1024;
const MAX_FLIGHT_NAME_LENGTH: usize = 120;
const DEFAULT_LOG_URL_TEMPLATE: &str =
    "https://app.motherduck.com/flights/{flight_id}/runs/{run_number}";
const DEFAULT_ACCESS_TOKEN_NAME: &str = "MotherDuck Flights";
const TERMINAL_STATUSES: &[&str] = &["SUCCEEDED", "FAILED", "CANCELLED"];

static DISTRIBUTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([A-Za-z0-9][A-Za-z0-9._-]*)\s*(?:\[|[=<>!~;@]|$)")
        .expect("distribution regex is valid")
});
static NORMALIZE_DISTRIBUTION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[-_.]+").expect("normalization regex is valid"));

const FLIGHT_ENTRYPOINT: &str = r#"

# --- dbt-duckdb flight entrypoint (generated) ---
__dbt_settings = __DBT_SETTINGS__


def __dbt_apply_settings(cursor):
    for statement in __dbt_settings:
        cursor.execute(statement)


def main():
    import duckdb as _duckdb

    # A bare MotherDuck connection resolves cloud databases by their real catalog names.
    con = _duckdb.connect("md:")
    __dbt_apply_settings(con)

    def load_df_function(table_name):
        return con.query(f"select * from {table_name}")

    dbt = dbtObj(load_df_function)
    df = model(dbt, con)
    if isinstance(df, _duckdb.DuckDBPyRelation):
        materialize(df, con)
    else:
        write_cursor = con.cursor()
        __dbt_apply_settings(write_cursor)
        materialize(df, write_cursor)


if __name__ == "__main__":
    main()
"#;

pub fn submit_python_job(
    adapter: &AdapterImpl,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    state: &State,
    model: &Value,
    compiled_code: &str,
    token: CancellationToken,
) -> AdapterResult<AdapterResponse> {
    let flights = flight_config(adapter)?;
    match submission_method(model, flights.as_ref())?.as_str() {
        "flight" => {}
        "local" => {
            return Err(AdapterError::new(
                AdapterErrorKind::NotSupported,
                "Local DuckDB Python model execution is not supported in dbt Core v2. \
                 Set `submission_method: flight` and connect to MotherDuck.",
            ));
        }
        method => {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Unsupported submission_method '{method}' for DuckDB; expected one of local, flight"
                ),
            ));
        }
    }

    let config = flights.unwrap_or_default();
    validate_config(&config)?;
    validate_flight_target(adapter, model)?;

    let macro_value = execute_macro(state, std::slice::from_ref(model), "flight_name")?;
    let name = macro_value.as_str().ok_or_else(|| {
        AdapterError::new(
            AdapterErrorKind::Configuration,
            "The flight_name macro must return a string",
        )
    })?;
    let name = sanitize_flight_name(name)?;

    let source = build_source(compiled_code, profile_settings(adapter))?;
    let mut runner = FlightRunner {
        adapter,
        ctx,
        conn,
        state,
        token,
        config,
    };
    let duckdb_version = match runner.config.duckdb_version.clone() {
        Some(version) => version,
        None => runner.duckdb_version()?,
    };
    let requirements = build_requirements(model, &runner.config, &duckdb_version)?;

    runner.submit(&name, &source, &requirements)
}

fn flight_config(adapter: &AdapterImpl) -> AdapterResult<Option<DuckDbFlightsConfig>> {
    adapter
        .get_db_config_value("flights")
        .cloned()
        .map(dbt_yaml::from_value)
        .transpose()
        .map_err(|err| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Invalid DuckDB `flights` configuration: {err}"),
            )
        })
}

fn submission_method(
    model: &Value,
    flights: Option<&DuckDbFlightsConfig>,
) -> AdapterResult<String> {
    let method = model
        .get_attr("config")
        .ok()
        .and_then(|config| config.get_attr("submission_method").ok())
        .filter(|value| !value.is_none() && !value.is_undefined())
        .and_then(|value| value.as_str().map(str::to_owned));

    Ok(method
        .unwrap_or_else(|| {
            if flights.is_some_and(|config| config.enabled_by_default) {
                "flight".to_string()
            } else {
                "local".to_string()
            }
        })
        .to_lowercase())
}

fn validate_config(config: &DuckDbFlightsConfig) -> AdapterResult<()> {
    if config.timeout_sec <= 0 {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "`flights.timeout_sec` must be greater than zero",
        ));
    }
    if !config.poll_interval_sec.is_finite() || config.poll_interval_sec <= 0.0 {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "`flights.poll_interval_sec` must be a finite number greater than zero",
        ));
    }
    poll_interval(config.poll_interval_sec)?;
    if Instant::now()
        .checked_add(Duration::from_secs(config.timeout_sec as u64))
        .is_none()
    {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "`flights.timeout_sec` is too large",
        ));
    }
    if config.log_lines < 0 {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "`flights.log_lines` cannot be negative",
        ));
    }
    if config.max_runtime_sec.is_some_and(|seconds| seconds < 0) {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "`flights.max_runtime_sec` cannot be negative",
        ));
    }
    Ok(())
}

fn poll_interval(seconds: f64) -> AdapterResult<Duration> {
    Duration::try_from_secs_f64(seconds).map_err(|_| {
        AdapterError::new(
            AdapterErrorKind::Configuration,
            "`flights.poll_interval_sec` is too large",
        )
    })
}

fn cached_flight_id(
    cache: &Mutex<Option<HashMap<String, String>>>,
    name: &str,
    load: impl FnOnce() -> AdapterResult<HashMap<String, String>>,
) -> AdapterResult<Option<String>> {
    let mut ids = cache.lock().map_err(|_| {
        AdapterError::new(
            AdapterErrorKind::Internal,
            "Python job ID cache lock is poisoned",
        )
    })?;
    if ids.is_none() {
        *ids = Some(load()?);
    }
    Ok(ids.as_ref().and_then(|ids| ids.get(name).cloned()))
}

fn validate_flight_target(adapter: &AdapterImpl, model: &Value) -> AdapterResult<()> {
    let path = adapter.get_db_config("path");
    let path_info = DuckDBPathInfo::parse_path(path.as_deref());
    let mut databases = Vec::new();
    if matches!(path_info.location, DuckDBLocation::Motherduck { .. }) {
        let configured_name = adapter
            .get_db_config("database")
            .map(|value| value.into_owned())
            .unwrap_or_else(|| path_info.database.to_string());
        if configured_name == path_info.database {
            databases.push(path_info.database.to_string());
        }
    }

    if let Some(YmlValue::Sequence(attachments, _)) = adapter.get_db_config_value("attach") {
        for attachment in attachments {
            let YmlValue::Mapping(mapping, _) = attachment else {
                continue;
            };
            let Some(path) = mapping.get("path").and_then(YmlValue::as_str) else {
                continue;
            };
            let info = DuckDBPathInfo::parse_path(Some(path));
            if !matches!(info.location, DuckDBLocation::Motherduck { .. }) {
                continue;
            }
            let alias = mapping.get("alias").and_then(YmlValue::as_str);
            if alias.is_none_or(|alias| alias == info.database) {
                databases.push(info.database.to_string());
            }
        }
    }

    let database = model
        .get_attr("database")
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    if databases.iter().any(|candidate| candidate == &database) {
        return Ok(());
    }

    let message = if databases.is_empty() {
        "Python models can only be submitted to MotherDuck Flights when the DuckDB profile \
         targets or attaches a MotherDuck database under its real name."
            .to_string()
    } else {
        format!(
            "Python model targets database '{database}', which is not reachable from a \
             MotherDuck Flight; reachable databases are: {}",
            databases.join(", ")
        )
    };
    Err(AdapterError::new(AdapterErrorKind::Configuration, message))
}

fn profile_settings(adapter: &AdapterImpl) -> Option<&dbt_yaml::Mapping> {
    match adapter.get_db_config_value("settings") {
        Some(YmlValue::Mapping(settings, _)) => Some(settings),
        _ => None,
    }
}

fn settings_statements(settings: Option<&dbt_yaml::Mapping>) -> AdapterResult<Vec<String>> {
    let mut statements = Vec::new();
    for (key, value) in settings.into_iter().flatten() {
        let Some(key) = key.as_str() else {
            continue;
        };
        if key.eq_ignore_ascii_case("motherduck_token") {
            continue;
        }
        if !is_safe_flight_setting(key) {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "DuckDB setting '{key}' is not safe to embed in MotherDuck Flight source \
                     code. Configure credential access in MotherDuck or use a scoped Flight \
                     access token instead."
                ),
            ));
        }
        let key = sanitize_identifier(key, AdapterType::DuckDB);
        if !key.is_empty() {
            statements.push(format!("SET {key} = {}", yaml_sql_literal(value)));
        }
    }
    Ok(statements)
}

fn is_safe_flight_setting(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['.', '-'], "_");
    const SAFE_SETTINGS: &[&str] = &[
        "calendar",
        "default_collation",
        "default_null_order",
        "default_order",
        "enable_progress_bar",
        "errors_as_json",
        "ieee_floating_point_ops",
        "integer_division",
        "max_memory",
        "memory_limit",
        "preserve_identifier_case",
        "preserve_insertion_order",
        "progress_bar_time",
        "scalar_subquery_error_on_multiple_rows",
        "threads",
        "timezone",
        "worker_threads",
    ];
    SAFE_SETTINGS.contains(&key.as_str())
}

fn yaml_sql_literal(value: &YmlValue) -> String {
    match value {
        YmlValue::String(value, _) => {
            format!("'{}'", escape_string_literal(value, AdapterType::DuckDB))
        }
        YmlValue::Number(value, _) => value.to_string(),
        YmlValue::Bool(value, _) => value.to_string(),
        YmlValue::Null(_) => "NULL".to_string(),
        _ => {
            let value = dbt_yaml::to_string(value).unwrap_or_default();
            format!(
                "'{}'",
                escape_string_literal(value.trim_end(), AdapterType::DuckDB)
            )
        }
    }
}

fn build_source(
    compiled_code: &str,
    settings: Option<&dbt_yaml::Mapping>,
) -> AdapterResult<String> {
    let settings = serde_json::to_string(&settings_statements(settings)?).map_err(|err| {
        AdapterError::new(
            AdapterErrorKind::Internal,
            format!("Could not serialize DuckDB settings for a Flight: {err}"),
        )
    })?;
    let entrypoint = FLIGHT_ENTRYPOINT.replace("__DBT_SETTINGS__", &settings);
    let source = format!("{}{}", compiled_code.trim_start(), entrypoint);
    let size = source.len();
    if size > MAX_SOURCE_BYTES {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "Python model is too large to run as a MotherDuck Flight: {size} bytes exceeds \
                 the {MAX_SOURCE_BYTES} byte limit"
            ),
        ));
    }
    Ok(source)
}

fn build_requirements(
    model: &Value,
    config: &DuckDbFlightsConfig,
    duckdb_version: &str,
) -> AdapterResult<String> {
    let mut packages = vec![format!(
        "duckdb=={}",
        duckdb_version.trim().trim_start_matches('v')
    )];
    packages.extend(config.requirements.iter().flatten().cloned());
    packages.extend(model_packages(model));

    let mut resolved = IndexMap::<String, String>::new();
    let mut passthrough = Vec::new();
    for package in packages {
        let package = package.trim();
        if package.is_empty() {
            continue;
        }
        if let Some(name) = distribution_name(package) {
            resolved.insert(name, package.to_string());
        } else {
            passthrough.push(package.to_string());
        }
    }

    passthrough.extend(resolved.into_values());
    let requirements = format!("{}\n", passthrough.join("\n"));
    let size = requirements.len();
    if size > MAX_REQUIREMENTS_BYTES {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "Python model requirements are too large for a MotherDuck Flight: {size} bytes \
                 exceeds the {MAX_REQUIREMENTS_BYTES} byte limit"
            ),
        ));
    }
    Ok(requirements)
}

fn model_packages(model: &Value) -> Vec<String> {
    model
        .get_attr("config")
        .ok()
        .and_then(|config| config.get_attr("packages").ok())
        .and_then(|packages| packages.try_iter().ok())
        .map(|packages| {
            packages
                .filter_map(|package| package.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn distribution_name(requirement: &str) -> Option<String> {
    let name = DISTRIBUTION_RE.captures(requirement)?.get(1)?.as_str();
    Some(
        NORMALIZE_DISTRIBUTION_RE
            .replace_all(name, "-")
            .to_lowercase(),
    )
}

fn sanitize_flight_name(name: &str) -> AdapterResult<String> {
    let name: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let name = name.trim_matches(['_', '-']);
    if name.is_empty() {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "The flight_name macro returned an empty Flight name",
        ));
    }
    if name.len() <= MAX_FLIGHT_NAME_LENGTH {
        return Ok(name.to_string());
    }
    let digest = format!("{:x}", md5::compute(name.as_bytes()));
    let prefix_length = MAX_FLIGHT_NAME_LENGTH - 1 - 8;
    Ok(format!("{}-{}", &name[..prefix_length], &digest[..8]))
}

struct FlightRunner<'a> {
    adapter: &'a AdapterImpl,
    ctx: &'a QueryCtx,
    conn: &'a mut dyn Connection,
    state: &'a State<'a, 'a>,
    token: CancellationToken,
    config: DuckDbFlightsConfig,
}

impl FlightRunner<'_> {
    fn submit(
        &mut self,
        name: &str,
        source: &str,
        requirements: &str,
    ) -> AdapterResult<AdapterResponse> {
        let flight_id = self.upsert_flight(name, source, requirements)?;
        let run_number = self.start_run(&flight_id)?;
        tracing::debug!(name, flight_id, run_number, "MotherDuck Flight started");

        let (status, exit_code) = self.await_run(&flight_id, run_number, name)?;
        if status != "SUCCEEDED" {
            return Err(AdapterError::new(
                AdapterErrorKind::Driver,
                format!(
                    "Python model failed on MotherDuck Flight '{name}' (run {run_number}, \
                     status {status}, exit code {}).\n{}",
                    exit_code
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    self.log_pointer(&flight_id, run_number)
                ),
            ));
        }

        Ok(AdapterResponse::new()
            .with_message("OK")
            .with_code("OK")
            .with_rows_affected(0)
            .with_query_id(format!("{flight_id}:{run_number}")))
    }

    fn duckdb_version(&mut self) -> AdapterResult<String> {
        let batch = self.query(
            "SELECT regexp_replace(library_version, '^v', '') AS duckdb_version \
             FROM pragma_version()",
        )?;
        required_string(&batch, "duckdb_version", 0)
    }

    fn upsert_flight(
        &mut self,
        name: &str,
        source: &str,
        requirements: &str,
    ) -> AdapterResult<String> {
        let Some(flight_id) = self.find_flight(name)? else {
            return self.create_flight(name, source, requirements);
        };
        if self.is_current(&flight_id, source, requirements)? {
            tracing::debug!(name, flight_id, "MotherDuck Flight is up to date");
            return Ok(flight_id);
        }
        self.update_flight(&flight_id, source, requirements)?;
        Ok(flight_id)
    }

    fn update_flight(
        &mut self,
        flight_id: &str,
        source: &str,
        requirements: &str,
    ) -> AdapterResult<()> {
        let sql = format!(
            "SELECT flight_id FROM MD_UPDATE_FLIGHT(flight_id := '{}', source_code := '{}', \
             requirements_txt := '{}'{});",
            sql_string(flight_id),
            sql_string(source),
            sql_string(requirements),
            self.optional_args()
        );
        self.query(&sql)?;
        Ok(())
    }

    fn create_flight(
        &mut self,
        name: &str,
        source: &str,
        requirements: &str,
    ) -> AdapterResult<String> {
        let sql = format!(
            "SELECT flight_id FROM MD_CREATE_FLIGHT(name := '{}', source_code := '{}', \
             requirements_txt := '{}'{});",
            sql_string(name),
            sql_string(source),
            sql_string(requirements),
            self.optional_args()
        );
        match self.query(&sql) {
            Ok(batch) => {
                let flight_id = required_string(&batch, "flight_id", 0)?;
                self.remember_flight(name, &flight_id)?;
                Ok(flight_id)
            }
            Err(err) if err.to_string().contains("already exists") => {
                let Some(flight_id) = self.refresh_flights()?.get(name).cloned() else {
                    return Err(AdapterError::new(
                        AdapterErrorKind::Configuration,
                        format!(
                            "A MotherDuck Flight named '{name}' already exists but is not owned \
                             by the current user. Override the `duckdb__flight_name` macro."
                        ),
                    ));
                };
                if !self.is_current(&flight_id, source, requirements)? {
                    self.update_flight(&flight_id, source, requirements)?;
                }
                Ok(flight_id)
            }
            Err(err) => Err(err),
        }
    }

    fn optional_args(&self) -> String {
        let mut args = format!(
            ", access_token_name := '{}'",
            sql_string(
                self.config
                    .access_token_name
                    .as_deref()
                    .unwrap_or(DEFAULT_ACCESS_TOKEN_NAME)
            )
        );
        if let Some(max_runtime_sec) = self.config.max_runtime_sec {
            args.push_str(&format!(", max_runtime_sec := {max_runtime_sec}"));
        }
        args
    }

    fn find_flight(&mut self, name: &str) -> AdapterResult<Option<String>> {
        let cache = self.adapter.python_job_id_cache();
        cached_flight_id(&cache, name, || self.list_flights())
    }

    fn refresh_flights(&mut self) -> AdapterResult<HashMap<String, String>> {
        let cache = self.adapter.python_job_id_cache();
        let mut cached = cache.lock().map_err(|_| {
            AdapterError::new(
                AdapterErrorKind::Internal,
                "Python job ID cache lock is poisoned",
            )
        })?;
        let ids = self.list_flights()?;
        *cached = Some(ids.clone());
        Ok(ids)
    }

    fn remember_flight(&self, name: &str, flight_id: &str) -> AdapterResult<()> {
        let cache = self.adapter.python_job_id_cache();
        let mut ids = cache.lock().map_err(|_| {
            AdapterError::new(
                AdapterErrorKind::Internal,
                "Python job ID cache lock is poisoned",
            )
        })?;
        ids.get_or_insert_with(HashMap::new)
            .insert(name.to_string(), flight_id.to_string());
        Ok(())
    }

    fn list_flights(&mut self) -> AdapterResult<HashMap<String, String>> {
        let mut flights = HashMap::new();
        let mut offset = 0;
        const PAGE_SIZE: i64 = 200;
        loop {
            let sql = format!(
                "SELECT flight_id, flight_name FROM MD_LIST_FLIGHTS(\
                 \"limit\" := {PAGE_SIZE}, \"offset\" := {offset}, owner_only := true);"
            );
            let batch = self.query(&sql)?;
            let ids = string_column(&batch, "flight_id")?;
            let names = string_column(&batch, "flight_name")?;
            for row in 0..batch.num_rows() {
                if !names.is_null(row) && !ids.is_null(row) {
                    flights.insert(names.value(row).to_string(), ids.value(row).to_string());
                }
            }
            if batch.num_rows() < PAGE_SIZE as usize {
                return Ok(flights);
            }
            offset += PAGE_SIZE;
        }
    }

    fn is_current(
        &mut self,
        flight_id: &str,
        source: &str,
        requirements: &str,
    ) -> AdapterResult<bool> {
        let sql = format!(
            "SELECT current_version FROM MD_GET_FLIGHT(flight_id := '{}');",
            sql_string(flight_id)
        );
        let batch = self.query(&sql)?;
        let Some(version) = optional_i64(&batch, "current_version", 0)? else {
            return Ok(false);
        };
        let sql = format!(
            "SELECT source_code, requirements_txt, access_token_name, max_runtime_sec \
             FROM MD_GET_FLIGHT_VERSION(\
             flight_id := '{}', version_number := {version});",
            sql_string(flight_id)
        );
        let batch = self.query(&sql)?;
        flight_version_matches(&batch, source, requirements, &self.config)
    }

    fn start_run(&mut self, flight_id: &str) -> AdapterResult<i64> {
        let sql = format!(
            "SELECT run_number FROM MD_RUN_FLIGHT(flight_id := '{}');",
            sql_string(flight_id)
        );
        let batch = self.query(&sql)?;
        optional_i64(&batch, "run_number", 0)?.ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                "MotherDuck did not return a Flight run number",
            )
        })
    }

    fn await_run(
        &mut self,
        flight_id: &str,
        run_number: i64,
        name: &str,
    ) -> AdapterResult<(String, Option<i64>)> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(self.config.timeout_sec as u64))
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "`flights.timeout_sec` is too large",
                )
            })?;
        loop {
            if self.token.is_cancelled() {
                self.cancel_run(flight_id, run_number);
                self.token.check_cancellation().map_err(|_| {
                    AdapterError::new(AdapterErrorKind::Cancelled, "operation cancelled")
                })?;
            }
            if Instant::now() >= deadline {
                return Err(self.timeout_error(flight_id, run_number, name, "unknown"));
            }
            let sql = format!(
                "SELECT status, exit_code FROM MD_GET_FLIGHT_RUN(\
                 flight_id := '{}', run_number := {run_number});",
                sql_string(flight_id)
            );
            let batch = match self.query(&sql) {
                Ok(batch) => batch,
                Err(err) => {
                    self.cancel_run(flight_id, run_number);
                    return Err(err);
                }
            };
            let status = match required_string(&batch, "status", 0) {
                Ok(status) => status,
                Err(err) => {
                    self.cancel_run(flight_id, run_number);
                    return Err(err);
                }
            };
            if TERMINAL_STATUSES.contains(&status.as_str()) {
                return Ok((status, optional_i64(&batch, "exit_code", 0)?));
            }
            if Instant::now() >= deadline {
                return Err(self.timeout_error(flight_id, run_number, name, &status));
            }
            self.wait_for_next_poll(flight_id, run_number, deadline)?;
        }
    }

    fn wait_for_next_poll(
        &mut self,
        flight_id: &str,
        run_number: i64,
        run_deadline: Instant,
    ) -> AdapterResult<()> {
        let poll_deadline = Instant::now()
            .checked_add(poll_interval(self.config.poll_interval_sec)?)
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "`flights.poll_interval_sec` is too large",
                )
            })?;
        while Instant::now() < poll_deadline && Instant::now() < run_deadline {
            if self.token.is_cancelled() {
                self.cancel_run(flight_id, run_number);
                return Err(AdapterError::new(
                    AdapterErrorKind::Cancelled,
                    "operation cancelled",
                ));
            }
            let remaining = poll_deadline
                .saturating_duration_since(Instant::now())
                .min(run_deadline.saturating_duration_since(Instant::now()))
                .min(Duration::from_millis(100));
            thread::sleep(remaining);
        }
        Ok(())
    }

    fn timeout_error(
        &mut self,
        flight_id: &str,
        run_number: i64,
        name: &str,
        status: &str,
    ) -> AdapterError {
        let cancelled = self.cancel_run(flight_id, run_number);
        let logs = self.log_pointer(flight_id, run_number);
        AdapterError::new(
            AdapterErrorKind::Driver,
            format!(
                "Timed out after {}s waiting for MotherDuck Flight '{name}' run {run_number} \
                 (last status: {status}). {}\n{logs}",
                self.config.timeout_sec,
                if cancelled {
                    "The run was cancelled."
                } else {
                    "The run could not be cancelled and may still be running."
                }
            ),
        )
    }

    fn cancel_run(&mut self, flight_id: &str, run_number: i64) -> bool {
        let sql = format!(
            "SELECT * FROM MD_CANCEL_FLIGHT_RUN(flight_id := '{}', run_number := {run_number});",
            sql_string(flight_id)
        );
        match self.query_with_token(&sql, CancellationToken::never_cancels()) {
            Ok(_) => true,
            Err(err) => {
                tracing::debug!(%err, flight_id, run_number, "Could not cancel Flight run");
                false
            }
        }
    }

    fn log_pointer(&mut self, flight_id: &str, run_number: i64) -> String {
        let template = self
            .config
            .log_url_template
            .as_deref()
            .unwrap_or(DEFAULT_LOG_URL_TEMPLATE);
        let url = template
            .replace("{flight_id}", flight_id)
            .replace("{run_number}", &run_number.to_string());
        if self.config.log_lines == 0 {
            return format!("Logs: {url}");
        }
        let sql = format!(
            "SELECT line FROM MD_GET_FLIGHT_LOGS(flight_id := '{}', run_number := {run_number}, \
             \"limit\" := {}, \"order\" := 'desc') ORDER BY line_number;",
            sql_string(flight_id),
            self.config.log_lines
        );
        match self.query(&sql).and_then(|batch| {
            let lines = string_column(&batch, "line")?;
            Ok((0..batch.num_rows())
                .filter(|row| !lines.is_null(*row))
                .map(|row| lines.value(row))
                .collect::<Vec<_>>()
                .join("\n"))
        }) {
            Ok(logs) if !logs.is_empty() => format!("Logs: {url}\n{logs}"),
            Ok(_) => format!("Logs: {url}"),
            Err(err) => format!("Logs: {url}\n(could not read Flight logs: {err})"),
        }
    }

    fn query(&mut self, sql: &str) -> AdapterResult<RecordBatch> {
        self.token
            .check_cancellation()
            .map_err(|_| AdapterError::new(AdapterErrorKind::Cancelled, "operation cancelled"))?;
        let token = self.token.clone();
        self.query_with_token(sql, token)
    }

    fn query_with_token(
        &mut self,
        sql: &str,
        token: CancellationToken,
    ) -> AdapterResult<RecordBatch> {
        let result = self.adapter.execute(
            Some(self.state),
            self.conn,
            Some(self.ctx),
            sql,
            false,
            true,
            None,
            None,
            token.clone(),
        );
        match result {
            Ok((_, table)) => Ok(table.original_record_batch().as_ref().clone()),
            Err(_) if token.is_cancelled() => Err(AdapterError::new(
                AdapterErrorKind::Cancelled,
                "operation cancelled",
            )),
            Err(err) => Err(AdapterError::new(
                AdapterErrorKind::Driver,
                redact(&err.to_string(), self.config.access_token_name.as_deref()),
            )),
        }
    }
}

fn sql_string(value: &str) -> String {
    escape_string_literal(value, AdapterType::DuckDB)
}

fn redact(message: &str, token_name: Option<&str>) -> String {
    token_name
        .filter(|token_name| !token_name.is_empty())
        .map(|token_name| message.replace(token_name, "***"))
        .unwrap_or_else(|| message.to_string())
}

fn string_column(batch: &RecordBatch, name: &str) -> AdapterResult<StringArray> {
    let column = batch.column_by_name(name).ok_or_else(|| {
        AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            format!("MotherDuck response did not include column '{name}'"),
        )
    })?;
    let column = cast(column, &DataType::Utf8).map_err(|err| {
        AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            format!("MotherDuck column '{name}' was not a string: {err}"),
        )
    })?;
    column
        .as_any()
        .downcast_ref::<StringArray>()
        .cloned()
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                format!("MotherDuck column '{name}' was not a string"),
            )
        })
}

fn required_string(batch: &RecordBatch, name: &str, row: usize) -> AdapterResult<String> {
    optional_string(batch, name, row)?.ok_or_else(|| {
        AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            format!("MotherDuck response did not include a value for '{name}'"),
        )
    })
}

fn optional_string(batch: &RecordBatch, name: &str, row: usize) -> AdapterResult<Option<String>> {
    let column = string_column(batch, name)?;
    if row >= column.len() || column.is_null(row) {
        Ok(None)
    } else {
        Ok(Some(column.value(row).to_string()))
    }
}

fn optional_i64(batch: &RecordBatch, name: &str, row: usize) -> AdapterResult<Option<i64>> {
    let column = batch.column_by_name(name).ok_or_else(|| {
        AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            format!("MotherDuck response did not include column '{name}'"),
        )
    })?;
    let column = cast(column, &DataType::Int64).map_err(|err| {
        AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            format!("MotherDuck column '{name}' was not an integer: {err}"),
        )
    })?;
    let column = column
        .as_any()
        .downcast_ref::<Int64Array>()
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                format!("MotherDuck column '{name}' was not an integer"),
            )
        })?;
    if row >= column.len() || column.is_null(row) {
        Ok(None)
    } else {
        Ok(Some(column.value(row)))
    }
}

fn flight_version_matches(
    batch: &RecordBatch,
    source: &str,
    requirements: &str,
    config: &DuckDbFlightsConfig,
) -> AdapterResult<bool> {
    if batch.num_rows() == 0 {
        return Ok(false);
    }
    let token_name = optional_string(batch, "access_token_name", 0)?;
    let token_matches = match (config.access_token_name.as_deref(), token_name.as_deref()) {
        (Some(desired), Some(current)) => desired == current,
        (Some(_), None) => false,
        (None, Some(current)) => current == DEFAULT_ACCESS_TOKEN_NAME,
        (None, None) => true,
    };
    let max_runtime_matches = match config.max_runtime_sec {
        Some(desired) => optional_i64(batch, "max_runtime_sec", 0)? == Some(desired),
        // The API has no reset-to-plan-default value. An omitted profile value
        // leaves the current/default timeout unchanged; use 0 to request no timeout.
        None => true,
    };
    Ok(required_string(batch, "source_code", 0)? == source
        && required_string(batch, "requirements_txt", 0)? == requirements
        && token_matches
        && max_runtime_matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{ArrayRef, Int64Array, StringArray};
    use arrow::datatypes::{Field, Schema};
    use minijinja::context;
    use std::sync::Arc;

    #[test]
    fn sanitizes_and_bounds_flight_names() {
        assert_eq!(
            sanitize_flight_name("dbt project/schema:model").unwrap(),
            "dbt_project_schema_model"
        );
        let first = sanitize_flight_name(&format!("{}-one", "a".repeat(200))).unwrap();
        let second = sanitize_flight_name(&format!("{}-two", "a".repeat(200))).unwrap();
        assert_eq!(first.len(), MAX_FLIGHT_NAME_LENGTH);
        assert_ne!(first, second);
        assert!(sanitize_flight_name("---").is_err());
    }

    #[test]
    fn model_packages_override_profile_requirements_by_distribution() {
        let model = context! {
            config => context! {
                packages => vec!["pandas==2.2.3", "scikit_learn==1.5.0"],
            }
        };
        let config = DuckDbFlightsConfig {
            requirements: Some(vec![
                "pandas==1.0.0".to_string(),
                "--extra-index-url https://example.invalid".to_string(),
            ]),
            ..Default::default()
        };
        let requirements = build_requirements(&model, &config, "1.5.0").unwrap();
        assert_eq!(
            requirements,
            "--extra-index-url https://example.invalid\n\
             duckdb==1.5.0\n\
             pandas==2.2.3\n\
             scikit_learn==1.5.0\n"
        );
    }

    #[test]
    fn source_applies_settings_without_forwarding_token() {
        let settings: dbt_yaml::Mapping =
            dbt_yaml::from_str("TimeZone: UTC\nMOTHERDUCK_TOKEN: secret\nthreads: 4\n").unwrap();
        let source = build_source(
            "def model(dbt, session):\n    return None\n",
            Some(&settings),
        )
        .unwrap();
        assert!(source.contains(r#"__dbt_settings = ["SET TimeZone = 'UTC'","SET threads = 4"]"#));
        assert!(!source.contains("secret"));
        assert!(source.contains(r#"con = _duckdb.connect("md:")"#));
    }

    #[test]
    fn source_only_accepts_safe_settings() {
        let settings: dbt_yaml::Mapping =
            dbt_yaml::from_str("s3_secret_access_key: secret\n").unwrap();
        let err = build_source(
            "def model(dbt, session):\n    return None\n",
            Some(&settings),
        )
        .unwrap_err();
        assert!(err.to_string().contains("s3_secret_access_key"));
        assert!(!is_safe_flight_setting("azure_storage_connection_string"));
        assert!(!is_safe_flight_setting("http_proxy"));
        assert!(!is_safe_flight_setting("extension_defined_setting"));
        assert!(is_safe_flight_setting("TimeZone"));
        assert!(is_safe_flight_setting("memory-limit"));
    }

    #[test]
    fn flight_id_cache_loads_once() {
        let cache = Mutex::new(None);
        let first = cached_flight_id(&cache, "model", || {
            Ok(HashMap::from([(
                "model".to_string(),
                "flight-id".to_string(),
            )]))
        })
        .unwrap();
        let second = cached_flight_id(&cache, "model", || {
            panic!("an initialized cache must not reload")
        })
        .unwrap();

        assert_eq!(first.as_deref(), Some("flight-id"));
        assert_eq!(second, first);
    }

    #[test]
    fn validates_flight_config_ranges() {
        assert!(validate_config(&DuckDbFlightsConfig::default()).is_ok());
        assert!(
            validate_config(&DuckDbFlightsConfig {
                poll_interval_sec: 0.0,
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            validate_config(&DuckDbFlightsConfig {
                log_lines: -1,
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            validate_config(&DuckDbFlightsConfig {
                poll_interval_sec: f64::MAX,
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            validate_config(&DuckDbFlightsConfig {
                timeout_sec: i64::MAX,
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn redacts_access_token_name() {
        assert_eq!(
            redact("token analytics-token failed", Some("analytics-token")),
            "token *** failed"
        );
    }

    #[test]
    fn flight_version_reuse_includes_token_and_runtime_config() {
        let batch = RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("source_code", DataType::Utf8, false),
                Field::new("requirements_txt", DataType::Utf8, false),
                Field::new("access_token_name", DataType::Utf8, true),
                Field::new("max_runtime_sec", DataType::Int64, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["source"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["duckdb==1.5.0\n"])) as ArrayRef,
                Arc::new(StringArray::from(vec![Some("analytics-token")])) as ArrayRef,
                Arc::new(Int64Array::from(vec![Some(1800)])) as ArrayRef,
            ],
        )
        .unwrap();
        let matching = DuckDbFlightsConfig {
            access_token_name: Some("analytics-token".to_string()),
            max_runtime_sec: Some(1800),
            ..Default::default()
        };
        assert!(flight_version_matches(&batch, "source", "duckdb==1.5.0\n", &matching).unwrap());
        assert!(
            !flight_version_matches(
                &batch,
                "source",
                "duckdb==1.5.0\n",
                &DuckDbFlightsConfig {
                    access_token_name: Some("other-token".to_string()),
                    max_runtime_sec: Some(1800),
                    ..Default::default()
                },
            )
            .unwrap()
        );
        assert!(
            !flight_version_matches(
                &batch,
                "source",
                "duckdb==1.5.0\n",
                &DuckDbFlightsConfig {
                    access_token_name: Some("analytics-token".to_string()),
                    max_runtime_sec: Some(3600),
                    ..Default::default()
                },
            )
            .unwrap()
        );
        assert!(
            !flight_version_matches(
                &batch,
                "source",
                "duckdb==1.5.0\n",
                &DuckDbFlightsConfig::default(),
            )
            .unwrap()
        );
    }
}
