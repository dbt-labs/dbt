use chrono::{DateTime, Utc};
use dbt_common::FsResult;
use dbt_common::io_args::StaticAnalysisOffReason;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::{collections::BTreeMap, path::Path, sync::Arc};

use crate::schemas::InternalDbtNodeAttributes;

use crate::schemas::legacy_catalog::DbtCatalog;
use crate::schemas::manifest::DbtManifest;
use crate::schemas::serde::typed_struct_from_json_file;
use crate::schemas::sources::FreshnessResultsArtifact;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

/// Metadata about the dbt run invocation.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct RunResultsMetadata {
    pub dbt_schema_version: String,
    pub dbt_version: String,
    pub generated_at: DateTime<Utc>,
    pub invocation_id: String,
    /// Timestamp when the invocation started, if available.
    pub invocation_started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Timing information for a specific phase of a node execution.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TimingInfo {
    pub name: String, // e.g., "compile", "execute"
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Represents the batch results structure within a RunResult.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BatchResults {
    pub successful: Vec<(String, String)>,
    pub failed: Vec<(String, String)>,
}

fn serialize_internal_dbt_node<S>(
    node: &Option<Arc<dyn InternalDbtNodeAttributes>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match node {
        Some(node) => node.serialize_keep_none().serialize(serializer),
        None => serializer.serialize_none(),
    }
}

/// Result object for a single node execution.
///
/// Note: this struct intentionally does *not* use `#[skip_serializing_none]`. dbt Core always
/// emits every per-node result key (with a `null` value when empty), and the `dbt-artifacts`
/// package (among other consumers) relies on keys such as `failures` always being present. The
/// only exception is `static_analysis_off_reason`, a Fusion-only field with no Core counterpart,
/// which we keep omitted when `None`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ContextRunResult {
    /// Final status of the node execution (e.g., "success", "error", "skipped", "pass", "fail").
    pub status: String,
    /// List of timing information for different phases.
    pub timing: Vec<TimingInfo>,
    /// ID of the thread that executed the node.
    pub thread_id: String,
    /// Total execution time for the node in seconds.
    pub execution_time: f64,
    /// Adapter-specific response information.
    pub adapter_response: BTreeMap<String, YmlValue>,
    /// Execution message (e.g., error message).
    pub message: Option<String>,
    /// Information about failures (often used for tests).
    pub failures: Option<i64>,
    /// The Node that was executed
    #[serde(serialize_with = "serialize_internal_dbt_node")]
    pub node: Option<Arc<dyn InternalDbtNodeAttributes>>,
    /// Unique identifier for the dbt node.
    pub unique_id: String,
    /// Results specific to batch processing, if applicable.
    #[serde(default)]
    pub batch_results: Option<BatchResults>,
    /// Compiled SQL code for the node.
    pub compiled_code: Option<String>,
    /// Reason why static analysis was disabled for this node (Fusion-only; omitted when absent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub static_analysis_off_reason: Option<StaticAnalysisOffReason>,
}

impl From<ContextRunResult> for RunResultOutput {
    fn from(result: ContextRunResult) -> Self {
        let (unique_id, relation_name) = match result.node {
            Some(node) => (
                Some(node.common().unique_id.clone()),
                node.base().relation_name.clone(),
            ),
            None => (None, None),
        };

        // Stats is also used for non internal dbtNodes so if its none, we use stat.unique_id
        let unique_id = unique_id.unwrap_or(result.unique_id);

        RunResultOutput {
            status: result.status,
            timing: result.timing,
            thread_id: result.thread_id,
            execution_time: result.execution_time,
            adapter_response: result.adapter_response,
            message: result.message,
            failures: result.failures,
            unique_id,
            compiled: None, // TODO: Handle compiled i think its a deprecated field
            compiled_code: result.compiled_code,
            relation_name,
            batch_results: result.batch_results,
            static_analysis_off_reason: result.static_analysis_off_reason,
        }
    }
}

