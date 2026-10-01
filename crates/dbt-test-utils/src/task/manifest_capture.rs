use super::{ProjectEnv, Task, TestEnv, TestResult};
use crate::task::{ArtifactComparisonTask, TestError};
use async_trait::async_trait;
use dbt_common::constants::{DBT_MANIFEST_JSON, DBT_TARGET_DIR_NAME};
use dbt_schemas::schemas::manifest::DbtManifest;
use std::sync::{Arc, Mutex};

/// A task that captures the DbtManifest from the target directory
pub struct CaptureDbtManifest {
    captured_manifest: Arc<Mutex<Option<DbtManifest>>>,
}

impl CaptureDbtManifest {
    /// Create a new manifest capturing task
    pub fn new() -> Self {
        Self {
            captured_manifest: Arc::new(Mutex::new(None)),
        }
    }

    /// Get the captured manifest after execution
    pub fn get_manifest(&self) -> Option<DbtManifest> {
        self.captured_manifest.lock().unwrap().take()
    }
}

impl Default for CaptureDbtManifest {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Task for CaptureDbtManifest {
    async fn run(
        &self,
        _project_env: &ProjectEnv,
        test_env: &TestEnv,
        _task_index: usize,
    ) -> TestResult<()> {
        // Read the manifest from the target directory
        let target_dir = test_env.temp_dir.join(DBT_TARGET_DIR_NAME);
        let manifest: DbtManifest = dbt_schemas::schemas::serde::typed_struct_from_json_file(
            target_dir.join(DBT_MANIFEST_JSON).as_path(),
        )?;
        // Store the captured manifest
        *self.captured_manifest.lock().unwrap() = Some(manifest);

        Ok(())
    }
}

#[async_trait]
impl Task for Arc<CaptureDbtManifest> {
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

pub struct CompareDbtManifest {
    captured_manifest: Arc<CaptureDbtManifest>,
    pub ignored_field_paths: Vec<String>,
}

impl CompareDbtManifest {
    pub fn new(
        captured_manifest: Arc<CaptureDbtManifest>,
        ignored_field_paths: Vec<String>,
    ) -> Self {
        Self {
            captured_manifest,
            ignored_field_paths,
        }
    }
}

#[async_trait]
impl Task for CompareDbtManifest {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        let manifest = self
            .captured_manifest
            .get_manifest()
            .ok_or_else(|| TestError::new("missing captured manifest"))?;
        ArtifactComparisonTask::new(
            "target/manifest.json",
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
