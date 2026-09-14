use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use arrow::record_batch::RecordBatch;
use async_trait::async_trait;
use dbt_common::{
    ErrorCode, FsError, FsResult,
    constants::{DBT_LOG_DIR_NAME, DBT_TARGET_DIR_NAME, default_metadata_dir},
    err, stdfs,
};
use dbt_test_primitives::is_update_golden_files_mode;
use dbt_tracing::{IndexedTelemetryDeserializeError, TelemetryRecord};
#[cfg(any(windows, test))]
use once_cell::sync::Lazy;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use sha2::{Digest, Sha256};

use crate::task::{
    goldie::{TextualPatch, diff_goldie, execute_and_compare},
    tasks::prepare_command_vec,
    utils::{
        maybe_normalize_schema_name, maybe_normalize_tmp_paths, normalize_inline_sql_files,
        normalize_version,
    },
};

use super::{ProjectEnv, Task, TestEnv, TestError, TestResult, task_seq::CommandFn};

/// Deserializes telemetry parquet record batches into telemetry records for snapshot comparison.
pub type TelemetryArrowDeserializer = fn(
    &RecordBatch,
) -> Result<
    (Vec<TelemetryRecord>, Vec<IndexedTelemetryDeserializeError>),
    Box<dyn std::error::Error>,
>;

/// Native JSONL telemetry file name requested from the dbt CLI during snapshot tests.
const OTEL_JSONL_FILE_NAME: &str = "otel.jsonl";

/// Native parquet telemetry file name requested from the dbt CLI during snapshot tests.
const OTEL_PARQUET_FILE_NAME: &str = "otel.parquet";

/// Stable invocation id used to keep telemetry snapshots reproducible.
const TELEMETRY_INVOCATION_ID: &str = "424242424242";

#[cfg(any(windows, test))]
static JSON_ESCAPED_BACKSLASH_PATTERN: Lazy<regex::Regex> =
    Lazy::new(|| regex::Regex::new(r"\\{2,}").unwrap());

/// Telemetry event types that are known to be unstable and removed before normalization.
const UNSTABLE_TELEMETRY_EVENT_TYPES: &[&str] = &[
    "v1.public.events.fusion.query.AdapterConnectionOpen",
    "v1.public.events.fusion.query.AdapterConnectionClose",
];

/// Executes a dbt command and compares its stdout, stderr, JSONL telemetry, and parquet telemetry.
pub struct ExecuteAndCompareTelemetry {
    name: String,
    cmd_vec: Vec<String>,
    func: Arc<CommandFn>,
    telemetry_deserializer: TelemetryArrowDeserializer,
    /// When true, span_id/event_id are added to volatile keys and
    /// a full line sort is applied as the final normalization step.
    /// Use for commands with non-deterministic span ordering (e.g. parallel deps).
    deterministic_sort: bool,
}

impl ExecuteAndCompareTelemetry {
    /// Creates a telemetry comparison task and injects the stable telemetry output flags.
    pub fn new(
        name: String,
        mut cmd_vec: Vec<String>,
        func: Arc<CommandFn>,
        telemetry_deserializer: TelemetryArrowDeserializer,
    ) -> Self {
        assert_flag_absent(
            &cmd_vec,
            "--otel-file-name",
            "ExecuteAndCompareTelemetry sets --otel-file-name automatically",
        );
        assert_flag_absent(
            &cmd_vec,
            "--otel-parquet-file-name",
            "ExecuteAndCompareTelemetry sets --otel-parquet-file-name automatically",
        );
        assert_flag_absent(
            &cmd_vec,
            "--invocation-id",
            "ExecuteAndCompareTelemetry sets --invocation-id automatically",
        );
        assert_flag_absent(
            &cmd_vec,
            "--no-parallel",
            "ExecuteAndCompareTelemetry forces --no-parallel",
        );

        cmd_vec.push("--no-parallel".to_string());
        cmd_vec.push(format!("--otel-file-name={OTEL_JSONL_FILE_NAME}"));
        cmd_vec.push(format!("--otel-parquet-file-name={OTEL_PARQUET_FILE_NAME}"));
        cmd_vec.push(format!("--invocation-id={TELEMETRY_INVOCATION_ID}"));

        Self {
            name,
            cmd_vec,
            func,
            telemetry_deserializer,
            deterministic_sort: false,
        }
    }

