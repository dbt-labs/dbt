//! Replay infrastructure for time-machine recordings.
//!
//! Loads and navigates recorded adapter events. Replay defaults to strict mode, where
//! events must match in exact recorded order. Semantic mode relaxes this: writes still
//! act as ordering barriers that must match in sequence, but reads between two barriers
//! (a "segment") can match in any order, so minor read reordering doesn't break replay.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, BufRead, BufReader};
use std::path::Path;
use std::str::FromStr;
use std::sync::LazyLock;

use flate2::read::GzDecoder;
use parking_lot::RwLock;
use regex::Regex;
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

use super::event::{
    AdapterCallEvent, CacheInvalidationEvent, MetadataCallArgs, MetadataCallEvent, RecordedEvent,
    RecordingHeader, RunCacheCloneEvent, RunRemoteAdhocEvent, SaoEvent,
};
use super::semantic::SemanticCategory;
use super::serde::values_match;
use super::validation::{SqlSanitizer, TmpSuffixSanitizer, UuidSanitizer};
use crate::AdapterType;
use crate::sql::diff::{canonicalize_python_model_pair, compare_sql};

/// Marker for temp-relation identifiers, which carry a non-deterministic suffix.
static TMP_MARKER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)__dbt_tmp").expect("valid regex"));

/// Extract the SQL string from args (first string in array, or the string itself).
///
/// For execute/run_query, args are serialized as `[sql, auto_begin, fetch, limit, options]`
/// so SQL is the first element of the array.
fn extract_sql_from_args(args: &serde_json::Value) -> Option<&str> {
    match args {
        serde_json::Value::String(s) => Some(s.as_str()),
        serde_json::Value::Array(arr) => arr.first().and_then(|v| v.as_str()),
        _ => None,
    }
}

fn is_sql_method(method: &str) -> bool {
    method == "execute" || method == "run_query"
}

/// Returns true if the SQL string is read-only and cannot mutate DB state.
///
/// These are calls that use execute/run_query but are semantically reads —
/// for example compile-time probe queries from dbt macros like dbt_utils.date_spine
/// (`SELECT datediff(...)`) or Snowflake parameter reads (`SHOW PARAMETERS ...`).
///
/// Public so the Mantle replay engine (`sdf-adapter`) classifies read-only statements
/// identically; both engines must agree on what counts as an ordering barrier.
pub fn is_read_only_sql(sql: &str) -> bool {
    // Only check the first 8 chars to keep this O(1).
    let prefix: String = sql
        .trim()
        .chars()
        .take(8)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    prefix.starts_with("SELECT")
        || prefix.starts_with("SHOW")
        || prefix.starts_with("EXPLAIN")
        || prefix.starts_with("DESCRIBE")
}

/// Returns true if this recorded event is a read-only execute/run_query call.
fn is_read_only_execute_event(event: &AdapterCallEvent) -> bool {
    is_sql_method(&event.method)
        && extract_sql_from_args(&event.args)
            .map(is_read_only_sql)
            .unwrap_or(false)
}

/// Returns true if this incoming replay call is a read-only execute/run_query.
pub(crate) fn is_read_only_execute_call(method: &str, args: &serde_json::Value) -> bool {
    is_sql_method(method)
        && extract_sql_from_args(args)
            .map(is_read_only_sql)
            .unwrap_or(false)
}

/// Compare two MetadataCallArgs for semantic equality.
///
/// This compares the structured arguments to ensure we match the correct
/// recorded event when replaying.
fn metadata_args_match(recorded: &MetadataCallArgs, actual: &MetadataCallArgs) -> bool {
    fn relations_match(recorded_relations: &[String], actual_relations: &[String]) -> bool {
        let recorded_set: HashSet<String> = recorded_relations
            .iter()
            .map(|r| r.to_ascii_uppercase())
            .collect();
        actual_relations
            .iter()
            .all(|rel| recorded_set.contains(&rel.to_ascii_uppercase()))
    }

    match (recorded, actual) {
        // For ListRelationsSchemas, check if recorded is a superset of actual.
        // If recorded contains all the relations requested in actual, we have
        // all the data needed to satisfy this request.
        (
            MetadataCallArgs::ListRelationsSchemas {
                relations: recorded_relations,
                ..
            },
            MetadataCallArgs::ListRelationsSchemas {
                relations: actual_relations,
                ..
            },
        ) => {
            // Case-insensitive superset check: semantic_fqn strings may differ
            // only in casing due to identifier normalization (e.g. Snowflake
            // uppercasing unquoted identifiers), so we compare uppercased forms.
            relations_match(recorded_relations, actual_relations)
        }
        (
            MetadataCallArgs::FreshnessAllInSchema {
                database: recorded_database,
                schema: recorded_schema,
                relations: recorded_relations,
                warehouse: recorded_warehouse,
            },
            MetadataCallArgs::FreshnessAllInSchema {
                database: actual_database,
                schema: actual_schema,
                relations: actual_relations,
                warehouse: actual_warehouse,
            },
        ) => {
            recorded_database.eq_ignore_ascii_case(actual_database)
                && recorded_schema.eq_ignore_ascii_case(actual_schema)
                && recorded_warehouse == actual_warehouse
                && relations_match(recorded_relations, actual_relations)
        }
        // For other types, use exact value matching
        _ => {
            std::mem::discriminant(recorded) == std::mem::discriminant(actual) && {
                match (serde_json::to_value(recorded), serde_json::to_value(actual)) {
                    (Ok(r), Ok(a)) => values_match(&r, &a),
                    _ => false,
                }
            }
        }
    }
}

pub(crate) fn adapter_args_match_for_type(
    method: &str,
    recorded: &serde_json::Value,
    actual: &serde_json::Value,
    adapter_type: AdapterType,
) -> bool {
    // Temp-relation identifiers embed a non-deterministic suffix, so normalize both
    // sides before any per-method comparison.
    let recorded_norm = normalized_tmp_suffixes(recorded);
    let actual_norm = normalized_tmp_suffixes(actual);
    let (recorded, actual) = (recorded_norm.as_ref(), actual_norm.as_ref());

    match method {
        "get_relation" => match (
            GetRelationArgs::try_from(recorded),
            GetRelationArgs::try_from(actual),
        ) {
            (Ok(recorded), Ok(actual)) => recorded == actual,
            _ => values_match(recorded, actual),
        },
        // A relation's name uniquely identifies the warehouse object, so its columns do not
        // depend on the relation *type* flags. Compare everything else strictly.
        "get_columns_in_relation" => {
            let recorded = without_relation_type_flags(recorded);
            let actual = without_relation_type_flags(actual);
            values_match(recorded.as_ref(), actual.as_ref())
        }
        "get_column_schema_from_query" | "get_columns_in_select_sql" => {
            extract_sql_from_args(recorded)
                .zip(extract_sql_from_args(actual))
                .map_or_else(
                    || values_match(recorded, actual),
                    |(rec_sql, act_sql)| {
                        UuidSanitizer.sanitize(rec_sql) == UuidSanitizer.sanitize(act_sql)
                    },
                )
        }
        "execute" | "run_query" => extract_sql_from_args(recorded)
            .zip(extract_sql_from_args(actual))
            .map_or_else(
                || values_match(recorded, actual),
                |(rec_sql, act_sql)| compare_sql(rec_sql, act_sql, adapter_type).is_ok(),
            ),
        "submit_python_job" => python_job_args_match(recorded, actual),
        _ => values_match(recorded, actual),
    }
}

/// Relation flags derived from a relation's materialization rather than its name.
///
/// These are the `RelationObject` fields written by `serializable_impls.rs` that answer
/// "what kind of thing is this", as opposed to "which thing is this".
const RELATION_TYPE_FLAG_KEYS: &[&str] = &[
    "is_delta",
    "is_dynamic_table",
    "is_interactive_table",
    "is_materialized_view",
    "is_metric_view",
    "is_streaming_table",
    "is_table",
    "is_view",
];

/// Drop the relation-type flags from every object in the tree, borrowing when none are present.
///
/// `Adapter::get_columns_in_relation` resolves a relation by `database`/`schema`/`identifier`
/// only — the type flags never reach the query — so a relation whose inferred type differs
/// between record and replay must still match. That is not hypothetical: a cross-project (Mesh)
/// `ref` to a public node with no materialization records as `relation_type: None`
/// (`is_table: false, is_view: false`) and replays as `View` (`is_view: true`), which made
/// otherwise-valid recordings unreplayable even on the version that produced them.
///
/// Stripping the flags rather than allowlisting identity fields keeps every *other* field
/// strict, so a genuine divergence (different `quote_policy`, `is_cte`, ...) still fails, and
/// newly added type flags are covered without another edit here — `is_interactive_table` was
/// added to the serialized shape after the recordings that hit this.
fn without_relation_type_flags(value: &serde_json::Value) -> Cow<'_, serde_json::Value> {
    if has_relation_type_flag(value) {
        Cow::Owned(strip_relation_type_flags(value))
    } else {
        Cow::Borrowed(value)
    }
}

/// True if any object in the JSON tree carries a relation-type flag.
fn has_relation_type_flag(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(arr) => arr.iter().any(has_relation_type_flag),
        serde_json::Value::Object(obj) => {
            obj.keys()
                .any(|key| RELATION_TYPE_FLAG_KEYS.contains(&key.as_str()))
                || obj.values().any(has_relation_type_flag)
        }
        _ => false,
    }
}

/// Recursively remove [`RELATION_TYPE_FLAG_KEYS`] from every object in the tree.
fn strip_relation_type_flags(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(strip_relation_type_flags).collect())
        }
        serde_json::Value::Object(obj) => serde_json::Value::Object(
            obj.iter()
                .filter(|(key, _)| !RELATION_TYPE_FLAG_KEYS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), strip_relation_type_flags(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Normalize `__dbt_tmp<suffix>` in every string, borrowing when no marker is present.
///
/// The sanitizer is the identity without a marker, so the gate only avoids the clone.
fn normalized_tmp_suffixes(value: &serde_json::Value) -> Cow<'_, serde_json::Value> {
    if has_tmp_marker(value) {
        Cow::Owned(normalize_tmp_suffixes(value))
    } else {
        Cow::Borrowed(value)
    }
}

/// True if any string in the JSON tree mentions `__dbt_tmp` (case-insensitive).
fn has_tmp_marker(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(s) => TMP_MARKER_RE.is_match(s),
        serde_json::Value::Array(arr) => arr.iter().any(has_tmp_marker),
        serde_json::Value::Object(obj) => obj.values().any(has_tmp_marker),
        _ => false,
    }
}

