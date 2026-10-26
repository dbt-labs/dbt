//! Regression tests for reported Redshift bugs: each reproduces a fixed issue
//! (linked in its doc comment) from a project under `tests/data/repros/`.

use crate::common::FS_RUNNER;

use dbt_adapter_core::AdapterType;
use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::adapter_record_replay::{
    AdapterRecordReplayTurn, run_fs_record_replay_task_seq,
};

/// Runs `commands` on the repro project at `project_path`, in record or replay mode.
async fn run_fs_record_replay_redshift<T>(
    test_name: &str,
    project_path: &str,
    commands: &[T],
) -> FsResult<()>
where
    T: Into<AdapterRecordReplayTurn> + Clone,
{
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        test_name,
        project_path,
        AdapterType::Redshift.as_ref(),
        None,
        commands,
        None,
        &[],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/1031
#[dbt_runtime::test]
async fn redshift_persist_docs_columns() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/repros/redshift_persist_docs_columns",
        &["run"],
    )
    .await
}

/// Two dependent views (`b` refs `a`); `run` twice.
///
/// On the second run, rebuilding `a` CASCADE-drops `b` from the warehouse, but Fusion
/// must also evict `b` from `RelationCache`. Without cascade eviction, `b` is treated
/// as still existing and view materialization generates incorrect SQL.
#[dbt_runtime::test]
async fn redshift_two_views_dag() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/repros/redshift_two_views_dag",
        &["run", "run"],
    )
    .await
}

/// Ported from https://github.com/dbt-labs/dbt-adapters/commit/860da89225e2ecf1bf47038f5ac40d4eaa4019a2
/// (dbt-adapters#2087 / dbt-adapters#2085): `redshift__create_table_as` must skip
/// `distkey`/`sortkey` on temporary tables. Incremental with `delete+insert` builds a
/// temp relation via `create_table_as(True, ...)` — those temp CTAS statements should
/// not carry dist/sort clauses, but the target table should.
#[dbt_runtime::test]
async fn redshift_temp_table_skip_dist_sort() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/repros/redshift_temp_table_skip_dist_sort",
        &["run", "run"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/1095
#[dbt_runtime::test]
async fn redshift_mv_hyphen_in_fqn() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/repros/redshift_mv_hyphen_in_fqn",
        &["run"],
    )
    .await
}
