use std::{
    collections::{BTreeMap, HashMap},
    time::SystemTime,
};

use chrono::{DateTime, Utc};
use dbt_adapter::response::AdapterResponse;
use dbt_common::{
    ErrorCode, FsResult, io_args::EvalArgs, stdfs::File, tracing::dbt_emit::emit_warn_log_message,
    warn_error_options::WarnErrorOptions,
};
use dbt_schemas::{
    schemas::{RunResultOutput, RunResultsArgs, RunResultsArtifact, RunResultsMetadata},
    stats::Stats,
};

use crate::generate_run_results;

/// Build a `RunResultsArtifact` from run stats.
///
/// Pass the resolved `DbtState::warn_error`/`warn_error_options`; `EvalArgs` holds only the CLI/env value.
pub fn build_run_results_artifact(
    stats: &Stats,
    adapter_responses: &HashMap<String, AdapterResponse>,
    arg: &EvalArgs,
    warn_error: bool,
    warn_error_options: &WarnErrorOptions,
) -> RunResultsArtifact {
    let now = SystemTime::now();
    let generated_at: DateTime<Utc> = DateTime::from(now);

    let results: Vec<RunResultOutput> = stats
        .stats
        .iter()
        .map(|stat| generate_run_results(stat, stats, adapter_responses).into())
        .collect();

    let total_elapsed_time: f64 = stats
        .stats
        .iter()
        .map(|stat| stat.get_duration().as_secs_f64())
        .sum();

    let metadata = RunResultsMetadata {
        dbt_schema_version: "https://schemas.getdbt.com/dbt/run-results/v6.json".to_string(),
        dbt_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at,
        invocation_id: arg.io.invocation_id.to_string(),
        invocation_started_at: None,
        env: dbt_common::constants::collect_dbt_custom_envs(),
    };

    // Extra CLI args beyond `command`/`which`. These are flattened to the top
    // level of `args` on serialization (via the `__other__` dunder-flatten
    // field), matching dbt-core's flat run_results.json args layout so that
    // `dbt retry` can read them back.
    let mut args_map = BTreeMap::new();
    let command_str = arg.command.as_str();

    args_map.insert(
        "static_analysis".to_string(),
        if let Some(sa) = arg.static_analysis {
            dbt_yaml::Value::string(sa.to_string())
        } else {
            dbt_yaml::Value::null()
        },
    );
    args_map.insert(
        "full_refresh".to_string(),
        dbt_yaml::Value::bool(arg.full_refresh),
    );
    // Always present, matching dbt-core (`{"error": [], "warn": [], "silence": []}` when unset).
    match dbt_yaml::to_value(warn_error_options) {
        Ok(value) => {
            args_map.insert("warn_error_options".to_string(), value);
        }
        Err(e) => emit_warn_log_message(
            ErrorCode::SerializationError,
            format!("Failed to serialize warn_error_options for run_results.json: {e}"),
        ),
    }
    // Omitted when false, matching dbt-core.
    if warn_error {
        args_map.insert("warn_error".to_string(), dbt_yaml::Value::bool(true));
    }

    let args = RunResultsArgs {
        command: command_str.to_string(),
        which: command_str.to_string(),
        __other__: args_map,
    };

    RunResultsArtifact {
        metadata,
        results,
        elapsed_time: total_elapsed_time,
        args,
    }
}

// TODO: We need to add more information to the run_results.json file
pub fn write_run_results_json(
    run_results_artifact: &RunResultsArtifact,
    arg: &EvalArgs,
) -> FsResult<()> {
    let run_results_path = arg.io.out_dir.join("run_results.json");
    let run_results_file = File::create(run_results_path)?;
    // Serialize via dbt_yaml first so the `__other__` dunder-flatten field is
    // flattened into the parent object (raw `serde_json` would emit it as a
    // literal nested `__other__` key, which the reader cannot interpret).
    let yml_val = dbt_yaml::to_value(run_results_artifact).map_err(|e| {
        dbt_common::fs_err!(
            ErrorCode::SerializationError,
            "Failed to serialize run_results: {e}"
        )
    })?;
    serde_json::to_writer(run_results_file, &yml_val)?;
    Ok(())
}

