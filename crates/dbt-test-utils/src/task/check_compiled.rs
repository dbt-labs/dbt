use std::path::Component;

use crate::task::{ProjectEnv, Task, TestEnv, TestResult, utils::iter_files_recursively};
use async_trait::async_trait;
use dbt_common::{
    constants::{DBT_COMPILED_DIR_NAME, DBT_TARGET_DIR_NAME},
    stdfs,
};
use dbt_test_primitives::is_update_golden_files_mode;

#[derive(Default)]
pub struct CheckCompiledFiles {
    /// When `false` (the default), compiled hook SQL (any path with a `hooks`
    /// component) is skipped — most fixtures have no golden files for hooks and
    /// some emit non-deterministic teardown SQL. Opt in to check hooks.
    pub check_hooks: bool,
}

#[async_trait]
impl Task for CheckCompiledFiles {
    async fn run(&self, _: &ProjectEnv, test_env: &TestEnv, _task_index: usize) -> TestResult<()> {
        iter_files_recursively(
            test_env
                .temp_dir
                .join(DBT_TARGET_DIR_NAME)
                .join(DBT_COMPILED_DIR_NAME)
                .as_path(),
            &|abs_path| {
                let path_str = abs_path.as_os_str().to_str().unwrap_or_default();
                let has_hooks = abs_path
                    .components()
                    .any(|c| c == Component::Normal("hooks".as_ref()));
                if path_str.ends_with(".sql") && (self.check_hooks || !has_hooks) {
                    let actual = stdfs::read_to_string(abs_path)?;
                    let path =
                        stdfs::diff_paths(abs_path, test_env.temp_dir.join(DBT_TARGET_DIR_NAME))
                            .unwrap();
                    let golden_path = test_env.golden_dir.join(path);
                    if is_update_golden_files_mode() {
                        if let Some(parent_dir) = golden_path.parent() {
                            stdfs::create_dir_all(parent_dir)?;
                        }
                        stdfs::write(golden_path, actual).unwrap();
                    } else {
                        let expected = stdfs::read_to_string(&golden_path).unwrap();
                        assert_eq!(actual, expected);
                    }
                }

                Ok(())
            },
        )
        .await?;

        Ok(())
    }
}
