use std::sync::Arc;

use dbt_common::{FsResult, current_function_name};
use dbt_schemas::schemas::{RunResultOutput, RunResultsArtifact};
use dbt_test_utils::task::{CaptureRunResults, ExecuteOnly, ProjectEnv, TaskSeq};

use crate::common::{make_fs_cmd_vec, make_fs_command_fn};

struct Outcome {
    exit_code: i32,
    stderr: String,
    run_results: RunResultsArtifact,
}

impl Outcome {
    fn only_result(&self) -> &RunResultOutput {
        match self.run_results.results.as_slice() {
            [result] => result,
            results => panic!("expected exactly one result, got {results:?}"),
        }
    }
}

/// Runs `command` against the `run_operation` fixture and reads `run_results.json`.
///
/// Tracing initializes once per process, so a test may call this only once.
async fn run(test_name: &str, command: &str) -> FsResult<Outcome> {
    let env = ProjectEnv::immutable_sa("tests/data/run_operation")?;
    let mut seq = TaskSeq::new(test_name);

    // `ExecuteOnly` does not unwrap `{...}` groups, so do that here. An argument
    // with spaces has to stay one token.
    let cmd_vec = make_fs_cmd_vec(command)
        .into_iter()
        .map(
            |arg| match arg.strip_prefix('{').and_then(|a| a.strip_suffix('}')) {
                Some(unwrapped) => unwrapped.to_owned(),
                None => arg,
            },
        )
        .collect();

    let execute = Arc::new(
        ExecuteOnly::new(seq.name().to_owned(), cmd_vec, make_fs_command_fn(), true)
            .with_allow_failure(true),
    );
    let capture = Arc::new(CaptureRunResults::new());

    seq.task(Box::new(Arc::clone(&execute)))
        .task(Box::new(Arc::clone(&capture)));
    seq.execute_in(&env).await?;

    Ok(Outcome {
        exit_code: execute.get_exit_code(),
        stderr: execute.get_stderr(),
        run_results: capture
            .get_run_results()
            .expect("the command should have written run_results.json"),
    })
}

#[dbt_runtime::test]
async fn run_operation_success_writes_one_operation_result() -> FsResult<()> {
    let outcome = run(current_function_name!(), "run-operation ok").await?;

    assert_eq!(outcome.exit_code, 0);
    assert_eq!(outcome.run_results.args.which, "run-operation");

    let result = outcome.only_result();
    assert_eq!(result.unique_id, "ok");
    assert_eq!(result.status, "success");

    Ok(())
}

#[dbt_runtime::test]
async fn failed_run_operation_writes_one_operation_result() -> FsResult<()> {
    let outcome = run(current_function_name!(), "run-operation boom_compile").await?;

    assert_eq!(outcome.exit_code, 1);
    assert_eq!(outcome.run_results.args.which, "run-operation");
    assert!(
        outcome.stderr.contains("intentional compile boom"),
        "stderr lost the macro error: {}",
        outcome.stderr
    );

    let result = outcome.only_result();
    assert_eq!(result.unique_id, "boom_compile");
    assert_eq!(result.status, "error");
    assert_ne!(result.message.as_deref(), Some("Compilation Error"));

    Ok(())
}

#[dbt_runtime::test]
async fn failed_run_operation_query_writes_one_operation_result() -> FsResult<()> {
    let outcome = run(current_function_name!(), "run-operation boom_sql").await?;

    assert_eq!(outcome.exit_code, 1);
    assert!(
        outcome.stderr.contains("this_table_does_not_exist_zz"),
        "stderr lost the query error: {}",
        outcome.stderr
    );

    let result = outcome.only_result();
    assert_eq!(result.unique_id, "boom_sql");
    assert_eq!(result.status, "error");
    assert_ne!(result.message.as_deref(), Some("Compilation Error"));

    Ok(())
}

#[dbt_runtime::test]
async fn failed_inline_sql_run_operation_writes_one_operation_result() -> FsResult<()> {
    let outcome = run(
        current_function_name!(),
        "run-operation --sql {select * from this_table_does_not_exist_zz}",
    )
    .await?;

    assert_eq!(outcome.exit_code, 1);

    let result = outcome.only_result();
    assert_eq!(result.unique_id, "sql_operation.run_operation.inline_query");
    assert_eq!(result.status, "error");
    assert_ne!(result.message.as_deref(), Some("Compilation Error"));

    Ok(())
}

#[dbt_runtime::test]
async fn failed_run_records_the_selected_model() -> FsResult<()> {
    let outcome = run(current_function_name!(), "run --select broken").await?;

    assert_eq!(outcome.exit_code, 1);

    let result = outcome.only_result();
    assert_eq!(result.unique_id, "model.run_operation.broken");
    assert_eq!(result.status, "error");
    assert_ne!(result.message.as_deref(), Some("Compilation Error"));

    Ok(())
}
