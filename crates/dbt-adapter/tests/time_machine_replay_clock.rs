//! Own test binary: `prepare_replay` freezes the process-wide Jinja clock, which must not leak
//! into other tests.

use std::path::Path;

use chrono::Utc;
use dbt_adapter::time_machine::prepare_replay;
use dbt_common::io_args::{ReplayMode, TimeMachineMode, TimeMachineReplayConfig};
use minijinja_contrib::modules::py_datetime::freeze_now;

/// Unfreezes the clock on drop so a failing assertion cannot leak it.
struct UnfreezeOnDrop;

impl Drop for UnfreezeOnDrop {
    fn drop(&mut self) {
        freeze_now(None);
    }
}

const YESTERDAY: &str = "{{ (modules.datetime.datetime.utcnow() - \
     modules.datetime.timedelta(days=1)).strftime('%Y-%m-%d') }}";

fn render(template: &str) -> String {
    let mut env = minijinja::Environment::new();
    minijinja_contrib::add_to_environment(&mut env);
    env.render_str(template, minijinja::context! {}, &[])
        .unwrap()
}

fn recording_dir_with_header(recorded_at: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let header = serde_json::json!({
        "format_version": 1,
        "fusion_version": "test",
        "adapter_type": "databricks",
        "recorded_at": recorded_at,
        "invocation_id": "test-123",
    });
    std::fs::write(dir.path().join("header.json"), header.to_string()).unwrap();
    dir
}

fn replay_of(artifact_path: &Path) -> ReplayMode {
    ReplayMode::FsTimeMachine(TimeMachineMode::Replay(TimeMachineReplayConfig {
        artifact_path: artifact_path.to_path_buf(),
        ..Default::default()
    }))
}

// One test body: the clock is process-wide, so the cases must run sequentially.
#[test]
fn prepare_replay_pins_clock_only_for_time_machine_replay() {
    let _unfreeze = UnfreezeOnDrop;
    let real_today = Utc::now().format("%Y-%m-%d").to_string();

    // No replay mode: the real clock is left alone.
    prepare_replay(None).unwrap();
    assert_eq!(
        render("{{ modules.datetime.datetime.utcnow().strftime('%Y-%m-%d') }}"),
        real_today
    );

    // Any other replay mode: no-op, even for a path with no recording.
    let missing = tempfile::tempdir().unwrap();
    prepare_replay(Some(&ReplayMode::FsReplay(missing.path().to_path_buf()))).unwrap();
    assert_eq!(
        render("{{ modules.datetime.datetime.utcnow().strftime('%Y-%m-%d') }}"),
        real_today
    );

    // Recording made just after midnight UTC: `utcnow() - 1 day` renders the previous day.
    let recording = recording_dir_with_header("2026-10-09T00:00:24.942531241+00:00");
    prepare_replay(Some(&replay_of(recording.path()))).unwrap();
    assert_eq!(render(YESTERDAY), "2026-10-08");
}

#[test]
fn prepare_replay_reports_unreadable_or_invalid_header() {
    let _unfreeze = UnfreezeOnDrop;

    let missing = tempfile::tempdir().unwrap();
    let err = prepare_replay(Some(&replay_of(missing.path()))).unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("Failed to load time machine recording from"),
        "{message}"
    );
    assert!(
        message.contains(&missing.path().display().to_string()),
        "{message}"
    );

    let bad_timestamp = recording_dir_with_header("yesterday");
    let err = prepare_replay(Some(&replay_of(bad_timestamp.path()))).unwrap_err();
    assert!(err.to_string().contains("recorded_at 'yesterday'"), "{err}");
}