    /// Enable deterministic sorting: span_id/event_id become volatile keys
    /// and a full line sort is applied as the final normalization step.
    /// Use for commands with non-deterministic span ordering (e.g. parallel deps).
    pub fn with_deterministic_sort(mut self) -> Self {
        self.deterministic_sort = true;
        self
    }
}

#[async_trait]
impl Task for ExecuteAndCompareTelemetry {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        // Prepare the remaining cli command using the common helper
        let cmd_vec = prepare_command_vec(self.cmd_vec.clone(), project_env, test_env, true);

        let mut patches = match execute_and_compare(
            &self.name,
            cmd_vec.as_slice(),
            project_env,
            test_env,
            task_index,
            false,
            self.func.clone(),
            &[],
        )
        .await
        {
            Ok(patches) => patches,
            Err(e) => return Err(e.into()),
        };

        let mut telemetry_patches =
            compare_telemetry(self, &cmd_vec, project_env, test_env, task_index)?;
        patches.append(&mut telemetry_patches);

        if patches.is_empty() {
            Ok(())
        } else {
            Err(TestError::GoldieMismatch(patches))
        }
    }

    fn is_counted(&self) -> bool {
        true
    }
}

/// Panics when a command vector already contains a flag owned by telemetry snapshot setup.
fn assert_flag_absent(cmd_vec: &[String], flag: &str, message: &str) {
    if cmd_vec.iter().any(|arg| arg.contains(flag)) {
        panic!("{message}");
    }
}

/// Compares the native JSONL and parquet telemetry outputs against golden snapshots.
fn compare_telemetry(
    task: &ExecuteAndCompareTelemetry,
    cmd_vec: &[String],
    project_env: &ProjectEnv,
    test_env: &TestEnv,
    task_index: usize,
) -> FsResult<Vec<TextualPatch>> {
    let log_dir = resolve_path(
        extract_flag_value(cmd_vec, "--log-path"),
        &project_env.absolute_project_dir,
        test_env.temp_dir.join(DBT_LOG_DIR_NAME),
    );
    let target_dir = resolve_path(
        extract_flag_value(cmd_vec, "--target-path"),
        &project_env.absolute_project_dir,
        test_env.temp_dir.join(DBT_TARGET_DIR_NAME),
    );

    let task_suffix = task_suffix(task_index);
    let actual_jsonl_path = log_dir.join(OTEL_JSONL_FILE_NAME);
    let actual_parquet_path = default_metadata_dir(&target_dir).join(OTEL_PARQUET_FILE_NAME);
    let golden_jsonl_path = test_env
        .golden_dir
        .join(format!("{}{}.otel.jsonl", task.name, task_suffix));
    let golden_parquet_jsonl_path = test_env
        .golden_dir
        .join(format!("{}{}.otel.parquet.jsonl", task.name, task_suffix));

    if !actual_jsonl_path.exists() {
        return err!(
            ErrorCode::FileNotFound,
            "expected telemetry jsonl file at {} but it was not produced",
            actual_jsonl_path.display()
        );
    }

    let deterministic_sort = task.deterministic_sort;
    let actual_jsonl_content = postprocess_jsonl(
        stdfs::read_to_string(&actual_jsonl_path)?,
        deterministic_sort,
    );

    if !actual_parquet_path.exists() {
        return err!(
            ErrorCode::FileNotFound,
            "expected telemetry parquet file at {} but it was not produced",
            actual_parquet_path.display()
        );
    }

    let actual_parquet_bytes = stdfs::read(&actual_parquet_path)?;
    let actual_parquet_as_jsonl = parquet_to_jsonl(
        actual_parquet_bytes,
        task.telemetry_deserializer,
        deterministic_sort,
    )?;

    if is_update_golden_files_mode() {
        stdfs::write(&golden_jsonl_path, &actual_jsonl_content)?;
        stdfs::write(&golden_parquet_jsonl_path, &actual_parquet_as_jsonl)?;
        return Ok(vec![]);
    }

    let normalize_golden =
        |content: String| -> String { postprocess_jsonl(content, deterministic_sort) };

    let patches = diff_goldie(
        "jsonl telemetry",
        actual_jsonl_content,
        false,
        &golden_jsonl_path,
        normalize_golden,
    )
    .into_iter()
    .chain(diff_goldie(
        "parquet telemetry",
        actual_parquet_as_jsonl,
        false,
        &golden_parquet_jsonl_path,
        normalize_golden,
    ))
    .collect::<Vec<_>>();

    Ok(patches)
}

