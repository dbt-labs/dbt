use crate::task::TestError;
use crate::task::env::TracingReloadHandle;

use super::tasks::{FnTask, ShExecute};
use super::utils::{check_set_user_env_var, redirect_buffer_to_stdin, strip_full_test_name};
use super::{ProjectEnv, Task, TestEnv, TestResult};

use dbt_common::error::FsResult;
use dbt_common::stdfs;
use dbt_common::string_utils::split_into_whitespace_and_brackets;
use dbt_common::tracing::reload::create_data_layer_for_tests;
use dbt_common::tracing::{
    TracingConfigProvider, dbt_data_layer_config, init_tracing_with_data_layer,
};
use dbt_features::feature_stack::FeatureStack;
use once_cell::sync::OnceCell;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::{fs, iter};

pub type FeatureStackFactory =
    dyn Fn(Arc<dyn TracingConfigProvider>) -> Arc<FeatureStack> + Send + Sync;

/// Global [Arc] of a [FeatureStackFactory] to be shared across tests.
pub static G_DBT_TEST_UTILS_FEATURE_STACK: OnceCell<Arc<FeatureStackFactory>> = OnceCell::new();

pub type BoxedSendFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
pub type CommandFn = dyn Fn(
        Vec<String>,
        PathBuf,
        PathBuf,
        fs::File,
        fs::File,
        TracingReloadHandle,
    ) -> BoxedSendFuture<FsResult<()>>
    + Send
    + Sync;

pub fn fs_cmd_vec(cmd: impl AsRef<str>) -> Vec<String> {
    let cmd_str = cmd.as_ref();
    let mut parts = split_into_whitespace_and_brackets(cmd_str);

    // Only add --show progress if --show is not already present
    if !parts.iter().any(|s| s == "--show") {
        parts.push("--show".to_string());
        parts.push("progress".to_string());
    }

    iter::once("fs".to_string())
        .chain(parts)
        .collect::<Vec<_>>()
}

/// A sequence of tasks. Created tasks are executed lazily. The
/// sequence can be executed multiple times using same or a different
/// workspace.
pub struct TaskSeq {
    name: String,
    full_name: String,
    tasks: Vec<Box<dyn Task>>,
}

