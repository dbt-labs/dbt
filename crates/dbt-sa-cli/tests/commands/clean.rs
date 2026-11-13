use dbt_common::{FsResult, current_function_name};
use dbt_test_utils::task::{ProjectEnv, TaskSeq};

use crate::common::TaskSeqExt;

/// Like dbt-core, `clean` does not read `profiles.yml`, so a profile that does
/// not exist is not an error.
#[dbt_runtime::test]
async fn clean_ignores_missing_profile() -> FsResult<()> {
    let env = ProjectEnv::immutable_sa("tests/data/hello_world")?;

    TaskSeq::new(current_function_name!())
        .fs_sa("clean --profile does_not_exist")
        .execute_in(&env)
        .await?;

    Ok(())
}

/// Without a profile `target` is empty, as with dbt-core's `UnsetProfile`, so
/// `{{ target.name }}` in `clean-targets` renders as an empty string.
#[dbt_runtime::test]
async fn clean_without_profiles_yml_renders_empty_target() -> FsResult<()> {
    let env = ProjectEnv::immutable_sa("tests/data/clean_without_profile")?;

    TaskSeq::new(current_function_name!())
        .fs_sa("clean")
        .execute_in(&env)
        .await?;

    Ok(())
}
