//! Snowflake adapter method and feature tests (see the `adapter` module).

use crate::common::FS_RUNNER;

use dbt_adapter_core::AdapterType;
use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::adapter_record_replay::{
    AdapterRecordReplayTurn, FileOverride, run_fs_record_replay_task_seq,
};

async fn run_fs_record_replay_snowflake(test_name: &str, project_path: &str) -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        test_name,
        project_path,
        AdapterType::Snowflake.as_ref(),
        None,
        &["compile"],
        None,
        &[],
    )
    .await
}

async fn run_fs_record_replay_snowflake_with_flags(
    test_name: &str,
    project_path: &str,
    flags: &str,
) -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        test_name,
        project_path,
        AdapterType::Snowflake.as_ref(),
        None,
        &["compile"],
        Some(flags.to_string()),
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_create_schema() -> FsResult<()> {
    run_fs_record_replay_snowflake(current_function_name!(), "tests/data/adapter/create_schema")
        .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_drop_schema() -> FsResult<()> {
    run_fs_record_replay_snowflake(current_function_name!(), "tests/data/adapter/drop_schema").await
}

#[dbt_runtime::test]
async fn adapters_snowflake_get_relation() -> FsResult<()> {
    run_fs_record_replay_snowflake(current_function_name!(), "tests/data/adapter/get_relation")
        .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_get_relation_from_cache() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/get_relation_from_cache",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_get_columns_in_relation() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/get_columns_in_relation",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_get_columns_in_relation_missing() -> FsResult<()> {
    run_fs_record_replay_snowflake_with_flags(
        current_function_name!(),
        "tests/data/adapter/get_columns_in_relation_missing",
        "--static-analysis off",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_get_column_schema_from_query() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/get_column_schema_from_query",
    )
    .await
}

#[dbt_runtime::test]
#[ignore = "@harry: this test is ignored because it requires a local query file"]
async fn adapters_snowflake_get_columns_in_relation_local() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/get_columns_in_relation_local",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_drop_relation() -> FsResult<()> {
    run_fs_record_replay_snowflake(current_function_name!(), "tests/data/adapter/drop_relation")
        .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_equals() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/equals",
        "snowflake",
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_get_ddl_prefix_for_create() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/get_ddl_prefix_for_create",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_get_relation_with_schema() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/get_relation_with_schema",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_needs_to_drop() -> FsResult<()> {
    run_fs_record_replay_snowflake(current_function_name!(), "tests/data/adapter/needs_to_drop")
        .await
}

// TODO: should we run this test in a separate DB for further
// isolation. Note that the test does not fail if db is polluted, but
// result is not stable.
#[dbt_runtime::test]
async fn adapters_snowflake_list_schemas() -> FsResult<()> {
    run_fs_record_replay_snowflake(current_function_name!(), "tests/data/adapter/list_schemas")
        .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_column_datatypes() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/column_datatypes",
        "snowflake",
        None,
        // run_0 creates the relations the test reads.
        &["run-operation seed_snowflake", "compile"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_metadata_source_freshness() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/metadata_source_freshness",
        "snowflake",
        None,
        // run_0 creates the relations the test reads.
        &["run-operation seed_snowflake", "source freshness"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_load_relation() -> FsResult<()> {
    run_fs_record_replay_snowflake(current_function_name!(), "tests/data/adapter/load_relation")
        .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_get_missing_columns() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/get_missing_columns",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_check_schema_exists() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/check_schema_exists",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_column_methods() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/column_methods",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_relation_methods() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/relation_methods",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_api_relation_create() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/api_relation_create",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_get_incremental_strategy_macro() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/get_incremental_strategy_macro",
    )
    .await
}

#[dbt_runtime::test]
#[ignore = "FIXME: the recordings for this example have a race condition due to concurrent schemas fetch in pre-compile"]
async fn adapters_snowflake_rename_relation() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/rename_relation",
    )
    .await
}

#[dbt_runtime::test]
#[ignore = "TODO: the test example has a race condition so is potentially flaky due to - 'select *' immediately after truncate_relation still returns data"]
async fn adapters_snowflake_truncate_relation() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/truncate_relation",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_add_query() -> FsResult<()> {
    run_fs_record_replay_snowflake(current_function_name!(), "tests/data/adapter/add_query").await
}

#[dbt_runtime::test]
async fn adapters_snowflake_behavior() -> FsResult<()> {
    run_fs_record_replay_snowflake(current_function_name!(), "tests/data/adapter/behavior").await
}

#[dbt_runtime::test]
async fn adapters_snowflake_snowflake_relation_methods() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/snowflake_relation_methods",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_quote_as_configured_default() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/quote_as_configured_default",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_quote_as_configured_provided() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/quote_as_configured_provided",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_api_relation_object() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/api_relation_object",
    )
    .await
}

#[dbt_runtime::test]
/// https://github.com/dbt-labs/fs/issues/4175
/// using repro: https://github.com/dbt-labs/scratch/tree/main/repros/snapshot_timestamp_types
async fn adapters_snowflake_get_relations_by_pattern() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/get_relations_by_pattern",
    )
    .await
}

