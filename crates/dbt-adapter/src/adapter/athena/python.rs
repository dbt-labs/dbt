//! Python models on Athena Spark: dbt-athena's `AthenaPythonJobHelper`
//! (`python_submissions.py`), `AthenaSparkSessionManager` (`session.py`) and
//! `AthenaSparkSessionConfig` (`config.py`).
//!
//! The compiled model runs as a calculation in a Spark session of the profile's
//! `spark_work_group`, through the driver's `athena.*_session` /
//! `athena.*_calculation_execution` operations. Sessions are shared by the models
//! of the process: a model takes a free session started with the same engine
//! configuration, or starts one; Athena ends idle sessions after
//! `SESSION_IDLE_TIMEOUT_MIN`.

use super::driver_ops::AthenaOps;
use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use crate::response::AdapterResponse;
use dbt_common::cancellation::CancellationToken;
use minijinja::Value;
use serde_json::{Map, Value as Json, json};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

// dbt-athena `constants.py`
const DEFAULT_CALCULATION_TIMEOUT_SECS: i64 = 43200;
const DEFAULT_SPARK_COORDINATOR_DPU_SIZE: i64 = 1;
const DEFAULT_SPARK_MAX_CONCURRENT_DPUS: i64 = 2;
const DEFAULT_SPARK_EXECUTOR_DPU_SIZE: i64 = 1;
const SESSION_IDLE_TIMEOUT_MIN: i64 = 10;
/// `AthenaCredentials.poll_interval`, which dbt-athena hands to the helper as the
/// polling interval when the model sets none.
const DEFAULT_POLLING_INTERVAL_SECS: f64 = 1.0;

const ENGINE_CONFIG_KEYS: &[&str] = &[
    "CoordinatorDpuSize",
    "MaxConcurrentDpus",
    "DefaultExecutorDpuSize",
    "SparkProperties",
    "AdditionalConfigs",
];

/// `DEFAULT_SPARK_PROPERTIES`
fn default_spark_properties(key: &str) -> &'static [(&'static str, &'static str)] {
    match key {
        "iceberg" => &[
            (
                "spark.sql.catalog.spark_catalog",
                "org.apache.iceberg.spark.SparkSessionCatalog",
            ),
            (
                "spark.sql.catalog.spark_catalog.catalog-impl",
                "org.apache.iceberg.aws.glue.GlueCatalog",
            ),
            (
                "spark.sql.catalog.spark_catalog.io-impl",
                "org.apache.iceberg.aws.s3.S3FileIO",
            ),
            (
                "spark.sql.extensions",
                "org.apache.iceberg.spark.extensions.IcebergSparkSessionExtensions",
            ),
        ],
        "hudi" => &[
            (
                "spark.sql.catalog.spark_catalog",
                "org.apache.spark.sql.hudi.catalog.HoodieCatalog",
            ),
            (
                "spark.serializer",
                "org.apache.spark.serializer.KryoSerializer",
            ),
            (
                "spark.sql.extensions",
                "org.apache.spark.sql.hudi.HoodieSparkSessionExtension",
            ),
        ],
        "delta_lake" => &[
            (
                "spark.sql.catalog.spark_catalog",
                "org.apache.spark.sql.delta.catalog.DeltaCatalog",
            ),
            (
                "spark.sql.extensions",
                "io.delta.sql.DeltaSparkSessionExtension",
            ),
        ],
        "spark_encryption" => &[
            ("spark.authenticate", "true"),
            ("spark.io.encryption.enabled", "true"),
            ("spark.network.crypto.enabled", "true"),
        ],
        "spark_cross_account_catalog" => &[("spark.hadoop.aws.glue.catalog.separator", "/")],
        "spark_requester_pays" => &[("spark.hadoop.fs.s3.useRequesterPaysHeader", "true")],
        _ => &[],
    }
}

fn config_error(msg: impl Into<String>) -> AdapterError {
    AdapterError::new(AdapterErrorKind::Configuration, msg)
}

fn runtime_error(msg: impl Into<String>) -> AdapterError {
    AdapterError::new(AdapterErrorKind::UnexpectedResult, msg)
}

/// The Spark settings of one Python model.
#[derive(Debug, Clone, PartialEq)]
pub struct SparkConfig {
    pub timeout: Duration,
    pub polling_interval: Duration,
    pub engine_config: Json,
}

fn config_attr(config: &Value, key: &str) -> Option<Value> {
    config
        .get_attr(key)
        .ok()
        .filter(|v| !v.is_undefined() && !v.is_none())
}

fn truthy(config: &Value, key: &str) -> bool {
    config_attr(config, key).is_some_and(|v| v.is_true())
}