/// Recursively normalize `__dbt_tmp<suffix>` substrings in every string value.
///
/// Temp-relation identifiers carry a non-deterministic suffix; normalize both sides like
/// `TmpSuffixSanitizer` does for SQL text, or replay false-fails.
fn normalize_tmp_suffixes(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => serde_json::Value::String(TmpSuffixSanitizer.sanitize(s)),
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(normalize_tmp_suffixes).collect())
        }
        serde_json::Value::Object(obj) => serde_json::Value::Object(
            obj.iter()
                .map(|(k, v)| (k.clone(), normalize_tmp_suffixes(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Compare `submit_python_job` args, ignoring the model object's `raw_code` field.
///
/// `submit_python_job` is invoked as `(model, compiled_code)`. The first arg is the
/// fully serialized model object and the second is the compiled python that is actually
/// submitted to the warehouse — the latter is what determines behavior.
///
/// The model object carries `raw_code`, which used to be a `--placeholder--` stub at run
/// time and is now populated with the verbatim source at parse time (see
/// dbt-parser `resolve_models.rs`, commit e2ff091). That field does not affect the
/// submitted job, so recordings captured before raw_code was populated would otherwise
/// false-fail replay against newer binaries. Strip `raw_code` from the model object on
/// both sides before comparing so the comparison reflects the actual submission.
///
/// The compiled code itself is compared with `python_code_matches` rather than byte-wise,
/// so pure layout changes in how Fusion joins the user's source to `py_script_postfix`
/// don't invalidate every python-model recording.
fn python_job_args_match(recorded: &serde_json::Value, actual: &serde_json::Value) -> bool {
    fn strip_model_raw_code(args: &serde_json::Value) -> serde_json::Value {
        let mut args = args.clone();
        if let Some(model) = args.as_array_mut().and_then(|arr| arr.first_mut()) {
            fn strip(value: &mut serde_json::Value, inside_model: bool) {
                if let Some(obj) = value.as_object_mut() {
                    let is_model = obj
                        .get("__type__")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|type_name| type_name.ends_with("LazyModelWrapper"));
                    if is_model || inside_model && obj.contains_key("adapter") {
                        obj.remove("raw_code");
                        obj.remove("adapter");
                    }
                    for child in obj.values_mut() {
                        strip(child, is_model || inside_model);
                    }
                } else if let Some(array) = value.as_array_mut() {
                    for child in array {
                        strip(child, inside_model);
                    }
                }
            }
            strip(model, false);
        }
        if let Some(code) = args.as_array_mut().and_then(|arr| arr.get_mut(1))
            && let Some(code) = code.as_str()
        {
            *args.as_array_mut().unwrap().get_mut(1).unwrap() = serde_json::Value::String(
                code.lines()
                    .filter(|line| !line.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        args
    }
    let recorded = strip_model_raw_code(recorded);
    let actual = strip_model_raw_code(actual);

    match (recorded.as_array(), actual.as_array()) {
        (Some(rec_args), Some(act_args)) if rec_args.len() == act_args.len() => rec_args
            .iter()
            .zip(act_args.iter())
            .enumerate()
            .all(
                |(index, (rec, act))| match (index, rec.as_str(), act.as_str()) {
                    (COMPILED_CODE_ARG_INDEX, Some(rec_code), Some(act_code)) => {
                        python_code_matches(rec_code, act_code)
                    }
                    _ => values_match(rec, act),
                },
            ),
        _ => values_match(&recorded, &actual),
    }
}

/// Position of the compiled python payload in `submit_python_job(model, compiled_code)`.
const COMPILED_CODE_ARG_INDEX: usize = 1;

/// Compare two compiled python model payloads, ignoring differences that cannot change
/// the program the warehouse runs: line endings, trailing whitespace, and how many blank
/// lines separate two statements.
///
/// Fusion assembles this payload as `<user source> + <py_script_postfix>`, and the joining
/// whitespace has changed before (PR #13818 switched to `trim_end()` plus a blank line to
/// match dbt Core's layout, which shifted every python model by one blank line). Blank
/// lines are not significant to the python tokenizer, so treating them as equal keeps
/// recordings valid across those edits while still catching real changes to the submitted
/// code. The one lossy case is blank lines inside a triple-quoted string literal.
fn python_code_matches(recorded: &str, actual: &str) -> bool {
    // Handles CRLF vs LF plus the config_dict/meta_get equivalences shared with the
    // Mantle replay path.
    let (actual, recorded) = canonicalize_python_model_pair(actual, recorded);
    normalize_python_blank_lines(&recorded) == normalize_python_blank_lines(&actual)
}

/// Collapse every run of blank lines to a single blank line, strip trailing whitespace on
/// each line, and drop leading/trailing blank lines. Indentation is preserved.
fn normalize_python_blank_lines(code: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut pending_blank = false;
    for line in code.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            pending_blank = true;
        } else {
            if pending_blank && !lines.is_empty() {
                lines.push("");
            }
            pending_blank = false;
            lines.push(line);
        }
    }
    lines.join("\n")
}

#[derive(Debug, PartialEq, Eq)]
struct GetRelationArgs {
    database: String,
    schema: String,
    identifier: String,
    needs_information: bool,
}

impl TryFrom<&serde_json::Value> for GetRelationArgs {
    type Error = ();

    fn try_from(args: &serde_json::Value) -> Result<Self, Self::Error> {
        let args = args.as_array().ok_or(())?;

        if args.len() == 1
            && let Some(kwargs) = args[0].as_object()
        {
            return Self::try_from_kwargs(kwargs);
        }

        let database = args.first().and_then(|arg| arg.as_str()).ok_or(())?;
        let schema = args.get(1).and_then(|arg| arg.as_str()).ok_or(())?;
        let identifier = args.get(2).and_then(|arg| arg.as_str()).ok_or(())?;
        let needs_information = args
            .get(3)
            .and_then(|arg| arg.as_bool())
            .or_else(|| {
                args.iter()
                    .filter_map(|arg| arg.as_object())
                    .find_map(|obj| obj.get("needs_information").and_then(|v| v.as_bool()))
            })
            .unwrap_or(false);

        Ok(Self {
            database: database.to_string(),
            schema: schema.to_string(),
            identifier: identifier.to_string(),
            needs_information,
        })
    }
}

impl GetRelationArgs {
    fn try_from_kwargs(kwargs: &serde_json::Map<String, serde_json::Value>) -> Result<Self, ()> {
        Ok(Self {
            database: kwargs
                .get("database")
                .and_then(|value| value.as_str())
                .ok_or(())?
                .to_string(),
            schema: kwargs
                .get("schema")
                .and_then(|value| value.as_str())
                .ok_or(())?
                .to_string(),
            identifier: kwargs
                .get("identifier")
                .and_then(|value| value.as_str())
                .ok_or(())?
                .to_string(),
            needs_information: kwargs
                .get("needs_information")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    }
}

/// Replay ordering mode.
///
/// Controls how events are matched during replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayMode {
    /// Events must match in exact recorded sequence order.
    ///
    /// This is the most restrictive mode and enforces deterministic replay.
    /// Used if calls must all strictly be in the same order.
    #[default]
    Strict,

    /// Events are matched based on semantic constraints.
    ///
    /// Write operations act as barriers and must match in strict order.
    /// Read operations can match flexibly within a "segment" (between writes).
    ///
    /// This mode is useful when:
    /// - The code version being tested may have minor reordering of reads
    /// - You want to test semantic equivalence rather than exact call sequence
    Semantic,
}

impl ReplayMode {
    pub fn allows_flexible_reads(&self) -> bool {
        matches!(self, Self::Semantic)
    }
}

impl std::fmt::Display for ReplayMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Strict => write!(f, "strict"),
            Self::Semantic => write!(f, "semantic"),
        }
    }
}

/// Errors that can occur during replay.
#[derive(Debug)]
pub enum ReplayError {
    /// IO error
    Io(io::Error),
    /// JSON parse error
    Json(serde_json::Error),
    /// Recording not found
    NotFound(String),
    /// Invalid recording format
    InvalidFormat(String),
    /// Method mismatch
    MethodMismatch { expected: String, actual: String },
    /// No recorded event found
    NoRecordedEvent {
        node_id: String,
        method: String,
        seq: u32,
    },
    /// Recorded call failed
    RecordedFailure(String),
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {}", e),
            Self::Json(e) => write!(f, "JSON parse error: {}", e),
            Self::NotFound(path) => write!(f, "Recording not found at: {}", path),
            Self::InvalidFormat(msg) => write!(f, "Invalid recording format: {}", msg),
            Self::MethodMismatch { expected, actual } => {
                write!(
                    f,
                    "Method mismatch: expected '{}', got '{}'",
                    expected, actual
                )
            }
            Self::NoRecordedEvent {
                node_id,
                method,
                seq,
            } => {
                write!(
                    f,
                    "No recorded event for node '{}' method '{}' (seq {})",
                    node_id, method, seq
                )
            }
            Self::RecordedFailure(msg) => write!(f, "Recorded call failed: {}", msg),
        }
    }
}

impl std::error::Error for ReplayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ReplayError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for ReplayError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// A loaded recording ready for replay.
#[derive(Debug)]
pub struct Recording {
    /// Recording metadata
    pub header: RecordingHeader,
    /// Adapter call events indexed by node_id, sorted by seq
    adapter_events_by_node: BTreeMap<String, Vec<AdapterCallEvent>>,
    /// Metadata call events indexed by caller_id, sorted by seq
    metadata_events_by_caller: BTreeMap<String, Vec<MetadataCallEvent>>,
    /// SAO skip events indexed by node_id
    sao_events: BTreeMap<String, SaoEvent>,
    /// dbt State service clone-decision events indexed by node_id.
    ///
    /// A node can have both a pre-render dev clone and a run-phase clone.
    clone_events: BTreeMap<String, Vec<RunCacheCloneEvent>>,
    /// Run-remote-adhoc events in order
    run_remote_adhoc_events: Vec<RunRemoteAdhocEvent>,
    /// Cache invalidation events in order
    cache_invalidation_events: Vec<CacheInvalidationEvent>,
    /// Current replay position per node for adapter calls (strict mode)
    adapter_positions: RwLock<HashMap<String, usize>>,
    /// Current replay position per caller for metadata calls (strict mode)
    metadata_positions: RwLock<HashMap<String, usize>>,
    /// Current replay position for run_remote_adhoc events
    run_remote_adhoc_position: RwLock<usize>,
    /// Current replay position for cache invalidation events
    cache_invalidation_position: RwLock<usize>,
    /// Semantic mode state: tracks write barriers per node.
    /// Only writes are tracked - reads can be matched any number of times.
    semantic_adapter_state: RwLock<HashMap<String, SemanticReplayState>>,
    /// Semantic mode state for metadata calls
    semantic_metadata_state: RwLock<HashMap<String, SemanticReplayState>>,
    /// Strict mode: seq numbers of read-only execute events already matched globally.
    /// These events are skipped by take_next so they don't consume the wrong write slot.
    ro_exec_matched: RwLock<HashMap<String, HashSet<u32>>>,
}

/// State for semantic replay mode per node/caller.
///
/// In semantic mode:
/// - Writes are barriers that must match in order (tracked via segment_start)
/// - Reads can match any number of times within a segment (NOT tracked)
#[derive(Debug, Clone, Default)]
struct SemanticReplayState {
    /// Index of the first event in the current segment.
    /// A segment is the range of events between two write barriers.
    /// After consuming a write at index N, segment_start becomes N+1.
    segment_start: usize,
    /// Index of the last write barrier we've passed (for debugging/stats)
    last_write_barrier: Option<usize>,
}

impl Recording {
    fn adapter_type(&self) -> AdapterType {
        AdapterType::from_str(&self.header.adapter_type).unwrap_or(AdapterType::Snowflake)
    }

    /// Load a recording from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ReplayError> {
        let path = path.as_ref();

        if !path.exists() {
            return Err(ReplayError::NotFound(path.display().to_string()));
        }

        // Load header
        let header_path = path.join("header.json");
        let header_content = std::fs::read_to_string(&header_path).map_err(|e| {
            ReplayError::InvalidFormat(format!("Failed to read header.json: {}", e))
        })?;
        let header: RecordingHeader = serde_json::from_str(&header_content)?;

        // Load events
        let events_path = path.join("events.ndjson.gz");
        let events = if events_path.exists() {
            load_events_gzipped(&events_path)?
        } else {
            let events_path = path.join("events.ndjson");
            if events_path.exists() {
                load_events_plain(&events_path)?
            } else {
                return Err(ReplayError::InvalidFormat(
                    "No events.ndjson.gz or events.ndjson found".to_string(),
                ));
            }
        };

        // Index events by node_id/caller_id
        let mut adapter_events_by_node: BTreeMap<String, Vec<AdapterCallEvent>> = BTreeMap::new();
        let mut metadata_events_by_caller: BTreeMap<String, Vec<MetadataCallEvent>> =
            BTreeMap::new();
        let mut sao_events: BTreeMap<String, SaoEvent> = BTreeMap::new();
        let mut clone_events: BTreeMap<String, Vec<RunCacheCloneEvent>> = BTreeMap::new();
        let mut run_remote_adhoc_events: Vec<RunRemoteAdhocEvent> = Vec::new();
        let mut cache_invalidation_events: Vec<CacheInvalidationEvent> = Vec::new();

        for event in events {
            match event {
                RecordedEvent::AdapterCall(adapter_event) => {
                    adapter_events_by_node
                        .entry(adapter_event.node_id.clone())
                        .or_default()
                        .push(adapter_event);
                }
                RecordedEvent::MetadataCall(metadata_event) => {
                    metadata_events_by_caller
                        .entry(metadata_event.caller_id.clone())
                        .or_default()
                        .push(metadata_event);
                }
                RecordedEvent::Sao(sao_event) => {
                    // SAO events are keyed by node_id
                    // ASSUMPTION: node_id is unique and 1-1 with SAO events
                    sao_events.insert(sao_event.node_id.clone(), sao_event);
                }
                RecordedEvent::RunRemoteAdhoc(event) => {
                    run_remote_adhoc_events.push(event);
                }
                RecordedEvent::CacheInvalidation(event) => {
                    cache_invalidation_events.push(event);
                }
                RecordedEvent::RunCacheClone(clone_event) => {
                    clone_events
                        .entry(clone_event.node_id.clone())
                        .or_default()
                        .push(clone_event);
                }
            }
        }

        // Sort each node's events by seq
        for events in adapter_events_by_node.values_mut() {
            events.sort_by_key(|e| e.seq);
        }
        for events in metadata_events_by_caller.values_mut() {
            events.sort_by_key(|e| e.seq);
        }

        // Sort run_remote_adhoc events by seq
        run_remote_adhoc_events.sort_by_key(|e| e.seq);