/// Result object for a single node execution.
///
/// Note: this struct intentionally does *not* use `#[skip_serializing_none]`. dbt Core always
/// emits every per-node result key (with a `null` value when empty), and the `dbt-artifacts`
/// package (among other consumers) relies on keys such as `failures` always being present. The
/// only exception is `static_analysis_off_reason`, a Fusion-only field with no Core counterpart,
/// which we keep omitted when `None`. The `#[serde(default)]` on the optional fields keeps
/// deserialization tolerant of older/partial artifacts that may omit a key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RunResultOutput {
    /// Final status of the node execution (e.g., "success", "error", "skipped", "pass", "fail").
    pub status: String,
    /// List of timing information for different phases.
    pub timing: Vec<TimingInfo>,
    /// ID of the thread that executed the node.
    pub thread_id: String,
    /// Total execution time for the node in seconds.
    pub execution_time: f64,
    /// Adapter-specific response information.
    pub adapter_response: BTreeMap<String, YmlValue>,
    /// Execution message (e.g., error message).
    #[serde(default)]
    pub message: Option<String>,
    /// Information about failures (often used for tests).
    #[serde(default)]
    pub failures: Option<i64>,
    /// Unique identifier for the dbt node.
    pub unique_id: String,
    /// Indicates if the node was compiled.
    #[serde(default)]
    pub compiled: Option<bool>,
    /// Compiled SQL code for the node.
    #[serde(default)]
    pub compiled_code: Option<String>,
    /// Fully qualified relation name in the database.
    #[serde(default)]
    pub relation_name: Option<String>,
    /// Results specific to batch processing, if applicable.
    #[serde(default)]
    pub batch_results: Option<BatchResults>,
    /// Reason why static analysis was disabled for this node (Fusion-only; omitted when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub static_analysis_off_reason: Option<StaticAnalysisOffReason>,
}

impl RunResultOutput {
    /// A result row synthesized outside the task graph (e.g. a parse-time check, or a node
    /// reported `skipped` because one failed) — every per-node field that only a real task
    /// execution could populate (timing, compiled SQL, batch results, …) is left at its
    /// "nothing ran" value.
    pub fn synthetic(
        unique_id: String,
        status: impl Into<String>,
        message: Option<String>,
        failures: Option<i64>,
    ) -> Self {
        Self {
            status: status.into(),
            timing: Vec::new(),
            thread_id: "main".to_string(),
            execution_time: 0.0,
            adapter_response: BTreeMap::new(),
            message,
            failures,
            unique_id,
            compiled: None,
            compiled_code: None,
            relation_name: None,
            batch_results: None,
            static_analysis_off_reason: None,
        }
    }
}

/// Arguments passed to the dbt command.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RunResultsArgs {
    /// The specific dbt command executed (e.g., "run", "test").
    pub command: String,
    /// Alias for the command executed.
    pub which: String,
    /// Capture any other arguments passed via CLI using flatten
    pub __other__: BTreeMap<String, YmlValue>,
}

/// Represents the structure of the run_results.json artifact.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RunResultsArtifact {
    /// Metadata about the dbt invocation.
    pub metadata: RunResultsMetadata,
    /// List of results for each executed node.
    pub results: Vec<RunResultOutput>,
    /// Total elapsed time for the entire dbt invocation in seconds.
    pub elapsed_time: f64,
    /// Arguments passed to the dbt command.
    pub args: RunResultsArgs,
}

impl RunResultsArtifact {
    pub fn from_file(path: &Path) -> FsResult<Self> {
        typed_struct_from_json_file(path)
    }
}

/// In-memory result of dbt command invocation.
///
/// Maybe partially populated depending on the command and if execution reached
/// the point of producing the artifacts.
#[derive(Default)]
pub struct DbtCommandExecutionArtifacts {
    pub manifest: Option<DbtManifest>,
    pub run_results: Option<RunResultsArtifact>,
    pub catalog: Option<DbtCatalog>,
    /// `source freshness`'s results, as written to `sources.json`.
    pub sources: Option<FreshnessResultsArtifact>,
    /// `list`'s selected nodes in selector format.
    pub list_items: Option<Vec<String>>,
    /// Rendered message of a real (non-exit-status) error, captured before it is
    /// flattened to a bare exit status for CLI callers. Embedders surface this;
    /// the diagnostics also went to the log either way.
    pub error_message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Build a `RunResultOutput` representing a skipped node: every optional per-node field is
    /// `None`, mirroring what Fusion produces for a skipped test/model (e.g. during Selective
    /// Apply Optimization model reuse).
    pub fn skipped_run_result_output() -> RunResultOutput {
        RunResultOutput {
            status: "skipped".to_string(),
            timing: vec![],
            thread_id: "Thread-1".to_string(),
            execution_time: 0.0,
            adapter_response: BTreeMap::new(),
            message: None,
            failures: None,
            unique_id: "test.test.not_null_view_model_id.c9346154f2".to_string(),
            compiled: None,
            compiled_code: None,
            relation_name: None,
            batch_results: None,
            static_analysis_off_reason: None,
        }
    }

