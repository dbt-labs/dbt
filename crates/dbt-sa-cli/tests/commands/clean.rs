use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::task::{ProjectEnv, TaskSeq};

use crate::common::TaskSeqExt;

/// RED: dbt#16534. `dbt clean --profile <missing>` exits 1 but prints
/// "Finished 'clean' successfully" and never shows the profile error that `dbt run` shows.
#[dbt_runtime::test]
async fn clean_missing_profile_reports_error() -> FsResult<()> {
    let env = ProjectEnv::immutable_sa("tests/data/hello_world")?;
    TaskSeq::new(current_function_name!())
        .fs_sa("clean --profile does_not_exist")
        .execute_in(&env)
        .await?;
    Ok(())
}
