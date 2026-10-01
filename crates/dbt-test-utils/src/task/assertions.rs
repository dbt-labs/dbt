//! Tasks used as assertions in a task sequence.

use super::{ProjectEnv, Task, TestEnv, TestResult};

use async_trait::async_trait;

use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

/// Builds a task closure that asserts a specific file contains specific text.
pub fn assert_file_contains(
    rel_file_path: impl Into<PathBuf>,
    text: impl Into<String>,
    use_test_env_path: bool,
) -> impl Fn(&ProjectEnv, &TestEnv, usize) -> TestResult<()> {
    let rel_file_path = rel_file_path.into();
    let text = text.into();
    move |project_env: &ProjectEnv, test_env: &TestEnv, _task_index: usize| {
        // First we check that file actually exists.
        let path = if use_test_env_path {
            test_env.temp_dir.join(&rel_file_path)
        } else {
            project_env.absolute_project_dir.join(&rel_file_path)
        };

        assert!(path.exists(), "Path {} does not exist", path.display());

        let mut file = File::open(&path)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        assert!(
            contents.contains(&text),
            "File at {} does not contain text",
            path.display()
        );

        Ok(())
    }
}

/// Task asserts that specific file exists.
#[derive(Debug, Clone)]
pub struct AssertFileExistsTask {
    /// Path to file relative to project in which the task is running
    rel_file_path: PathBuf,
}

impl AssertFileExistsTask {
    pub fn new(rel_file_path: impl Into<PathBuf>) -> Self {
        Self {
            rel_file_path: rel_file_path.into(),
        }
    }
}

#[async_trait]
impl Task for AssertFileExistsTask {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        _test_env: &TestEnv,
        _task_index: usize,
    ) -> TestResult<()> {
        let path = project_env.absolute_project_dir.join(&self.rel_file_path);
        assert!(path.exists(), "Path {} does not exist", path.display());
        assert!(path.is_file(), "Path {} is not a file", path.display());

        Ok(())
    }
}

/// Task asserts that specific directory exists.
#[derive(Debug, Clone)]
pub struct AssertDirExistsTask {
    /// Path to dir relative to project in which the task is running
    rel_file_path: PathBuf,
}

impl AssertDirExistsTask {
    pub fn new(rel_file_path: impl Into<PathBuf>) -> Self {
        Self {
            rel_file_path: rel_file_path.into(),
        }
    }
}

#[async_trait]
impl Task for AssertDirExistsTask {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        _test_env: &TestEnv,
        _task_index: usize,
    ) -> TestResult<()> {
        let path = project_env.absolute_project_dir.join(&self.rel_file_path);
        assert!(path.exists(), "Path {} does not exist", path.display());
        assert!(path.is_dir(), "Path {} is not a directory", path.display());

        Ok(())
    }
}
