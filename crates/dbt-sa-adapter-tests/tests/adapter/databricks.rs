//! Databricks adapter method and feature tests (see the `adapter` module).

use crate::common::FS_RUNNER;

use dbt_adapter_core::AdapterType;
use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::adapter_record_replay::run_fs_record_replay_task_seq;
use dbt_test_utils::adapter_record_replay::{AdapterRecordReplayTurn, FileOverride};

async fn run_fs_record_replay_databricks(test_name: &str, project_path: &str) -> FsResult<()> {
    run_fs_record_replay_databricks_with_flags(test_name, project_path, None).await
}

async fn run_fs_record_replay_databricks_with_flags(
    test_name: &str,
    project_path: &str,
    flags: Option<&str>,
) -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        test_name,
        project_path,
        AdapterType::Databricks.as_ref(),
        None,
        &["compile"],
        flags.map(str::to_string),
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_rename_relation() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/rename_relation",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_create_schema() -> FsResult<()> {
    run_fs_record_replay_databricks(current_function_name!(), "tests/data/adapter/create_schema")
        .await
}

#[dbt_runtime::test]
async fn adapters_databricks_drop_schema() -> FsResult<()> {
    run_fs_record_replay_databricks(current_function_name!(), "tests/data/adapter/drop_schema")
        .await
}

#[dbt_runtime::test]
async fn adapters_databricks_add_query() -> FsResult<()> {
    run_fs_record_replay_databricks(current_function_name!(), "tests/data/adapter/add_query").await
}

#[dbt_runtime::test]
async fn adapters_databricks_get_relation() -> FsResult<()> {
    run_fs_record_replay_databricks(current_function_name!(), "tests/data/adapter/get_relation")
        .await
}

#[dbt_runtime::test]
async fn adapters_databricks_drop_relation() -> FsResult<()> {
    run_fs_record_replay_databricks(current_function_name!(), "tests/data/adapter/drop_relation")
        .await
}

#[dbt_runtime::test]
async fn adapters_databricks_get_incremental_strategy_macro() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/get_incremental_strategy_macro",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_get_relation_from_cache() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/get_relation_from_cache",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_get_columns_in_relation() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/get_columns_in_relation",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_get_columns_in_relation_missing() -> FsResult<()> {
    run_fs_record_replay_databricks_with_flags(
        current_function_name!(),
        "tests/data/adapter/get_columns_in_relation_missing",
        Some("--static-analysis off"),
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_get_missing_columns() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/get_missing_columns",
    )
    .await
}

// ! The dbt behavior is incorrect on this, try dbt compile on this workspace
// since spark__get_columns_in_relation_raw macro doesn't return var size,
// expand_target_column_types won't work for tables has datatype var(size)
#[dbt_runtime::test]
async fn adapters_databricks_expand_target_column_types() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/expand_target_column_types",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_truncate_relation() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/truncate_relation",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_check_schema_exists() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/check_schema_exists",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_compare_dbr_version() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/compare_dbr_version",
    )
    .await
}

#[dbt_runtime::test]
#[ignore = "This test is currently failing locally"]
async fn adapters_databricks_compute_external_path() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/compute_external_path",
    )
    .await
}

#[dbt_runtime::test]
#[ignore = "This test currently fails due to RunConfig not being fully populated"]
async fn adapters_databricks_update_tblproperties_for_uniform_iceberg() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/update_tblproperties_for_uniform_iceberg",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_render_raw_constraint() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/render_raw_constraint",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_behavior() -> FsResult<()> {
    run_fs_record_replay_databricks(current_function_name!(), "tests/data/adapter/behavior").await
}

