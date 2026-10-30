//! BigQuery adapter method and feature tests (see the `adapter` module).

use crate::common::FS_RUNNER;

use dbt_adapter_core::AdapterType;
use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::adapter_record_replay::run_fs_record_replay_task_seq;

/// Every BigQuery test runs on the Foundry driver unless it says otherwise.
const BIGQUERY_DRIVER: &[(&str, &str)] = &[("DBT_ADBC_BIGQUERY_USE_FOUNDRY", "1")];

async fn run_fs_record_replay_bigquery(test_name: &str, project_path: &str) -> FsResult<()> {
    run_fs_record_replay_bigquery_with_flags(test_name, project_path, None).await
}

async fn run_fs_record_replay_bigquery_with_flags(
    test_name: &str,
    project_path: &str,
    flags: Option<&str>,
) -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        test_name,
        project_path,
        AdapterType::Bigquery.as_ref(),
        None,
        &["compile"],
        flags.map(str::to_string),
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_create_schema() -> FsResult<()> {
    run_fs_record_replay_bigquery(current_function_name!(), "tests/data/adapter/create_schema")
        .await
}

/// Verifies that create_schema uses the target project (database), not execution_project,
/// when the two differ — so the dataset is created in the right place.
#[dbt_runtime::test]
async fn adapters_bigquery_create_schema_execution_project() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/create_schema_execution_project",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_drop_schema() -> FsResult<()> {
    run_fs_record_replay_bigquery(current_function_name!(), "tests/data/adapter/drop_schema").await
}

#[dbt_runtime::test]
async fn adapters_bigquery_get_relation() -> FsResult<()> {
    run_fs_record_replay_bigquery(current_function_name!(), "tests/data/adapter/get_relation").await
}

