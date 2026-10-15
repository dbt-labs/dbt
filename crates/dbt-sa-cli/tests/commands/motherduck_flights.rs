use std::sync::Arc;

use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::task::{ExecuteOnly, ProjectEnv, TaskSeq, TestError};

use crate::common::TaskSeqExt;

const ENABLE_ENV: &str = "DBT_TEST_MOTHERDUCK_FLIGHTS";

fn assert_success(command: &str, task: &ExecuteOnly) -> FsResult<()> {
    if task.get_exit_code() == 0 {
        return Ok(());
    }

    Err(TestError::new(format!(
        "`dbt {command}` failed with exit code {}.\nstdout:\n{}\nstderr:\n{}",
        task.get_exit_code(),
        task.get_stdout(),
        task.get_stderr()
    ))
    .into())
}

/// Live MotherDuck test. Opt in with DBT_TEST_MOTHERDUCK_FLIGHTS=1 and
/// MOTHERDUCK_TOKEN. DBT_TEST_MOTHERDUCK_DATABASE defaults to `my_db`.
#[test]
fn motherduck_flights_runs_and_reuses_python_model() -> FsResult<()> {
    let test = std::thread::Builder::new()
        .name("motherduck-flights-test".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let dbt_runtime = dbt_runtime::builder::Builder::new()
                .max_blocking_threads(4)
                .build();
            let _dbt_runtime_guard = dbt_runtime.handle().enter();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build Tokio runtime")
                .block_on(run_motherduck_flights_test())
        })
        .expect("failed to spawn MotherDuck Flights test thread");

    match test.join() {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

async fn run_motherduck_flights_test() -> FsResult<()> {
    if std::env::var(ENABLE_ENV).as_deref() != Ok("1") {
        eprintln!("skipping MotherDuck Flights test; set {ENABLE_ENV}=1 to run it");
        return Ok(());
    }
    let token = std::env::var("MOTHERDUCK_TOKEN")
        .map_err(|_| TestError::new(format!("MOTHERDUCK_TOKEN must be set when {ENABLE_ENV}=1")))?;
    let database =
        std::env::var("DBT_TEST_MOTHERDUCK_DATABASE").unwrap_or_else(|_| "my_db".to_string());

    let env = ProjectEnv::immutable_sa("tests/data/motherduck_flights")?;
    let schema = format!("dbt_flights_core_v2_test_{}", std::process::id());
    let mut tasks = TaskSeq::new(current_function_name!());
    let commands = [
        "build --target motherduck",
        "build --target motherduck",
        "run-operation cleanup_flight_test --target motherduck",
    ];
    let executions = commands
        .iter()
        .map(|command| tasks.fs_sa_execute_only(command))
        .collect::<Vec<Arc<ExecuteOnly>>>();

    tasks
        .task_fn(move |_, _, _| {
            for (command, task) in commands.iter().zip(&executions) {
                assert_success(command, task)?;
            }
            Ok(())
        })
        .execute_in_with_env(
            &env,
            &[
                ("MOTHERDUCK_TOKEN", &token),
                ("DBT_TEST_MOTHERDUCK_DATABASE", &database),
                ("DBT_TEST_MOTHERDUCK_SCHEMA", &schema),
            ],
        )
        .await?;

    Ok(())
}
