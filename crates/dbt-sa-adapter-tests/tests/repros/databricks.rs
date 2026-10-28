//! Regression tests for reported Databricks bugs: each reproduces a fixed issue
//! (linked in its doc comment) from a project under `tests/data/repros/`.

use crate::common::FS_RUNNER;

use dbt_adapter_core::AdapterType;
use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::adapter_record_replay::{
    AdapterRecordReplayTurn, FileOverride, run_fs_record_replay_task_seq,
};

/// Runs `commands` on the repro project at `project_path`, in record or replay mode.
async fn run_fs_record_replay_databricks<T>(
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
        AdapterType::Databricks.as_ref(),
        None,
        commands,
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_repros_hello_world_seed() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/repros/hello_world_seed",
        &["seed"],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_repros_seed_empty_column() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/repros/seed_empty_column",
        &["seed"],
    )
    .await
}

#[dbt_runtime::test]
async fn databricks_incremental_simple() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/repros/incremental_simple",
        &["run", "run"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/378
#[dbt_runtime::test]
async fn databricks_use_system_relations() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/repros/databricks_use_system_relations",
        &["run"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/298
#[dbt_runtime::test]
async fn databricks_test_test_array_data() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/repros/test_array_data",
        &["build -s +add_array"],
    )
    .await
}

#[dbt_runtime::test]
async fn databricks_snapshot_simple() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/repros/databricks_snapshot_simple",
        &[
            "run --select orders",
            "snapshot --select orders_snapshot",
            "snapshot --select orders_snapshot",
        ],
    )
    .await
}

/// From https://github.com/databricks/dbt-databricks/issues/1611
///
/// Composite `unique_key` with `delete+insert` on the legacy (DBR <17.1)
/// path must correlate on the whole tuple via `EXISTS (SELECT 1 ...)`. The
/// prior row-valued `IN (...)` raised `DELTA_UNSUPPORTED_MULTI_COL_IN_PREDICATE`
/// on Delta. This project overrides `get_delete_insert_sql` to force the
/// legacy path so the fix is exercised on any warehouse.
#[dbt_runtime::test]
async fn databricks_delete_insert_composite_key() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/repros/databricks_delete_insert_composite_key",
        &["run", "run"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-core/issues/15583
///
/// With `view_update_via_alter` and `persist_docs: {relation: true, columns: true}`, adding a
/// column to a view must emit `ALTER VIEW ... AS (...)` before `COMMENT ON COLUMN` for the new
/// column. The Databricks `query` relation config component does not report the changed view
/// definition, so the changeset only carries the column comments and the run fails with
/// `UNRESOLVED_COLUMN.WITH_SUGGESTION`.
#[dbt_runtime::test]
async fn databricks_view_update_via_alter_add_column() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/repros/databricks_view_update_via_alter_add_column",
        &[
            // run_0: create the view over foo_key, foo_name
            AdapterRecordReplayTurn::new("run", &[]),
            // run_1: add foo_description to both the model and its column docs
            AdapterRecordReplayTurn::new(
                "run",
                &[
                    FileOverride {
                        original: "models/foobar.sql",
                        overriding_file: "_overrides/foobar_v2.sql",
                    },
                    FileOverride {
                        original: "models/schema.yml",
                        overriding_file: "_overrides/schema_v2.yml",
                    },
                ],
            ),
        ],
    )
    .await
}

#[dbt_runtime::test]
async fn databricks_microbatch_simple() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/repros/microbatch_simple",
        &["run --event-time-end 2025-01-05"],
    )
    .await
}