impl SparkConfig {
    /// `AthenaSparkSessionConfig.set_timeout` / `set_polling_interval` /
    /// `set_engine_config`, from the model's `config`.
    pub fn from_model_config(config: &Value, poll_interval: Option<f64>) -> AdapterResult<Self> {
        let timeout = match config_attr(config, "timeout") {
            None => DEFAULT_CALCULATION_TIMEOUT_SECS,
            // An integer, not a float, as `isinstance(timeout, int)` requires.
            Some(v) => serde_json::to_value(&v)
                .ok()
                .and_then(|v| v.as_i64())
                .ok_or_else(|| config_error("Timeout must be an integer"))?,
        };
        if timeout <= 0 {
            return Err(config_error("Timeout must be a positive integer"));
        }

        let polling_interval = match config_attr(config, "polling_interval") {
            None => poll_interval.unwrap_or(DEFAULT_POLLING_INTERVAL_SECS),
            Some(v) => serde_json::to_value(&v)
                .ok()
                .and_then(|v| v.as_f64())
                .ok_or_else(|| {
                    config_error(format!(
                        "Polling_interval must be a positive number. Got: {v}"
                    ))
                })?,
        };
        if !(polling_interval.is_finite() && polling_interval > 0.0) {
            return Err(config_error(format!(
                "Polling_interval must be a positive number. Got: {polling_interval}"
            )));
        }

        Ok(Self {
            timeout: Duration::from_secs(timeout as u64),
            polling_interval: Duration::from_secs_f64(polling_interval),
            engine_config: engine_config(config)?,
        })
    }
}

fn engine_config(config: &Value) -> AdapterResult<Json> {
    let table_type = config_attr(config, "table_type")
        .map(|v| v.to_string().to_lowercase())
        .unwrap_or_else(|| "hive".to_string());
    let mut spark_properties = Map::new();
    let mut add_defaults = |key: &str| {
        for (name, value) in default_spark_properties(key) {
            spark_properties.insert(name.to_string(), json!(value));
        }
    };
    add_defaults(&table_type);
    for flag in [
        "spark_encryption",
        "spark_cross_account_catalog",
        "spark_requester_pays",
    ] {
        if truthy(config, flag) {
            add_defaults(flag);
        }
    }

    let mut engine = Map::from_iter([
        (
            "CoordinatorDpuSize".to_string(),
            json!(DEFAULT_SPARK_COORDINATOR_DPU_SIZE),
        ),
        (
            "MaxConcurrentDpus".to_string(),
            json!(DEFAULT_SPARK_MAX_CONCURRENT_DPUS),
        ),
        (
            "DefaultExecutorDpuSize".to_string(),
            json!(DEFAULT_SPARK_EXECUTOR_DPU_SIZE),
        ),
    ]);
    if let Some(provided) = config_attr(config, "engine_config") {
        let provided = serde_json::to_value(&provided)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .ok_or_else(|| config_error("Engine configuration has to be of type dict"))?;
        for (key, value) in provided {
            if !ENGINE_CONFIG_KEYS.contains(&key.as_str()) {
                return Err(config_error(format!(
                    "The engine configuration keys provided do not match the expected athena \
                     engine keys: {ENGINE_CONFIG_KEYS:?}"
                )));
            }
            match (key.as_str(), value) {
                ("SparkProperties", Json::Object(properties)) => {
                    spark_properties.extend(properties)
                }
                (_, value) => {
                    engine.insert(key, value);
                }
            }
        }
    }
    if engine["MaxConcurrentDpus"] == json!(1) {
        return Err(config_error(
            "The lowest value supported for MaxConcurrentDpus is 2",
        ));
    }
    engine.insert(
        "SparkProperties".to_string(),
        Json::Object(spark_properties),
    );
    Ok(Json::Object(engine))
}

struct PooledSession {
    id: String,
    description: String,
    busy: bool,
}

static SESSIONS: LazyLock<Mutex<Vec<PooledSession>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn with_sessions<T>(f: impl FnOnce(&mut Vec<PooledSession>) -> T) -> T {
    let mut sessions = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut sessions)
}

fn str_at<'j>(value: &'j Json, path: &[&str]) -> &'j str {
    path.iter()
        .fold(value, |value, key| &value[*key])
        .as_str()
        .unwrap_or_default()
}

/// One Python model's run: the operations, its settings and the run's cancellation.
pub struct SparkJob<'o, 'a, 't, 'e> {
    pub ops: &'o AthenaOps<'a, 't, 'e>,
    pub work_group: String,
    pub config: SparkConfig,
    /// `AthenaSparkSessionManager.session_description`: sessions are reused only
    /// with the same engine configuration.
    pub description: String,
    pub relation_name: String,
    pub token: CancellationToken,
}

