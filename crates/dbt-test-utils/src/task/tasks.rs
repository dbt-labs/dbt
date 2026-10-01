//! Core tasks.

use std::{
    io::Write,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex, atomic::AtomicI32},
};

use async_trait::async_trait;
use dbt_common::{
    FsError, FsResult,
    constants::{DBT_INTERNAL_PACKAGES_DIR_NAME, DBT_LOG_DIR_NAME, DBT_TARGET_DIR_NAME},
    stdfs,
};
use dbt_test_primitives::is_update_golden_files_mode;

use crate::task::goldie::{OutputNormalizer, compare_or_update};

use super::{
    ProjectEnv, Task, TestEnv, TestError, TestResult, goldie::execute_and_compare,
    task_seq::CommandFn,
};

pub use super::telemetry::ExecuteAndCompareTelemetry;

/// Common helper function to prepare command vector with standard DBT paths and options
pub fn prepare_command_vec(
    mut cmd_vec: Vec<String>,
    project_env: &ProjectEnv,
    test_env: &TestEnv,
    filter_brackets: bool,
) -> Vec<String> {
    let project_dir = &project_env.absolute_project_dir;
    let target_dir = &test_env.temp_dir.join(DBT_TARGET_DIR_NAME);
    let logs_dir = &test_env.temp_dir.join(DBT_LOG_DIR_NAME);
    let internal_packages_install_path = &test_env.temp_dir.join(DBT_INTERNAL_PACKAGES_DIR_NAME);

    // Filter command arguments if requested (for ExecuteAndCompare)
    if filter_brackets {
        cmd_vec = cmd_vec
            .iter()
            .map(|cmd| {
                if cmd.starts_with('{') && cmd.ends_with('}') {
                    cmd[1..cmd.len() - 1].to_string()
                } else {
                    cmd.to_string()
                }
            })
            .collect();
    }

    // Redirect logs unless it is already specified
    if !cmd_vec.iter().any(|s| s.starts_with("--log-path")) {
        cmd_vec.push(format!("--log-path={}", logs_dir.display()));
    }

    // Add standard DBT flags (allow thetest to fail if caller added them manually)
    cmd_vec.push(format!("--target-path={}", target_dir.display()));
    cmd_vec.push(format!("--project-dir={}", project_dir.display()));
    cmd_vec.push(format!(
        "--internal-packages-install-path={}",
        internal_packages_install_path.display()
    ));

    cmd_vec
}

/// A task that executes a command without comparing output to goldie files and captures stdout and stderr.
pub struct ExecuteOnly {
    name: String,
    cmd_vec: Vec<String>,
    func: Arc<CommandFn>,
    redirect_outputs: bool,
    allow_failure: bool,
    stdout_name: Arc<Mutex<Option<String>>>,
    stderr_name: Arc<Mutex<Option<String>>>,
    stdout: Arc<Mutex<String>>,
    stderr: Arc<Mutex<String>>,
    exit_code: AtomicI32,
}

impl ExecuteOnly {
    /// Construct a new execute only task.
    ///
    /// If `redirect_outputs` is true, `target-path`, `project-dir`, and `log-path`
    /// will be added to the command vector automatically.
    pub fn new(
        name: String,
        cmd_vec: Vec<String>,
        func: Arc<CommandFn>,
        redirect_outputs: bool,
    ) -> Self {
        Self {
            name,
            cmd_vec,
            func,
            redirect_outputs,
            allow_failure: false,
            stdout_name: Arc::new(Mutex::new(None)),
            stderr_name: Arc::new(Mutex::new(None)),
            stdout: Arc::new(Mutex::new(String::default())),
            stderr: Arc::new(Mutex::new(String::default())),
            exit_code: AtomicI32::new(0),
        }
    }

    pub fn with_allow_failure(mut self, allow_failure: bool) -> Self {
        self.allow_failure = allow_failure;
        self
    }

    pub fn get_exit_code(&self) -> i32 {
        self.exit_code.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn get_stdout(&self) -> String {
        self.stdout.lock().expect("Lock is poisoned").clone()
    }

    pub fn get_stderr(&self) -> String {
        self.stderr.lock().expect("Lock is poisoned").clone()
    }

    pub fn get_stdout_name(&self) -> Option<String> {
        self.stdout_name.lock().expect("Lock is poisoned").clone()
    }

    pub fn get_stderr_name(&self) -> Option<String> {
        self.stderr_name.lock().expect("Lock is poisoned").clone()
    }
}

#[async_trait]
impl Task for ExecuteOnly {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        let mut cmd_vec = self.cmd_vec.clone();

        let mut target_dir = project_env.absolute_project_dir.join(DBT_TARGET_DIR_NAME);
        // Prepare cli command using the common helper if `redirect_outputs` is true
        if self.redirect_outputs {
            cmd_vec = prepare_command_vec(
                cmd_vec,
                project_env,
                test_env,
                false, // don't filter brackets for ExecuteOnly
            );
            target_dir = test_env.temp_dir.join(DBT_TARGET_DIR_NAME);
        }

