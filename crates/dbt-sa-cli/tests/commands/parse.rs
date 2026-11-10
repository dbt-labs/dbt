use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::task::{ProjectEnv, TaskSeq};

use crate::common::TaskSeqExt;

#[dbt_runtime::test]
async fn parse_hello_world() -> FsResult<()> {
    let env = ProjectEnv::immutable_sa("tests/data/hello_world")?;
    TaskSeq::new(current_function_name!())
        .fs_sa("parse --show progress")
        .execute_in(&env)
        .await?;
    Ok(())
}