impl TaskSeq {
    pub fn new(full_test_name: impl Into<String>) -> Self {
        let full_name = full_test_name.into();
        let name = strip_full_test_name(full_name.as_str());
        Self {
            name,
            full_name,
            tasks: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Construct a unique path for this test.
    pub fn unique_path(&self) -> PathBuf {
        PathBuf::from(self.full_name.replace("::", "/"))
    }

    /// Creates an arbitrary Task in the sequence. This can be useful
    /// to inspect target_dir or anything in the middle or at the end
    /// of the sequence.
    pub fn task(&mut self, task: Box<dyn Task>) -> &mut Self {
        self.tasks.push(task);
        self
    }

    /// Creates a helper task backed by a synchronous closure.
    pub fn task_fn<F>(&mut self, func: F) -> &mut Self
    where
        F: Fn(&ProjectEnv, &TestEnv, usize) -> TestResult<()> + Send + Sync + 'static,
    {
        self.task(Box::new(FnTask::new(func)))
    }

    /// Creates a task for run a shell command.  NOTE: using this
    /// command will lead to platform dependent tests and should be
    /// used as appropriate.
    pub fn sh(&mut self, cmd_vec: &[impl ToString]) -> &mut Self {
        self.task(Box::new(ShExecute::new(
            self.name().to_owned(),
            cmd_vec.iter().map(|s| s.to_string()).collect(),
        )))
    }

    /// Creates a touch task on the given path.
    pub fn touch(&mut self, path: impl Into<String>) -> &mut Self {
        let path = path.into();
        self.task_fn(move |_, _, _| {
            super::io::touch(PathBuf::from(&path))?;
            Ok(())
        })
    }

    /// Creates a task to write the given content to the file at the specified
    /// path.
    pub fn write_file(
        &mut self,
        file_path: impl Into<String>,
        content: impl Into<String>,
    ) -> &mut Self {
        self.task(Box::new(super::io::FileWriteTask::new(file_path, content)))
    }

    /// Creates a remove task to delete the file at the given path.
    pub fn rm_file(&mut self, path: impl Into<String>) -> &mut Self {
        let path = path.into();
        self.task_fn(move |_, _, _| {
            stdfs::remove_file(&path).expect("could not remove a file");
            Ok(())
        })
    }

    /// Executes this sequence in the given environment, with the given buffer
    /// as stdin.
    ///
    /// This is useful for testing commands that read from stdin, e.g. `run -i`.
    pub async fn execute_in_with_stdin(
        &self,
        workspace: &ProjectEnv,
        buffer: &str,
    ) -> TestResult<()> {
        let _temp_file = redirect_buffer_to_stdin(buffer)?;
        self.execute_in(workspace).await?;
        Ok(())
    }

    /// Executes this sequence in the given environment.
    pub async fn execute_in(&self, project_env: &ProjectEnv) -> TestResult<()> {
        self.execute_in_with_env(project_env, &[]).await
    }

    /// Executes this sequence in the given environment with optional environment variables.
    pub async fn execute_in_with_env(
        &self,
        project_env: &ProjectEnv,
        set_env: &[(&str, &str)],
    ) -> TestResult<()> {
        // Try initializing tracing. It will succeed only once per process, because it
        // sets global subscriber. We initialize with a special reloadable data layer
        // that can be populated with actual consumer layers by individual tasks.
        // We use fixed fallback trace ID of 1 for all tests for reproducibility.
        let (data_layer, reload_handle) =
            create_data_layer_for_tests(dbt_data_layer_config(1u128, None), vec![], vec![]);

        // Keep the guard alive for the duration of the test run so the process span
        // remains available to worker threads emitting telemetry.
        let process_span_guard = init_tracing_with_data_layer(
            tracing::level_filters::LevelFilter::TRACE,
            dbt_common::tracing::dbt_process_span_attributes("dbt-tests"),
            data_layer,
        )?;

        let mut set_env = set_env.to_vec();

        let enable_query_cache_testing =
            std::env::var("DBT_TEST_QUERY_CACHE") == Ok("1".to_string());
        if enable_query_cache_testing {
            set_env.push(("DBT_TEST_QUERY_CACHE", "1"));
        }

        {
            let mut test_env = project_env.create_test_env()?;
            test_env = test_env.with_tracing_handle(reload_handle.clone());
            let _cwd_guard = CurrentWorkingDirGuard::new(&project_env.absolute_project_dir);
            run_test_tasks(&self.tasks, project_env, &test_env, &set_env).await?;
        }

        // Explicitly drop the guard only after all telemetry-producing work finishes.
        drop(process_span_guard);

        Ok(())
    }
}

struct CurrentWorkingDirGuard {
    original_dir: PathBuf,
}

impl CurrentWorkingDirGuard {
    fn new(dir: impl AsRef<Path>) -> Self {
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.as_ref()).unwrap();
        Self { original_dir }
    }
}

impl Drop for CurrentWorkingDirGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.original_dir).unwrap();
    }
}

async fn run_test_tasks(
    tasks: &[Box<dyn Task + '_>],
    project_env: &ProjectEnv,
    test_env: &TestEnv,
    set_env: &[(&str, &str)],
) -> TestResult<()> {
    use crate::test_env_guard::TestEnvGuard;

    // Create environment guard to isolate tests from external environment variables
    let _env_guard = TestEnvGuard::default();

    // Isolate tests from the developer's `~/.dbt/dbt_cloud.yml` by pointing
    // the cloud-config loader at a non-existent per-process directory. The
    // loader's NotFound branch returns `Ok(None)`, so any lookup sees "no
    // dbt_cloud.yml". Nextest runs each test in its own process, so no
    // restore is needed.
    unsafe {
        #[allow(clippy::disallowed_methods)]
        std::env::set_var(
            dbt_cloud_config::TEST_CLOUD_CONFIG_DIR_ENV,
            std::env::temp_dir().join(format!("dbt-test-no-cloud-config-{}", std::process::id())),
        );
    }

    // Set provided environment variables (may be empty)
    for (key, value) in set_env {
        #[allow(clippy::disallowed_methods)]
        unsafe {
            std::env::set_var(key, value)
        };
    }

    check_set_user_env_var();

    let mut index = 0;
    let mut patches = vec![];
    for task in tasks {
        match task.run(project_env, test_env, index).await {
            Ok(()) => {}
            Err(TestError::GoldieMismatch(p)) => {
                patches.extend(p);
            }
            Err(e) => return Err(e),
        }
        if task.is_counted() {
            index += 1;
        }
    }
    if !patches.is_empty() {
        eprintln!("<<<<<<<< BEGIN PATCH");
        for patch in patches {
            eprintln!("{patch}");
        }
        eprintln!(">>>>>>>> END PATCH");
        panic!(
            "Test case output does not match one or more golden files. See diff above. \
        To accept this output as golden file, open a terminal in the root of the git repository and run: \
          `git apply -` \
        then copy-paste the diff above into the terminal and press Ctrl+D.\
        (Note: if you're copy-pasting from the Github web UI, run `sed 's/^    //' | git apply -` instead) \
        ",
        )
    }

    Ok(())
}