        // Create stdout and stderr files
        let task_suffix = if task_index > 0 {
            format!("_{task_index}")
        } else {
            "".to_string()
        };
        let stdout_name = format!("{}{}.stdout", self.name, task_suffix);
        let stderr_name = format!("{}{}.stderr", self.name, task_suffix);
        *self.stdout_name.lock().unwrap() = Some(stdout_name.clone());
        *self.stderr_name.lock().unwrap() = Some(stderr_name.clone());
        let stdout_path = test_env.temp_dir.join(stdout_name);
        let stderr_path = test_env.temp_dir.join(stderr_name);

        let stdout_file = stdfs::File::create(&stdout_path)?;
        let stderr_file = stdfs::File::create(&stderr_path)?;

        // Execute the command
        let res = (self.func)(
            cmd_vec,
            project_env.absolute_project_dir.clone(),
            target_dir,
            stdout_file,
            stderr_file,
            test_env.get_tracing_handle(),
        )
        .await;

        // Store stdout and stderr contents in the struct for later access if needed
        *self.stdout.lock().unwrap() = stdfs::read_to_string(&stdout_path)?;
        *self.stderr.lock().unwrap() = stdfs::read_to_string(&stderr_path)?;

        let res = match res {
            Ok(()) => Ok(0),
            Err(err) => match err.exit_status() {
                Some(code) => Ok(code),
                None => Err(err),
            },
        };
        match res {
            Ok(exit_code) => {
                self.exit_code
                    .store(exit_code, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
            Err(_e) if self.allow_failure => {
                // We still want access to captured stdout/stderr even if the command failed.
                self.exit_code.store(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    fn is_counted(&self) -> bool {
        true
    }
}

#[async_trait]
impl Task for Arc<ExecuteOnly> {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        self.as_ref().run(project_env, test_env, task_index).await
    }

    fn is_counted(&self) -> bool {
        true
    }
}

/// Compare stdout/stderr captured by `ExecuteOnly` against golden files.
///
/// Use this when you need to inspect captured output or run intermediate tasks
/// before the snapshot comparison. If you only need run+compare with the
/// standard normalization, prefer `ExecuteAndCompare`.
pub struct CompareStdoutStderr {
    name: String,
    execute_task: Arc<ExecuteOnly>,
    extra_normalizers: Vec<OutputNormalizer>,
}

impl CompareStdoutStderr {
    pub fn new(
        name: impl Into<String>,
        execute_task: Arc<ExecuteOnly>,
        extra_normalizers: Vec<OutputNormalizer>,
    ) -> Self {
        let name = name.into();
        Self {
            name,
            execute_task,
            extra_normalizers,
        }
    }
}

#[async_trait]
impl Task for CompareStdoutStderr {
    async fn run(
        &self,
        _project_env: &ProjectEnv,
        test_env: &TestEnv,
        _task_index: usize,
    ) -> TestResult<()> {
        if self.name != self.execute_task.name {
            return Err(TestError::new(format!(
                "CompareStdoutStderr name '{}' must match ExecuteOnly name '{}'",
                self.name, self.execute_task.name,
            )));
        }

        let stdout_name = self.execute_task.get_stdout_name().ok_or_else(|| {
            TestError::new(
                "ExecuteOnly did not record stdout file name. Ensure ExecuteOnly runs before CompareStdoutStderr.",
            )
        })?;
        let stderr_name = self.execute_task.get_stderr_name().ok_or_else(|| {
            TestError::new(
                "ExecuteOnly did not record stderr file name. Ensure ExecuteOnly runs before CompareStdoutStderr.",
            )
        })?;

        let stdout_path = test_env.temp_dir.join(&stdout_name);
        let stderr_path = test_env.temp_dir.join(&stderr_name);
        if !stdout_path.exists() || !stderr_path.exists() {
            return Err(TestError::new(format!(
                "CompareStdoutStderr expected ExecuteOnly outputs at '{}' and '{}'. \
Ensure ExecuteOnly completed before comparison.",
                stdout_path.display(),
                stderr_path.display(),
            )));
        }

        let goldie_stdout_path = test_env.golden_dir.join(stdout_name);
        let goldie_stderr_path = test_env.golden_dir.join(stderr_name);
        let patches = compare_or_update(
            is_update_golden_files_mode(),
            false,
            stderr_path,
            goldie_stderr_path,
            stdout_path,
            goldie_stdout_path,
            &self.extra_normalizers,
        )?;

        if patches.is_empty() {
            Ok(())
        } else {
            Err(TestError::GoldieMismatch(patches))
        }
    }

    fn is_counted(&self) -> bool {
        false
    }
}

pub struct ExecuteAndCompare {
    name: String,
    cmd_vec: Vec<String>,
    threads: usize,
    use_recording: bool,
    func: Arc<CommandFn>,
    normalizers: Vec<OutputNormalizer>,
}

impl ExecuteAndCompare {
    /// Construct a new sequential execute and compare task
    pub fn new(
        name: String,
        mut cmd_vec: Vec<String>,
        func: Arc<CommandFn>,
        use_recording: bool,
    ) -> Self {
        // `--no-parallel` forces sequential task execution for deterministic
        // golden output without shrinking the `dbt-runtime` blocking pool via
        // `--threads`.
        cmd_vec.push("--no-parallel".to_string());
        if !cmd_vec.iter().any(|s| *s == "--log-format") {
            cmd_vec.push("--log-format=text".to_string());
        }

        Self {
            name,
            cmd_vec,
            threads: 1,
            use_recording,
            func,
            normalizers: vec![],
        }
    }

    /// Construct a new parallel execute and compare task
    pub fn new_parallel(
        name: String,
        mut cmd_vec: Vec<String>,
        func: Arc<CommandFn>,
        threads: usize,
    ) -> Self {
        cmd_vec.push(format!("--threads={threads}"));
        if !cmd_vec.iter().any(|s| *s == "--log-format") {
            cmd_vec.push("--log-format=text".to_string());
        }

        Self {
            name,
            cmd_vec,
            // Cannot use recording in parallel mode since order of events is
            // not deterministic
            threads,
            use_recording: false,
            func,
            normalizers: vec![],
        }
    }

    /// Set extra output normalizers applied before golden comparison.
    pub fn with_normalizers(mut self, normalizers: Vec<OutputNormalizer>) -> Self {
        self.normalizers = normalizers;
        self
    }
}

#[async_trait]
impl Task for ExecuteAndCompare {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        // Prepare cli command using the common helper
        let mut cmd_vec = prepare_command_vec(
            self.cmd_vec.clone(),
            project_env,
            test_env,
            true, // filter brackets for ExecuteAndCompare
        );

        // Add recording flag if needed
        if self.use_recording {
            cmd_vec.push(format!(
                "--dbt-replay={}",
                test_env
                    .golden_dir
                    .join(format!("recording_{task_index}.json"))
                    .display()
            ));
        }

        match execute_and_compare(
            &self.name,
            cmd_vec.as_slice(),
            project_env,
            test_env,
            task_index,
            self.threads != 1,
            self.func.clone(),
            &self.normalizers,
        )
        .await
        {
            Ok(patches) if patches.is_empty() => Ok(()),
            Ok(patches) => Err(TestError::GoldieMismatch(patches)),
            Err(e) => Err(e.into()),
        }
    }

    fn is_counted(&self) -> bool {
        true
    }
}

pub struct FnTask<F> {
    func: F,
    counted: bool,
}

impl<F> FnTask<F> {
    pub fn new(func: F) -> Self {
        Self {
            func,
            counted: false,
        }
    }

    pub fn counted(mut self) -> Self {
        self.counted = true;
        self
    }
}

#[async_trait]
impl<F> Task for FnTask<F>
where
    F: Fn(&ProjectEnv, &TestEnv, usize) -> TestResult<()> + Send + Sync,
{
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        (self.func)(project_env, test_env, task_index)
    }

    fn is_counted(&self) -> bool {
        self.counted
    }
}

/// Task to execute any sh command.
pub struct ShExecute {
    name: String,
    cmd_vec: Vec<String>,
}

impl ShExecute {
    pub fn new(name: String, raw_cmd: Vec<String>) -> Self {
        Self {
            name,
            cmd_vec: raw_cmd,
        }
    }
}

#[async_trait]
impl Task for ShExecute {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        let boxed_fn: Arc<CommandFn> = Arc::new(|cmd_vec, dir, _, stdout, stderr, _| {
            Box::pin(exec_sh(cmd_vec, dir, stdout, stderr))
        });

        match execute_and_compare(
            &self.name,
            self.cmd_vec.as_slice(),
            project_env,
            test_env,
            task_index,
            false,
            boxed_fn,
            &[],
        )
        .await
        {
            Ok(patches) if patches.is_empty() => Ok(()),
            Ok(patches) => Err(TestError::GoldieMismatch(patches)),
            Err(e) => Err(e.into()),
        }
    }

    fn is_counted(&self) -> bool {
        true
    }
}

// Util function to execute sh commands
async fn exec_sh(
    cmd_vec: Vec<String>,
    project_dir: PathBuf,
    stdout_file: std::fs::File,
    stderr_file: std::fs::File,
) -> FsResult<()> {
    let status = Command::new(&cmd_vec[0])
        .args(&cmd_vec[1..])
        .stdout(
            stdout_file
                .try_clone()
                .expect("Could not clone stdout_file"),
        )
        .stderr(
            stderr_file
                .try_clone()
                .expect("Could not clone stderr_file"),
        )
        .current_dir(project_dir)
        .spawn();

    match status {
        Ok(mut child) => {
            child.wait().expect("Could not wait on process");
            Ok(())
        }
        Err(e) => {
            writeln!(&stderr_file, "Error spawning command: {cmd_vec:?} {e}")
                .expect("Could not write");
            Err(FsError::exit_with_status(1))
        }
    }
}