/// Converts parquet bytes into normalized JSONL snapshot content.
fn parquet_to_jsonl(
    parquet_bytes: Vec<u8>,
    deserializer: TelemetryArrowDeserializer,
    deterministic_sort: bool,
) -> FsResult<String> {
    let records = read_parquet_to_records(parquet_bytes, deserializer)?;
    let jsonl_lines: Result<Vec<String>, _> = records
        .into_iter()
        .map(|record| {
            serde_json::to_string(&record).map_err(|err| {
                Box::new(FsError::new(
                    ErrorCode::SerializationError,
                    format!("failed to serialize telemetry record to json: {err}"),
                ))
            })
        })
        .collect();
    let jsonl = jsonl_lines?.join("\n");

    Ok(postprocess_jsonl(jsonl, deterministic_sort))
}

/// Reads parquet bytes and deserializes each record batch into telemetry records.
fn read_parquet_to_records(
    parquet_bytes: Vec<u8>,
    deserializer: TelemetryArrowDeserializer,
) -> FsResult<Vec<TelemetryRecord>> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(bytes::Bytes::from_owner(parquet_bytes))
        .map_err(|err| {
            FsError::new(
                ErrorCode::ParquetError,
                format!("failed to construct parquet reader: {err}"),
            )
        })?
        .build()
        .map_err(|err| {
            FsError::new(
                ErrorCode::ParquetError,
                format!("failed to build parquet batch reader: {err}"),
            )
        })?;

    let mut records = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|err| {
            FsError::new(
                ErrorCode::ParquetError,
                format!("failed to read parquet batch: {err}"),
            )
        })?;
        let (mut batch_records, errors) = deserializer(&batch).map_err(|err| {
            FsError::new(
                ErrorCode::ParquetError,
                format!("failed to deserialize telemetry records: {err}"),
            )
        })?;
        if !errors.is_empty() {
            let message = errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            return Err(FsError::new(
                ErrorCode::ParquetError,
                format!("failed to deserialize telemetry rows: {message}"),
            )
            .into());
        }
        records.append(&mut batch_records);
    }

    Ok(records)
}

/// Resolves a command path flag to an absolute path, using a default when the flag is absent.
fn resolve_path(maybe_path: Option<String>, project_dir: &Path, default: PathBuf) -> PathBuf {
    match maybe_path {
        Some(value) => {
            let candidate = PathBuf::from(value);
            if candidate.is_relative() {
                project_dir.join(candidate)
            } else {
                candidate
            }
        }
        None => default,
    }
}

/// Builds the golden file suffix for a task index.
fn task_suffix(task_index: usize) -> String {
    if task_index > 0 {
        format!("_{task_index}")
    } else {
        String::new()
    }
}

/// Extracts a command-line flag value from either `--flag value` or `--flag=value` forms.
fn extract_flag_value(cmd_vec: &[String], flag: &str) -> Option<String> {
    let prefix = format!("{flag}=");
    let iter = cmd_vec.iter().enumerate();
    for (idx, arg) in iter {
        if arg == flag {
            return cmd_vec.get(idx + 1).cloned();
        }
        if arg.starts_with(&prefix) {
            return Some(arg[prefix.len()..].to_string());
        }
    }
    None
}

/// Applies a mutation to a JSON value found by a dot-delimited object path.
fn apply_by_path<F>(value: &mut serde_json::Value, key: &str, mut transform: F)
where
    F: FnMut(&mut serde_json::Value),
{
    let parts: Vec<&str> = key.split('.').collect();

    fn apply_by_path_inner<F: FnMut(&mut serde_json::Value)>(
        value: &mut serde_json::Value,
        path: &[&str],
        mut transform: F,
    ) {
        if path.is_empty() {
            return;
        }
        if let Some(obj) = value.as_object_mut() {
            if path.len() == 1 {
                if let Some(v) = obj.get_mut(path[0]) {
                    transform(v);
                }
            } else if let Some(next) = obj.get_mut(path[0]) {
                apply_by_path_inner(next, &path[1..], transform);
            }
        }
    }

    apply_by_path_inner(value, parts.as_slice(), &mut transform);
}