impl SparkJob<'_, '_, '_, '_> {
    /// Sleep one polling interval, waking early when the run is cancelled.
    fn wait(&self) -> AdapterResult<()> {
        let until = Instant::now() + self.config.polling_interval;
        while Instant::now() < until {
            if self.token.is_cancelled() {
                return Err(AdapterError::new(
                    AdapterErrorKind::Cancelled,
                    "Python model execution was cancelled",
                ));
            }
            std::thread::sleep(Duration::from_millis(200).min(self.config.polling_interval));
        }
        Ok(())
    }

    /// A free session with the job's engine configuration, or a new one.
    fn acquire_session(&self) -> AdapterResult<String> {
        let free = with_sessions(|sessions| {
            sessions
                .iter_mut()
                .find(|s| !s.busy && s.description == self.description)
                .map(|s| {
                    s.busy = true;
                    s.id.clone()
                })
        });
        if let Some(id) = free {
            return Ok(id);
        }
        let started = self.ops.call(
            "athena.start_session",
            json!({
                "Description": self.description,
                "WorkGroup": self.work_group,
                "EngineConfiguration": self.config.engine_config,
                "SessionIdleTimeoutInMinutes": SESSION_IDLE_TIMEOUT_MIN,
            }),
        )?;
        let id = str_at(&started, &["SessionId"]).to_string();
        if str_at(&started, &["State"]) != "IDLE" {
            self.wait_until_session_idle(&id)?;
        }
        with_sessions(|sessions| {
            sessions.push(PooledSession {
                id: id.clone(),
                description: self.description.clone(),
                busy: true,
            })
        });
        Ok(id)
    }

    /// `AthenaSparkSessionManager.poll_until_session_creation`
    fn wait_until_session_idle(&self, id: &str) -> AdapterResult<()> {
        let started = Instant::now();
        loop {
            let status = self
                .ops
                .call("athena.get_session_status", json!({ "SessionId": id }))?;
            match str_at(&status, &["Status", "State"]) {
                "IDLE" => return Ok(()),
                state @ ("FAILED" | "TERMINATED" | "DEGRADED") => {
                    return Err(runtime_error(format!(
                        "Unable to create session: {id}. Got status: {state} with reason: {}.",
                        str_at(&status, &["Status", "StateChangeReason"])
                    )));
                }
                _ => {}
            }
            if started.elapsed() > self.config.timeout {
                return Err(runtime_error(format!(
                    "Session {id} did not create within {} seconds.",
                    self.config.timeout.as_secs()
                )));
            }
            self.wait()?;
        }
    }

    fn release_session(id: &str, usable: bool) {
        with_sessions(|sessions| {
            if usable {
                if let Some(session) = sessions.iter_mut().find(|s| s.id == id) {
                    session.busy = false;
                }
            } else {
                sessions.retain(|s| s.id != id);
            }
        });
    }

    /// `AthenaPythonJobHelper.submit`: run the code as a calculation and wait for it.
    pub fn submit(&self, compiled_code: &str) -> AdapterResult<AdapterResponse> {
        // dbt-athena answers an empty submission without running it.
        if compiled_code.trim().is_empty() {
            return Ok(AdapterResponse::new().with_message("OK"));
        }
        // A session that ended between two models is dropped and replaced, as
        // dbt-athena does; a failure to start in a fresh session is final.
        let mut attempts = 0;
        let (session, calculation) = loop {
            let session = self.acquire_session()?;
            match self.ops.call(
                "athena.start_calculation_execution",
                json!({ "SessionId": session, "CodeBlock": compiled_code.trim_start() }),
            ) {
                Ok(started) => {
                    break (
                        session,
                        str_at(&started, &["CalculationExecutionId"]).to_string(),
                    );
                }
                Err(e) => {
                    let ended = ["TERMINATED", "TERMINATING", "DEGRADED", "FAILED"]
                        .iter()
                        .any(|state| e.message().contains(state));
                    Self::release_session(&session, !ended);
                    attempts += 1;
                    if !ended || attempts >= 3 {
                        return Err(runtime_error(format!(
                            "Unable to start spark python code execution. {}",
                            e.message()
                        )));
                    }
                }
            }
        };
        let outcome = self.wait_for_calculation(&session, &calculation);
        Self::release_session(&session, true);
        outcome.map(|()| AdapterResponse::new().with_message("OK"))
    }

