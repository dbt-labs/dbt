//! Regression tests for reported Snowflake bugs: each reproduces a fixed issue
//! (linked in its doc comment) from a project under `tests/data/repros/`.

use crate::common::FS_RUNNER;

use dbt_adapter_core::AdapterType;
use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::adapter_record_replay::run_fs_record_replay_task_seq;

/// Runs `commands` on the repro project at `project_path`, in record or replay mode.
async fn run_fs_record_replay_snowflake(
    test_name: &str,
    project_path: &str,
    commands: &[&'static str],
) -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        test_name,
        project_path,
        AdapterType::Snowflake.as_ref(),
        None,
        commands,
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_repros_sql_header() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/sql_header",
        &["run"],
    )
    .await
}

#[dbt_runtime::test]
#[ignore = "FIXME: this fails on Windows, https://github.com/dbt-labs/fs/actions/runs/15286075343/job/42996451132?pr=3427"]
async fn adapters_snowflake_repros_hello_world_table_view() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/hello_world_table_view",
        &["run"],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_repros_hello_world_seed() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/hello_world_seed",
        &["seed"],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_repros_seed_empty_column() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/seed_empty_column",
        &["seed"],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_repros_snowflake_incremental_transient() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/snowflake_incremental_transient",
        &["run", "run"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/132
#[dbt_runtime::test]
async fn snowflake_query_tag() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/query_tag",
        &["run"],
    )
    .await
}

#[dbt_runtime::test]
#[cfg(target_os = "linux")] // This test behaves differently on macos / linux
async fn snowflake_custom_incremental() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/custom_incremental",
        &["run", "run"],
    )
    .await
}

/// From https://github.com/dbt-labs/fs/issues/4021
/// Also covers https://github.com/dbt-labs/dbt-fusion/issues/1088 (snowflake_warehouse for data tests)
#[dbt_runtime::test]
async fn snowflake_config_warehouse() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/snowflake_config_warehouse",
        &["build"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/227
/// stderr is as expected, this test is to verify the SQL compilation error is gone
/// when the database name that contains special characters that requires quoting
#[dbt_runtime::test]
async fn snowflake_database_quoting() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/database_quoting",
        &["run"],
    )
    .await
}

/// https://github.com/dbt-labs/dbt-fusion/issues/155
#[dbt_runtime::test]
async fn adapters_snowflake_create_table_when_view_exists() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/repros/create_table_when_view_exists",
        "snowflake",
        None,
        // run_0 creates the relations the test reads.
        &["run-operation seed_snowflake", "run"],
        None,
        &[],
    )
    .await
}

/// From https://github.com/dbt-labs/fs/issues/4000
#[dbt_runtime::test]
async fn snowflake_render_sql_with_incorrect_arg_to_run_query() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/snowflake_render_sql_with_incorrect_arg_to_run_query",
        &["compile"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/371
#[dbt_runtime::test]
async fn snowflake_binary_16_seed() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/snowflake_binary_16_seed",
        &["seed"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/625
#[dbt_runtime::test]
async fn snowflake_dtype_no_precision() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/snowflake_dtype_no_precision",
        &["build"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/140
#[dbt_runtime::test]
async fn snowflake_use_schema() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/snowflake_use_schema",
        &["run"],
    )
    .await
}

/// Verify that SET variables in set_sql_header don't produce false dbt0227 errors for Snowflake
#[dbt_runtime::test]
async fn snowflake_sql_header_set() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/snowflake_sql_header_set",
        &["compile"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/161
#[dbt_runtime::test]
async fn snowflake_seed_escape_backslashes() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/snowflake_seed_escape_backslashes",
        &["build"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/789
#[dbt_runtime::test]
async fn convert_time() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/convert_datetime",
        &["compile"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/1546
#[dbt_runtime::test]
async fn snowflake_python_model_empty_flag() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/snowflake_python_model_empty",
        &["build --empty --no-manage-state"],
    )
    .await
}

/// Regression test for https://github.com/dbt-labs/dbt-core/issues/15170.
///
/// A microbatch model configured with `full_refresh: false` must NOT replay
/// every batch from `begin` when `dbt run --full-refresh` is invoked — the
/// model-level config overrides the CLI flag (matching dbt-core's
/// `_is_incremental` precedence).
///
/// The test pins `--event-time-end` so batch windows are deterministic:
///   - Run 1 (initial seed): `begin=2025-01-01` → `event_time_end=2025-01-05`
///     produces 4 day-batches.
///   - Run 2 (`--full-refresh` with `full_refresh: false`): must execute as
///     incremental with `lookback=1` → 2 trailing day-batches (the current
///     batch plus one lookback batch), not a full replay of all 4. Before the
///     fix, the CLI flag won and 4 batches were replayed; the recorded query
///     count under run_1 catches the regression on replay.
#[dbt_runtime::test]
async fn snowflake_microbatch_full_refresh_guard() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/repros/microbatch_full_refresh_guard",
        &[
            "run --event-time-end 2025-01-05",
            "run --full-refresh --event-time-end 2025-01-05",
        ],
    )
    .await
}
