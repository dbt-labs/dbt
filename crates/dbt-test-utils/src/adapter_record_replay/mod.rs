pub mod fs_runner;

use self::fs_runner::FsRunner;

use crate::{
    is_update_mode_with_warehouse, random_schema, should_strip_query_comments,
    task::{HydrateProfilesTask, OverrideFileTask, ProjectEnv, SedTask, TaskSeq},
};
use dbt_common::{FsResult, stdfs};
use dbt_test_primitives::cleanup_schema_name;
use std::path::PathBuf;

const DROP_DEV_SCHEMA_MACRO: &str = include_str!("drop_dev_schema.sql");
const DROP_DEV_SCHEMA_FILENAME: &str = "_drop_dev_schema.sql";

fn command_supports_static_analysis(command: &str) -> bool {
    matches!(
        command.split_whitespace().next(),
        Some("compile" | "run" | "build" | "test" | "seed" | "snapshot" | "show")
    )
}

/// A file substitution to apply before a run step.
pub struct FileOverride {
    /// Relative path within the project dir to override.
    pub original: &'static str,
    /// Relative path from the test project root to the overriding file.
    pub overriding_file: &'static str,
}

/// A single record-replay command to run.
#[derive(Clone)]
pub struct AdapterRecordReplayTurn {
    /// The command string to run (`run --select m1`, `parse`).
    pub command: &'static str,
    /// File overrides to apply before this command runs.
    pub file_overrides: &'static [FileOverride],
}

impl AdapterRecordReplayTurn {
    pub fn new(command: &'static str, file_overrides: &'static [FileOverride]) -> Self {
        Self {
            command,
            file_overrides,
        }
    }
}

/// Builds a turn with no YAML overrides. `command` must be `'static` (for example a string literal).
impl From<&'static str> for AdapterRecordReplayTurn {
    fn from(command: &'static str) -> Self {
        Self {
            command,
            file_overrides: &[],
        }
    }
}

