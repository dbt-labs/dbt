use crate::common::FS_RUNNER;

use dbt_adapter_core::AdapterType;
use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::adapter_record_replay::run_fs_record_replay_task_seq;
use dbt_test_utils::is_update_mode_with_warehouse;

async fn run_fs_record_replay_clickhouse(test_name: &str, project_path: &str) -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        test_name,
        project_path,
        AdapterType::ClickHouse.as_ref(),
        None,
        &["compile"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_create_schema() -> FsResult<()> {
    run_fs_record_replay_clickhouse(current_function_name!(), "tests/data/adapter/create_schema")
        .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_drop_schema() -> FsResult<()> {
    run_fs_record_replay_clickhouse(current_function_name!(), "tests/data/adapter/drop_schema")
        .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_get_relation() -> FsResult<()> {
    run_fs_record_replay_clickhouse(current_function_name!(), "tests/data/adapter/get_relation")
        .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_get_relation_no_hydration() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/get_relation_no_hydration",
        AdapterType::ClickHouse.as_ref(),
        None,
        &["run-operation check_get_relation"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_drop_relation() -> FsResult<()> {
    run_fs_record_replay_clickhouse(current_function_name!(), "tests/data/adapter/drop_relation")
        .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_get_columns_in_relation() -> FsResult<()> {
    run_fs_record_replay_clickhouse(
        current_function_name!(),
        "tests/data/adapter/get_columns_in_relation",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_check_schema_exists() -> FsResult<()> {
    run_fs_record_replay_clickhouse(
        current_function_name!(),
        "tests/data/adapter/check_schema_exists",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_load_relation() -> FsResult<()> {
    run_fs_record_replay_clickhouse(current_function_name!(), "tests/data/adapter/load_relation")
        .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_truncate_relation() -> FsResult<()> {
    run_fs_record_replay_clickhouse(
        current_function_name!(),
        "tests/data/adapter/truncate_relation",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_rename_relation() -> FsResult<()> {
    run_fs_record_replay_clickhouse(
        current_function_name!(),
        "tests/data/adapter/rename_relation",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_add_query() -> FsResult<()> {
    run_fs_record_replay_clickhouse(current_function_name!(), "tests/data/adapter/add_query").await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_convert_type() -> FsResult<()> {
    run_fs_record_replay_clickhouse(current_function_name!(), "tests/data/adapter/convert_type")
        .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_column_methods() -> FsResult<()> {
    run_fs_record_replay_clickhouse(
        current_function_name!(),
        "tests/data/adapter/column_methods",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_quote_as_configured_default() -> FsResult<()> {
    run_fs_record_replay_clickhouse(
        current_function_name!(),
        "tests/data/adapter/quote_as_configured_default",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_quote_as_configured_provided() -> FsResult<()> {
    run_fs_record_replay_clickhouse(
        current_function_name!(),
        "tests/data/adapter/quote_as_configured_provided",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_api_relation_create() -> FsResult<()> {
    run_fs_record_replay_clickhouse(
        current_function_name!(),
        "tests/data/adapter/api_relation_create",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_flags() -> FsResult<()> {
    run_fs_record_replay_clickhouse(current_function_name!(), "tests/data/adapter/flags").await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_verify_database() -> FsResult<()> {
    run_fs_record_replay_clickhouse(
        current_function_name!(),
        "tests/data/adapter/verify_database",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_list_schemas() -> FsResult<()> {
    run_fs_record_replay_clickhouse(current_function_name!(), "tests/data/adapter/list_schemas")
        .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_get_columns_in_relation_missing() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/get_columns_in_relation_missing",
        AdapterType::ClickHouse.as_ref(),
        None,
        &["compile"],
        Some("--static-analysis off".to_string()),
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_get_missing_columns() -> FsResult<()> {
    run_fs_record_replay_clickhouse(
        current_function_name!(),
        "tests/data/adapter/get_missing_columns",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_get_relation_from_cache() -> FsResult<()> {
    run_fs_record_replay_clickhouse(
        current_function_name!(),
        "tests/data/adapter/get_relation_from_cache",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_query_comment() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/query_comment",
        AdapterType::ClickHouse.as_ref(),
        None,
        &["run"],
        Some("--static-analysis off".to_string()),
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_query_comment_with_split() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/query_comment_with_split",
        AdapterType::ClickHouse.as_ref(),
        None,
        &["run"],
        Some("--static-analysis off".to_string()),
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_store_failures() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/store_failures",
        AdapterType::ClickHouse.as_ref(),
        None,
        &["run --select one", "test --select not_null_one_id", "test"],
        Some("--static-analysis off".to_string()),
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_clickhouse_metadata_source_schemas() -> FsResult<()> {
    assert!(
        !is_update_mode_with_warehouse(),
        "STUB: re-recording needs the warehouse seeding context, which is not available in SA"
    );
    let schema_name: Option<String> = None;
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/metadata_source_schemas",
        AdapterType::ClickHouse.as_ref(),
        schema_name,
        &["compile"],
        Some("--static-analysis off".to_string()),
        &[],
    )
    .await
}

#[dbt_runtime::test]
#[ignore = "ClickHouseMetadataAdapter::freshness not yet implemented — see dbt-fusion#1679"]
async fn adapters_clickhouse_metadata_source_freshness() -> FsResult<()> {
    assert!(
        !is_update_mode_with_warehouse(),
        "STUB: re-recording needs the warehouse seeding context, which is not available in SA"
    );
    let schema_name: Option<String> = None;
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/metadata_source_freshness",
        AdapterType::ClickHouse.as_ref(),
        schema_name,
        &["source freshness"],
        Some("--static-analysis off".to_string()),
        &[],
    )
    .await
}

#[dbt_runtime::test]
#[ignore = "no recording: recording `build` stores the live service's full database list (select name from system.databases); needs a clean instance"]
async fn test_clickhouse_basic_replay() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/clickhouse_basic_replay",
        AdapterType::ClickHouse.as_ref(),
        None,
        &["build"],
        Some("--static-analysis off".to_string()),
        &[],
    )
    .await
}