    /// `AthenaPythonJobHelper.poll_until_execution_completion`; a timeout or a
    /// cancelled run stops the calculation.
    fn wait_for_calculation(&self, session: &str, calculation: &str) -> AdapterResult<()> {
        let started = Instant::now();
        loop {
            let execution = self.ops.call(
                "athena.get_calculation_execution",
                json!({ "CalculationExecutionId": calculation }),
            )?;
            match str_at(&execution, &["Status", "State"]) {
                "COMPLETED" => return Ok(()),
                state @ ("FAILED" | "CANCELED") => {
                    return Err(runtime_error(format!(
                        "Model {}\nCalculation Id:   {calculation}\nSession Id:     {session}\n\
                         Status:         {state}\nReason:         {}\nStderr s3 path: {}",
                        self.relation_name,
                        str_at(&execution, &["Status", "StateChangeReason"]),
                        str_at(&execution, &["Result", "StdErrorS3Uri"]),
                    )));
                }
                _ => {}
            }
            let stop = |error: AdapterError| {
                // Best effort: the error below is the one to report.
                let _ = self.ops.call(
                    "athena.stop_calculation_execution",
                    json!({ "CalculationExecutionId": calculation }),
                );
                Err(error)
            };
            if started.elapsed() > self.config.timeout {
                return stop(runtime_error(format!(
                    "Execution {calculation} did not complete within {} seconds.",
                    self.config.timeout.as_secs()
                )));
            }
            if let Err(cancelled) = self.wait() {
                return stop(cancelled);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(entries: &[(&str, Value)]) -> Value {
        Value::from_iter(entries.iter().map(|(k, v)| (k.to_string(), v.clone())))
    }

    #[test]
    fn spark_config_defaults_follow_dbt_athena() {
        let spark = SparkConfig::from_model_config(&config(&[]), None).unwrap();
        assert_eq!(spark.timeout, Duration::from_secs(43200));
        assert_eq!(spark.polling_interval, Duration::from_secs(1));
        assert_eq!(
            spark.engine_config,
            json!({
                "CoordinatorDpuSize": 1,
                "MaxConcurrentDpus": 2,
                "DefaultExecutorDpuSize": 1,
                "SparkProperties": {},
            })
        );

        let from_profile = SparkConfig::from_model_config(&config(&[]), Some(0.5)).unwrap();
        assert_eq!(from_profile.polling_interval, Duration::from_millis(500));
    }

    #[test]
    fn spark_config_merges_table_format_flags_and_engine_config() {
        let spark = SparkConfig::from_model_config(
            &config(&[
                ("table_type", Value::from("ICEBERG")),
                ("spark_requester_pays", Value::from(true)),
                ("timeout", Value::from(600)),
                ("polling_interval", Value::from(2)),
                (
                    "engine_config",
                    Value::from_iter([
                        ("MaxConcurrentDpus", Value::from(4)),
                        (
                            "SparkProperties",
                            Value::from_iter([("spark.sql.shuffle.partitions", Value::from("8"))]),
                        ),
                    ]),
                ),
            ]),
            Some(0.5),
        )
        .unwrap();
        assert_eq!(spark.timeout, Duration::from_secs(600));
        assert_eq!(spark.polling_interval, Duration::from_secs(2));
        let engine = &spark.engine_config;
        assert_eq!(engine["MaxConcurrentDpus"], json!(4));
        let properties = engine["SparkProperties"].as_object().unwrap();
        assert_eq!(
            properties["spark.sql.catalog.spark_catalog.catalog-impl"],
            json!("org.apache.iceberg.aws.glue.GlueCatalog")
        );
        assert_eq!(
            properties["spark.hadoop.fs.s3.useRequesterPaysHeader"],
            json!("true")
        );
        assert_eq!(properties["spark.sql.shuffle.partitions"], json!("8"));
    }

    #[test]
    fn spark_config_rejects_what_dbt_athena_rejects() {
        for (entries, message) in [
            (vec![("timeout", Value::from(0))], "positive integer"),
            (vec![("timeout", Value::from("1h"))], "must be an integer"),
            (
                vec![("polling_interval", Value::from(-1))],
                "positive number",
            ),
            (
                vec![(
                    "engine_config",
                    Value::from_iter([("MaxConcurrentDpus", Value::from(1))]),
                )],
                "MaxConcurrentDpus is 2",
            ),
            (
                vec![(
                    "engine_config",
                    Value::from_iter([("Workers", Value::from(1))]),
                )],
                "expected athena engine keys",
            ),
        ] {
            let err = SparkConfig::from_model_config(&config(&entries), None).unwrap_err();
            assert!(err.message().contains(message), "got: {}", err.message());
        }
    }
}