/// Replaces a JSON value with a type-appropriate normalized placeholder.
fn normalize_value(v: &mut serde_json::Value) {
    if v.is_string() {
        *v = serde_json::Value::String("<normalized>".to_string());
    } else if v.is_number() {
        *v = serde_json::Value::Number(serde_json::Number::from(0));
    } else if v.is_array() {
        *v = serde_json::Value::Array(vec![]);
    } else if v.is_object() {
        *v = serde_json::Value::Object(serde_json::Map::new());
    } else if v.is_boolean() {
        *v = serde_json::Value::Bool(false);
    } else if v.is_null() {
        // do nothing
    }
}

/// Recursively finds the first matching key at each object level and normalizes its value.
fn find_key_and_normalize(value: &mut serde_json::Value, key: &str) {
    if let Some(obj) = value.as_object_mut() {
        if let Some(v) = obj.get_mut(key) {
            normalize_value(v);
        } else {
            for (_k, v) in obj.iter_mut() {
                find_key_and_normalize(v, key);
            }
        }
    } else if let Some(arr) = value.as_array_mut() {
        for v in arr.iter_mut() {
            find_key_and_normalize(v, key);
        }
    }
}

/// Returns a top-level string field from a JSON object.
fn string_field<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value.as_object()?.get(key)?.as_str()
}

/// Removes top-level keys from a JSON object.
fn remove_top_level_keys(value: &mut serde_json::Value, keys: &[&str]) {
    if let Some(obj) = value.as_object_mut() {
        for key in keys {
            obj.remove(*key);
        }
    }
}

