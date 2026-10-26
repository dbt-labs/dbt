//! Redshift adapter method and feature tests (see the `adapter` module).

use crate::common::FS_RUNNER;

use dbt_adapter_core::AdapterType;
use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::adapter_record_replay::{AdapterRecordReplayTurn, FileOverride};
use dbt_test_utils::adapter_record_replay::{
    build_fs_record_replay_task_seq, read_catalog_nodes, run_fs_record_replay_task_seq,
};

async fn run_fs_record_replay_redshift(test_name: &str, project_path: &str) -> FsResult<()> {
    run_fs_record_replay_redshift_with_flags(test_name, project_path, None).await
}

async fn run_fs_record_replay_redshift_with_flags(
    test_name: &str,
    project_path: &str,
    flags: Option<&str>,
) -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        test_name,
        project_path,
        AdapterType::Redshift.as_ref(),
        None,
        &["compile"],
        flags.map(str::to_string),
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_metadata_source_freshness() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/metadata_source_freshness",
        AdapterType::Redshift.as_ref(),
        None,
        // run_0 creates the relations the test reads.
        &["run-operation seed_redshift", "source freshness"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_create_schema() -> FsResult<()> {
    run_fs_record_replay_redshift(current_function_name!(), "tests/data/adapter/create_schema")
        .await
}

#[dbt_runtime::test]
async fn adapters_redshift_list_schemas() -> FsResult<()> {
    run_fs_record_replay_redshift(current_function_name!(), "tests/data/adapter/list_schemas").await
}

#[dbt_runtime::test]
async fn adapters_redshift_list_schemas_datasharing() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/list_schemas_datasharing",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_drop_schema() -> FsResult<()> {
    run_fs_record_replay_redshift(current_function_name!(), "tests/data/adapter/drop_schema").await
}

#[dbt_runtime::test]
async fn adapters_redshift_execute() -> FsResult<()> {
    run_fs_record_replay_redshift(current_function_name!(), "tests/data/adapter/execute").await
}

#[dbt_runtime::test]
async fn adapters_redshift_equals() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/equals",
        AdapterType::Redshift.as_ref(),
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_incremental_merge_schema_change() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/redshift_incremental_merge_schema_change",
        AdapterType::Redshift.as_ref(),
        None,
        &[
            AdapterRecordReplayTurn::new("run", &[]),
            AdapterRecordReplayTurn::new(
                "run",
                &[FileOverride {
                    original: "models/target.sql",
                    overriding_file: "_project_overrides/target_v2.sql",
                }],
            ),
        ],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
#[ignore = "FIXME: https://github.com/dbt-labs/fs/issues/5401"]
async fn adapters_redshift_materialized_view_config_changeset() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/materialized_view_config_changeset",
        AdapterType::Redshift.as_ref(),
        None,
        // run_0 creates the relations the test reads.
        &["run-operation seed_redshift", "run", "run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_get_relation() -> FsResult<()> {
    run_fs_record_replay_redshift(current_function_name!(), "tests/data/adapter/get_relation").await
}

#[dbt_runtime::test]
async fn adapters_redshift_list_relations_datasharing() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/list_relations_datasharing",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_drop_relation() -> FsResult<()> {
    run_fs_record_replay_redshift(current_function_name!(), "tests/data/adapter/drop_relation")
        .await
}

#[dbt_runtime::test]
async fn adapters_redshift_get_incremental_strategy_macro() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/get_incremental_strategy_macro",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_check_schema_exists() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/check_schema_exists",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_get_relation_from_cache() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/get_relation_from_cache",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_get_columns_in_relation() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/get_columns_in_relation",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_get_columns_in_relation_datasharing() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/get_columns_in_relation_datasharing",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_standardize_grants_dict_datasharing() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/standardize_grants_dict_datasharing",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_standardize_grants_dict_extended() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/standardize_grants_dict_extended",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_standardize_grants_dict_extended_datasharing() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/standardize_grants_dict_extended_datasharing",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_apply_grants_revoke_special_char() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/apply_grants_revoke_special_char",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_get_missing_columns() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/get_missing_columns",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_expand_target_column_types() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/expand_target_column_types",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_verify_database() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/verify_database",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_ra3_node_enabled_verify_database() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/ra3_node_enabled_verify_database",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_datasharing_config() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/redshift_datasharing_config",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_cross_database_use() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/redshift_cross_database_use",
        AdapterType::Redshift.as_ref(),
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_truncate_relation() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/truncate_relation",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_column_methods() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/column_methods",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_quote_as_configured_default() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/quote_as_configured_default",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_quote_as_configured_provided() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/quote_as_configured_provided",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_api_relation_create() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/api_relation_create",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_add_query() -> FsResult<()> {
    run_fs_record_replay_redshift(current_function_name!(), "tests/data/adapter/add_query").await
}