#[dbt_runtime::test]
async fn adapters_databricks_quote_as_configured_default() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/quote_as_configured_default",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_quote_as_configured_provided() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/quote_as_configured_provided",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_api_relation_object() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/api_relation_object",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_materialized_view() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_materialized_view",
        AdapterType::Databricks.as_ref(),
        None,
        &[
            // Test that creating a materialized view works
            "run --select test_create",
            // Test that altering a materialized view works
            "run --select test_alter",
            // Test that failing on configuration change works
            "run --select test_fail",
            // Test that full refresh replaces the materialized view in place
            "run --select test_full_refresh --full-refresh",
            // Test that use_safer_relation_operations still replaces in place
            "run --select test_full_refresh --full-refresh --vars {{ safe: true }}",
            // Test that adding partition_by replaces in place
            "run --select test_full_refresh --full-refresh --vars {{ partition_by: id }}",
            // Test that an unchanged partition_by replaces in place
            "run --select test_full_refresh --full-refresh --vars {{ partition_by: id }}",
            // Test that changing partition_by through on_configuration_change replaces in place
            "run --select test_full_refresh --vars {{ partition_by: user_id }}",
            // Test that removing partition_by drops and recreates
            "run --select test_full_refresh --full-refresh",
        ],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_api_relation_create() -> FsResult<()> {
    run_fs_record_replay_databricks(
        current_function_name!(),
        "tests/data/adapter/api_relation_create",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_column_datatypes() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/column_datatypes",
        AdapterType::Databricks.as_ref(),
        None,
        // run_0 creates the relations the test reads.
        &["run-operation seed_databricks", "compile"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_persist_docs() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/persist_docs",
        AdapterType::Databricks.as_ref(),
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_metadata_source_freshness() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/metadata_source_freshness",
        AdapterType::Databricks.as_ref(),
        None,
        // run_0 creates the relations the test reads.
        &["run-operation seed_databricks", "source freshness"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_seed_datatypes() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/seed_datatypes/databricks",
        AdapterType::Databricks.as_ref(),
        None,
        &["seed"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_seed_empty() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/seed_empty",
        AdapterType::Databricks.as_ref(),
        None,
        &["seed"],
        Some("--empty".to_string()),
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_list_schemas() -> FsResult<()> {
    run_fs_record_replay_databricks(current_function_name!(), "tests/data/adapter/list_schemas")
        .await
}

#[dbt_runtime::test]
async fn adapters_databricks_sql_with_trailing_comments() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/sql_with_trailing_comments",
        AdapterType::Databricks.as_ref(),
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_query_comment() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/query_comment",
        AdapterType::Databricks.as_ref(),
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_query_comment_with_split() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/query_comment_with_split",
        AdapterType::Databricks.as_ref(),
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_incremental_tags() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_incremental_tags",
        AdapterType::Databricks.as_ref(),
        None,
        &[
            // First run with initial tags
            "run",
            // Second run with updated tag value
            "run --vars {{ tag_value: existing }}",
            // Third run with updated key and value
            "run --vars {{ tag_value: updated, key_value: key_value }}",
        ],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_standardize_grants_dict() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/standardize_grants_dict",
        AdapterType::Databricks.as_ref(),
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_store_failures() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/store_failures",
        AdapterType::Databricks.as_ref(),
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
async fn adapters_databricks_iceberg_v2() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_iceberg_v2",
        AdapterType::Databricks.as_ref(),
        None,
        &["run --no-manage-state", "run --no-manage-state"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_column_tags() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_column_tags",
        AdapterType::Databricks.as_ref(),
        None,
        &[
            // First run with initial column tags
            "run",
            // Second run with updated tag values
            "run --vars {{ data_type_value: 'text', owner: updated_team }}",
        ],
        None,
        &[],
    )
    .await
}

/// `system.query.history` lags behind execution, so a fresh recording can need a
/// retry once the tagged statements land there.
#[dbt_runtime::test]
async fn databricks_query_tags() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_query_tags",
        AdapterType::Databricks.as_ref(),
        None,
        &["run", "test"],
        None,
        &[],
    )
    .await
}

/// See: https://docs.getdbt.com/reference/resource-configs/databricks-configs#python-model-configuration
#[dbt_runtime::test]
#[ignore = "FIXME: enable after supporting AdapterEngine R&R for python models"]
async fn databricks_python_models() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_python_models",
        AdapterType::Databricks.as_ref(),
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_is_cluster() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/is_cluster",
        AdapterType::Databricks.as_ref(),
        None,
        &["compile", "run"],
        None,
        &[],
    )
    .await
}

/// Test that makes sure column masks apply correctly when creating and altering tables.
#[dbt_runtime::test]
async fn adapters_databricks_column_masks() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_column_masks",
        AdapterType::Databricks.as_ref(),
        None,
        &["build"],
        None,
        &[],
    )
    .await
}

/// Test that a contract-enforced table model with a primary_key constraint materializes
/// correctly on Databricks Unity Catalog: CTAS runs, NOT NULL is applied, and
/// ALTER TABLE ADD CONSTRAINT PRIMARY KEY succeeds via persist_constraints.
///
/// This is a regression guard for the `databricks__persist_constraints` fix that
/// drops any inherited primary key (from a prior SHALLOW CLONE) before applying
/// the model's own constraint.
///
/// The contract column `data_type`s are declared uppercase (INT, STRING) so the
/// recording also proves Databricks lowercases types when rendering DDL
/// (ColumnStatic::translate_type).
#[dbt_runtime::test]
async fn adapters_databricks_pk_constraint() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_pk_constraint",
        AdapterType::Databricks.as_ref(),
        None,
        &["run --select customers"],
        None,
        &[],
    )
    .await
}

