//! Regression tests for reported BigQuery bugs: each reproduces a fixed issue
//! (linked in its doc comment) from a project under `tests/data/repros/`.

use std::sync::Arc;

use crate::common::FS_RUNNER;

use dbt_adapter_core::AdapterType;
use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::adapter_record_replay::{
    AdapterRecordReplayTurn, FileOverride, build_fs_record_replay_task_seq,
    run_fs_record_replay_task_seq,
};
use dbt_test_utils::task::CaptureRunResults;

/// Every BigQuery test runs on the Foundry driver unless it says otherwise.
const BIGQUERY_DRIVER: &[(&str, &str)] = &[("DBT_ADBC_BIGQUERY_USE_FOUNDRY", "1")];

/// Runs `commands` on the repro project at `project_path`, in record or replay mode.
async fn run_fs_record_replay_bigquery(
    test_name: &str,
    project_path: &str,
    commands: &[&'static str],
) -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        test_name,
        project_path,
        AdapterType::Bigquery.as_ref(),
        None,
        commands,
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_repros_hello_world_seed() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/hello_world_seed",
        &["seed"],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_repros_seed_empty_column() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/seed_empty_column",
        &["seed"],
    )
    .await
}

/// Regression for https://github.com/dbt-labs/dbt/issues/15961.
/// Dict and CSV fixtures must keep the existing parent's TIMESTAMP type.
/// Scalar projections check UTC offsets and DATETIME wall hours.
#[dbt_runtime::test]
async fn bigquery_unit_test_existing_parent_timestamp_fixtures() -> FsResult<()> {
    let (env, mut task_seq) = build_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/repros/unit_test_timestamp_fixtures",
        AdapterType::Bigquery.as_ref(),
        None,
        &[
            "run --select events",
            "test --select test_type:unit --write-json",
        ],
        Some("--static-analysis off".to_string()),
    )
    .await?;

    let captured = Arc::new(CaptureRunResults::new());
    task_seq.task(Box::new(Arc::clone(&captured)));
    // Keep cleanup from replacing the test artifact in recording mode.
    task_seq
        .execute_in_with_env(
            &env,
            &[
                ("DBT_ADBC_BIGQUERY_USE_FOUNDRY", "1"),
                ("DBT_WRITE_JSON", "false"),
            ],
        )
        .await?;

    let artifact = captured
        .get_run_results()
        .expect("missing unit-test results");
    assert_eq!(artifact.results.len(), 4);
    for (name, status, message) in [
        ("dict_timezone_matches", "pass", "Pass"),
        ("csv_timezone_matches", "pass", "Pass"),
        ("dict_wrong_id", "error", "Error"),
        ("csv_wrong_id", "error", "Error"),
    ] {
        let unique_id = format!("unit_test.unit_test_timestamp_fixtures.event_projection.{name}");
        let result = artifact
            .results
            .iter()
            .find(|result| result.unique_id == unique_id)
            .unwrap_or_else(|| panic!("missing {unique_id}"));
        assert_eq!(result.status, status, "{unique_id}: {:?}", result.message);
        // A row mismatch keeps the default message. SQL and fixture-rendering
        // errors carry diagnostics and must not become accepted goldens.
        assert_eq!(result.message.as_deref(), Some(message), "{unique_id}");
    }
    Ok(())
}

#[dbt_runtime::test]
#[ignore = "FIXME: https://github.com/dbt-labs/fs/issues/5401"]
/// https://github.com/dbt-labs/dbt-fusion/issues/338
async fn adapters_bigquery_repros_sharded_tables() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/repros/sharded_tables",
        AdapterType::Bigquery.as_ref(),
        None,
        // run_0 creates the relations the test reads.
        &["run-operation seed_bigquery", "compile", "run"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

/// E2E test for BigQuery `source freshness` with per-source overrides
/// (`loaded_at_field` and `loaded_at_query`). Exercises the
/// `freshness_with_overrides` code path which mixes:
///
///   - one bulk `__TABLES__` query for the non-override source, and
///   - one targeted query per override source.
#[dbt_runtime::test]
async fn adapters_bigquery_metadata_source_freshness_overrides() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/repros/bigquery_freshness_overrides",
        AdapterType::Bigquery.as_ref(),
        None,
        // run_0 creates the relations the test reads.
        &["run-operation seed_bigquery", "source freshness"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn bigquery_incremental_simple() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/incremental_simple",
        &["run", "run"],
    )
    .await
}

