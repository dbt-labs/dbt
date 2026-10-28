//! Databricks materialization and clone record/replay tests.

use std::sync::Arc;

use crate::common::FS_RUNNER;
use dbt_adapter_core::AdapterType;
use dbt_common::{FsResult, current_function_name, stdfs};
use dbt_test_utils::adapter_record_replay::{fs_runner::FsRunner, run_fs_record_replay_task_seq};
use dbt_test_utils::task::CommandFn;

fn clone_command_fn() -> Arc<CommandFn> {
    let run = (FS_RUNNER.command_fn)();
    Arc::new(move |cmd, project, target, stdout, stderr, tracing| {
        let save_manifest = cmd.iter().any(|arg| arg == "run");
        let run = Arc::clone(&run);
        Box::pin(async move {
            run(
                cmd,
                project.clone(),
                target.clone(),
                stdout,
                stderr,
                tracing,
            )
            .await?;
            if save_manifest {
                // Later commands overwrite target/manifest.json; clone needs the prod aliases.
                let state = project.join("state");
                stdfs::create_dir_all(&state)?;
                stdfs::copy(target.join("manifest.json"), state.join("manifest.json"))?;
            }
            Ok(())
        })
    })
}

/// Regression coverage for https://github.com/dbt-labs/dbt/pull/16891.
#[dbt_runtime::test]
async fn adapters_databricks_clone_governed_views() -> FsResult<()> {
    let runner = FsRunner {
        command_fn: clone_command_fn,
        default_static_analysis: Some("off"),
        ..FS_RUNNER
    };
    let credentials: Vec<_> = [
        "DBT_DATABRICKS_HOST_NAME",
        "DBT_DATABRICKS_UC_ENDPOINT_HTTP_PATH",
        "DBT_DATABRICKS_TOKEN",
    ]
    .into_iter()
    .filter_map(|key| std::env::var(key).ok().map(|value| (key, value)))
    .collect();
    let credentials: Vec<_> = credentials
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &runner,
        current_function_name!(),
        "tests/data/adapter/databricks_clone_governed_views",
        AdapterType::Databricks.as_ref(),
        None,
        &[
            "run-operation create_policy_functions",
            "seed",
            "run --vars {phase: prod}",
            "clone --state state --select governed_mv filtered_st masked_st",
            "run-operation assert_cloned_views",
            "run-operation replace_clones_with_sentinels",
            "clone --state state --full-refresh --select governed_mv filtered_st masked_st",
            "run-operation assert_cloned_views",
        ],
        None,
        &credentials,
    )
    .await
}
