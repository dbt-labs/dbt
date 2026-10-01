//! Tasks to support record and replay.

use async_trait::async_trait;

use crate::is_update_mode_with_warehouse;

use super::{ProjectEnv, Task, TestEnv, TestResult};

/// Task to enable/disable record and replay depending on the run mode.
pub struct RrTask {
    task: Box<dyn Task + Send + Sync>,
}

impl RrTask {
    pub fn new(task: Box<dyn Task + Send + Sync>) -> Self {
        Self { task }
    }
}

#[async_trait]
impl Task for RrTask {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        if is_update_mode_with_warehouse() {
            self.task.run(project_env, test_env, task_index).await
        } else {
            Ok(())
        }
    }
}