/// Builds a stable 16-character span id from a span signature.
fn canonical_span_id(signature: &str, duplicate_index: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"span\0");
    hasher.update(signature.as_bytes());
    hasher.update(b"\0");
    hasher.update(duplicate_index.to_string().as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Builds a stable UUID-shaped event id from a log-record signature.
fn canonical_event_id(signature: &str, duplicate_index: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"event\0");
    hasher.update(signature.as_bytes());
    hasher.update(b"\0");
    hasher.update(duplicate_index.to_string().as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let hex = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Rewrites span references in a telemetry record using canonical span ids.
fn rewrite_span_references(value: &mut serde_json::Value, span_id_map: &HashMap<String, String>) {
    if let Some(obj) = value.as_object_mut() {
        for key in ["span_id", "parent_span_id"] {
            if let Some(id) = obj
                .get(key)
                .and_then(|value| value.as_str())
                .and_then(|id| span_id_map.get(id))
                .cloned()
            {
                obj.insert(key.to_string(), serde_json::Value::String(id));
            }
        }

        if let Some(links) = obj.get_mut("links").and_then(|value| value.as_array_mut()) {
            for link in links {
                if let Some(link_obj) = link.as_object_mut()
                    && let Some(id) = link_obj
                        .get("span_id")
                        .and_then(|value| value.as_str())
                        .and_then(|id| span_id_map.get(id))
                        .cloned()
                {
                    link_obj.insert("span_id".to_string(), serde_json::Value::String(id));
                }
            }
        }
    }
}

/// Canonicalizes telemetry span and event ids without changing JSONL record order.
fn canonicalize_telemetry_ids(content: String) -> String {
    let records = content
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                None
            } else {
                Some(
                    serde_json::from_str::<serde_json::Value>(line)
                        .unwrap_or_else(|_| panic!("Failed to parse jsonl line: {line}")),
                )
            }
        })
        .collect::<Vec<_>>();

    let mut span_id_map = HashMap::new();
    let mut span_signature_counts = HashMap::new();

    for record in records.iter().flatten() {
        if string_field(record, "record_type") != Some("SpanStart") {
            continue;
        }

        let Some(original_span_id) = string_field(record, "span_id").map(ToOwned::to_owned) else {
            continue;
        };
        let canonical_parent_span_id = string_field(record, "parent_span_id")
            .and_then(|parent_span_id| span_id_map.get(parent_span_id))
            .cloned();

        let mut signature_record = record.clone();
        remove_top_level_keys(
            &mut signature_record,
            &[
                "trace_id",
                "span_id",
                "parent_span_id",
                "links",
                "start_time_unix_nano",
            ],
        );
        if let Some(obj) = signature_record.as_object_mut() {
            obj.insert(
                "__canonical_parent_span_id".to_string(),
                canonical_parent_span_id
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null),
            );
        }

        let signature = serde_json::to_string(&signature_record)
            .expect("Failed to serialize telemetry span signature");
        let duplicate_index = span_signature_counts.entry(signature.clone()).or_insert(0);
        *duplicate_index += 1;

        span_id_map.insert(
            original_span_id,
            canonical_span_id(&signature, *duplicate_index),
        );
    }

    let mut event_signature_counts = HashMap::new();
    records
        .into_iter()
        .map(|record| {
            let Some(mut record) = record else {
                return String::new();
            };

            let canonical_log_span_id = string_field(&record, "span_id")
                .and_then(|span_id| span_id_map.get(span_id))
                .cloned();
            rewrite_span_references(&mut record, &span_id_map);

            if string_field(&record, "record_type") == Some("LogRecord")
                && string_field(&record, "event_id").is_some()
            {
                let canonical_span_id = canonical_log_span_id
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null);
                let mut signature_record = record.clone();
                remove_top_level_keys(
                    &mut signature_record,
                    &["trace_id", "event_id", "span_id", "time_unix_nano"],
                );
                if let Some(obj) = signature_record.as_object_mut() {
                    obj.insert("__canonical_span_id".to_string(), canonical_span_id);
                }

                let signature = serde_json::to_string(&signature_record)
                    .expect("Failed to serialize telemetry event signature");
                let duplicate_index = event_signature_counts.entry(signature.clone()).or_insert(0);
                *duplicate_index += 1;
                let event_id = canonical_event_id(&signature, *duplicate_index);
                if let Some(obj) = record.as_object_mut() {
                    obj.insert("event_id".to_string(), serde_json::Value::String(event_id));
                }
            }

            serde_json::to_string(&record).expect("Failed to serialize modified jsonl line")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Matches the `body` of ad hoc perf-debugging `tracing::debug!("... took {duration:?}")`
/// log records, which embed a live-measured duration and can never reproduce byte-for-byte
/// across runs.
const PERF_TIMING_LOG_BODY_PATTERN: &str = r"took [0-9.]+(?:ns|µs|ms|s)$";

/// Removes telemetry records whose event type is listed as unstable, and ad hoc
/// perf-timing log records whose body embeds a live-measured, non-reproducible duration.
fn remove_unstable_telemetry_records(content: String) -> String {
    let timing_re = regex::Regex::new(PERF_TIMING_LOG_BODY_PATTERN)
        .expect("Invalid perf-timing log body regex");

    content
        .lines()
        .filter_map(|line| {
            if line.trim().is_empty() {
                return Some(String::new());
            }

            let json: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|_| panic!("Failed to parse jsonl line: {line}"));
            let event_type = string_field(&json, "event_type");
            if event_type
                .is_some_and(|event_type| UNSTABLE_TELEMETRY_EVENT_TYPES.contains(&event_type))
            {
                return None;
            }
            if event_type == Some("v1.public.events.fusion.log.LogMessage")
                && string_field(&json, "body").is_some_and(|body| timing_re.is_match(body))
            {
                return None;
            }
            Some(line.to_string())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Returns JSON keys whose values should be replaced during volatile-field normalization.
fn volatile_keys(deterministic_sort: bool) -> Vec<&'static str> {
    let mut keys = vec![
        "time_unix_nano",
        "start_time_unix_nano",
        "end_time_unix_nano",
        "duration_ms",
        "raw_command",
        "log_path",
        "project_dir",
        "target_path",
        "sql_hash",
        "host_os",
        "host_arch",
        "version",
        // `mem::size_of` of the closure handed to `spawn_blocking`, so it is a
        // target-layout fact, not an observation: the same closure measures 136
        // bytes on Linux/macOS and 144 on windows-msvc. Snapshotting it makes
        // the golden pass on one platform and fail on the others.
        "size.bytes",
        "original_size.bytes",
    ];
    if deterministic_sort {
        keys.extend(["span_id", "event_id", "parent_span_id"]);
    }
    keys
}

/// Normalizes volatile telemetry JSON fields and version-like progress targets.
fn normalize_volatile_keys(content: String, deterministic_sort: bool) -> String {
    const REGEX_KEYS: &[(&str, &str, &str)] = &[(
        "attributes.target",
        r"(?:\d+\.\d+\.\d+|dbt-fusion-version)(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?",
        "dbt-fusion-version",
    )];
    let keys = volatile_keys(deterministic_sort);

    content
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                return String::new();
            }

            let mut json: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|_| panic!("Failed to parse jsonl line: {line}"));

            for key in &keys {
                find_key_and_normalize(&mut json, key);
            }

            for (key_path, pattern, replacement) in REGEX_KEYS {
                let re = regex::Regex::new(pattern)
                    .unwrap_or_else(|_| panic!("Invalid regex pattern: {pattern}"));
                apply_by_path(&mut json, key_path, |v| {
                    if let Some(s) = v.as_str() {
                        *v = serde_json::Value::String(re.replace_all(s, *replacement).to_string());
                    }
                });
            }

            serde_json::to_string(&json).expect("Failed to serialize modified jsonl line")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Nulls telemetry keys that cannot be reproduced by replayed warehouse responses.
fn strip_non_replayable_keys(content: String) -> String {
    const KEYS: &[&str] = &["attributes.query_id"];

    content
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                return String::new();
            }

            let mut json: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|_| panic!("Failed to parse jsonl line: {line}"));

            for key in KEYS {
                apply_by_path(&mut json, key, |v| *v = serde_json::Value::Null);
            }

            serde_json::to_string(&json).expect("Failed to serialize modified jsonl line")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Normalizes path separators in JSON strings for Windows compatibility.
