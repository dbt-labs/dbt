use std::ffi::OsStr;
use std::path::Path;

use dbt_common::stdfs;
use dbt_test_primitives::is_update_golden_files_mode;

use super::{ProjectEnv, TestEnv, TestError, TestResult};
use crate::task::goldie::diff_goldie;

/// Builds a task closure that reads a file from the project directory and
/// compares its content against a golden file. Updates the golden if
/// `GOLDIE_UPDATE=1`.
///
/// Works for any text file: `package-lock.yml`, `run_results.json`, etc.
pub fn compare_file_golden(
    name: impl Into<String>,
    file_path: impl Into<String>,
) -> impl Fn(&ProjectEnv, &TestEnv, usize) -> TestResult<()> {
    let name = name.into();
    let file_path = file_path.into();
    move |project_env: &ProjectEnv, test_env: &TestEnv, task_index: usize| {
        let target_path = project_env.absolute_project_dir.join(&file_path);
        if !target_path.exists() {
            return Err(TestError::new(format!(
                "File '{}' does not exist at '{}'",
                file_path,
                target_path.display()
            )));
        }

        let content = stdfs::read_to_string(&target_path)
            .map_err(|e| TestError::new(format!("Failed to read '{file_path}': {e}")))?;

        let task_suffix = if task_index > 0 {
            format!("_{task_index}")
        } else {
            String::new()
        };

        // Use the file extension as the golden file extension (e.g. .yml, .json)
        let ext = Path::new(&file_path)
            .extension()
            .and_then(OsStr::to_str)
            .unwrap_or("txt");
        let golden_name = format!("{name}{task_suffix}.{ext}");

        stdfs::create_dir_all(&test_env.golden_dir)
            .map_err(|e| TestError::new(format!("Failed to create golden dir: {e}")))?;

        let golden_path = test_env.golden_dir.join(&golden_name);

        if is_update_golden_files_mode() {
            stdfs::write(&golden_path, &content)
                .map_err(|e| TestError::new(format!("Failed to write golden file: {e}")))?;
            return Ok(());
        }

        if let Some(patch) = diff_goldie(ext, content, false, &golden_path, |g| g) {
            return Err(TestError::GoldieMismatch(vec![patch]));
        }

        Ok(())
    }
}