/// Write `run_results.json`, emitting a warning on failure instead of propagating the error.
pub fn write_run_results_json_or_warn(run_results_artifact: &RunResultsArtifact, arg: &EvalArgs) {
    if let Err(e) = write_run_results_json(run_results_artifact, arg) {
        emit_warn_log_message(
            ErrorCode::IoError,
            format!("Failed to write run_results.json: {e}"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_common::warn_error_options::{
        LegacyWarnErrorGroupValue, SupportedLegacyWarnError, WarnErrorOptionValue,
    };
    use dbt_schemas::schemas::RunResultsArtifact;

    fn empty_stats() -> Stats {
        Stats {
            stats: vec![],
            nodes: None,
            batch_results: Default::default(),
            compiled_code: Default::default(),
        }
    }

    /// The args written to run_results.json must be flat at the top level (matching
    /// dbt-core), so `dbt retry` can read them back. Guards against regressing to a raw
    /// `serde_json` write that would emit `__other__` as a literal nested object.
    #[test]
    fn test_run_results_args_round_trip_full_refresh() {
        let tmp = tempfile::tempdir().unwrap();
        let mut arg = EvalArgs::default();
        arg.io.out_dir = tmp.path().to_path_buf();
        arg.full_refresh = true;

        write_run_results_json(
            &build_run_results_artifact(
                &empty_stats(),
                &HashMap::new(),
                &arg,
                false,
                &WarnErrorOptions::default(),
            ),
            &arg,
        )
        .unwrap();

        let artifact = RunResultsArtifact::from_file(&tmp.path().join("run_results.json")).unwrap();
        assert_eq!(
            artifact
                .args
                .__other__
                .get("full_refresh")
                .and_then(|v| v.as_bool()),
            Some(true),
            "full_refresh must round-trip as a flat, readable arg"
        );
    }

    /// Default (empty) `warn_error_options` must still round-trip as all three keys present
    /// but empty, matching dbt-core. `warn_error` must be omitted entirely when false.
    #[test]
    fn test_run_results_args_warn_error_options_default() {
        let tmp = tempfile::tempdir().unwrap();
        let mut arg = EvalArgs::default();
        arg.io.out_dir = tmp.path().to_path_buf();

        write_run_results_json(
            &build_run_results_artifact(
                &empty_stats(),
                &HashMap::new(),
                &arg,
                false,
                &WarnErrorOptions::default(),
            ),
            &arg,
        )
        .unwrap();

        let artifact = RunResultsArtifact::from_file(&tmp.path().join("run_results.json")).unwrap();
        let weo = artifact
            .args
            .__other__
            .get("warn_error_options")
            .expect("warn_error_options must always be present");
        assert_eq!(
            weo.get("error").and_then(|v| v.as_sequence()),
            Some(&vec![])
        );
        assert_eq!(weo.get("warn").and_then(|v| v.as_sequence()), Some(&vec![]));
        assert_eq!(
            weo.get("silence").and_then(|v| v.as_sequence()),
            Some(&vec![])
        );
        assert!(
            !artifact.args.__other__.contains_key("warn_error"),
            "warn_error must be omitted when false, matching dbt-core"
        );
    }

    /// Non-empty `warn_error_options` plus `warn_error: true` must round-trip.
    #[test]
    fn test_run_results_args_warn_error_options_non_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let mut arg = EvalArgs::default();
        arg.io.out_dir = tmp.path().to_path_buf();

        let warn_error_options = WarnErrorOptions {
            error: vec![WarnErrorOptionValue::SupportedLegacy(
                SupportedLegacyWarnError::NoNodesForSelectionCriteria,
            )],
            silence: vec![WarnErrorOptionValue::LegacyGroup(
                LegacyWarnErrorGroupValue::Deprecations,
            )],
            ..Default::default()
        };

        write_run_results_json(
            &build_run_results_artifact(
                &empty_stats(),
                &HashMap::new(),
                &arg,
                true,
                &warn_error_options,
            ),
            &arg,
        )
        .unwrap();

        let artifact = RunResultsArtifact::from_file(&tmp.path().join("run_results.json")).unwrap();
        let weo = artifact
            .args
            .__other__
            .get("warn_error_options")
            .expect("warn_error_options must always be present");
        assert_eq!(
            weo.get("error").and_then(|v| v.as_sequence()),
            Some(&vec![dbt_yaml::Value::string(
                "NoNodesForSelectionCriteria".to_string()
            )])
        );
        assert_eq!(
            weo.get("silence").and_then(|v| v.as_sequence()),
            Some(&vec![dbt_yaml::Value::string("Deprecations".to_string())])
        );
        assert_eq!(
            artifact
                .args
                .__other__
                .get("warn_error")
                .and_then(|v| v.as_bool()),
            Some(true),
            "warn_error must round-trip as true when set"
        );
    }
}
