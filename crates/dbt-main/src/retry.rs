//! Retry command implementation for re-running failed nodes from previous executions.

use dbt_clap_core::*;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::{ErrorCode, FsResult, err};
use dbt_schemas::schemas::{BatchResults, RunResultsArtifact};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

/// Statuses that are always retryable. Matches dbt-core's `RETRYABLE_STATUSES`
/// (`error`, `fail`, `skipped`).
pub const RETRYABLE_STATUSES: &[&str] = &["error", "fail", "skipped"];

/// Additional status that is retryable only when the *retry* invocation passes
/// `--warn-error`, matching dbt-core:
/// `if args.warn_error: RETRYABLE_STATUSES.add(NodeStatus.Warn)`.
pub const WARN_ERROR_RETRYABLE_STATUSES: &[&str] = &["warn"];

pub const RETRIABLE_COMMANDS: &[&str] = &[
    "run", "build", "test", "seed", "snapshot", "compile", "check",
];

/// Holds the state extracted from a previous run's run_results.json
/// needed to execute a retry command.
#[derive(Debug)]
pub struct RetryState {
    /// The original command type that was run (e.g., "run", "build", "test")
    pub original_command: String,
    /// List of unique_ids for nodes that should be retried
    pub retryable_node_ids: Vec<String>,
    /// The static analysis setting from the original run, if present
    pub original_static_analysis: Option<StaticAnalysisKind>,
    /// Per-node batch results from the previous run (for overload retry skip)
    pub previous_batch_results: HashMap<String, BatchResults>,
    /// Whether the original run was invoked with --full-refresh
    pub original_full_refresh: bool,
}

/// Decode check names from `check.<package>.<name>` unique_ids.
///
/// Everything after the second `.` is the name, so a name containing a
/// dot survives. Keys that don't fit (e.g. a legacy artifact) drop out;
/// an empty result means "run every discovered check".
pub fn check_names_from_retry_ids(ids: &[String]) -> Vec<String> {
    ids.iter()
        .filter_map(|id| {
            id.strip_prefix("check.")
                .and_then(|rest| rest.split_once('.').map(|(_, name)| name.to_string()))
        })
        .collect()
}

