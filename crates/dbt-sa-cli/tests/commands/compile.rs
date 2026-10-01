use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::task::{ProjectEnv, TaskSeq};

use crate::common::TaskSeqExt;

#[dbt_runtime::test]
async fn compile_inline_quiet_preserves_sql() -> FsResult<()> {
    let env = ProjectEnv::immutable_sa("tests/data/hello_world")?;

    TaskSeq::new(current_function_name!())
        .fs_sa("compile --inline select(1) -q")
        .execute_in_with_env(&env, &[("target_env_var", "duckdb_mem")])
        .await?;

    Ok(())
}

#[dbt_runtime::test]
async fn compile_inline_show_none_suppresses_sql() -> FsResult<()> {
    let env = ProjectEnv::immutable_sa("tests/data/hello_world")?;

    TaskSeq::new(current_function_name!())
        .fs_sa("compile --inline select(1) --show none")
        .execute_in_with_env(&env, &[("target_env_var", "duckdb_mem")])
        .await?;

    Ok(())
}
