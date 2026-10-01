mod assertions {
    use dbt_common::{FsResult, current_function_name};
    use dbt_test_utils::task::{
        AssertDirExistsTask, AssertFileExistsTask, ProjectEnv, TaskSeq, assert_file_contains,
    };

    #[dbt_runtime::test]
    async fn tasks_file_contains() -> FsResult<()> {
        let root = env!("CARGO_MANIFEST_DIR");
        let env = ProjectEnv::immutable_from(root, "tests/data/hello")?;

        TaskSeq::new(current_function_name!())
            .task_fn(assert_file_contains("profiles.yml", "datafusion", false))
            .execute_in(&env)
            .await?;

        Ok(())
    }

    #[dbt_runtime::test]
    async fn tasks_file_exists() -> FsResult<()> {
        let root = env!("CARGO_MANIFEST_DIR");
        let env = ProjectEnv::immutable_from(root, "tests/data/hello")?;

        TaskSeq::new(current_function_name!())
            .task(Box::new(AssertFileExistsTask::new("profiles.yml")))
            .execute_in(&env)
            .await?;

        Ok(())
    }

    #[dbt_runtime::test]
    async fn tasks_dir_exists() -> FsResult<()> {
        let root = env!("CARGO_MANIFEST_DIR");
        let env = ProjectEnv::immutable_from(root, "tests/data/hello")?;

        TaskSeq::new(current_function_name!())
            .task(Box::new(AssertDirExistsTask::new("models")))
            .execute_in(&env)
            .await?;

        Ok(())
    }
}
