use std::sync::Arc;

use dbt_test_utils::task::{ExecuteOnly, ProjectEnv, TaskSeq};

use crate::common::{make_fs_cmd_vec, make_fs_cmd_vec_with_version_check, make_fs_command_fn};

const PROJECT: &str = "tests/data/require_dbt_version";

struct ParseRun {
    exit_code: i32,
    output: String,
}

struct ParseRuns {
    warn: ParseRun,
    bypass: ParseRun,
}

static PARSES: tokio::sync::OnceCell<ParseRuns> = tokio::sync::OnceCell::const_new();

fn captured(task: &ExecuteOnly) -> ParseRun {
    ParseRun {
        exit_code: task.get_exit_code(),
        output: format!("{}{}", task.get_stdout(), task.get_stderr()),
    }
}

async fn parses() -> &'static ParseRuns {
    PARSES
        .get_or_init(|| async {
            let env =
                ProjectEnv::immutable_sa(PROJECT).expect("require_dbt_version fixture exists");
            let warn = Arc::new(ExecuteOnly::new(
                "parse_warn".to_string(),
                make_fs_cmd_vec_with_version_check("parse --log-format text"),
                make_fs_command_fn(),
                true,
            ));
            let bypass = Arc::new(ExecuteOnly::new(
                "parse_bypass".to_string(),
                make_fs_cmd_vec("parse --log-format text"),
                make_fs_command_fn(),
                true,
            ));
            TaskSeq::new("require_dbt_version")
                .task(Box::new(Arc::clone(&warn)))
                .task(Box::new(Arc::clone(&bypass)))
                .execute_in(&env)
                .await
                .expect("parse require_dbt_version");
            ParseRuns {
                warn: captured(&warn),
                bypass: captured(&bypass),
            }
        })
        .await
}

#[dbt_runtime::test]
async fn parse_warns_when_require_dbt_version_excludes_running_dbt() {
    let run = &parses().await.warn;
    assert_eq!(
        run.exit_code, 0,
        "parse should succeed when require-dbt-version excludes the running dbt, got:\n{}",
        run.output
    );
    assert!(
        run.output.contains("dbt1160"),
        "parse should warn dbt1160 when require-dbt-version excludes the running dbt, got:\n{}",
        run.output
    );
    assert!(
        run.output.contains(">=99.0.0"),
        "the warning should include the authored require-dbt-version, got:\n{}",
        run.output
    );
}

#[dbt_runtime::test]
async fn parse_with_no_version_check_skips_require_dbt_version_warning() {
    let run = &parses().await.bypass;
    assert_eq!(
        run.exit_code, 0,
        "parse --no-version-check should succeed, got:\n{}",
        run.output
    );
    assert!(
        !run.output.contains("dbt1160"),
        "parse --no-version-check should not warn dbt1160, got:\n{}",
        run.output
    );
}