        Ok(Self {
            header,
            adapter_events_by_node,
            metadata_events_by_caller,
            sao_events,
            clone_events,
            run_remote_adhoc_events,
            cache_invalidation_events,
            adapter_positions: RwLock::new(HashMap::new()),
            metadata_positions: RwLock::new(HashMap::new()),
            run_remote_adhoc_position: RwLock::new(0),
            cache_invalidation_position: RwLock::new(0),
            semantic_adapter_state: RwLock::new(HashMap::new()),
            semantic_metadata_state: RwLock::new(HashMap::new()),
            ro_exec_matched: RwLock::new(HashMap::new()),
        })
    }

    // -------------------------------------------------------------------------
    // SAO (State Aware Orchestration) methods
    // -------------------------------------------------------------------------

    /// Present only when the node was skipped due to a cache hit during recording.
    pub fn get_sao_event(&self, node_id: &str) -> Option<&SaoEvent> {
        self.sao_events.get(node_id)
    }

    pub fn has_sao_event(&self, node_id: &str) -> bool {
        self.sao_events.contains_key(node_id)
    }

    pub fn sao_node_ids(&self) -> impl Iterator<Item = &str> {
        self.sao_events.keys().map(|s| s.as_str())
    }

    pub fn total_sao_events(&self) -> usize {
        self.sao_events.len()
    }

    // -------------------------------------------------------------------------
    // dbt State service clone-decision methods
    // -------------------------------------------------------------------------

    /// Present only when the node's execution was satisfied by a clone
    /// decision from the dbt State service during recording.
    pub fn get_run_cache_clone_event(&self, node_id: &str) -> Option<&RunCacheCloneEvent> {
        self.get_run_cache_clone_event_for_phase(node_id, false)
            .or_else(|| self.clone_events.get(node_id)?.first())
    }

    /// Get the recorded clone decision for a node and clone phase.
    pub fn get_run_cache_clone_event_for_phase(
        &self,
        node_id: &str,
        dev_clone: bool,
    ) -> Option<&RunCacheCloneEvent> {
        self.clone_events
            .get(node_id)?
            .iter()
            .find(|event| event.dev_clone == dev_clone)
    }

    // -------------------------------------------------------------------------
    // Adapter call methods
    // -------------------------------------------------------------------------

    /// Get the next recorded adapter event for a node without advancing the position.
    ///
    /// Skips events already consumed as read-only execute reads (ro_exec_matched).
    pub fn peek_next(&self, node_id: &str) -> Option<&AdapterCallEvent> {
        let events = self.adapter_events_by_node.get(node_id)?;
        let ro_matched: HashSet<u32> = {
            let guard = self.ro_exec_matched.read();
            guard.get(node_id).cloned().unwrap_or_default()
        };
        let positions = self.adapter_positions.read();
        let mut scan_pos = positions.get(node_id).copied().unwrap_or(0);
        loop {
            let event = events.get(scan_pos)?;
            scan_pos += 1;
            if !ro_matched.contains(&event.seq) {
                return Some(event);
            }
        }
    }

    /// Get the next recorded adapter event for a node and advance the position.
    ///
    /// Skips events already consumed as read-only execute reads (ro_exec_matched).
    pub fn take_next(&self, node_id: &str) -> Option<&AdapterCallEvent> {
        let events = self.adapter_events_by_node.get(node_id)?;
        let ro_matched: HashSet<u32> = {
            let guard = self.ro_exec_matched.read();
            guard.get(node_id).cloned().unwrap_or_default()
        };
        let mut positions = self.adapter_positions.write();
        let pos = positions.entry(node_id.to_string()).or_insert(0);
        let event = loop {
            let e = events.get(*pos)?;
            *pos += 1;
            if !ro_matched.contains(&e.seq) {
                break e;
            }
        };
        Some(event)
    }

    pub fn events_for_node(&self, node_id: &str) -> Option<&[AdapterCallEvent]> {
        self.adapter_events_by_node
            .get(node_id)
            .map(|v| v.as_slice())
    }

    pub fn node_ids(&self) -> impl Iterator<Item = &str> {
        self.adapter_events_by_node.keys().map(|s| s.as_str())
    }

    /// Find a read-only execute event (SELECT/SHOW SQL) anywhere in this node's recording
    /// and mark it as consumed so it is skipped by strict-mode sequential matching.
    ///
    /// dbt macros such as dbt_utils.date_spine call `run_query("SELECT datediff(...)")`
    /// during Jinja compilation to compute parameters. Python dbt records these after the
    /// query-tag SHOW call, but Fusion emits them earlier (during compilation). The result
    /// is an ordering mismatch that would cause strict mode to consume the wrong event.
    ///
    /// By matching these calls globally (anywhere in the node's event list) and marking
    /// them in `ro_exec_matched`, we let subsequent sequential writes still find their
    /// correct recorded events.
    pub fn take_ro_exec_read(
        &self,
        node_id: &str,
        method: &str,
        args: &serde_json::Value,
    ) -> Option<&AdapterCallEvent> {
        let events = self.adapter_events_by_node.get(node_id)?;
        let adapter_type = self.adapter_type();

        // Snapshot the skip-set without holding the write lock during the linear scan.
        let already_matched: HashSet<u32> = {
            let guard = self.ro_exec_matched.read();
            guard.get(node_id).cloned().unwrap_or_default()
        };

        let event = events.iter().find(|e| {
            !already_matched.contains(&e.seq)
                && is_read_only_execute_event(e)
                && e.method == method
                && adapter_args_match_for_type(method, &e.args, args, adapter_type)
        })?;

        // Mark as consumed so take_next skips it during sequential strict replay.
        self.ro_exec_matched
            .write()
            .entry(node_id.to_string())
            .or_default()
            .insert(event.seq);

        Some(event)
    }

    // -------------------------------------------------------------------------
    // Semantic mode adapter methods
    // -------------------------------------------------------------------------

    /// Find and consume a matching adapter event using semantic rules.
    ///
    /// For Write operations: Must match the next write in sequence (barrier semantics).
    ///     Writes are tracked and consumed - they act as segment barriers.
    ///     Args are verified to ensure we're matching the correct write.
    /// For MetadataRead operations: Can match any read in the current segment with matching args.
    ///     Reads are NOT tracked - the same read can be matched multiple times. If the current
    ///     segment holds no match, falls back to a content match in the region replay already
    ///     passed (see [`Self::find_read_before_segment_untracked`]).
    ///
    /// PRECONDITION: Pure/Cache operations are filtered at the adapter level and should never reach here.
    ///
    /// Returns the matched event if found.
    pub fn take_semantic_match(
        &self,
        node_id: &str,
        method: &str,
        args: &serde_json::Value,
        category: SemanticCategory,
    ) -> Option<&AdapterCallEvent> {
        let events = self.adapter_events_by_node.get(node_id)?;
        let mut state = self.semantic_adapter_state.write();
        let node_state = state.entry(node_id.to_string()).or_default();

        match category {
            SemanticCategory::Write => {
                // Check whether the actual SQL is read-only (SELECT, SHOW, ...).
                // dbt macros like dbt_utils.date_spine call execute/run_query with a
                // SELECT during Jinja compilation. These are semantically reads even
                // though the method is classified as Write. Treat them as untracked
                // reads so ordering differences don't cause false-positive mismatches.
                if is_read_only_execute_call(method, args) {
                    return self.find_read_in_segment_untracked(events, node_state, method, args);
                }
                // Real writes are barriers - must match the next write in sequence.
                // Writes ARE tracked and consumed.
                self.find_next_write_in_segment(events, node_state, method)
            }
            SemanticCategory::MetadataRead => {
                // Reads can match any read in the current segment with matching args.
                // Reads are NOT tracked - same read can be matched 0 or more times.
                self.find_read_in_segment_untracked(events, node_state, method, args)
                    .or_else(|| {
                        self.find_read_before_segment_untracked(
                            events, node_state, method, args, node_id,
                        )
                    })
            }
            SemanticCategory::Pure | SemanticCategory::Cache => {
                unreachable!()
            }
        }
    }

    /// Find the next write operation in sequence (barrier semantics).
    ///
    /// Writes are tracked and consumed. They act as segment barriers.
    /// Matching is by method name only - the sequence order provides correctness.
    /// SQL validation happens separately via `validate_replay()`, but the args
    /// must still match here before advancing the replay state. Otherwise an
    /// unrecorded write can consume the next recorded write and turn the real
    /// error into a misleading SQL mismatch on a later call.
    fn find_next_write_in_segment<'a>(
        &'a self,
        events: &'a [AdapterCallEvent],
        state: &mut SemanticReplayState,
        method: &str,
    ) -> Option<&'a AdapterCallEvent> {
        let search_start = state.segment_start;

        // Find the first write from segment_start onwards
        for (idx, event) in events[search_start..].iter().enumerate() {
            // Skip non-write operations and read-only execute events (SELECT/SHOW SQL).
            // Read-only executes are not barriers; they belong to the read search space.
            if !event.semantic_category.is_mutating() || is_read_only_execute_event(event) {
                continue;
            }

            // Found a write - check method name matches
            if event.method != method {
                // Write mismatch (method) - sequencing error
                return None;
            }

            // Match found by method name. SQL content is validated separately by
            // `validate_event`, which uses a more permissive sanitizer pipeline
            // (strips comments, query tags, UUIDs, ...) than the comparison here
            // would. Gating consumption on SQL equality duplicated that check with
            // stricter rules, so legitimate writes whose SQL contains tolerable
            // dynamic content (e.g. audit-log timestamps) failed to match at all
            // instead of surfacing as a normal SQL-mismatch validation error.

            // Advance the segment past this write
            let abs_idx = search_start + idx;
            state.segment_start = abs_idx + 1;
            state.last_write_barrier = Some(abs_idx);
            return Some(event);
        }

        None
    }

    /// Find a matching read operation within the current segment (untracked).
    ///
    /// In semantic mode, reads are NOT tracked for consumption. The same read
    /// can be matched multiple times. This reflects that reads are idempotent
    /// and the replay code may call them more or fewer times than recorded.
    ///
    /// Matching is done on both method name AND arguments to ensure we return
    /// the correct result for the specific call. SQL strings within the args
    /// are compared using fuzzy SQL matching to tolerate formatting differences.
    fn find_read_in_segment_untracked<'a>(
        &'a self,
        events: &'a [AdapterCallEvent],
        state: &SemanticReplayState,
        method: &str,
        args: &serde_json::Value,
    ) -> Option<&'a AdapterCallEvent> {
        let search_start = state.segment_start;

        // Determine segment end: the next real write barrier.
        // Read-only execute events (SELECT/SHOW SQL) are NOT barriers even though
        // their recorded semantic_category is Write; they live in the read search space.
        let segment_end = events[search_start..]
            .iter()
            .position(|e| e.semantic_category.is_mutating() && !is_read_only_execute_event(e))
            .map(|pos| search_start + pos)
            .unwrap_or(events.len());

        // Search within the segment for a matching read (method + args).
        // Use the recording's adapter type for SQL comparison.
        let adapter_type = self.adapter_type();
        events[search_start..segment_end].iter().find(|event| {
            event.method == method
                && adapter_args_match_for_type(method, &event.args, args, adapter_type)
        })
    }

    /// Last resort for a read the current segment cannot answer: find a content match in the
    /// region replay has *already passed*.
    ///
    /// Segment bounds exist to keep write ordering honest. Looking **forward** past the next
    /// barrier would defeat that — a read answered from a future segment hides the fact that the
    /// replayer never performed the write in between (see
    /// `test_semantic_mode_read_does_not_advance_over_skipped_write`, and
    /// `test_semantic_mode_read_not_in_segment_fails`). So this only ever looks **backward**,
    /// before `segment_start`: every write in that region has already been matched in sequence,
    /// so nothing can be hidden by reusing a read from it, and reads are explicitly idempotent
    /// and multi-matchable to begin with.
    ///
    /// Backward misses are real and not the replayer's fault. Whether a read reaches the adapter
    /// at all depends on cache state, and cache state is not recorded: when a relation's columns
    /// are already hydrated in the recorded run, the read is recorded at whatever earlier point
    /// did hydrate it, and a replay with a differently-warmed cache asks for it a segment or two
    /// later. That fails even when replaying a recording against the very version that produced
    /// it — which is how it shows up in nightly, as `get_columns_in_relation` "No matching read
    /// found in current segment" on a snapshot that its own recorded version cannot replay.
    ///
    /// The metadata replay path has carried an equivalent last-resort search for a while (see
    /// `find_metadata_read_across_all_callers`, "non-determinism in which node facilitates
    /// hydration of shared resources such as the schema cache"); adapter reads never got one.
    ///
    /// Deliberately narrow beyond the backward-only bound:
    /// - reads only — write barriers keep their strict in-sequence matching
    /// - the same `method` + args predicate used inside the segment, so this widens *where* a
    ///   read may be found, never *what* counts as an answer to it
    /// - real writes are skipped, so a read is never answered by a mutating event
    /// - this node's own stream only; a read recorded under a different node is still a miss
    ///
    /// Logs on success: recorded and replayed call order diverged, which is worth seeing even
    /// though it is not fatal.
    fn find_read_before_segment_untracked<'a>(
        &'a self,
        events: &'a [AdapterCallEvent],
        state: &SemanticReplayState,
        method: &str,
        args: &serde_json::Value,
        node_id: &str,
    ) -> Option<&'a AdapterCallEvent> {
        let adapter_type = self.adapter_type();
        let already_passed = events.get(..state.segment_start)?;
        let event = already_passed.iter().find(|event| {
            let is_real_write =
                event.semantic_category.is_mutating() && !is_read_only_execute_event(event);
            !is_real_write
                && event.method == method
                && adapter_args_match_for_type(method, &event.args, args, adapter_type)
        })?;

        tracing::debug!(
            node_id,
            method,
            seq = event.seq,
            segment_start = state.segment_start,
            "Replay: answered read from an already-passed recorded segment"
        );

        Some(event)
    }

    /// Peek at the next event in semantic mode without consuming it.
    ///
    /// For reads: Returns the first matching event in the current segment
    /// For writes: Returns the next write barrier
    pub fn peek_semantic(
        &self,
        node_id: &str,
        category: SemanticCategory,
    ) -> Option<&AdapterCallEvent> {
        let events = self.adapter_events_by_node.get(node_id)?;
        let state = self.semantic_adapter_state.read();
        let node_state = state.get(node_id).cloned().unwrap_or_default();

        let search_start = node_state.segment_start;

        match category {
            SemanticCategory::Write => {
                // Find the next write (writes ARE tracked)
                events[search_start..]
                    .iter()
                    .find(|e| e.semantic_category.is_mutating())
            }
            SemanticCategory::MetadataRead => {
                // Find first read in segment (reads are NOT tracked)
                let segment_end = events[search_start..]
                    .iter()
                    .position(|e| e.semantic_category.is_mutating())
                    .map(|pos| search_start + pos)
                    .unwrap_or(events.len());

                events[search_start..segment_end].first()
            }
            SemanticCategory::Pure | SemanticCategory::Cache => {
                // Already handled above
                unreachable!()
            }
        }
    }

    /// Check if the current segment has any reads available.
    ///
    /// Note: In semantic mode, reads are not tracked for consumption,
    /// so this just checks if any reads exist in the current segment.
    pub fn has_reads_in_segment(&self, node_id: &str) -> bool {
        let Some(events) = self.adapter_events_by_node.get(node_id) else {
            return false;
        };
        let state = self.semantic_adapter_state.read();
        let node_state = state.get(node_id).cloned().unwrap_or_default();

        let search_start = node_state.segment_start;
        let segment_end = events[search_start..]
            .iter()
            .position(|e| e.semantic_category.is_mutating())
            .map(|pos| search_start + pos)
            .unwrap_or(events.len());

        events[search_start..segment_end]
            .iter()
            .any(|e| !e.semantic_category.is_mutating())
    }

    pub fn has_pending_write(&self, node_id: &str) -> bool {
        let Some(events) = self.adapter_events_by_node.get(node_id) else {
            return false;
        };
        let state = self.semantic_adapter_state.read();
        let node_state = state.get(node_id).cloned().unwrap_or_default();

        events[node_state.segment_start..]
            .iter()
            .any(|e| e.semantic_category.is_mutating())
    }

    // -------------------------------------------------------------------------
    // Semantic mode metadata methods
    // -------------------------------------------------------------------------

    /// Find and consume a matching metadata event using semantic rules.
    ///
    /// Same semantics as adapter calls:
    /// - Writes are tracked and must match in order (barriers)
    /// - Reads are NOT tracked and can match 0 or more times
    ///
    /// Args are matched to ensure we return the correct result.
    pub fn take_semantic_metadata_match(
        &self,
        caller_id: &str,
        method: &str,
        args: &MetadataCallArgs,
        category: SemanticCategory,
    ) -> Option<&MetadataCallEvent> {
        let events = self.metadata_events_by_caller.get(caller_id)?;
        let mut state = self.semantic_metadata_state.write();
        let caller_state = state.entry(caller_id.to_string()).or_default();

        match category {
            SemanticCategory::Write => {
                // Writes are barriers - tracked and consumed
                self.find_next_metadata_write(events, caller_state, method, args)
            }
            SemanticCategory::MetadataRead => {
                // Reads can match any read in segment with matching args - NOT tracked
                self.find_metadata_read_in_segment_untracked(events, caller_state, method, args)
            }
            SemanticCategory::Pure | SemanticCategory::Cache => {
                unreachable!()
            }
        }
    }

    /// Find the next metadata write operation in sequence (tracked).
    fn find_next_metadata_write<'a>(
        &'a self,
        events: &'a [MetadataCallEvent],
        state: &mut SemanticReplayState,
        method: &str,
        args: &MetadataCallArgs,
    ) -> Option<&'a MetadataCallEvent> {
        let search_start = state.segment_start;

        // Find the first write from segment_start onwards
        for (idx, event) in events[search_start..].iter().enumerate() {
            if !event.semantic_category.is_mutating() {
                continue;
            }

            // Found a write - method and args must match
            if event.method == method && metadata_args_match(&event.args, args) {
                let abs_idx = search_start + idx;
                state.segment_start = abs_idx + 1; // Advance past this write
                state.last_write_barrier = Some(abs_idx);
                return Some(event);
            } else {
                // Write mismatch (method or args) - sequencing error
                return None;
            }
        }

        None
    }

    /// Find a matching metadata read within the current segment (untracked).
    fn find_metadata_read_in_segment_untracked<'a>(
        &'a self,
        events: &'a [MetadataCallEvent],
        state: &SemanticReplayState,
        method: &str,
        args: &MetadataCallArgs,
    ) -> Option<&'a MetadataCallEvent> {
        let search_start = state.segment_start;

        let segment_end = events[search_start..]
            .iter()
            .position(|e| e.semantic_category.is_mutating())
            .map(|pos| search_start + pos)
            .unwrap_or(events.len());

        // Search for matching read (method + args) - no consumption tracking
        events[search_start..segment_end]
            .iter()
            .find(|event| event.method == method && metadata_args_match(&event.args, args))
    }

    // -------------------------------------------------------------------------
    // Metadata call methods (strict mode)
    // -------------------------------------------------------------------------

    pub fn has_metadata_events_for_caller(&self, caller_id: &str) -> bool {
        self.metadata_events_by_caller.contains_key(caller_id)
    }

    /// Find a matching metadata read across ALL callers using superset matching
    /// within their current segment.
    /// This cross-node check is necessary as there may be non-determinism in
    /// which node facilitates hydration of shared resources such as the schema cache.
    pub fn find_metadata_read_across_all_callers(
        &self,
        method: &str,
        args: &MetadataCallArgs,
    ) -> Option<&MetadataCallEvent> {
        let state = self.semantic_metadata_state.read();

        for (caller_id, events) in &self.metadata_events_by_caller {
            // Get segment start for this caller (0 if not yet visited)
            let segment_start = state.get(caller_id).map(|s| s.segment_start).unwrap_or(0);

            // Find segment end (next write or end of events)
            let segment_end = events[segment_start..]
                .iter()
                .position(|e| e.semantic_category.is_mutating())
                .map(|pos| segment_start + pos)
                .unwrap_or(events.len());

            // Search within this caller's current segment
            if let Some(event) = events[segment_start..segment_end]
                .iter()
                .find(|e| e.method == method && metadata_args_match(&e.args, args))
            {
                return Some(event);
            }
        }
        None
    }

    /// Get the next recorded metadata event for a caller without advancing the position.
    pub fn peek_next_metadata(&self, caller_id: &str) -> Option<&MetadataCallEvent> {
        let positions = self.metadata_positions.read();
        let pos = positions.get(caller_id).copied().unwrap_or(0);
        self.metadata_events_by_caller.get(caller_id)?.get(pos)
    }

    /// Get the next recorded metadata event for a caller and advance the position.
    pub fn take_next_metadata(&self, caller_id: &str) -> Option<&MetadataCallEvent> {
        let events = self.metadata_events_by_caller.get(caller_id)?;
        let mut positions = self.metadata_positions.write();
        let pos = positions.entry(caller_id.to_string()).or_insert(0);
        let event = events.get(*pos)?;
        *pos += 1;
        Some(event)
    }

    pub fn metadata_events_for_caller(&self, caller_id: &str) -> Option<&[MetadataCallEvent]> {
        self.metadata_events_by_caller
            .get(caller_id)
            .map(|v| v.as_slice())
    }

    pub fn metadata_caller_ids(&self) -> impl Iterator<Item = &str> {
        self.metadata_events_by_caller.keys().map(|s| s.as_str())
    }

    // -------------------------------------------------------------------------
    // Statistics and reset
    // -------------------------------------------------------------------------

    /// Get the total number of events (adapter, metadata, SAO, run-cache clone,
    /// and run_remote_adhoc).
    pub fn total_events(&self) -> usize {
        self.adapter_events_by_node
            .values()
            .map(|v| v.len())
            .sum::<usize>()
            + self
                .metadata_events_by_caller
                .values()
                .map(|v| v.len())
                .sum::<usize>()
            + self.sao_events.len()
            + self.clone_events.len()
            + self.run_remote_adhoc_events.len()
    }

    pub fn total_adapter_events(&self) -> usize {
        self.adapter_events_by_node.values().map(|v| v.len()).sum()
    }

    pub fn total_metadata_events(&self) -> usize {
        self.metadata_events_by_caller
            .values()
            .map(|v| v.len())
            .sum()
    }

    pub fn total_run_remote_adhoc_events(&self) -> usize {
        self.run_remote_adhoc_events.len()
    }

    /// Get the next run_remote_adhoc event, advancing the position.
    pub fn take_next_run_remote_adhoc(&self) -> Option<&RunRemoteAdhocEvent> {
        let mut pos = self.run_remote_adhoc_position.write();
        let event = self.run_remote_adhoc_events.get(*pos)?;
        *pos += 1;
        Some(event)
    }

    /// Get the next cache invalidation event, advancing the position.
    pub fn take_next_cache_invalidation(&self) -> Option<&CacheInvalidationEvent> {
        let mut pos = self.cache_invalidation_position.write();
        let event = self.cache_invalidation_events.get(*pos)?;
        *pos += 1;
        Some(event)
    }

    /// Reset replay positions for all nodes and callers.
    pub fn reset(&self) {
        self.adapter_positions.write().clear();
        self.metadata_positions.write().clear();
        *self.run_remote_adhoc_position.write() = 0;
        *self.cache_invalidation_position.write() = 0;
        self.semantic_adapter_state.write().clear();
        self.semantic_metadata_state.write().clear();
        self.ro_exec_matched.write().clear();
    }

    /// Reset replay position for a specific node.
    pub fn reset_node(&self, node_id: &str) {
        self.adapter_positions.write().remove(node_id);
        self.semantic_adapter_state.write().remove(node_id);
        self.ro_exec_matched.write().remove(node_id);
    }

    /// Reset replay position for a specific metadata caller.
    pub fn reset_metadata_caller(&self, caller_id: &str) {
        self.metadata_positions.write().remove(caller_id);
        self.semantic_metadata_state.write().remove(caller_id);
    }
}