#[dbt_runtime::test]
/// This `build` step is expected to be successful
/// Test `compile` step is expected to see no outputs from the debug block
async fn adapters_snowflake_debug_snapshot_simple() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/snapshot_simple",
        "snowflake",
        None,
        &[
            // this materializes model and snapshot
            "build",
            "compile --select +snapshot_columns",
        ],
        None,
        &[("DBT_ENGINE_EXPERIMENTAL_SNAPSHOT_COLUMNS", "1")],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_run_filter_flags() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/run_filter_flags",
        "snowflake",
        None,
        &[
            "run --select data",
            "run --select empty_model --empty", // the recorded sql has limit 0
            "run --select empty_ignored_model --empty", // no limit
            "run --select sample_model --sample {{'end': '2025-01-03', 'start': '2025-01-01'}}", // has a time range based filter
            "run --select sample_ignored_model --sample {{'end': '2025-01-03', 'start': '2025-01-01'}}", // no filter
        ],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_udfs() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/snowflake_udfs",
        AdapterType::Snowflake.as_ref(),
        None,
        &["run"],
        None,
        &[("DBT_ENGINE_EXPERIMENTAL_LIST_UDFS", "1")],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_dynamic_table() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/dynamic_table",
        AdapterType::Snowflake.as_ref(),
        None,
        &[
            AdapterRecordReplayTurn::new("run-operation set_up", &[]), //run_0: create source table
            AdapterRecordReplayTurn::new("run", &[]), // run_1: CREATE all dynamic tables
            AdapterRecordReplayTurn::new(
                "run",
                &[FileOverride {
                    original: "dbt_project.yml",
                    overriding_file: "_project_overrides/run_2.yml",
                }],
            ), // run_2: ALTER (config changes via override)
            AdapterRecordReplayTurn::new(
                "run --full-refresh",
                &[FileOverride {
                    original: "dbt_project.yml",
                    overriding_file: "_project_overrides/run_3.yml",
                }],
            ), // run_3: REPLACE (restored config via override)
        ],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_seed_datatypes() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/seed_datatypes/snowflake",
        AdapterType::Snowflake.as_ref(),
        None,
        &["seed"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_seed_empty() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/seed_empty",
        AdapterType::Snowflake.as_ref(),
        None,
        &["seed"],
        Some("--empty".to_string()),
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_sql_with_trailing_comments() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/sql_with_trailing_comments",
        AdapterType::Snowflake.as_ref(),
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_query_comment() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/query_comment",
        "snowflake",
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_query_comment_with_split() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/query_comment_with_split",
        "snowflake",
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_legacy_iceberg_materializations() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/iceberg_tables_legacy",
        AdapterType::Snowflake.as_ref(),
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_iceberg_builtin() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/snowflake_iceberg_builtin_catalog",
        AdapterType::Snowflake.as_ref(),
        Some("icebergBuiltinTestSchema".to_string()),
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_iceberg_grants_tags() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/snowflake_iceberg_grants_tags",
        AdapterType::Snowflake.as_ref(),
        Some("icebergrestschemafusion".to_string()),
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_catalog_v2_format_change_drop() -> FsResult<()> {
    // Regression test: with catalogs.yml v2 active, a table model that
    // switches format (Iceberg-REST -> plain native Snowflake) across runs,
    // against the same physical relation, must go through the shared
    // `drop.sql` -> `snowflake__drop_table` path, which calls
    // `adapter.build_catalog_relation(relation.database)` with a bare
    // database-name string. This previously panicked a `debug_assert!` in
    // `from_model_config_and_catalogs_v2` because the v1/v2 dispatch branch
    // was checked before the bare-string special case. See
    // `from_linked_database_name` in `catalog_relation.rs`.
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/snowflake_catalog_v2_format_change_drop",
        AdapterType::Snowflake.as_ref(),
        None,
        &[
            AdapterRecordReplayTurn::new("run", &[]), // run_1: CREATE as Iceberg-REST table
            AdapterRecordReplayTurn::new(
                "run",
                &[FileOverride {
                    original: "dbt_project.yml",
                    overriding_file: "_project_overrides/run_2.yml",
                }],
            ), // run_2: format change -> DROP + CREATE as native table
        ],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_expand_collated_columns() -> FsResult<()> {
    run_fs_record_replay_snowflake(
        current_function_name!(),
        "tests/data/adapter/snowflake_expand_collated_columns",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_store_failures() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/store_failures",
        AdapterType::Snowflake.as_ref(),
        None,
        &[
            // Make sure `one` exists
            "run --select one",
            // Run a test that does _not_ store failures
            "test --select not_null_one_id",
            // Run a test that _does_ store failures
            "test",
        ],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_snowflake_copy_tags() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/snowflake_copy_tags",
        AdapterType::Snowflake.as_ref(),
        None,
        &["run"],
        None,
        &[],
    )
    .await
}
