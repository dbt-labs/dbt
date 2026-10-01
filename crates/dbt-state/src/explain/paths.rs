use std::path::{Path, PathBuf};

use dbt_common::{ErrorCode, FsResult, constants::DBT_LOG_DIR_NAME, fs_err};

use crate::service_config::{DEFAULT_LOG_PREFIX, RunCacheServiceConfig};

use super::types::StateExplainOptions;

pub(super) fn state_explain_log_config_from_env() -> RunCacheServiceConfig {
    state_explain_log_config_from_getter(|name| std::env::var(name).ok())
}

pub(super) fn state_explain_log_config_from_getter<F>(mut get_env: F) -> RunCacheServiceConfig
where
    F: FnMut(&str) -> Option<String>,
{
    let mut config = RunCacheServiceConfig::disabled();
    if let Some(value) = get_env("RUN_CACHE_LOG_DIR_OVERRIDE").filter(|value| !value.is_empty()) {
        config.log_dir_override = Some(value);
    }
    if let Some(value) = get_env("RUN_CACHE_LOG_PREFIX").filter(|value| !value.is_empty()) {
        config.log_prefix = value;
    }
    config
}
pub fn new_state_explain_log_path(
    project_dir: &Path,
    log_path: Option<&Path>,
    config: &RunCacheServiceConfig,
) -> PathBuf {
    let timestamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
    decision_log_dir_for_inputs(project_dir, log_path, config).join(format!(
        "{}{}-{}.jsonl",
        log_prefix(config),
        timestamp,
        uuid::Uuid::new_v4()
    ))
}
/// Best-effort retention for the run cache log directory holding `new_log_path`:
/// keeps the newest `config.log_file_limit` logs and deletes the rest. A non-positive
/// limit disables pruning.
pub fn prune_state_explain_logs(new_log_path: &Path, config: &RunCacheServiceConfig) {
    let Ok(limit) = usize::try_from(config.log_file_limit) else {
        return;
    };
    if limit == 0 {
        return;
    }
    let Some(log_dir) = new_log_path.parent() else {
        return;
    };

    let files = state_explain_log_files(log_dir, log_prefix(config));
    for path in files.iter().take(files.len().saturating_sub(limit)) {
        if let Err(err) = std::fs::remove_file(path) {
            tracing::warn!(
                "Failed to prune dbt State explain log {}: {err}",
                path.display()
            );
        }
    }
}

pub(super) fn resolve_log_file(
    options: &StateExplainOptions,
    config: &RunCacheServiceConfig,
) -> FsResult<Option<PathBuf>> {
    let log_dir = decision_log_dir(options, config);

    if let Some(log_file) = &options.log_file {
        let path = if log_file.is_absolute() || log_file.exists() {
            log_file.clone()
        } else {
            log_dir.join(log_file)
        };
        return if path.exists() {
            Ok(Some(path))
        } else {
            Err(fs_err!(
                ErrorCode::InvalidArgument,
                "Log file not found: {}",
                path.display()
            ))
        };
    }

    Ok(newest_log_file(&log_dir, &config.log_prefix))
}

fn decision_log_dir(options: &StateExplainOptions, config: &RunCacheServiceConfig) -> PathBuf {
    decision_log_dir_for_inputs(&options.project_dir, options.log_path.as_deref(), config)
}

fn decision_log_dir_for_inputs(
    project_dir: &Path,
    log_path: Option<&Path>,
    config: &RunCacheServiceConfig,
) -> PathBuf {
    if let Some(override_dir) = config.log_dir_override.as_deref() {
        return expand_tilde(override_dir);
    }

    let project_dir = absolute_path(project_dir);
    let log_root = log_path
        .map(|path| absolute_from(&project_dir, path))
        .unwrap_or_else(|| project_dir.join(DBT_LOG_DIR_NAME));
    log_root.join("run_cache")
}

fn newest_log_file(log_dir: &Path, log_prefix: &str) -> Option<PathBuf> {
    let prefix = if log_prefix.is_empty() {
        DEFAULT_LOG_PREFIX
    } else {
        log_prefix
    };
    state_explain_log_files(log_dir, prefix).pop()
}

/// Log names embed a fixed-width UTC timestamp, so a lexicographic sort is
/// chronological: oldest first.
fn state_explain_log_files(log_dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(log_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::warn!(
                "Failed to read dbt State explain log directory {}: {err}",
                log_dir.display()
            );
            return Vec::new();
        }
    };
    let mut files: Vec<_> = entries
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry.path()),
            Err(err) => {
                tracing::warn!(
                    "Failed to read an entry in dbt State explain log directory {}: {err}",
                    log_dir.display()
                );
                None
            }
        })
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".jsonl"))
        })
        .collect();
    files.sort();
    files
}

fn log_prefix(config: &RunCacheServiceConfig) -> &str {
    if config.log_prefix.is_empty() {
        DEFAULT_LOG_PREFIX
    } else {
        &config.log_prefix
    }
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|err| {
                tracing::warn!(
                    "Failed to resolve the current directory, resolving {} against '.': {err}",
                    path.display()
                );
                PathBuf::from(".")
            })
            .join(path)
    }
}

fn absolute_from(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn expand_tilde(path: &str) -> PathBuf {
    let Some(suffix) = path.strip_prefix("~/") else {
        return PathBuf::from(path);
    };
    match dirs::home_dir() {
        Some(home) => home.join(suffix),
        None => {
            tracing::warn!("Failed to resolve the home directory while expanding '{path}'");
            PathBuf::from(path)
        }
    }
}