/// Load events from a gzipped NDJSON file.
fn load_events_gzipped(path: &Path) -> Result<Vec<RecordedEvent>, ReplayError> {
    let file = std::fs::File::open(path)?;
    let decoder = GzDecoder::new(file);
    let reader = BufReader::new(decoder);
    load_events_from_reader(reader)
}

/// Load events from a plain NDJSON file.
fn load_events_plain(path: &Path) -> Result<Vec<RecordedEvent>, ReplayError> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    load_events_from_reader(reader)
}

/// Load events from a reader.
fn load_events_from_reader<R: BufRead>(reader: R) -> Result<Vec<RecordedEvent>, ReplayError> {
    let mut events = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let event: RecordedEvent = serde_json::from_str(&line)?;
        events.push(event);
    }
    Ok(events)
}

// -----------------------------------------------------------------------------
// Replay validation
// -----------------------------------------------------------------------------

/// Result of replaying an event.
#[derive(Debug)]
pub struct ReplayResult {
    /// The recorded event that was matched
    pub recorded: AdapterCallEvent,
    /// Whether the replay matched the recording
    pub matched: bool,
    /// Differences found (if any)
    pub differences: Vec<ReplayDifference>,
}

/// A difference found during replay.
#[derive(Debug)]
pub enum ReplayDifference {
    MethodMismatch {
        expected: String,
        actual: String,
    },
    ArgCountMismatch {
        expected: usize,
        actual: usize,
    },
    ArgValueMismatch {
        index: usize,
        expected: serde_json::Value,
        actual: serde_json::Value,
    },
    SqlMismatch {
        expected: String,
        actual: String,
    },
}

impl std::fmt::Display for ReplayDifference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayDifference::MethodMismatch { expected, actual } => {
                write!(
                    f,
                    "Method mismatch: expected '{}', got '{}'",
                    expected, actual
                )
            }
            ReplayDifference::ArgCountMismatch { expected, actual } => {
                write!(
                    f,
                    "Arg count mismatch: expected {}, got {}",
                    expected, actual
                )
            }
            ReplayDifference::ArgValueMismatch {
                index,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "Arg[{}] mismatch:\n  expected: {}\n  actual: {}",
                    index, expected, actual
                )
            }
            ReplayDifference::SqlMismatch { expected, actual } => {
                writeln!(f, "SQL mismatch:")?;
                let diff = TextDiff::from_lines(expected, actual);
                for change in diff.iter_all_changes() {
                    let sign = match change.tag() {
                        ChangeTag::Delete => "-",
                        ChangeTag::Insert => "+",
                        ChangeTag::Equal => " ",
                    };
                    write!(f, "{}{}", sign, change)?;
                }
                Ok(())
            }
        }
    }
}

/// Validate a replay call against a recorded event.
pub fn validate_replay(
    recorded: &AdapterCallEvent,
    method: &str,
    args: &serde_json::Value,
    adapter_type: AdapterType,
) -> ReplayResult {
    let mut differences = Vec::new();

    if recorded.method != method {
        differences.push(ReplayDifference::MethodMismatch {
            expected: recorded.method.clone(),
            actual: method.to_string(),
        });
    }

    // For SQL methods, validate the SQL string
    if is_sql_method(method) {
        // Extract SQL from first arg
        let recorded_sql = extract_sql_from_args(&recorded.args);
        let actual_sql = extract_sql_from_args(args);

        match (recorded_sql, actual_sql) {
            (Some(exp_sql), Some(act_sql)) => {
                match compare_sql(exp_sql, act_sql, adapter_type) {
                    Ok(()) => {
                        // SQL is semantically equivalent, no difference to report
                    }
                    Err(_) => {
                        differences.push(ReplayDifference::SqlMismatch {
                            expected: exp_sql.to_string(),
                            actual: act_sql.to_string(),
                        });
                    }
                }
            }
            (None, None) => {}
            _ => {
                differences.push(ReplayDifference::ArgValueMismatch {
                    index: 0,
                    expected: recorded.args.clone(),
                    actual: args.clone(),
                });
            }
        }
    } else if let (Some(recorded_args), Some(actual_args)) =
        (recorded.args.as_array(), args.as_array())
    {
        // Non-SQL methods: validate all args
        if recorded_args.len() != actual_args.len() {
            differences.push(ReplayDifference::ArgCountMismatch {
                expected: recorded_args.len(),
                actual: actual_args.len(),
            });
        } else {
            if !adapter_args_match_for_type(method, &recorded.args, args, adapter_type) {
                differences.push(ReplayDifference::ArgValueMismatch {
                    index: 0,
                    expected: recorded.args.clone(),
                    actual: args.clone(),
                });
            }
        }
    }

    ReplayResult {
        recorded: recorded.clone(),
        matched: differences.is_empty(),
        differences,
    }
}

