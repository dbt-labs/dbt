use crate::io_args::{EvalArgs, Phases, StaticAnalysisKind, StaticAnalysisOffReason};
use crate::path::DbtPath;
use crate::stdfs::File;
use crate::tracing::dbt_metrics::{error_count_checkpoint, return_exit_code_from_error_counter};
use crate::{ErrorCode, FsResult, err, fs_err, stdfs::canonicalize};
use dbt_telemetry::{ExecutionPhase, NodeOutcome};
use dbt_yaml::Span;
use pathdiff::diff_paths;
use std::{
    any::Any,
    env,
    ffi::OsStr,
    io::Read,
    path::{Path, PathBuf},
};

/// A trait for reporting execution status to an external client.
pub trait StatusReporter: Any + Send + Sync {
    fn collect_node_evaluation(
        &self,
        unique_id: &str,
        execution_phase: ExecutionPhase,
        node_outcome: NodeOutcome,
        upstream_target: Option<(String, String, bool)>,
        static_analysis: StaticAnalysisKind,
        static_analysis_off_reason: (Option<StaticAnalysisOffReason>, Span),
    );
    /// Called to show progress in the UI
    fn show_progress(&self, action: &str, target: &str, description: Option<&str>);
    fn bulk_publish_empty(&self, file_paths: Vec<DbtPath>);
}

/// Reads the contents of a file as a string.
pub fn try_read_yml_to_str(path: &Path) -> FsResult<String> {
    let mut file = File::open(path).map_err(|e| {
        fs_err!(
            ErrorCode::IoError,
            "Cannot open file {}: {}",
            path.display(),
            e
        )
    })?;
    let mut data = String::new();
    file.read_to_string(&mut data).map_err(|e| {
        fs_err!(
            ErrorCode::IoError,
            "Cannot read file {}: {}",
            path.display(),
            e
        )
    })?;
    Ok(data)
}

pub fn determine_project_dir(inputs: &[String], project_file: &str) -> FsResult<PathBuf> {
    // start the search at
    // - the current directory (the default) or
    // - the directory of the first sql file
    // - the directory of the first workspace.sdf file
    let mut search_start = env::current_dir()?;

    if let Some(input) = inputs.iter().next() {
        let input_path = Path::new(&input);
        if input_path.is_file()
            && (is_allowed_extension_for_project(input_path)
                || input_path.file_name() == Some(OsStr::new(project_file)))
        {
            match canonicalize(input_path) {
                Ok(path_buf) => {
                    search_start = path_buf.parent().unwrap().to_path_buf();
                }
                Err(_) => {
                    return err!(
                        ErrorCode::IoError,
                        "Input file '{input}' not found; make sure that it exists under the provided path"
                    );
                }
            }
        } else if input_path.is_dir() {
            match canonicalize(input_path) {
                Ok(path_buf) => {
                    search_start = path_buf;
                }
                Err(_) => {
                    return err!(
                        ErrorCode::IoError,
                        "Input dir '{input}' not found; make sure that it exists under the provided path"
                    );
                }
            }
        }
    }

    let working_dir = find_path(&search_start, Path::new(project_file));
    match working_dir {
        None => {
            if search_start == env::current_dir()? {
                // check whether the inputs had a path among them, then that is the problem
                if !inputs.is_empty() {
                    err!(
                        ErrorCode::IoError,
                        "Invalid value '{}' for <TARGETS>: Please pass a path that points to or into a dbt project dir\nHint: pass a dbt project dir via `--project-dir <path>` or run `dbt init` to scaffold a project",
                        inputs[0]
                    )
                } else {
                    err!(
                        ErrorCode::IoError,
                        "The current dir is not a dbt project dir\nHint: `cd` into it; pass a <path> to it via `--project-dir <path>`; or run `dbt init` to scaffold a project"
                    )
                }
            } else {
                let relative_path =
                    diff_paths(search_start, env::current_dir()?).unwrap_or(env::current_dir()?);
                err!(
                    ErrorCode::IoError,
                    "Invalid value '{}' for <TARGETS>: Please pass a path that points to or into a dbt project dir\nHint: pass a dbt project dir via `--project-dir <path>` or run `dbt init` to scaffold a project",
                    relative_path.display()
                )
            }
        }
        Some(working_dir) => Ok(working_dir),
    }
}

pub fn find_path(starting_directory: &Path, file: &Path) -> Option<PathBuf> {
    if let Some(path) = find_file(starting_directory, file) {
        let mut tmp: PathBuf = path;
        tmp.pop();
        Some(tmp)
    } else {
        None
    }
}
pub fn find_file(starting_directory: &Path, file: &Path) -> Option<PathBuf> {
    let mut path: PathBuf = starting_directory.into();
    loop {
        path.push(file);
        if path.is_file() {
            break canonicalize(path).ok();
        }
        if !(path.pop() && path.pop()) {
            // remove file && remove parent
            break None;
        }
    }
}
pub const YML_EXT: &str = "yml";
pub const SQL_EXT: &str = "sql";
pub const JSON_EXT: &str = "json";
pub const CSV_EXT: &str = "csv";

pub fn is_allowed_extension_for_project(input_path: &Path) -> bool {
    let extension = input_path.extension().unwrap();
    extension == SQL_EXT || extension == JSON_EXT || extension == YML_EXT
}

pub fn and_n_others(n: usize, items: &[impl ToString]) -> String {
    if items.len() > n {
        format!(
            "{} and {} others",
            items
                .iter()
                .take(n)
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            items.len() - n
        )
    } else {
        items
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub fn checkpoint_maybe_exit(arg: &EvalArgs, phase: Phases) -> FsResult<()> {
    if arg.skip_checkpoints {
        return Ok(());
    }

    if arg.phase <= phase {
        Err(return_exit_code_from_error_counter())
    } else {
        error_count_checkpoint()
    }
}

pub fn checkpoint_error_count_maybe_exit(arg: &EvalArgs) -> FsResult<()> {
    if arg.skip_checkpoints {
        return Ok(());
    }

    error_count_checkpoint()
}
