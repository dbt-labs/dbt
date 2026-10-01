use super::{ProjectEnv, Task, TestEnv, TestResult};
use crate::task::{ArtifactComparisonTask, TestError};
use async_trait::async_trait;
use dbt_common::constants::DBT_TARGET_DIR_NAME;
use dbt_schemas::schemas::RunResultsArtifact;
use dbt_schemas::schemas::serde::typed_struct_from_json_file;
use std::sync::{Arc, Mutex};

/// Task that captures the `run_results.json` artifact from the target directory.
pub struct CaptureRunResults {
    captured: Arc<Mutex<Option<RunResultsArtifact>>>,
}

impl CaptureRunResults {
    pub fn new() -> Self {
        Self {
            captured: Arc::new(Mutex::new(None)),
        }
    }

    pub fn get_run_results(&self) -> Option<RunResultsArtifact> {
        self.captured.lock().unwrap().take()
    }
}

impl Default for CaptureRunResults {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Task for CaptureRunResults {
    async fn run(
        &self,
        _project_env: &ProjectEnv,
        test_env: &TestEnv,
        _task_index: usize,
    ) -> TestResult<()> {
        let target_dir = test_env.temp_dir.join(DBT_TARGET_DIR_NAME);
        let run_results =
            typed_struct_from_json_file(target_dir.join("run_results.json").as_path())?;
        *self.captured.lock().unwrap() = Some(run_results);
        Ok(())
    }
}

#[async_trait]
impl Task for Arc<CaptureRunResults> {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        self.as_ref().run(project_env, test_env, task_index).await
    }

    fn is_counted(&self) -> bool {
        false
    }
}

pub struct CompareRunResults {
    captured_run_results: Arc<CaptureRunResults>,
    pub ignored_field_paths: Vec<String>,
}

impl CompareRunResults {
    pub fn new(
        captured_run_results: Arc<CaptureRunResults>,
        ignored_field_paths: Vec<String>,
    ) -> Self {
        Self {
            captured_run_results,
            ignored_field_paths,
        }
    }
}

#[async_trait]
impl Task for CompareRunResults {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        let manifest = self
            .captured_run_results
            .get_run_results()
            .ok_or_else(|| TestError::new("missing captured run results"))?;
        ArtifactComparisonTask::new(
            "target/run_results.json",
            manifest,
            self.ignored_field_paths.clone(),
            true,
        )
        .run(project_env, test_env, task_index)
        .await
    }

    fn is_counted(&self) -> bool {
        false
    }
}