    /// Regression for dbt-core#14554: a skipped node's `run_results.json` entry must include the
    /// full set of per-node keys that dbt Core emits, each present with a `null` value when
    /// empty (rather than being dropped by `#[skip_serializing_none]`). Downstream consumers such
    /// as the `dbt-artifacts` package rely on keys like `failures` always being present.
    #[test]
    fn test_run_result_output_serializes_core_keys_as_null_for_skipped_node() {
        let value = serde_json::to_value(skipped_run_result_output()).unwrap();
        let obj = value.as_object().expect("expected a JSON object");

        for key in [
            "message",
            "failures",
            "compiled",
            "compiled_code",
            "relation_name",
            "batch_results",
        ] {
            assert!(obj.contains_key(key), "key `{key}` should be present");
            assert_eq!(
                obj[key],
                Value::Null,
                "key `{key}` should serialize as null"
            );
        }

        // Fusion-only field with no Core counterpart stays omitted when absent.
        assert!(
            !obj.contains_key("static_analysis_off_reason"),
            "Fusion-only `static_analysis_off_reason` should be omitted when None",
        );
    }

    /// The Jinja-facing `ContextRunResult` (surfaced to the `on-run-end` `results` collection,
    /// which `dbt-artifacts` consumes) must expose the same keys as `null`.
    #[test]
    fn test_context_run_result_serializes_failures_as_null_for_skipped_node() {
        let context_result = ContextRunResult {
            status: "skipped".to_string(),
            timing: vec![],
            thread_id: "Thread-1".to_string(),
            execution_time: 0.0,
            adapter_response: BTreeMap::new(),
            message: None,
            failures: None,
            node: None,
            unique_id: "test.test.not_null_view_model_id.c9346154f2".to_string(),
            batch_results: None,
            compiled_code: None,
            static_analysis_off_reason: None,
        };

        let value = serde_json::to_value(&context_result).unwrap();
        let obj = value.as_object().expect("expected a JSON object");

        assert!(
            obj.contains_key("failures"),
            "`failures` key should be present"
        );
        assert_eq!(
            obj["failures"],
            Value::Null,
            "`failures` should serialize as null"
        );
        assert_eq!(
            obj["message"],
            Value::Null,
            "`message` should serialize as null"
        );
        assert!(
            !obj.contains_key("static_analysis_off_reason"),
            "Fusion-only `static_analysis_off_reason` should be omitted when None",
        );
    }

    /// Deserialization must remain tolerant of artifacts that omit the now-always-serialized
    /// optional keys (e.g. artifacts produced before this fix), thanks to `#[serde(default)]`.
    #[test]
    fn test_run_result_output_deserializes_with_missing_optional_keys() {
        let json = serde_json::json!({
            "status": "skipped",
            "timing": [],
            "thread_id": "Thread-1",
            "execution_time": 0.0,
            "adapter_response": {},
            "unique_id": "test.test.not_null_view_model_id.c9346154f2"
        });

        let result: RunResultOutput = serde_json::from_value(json).unwrap();
        assert_eq!(result.status, "skipped");
        assert_eq!(result.failures, None);
        assert_eq!(result.message, None);
        assert_eq!(result.compiled, None);
        assert_eq!(result.compiled_code, None);
        assert_eq!(result.relation_name, None);
        assert!(result.batch_results.is_none());
        assert!(result.static_analysis_off_reason.is_none());
    }
}