/// Test that the new configs added by use_materialization_v2 work:
/// - view_update_via_alter
/// - incremental_apply_config_changes
/// - use_safer_relation_operations
#[dbt_runtime::test]
async fn adapters_databricks_use_materialization_v2_configs() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_use_materialization_v2_configs",
        AdapterType::Databricks.as_ref(),
        None,
        &[
            AdapterRecordReplayTurn::new("run-operation set_up", &[]),
            AdapterRecordReplayTurn::new("run", &[]),
        ],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_row_filters() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_row_filters",
        AdapterType::Databricks.as_ref(),
        None,
        &[
            // Create the schema and the region_filter and user_filter functions.
            AdapterRecordReplayTurn::new("run-operation set_up", &[]),
            // Create every model with its initial row filter, including the hook-created and multi-argument function cases.
            AdapterRecordReplayTurn::new("build", &[]),
            // Run with unchanged configs: preserve existing filters without issuing SET or DROP ROW FILTER.
            AdapterRecordReplayTurn::new("run", &[]),
            // Change lifecycle models to user_filter(user_id), using ALTER or replacement as appropriate.
            AdapterRecordReplayTurn::new(
                "run",
                &[FileOverride {
                    original: "models/row_filter_lifecycle.yml",
                    overriding_file: "_project_overrides/row_filter_updated.yml",
                }],
            ),
            // Remove lifecycle filters, using DROP ROW FILTER or an unfiltered replacement as appropriate.
            AdapterRecordReplayTurn::new(
                "run",
                &[FileOverride {
                    original: "models/row_filter_lifecycle.yml",
                    overriding_file: "_project_overrides/row_filter_removed.yml",
                }],
            ),
        ],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_row_filters_v1() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_row_filters_v1",
        AdapterType::Databricks.as_ref(),
        None,
        &[
            // Create the schema and the region_filter and user_filter functions.
            AdapterRecordReplayTurn::new("run-operation set_up", &[]),
            // Create the table and incremental model with region_filter(region).
            AdapterRecordReplayTurn::new("build", &[]),
            // Rebuild unchanged: retain region_filter(region) without issuing SET or DROP ROW FILTER.
            AdapterRecordReplayTurn::new("build", &[]),
            // Recreate only row_filter_table with user_filter(user_id); do not run the incremental model.
            AdapterRecordReplayTurn::new(
                "build --select row_filter_table",
                &[FileOverride {
                    original: "models/properties.yml",
                    overriding_file: "_project_overrides/updated_filter.yml",
                }],
            ),
            // Drop the existing filter and recreate only row_filter_table without a row filter.
            AdapterRecordReplayTurn::new(
                "build --select row_filter_table",
                &[FileOverride {
                    original: "models/properties.yml",
                    overriding_file: "_project_overrides/no_filter.yml",
                }],
            ),
        ],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_python_scalar_udf() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_python_scalar_udf",
        AdapterType::Databricks.as_ref(),
        None,
        &["build"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_liquid_clustering() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_liquid_clustering",
        AdapterType::Databricks.as_ref(),
        None,
        &[
            // Create every model clustered by `region`.
            AdapterRecordReplayTurn::new("build", &[]),
            // Run with unchanged configs: preserve clustering without issuing an ALTER.
            AdapterRecordReplayTurn::new("run", &[]),
            // Change cluster columns to `user_id`, issuing ALTER ... CLUSTER BY (user_id).
            AdapterRecordReplayTurn::new(
                "run",
                &[FileOverride {
                    original: "models/lc_lifecycle.yml",
                    overriding_file: "_project_overrides/lc_updated.yml",
                }],
            ),
            // Remove clustering, issuing ALTER ... CLUSTER BY NONE.
            AdapterRecordReplayTurn::new(
                "run",
                &[FileOverride {
                    original: "models/lc_lifecycle.yml",
                    overriding_file: "_project_overrides/lc_removed.yml",
                }],
            ),
        ],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_metric_view() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_metric_view",
        AdapterType::Databricks.as_ref(),
        None,
        &[
            // Build each metric view model
            AdapterRecordReplayTurn::new("run", &[]),
            // Build basic_metric_view again -- there should be no change
            AdapterRecordReplayTurn::new("run --select basic_metric_view", &[]),
            // Test adding configs
            AdapterRecordReplayTurn::new(
                "run --select basic_metric_view",
                &[FileOverride {
                    original: "models/properties.yml",
                    overriding_file: "_project_overrides/add_configs.yml",
                }],
            ),
            // Test configs changing
            AdapterRecordReplayTurn::new(
                "run --select basic_metric_view",
                &[FileOverride {
                    original: "models/properties.yml",
                    overriding_file: "_project_overrides/update_configs.yml",
                }],
            ),
            // Test removing configs by going back to the original property file
            AdapterRecordReplayTurn::new("run --select basic_metric_view", &[]),
        ],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_databricks_refresh_schedule_lifecycle() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/databricks_refresh_schedule_lifecycle",
        AdapterType::Databricks.as_ref(),
        None,
        &[
            AdapterRecordReplayTurn::new("run-operation set_up", &[]),
            AdapterRecordReplayTurn::new("run", &[]),
            AdapterRecordReplayTurn::new(
                "run",
                &[FileOverride {
                    original: "dbt_project.yml",
                    overriding_file: "_project_overrides/swapped_modes.yml",
                }],
            ),
            AdapterRecordReplayTurn::new(
                "run",
                &[FileOverride {
                    original: "dbt_project.yml",
                    overriding_file: "_project_overrides/property_changed.yml",
                }],
            ),
            AdapterRecordReplayTurn::new(
                "run",
                &[FileOverride {
                    original: "dbt_project.yml",
                    overriding_file: "_project_overrides/cron.yml",
                }],
            ),
            AdapterRecordReplayTurn::new(
                "run",
                &[FileOverride {
                    original: "dbt_project.yml",
                    overriding_file: "_project_overrides/manual.yml",
                }],
            ),
        ],
        None,
        &[],
    )
    .await
}