#[dbt_runtime::test]
async fn adapters_redshift_render_raw_constraint() -> FsResult<()> {
    run_fs_record_replay_redshift(
        current_function_name!(),
        "tests/data/adapter/render_raw_constraint",
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_run_catalog() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/test_project_jimmy",
        AdapterType::Redshift.as_ref(),
        None,
        &["run --write-catalog"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_catalog_datasharing() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/catalog_datasharing",
        AdapterType::Redshift.as_ref(),
        None,
        &["run --write-catalog"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_query_comment() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/query_comment",
        AdapterType::Redshift.as_ref(),
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_query_comment_with_split() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/query_comment_with_split",
        AdapterType::Redshift.as_ref(),
        None,
        &["run"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_store_failures() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/store_failures",
        AdapterType::Redshift.as_ref(),
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
async fn adapters_redshift_oid_column() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/redshift_oid",
        AdapterType::Redshift.as_ref(),
        None,
        &["build"],
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redshift_seed_empty() -> FsResult<()> {
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/seed_empty",
        AdapterType::Redshift.as_ref(),
        None,
        &["seed"],
        Some("--empty".to_string()),
        &[],
    )
    .await
}

// asserts that catalog.json comes back with `has_stats: false` for views
#[dbt_runtime::test]
async fn adapters_redshift_run_catalog_no_extended_stats() -> FsResult<()> {
    let (env, mut task_seq) = build_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/test_project_jimmy",
        "redshift",
        // Reuse the existing `adapters_redshift_run_catalog` recording as-is,
        // so its schema name must match what was recorded.
        Some("adapters_redshift_run_catalog".to_string()),
        &["run --write-catalog"],
        None,
    )
    .await?;

    task_seq.task_fn(|_project_env, test_env, _idx| {
        let nodes = read_catalog_nodes(test_env)?;
        assert!(!nodes.is_empty(), "expected at least one catalog node");

        for (unique_id, node) in &nodes {
            let stats = node["stats"]
                .as_object()
                .unwrap_or_else(|| panic!("node {unique_id} missing \"stats\" object"));
            assert_eq!(
                stats.len(),
                1,
                "expected only the placeholder \"has_stats\" field for {unique_id} \
                 while the Redshift catalog join stays disabled (see #15451); \
                 found: {stats:?}"
            );
            let has_stats = stats.get("has_stats").and_then(|v| v["value"].as_bool());
            assert_eq!(
                has_stats,
                Some(false),
                "expected has_stats=false for {unique_id}; found: {:?}",
                stats.get("has_stats")
            );
        }

        Ok(())
    });

    task_seq.execute_in_with_env(&env, &[]).await?;
    Ok(())
}

#[dbt_runtime::test]
async fn adapters_redshift_run_catalog_with_extended_stats() -> FsResult<()> {
    let (env, mut task_seq) = build_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &FS_RUNNER,
        current_function_name!(),
        "tests/data/adapter/catalog_with_extended_stats",
        "redshift",
        Some("adapters_redshift_run_catalog_with_extended_stats".to_string()),
        &["run --write-catalog"],
        None,
    )
    .await?;

    task_seq.task_fn(|_project_env, test_env, _idx| {
        let nodes = read_catalog_nodes(test_env)?;

        let table_node = nodes
            .iter()
            .find(|(id, _)| id.ends_with(".catalog_table"))
            .map(|(_, node)| node)
            .ok_or("catalog.json missing the catalog_table node")?;
        let table_stats = table_node["stats"]
            .as_object()
            .ok_or("catalog_table node missing \"stats\" object")?;
        assert_eq!(
            table_stats
                .get("has_stats")
                .and_then(|v| v["value"].as_bool()),
            Some(true),
            "expected has_stats=true for catalog_table (a real table with svv_table_info \
             stats); found: {table_stats:?}"
        );
        assert!(
            table_stats.contains_key("rows"),
            "expected a \"rows\" stat for catalog_table; found: {table_stats:?}"
        );
        assert!(
            table_stats.contains_key("size"),
            "expected a \"size\" stat for catalog_table; found: {table_stats:?}"
        );

        let view_node = nodes
            .iter()
            .find(|(id, _)| id.ends_with(".catalog_view"))
            .map(|(_, node)| node)
            .ok_or("catalog.json missing the catalog_view node")?;
        let view_stats = view_node["stats"]
            .as_object()
            .ok_or("catalog_view node missing \"stats\" object")?;
        assert_eq!(
            view_stats
                .get("has_stats")
                .and_then(|v| v["value"].as_bool()),
            Some(false),
            "expected has_stats=false for catalog_view (a view never has \
             svv_table_info stats); found: {view_stats:?}"
        );

        Ok(())
    });

    task_seq.execute_in_with_env(&env, &[]).await?;
    Ok(())
}