/// Run one or more `fs` commands in record-or-replay mode.
///
/// Multi-command tests get per-run subdirectories (`run_0/`, `run_1/`, …)
/// so that each command's recordings are isolated. Single-command tests
/// record directly into the golden directory.
pub async fn build_fs_record_replay_task_seq<T>(
    root: &str,
    runner: &FsRunner,
    test_name: &str,
    project_path: &str,
    dialect: &str,
    override_schema_name: Option<String>,
    commands: &[T],
    additional_flags: Option<String>,
) -> FsResult<(ProjectEnv, TaskSeq)>
where
    T: Into<AdapterRecordReplayTurn> + Clone,
{
    let commands: Vec<AdapterRecordReplayTurn> = commands.iter().cloned().map(Into::into).collect();
    let is_full_update = is_update_mode_with_warehouse();
    let dialect = dialect.to_lowercase();

    let env = ProjectEnv::mutable_from(root, project_path, false)?;
    let mut task_seq = TaskSeq::new(test_name);
    let task_seq_name: String = task_seq.name().to_string();

    let golden_recordings_path = PathBuf::from(root)
        .join(project_path)
        .join("golden")
        .join(&dialect);

    // Clear existing goldies and prep goldie output dir
    if is_full_update && golden_recordings_path.exists() {
        stdfs::remove_dir_all(&golden_recordings_path)?;
    }
    stdfs::create_dir_all(&golden_recordings_path)?;

    // Schema Pt 1
    let schema_name = match override_schema_name.clone() {
        Some(override_schema_name) => override_schema_name,
        None => {
            if is_full_update {
                random_schema(&task_seq_name)
            } else {
                task_seq_name.clone()
            }
        }
    };

    task_seq.task(Box::new(HydrateProfilesTask {
        schema: schema_name.to_string(),
        target: dialect.to_string(),
    }));

    let multi_command = commands.len() > 1;

    // Build recording paths: run_N/ subdirectories for multi-command tests,
    // the golden directory itself for single-command tests.
    let mut recording_paths = Vec::new();
    for (i, turn) in commands.iter().enumerate() {
        let command = turn.command;
        let rec_path = if multi_command {
            let run_path = golden_recordings_path.join(format!("run_{i}"));
            stdfs::create_dir_all(&run_path)?;
            run_path
        } else {
            golden_recordings_path.clone()
        };

        let mut fs_flag = if is_full_update {
            format!("--fs-record {}", rec_path.display())
        } else {
            format!("--fs-replay {}", rec_path.display())
        };
        if let Some(flags) = &additional_flags {
            fs_flag.push_str(&format!(" {flags}"));
        }
        if let Some(mode) = runner.default_static_analysis
            && command_supports_static_analysis(command)
            && !fs_flag.contains("--static-analysis")
        {
            fs_flag.push_str(&format!(" --static-analysis {mode}"));
        }

        for file_override in turn.file_overrides {
            let source = PathBuf::from(root)
                .join(project_path)
                .join(file_override.overriding_file);
            let target = PathBuf::from(file_override.original);
            task_seq.task(Box::new(OverrideFileTask { source, target }));
        }

        fs_runner::fs(
            &mut task_seq,
            runner,
            &format!(
                "{} --target {} {} --show progress",
                command,
                dialect.to_lowercase(),
                fs_flag
            ),
        );

        recording_paths.push(rec_path);
    }

    // Drop ephemeral schemas from the remote warehouse after recording.
    if is_full_update {
        let macro_abs_path = PathBuf::from(root)
            .join(project_path)
            .join("macros")
            .join(DROP_DEV_SCHEMA_FILENAME);

        let write_path = macro_abs_path.clone();
        task_seq.task_fn(move |_project_env, _test_env, _idx| {
            if let Some(parent) = write_path.parent() {
                stdfs::create_dir_all(parent)?;
            }
            stdfs::write(&write_path, DROP_DEV_SCHEMA_MACRO)?;
            Ok(())
        });

        fs_runner::fs_no_golden(
            &mut task_seq,
            runner,
            &format!("run-operation drop_dev_schema --target {}", dialect),
        );

        let rm_path = macro_abs_path;
        task_seq.task_fn(move |_project_env, _test_env, _idx| {
            if rm_path.exists() {
                stdfs::remove_file(&rm_path)?;
            }
            Ok(())
        });
    }

    // Schema Pt 2 — SedTask for each recording directory
    if is_full_update && override_schema_name.is_none() {
        for rec_path in &recording_paths {
            task_seq.task(Box::new(SedTask {
                from: schema_name.to_string(),
                to: task_seq_name.to_string(),
                dir: Some(rec_path.clone()),
                strip_comments: should_strip_query_comments(),
            }));
        }
    }

    // Schema Pt 3
    if is_full_update {
        let cleaned_schema_name = cleanup_schema_name(&schema_name);
        for rec_path in &recording_paths {
            task_seq.task(Box::new(SedTask {
                from: schema_name.clone(),
                to: cleaned_schema_name.clone(),
                dir: Some(rec_path.clone()),
                strip_comments: should_strip_query_comments(),
            }));
        }
    }

    Ok((env, task_seq))
}

#[allow(clippy::too_many_arguments)]
pub async fn run_fs_record_replay_task_seq<T>(
    root: &str,
    runner: &FsRunner,
    test_name: &str,
    project_path: &str,
    dialect: &str,
    override_schema_name: Option<String>,
    commands: &[T],
    additional_flags: Option<String>,
    set_env: &[(&str, &str)],
) -> FsResult<()>
where
    T: Into<AdapterRecordReplayTurn> + Clone,
{
    let (env, task_seq) = build_fs_record_replay_task_seq(
        root,
        runner,
        test_name,
        project_path,
        dialect,
        override_schema_name,
        commands,
        additional_flags,
    )
    .await?;

    task_seq.execute_in_with_env(&env, set_env).await?;

    Ok(())
}

pub async fn run_dbt_replay_task_seq(
    root: &str,
    runner: &FsRunner,
    test_name: &str,
    project_path: &str,
    command: &str,
    recording_file: &str,
) -> FsResult<()> {
    let env = ProjectEnv::mutable_from(root, project_path, false)?;
    let mut task_seq = TaskSeq::new(test_name);
    fs_runner::fs(
        &mut task_seq,
        runner,
        &format!("{command} --dbt-replay {recording_file} --no-send-anonymous-usage-stats"),
    );
    task_seq.execute_in(&env).await?;
    Ok(())
}