fn json_safe_normalize_slashes(output: String) -> String {
    #[cfg(windows)]
    {
        normalize_json_escaped_slashes(output)
    }
    #[cfg(not(windows))]
    {
        output
    }
}

#[cfg(any(windows, test))]
fn normalize_json_escaped_slashes(output: String) -> String {
    JSON_ESCAPED_BACKSLASH_PATTERN
        .replace_all(&output, "|")
        .replace('/', "|")
}

/// Applies every telemetry JSONL normalization transform in comparison order.
fn postprocess_jsonl(content: String, deterministic_sort: bool) -> String {
    let mut transforms: Vec<Box<dyn Fn(String) -> String>> = vec![
        Box::new(remove_unstable_telemetry_records),
        Box::new(maybe_normalize_schema_name),
        Box::new(maybe_normalize_tmp_paths),
        Box::new(normalize_unrendered_config_in_model_strings),
        Box::new(|content| normalize_volatile_keys(content, false)),
        Box::new(strip_non_replayable_keys),
        Box::new(json_safe_normalize_slashes),
        Box::new(normalize_version),
        Box::new(normalize_inline_sql_files),
        Box::new(canonicalize_telemetry_ids),
    ];

    if deterministic_sort {
        transforms.push(Box::new(|content| normalize_volatile_keys(content, true)));
        transforms.push(Box::new(|content| {
            let mut lines: Vec<&str> = content.lines().collect();
            lines.sort();
            lines.join("\n")
        }));
    }

    transforms
        .into_iter()
        .fold(content, |acc, transform| transform(acc))
}

/// Strips unstable `unrendered_config` fragments from debug-printed node strings.
fn normalize_unrendered_config_in_model_strings(output: String) -> String {
    output
        .replace(", 'unrendered_config': {}, ", ", ")
        .replace(", 'unrendered_config': {}", "")
}

#[cfg(test)]
mod tests {
    use super::{normalize_json_escaped_slashes, normalize_volatile_keys};
    use crate::task::utils::normalize_windows_tmp_paths;

    #[test]
    fn normalizes_nested_windows_temp_paths() {
        let expected = r#"{"string":"{'root_path': '|tmp|.tmpXXXXXX|root'}"}"#;
        for content in [
            r#"{"string":"{'root_path': 'C:\\Users\\Administrator\\AppData\\Local\\Temp\\.tmpcSTNo8\\root'}"}"#,
            r#"{"string":"{'root_path': 'C:\\\\Users\\\\Administrator\\\\AppData\\\\Local\\\\Temp\\\\.tmpcSTNo8\\\\root'}"}"#,
        ] {
            let normalized = normalize_windows_tmp_paths(content.to_string());
            assert_eq!(normalize_json_escaped_slashes(normalized), expected);
        }
    }

    #[test]
    fn normalizes_preview_nightly_version_targets() {
        let content = r#"{"attributes":{"action":"dbt-fusion","target":"dbt-fusion-version-preview-nightly.176"},"event_type":"v1.public.events.fusion.log.ProgressMessage"}"#.to_string();

        let normalized = normalize_volatile_keys(content, false);

        assert!(normalized.contains(r#""target":"dbt-fusion-version""#));
        assert!(!normalized.contains("preview-nightly.176"));
    }
}
