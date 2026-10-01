use std::{
    io::Write,
    path::Path,
    sync::{LazyLock, Mutex},
};

use dbt_common::{ErrorCode, FsResult, fs_err};

use super::types::{
    STATE_EXPLAIN_RECORD_VERSION, StateExplainLog, StateExplainLogRecord, StateExplainRecord,
    StateExplainRunStart,
};

static STATE_EXPLAIN_LOG_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub(super) struct StateExplainInput {
    pub records: Vec<StateExplainRecord>,
    pub run_start: Option<StateExplainRunStart>,
}

pub fn read_explain_records(path: &Path) -> FsResult<Vec<StateExplainRecord>> {
    std::fs::read_to_string(path)
        .map_err(|err| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to read {}: {err}",
                path.display()
            )
        })?
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(idx, line)| {
            let record: StateExplainRecord = serde_json::from_str(line).map_err(|err| {
                fs_err!(
                    ErrorCode::InvalidArgument,
                    "Invalid dbt State explain record at {}:{}: {err}",
                    path.display(),
                    idx + 1
                )
            })?;
            if record.version == STATE_EXPLAIN_RECORD_VERSION {
                Ok(record)
            } else {
                Err(fs_err!(
                    ErrorCode::InvalidArgument,
                    "Unsupported dbt State explain record version {} at {}:{}",
                    record.version,
                    path.display(),
                    idx + 1
                ))
            }
        })
        .collect()
}

/// Read a Fusion-native dbt State explain log from a JSONL file.
pub fn read_state_explain_log(path: &Path) -> FsResult<StateExplainLog> {
    let mut output = StateExplainLog::default();
    for (idx, line) in std::fs::read_to_string(path)
        .map_err(|err| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to read {}: {err}",
                path.display()
            )
        })?
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
    {
        let record: StateExplainLogRecord = serde_json::from_str(line).map_err(|err| {
            fs_err!(
                ErrorCode::InvalidArgument,
                "Invalid dbt State explain log record at {}:{}: {err}",
                path.display(),
                idx + 1
            )
        })?;
        match record {
            StateExplainLogRecord::RunStart(run_start) => output.run_start = Some(run_start),
            StateExplainLogRecord::Node(node) => output.nodes.push(node),
        }
    }
    Ok(output)
}

/// Append one Fusion-native dbt State explain record to a JSONL log file.
pub fn append_state_explain_log_record(
    path: &Path,
    record: &StateExplainLogRecord,
) -> FsResult<()> {
    let mut line = Vec::new();
    serde_json::to_writer(&mut line, record).map_err(|err| {
        fs_err!(
            ErrorCode::InvalidArgument,
            "Failed to serialize dbt State explain record: {err}"
        )
    })?;
    line.push(b'\n');

    let _guard = STATE_EXPLAIN_LOG_WRITE_LOCK.lock().map_err(|err| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to lock dbt State explain log writer: {err}"
        )
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to create {}: {err}",
                parent.display()
            )
        })?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to open {}: {err}",
                path.display()
            )
        })?;
    file.write_all(&line).map_err(|err| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to write {}: {err}",
            path.display()
        )
    })
}

/// Build a new auto-discoverable dbt State explain log path for a run.
#[cfg(test)]
pub(super) fn read_records_for_state_explain(path: &Path) -> FsResult<Vec<StateExplainRecord>> {
    Ok(read_input_for_state_explain(path)?.records)
}

pub(super) fn read_input_for_state_explain(path: &Path) -> FsResult<StateExplainInput> {
    if log_uses_structured_schema(path)? {
        let log = read_state_explain_log(path)?;
        let run_start = log.run_start.clone();
        Ok(StateExplainInput {
            records: fallback_records_from_log(log)?,
            run_start,
        })
    } else {
        Ok(StateExplainInput {
            records: read_explain_records(path)?,
            run_start: None,
        })
    }
}

pub(super) fn fallback_records_from_log(log: StateExplainLog) -> FsResult<Vec<StateExplainRecord>> {
    if log.run_start.is_none() {
        return Err(fs_err!(
            ErrorCode::InvalidArgument,
            "Log file does not contain a run start entry - the file may be empty, corrupt, or in an older format."
        ));
    }

    Ok(log
        .nodes
        .into_iter()
        .map(StateExplainRecord::fallback_from_node)
        .collect())
}

fn log_uses_structured_schema(path: &Path) -> FsResult<bool> {
    let contents = std::fs::read_to_string(path).map_err(|err| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to read {}: {err}",
            path.display()
        )
    })?;
    let Some((idx, line)) = contents
        .lines()
        .enumerate()
        .find(|(_, line)| !line.trim().is_empty())
    else {
        return Ok(true);
    };
    let value: serde_json::Value = serde_json::from_str(line).map_err(|err| {
        fs_err!(
            ErrorCode::InvalidArgument,
            "Invalid dbt State explain log record at {}:{}: {err}",
            path.display(),
            idx + 1
        )
    })?;
    Ok(value.get("entry_type").is_some())
}
