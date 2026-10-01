use std::path::{Path, PathBuf};

use dbt_common::io_args::{
    ReplayMode, TimeMachineMode, TimeMachineModeKind, TimeMachineRecordConfig,
    TimeMachineReplayConfig,
};
use uuid::Uuid;

use crate::CommonArgs;

/// Find the latest time machine recording in a directory.
///
/// Recordings are stored in subdirectories named with invocation IDs (UUID v7, e.g.,
/// `019384a5-b6c7-7def-8901-234567890abc`). UUID v7 is time-ordered, so sorting
/// by directory name gives us chronological order.
pub fn find_latest_recording(base_dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(base_dir).ok()?;

    let mut recording_dirs: Vec<_> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            // Only consider directories that contain a header.json (valid recordings)
            if path.is_dir() && path.join("header.json").exists() {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    // Sort by directory name (UUID v7 sorts lexicographically in time order)
    recording_dirs.sort_by(|a, b| b.file_name().cmp(&a.file_name()));

    recording_dirs.into_iter().next()
}

/// Determine replay mode based on provided flags.
///
/// Time Machine mode takes precedence if both are specified.
pub fn pick_replay_mode(
    common_args: &CommonArgs,
    invocation_id: Uuid,
    out_dir: &Path,
) -> Option<ReplayMode> {
    match common_args.time_machine_mode {
        Some(TimeMachineModeKind::Record) => {
            let output_path = common_args.time_machine_path.clone().unwrap_or_else(|| {
                // Default to project's target/time_machine directory
                out_dir.join("time_machine")
            });
            // Capture the invocation command for disambiguation in the recording header
            let invocation_command = Some(std::env::args().collect::<Vec<_>>().join(" "));
            Some(ReplayMode::FsTimeMachine(TimeMachineMode::Record(
                TimeMachineRecordConfig {
                    output_path,
                    invocation_id,
                    invocation_command,
                },
            )))
        }
        Some(TimeMachineModeKind::Replay) => {
            let artifact_path = common_args.time_machine_path.clone().unwrap_or_else(|| {
                // Default to the latest recording in target/time_machine/
                let base_dir = out_dir.join("time_machine");
                find_latest_recording(&base_dir).unwrap_or(base_dir)
            });
            Some(ReplayMode::FsTimeMachine(TimeMachineMode::Replay(
                TimeMachineReplayConfig {
                    artifact_path,
                    ordering: common_args.time_machine_ordering,
                },
            )))
        }
        Some(TimeMachineModeKind::Off) | None => {
            // Other record/replay modes
            match (
                &common_args.dbt_replay,
                &common_args.fs_record,
                &common_args.fs_replay,
            ) {
                (Some(dbt_replay), None, None) => {
                    Some(ReplayMode::MantleReplay(dbt_replay.to_path_buf()))
                }
                (None, Some(fs_record), None) => Some(ReplayMode::FsRecord(fs_record.clone())),
                (None, None, Some(fs_replay)) => Some(ReplayMode::FsReplay(fs_replay.clone())),
                _ => None,
            }
        }
    }
}