// Tests for serde functionality have been moved to the `serde` module.
// See `super::serde::tests` for json_to_value and values_match tests.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::diff::compare_sql;

    /// Compare SQL args for execute/run_query methods.
    fn sql_args_match(
        recorded: &serde_json::Value,
        actual: &serde_json::Value,
        adapter_type: AdapterType,
    ) -> bool {
        let recorded_sql = extract_sql_from_args(recorded);
        let actual_sql = extract_sql_from_args(actual);

        match (recorded_sql, actual_sql) {
            (Some(r), Some(a)) => compare_sql(r, a, adapter_type).is_ok(),
            (None, None) => true,
            _ => false,
        }
    }

    /// Helper to create a test AdapterCallEvent
    fn make_event(
        node_id: &str,
        seq: u32,
        method: &str,
        category: SemanticCategory,
    ) -> AdapterCallEvent {
        AdapterCallEvent {
            node_id: node_id.to_string(),
            seq,
            method: method.to_string(),
            semantic_category: category,
            args: serde_json::json!([]),
            result: serde_json::json!(null),
            success: true,
            error: None,
            timestamp_ns: seq as u64 * 1000,
        }
    }

    /// Helper to get empty args for test calls
    fn empty_args() -> serde_json::Value {
        serde_json::json!([])
    }

    /// Helper to create a test Recording from events
    fn make_recording(events: Vec<AdapterCallEvent>) -> Recording {
        let mut adapter_events_by_node: BTreeMap<String, Vec<AdapterCallEvent>> = BTreeMap::new();
        for event in events {
            adapter_events_by_node
                .entry(event.node_id.clone())
                .or_default()
                .push(event);
        }
        for events in adapter_events_by_node.values_mut() {
            events.sort_by_key(|e| e.seq);
        }

        Recording {
            header: RecordingHeader {
                format_version: 1,
                fusion_version: "test".to_string(),
                adapter_type: "snowflake".to_string(),
                recorded_at: "2024-01-01T00:00:00Z".to_string(),
                invocation_id: "test-123".to_string(),
                invocation_command: None,
                metadata: serde_json::Map::new(),
            },
            adapter_events_by_node,
            metadata_events_by_caller: BTreeMap::new(),
            sao_events: BTreeMap::new(),
            clone_events: BTreeMap::new(),
            run_remote_adhoc_events: Vec::new(),
            cache_invalidation_events: Vec::new(),
            adapter_positions: RwLock::new(HashMap::new()),
            metadata_positions: RwLock::new(HashMap::new()),
            run_remote_adhoc_position: RwLock::new(0),
            cache_invalidation_position: RwLock::new(0),
            semantic_adapter_state: RwLock::new(HashMap::new()),
            semantic_metadata_state: RwLock::new(HashMap::new()),
            ro_exec_matched: RwLock::new(HashMap::new()),
        }
    }

    #[test]
    fn test_replay_mode_default_is_strict() {
        assert_eq!(ReplayMode::default(), ReplayMode::Strict);
    }

    #[test]
    fn test_replay_mode_display() {
        assert_eq!(format!("{}", ReplayMode::Strict), "strict");
        assert_eq!(format!("{}", ReplayMode::Semantic), "semantic");
    }

    #[test]
    fn test_replay_mode_serialization() {
        let strict = ReplayMode::Strict;
        let json = serde_json::to_string(&strict).unwrap();
        assert_eq!(json, "\"strict\"");

        let semantic = ReplayMode::Semantic;
        let json = serde_json::to_string(&semantic).unwrap();
        assert_eq!(json, "\"semantic\"");

        let parsed: ReplayMode = serde_json::from_str("\"semantic\"").unwrap();
        assert_eq!(parsed, ReplayMode::Semantic);
    }

    #[test]
    fn test_strict_mode_sequential_matching() {
        // In strict mode, events must match in exact sequence
        let events = vec![
            make_event("node1", 0, "get_relation", SemanticCategory::MetadataRead),
            make_event(
                "node1",
                1,
                "get_columns_in_relation",
                SemanticCategory::MetadataRead,
            ),
            make_event("node1", 2, "execute", SemanticCategory::Write),
        ];
        let recording = make_recording(events);

        // First call should match first event
        let event = recording.take_next("node1").unwrap();
        assert_eq!(event.method, "get_relation");

        // Second call should match second event
        let event = recording.take_next("node1").unwrap();
        assert_eq!(event.method, "get_columns_in_relation");

        // Third call should match third event
        let event = recording.take_next("node1").unwrap();
        assert_eq!(event.method, "execute");

        // No more events
        assert!(recording.take_next("node1").is_none());
    }

    #[test]
    fn test_semantic_mode_reads_can_match_out_of_order() {
        // In semantic mode, reads within a segment can match in any order
        let events = vec![
            make_event("node1", 0, "get_relation", SemanticCategory::MetadataRead),
            make_event(
                "node1",
                1,
                "get_columns_in_relation",
                SemanticCategory::MetadataRead,
            ),
            make_event("node1", 2, "list_schemas", SemanticCategory::MetadataRead),
            make_event("node1", 3, "execute", SemanticCategory::Write),
        ];
        let recording = make_recording(events);

        // Request in different order than recorded
        // First, get_columns_in_relation (recorded as seq 1)
        let event = recording
            .take_semantic_match(
                "node1",
                "get_columns_in_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        assert_eq!(event.method, "get_columns_in_relation");

        // Second, list_schemas (recorded as seq 2)
        let event = recording
            .take_semantic_match(
                "node1",
                "list_schemas",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        assert_eq!(event.method, "list_schemas");

        // Third, get_relation (recorded as seq 0)
        let event = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        assert_eq!(event.method, "get_relation");

        // Now the write barrier - must still be next
        let event = recording
            .take_semantic_match("node1", "execute", &empty_args(), SemanticCategory::Write)
            .unwrap();
        assert_eq!(event.method, "execute");
    }

    /// A read recorded before a write barrier must still be answerable after replay has crossed
    /// that barrier. Cache state decides whether a read reaches the adapter at all, and cache
    /// state isn't recorded, so recorded and replayed call order legitimately differ here — this
    /// is the shape that fails even when replaying a recording against the version that produced
    /// it (nightly's `get_columns_in_relation` "No matching read found in current segment").
    #[test]
    fn test_semantic_mode_read_recorded_before_a_barrier_is_still_matched() {
        let events = vec![
            make_event(
                "node1",
                0,
                "get_columns_in_relation",
                SemanticCategory::MetadataRead,
            ),
            make_event("node1", 1, "execute", SemanticCategory::Write),
        ];
        let recording = make_recording(events);

        // Cross the write barrier first, leaving the recorded read behind the cursor.
        assert_eq!(
            recording
                .take_semantic_match("node1", "execute", &empty_args(), SemanticCategory::Write)
                .unwrap()
                .seq,
            1
        );

        // The read is now outside the current segment, but the recording does hold the answer.
        assert_eq!(
            recording
                .take_semantic_match(
                    "node1",
                    "get_columns_in_relation",
                    &empty_args(),
                    SemanticCategory::MetadataRead,
                )
                .unwrap()
                .seq,
            0
        );
    }

    /// The fallback widens *where* a read may be found, never *what* answers it: a read with no
    /// content match anywhere in the node's stream still misses.
    #[test]
    fn test_semantic_mode_unrecorded_read_still_misses() {
        let events = vec![make_event(
            "node1",
            0,
            "get_columns_in_relation",
            SemanticCategory::MetadataRead,
        )];
        let recording = make_recording(events);

        assert!(
            recording
                .take_semantic_match(
                    "node1",
                    "list_schemas",
                    &empty_args(),
                    SemanticCategory::MetadataRead,
                )
                .is_none()
        );
    }

    /// The fallback must never hand a read a mutating event, even when the methods match — an
    /// `execute` that really wrote is not an answer to an `execute` read probe.
    #[test]
    fn test_semantic_mode_read_fallback_skips_real_writes() {
        let events = vec![
            make_event("node1", 0, "execute", SemanticCategory::Write),
            make_event("node1", 1, "drop_relation", SemanticCategory::Write),
        ];
        let recording = make_recording(events);

        // Cross both barriers so nothing is left in the current segment.
        recording
            .take_semantic_match("node1", "execute", &empty_args(), SemanticCategory::Write)
            .unwrap();
        recording
            .take_semantic_match(
                "node1",
                "drop_relation",
                &empty_args(),
                SemanticCategory::Write,
            )
            .unwrap();

        assert!(
            recording
                .take_semantic_match(
                    "node1",
                    "execute",
                    &empty_args(),
                    SemanticCategory::MetadataRead,
                )
                .is_none()
        );
    }

    /// The backward fallback must not become a forward one. With replay mid-stream (so the
    /// already-passed region is non-empty), a read that exists only in a *future* segment still
    /// misses — answering it would hide the write barrier in between that replay hasn't reached,
    /// which is the invariant `test_semantic_mode_read_does_not_advance_over_skipped_write`
    /// exists to protect.
    #[test]
    fn test_semantic_mode_read_fallback_never_looks_forward() {
        let events = vec![
            make_event("node1", 0, "get_relation", SemanticCategory::MetadataRead),
            make_event("node1", 1, "execute", SemanticCategory::Write),
            make_event("node1", 2, "list_schemas", SemanticCategory::MetadataRead),
            make_event("node1", 3, "drop_relation", SemanticCategory::Write),
            make_event(
                "node1",
                4,
                "get_columns_in_relation",
                SemanticCategory::MetadataRead,
            ),
        ];
        let recording = make_recording(events);

        // Cross the first barrier, so segment_start is 2 and the region behind it is non-empty.
        recording
            .take_semantic_match("node1", "execute", &empty_args(), SemanticCategory::Write)
            .unwrap();

        // `get_columns_in_relation` (seq 4) sits beyond the *next* barrier (seq 3): still a miss.
        assert!(
            recording
                .take_semantic_match(
                    "node1",
                    "get_columns_in_relation",
                    &empty_args(),
                    SemanticCategory::MetadataRead,
                )
                .is_none()
        );

        // The read behind the cursor is still reachable, and the one in the current segment too.
        assert_eq!(
            recording
                .take_semantic_match(
                    "node1",
                    "get_relation",
                    &empty_args(),
                    SemanticCategory::MetadataRead,
                )
                .unwrap()
                .seq,
            0
        );
        assert_eq!(
            recording
                .take_semantic_match(
                    "node1",
                    "list_schemas",
                    &empty_args(),
                    SemanticCategory::MetadataRead,
                )
                .unwrap()
                .seq,
            2
        );
    }

    /// Reads stay scoped to their own node: one recorded under a different node is still a miss.
    #[test]
    fn test_semantic_mode_read_fallback_does_not_cross_nodes() {
        let events = vec![make_event(
            "node1",
            0,
            "get_columns_in_relation",
            SemanticCategory::MetadataRead,
        )];
        let recording = make_recording(events);

        assert!(
            recording
                .take_semantic_match(
                    "node2",
                    "get_columns_in_relation",
                    &empty_args(),
                    SemanticCategory::MetadataRead,
                )
                .is_none()
        );
    }

    #[test]
    fn test_semantic_mode_writes_are_barriers() {
        // Writes must match in sequence order - they act as barriers
        let events = vec![
            make_event("node1", 0, "get_relation", SemanticCategory::MetadataRead),
            make_event("node1", 1, "execute", SemanticCategory::Write),
            make_event("node1", 2, "get_relation", SemanticCategory::MetadataRead),
            make_event("node1", 3, "drop_relation", SemanticCategory::Write),
        ];
        let recording = make_recording(events);

        // Consume the read in first segment
        let event = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        assert_eq!(event.seq, 0);

        // Now we must match the write barrier
        let event = recording
            .take_semantic_match("node1", "execute", &empty_args(), SemanticCategory::Write)
            .unwrap();
        assert_eq!(event.seq, 1);

        // Now we're in a new segment - can match the second get_relation
        let event = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        assert_eq!(event.seq, 2);

        // Match the second write barrier
        let event = recording
            .take_semantic_match(
                "node1",
                "drop_relation",
                &empty_args(),
                SemanticCategory::Write,
            )
            .unwrap();
        assert_eq!(event.seq, 3);
    }

    #[test]
    fn test_semantic_mode_wrong_write_order_fails() {
        // Trying to match writes out of order should fail
        let events = vec![
            make_event("node1", 0, "execute", SemanticCategory::Write),
            make_event("node1", 1, "drop_relation", SemanticCategory::Write),
        ];
        let recording = make_recording(events);

        // Try to match drop_relation first (but execute is the next write barrier)
        let result = recording.take_semantic_match(
            "node1",
            "drop_relation",
            &empty_args(),
            SemanticCategory::Write,
        );
        assert!(result.is_none(), "Should not match wrong write order");
    }

    #[test]
    fn test_semantic_mode_read_not_in_segment_fails() {
        // Trying to match a read that's past the current segment boundary should fail
        let events = vec![
            make_event("node1", 0, "get_relation", SemanticCategory::MetadataRead),
            make_event("node1", 1, "execute", SemanticCategory::Write),
            make_event("node1", 2, "list_schemas", SemanticCategory::MetadataRead),
        ];
        let recording = make_recording(events);

        // Try to match list_schemas (which is after the write barrier in segment 2)
        let result = recording.take_semantic_match(
            "node1",
            "list_schemas",
            &empty_args(),
            SemanticCategory::MetadataRead,
        );
        assert!(
            result.is_none(),
            "Should not match read from future segment"
        );

        // But we can match get_relation from current segment
        let event = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        assert_eq!(event.seq, 0);
    }

    #[test]
    fn test_semantic_mode_multiple_segments() {
        // Test behavior across multiple segments
        let events = vec![
            // Segment 1
            make_event("node1", 0, "get_relation", SemanticCategory::MetadataRead),
            make_event(
                "node1",
                1,
                "get_columns_in_relation",
                SemanticCategory::MetadataRead,
            ),
            make_event("node1", 2, "execute", SemanticCategory::Write),
            // Segment 2
            make_event("node1", 3, "list_schemas", SemanticCategory::MetadataRead),
            make_event(
                "node1",
                4,
                "check_schema_exists",
                SemanticCategory::MetadataRead,
            ),
            make_event("node1", 5, "create_schema", SemanticCategory::Write),
            // Segment 3
            make_event("node1", 6, "get_relation", SemanticCategory::MetadataRead),
        ];
        let recording = make_recording(events);

        // Segment 1: Match reads in reverse order
        let _ = recording
            .take_semantic_match(
                "node1",
                "get_columns_in_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        let _ = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        let _ = recording
            .take_semantic_match("node1", "execute", &empty_args(), SemanticCategory::Write)
            .unwrap();

        // Segment 2: Match reads in reverse order
        let _ = recording
            .take_semantic_match(
                "node1",
                "check_schema_exists",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        let _ = recording
            .take_semantic_match(
                "node1",
                "list_schemas",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        let _ = recording
            .take_semantic_match(
                "node1",
                "create_schema",
                &empty_args(),
                SemanticCategory::Write,
            )
            .unwrap();

        // Segment 3 (no write barrier at end)
        let event = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        assert_eq!(event.seq, 6);
    }

    #[test]
    fn test_semantic_mode_peek_does_not_consume() {
        let events = vec![
            make_event("node1", 0, "get_relation", SemanticCategory::MetadataRead),
            make_event("node1", 1, "execute", SemanticCategory::Write),
        ];
        let recording = make_recording(events);

        // Peek multiple times
        let event1 = recording.peek_semantic("node1", SemanticCategory::MetadataRead);
        let event2 = recording.peek_semantic("node1", SemanticCategory::MetadataRead);

        assert!(event1.is_some());
        assert!(event2.is_some());
        assert_eq!(event1.unwrap().method, event2.unwrap().method);

        // Now actually consume it
        let consumed = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        assert_eq!(consumed.method, "get_relation");
    }

    #[test]
    fn test_reset_clears_semantic_state() {
        let events = vec![
            make_event("node1", 0, "get_relation", SemanticCategory::MetadataRead),
            make_event("node1", 1, "execute", SemanticCategory::Write),
        ];
        let recording = make_recording(events);

        // Consume the read
        let _ = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();

        // Reset
        recording.reset();

        // Should be able to match again
        let event = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        assert_eq!(event.seq, 0);
    }

    #[test]
    fn test_has_reads_in_segment() {
        let events = vec![
            make_event("node1", 0, "get_relation", SemanticCategory::MetadataRead),
            make_event("node1", 1, "execute", SemanticCategory::Write),
            make_event("node1", 2, "list_schemas", SemanticCategory::MetadataRead),
        ];
        let recording = make_recording(events);

        // Initially has reads in segment
        assert!(recording.has_reads_in_segment("node1"));

        // Consume the write to advance to next segment
        let _ = recording
            .take_semantic_match("node1", "execute", &empty_args(), SemanticCategory::Write)
            .unwrap();

        // New segment also has reads
        assert!(recording.has_reads_in_segment("node1"));
    }

    #[test]
    fn test_semantic_mode_reads_can_match_multiple_times() {
        // In semantic mode, reads are NOT tracked - same read can be matched multiple times
        let events = vec![
            make_event("node1", 0, "get_relation", SemanticCategory::MetadataRead),
            make_event("node1", 1, "execute", SemanticCategory::Write),
        ];
        let recording = make_recording(events);

        // Match the same read multiple times - should succeed each time
        let event1 = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        assert_eq!(event1.method, "get_relation");

        let event2 = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        assert_eq!(event2.method, "get_relation");

        let event3 = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &empty_args(),
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        assert_eq!(event3.method, "get_relation");

        // But writes are still tracked - can only match once
        let write = recording
            .take_semantic_match("node1", "execute", &empty_args(), SemanticCategory::Write)
            .unwrap();
        assert_eq!(write.method, "execute");

        // Trying to match the write again fails (no more writes)
        let no_write = recording.take_semantic_match(
            "node1",
            "execute",
            &empty_args(),
            SemanticCategory::Write,
        );
        assert!(no_write.is_none());
    }

    #[test]
    fn test_has_pending_write() {
        let events = vec![
            make_event("node1", 0, "get_relation", SemanticCategory::MetadataRead),
            make_event("node1", 1, "execute", SemanticCategory::Write),
            make_event("node1", 2, "get_relation", SemanticCategory::MetadataRead),
        ];
        let recording = make_recording(events);

        // Has pending write
        assert!(recording.has_pending_write("node1"));

        // Consume the write
        let _ = recording
            .take_semantic_match("node1", "execute", &empty_args(), SemanticCategory::Write)
            .unwrap();

        // No more pending writes
        assert!(!recording.has_pending_write("node1"));
    }

    #[test]
    fn test_semantic_mode_matches_by_args() {
        // Helper to create an event with specific args
        fn make_event_with_args(
            node_id: &str,
            seq: u32,
            method: &str,
            args: serde_json::Value,
            category: SemanticCategory,
        ) -> AdapterCallEvent {
            AdapterCallEvent {
                node_id: node_id.to_string(),
                seq,
                method: method.to_string(),
                semantic_category: category,
                args,
                result: serde_json::json!({"seq": seq}), // Different result for each
                success: true,
                error: None,
                timestamp_ns: seq as u64 * 1000,
            }
        }

        // Create events with same method but different args
        let events = vec![
            make_event_with_args(
                "node1",
                0,
                "get_relation",
                serde_json::json!(["DB", "SCHEMA", "TABLE_A"]),
                SemanticCategory::MetadataRead,
            ),
            make_event_with_args(
                "node1",
                1,
                "get_relation",
                serde_json::json!(["DB", "SCHEMA", "TABLE_B"]),
                SemanticCategory::MetadataRead,
            ),
            make_event_with_args(
                "node1",
                2,
                "execute",
                serde_json::json!(["CREATE TABLE foo"]),
                SemanticCategory::Write,
            ),
        ];

        let mut adapter_events_by_node: BTreeMap<String, Vec<AdapterCallEvent>> = BTreeMap::new();
        for event in events {
            adapter_events_by_node
                .entry(event.node_id.clone())
                .or_default()
                .push(event);
        }

        let recording = Recording {
            header: RecordingHeader {
                format_version: 1,
                fusion_version: "test".to_string(),
                adapter_type: "snowflake".to_string(),
                recorded_at: "2024-01-01T00:00:00Z".to_string(),
                invocation_id: "test-123".to_string(),
                metadata: serde_json::Map::new(),
                invocation_command: None,
            },
            adapter_events_by_node,
            metadata_events_by_caller: BTreeMap::new(),
            sao_events: BTreeMap::new(),
            clone_events: BTreeMap::new(),
            run_remote_adhoc_events: Vec::new(),
            cache_invalidation_events: Vec::new(),
            adapter_positions: RwLock::new(HashMap::new()),
            metadata_positions: RwLock::new(HashMap::new()),
            run_remote_adhoc_position: RwLock::new(0),
            cache_invalidation_position: RwLock::new(0),
            semantic_adapter_state: RwLock::new(HashMap::new()),
            semantic_metadata_state: RwLock::new(HashMap::new()),
            ro_exec_matched: RwLock::new(HashMap::new()),
        };

        // Request TABLE_B first (out of order by seq, but should match by args)
        let args_b = serde_json::json!(["DB", "SCHEMA", "TABLE_B"]);
        let event = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &args_b,
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        // Should get the TABLE_B result (seq 1)
        assert_eq!(event.result, serde_json::json!({"seq": 1}));

        // Request TABLE_A (should still find it since reads aren't consumed)
        let args_a = serde_json::json!(["DB", "SCHEMA", "TABLE_A"]);
        let event = recording
            .take_semantic_match(
                "node1",
                "get_relation",
                &args_a,
                SemanticCategory::MetadataRead,
            )
            .unwrap();
        // Should get the TABLE_A result (seq 0)
        assert_eq!(event.result, serde_json::json!({"seq": 0}));

        // Request TABLE_C which doesn't exist - should return None
        let args_c = serde_json::json!(["DB", "SCHEMA", "TABLE_C"]);
        let result = recording.take_semantic_match(
            "node1",
            "get_relation",
            &args_c,
            SemanticCategory::MetadataRead,
        );
        assert!(result.is_none(), "Should not find TABLE_C");
    }

    #[test]
    fn test_submit_python_job_ignores_model_raw_code() {
        // Recordings made before raw_code was populated at parse time captured the
        // `--placeholder--` stub in the serialized model object; newer binaries populate
        // it with the verbatim source. raw_code does not affect the submitted job, so the
        // two must still match. See `python_job_args_match`.
        let recorded = serde_json::json!([
            { "__type__": "LazyModelWrapper", "alias": "m", "raw_code": "--placeholder--" },
            "import pandas as pd\n\ndef model(dbt, session):\n    return None\n"
        ]);
        let actual = serde_json::json!([
            { "__type__": "LazyModelWrapper", "alias": "m", "raw_code": "import pandas as pd\n\ndef model(dbt, session):\n    return None" },
            "import pandas as pd\n\ndef model(dbt, session):\n    return None\n"
        ]);

        assert!(adapter_args_match_for_type(
            "submit_python_job",
            &recorded,
            &actual,
            AdapterType::Snowflake
        ));
        assert!(adapter_args_match_for_type(
            "submit_python_job",
            &actual,
            &recorded,
            AdapterType::Snowflake
        ));
    }

    #[test]
    fn test_submit_python_job_ignores_compiled_code_blank_lines() {
        // PR #13818 changed how `render_python_model` joins the user's source to
        // py_script_postfix (`{}\n{}` on raw_python -> `{}\n\n{}` on raw_python.trim_end()),
        // which shifted every python model payload by one blank line and false-failed
        // replay of recordings made before it. Blank-line runs must compare equal.
        let recorded = serde_json::json!([
            { "__type__": "LazyModelWrapper", "alias": "m" },
            "def model(dbt, session):\n    return df\n\n# COMMAND ----------\ndef main(session):\n    return \"OK\"\n"
        ]);
        let actual = serde_json::json!([
            { "__type__": "LazyModelWrapper", "alias": "m" },
            "def model(dbt, session):\n    return df\n\n\n# COMMAND ----------\ndef main(session):\n    return \"OK\"  \n\n\n"
        ]);

        assert!(adapter_args_match_for_type(
            "submit_python_job",
            &recorded,
            &actual,
            AdapterType::Snowflake
        ));
        assert!(adapter_args_match_for_type(
            "submit_python_job",
            &actual,
            &recorded,
            AdapterType::Snowflake
        ));
    }

    #[test]
    fn test_submit_python_job_still_detects_indentation_diff() {
        // Blank-line tolerance must not extend to indentation, which is significant.
        let recorded = serde_json::json!([
            { "__type__": "LazyModelWrapper", "alias": "m" },
            "def model(dbt, session):\n    return df\n"
        ]);
        let actual = serde_json::json!([
            { "__type__": "LazyModelWrapper", "alias": "m" },
            "def model(dbt, session):\n        return df\n"
        ]);

        assert!(!adapter_args_match_for_type(
            "submit_python_job",
            &recorded,
            &actual,
            AdapterType::Snowflake
        ));
    }

    #[test]
    fn test_submit_python_job_still_detects_compiled_code_diff() {
        // Stripping raw_code must not mask a real difference in the submitted python.
        let recorded = serde_json::json!([
            { "__type__": "LazyModelWrapper", "alias": "m", "raw_code": "--placeholder--" },
            "def model(dbt, session):\n    return 1\n"
        ]);
        let actual = serde_json::json!([
            { "__type__": "LazyModelWrapper", "alias": "m", "raw_code": "def model(dbt, session):\n    return 2" },
            "def model(dbt, session):\n    return 2\n"
        ]);

        assert!(!adapter_args_match_for_type(
            "submit_python_job",
            &recorded,
            &actual,
            AdapterType::Snowflake
        ));
    }

    #[test]
    fn test_get_relation_args_match_positional_and_kwargs() {
        let positional = serde_json::json!(["rioter_dbt", "dbt_artifacts", "model_executions"]);
        let kwargs = serde_json::json!([
            {
                "__type__": "minijinja::value::argtypes::KwargsMutableMap",
                "database": "rioter_dbt",
                "schema": "dbt_artifacts",
                "identifier": "model_executions"
            }
        ]);

        assert!(adapter_args_match_for_type(
            "get_relation",
            &kwargs,
            &positional,
            AdapterType::Snowflake
        ));
        assert!(adapter_args_match_for_type(
            "get_relation",
            &positional,
            &kwargs,
            AdapterType::Snowflake
        ));
    }

    /// Serialized `RelationObject` in the shape `serializable_impls.rs` writes, so these tests
    /// exercise the real recorded payload rather than a hand-trimmed subset.
    fn relation_args(extra: serde_json::Value) -> serde_json::Value {
        let mut relation = serde_json::json!({
            "__type__": "RelationObject",
            "adapter_type": "snowflake",
            "database": "CERTIFIED_UAT",
            "schema": "common",
            "identifier": "dim_opportunity",
            "is_cte": false,
            "is_delta": false,
            "is_dynamic_table": false,
            "is_materialized_view": false,
            "is_metric_view": false,
            "is_streaming_table": false,
            "is_table": false,
            "is_view": false,
            "quote_policy": { "database": false, "identifier": false, "schema": false },
        });
        let map = relation.as_object_mut().expect("object");
        for (key, value) in extra.as_object().expect("object") {
            map.insert(key.clone(), value.clone());
        }
        serde_json::json!([relation])
    }

    /// A cross-project (Mesh) `ref` to a public node with no materialization records as
    /// `relation_type: None` and replays as `View`. The relation name is identical, so the
    /// recorded columns are a valid answer and the match must succeed.
    #[test]
    fn test_get_columns_in_relation_ignores_relation_type_flags() {
        let recorded = relation_args(serde_json::json!({}));
        // `is_interactive_table` is absent from recordings predating that materialization.
        let actual = relation_args(serde_json::json!({
            "is_view": true,
            "is_interactive_table": false,
            "is_metric_view": true,
        }));

        assert!(adapter_args_match_for_type(
            "get_columns_in_relation",
            &recorded,
            &actual,
            AdapterType::Snowflake
        ));
        assert!(adapter_args_match_for_type(
            "get_columns_in_relation",
            &actual,
            &recorded,
            AdapterType::Snowflake
        ));
    }

    /// Only the type flags are exempt: relation identity must still match exactly, or a read
    /// against one relation could be answered with another relation's columns.
    #[test]
    fn test_get_columns_in_relation_still_compares_identity() {
        let recorded = relation_args(serde_json::json!({}));

        for divergence in [
            serde_json::json!({ "database": "CERTIFIED_PROD" }),
            serde_json::json!({ "schema": "customer_success" }),
            serde_json::json!({ "identifier": "dim_product" }),
        ] {
            assert!(
                !adapter_args_match_for_type(
                    "get_columns_in_relation",
                    &recorded,
                    &relation_args(divergence.clone()),
                    AdapterType::Snowflake
                ),
                "identity divergence must not match: {divergence}"
            );
        }
    }

    /// Non-type fields stay strict — quoting changes how the relation renders into the
    /// `get_columns_in_relation` query, so it is not interchangeable.
    #[test]
    fn test_get_columns_in_relation_still_compares_quote_policy() {
        let recorded = relation_args(serde_json::json!({}));
        let actual = relation_args(serde_json::json!({
            "quote_policy": { "database": false, "identifier": true, "schema": false },
        }));

        assert!(!adapter_args_match_for_type(
            "get_columns_in_relation",
            &recorded,
            &actual,
            AdapterType::Snowflake
        ));
    }

    /// The exemption is scoped to `get_columns_in_relation`. Other relation-taking methods keep
    /// the strict structural comparison, because for them the type can change the behavior.
    #[test]
    fn test_relation_type_flags_still_compared_for_other_methods() {
        let recorded = relation_args(serde_json::json!({}));
        let actual = relation_args(serde_json::json!({ "is_view": true }));

        assert!(!adapter_args_match_for_type(
            "drop_relation",
            &recorded,
            &actual,
            AdapterType::Snowflake
        ));
    }

    #[test]
    fn test_semantic_mode_read_does_not_advance_over_skipped_write() {
        let events = vec![
            AdapterCallEvent {
                node_id: "node1".to_string(),
                seq: 0,
                method: "get_relation".to_string(),
                semantic_category: SemanticCategory::MetadataRead,
                args: serde_json::json!(["DB", "SCHEMA", "model_executions"]),
                result: serde_json::json!({"seq": 0}),
                success: true,
                error: None,
                timestamp_ns: 0,
            },
            AdapterCallEvent {
                node_id: "node1".to_string(),
                seq: 1,
                method: "execute".to_string(),
                semantic_category: SemanticCategory::Write,
                args: serde_json::json!(["insert model_executions"]),
                result: serde_json::json!({"seq": 1}),
                success: true,
                error: None,
                timestamp_ns: 1,
            },
            AdapterCallEvent {
                node_id: "node1".to_string(),
                seq: 2,
                method: "get_relation".to_string(),
                semantic_category: SemanticCategory::MetadataRead,
                args: serde_json::json!(["DB", "SCHEMA", "test_executions"]),
                result: serde_json::json!({"seq": 2}),
                success: true,
                error: None,
                timestamp_ns: 2,
            },
            AdapterCallEvent {
                node_id: "node1".to_string(),
                seq: 3,
                method: "execute".to_string(),
                semantic_category: SemanticCategory::Write,
                args: serde_json::json!(["insert test_executions"]),
                result: serde_json::json!({"seq": 3}),
                success: true,
                error: None,
                timestamp_ns: 3,
            },
        ];
        let recording = make_recording(events);

        let test_args = serde_json::json!(["DB", "SCHEMA", "test_executions"]);
        let read = recording.take_semantic_match(
            "node1",
            "get_relation",
            &test_args,
            SemanticCategory::MetadataRead,
        );
        assert!(
            read.is_none(),
            "Should not match a future read across a skipped write"
        );
    }

    #[test]
    fn test_get_relation_args_match_needs_information() {
        let default_needs_information =
            serde_json::json!(["rioter_dbt", "dbt_artifacts", "model_executions"]);
        let explicit_false = serde_json::json!([
            "rioter_dbt",
            "dbt_artifacts",
            "model_executions",
            {
                "__type__": "minijinja::value::argtypes::KwargsMutableMap",
                "needs_information": false
            }
        ]);
        let explicit_true = serde_json::json!([
            {
                "__type__": "minijinja::value::argtypes::KwargsMutableMap",
                "database": "rioter_dbt",
                "schema": "dbt_artifacts",
                "identifier": "model_executions",
                "needs_information": true
            }
        ]);
        let positional_true =
            serde_json::json!(["rioter_dbt", "dbt_artifacts", "model_executions", true]);

        assert!(adapter_args_match_for_type(
            "get_relation",
            &default_needs_information,
            &explicit_false,
            AdapterType::Snowflake,
        ));
        assert!(!adapter_args_match_for_type(
            "get_relation",
            &default_needs_information,
            &explicit_true,
            AdapterType::Snowflake,
        ));
        assert!(!adapter_args_match_for_type(
            "get_relation",
            &default_needs_information,
            &positional_true,
            AdapterType::Snowflake,
        ));
        assert!(adapter_args_match_for_type(
            "get_relation",
            &explicit_true,
            &positional_true,
            AdapterType::Snowflake,
        ));
    }

    /// Minimal `RelationObject`-shaped arg, per `RelationObject::to_time_machine_json`.
    fn relation_arg(database: &str, schema: &str, identifier: &str) -> serde_json::Value {
        relation_arg_for("bigquery", database, schema, identifier)
    }

    fn relation_arg_for(
        adapter_type: &str,
        database: &str,
        schema: &str,
        identifier: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "__type__": "RelationObject",
            "adapter_type": adapter_type,
            "database": database,
            "schema": schema,
            "identifier": identifier,
            "is_table": true,
        })
    }

    fn bare_staging_relation() -> serde_json::Value {
        relation_arg("my_project", "my_schema", "my_snapshot__dbt_tmp")
    }

    fn suffixed_staging_relation() -> serde_json::Value {
        relation_arg(
            "my_project",
            "my_schema",
            "my_snapshot__dbt_tmp152555838935494",
        )
    }

    fn target_relation() -> serde_json::Value {
        relation_arg("my_project", "my_schema", "my_snapshot")
    }

    fn assert_args_match_both_ways(
        method: &str,
        recorded: &serde_json::Value,
        actual: &serde_json::Value,
    ) {
        assert_args_match_both_ways_for(AdapterType::Bigquery, method, recorded, actual);
    }

    fn assert_args_match_both_ways_for(
        adapter_type: AdapterType,
        method: &str,
        recorded: &serde_json::Value,
        actual: &serde_json::Value,
    ) {
        assert!(
            adapter_args_match_for_type(method, recorded, actual, adapter_type),
            "{adapter_type} args should match recorded -> actual"
        );
        assert!(
            adapter_args_match_for_type(method, actual, recorded, adapter_type),
            "{adapter_type} args should match actual -> recorded"
        );
    }

    #[test]
    fn test_get_missing_columns_matches_bare_and_suffixed_tmp_relation() {
        // A bare staging identifier on one side and a clock-suffixed one on the other must
        // still compare equal; every other relation field is identical.
        let recorded = serde_json::json!([bare_staging_relation(), target_relation()]);
        let actual = serde_json::json!([suffixed_staging_relation(), target_relation()]);

        assert_args_match_both_ways("get_missing_columns", &recorded, &actual);
    }

    #[test]
    fn test_get_columns_in_relation_matches_bare_and_suffixed_tmp_relation() {
        let recorded = serde_json::json!([bare_staging_relation()]);
        let actual = serde_json::json!([suffixed_staging_relation()]);

        assert_args_match_both_ways("get_columns_in_relation", &recorded, &actual);
    }

    #[test]
    fn test_drop_relation_matches_bare_and_suffixed_tmp_relation() {
        let recorded = serde_json::json!([bare_staging_relation()]);
        let actual = serde_json::json!([suffixed_staging_relation()]);

        assert_args_match_both_ways("drop_relation", &recorded, &actual);
    }

    #[test]
    fn test_tmp_suffix_normalization_still_rejects_different_relations() {
        let recorded = serde_json::json!([
            relation_arg("my_project", "my_schema", "snapshot_a__dbt_tmp"),
            target_relation()
        ]);
        let different_base = serde_json::json!([
            relation_arg("my_project", "my_schema", "snapshot_b__dbt_tmp111"),
            target_relation()
        ]);
        assert!(!adapter_args_match_for_type(
            "get_missing_columns",
            &recorded,
            &different_base,
            AdapterType::Bigquery
        ));

        let different_schema = serde_json::json!([
            relation_arg("my_project", "my_other_schema", "snapshot_a__dbt_tmp111"),
            target_relation()
        ]);
        assert!(!adapter_args_match_for_type(
            "get_missing_columns",
            &recorded,
            &different_schema,
            AdapterType::Bigquery
        ));
    }

    #[test]
    fn test_expand_target_column_types_still_matches_after_arm_removal() {
        let recorded = serde_json::json!([
            {
                "__type__": "minijinja::value::argtypes::KwargsMutableMap",
                "from_relation": bare_staging_relation(),
                "to_relation": target_relation(),
            }
        ]);
        let actual = serde_json::json!([
            {
                "__type__": "minijinja::value::argtypes::KwargsMutableMap",
                "from_relation": suffixed_staging_relation(),
                "to_relation": target_relation(),
            }
        ]);

        assert_args_match_both_ways("expand_target_column_types", &recorded, &actual);
    }

    #[test]
    fn test_get_columns_in_select_sql_matches_suffixed_tmp_relation_in_sql() {
        let recorded =
            serde_json::json!(["select * from `my_project`.`my_schema`.`my_snapshot__dbt_tmp`"]);
        let actual = serde_json::json!([
            "select * from `my_project`.`my_schema`.`my_snapshot__dbt_tmp152555838935494`"
        ]);
        assert_args_match_both_ways("get_columns_in_select_sql", &recorded, &actual);

        let other_table = serde_json::json!([
            "select * from `my_project`.`my_schema`.`my_other_snapshot__dbt_tmp152555838935494`"
        ]);
        assert!(!adapter_args_match_for_type(
            "get_columns_in_select_sql",
            &recorded,
            &other_table,
            AdapterType::Bigquery
        ));
    }

    /// Normalization runs before the per-method comparison, so it must hold for every
    /// adapter, not just the BigQuery snapshots that motivated it.
    #[test]
    fn test_tmp_suffix_normalization_covers_other_adapters() {
        // Postgres and Redshift append strftime("%H%M%S%f") with no separator, as BigQuery
        // does; Snowflake and Spark reuse the bare identifier.
        let cases = [
            (AdapterType::Postgres, "my_snapshot__dbt_tmp152555838935494"),
            (AdapterType::Redshift, "my_snapshot__dbt_tmp152555838935494"),
            (AdapterType::Snowflake, "my_snapshot__dbt_tmp"),
            (AdapterType::Spark, "my_snapshot__dbt_tmp"),
        ];

        for (adapter_type, identifier) in cases {
            let serialized = adapter_type.to_string();
            let recorded = serde_json::json!([relation_arg_for(
                &serialized,
                "my_db",
                "my_schema",
                "my_snapshot__dbt_tmp"
            )]);
            let actual = serde_json::json!([relation_arg_for(
                &serialized,
                "my_db",
                "my_schema",
                identifier
            )]);
            assert_args_match_both_ways_for(
                adapter_type,
                "get_columns_in_relation",
                &recorded,
                &actual,
            );

            // A different base name must still be rejected on every adapter.
            let other = serde_json::json!([relation_arg_for(
                &serialized,
                "my_db",
                "my_schema",
                "my_other_snapshot__dbt_tmp"
            )]);
            assert!(
                !adapter_args_match_for_type(
                    "get_columns_in_relation",
                    &recorded,
                    &other,
                    adapter_type
                ),
                "{adapter_type} must not match a different base relation"
            );
        }
    }

    /// Pins a known over-match: microbatch appends `_<batch.id>`, which `TmpSuffixSanitizer`
    /// consumes, so two batches of one node compare equal. Tightening the regex flips this.
    #[test]
    fn test_microbatch_batch_ids_currently_collapse() {
        let batch_a = serde_json::json!([relation_arg_for(
            "snowflake",
            "my_db",
            "my_schema",
            "my_model__dbt_tmp_20240115"
        )]);
        let batch_b = serde_json::json!([relation_arg_for(
            "snowflake",
            "my_db",
            "my_schema",
            "my_model__dbt_tmp_20240116"
        )]);

        assert!(adapter_args_match_for_type(
            "get_columns_in_relation",
            &batch_a,
            &batch_b,
            AdapterType::Snowflake
        ));
    }

    /// Pins a known gap: Databricks `unique_tmp_table_suffix` and duckdb build the suffix
    /// from a UUID, which `TmpSuffixSanitizer` never matches, so it is compared verbatim.
    #[test]
    fn test_uuid_tmp_suffix_is_not_normalized() {
        let recorded = serde_json::json!([relation_arg_for(
            "databricks",
            "my_catalog",
            "my_schema",
            "my_snapshot__dbt_tmp"
        )]);
        let actual = serde_json::json!([relation_arg_for(
            "databricks",
            "my_catalog",
            "my_schema",
            "my_snapshot__dbt_tmp_3f2a1b4c_a0ba_4708_a0b1_813316032bfb"
        )]);

        assert!(!adapter_args_match_for_type(
            "get_columns_in_relation",
            &recorded,
            &actual,
            AdapterType::Databricks
        ));
    }

    #[test]
    fn test_sql_args_match_handles_whitespace_differences() {
        // Test that SQL values are matched using fuzzy comparison
        let sql1 = serde_json::json!(["SELECT   *\nFROM    users"]);
        let sql2 = serde_json::json!(["SELECT*FROMusers"]);
        assert!(
            sql_args_match(&sql1, &sql2, AdapterType::Snowflake),
            "SQL strings should match ignoring whitespace"
        );
    }

    #[test]
    fn test_sql_args_match_handles_query_tag_differences() {
        // Test that query tag payloads are canonicalized
        let sql1 = serde_json::json!([r#"alter session set query_tag = '{"model": "a"}'"#]);
        let sql2 = serde_json::json!([r#"alter session set query_tag = '{"model": "b"}'"#]);
        assert!(
            sql_args_match(&sql1, &sql2, AdapterType::Snowflake),
            "Query tag payloads should be canonicalized"
        );
    }

    #[test]
    fn test_sql_args_match_handles_uuid_differences() {
        // Test that UUID literals are canonicalized
        let sql1 = serde_json::json!(["SELECT '8f439b7e-752f-460a-8d1a-f469231d169c' AS id"]);
        let sql2 = serde_json::json!(["SELECT '019a71ca-e5ad-7ca3-99d8-49b58a470d82' AS id"]);
        assert!(
            sql_args_match(&sql1, &sql2, AdapterType::Snowflake),
            "UUID literals should be canonicalized"
        );
    }

    #[test]
    fn test_sql_args_match_handles_timestamp_differences() {
        // Test that timestamp literals are matched flexibly
        let sql1 = serde_json::json!(["WHERE created_at >= '2025-09-10T18:07:45'"]);
        let sql2 = serde_json::json!(["WHERE created_at >= '2025-09-10T14:16:52'"]);
        assert!(
            sql_args_match(&sql1, &sql2, AdapterType::Snowflake),
            "Timestamp literals should be matched flexibly"
        );
    }

    #[test]
    fn test_sql_args_match_detects_real_differences() {
        // Test that actual semantic differences are still detected
        let sql1 = serde_json::json!(["SELECT * FROM users"]);
        let sql2 = serde_json::json!(["SELECT * FROM orders"]);
        assert!(
            !sql_args_match(&sql1, &sql2, AdapterType::Snowflake),
            "Different table names should not match"
        );
    }

    #[test]
    fn test_semantic_mode_does_not_match_date_spine_probe_to_model_execute() {
        let node_id = "model.project.date_details";
        let recorded_model_sql = r#"
            create or replace view analytics.public.date_details as (
                with date_spine as (
                    with rawdata as (
                        select 1 as generated_number
                    )
                    select * from rawdata
                )
                select * from date_spine
            )
        "#;
        let actual_date_spine_probe_sql = r#"
            select datediff(
                day,
                to_date('01/01/2019', 'mm/dd/yyyy'),
                current_date+1
            )
        "#;
        let recording = make_recording(vec![AdapterCallEvent {
            node_id: node_id.to_string(),
            seq: 0,
            method: "execute".to_string(),
            semantic_category: SemanticCategory::Write,
            args: serde_json::json!([recorded_model_sql]),
            result: serde_json::json!({"rows": 0}),
            success: true,
            error: None,
            timestamp_ns: 0,
        }]);

        let actual_args = serde_json::json!([actual_date_spine_probe_sql]);
        let event = recording.take_semantic_match(
            node_id,
            "execute",
            &actual_args,
            SemanticCategory::Write,
        );

        assert!(
            event.is_none(),
            "date_spine's compile-time datediff probe should not consume the recorded model DDL write"
        );

        let model_args = serde_json::json!([recorded_model_sql]);
        let event = recording
            .take_semantic_match(node_id, "execute", &model_args, SemanticCategory::Write)
            .expect("the recorded model DDL should remain available after the probe mismatch");
        assert_eq!(event.args, model_args);
    }

    /// Recording has [SHOW_PARAMS, DATEDIFF_PROBE, CREATE_VIEW] but Fusion emits them
    /// as [DATEDIFF_PROBE, SHOW_PARAMS, CREATE_VIEW]. In semantic mode all three should
    /// still be matched correctly.
    #[test]
    fn test_semantic_mode_date_spine_ordering_mismatch() {
        let node_id = "model.project.date_details";
        let show_sql = "show parameters like 'query_tag' in session";
        let probe_sql =
            "select datediff(day, '2018-12-31'::date, sysdate()::date + interval '1 day')";
        let ddl_sql = "create or replace view analytics.public.date_details as (select 1)";

        let make_exec = |seq: u32, sql: &str| AdapterCallEvent {
            node_id: node_id.to_string(),
            seq,
            method: "execute".to_string(),
            semantic_category: SemanticCategory::Write,
            args: serde_json::json!([sql]),
            result: serde_json::json!({"rows": 0}),
            success: true,
            error: None,
            timestamp_ns: seq as u64,
        };

        // Recording order: SHOW_PARAMS (0), DATEDIFF_PROBE (1), CREATE_VIEW (2)
        let recording = make_recording(vec![
            make_exec(0, show_sql),
            make_exec(1, probe_sql),
            make_exec(2, ddl_sql),
        ]);

        // Fusion replay order: DATEDIFF_PROBE first, then SHOW_PARAMS, then CREATE_VIEW
        let probe_args = serde_json::json!([probe_sql]);
        let e = recording
            .take_semantic_match(node_id, "execute", &probe_args, SemanticCategory::Write)
            .expect("probe should be found as untracked read in segment");
        assert_eq!(e.seq, 1);

        let show_args = serde_json::json!([show_sql]);
        let e = recording
            .take_semantic_match(node_id, "execute", &show_args, SemanticCategory::Write)
            .expect("SHOW PARAMETERS should be found as untracked read in segment");
        assert_eq!(e.seq, 0);

        let ddl_args = serde_json::json!([ddl_sql]);
        let e = recording
            .take_semantic_match(node_id, "execute", &ddl_args, SemanticCategory::Write)
            .expect("CREATE VIEW should still be matched as a write barrier");
        assert_eq!(e.seq, 2);
    }

    /// In strict mode, read-only execute calls (SELECT/SHOW) should be matched globally
    /// via take_ro_exec_read and should not consume the sequential write slot.
    #[test]
    fn test_strict_mode_read_only_execute_global_match() {
        let node_id = "model.project.date_details";
        let show_sql = "show parameters like 'query_tag' in session";
        let probe_sql = "select datediff(day, '2018-12-31'::date, sysdate()::date)";
        let ddl_sql = "create or replace view analytics.public.date_details as (select 1)";

        let make_exec = |seq: u32, sql: &str| AdapterCallEvent {
            node_id: node_id.to_string(),
            seq,
            method: "execute".to_string(),
            semantic_category: SemanticCategory::Write,
            args: serde_json::json!([sql]),
            result: serde_json::json!({"rows": 0}),
            success: true,
            error: None,
            timestamp_ns: seq as u64,
        };

        // Recording order: SHOW_PARAMS (0), DATEDIFF_PROBE (1), CREATE_VIEW (2)
        let recording = make_recording(vec![
            make_exec(0, show_sql),
            make_exec(1, probe_sql),
            make_exec(2, ddl_sql),
        ]);

        // Strict mode replay: DATEDIFF_PROBE arrives first via global read search
        let probe_args = serde_json::json!([probe_sql]);
        let e = recording
            .take_ro_exec_read(node_id, "execute", &probe_args)
            .expect("probe should be matched globally");
        assert_eq!(e.seq, 1, "should match the probe event (seq 1)");

        // SHOW_PARAMS also arrives out of order; matched globally
        let show_args = serde_json::json!([show_sql]);
        let e = recording
            .take_ro_exec_read(node_id, "execute", &show_args)
            .expect("SHOW PARAMETERS should be matched globally");
        assert_eq!(e.seq, 0);

        // take_next (strict sequential) should skip seq 0 and seq 1 (already consumed
        // as read-only execute reads) and return seq 2 (CREATE_VIEW).
        let e = recording
            .take_next(node_id)
            .expect("CREATE VIEW should be the next sequential event");
        assert_eq!(
            e.seq, 2,
            "take_next should skip ro_exec_matched events and return CREATE_VIEW"
        );
    }

    #[test]
    fn test_sql_args_match_string_format() {
        // Test that SQL can be passed as a direct string (not wrapped in array)
        let sql1 = serde_json::json!("SELECT   *  FROM  users");
        let sql2 = serde_json::json!("SELECT * FROM users");
        assert!(
            sql_args_match(&sql1, &sql2, AdapterType::Snowflake),
            "Direct SQL strings should be matched with fuzzy comparison"
        );
    }

    #[test]
    fn test_semantic_mode_matches_sql_with_whitespace_differences() {
        // Helper to create an event with SQL args
        fn make_sql_event(
            node_id: &str,
            seq: u32,
            method: &str,
            sql: &str,
            category: SemanticCategory,
        ) -> AdapterCallEvent {
            AdapterCallEvent {
                node_id: node_id.to_string(),
                seq,
                method: method.to_string(),
                semantic_category: category,
                args: serde_json::json!([sql]),
                result: serde_json::json!({"rows": 0}),
                success: true,
                error: None,
                timestamp_ns: seq as u64 * 1000,
            }
        }

        // Create events with SQL that differs only by whitespace
        let events = vec![make_sql_event(
            "node1",
            0,
            "execute",
            "SELECT   *\nFROM    users\nWHERE   id = 1",
            SemanticCategory::Write,
        )];
        let recording = make_recording(events);

        // Should match with compact SQL (whitespace removed)
        let compact_sql = serde_json::json!(["SELECT * FROM users WHERE id = 1"]);
        let event = recording
            .take_semantic_match("node1", "execute", &compact_sql, SemanticCategory::Write)
            .expect("Should match SQL ignoring whitespace differences");
        assert_eq!(event.method, "execute");
    }

    // -------------------------------------------------------------------------
    // SAO (State Aware Orchestration) tests
    // -------------------------------------------------------------------------

    use super::super::event::{RecordedRunCacheCloneDecision, RunCacheCloneEvent, SaoStatus};

    /// Helper to create a test Recording with SAO events
    fn make_recording_with_sao(
        adapter_events: Vec<AdapterCallEvent>,
        sao_events: Vec<SaoEvent>,
    ) -> Recording {
        let mut adapter_events_by_node: BTreeMap<String, Vec<AdapterCallEvent>> = BTreeMap::new();
        for event in adapter_events {
            adapter_events_by_node
                .entry(event.node_id.clone())
                .or_default()
                .push(event);
        }
        for events in adapter_events_by_node.values_mut() {
            events.sort_by_key(|e| e.seq);
        }

        let mut sao_events_map: BTreeMap<String, SaoEvent> = BTreeMap::new();
        for event in sao_events {
            sao_events_map.insert(event.node_id.clone(), event);
        }

        Recording {
            header: RecordingHeader {
                format_version: 1,
                fusion_version: "test".to_string(),
                adapter_type: "snowflake".to_string(),
                recorded_at: "2024-01-01T00:00:00Z".to_string(),
                invocation_id: "test-123".to_string(),
                invocation_command: None,
                metadata: serde_json::Map::new(),
            },
            adapter_events_by_node,
            metadata_events_by_caller: BTreeMap::new(),
            sao_events: sao_events_map,
            clone_events: BTreeMap::new(),
            run_remote_adhoc_events: Vec::new(),
            cache_invalidation_events: Vec::new(),
            adapter_positions: RwLock::new(HashMap::new()),
            metadata_positions: RwLock::new(HashMap::new()),
            run_remote_adhoc_position: RwLock::new(0),
            cache_invalidation_position: RwLock::new(0),
            semantic_adapter_state: RwLock::new(HashMap::new()),
            semantic_metadata_state: RwLock::new(HashMap::new()),
            ro_exec_matched: RwLock::new(HashMap::new()),
        }
    }

    #[test]
    fn test_sao_event_lookup() {
        let sao_events = vec![
            SaoEvent {
                node_id: "model.test.orders".to_string(),
                status: SaoStatus::ReusedNoChanges,
                message: "No new changes on any upstreams".to_string(),
                stored_hash: "abc123".to_string(),
                cached_test_result: None,
                timestamp_ns: 1000,
            },
            SaoEvent {
                node_id: "model.test.customers".to_string(),
                status: SaoStatus::ReusedStillFresh {
                    freshness_seconds: 3600,
                    last_updated_seconds: 1800,
                },
                message: "Still within freshness period".to_string(),
                stored_hash: "def456".to_string(),
                cached_test_result: None,
                timestamp_ns: 2000,
            },
        ];
        let recording = make_recording_with_sao(vec![], sao_events);

        // Should find SAO events by node_id
        let event = recording.get_sao_event("model.test.orders").unwrap();
        assert!(matches!(event.status, SaoStatus::ReusedNoChanges));
        assert_eq!(event.message, "No new changes on any upstreams");
        assert_eq!(event.stored_hash, "abc123");

        let event = recording.get_sao_event("model.test.customers").unwrap();
        if let SaoStatus::ReusedStillFresh {
            freshness_seconds,
            last_updated_seconds,
        } = event.status
        {
            assert_eq!(freshness_seconds, 3600);
            assert_eq!(last_updated_seconds, 1800);
        } else {
            panic!("Expected ReusedStillFresh status");
        }

        // Should return None for unknown node
        assert!(recording.get_sao_event("model.test.unknown").is_none());
    }

    #[test]
    fn test_has_sao_event() {
        let sao_events = vec![SaoEvent {
            node_id: "model.test.orders".to_string(),
            status: SaoStatus::ReusedNoChanges,
            message: "No new changes".to_string(),
            stored_hash: "abc123".to_string(),
            cached_test_result: None,
            timestamp_ns: 1000,
        }];
        let recording = make_recording_with_sao(vec![], sao_events);

        assert!(recording.has_sao_event("model.test.orders"));
        assert!(!recording.has_sao_event("model.test.unknown"));
    }

    #[test]
    fn test_sao_node_ids() {
        let sao_events = vec![
            SaoEvent {
                node_id: "model.test.orders".to_string(),
                status: SaoStatus::ReusedNoChanges,
                message: "No new changes".to_string(),
                stored_hash: "abc123".to_string(),
                cached_test_result: None,
                timestamp_ns: 1000,
            },
            SaoEvent {
                node_id: "model.test.customers".to_string(),
                status: SaoStatus::ReusedStillFreshNoChanges,
                message: "Still fresh".to_string(),
                stored_hash: "def456".to_string(),
                cached_test_result: None,
                timestamp_ns: 2000,
            },
        ];
        let recording = make_recording_with_sao(vec![], sao_events);

        let node_ids: Vec<_> = recording.sao_node_ids().collect();
        assert_eq!(node_ids.len(), 2);
        assert!(node_ids.contains(&"model.test.orders"));
        assert!(node_ids.contains(&"model.test.customers"));
    }

    #[test]
    fn test_total_sao_events() {
        let sao_events = vec![
            SaoEvent {
                node_id: "model.test.orders".to_string(),
                status: SaoStatus::ReusedNoChanges,
                message: "No new changes".to_string(),
                stored_hash: "abc123".to_string(),
                cached_test_result: None,
                timestamp_ns: 1000,
            },
            SaoEvent {
                node_id: "model.test.customers".to_string(),
                status: SaoStatus::ReusedNoChanges,
                message: "No new changes".to_string(),
                stored_hash: "def456".to_string(),
                cached_test_result: None,
                timestamp_ns: 2000,
            },
        ];
        let recording = make_recording_with_sao(vec![], sao_events);

        assert_eq!(recording.total_sao_events(), 2);
    }

    #[test]
    fn test_total_events_includes_sao() {
        let adapter_events = vec![make_event("node1", 0, "execute", SemanticCategory::Write)];
        let sao_events = vec![SaoEvent {
            node_id: "model.test.orders".to_string(),
            status: SaoStatus::ReusedNoChanges,
            message: "No new changes".to_string(),
            stored_hash: "abc123".to_string(),
            cached_test_result: None,
            timestamp_ns: 1000,
        }];
        let mut recording = make_recording_with_sao(adapter_events, sao_events);
        recording.clone_events.insert(
            "model.test.cloned".to_string(),
            vec![RunCacheCloneEvent {
                node_id: "model.test.cloned".to_string(),
                dev_clone: false,
                clone: RecordedRunCacheCloneDecision {
                    request_id: "request-1".to_string(),
                    clone_sqls: vec!["CREATE TABLE target CLONE source".to_string()],
                    clone_source: "db.schema.source".to_string(),
                    clone_target: "db.schema.target".to_string(),
                    required_source_epoch: None,
                    execution_runtime_ms: None,
                    freshness_tolerance_seconds: 0,
                    is_stale: false,
                    execution_decision_id: None,
                },
                timestamp_ns: 3000,
            }],
        );

        // Total should include adapter, SAO, and run-cache clone events.
        assert_eq!(recording.total_events(), 3);
        assert_eq!(recording.total_adapter_events(), 1);
        assert_eq!(recording.total_sao_events(), 1);
    }

    #[test]
    fn test_mixed_adapter_and_sao_events() {
        // Some nodes have adapter events, some have SAO events
        let adapter_events = vec![make_event(
            "model.test.executed",
            0,
            "execute",
            SemanticCategory::Write,
        )];
        let sao_events = vec![SaoEvent {
            node_id: "model.test.skipped".to_string(),
            status: SaoStatus::ReusedNoChanges,
            message: "No new changes".to_string(),
            stored_hash: "abc123".to_string(),
            cached_test_result: None,
            timestamp_ns: 1000,
        }];
        let recording = make_recording_with_sao(adapter_events, sao_events);

        // Should have adapter events for executed node
        assert!(recording.events_for_node("model.test.executed").is_some());
        assert!(!recording.has_sao_event("model.test.executed"));

        // Should have SAO event for skipped node
        assert!(recording.events_for_node("model.test.skipped").is_none());
        assert!(recording.has_sao_event("model.test.skipped"));
    }

    #[test]
    fn test_metadata_args_match_list_relations_case_insensitive() {
        let recorded = MetadataCallArgs::ListRelationsSchemas {
            unique_id: Some("model.proj.my_model".to_string()),
            phase: Some("analyze".to_string()),
            relations: vec![
                "\"my_database\".\"public\".\"TABLE_A\"".to_string(),
                "\"my_database\".\"public\".\"TABLE_B\"".to_string(),
            ],
        };

        let actual = MetadataCallArgs::ListRelationsSchemas {
            unique_id: Some("model.proj.my_model".to_string()),
            phase: Some("analyze".to_string()),
            relations: vec!["\"MY_DATABASE\".\"PUBLIC\".\"TABLE_A\"".to_string()],
        };

        assert!(
            metadata_args_match(&recorded, &actual),
            "should match relations with different identifier casing"
        );

        let actual_exact = MetadataCallArgs::ListRelationsSchemas {
            unique_id: None,
            phase: None,
            relations: vec!["\"my_database\".\"public\".\"TABLE_A\"".to_string()],
        };
        assert!(metadata_args_match(&recorded, &actual_exact));

        let actual_missing = MetadataCallArgs::ListRelationsSchemas {
            unique_id: None,
            phase: None,
            relations: vec!["\"MY_DATABASE\".\"PUBLIC\".\"TABLE_C\"".to_string()],
        };
        assert!(
            !metadata_args_match(&recorded, &actual_missing),
            "should not match a relation that was never recorded"
        );
    }

    #[test]
    fn test_metadata_args_match_schema_freshness_case_insensitive() {
        let recorded = MetadataCallArgs::FreshnessAllInSchema {
            database: "my_database".to_string(),
            schema: "public".to_string(),
            relations: vec!["my_database.public.table_a".to_string()],
            warehouse: None,
        };
        let actual = MetadataCallArgs::FreshnessAllInSchema {
            database: "MY_DATABASE".to_string(),
            schema: "PUBLIC".to_string(),
            relations: vec!["MY_DATABASE.PUBLIC.TABLE_A".to_string()],
            warehouse: None,
        };

        assert!(metadata_args_match(&recorded, &actual));
    }
}
