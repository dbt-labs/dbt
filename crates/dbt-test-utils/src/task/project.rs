//! Tasks for working with dbt project files.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::task::{ProjectEnv, Task, TestEnv, TestError, TestResult};

/// Overrides a file in the temp project dir with a different file.
/// Used to simulate config changes between run steps in a multi-run test.
pub struct OverrideFileTask {
    /// Absolute path to the overriding file.
    pub source: PathBuf,
    /// Relative path within the project dir to override.
    pub target: PathBuf,
}

#[async_trait]
impl Task for OverrideFileTask {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        _test_env: &TestEnv,
        _task_index: usize,
    ) -> TestResult<()> {
        let dest = project_env.absolute_project_dir.join(&self.target);
        std::fs::copy(&self.source, &dest).map_err(|e| {
            TestError::new(format!(
                "failed to copy project override {:?} -> {:?}: {e}",
                self.source, dest
            ))
        })?;
        Ok(())
    }
}
