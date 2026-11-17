//! Redis adapter features recorded against Redis Software, then replayed offline.
use crate::common::FS_RUNNER;
use dbt_adapter_core::AdapterType;
use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::adapter_record_replay::{fs_runner::FsRunner, run_fs_record_replay_task_seq};

struct AdapterCase {
    name: &'static str,
    project: &'static str,
    commands: &'static [&'static str],
}

async fn run_case(case: AdapterCase) -> FsResult<()> {
    let runner = FsRunner {
        default_static_analysis: Some("off"),
        ..FS_RUNNER
    };
    run_fs_record_replay_task_seq(
        env!("CARGO_MANIFEST_DIR"),
        &runner,
        case.name,
        case.project,
        AdapterType::RedisAdbc.as_ref(),
        None,
        case.commands,
        None,
        &[],
    )
    .await
}

#[dbt_runtime::test]
async fn adapters_redis_adbc_create_schema() -> FsResult<()> {
    run_case(AdapterCase {
        name: current_function_name!(),
        project: "tests/data/adapter/create_schema",
        commands: &["compile"],
    })
    .await
}

#[dbt_runtime::test]
async fn adapters_redis_adbc_get_relation_no_hydration() -> FsResult<()> {
    run_case(AdapterCase {
        name: current_function_name!(),
        project: "tests/data/adapter/get_relation_no_hydration",
        commands: &["run-operation check_get_relation"],
    })
    .await
}

#[dbt_runtime::test]
async fn adapters_redis_adbc_get_columns_in_relation() -> FsResult<()> {
    run_case(AdapterCase {
        name: current_function_name!(),
        project: "tests/data/adapter/get_columns_in_relation",
        commands: &["compile"],
    })
    .await
}

#[dbt_runtime::test]
async fn adapters_redis_adbc_basic_materializations() -> FsResult<()> {
    run_case(AdapterCase {
        name: current_function_name!(),
        project: "tests/data/adapter/basic_materializations",
        commands: &["seed", "run", "test", "run", "test"],
    })
    .await
}