#[dbt_runtime::test]
async fn adapters_bigquery_get_relation_from_cache() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/get_relation_from_cache",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_get_columns_in_relation() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/get_columns_in_relation",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_get_columns_in_relation_missing() -> FsResult<()> {
    run_fs_record_replay_bigquery_with_flags(
        current_function_name!(),
        "tests/data/adapter/get_columns_in_relation_missing",
        Some("--static-analysis off"),
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_drop_relation() -> FsResult<()> {
    run_fs_record_replay_bigquery(current_function_name!(), "tests/data/adapter/drop_relation")
        .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_check_schema_exists() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/check_schema_exists",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_load_relation() -> FsResult<()> {
    run_fs_record_replay_bigquery(current_function_name!(), "tests/data/adapter/load_relation")
        .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_get_missing_columns() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/get_missing_columns",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_gnest_column_data_types() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/nest_column_data_types",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_add_time_ingestion_partition_column() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/add_time_ingestion_partition_column",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_non_connection_methods() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/bigquery_non_connection_methods",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_use_connection_methods() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/bigquery_use_connection_methods",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_column_methods() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/column_methods",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_render_raw_constraint() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/render_raw_constraint",
        AdapterType::Bigquery.as_ref(),
        None,
        &["compile", "compile --empty"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_get_incremental_strategy_macro() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/get_incremental_strategy_macro",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_rename_relation() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/rename_relation",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_alter_table_add_columns() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/bigquery_alter_table_add_columns",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_update_columns() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/bigquery_update_columns",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_list_schemas() -> FsResult<()> {
    run_fs_record_replay_bigquery(current_function_name!(), "tests/data/adapter/list_schemas").await
}

#[dbt_runtime::test]
async fn adapters_bigquery_convert_type() -> FsResult<()> {
    run_fs_record_replay_bigquery(current_function_name!(), "tests/data/adapter/convert_type").await
}

#[dbt_runtime::test]
async fn adapters_bigquery_copy_table() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/bigquery_copy_table",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_describe_relation_related() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/bigquery_describe_relation_related",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_quote_as_configured_default() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/quote_as_configured_default",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_quote_as_configured_provided() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/quote_as_configured_provided",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_seed_datatypes() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/seed_datatypes/bigquery",
        AdapterType::Bigquery.as_ref(),
        None,
        &["seed"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_seed_empty() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/seed_empty",
        AdapterType::Bigquery.as_ref(),
        None,
        &["seed"],
        Some("--empty".to_string()),
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_api_relation_create() -> FsResult<()> {
    run_fs_record_replay_bigquery(
        current_function_name!(),
        "tests/data/adapter/api_relation_create",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_information_schema() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/information_schema",
        AdapterType::Bigquery.as_ref(),
        None,
        &["run"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_column_datatypes() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/column_datatypes",
        AdapterType::Bigquery.as_ref(),
        None,
        // run_0 creates the relations the test reads.
        &["run-operation seed_bigquery", "compile"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_sql_with_trailing_comments() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/sql_with_trailing_comments",
        AdapterType::Bigquery.as_ref(),
        None,
        &["run"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_query_comment() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/query_comment",
        AdapterType::Bigquery.as_ref(),
        None,
        &["run"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_query_comment_with_split() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/query_comment_with_split",
        AdapterType::Bigquery.as_ref(),
        None,
        &["run"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_materialized_views() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/bigquery_materialized_views",
        AdapterType::Bigquery.as_ref(),
        None,
        &[
            "build",
            "run-operation pull_materialized_view_options --args {\"view_name\": \"mat_view\"}",
            "run-operation update_materialized_view --args {\"view_name\": \"mat_view\"}",
            "run-operation pull_materialized_view_options --args {\"view_name\": \"mat_view\"}",
            "build -s mat_view",
            "run-operation pull_materialized_view_options --args {\"view_name\": \"mat_view\"}",
        ],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_store_failures() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/store_failures",
        AdapterType::Bigquery.as_ref(),
        None,
        &[
            // Make sure `one` exists
            "run --select one",
            // Run a test that does _not_ store failures
            "test --select +not_null_one_id",
            // Run a test that _does_ store failures
            "test --select +unique_one_id",
        ],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_unit_test() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/unit_test",
        AdapterType::Bigquery.as_ref(),
        None,
        // build executes tests
        &["build"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_unit_test_issue_15561() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/unit_test_issue_15561",
        AdapterType::Bigquery.as_ref(),
        Some(dbt_test_utils::record_or_replay_schema(
            "adapters_bigquery_unit_test",
        )),
        // build executes tests
        &["build"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

// Regression: unit test on an ephemeral model whose `+schema` points at a
// dataset that does not exist in BigQuery. Prior to the fix, schema inference
// for the model-under-test went through the temp-table probe materialization,
// which tried to create `__dbt_tmp` in the ephemeral model's (non-existent)
// dataset and failed with "Dataset not found".
//
// Static analysis is disabled here so the schema is not pre-populated from
// the analysis phase; that forces the unit-test renderer to go through
// `populate_schema_from_empty_relation`, where the fix lives.
#[dbt_runtime::test]
async fn adapters_bigquery_unit_test_issue_15708() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/unit_test_issue_15708",
        AdapterType::Bigquery.as_ref(),
        Some(dbt_test_utils::record_or_replay_schema(
            "adapters_bigquery_unit_test",
        )),
        // build executes tests
        &["build"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_unit_test_ephemeral() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/unit_test_ephemeral",
        AdapterType::Bigquery.as_ref(),
        None,
        // build executes tests
        &["build"],
        Some("--static-analysis off".to_string()),
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_catalogs() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/bigquery_catalogs",
        AdapterType::Bigquery.as_ref(),
        None,
        &["run"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_iceberg_v2() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/bigquery_iceberg_v2",
        AdapterType::Bigquery.as_ref(),
        None,
        &["run"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_metadata_source_freshness() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/metadata_source_freshness",
        AdapterType::Bigquery.as_ref(),
        None,
        // run_0 creates the relations the test reads.
        &["run-operation seed_bigquery", "source freshness"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

/// Test for BigQuery Python models using BigFrames submission method
/// See: https://docs.getdbt.com/guides/dbt-python-bigframes
#[dbt_runtime::test]
#[ignore = "FIXME: enable after supporting AdapterEngine R&R for python models"]
async fn bigquery_python_models() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/bigquery_python_models",
        AdapterType::Bigquery.as_ref(),
        None,
        &["run"],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_snapshot_new_record() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/bigquery_snapshot_new_record",
        AdapterType::Bigquery.as_ref(),
        None,
        &[
            "run",
            "snapshot",
            "run --vars {{ drop_rows: true }}",
            "snapshot",
        ],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_bigquery_hidden_dataset() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/bigquery_hidden_dataset",
        AdapterType::Bigquery.as_ref(),
        Some("_adapters_bigquery_hidden_dataset".to_string()),
        &[
            "run-operation set_up",
            "run",
            "run-operation check_relations",
        ],
        None,
        BIGQUERY_DRIVER,
    )
    .await
}