impl RetryState {
    /// Load retry state from a run_results.json file.
    ///
    /// # Arguments
    /// * `path` - Path to the run_results.json file
    /// * `warn_error` - Whether the *retry* invocation passed `--warn-error`.
    ///   When true, nodes recorded as `warn` are also retryable (and will be
    ///   escalated to failures during execution), matching dbt-core which keys
    ///   off the retry invocation's own `--warn-error` flag.
    ///
    /// # Returns
    /// * `Ok(RetryState)` - If the file was parsed and contains retryable nodes
    /// * `Err` - If the file doesn't exist, is invalid, or has no failed nodes
    pub fn from_run_results(path: &Path, warn_error: bool) -> FsResult<Self> {
        let artifact = RunResultsArtifact::from_file(path)?;

        let original_command = artifact.args.which.clone();

        // Parse static analysis setting from args.__other__
        let original_static_analysis = artifact
            .args
            .__other__
            .get("static_analysis")
            .and_then(|v| v.as_str())
            .and_then(|s| StaticAnalysisKind::from_str(s).ok());

        // Parse full_refresh setting from args.__other__ so retry preserves the
        // original run's --full-refresh behavior for incremental models.
        let original_full_refresh = artifact
            .args
            .__other__
            .get("full_refresh")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Collect all retryable nodes: error, fail, skipped (and warn only under
        // --warn-error). The graph infrastructure handles dependency ordering.
        let retryable_node_ids: Vec<String> = artifact
            .results
            .iter()
            .filter(|r| {
                RETRYABLE_STATUSES.contains(&r.status.as_str())
                    || (warn_error && WARN_ERROR_RETRYABLE_STATUSES.contains(&r.status.as_str()))
            })
            // dbt-core skips operation nodes unless the original command was
            // `run-operation`, because on-run-start / on-run-end hooks are attached
            // to the command and re-execute as part of whatever command retry
            // reconstructs — retrying the operation node itself would be wrong.
            // (dbt-labs/fs#12418, core/dbt/task/retry.py.)
            .filter(|r| {
                original_command == "run-operation" || !r.unique_id.starts_with("operation.")
            })
            .map(|r| r.unique_id.clone())
            .collect();

        if retryable_node_ids.is_empty() {
            return err!(
                ErrorCode::Generic,
                "No failed nodes found in run_results.json - nothing to retry"
            );
        }

        let previous_batch_results: HashMap<String, BatchResults> = artifact
            .results
            .iter()
            .filter_map(|r| {
                r.batch_results
                    .as_ref()
                    .map(|br| (r.unique_id.clone(), br.clone()))
            })
            .collect();

        Ok(Self {
            original_command,
            retryable_node_ids,
            original_static_analysis,
            previous_batch_results,
            original_full_refresh,
        })
    }

    /// Convert the original command string to a CoreCommand.
    ///
    /// Returns an Err containing the original command string if it is not supported for retry.
    pub fn to_command(&self, retry_args: &RetryArgs) -> Result<CoreCommand, String> {
        let common_args = retry_args.common_args.clone();

        // Determine effective static analysis setting:
        //
        // 1. Explicit CLI flag (from retry_args) takes highest priority
        // 2. Original run's setting (if present) is used to preserve behavior
        // 3. Otherwise preserve the absence of an explicit setting
        let static_analysis = retry_args.static_analysis.or(self.original_static_analysis);

        // XXX: the RetryState should have enough information to reconstruct
        // the original command with all necessary args, but that's unfortunately
        // not the case yet
        // Preserve the original run's --full-refresh for commands that support it.
        // (test/snapshot have no such flag.)
        let full_refresh = self.original_full_refresh;

        let core_cmd = match self.original_command.as_str() {
            "run" => CoreCommand::Run(RunArgs {
                common_args,
                static_analysis,
                full_refresh,
                ..RunArgs::default()
            }),
            "build" => CoreCommand::Build(BuildArgs {
                common_args,
                static_analysis,
                full_refresh,
                ..BuildArgs::default()
            }),
            "test" => CoreCommand::Test(TestArgs {
                common_args,
                static_analysis,
                ..TestArgs::default()
            }),
            "seed" => CoreCommand::Seed(SeedArgs {
                common_args,
                static_analysis,
                full_refresh,
                ..SeedArgs::default()
            }),
            "snapshot" => CoreCommand::Snapshot(SnapshotArgs {
                common_args,
                static_analysis,
                ..SnapshotArgs::default()
            }),
            "compile" => CoreCommand::Compile(CompileArgs {
                common_args,
                static_analysis,
                full_refresh,
                ..CompileArgs::default()
            }),
            "check" => {
                // Re-run exactly the checks that failed, by name. See
                // `check_names_from_retry_ids`.
                let check_names = check_names_from_retry_ids(&self.retryable_node_ids);
                CoreCommand::Check(CheckArgs {
                    check_names,
                    common_args,
                    static_analysis,
                })
            }
            other => {
                debug_assert!(!RETRIABLE_COMMANDS.contains(&other));
                return Err(other.to_string());
            }
        };
        Ok(core_cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_retryable_statuses_contains_expected() {
        assert!(RETRYABLE_STATUSES.contains(&"error"));
        assert!(RETRYABLE_STATUSES.contains(&"fail"));
        assert!(RETRYABLE_STATUSES.contains(&"skipped"));
        assert!(!RETRYABLE_STATUSES.contains(&"success"));
        assert!(!RETRYABLE_STATUSES.contains(&"pass"));
        // `warn` is retryable only under --warn-error, so it lives in a separate set.
        assert!(!RETRYABLE_STATUSES.contains(&"warn"));
        assert!(WARN_ERROR_RETRYABLE_STATUSES.contains(&"warn"));
    }

    fn cmd_for_retry(
        original_cmd: &str,
        original_sa: Option<StaticAnalysisKind>,
        retry_sa: Option<StaticAnalysisKind>,
    ) -> Result<CoreCommand, String> {
        let state = RetryState {
            original_command: original_cmd.into(),
            retryable_node_ids: vec!["some_node_id".to_string()],
            original_static_analysis: original_sa,
            previous_batch_results: Default::default(),
            original_full_refresh: false,
        };
        let retry_args = RetryArgs {
            common_args: CommonArgs::default(),
            static_analysis: retry_sa,
        };
        state.to_command(&retry_args)
    }

    fn check_cmd_for_retry(
        original_cmd: &str,
        original_sa: Option<StaticAnalysisKind>,
        retry_sa: Option<StaticAnalysisKind>,
    ) {
        let cmd = cmd_for_retry(original_cmd, original_sa, retry_sa).unwrap();
        assert_eq!(cmd.name(), original_cmd);

        let expected_sa = retry_sa.or(original_sa);

        assert_eq!(
            cmd.static_analysis(),
            expected_sa,
            "Failed for command: {original_cmd}, \
original_sa: {original_sa:?}, \
retry_args.sa: {retry_sa:?}, \
expected_sa: {expected_sa:?}",
        );
    }

    #[test]
    fn test_command_for_retry() {
        const SA: [Option<StaticAnalysisKind>; 5] = [
            None,
            Some(StaticAnalysisKind::On),
            Some(StaticAnalysisKind::Strict),
            Some(StaticAnalysisKind::Off),
            Some(StaticAnalysisKind::Unsafe),
        ];
        for cmd in RETRIABLE_COMMANDS {
            for original_sa in SA.iter() {
                for retry_sa in SA.iter() {
                    check_cmd_for_retry(cmd, *original_sa, *retry_sa);
                }
            }
        }
    }

    fn cmd_for_retry_full_refresh(original_cmd: &str, original_full_refresh: bool) -> CoreCommand {
        let state = RetryState {
            original_command: original_cmd.into(),
            retryable_node_ids: vec!["some_node_id".to_string()],
            original_static_analysis: None,
            previous_batch_results: Default::default(),
            original_full_refresh,
        };
        let retry_args = RetryArgs {
            common_args: CommonArgs::default(),
            static_analysis: None,
        };
        state.to_command(&retry_args).unwrap()
    }

    #[test]
    fn test_command_for_retry_preserves_full_refresh() {
        // Commands that support --full-refresh must propagate it from the original run.
        for cmd in &["run", "build", "seed", "compile"] {
            assert!(
                cmd_for_retry_full_refresh(cmd, true).full_refresh(),
                "expected full_refresh=true to be preserved for command: {cmd}"
            );
            assert!(
                !cmd_for_retry_full_refresh(cmd, false).full_refresh(),
                "expected full_refresh=false to be preserved for command: {cmd}"
            );
        }
        // test/snapshot have no --full-refresh flag; they always report false.
        for cmd in &["test", "snapshot"] {
            assert!(!cmd_for_retry_full_refresh(cmd, true).full_refresh());
        }
    }

    #[test]
    fn test_from_run_results_parses_full_refresh() {
        let with_ff = create_run_results_json_with_full_refresh(
            &[("model.my_project.model_a", "error")],
            "build",
            Some(true),
        );
        let state = RetryState::from_run_results(with_ff.path(), false).unwrap();
        assert!(state.original_full_refresh);

        let without_ff = create_run_results_json_with_full_refresh(
            &[("model.my_project.model_a", "error")],
            "build",
            Some(false),
        );
        let state = RetryState::from_run_results(without_ff.path(), false).unwrap();
        assert!(!state.original_full_refresh);

        // Missing full_refresh (e.g. run_results from an older version) defaults to false.
        let file = create_run_results_json(&[("model.my_project.model_a", "error")], "build");
        let state = RetryState::from_run_results(file.path(), false).unwrap();
        assert!(!state.original_full_refresh);
    }

    #[test]
    fn test_retry_check_decodes_check_names_from_recorded_ids() {
        let state = RetryState {
            original_command: "check".into(),
            retryable_node_ids: vec![
                "check.my_project.no_undocumented_models".to_string(),
                // A check name that itself contains a dot must survive decoding.
                "check.my_project.pii.not_leaked".to_string(),
            ],
            original_static_analysis: None,
            previous_batch_results: Default::default(),
            original_full_refresh: false,
        };
        let retry_args = RetryArgs {
            common_args: CommonArgs::default(),
            static_analysis: None,
        };
        match state.to_command(&retry_args).unwrap() {
            CoreCommand::Check(args) => assert_eq!(
                args.check_names,
                vec![
                    "no_undocumented_models".to_string(),
                    "pii.not_leaked".to_string(),
                ]
            ),
            other => panic!("expected Check, got {other:?}"),
        }
    }

    #[test]
    fn test_check_names_from_retry_ids_ignores_non_check_nodes() {
        assert_eq!(
            check_names_from_retry_ids(&[
                "check.pkg.failed_check".to_string(),
                "model.pkg.skipped".to_string(),
                "check.pkg.dot.named".to_string(),
            ]),
            vec!["failed_check".to_string(), "dot.named".to_string()]
        );
    }

    #[test]
    fn test_invalid_command_for_retry() {
        assert_eq!(
            cmd_for_retry("other-command-that-cant-be-retried", None, None).unwrap_err(),
            "other-command-that-cant-be-retried"
        );
    }

    /// Helper to create a run_results.json file for testing
    fn create_run_results_json(results: &[(&str, &str)], which: &str) -> NamedTempFile {
        create_run_results_json_with_sa(results, which, None)
    }

    /// Helper to create a run_results.json file for testing with optional static_analysis
    fn create_run_results_json_with_sa(
        results: &[(&str, &str)],
        which: &str,
        static_analysis: Option<&str>,
    ) -> NamedTempFile {
        let results_json: Vec<String> = results
            .iter()
            .map(|(unique_id, status)| {
                format!(
                    r#"{{"status": "{}", "unique_id": "{}", "timing": [], "thread_id": "Thread-1", "execution_time": 0.1, "adapter_response": {{}}}}"#,
                    status, unique_id
                )
            })
            .collect();

        let sa_part = static_analysis
            .map(|sa| format!(r#", "static_analysis": "{}""#, sa))
            .unwrap_or_default();

        let json = format!(
            r#"{{
                "metadata": {{
                    "dbt_schema_version": "https://schemas.getdbt.com/dbt/run-results/v6.json",
                    "dbt_version": "1.9.0",
                    "generated_at": "2024-01-01T00:00:00Z",
                    "invocation_id": "test-invocation-id",
                    "env": {{}}
                }},
                "results": [{}],
                "elapsed_time": 1.0,
                "args": {{
                    "command": "{}",
                    "which": "{}"{sa_part}
                }}
            }}"#,
            results_json.join(","),
            which,
            which
        );

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(json.as_bytes()).unwrap();
        file
    }

    /// Helper to create a run_results.json file for testing with optional full_refresh
    fn create_run_results_json_with_full_refresh(
        results: &[(&str, &str)],
        which: &str,
        full_refresh: Option<bool>,
    ) -> NamedTempFile {
        let results_json: Vec<String> = results
            .iter()
            .map(|(unique_id, status)| {
                format!(
                    r#"{{"status": "{}", "unique_id": "{}", "timing": [], "thread_id": "Thread-1", "execution_time": 0.1, "adapter_response": {{}}}}"#,
                    status, unique_id
                )
            })
            .collect();

        let ff_part = full_refresh
            .map(|ff| format!(r#", "full_refresh": {}"#, ff))
            .unwrap_or_default();

        let json = format!(
            r#"{{
                "metadata": {{
                    "dbt_schema_version": "https://schemas.getdbt.com/dbt/run-results/v6.json",
                    "dbt_version": "1.9.0",
                    "generated_at": "2024-01-01T00:00:00Z",
                    "invocation_id": "test-invocation-id",
                    "env": {{}}
                }},
                "results": [{}],
                "elapsed_time": 1.0,
                "args": {{
                    "command": "{}",
                    "which": "{}"{ff_part}
                }}
            }}"#,
            results_json.join(","),
            which,
            which
        );

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(json.as_bytes()).unwrap();
        file
    }

    #[test]
    fn test_from_run_results_with_failures() {
        let file = create_run_results_json(
            &[
                ("model.my_project.model_a", "success"),
                ("model.my_project.model_b", "error"),
                ("model.my_project.model_c", "fail"),
                ("model.my_project.model_d", "skipped"),
            ],
            "run",
        );

        let state = RetryState::from_run_results(file.path(), false).unwrap();

        assert_eq!(state.original_command, "run");
        assert_eq!(state.retryable_node_ids.len(), 3);
        assert!(
            state
                .retryable_node_ids
                .contains(&"model.my_project.model_b".to_string())
        );
        assert!(
            state
                .retryable_node_ids
                .contains(&"model.my_project.model_c".to_string())
        );
        assert!(
            state
                .retryable_node_ids
                .contains(&"model.my_project.model_d".to_string())
        );
        // success should NOT be included
        assert!(
            !state
                .retryable_node_ids
                .contains(&"model.my_project.model_a".to_string())
        );
    }

    #[test]
    fn test_from_run_results_all_success_errors() {
        let file = create_run_results_json(
            &[
                ("model.my_project.model_a", "success"),
                ("model.my_project.model_b", "success"),
            ],
            "run",
        );

        let result = RetryState::from_run_results(file.path(), false);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("No failed nodes"));
    }

    #[test]
    fn test_from_run_results_file_not_found() {
        let result =
            RetryState::from_run_results(Path::new("/nonexistent/run_results.json"), false);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_run_results_preserves_command_type() {
        for cmd in &["run", "build", "test", "seed", "snapshot", "compile"] {
            let file = create_run_results_json(&[("model.my_project.model_a", "error")], cmd);

            let state = RetryState::from_run_results(file.path(), false).unwrap();
            assert_eq!(state.original_command, *cmd);
        }
    }

    #[test]
    fn test_from_run_results_gates_warn_status_on_warn_error() {
        // A passing test plus a warning test: the only non-success status is `warn`.
        let make = || {
            create_run_results_json(
                &[
                    ("test.my_project.test_a", "pass"),
                    ("test.my_project.test_b", "warn"),
                ],
                "test",
            )
        };

        // Without --warn-error, `warn` is NOT retryable -> nothing to retry.
        let file = make();
        assert!(
            RetryState::from_run_results(file.path(), false).is_err(),
            "warn-only run must have nothing to retry without --warn-error"
        );

        // With --warn-error, the warned node becomes retryable (the passing one does not).
        let file = make();
        let state = RetryState::from_run_results(file.path(), true).unwrap();
        assert_eq!(
            state.retryable_node_ids,
            vec!["test.my_project.test_b".to_string()],
        );
    }

    #[test]
    fn test_from_run_results_parses_static_analysis_on() {
        let file = create_run_results_json_with_sa(
            &[("model.my_project.model_a", "error")],
            "run",
            Some("on"),
        );

        let state = RetryState::from_run_results(file.path(), false).unwrap();
        assert_eq!(state.original_static_analysis, Some(StaticAnalysisKind::On));
    }

    #[test]
    fn test_from_run_results_parses_static_analysis_off() {
        let file = create_run_results_json_with_sa(
            &[("model.my_project.model_a", "error")],
            "run",
            Some("off"),
        );

        let state = RetryState::from_run_results(file.path(), false).unwrap();
        assert_eq!(
            state.original_static_analysis,
            Some(StaticAnalysisKind::Off)
        );
    }

    #[test]
    fn test_from_run_results_parses_static_analysis_unsafe() {
        let file = create_run_results_json_with_sa(
            &[("model.my_project.model_a", "error")],
            "run",
            Some("unsafe"),
        );

        let state = RetryState::from_run_results(file.path(), false).unwrap();
        assert_eq!(
            state.original_static_analysis,
            Some(StaticAnalysisKind::Unsafe)
        );
    }

    #[test]
    fn test_from_run_results_parses_static_analysis_baseline() {
        let file = create_run_results_json_with_sa(
            &[("model.my_project.model_a", "error")],
            "run",
            Some("baseline"),
        );

        let state = RetryState::from_run_results(file.path(), false).unwrap();
        assert_eq!(
            state.original_static_analysis,
            Some(StaticAnalysisKind::Baseline)
        );
    }

    #[test]
    fn test_from_run_results_no_static_analysis() {
        let file = create_run_results_json(&[("model.my_project.model_a", "error")], "run");

        let state = RetryState::from_run_results(file.path(), false).unwrap();
        assert_eq!(state.original_static_analysis, None);
    }
}