/// Regression for https://github.com/dbt-labs/fs/issues/12830: an empty relation
/// listing from the execution project must not hide an incremental table in the
/// target project.
#[dbt_runtime::test]
async fn bigquery_cross_project_incremental_preserves_existing_rows() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/bigquery_cross_project_incremental",
        &[
            "run --select incremental_model --full-refresh",
            "run --select incremental_model",
            "test --select assert_incremental_preserves_existing_rows",
        ],
    )
    .await
}

#[dbt_runtime::test]
#[ignore = "FIXME: https://github.com/dbt-labs/fs/issues/3999"]
async fn bigquery_create_table_run_twice() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/hello_world",
        &["run", "run"],
    )
    .await
}

/// From https://github.com/dbt-labs/fs/issues/4116
#[dbt_runtime::test]
#[ignore = "FIXME: https://github.com/dbt-labs/fs/issues/3999"]
async fn bigquery_table_view_persist_docs() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/table_view_persist_docs",
        &["run"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/1383
/// Seeds with persist_docs.relation=true but no description in schema.yml should
/// not produce a BigQuery syntax error. The bug was `'description' in model`
/// which is always true (description defaults to ""), causing an empty ALTER TABLE.
#[dbt_runtime::test]
async fn bigquery_seed_empty_description() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/bigquery_seed_empty_description",
        &["seed"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/399
/// This test also verifies we take advantage of the multi-statement support from BQ properly
/// https://cloud.google.com/bigquery/docs/reference/standard-sql/procedural-language
/// checkout this recording model.BigQueryPartitionConfigRender.model-4.sql,
/// it declares a variable first then attempts to use it in a later statement
///
/// Updated to be used for
/// https://github.com/dbt-labs/dbt-fusion/issues/1037
#[dbt_runtime::test]
async fn bigquery_incremental_partition() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/bigquery_incremental_partition",
        &["seed", "run", "run"],
    )
    .await
}

/// From https://github.com/dbt-labs/fs/issues/4208
#[dbt_runtime::test]
async fn bigquery_table_with_labels() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/bigquery_table_with_labels",
        &["run"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/318
/// This test .stderr is non-empty as expected to test the `execute_project` config is properly propagated to the connection
/// But the project_id (database) used in the SQL should still be what's defined in `database` field
#[ignore = "FIXME: https://github.com/dbt-labs/fs/issues/12096"]
#[dbt_runtime::test]
async fn bigquery_execute_project_profile_config() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/bigquery_execute_project_profile_config",
        &["run"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/498
#[dbt_runtime::test]
async fn bigquery_use_direct_information_schema_views() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/bigquery_use_direct_information_schema_views",
        &["run"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/637
#[dbt_runtime::test]
async fn bigquery_array_docs() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/bigquery_array_docs",
        &["run"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/899
#[dbt_runtime::test]
async fn bigquery_use_directly() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/bigquery_use_directly",
        &["compile"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-core/issues/14097
///
/// A BigQuery model with an enforced contract and a column declared as
/// `data_type: FLOAT` (with `alias_types: true`, dbt's default) must have its
/// leaf type aliased to `FLOAT64` before the CREATE TABLE is issued. Before the
/// fix, the contract DDL emitted `cast(null as FLOAT)` and BigQuery rejected it
/// with `Type not found: FLOAT`.
#[dbt_runtime::test]
async fn bigquery_contract_alias_types_float() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/bigquery_contract_alias_types_float",
        &["run"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt-fusion/issues/878
#[dbt_runtime::test]
async fn bigquery_nested_datatypes() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/repros/bigquery_nested_datatypes",
        &["build"],
    )
    .await
}

/// From https://github.com/dbt-labs/dbt/issues/15352
///
/// Rebuilding a view as a table drops the view, then `is_replaceable` looks the
/// dropped view up. The legacy driver reports that 404 as an untyped driver
/// error, which fails the model instead of reading as "replaceable".
#[dbt_runtime::test]
async fn bigquery_table_replaces_view() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/repros/bigquery_table_replaces_view",
        AdapterType::Bigquery.as_ref(),
        None,
        &[
            AdapterRecordReplayTurn::new("run", &[]),
            AdapterRecordReplayTurn::new(
                "run",
                &[FileOverride {
                    original: "models/m.sql",
                    overriding_file: "_overrides/m_table.sql",
                }],
            ),
        ],
        None,
        // The Foundry driver already types the 404 as NotFound; the bug is on the canonical one.
        &[("DBT_ADBC_BIGQUERY_USE_FOUNDRY", "0")],
    )
    .await
}
