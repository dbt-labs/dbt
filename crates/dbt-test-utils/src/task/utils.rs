use super::TestResult;
use super::log_capture::JsonLogEvent;
use crate::task::env::TracingReloadHandle;
use crate::task::task_seq::FeatureStackFactory;
use dbt_clap_core::{Cli, CliParser};
use dbt_common::FsError;
use dbt_common::cancellation::CancellationToken;
use dbt_common::constants::DBT_BRAND_NAME;
use dbt_common::tracing::FsTraceConfigBuilder;
use dbt_main::ctrl_c::run_future_with_ctrlc_support;
use futures::FutureExt as _;
use std::fmt::Debug;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::{
    fs::File,
    future::Future,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use once_cell::sync::Lazy;
use regex::Regex;

use dbt_common::{
    FsResult,
    io_args::SystemArgs,
    stdfs::{self},
    tokiofs, unexpected_err,
};
use dbt_features::feature_stack::FeatureStack;

// Pre-compiled regex patterns for optimal performance
static SCHEMA_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)fusion_tests_schema__[a-zA-Z0-9_]*").unwrap());
/// Matches the ___<timestamp>___ suffix from random_schema() used for test isolation
static SCHEMA_TIMESTAMP_SUFFIX_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"___\d+___").unwrap());
static ISO_TIMESTAMP_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z").unwrap());
static TIME_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b\d{2}:\d{2}:\d{2}\b").unwrap());
static NODE_INDEX_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[(\d+) of (\d+)").unwrap());
static BRACKETED_DURATION_PATTERN: Lazy<Regex> = Lazy::new(|| {
    // Matches bracketed durations in fixed 7-char width format with optional spacing:
    // "[  1.5s ]", "[ 2m42s]", "[1h 2m3s]", "[ 500ms ]", "[  100us]", "[1000ns ]", "[-------]"
    Regex::new(r"\[\s*(?:\d+h(?:\s*\d+m(?:\s*\d+s)?)?|\d+m(?:\s*\d+s)?|\d+(?:\.\d+)?(?:ns|us|μs|ms|s)|-------)\s*\]")
        .unwrap()
});
static IN_DURATION_PATTERN: Lazy<Regex> = Lazy::new(|| {
    // Matches: "in 1s", "in 500ms", "in 1s 298ms", "in 2m 10s", etc.
    Regex::new(
        r"\bin\s+\d+(?:\.\d+)?(?:ns|us|μs|ms|s|m|h)(?:\s+\d+(?:\.\d+)?(?:ns|us|μs|ms|s|m|h))*\b",
    )
    .unwrap()
});
static LAST_UPDATED_DURATION_PATTERN: Lazy<Regex> = Lazy::new(|| {
    // Matches: "Last updated 1s ago", "Last updated 500ms ago", "Last updated 1s 298ms ago", "Last updated 2m 10s ago", etc.
    Regex::new(r"\bLast updated\s+\d+(?:\.\d+)?(?:ns|us|μs|ms|s|m|h)(?:\s+\d+(?:\.\d+)?(?:ns|us|μs|ms|s|m|h))* ago\b")
        .unwrap()
});
static MULTI_UNIT_DURATION_PATTERN: Lazy<Regex> = Lazy::new(|| {
    // Matches sequences of 1+ duration tokens (e.g., "939ns", "32ms 101us", "4s 703ms 195us 939ns")
    Regex::new(r"\b\d+(?:\.\d+)?(?:ns|us|μs|ms|s|m|h)(?:\s+\d+(?:\.\d+)?(?:ns|us|μs|ms|s|m|h))*\b")
        .unwrap()
});
static AGE_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bage:\s*\d+").unwrap());
static INLINE_SQL_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"inline_[a-f0-9]{8}\.sql").unwrap());
#[cfg(not(windows))]
static TEMP_ROOT_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:/private)?/var/folders/[^/]+/[^/]+/T/").unwrap());
#[cfg(any(windows, test))]
static WINDOWS_TEMP_ROOT_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)[A-Z]:\\+Users\\+[^\\]+\\+AppData\\+Local\\+Temp\\+|[A-Z]:\\+Windows\\+Temp\\+|[A-Z]:\\+Temp\\+",
    )
    .unwrap()
});
static MKTEMP_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"/\.tmp[0-9A-Za-z_-]+").unwrap());
// Matches OS thread IDs in Rust panic messages: "thread 'name' (12345678) panicked"
static THREAD_ID_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(thread '[^']+') \(\d+\) (panicked)").unwrap());
// Matches absolute replay recording paths in error messages: "(path: /abs/path)"
static REPLAY_PATH_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\(path: [^)]+\)").unwrap());
// Matches the platform-specific text `std::io::Error`'s `Display` embeds
static OS_ERROR_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[A-Za-z][A-Za-z0-9 .,'/-]*\(os error \d+\)").unwrap());

/// Copies a directory and its contents, excluding .gitignored files.
pub fn copy_dir_non_ignored(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> FsResult<()> {
    stdfs::create_dir_all(&dst)?;
    for entry in ignore::WalkBuilder::new(src.as_ref())
        .hidden(false)
        .follow_links(true)
        .git_global(false)
        .ignore(false)
        .build()
    {
        let Ok(entry) = entry else {
            return unexpected_err!(
                "Failed to read entry in directory: {}",
                src.as_ref().display()
            );
        };

        let relative_path = entry
            .path()
            .strip_prefix(src.as_ref())
            .expect("entry path should be relative to source");
        let target_path = dst.as_ref().join(relative_path);

        let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
        if is_dir {
            stdfs::create_dir_all(&target_path)?;
        } else {
            stdfs::copy(entry.path(), &target_path)?;
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn redirect_buffer_to_stdin(buffer_content: &str) -> TestResult<File> {
    use std::io::{Seek as _, Write};
    use std::os::fd::AsRawFd as _;

    // Create a buffer with the desired content.
    let mut temp_file = tempfile::tempfile()?;
    temp_file.write_all(buffer_content.as_bytes())?;
    temp_file.seek(std::io::SeekFrom::Start(0))?;

    unsafe {
        // Close the original stdin.
        libc::close(0);

        // Duplicate the buffer's file descriptor to stdin (0).
        libc::dup2(temp_file.as_raw_fd(), 0);
    }

    Ok(temp_file)
}

#[cfg(target_os = "windows")]
pub fn redirect_buffer_to_stdin(buffer_content: &str) -> TestResult<File> {
    use std::io::{Seek, Write};
    use std::os::windows::io::AsRawHandle;

    use winapi::um::processenv::SetStdHandle;
    use winapi::um::winbase::STD_INPUT_HANDLE;

    // Create a temporary file and write the buffer content to it
    let mut temp_file = tempfile::tempfile()?;
    temp_file.write_all(buffer_content.as_bytes())?;
    temp_file.seek(std::io::SeekFrom::Start(0))?;

    // Get the raw handle of the temporary file
    let raw_handle = temp_file.as_raw_handle();

    // Redirect the standard input to the temporary file
    let success = unsafe { SetStdHandle(STD_INPUT_HANDLE, raw_handle as _) };
    if success == 0 {
        return Err("Failed to redirect stdin".into());
    }

    // Return the temporary file to keep it open for the duration of the redirection
    Ok(temp_file)
}

/// Iterates over file paths in the directory and subdirectories,
/// invoking a handler for each file.
pub async fn iter_files_recursively<'a, F>(root: &'a Path, handler: &'a F) -> TestResult<()>
where
    F: Fn(&Path) -> TestResult<()> + Send,
{
    let mut read_dir = tokiofs::read_dir(root).await?;
    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        if path.is_dir() {
            // Recursively handle files in subdirectories.
            Box::pin(iter_files_recursively(&path, handler)).await?;
        } else {
            // Invoke the handler for each file.
            handler(&path)?;
        }
    }

    Ok(())
}

pub fn check_set_user_env_var() {
    if std::env::var("USER").is_err() {
        // set_var is generally disallowed but intentional here
        let run_id = std::env::var("GITHUB_RUN_ID").unwrap_or_else(|_| "0000000".to_string());
        let run_number = std::env::var("GITHUB_RUN_NUMBER").unwrap_or_else(|_| "0".to_string());
        unsafe {
            #[allow(clippy::disallowed_methods)]
            std::env::set_var("USER", format!("{run_id}x{run_number}"));
        }
    }
}

/// Normalize OS temp roots and collapse mktemp-style segments like /.tmpabcdef -> /.tmpXXXXXX.
pub fn maybe_normalize_tmp_paths(output: String) -> String {
    #[cfg(windows)]
    {
        normalize_windows_tmp_paths(output)
    }
    #[cfg(not(windows))]
    {
        let normalized = TEMP_ROOT_PATTERN.replace_all(&output, "/tmp/");
        MKTEMP_PATTERN
            .replace_all(&normalized, "/.tmpXXXXXX")
            .to_string()
    }
}

#[cfg(any(windows, test))]
pub(super) fn normalize_windows_tmp_paths(output: String) -> String {
    let normalized = WINDOWS_TEMP_ROOT_PATTERN.replace_all(&output, "/tmp/");
    MKTEMP_PATTERN
        .replace_all(&normalized, "/.tmpXXXXXX")
        .to_string()
}

/// On Windows, this normalizes forward/backward slashes to '|' so as to ignore
/// the difference in path separators.
///
/// On other platforms, this is a no-op.
pub fn maybe_normalize_slashes(output: String) -> String {
    #[cfg(windows)]
    {
        output.replace("\\", "|").replace("/", "|")
    }
    #[cfg(not(windows))]
    {
        output
    }
}

pub fn maybe_normalize_schema_name(output: String) -> String {
    // Use pre-compiled regex to replace schema patterns like "fusion_tests_schema__alex"
    // with "fusion_tests_schema__replaced" without breaking duration patterns like "44.65s"
    let replaced = SCHEMA_PATTERN
        .replace_all(&output, "fusion_tests_schema__replaced")
        .to_string();
    // Strip ___<timestamp>___ suffix from random_schema() used for test isolation
    SCHEMA_TIMESTAMP_SUFFIX_PATTERN
        .replace_all(&replaced, "")
        .to_string()
}

pub fn maybe_normalize_time(output: String) -> String {
    let mut result = output;

    // Replace ISO 8601 timestamps like "2025-05-27T22:38:47.667Z" and "2017-09-01T00:00:00Z"
    result = ISO_TIMESTAMP_PATTERN
        .replace_all(&result, "YYYY-MM-DDTHH:MM:SS.sssZ")
        .to_string();

    // Replace time formats like "15:39:21"
    result = TIME_PATTERN.replace_all(&result, "HH:MM:SS").to_string();

    // Replace trailing "in ..." duration phrases with a stable token
    result = IN_DURATION_PATTERN
        .replace_all(&result, "in duration")
        .to_string();

    // Replace trailing "Last updated ... ago" duration phrases with a stable token
    result = LAST_UPDATED_DURATION_PATTERN
        .replace_all(&result, "Last updated Xs ago")
        .to_string();

    // Replace bracketed duration formats like "[ 44.65s]" and "[-------]" with "[duration]"
    // This works to unify all durations, as the following pattern captures more duration formats
    // outside of brackets, and replaces them with `duration`.
    result = BRACKETED_DURATION_PATTERN
        .replace_all(&result, "[duration]")
        .to_string();
    // Replace multi-unit duration sequences like "32ms 101us 694ns" with a stable token
    result = MULTI_UNIT_DURATION_PATTERN
        .replace_all(&result, "duration")
        .to_string();

    // Replace age patterns like "age: 244165330" with normalized value
    result = AGE_PATTERN
        .replace_all(&result, "age: NORMALIZED")
        .to_string();

    result
}

/// Strip the version number out of the startup version banner, whose brand name
/// varies per binary (`dbt`, `dbt-oss`, `dbt-repl`).
pub fn normalize_version(output: String) -> String {
    const BRANDS: [&str; 3] = ["dbt", "dbt-oss", "dbt-repl"];

    BRANDS.iter().fold(output, |acc, brand| {
        acc.replace(
            format!("{brand} {}", env!("CARGO_PKG_VERSION")).as_str(),
            format!("{brand} ").as_str(),
        )
    })
}

pub fn normalize_inline_sql_files(output: String) -> String {
    // Replace inline SQL file names like "inline_a1b2c3d4.sql" with "inline_#randhash#.sql"
    INLINE_SQL_PATTERN
        .replace_all(&output, "inline_#randhash#.sql")
        .to_string()
}

/// Strips non-deterministic OS thread IDs from Rust panic lines.
///
/// Replaces `thread 'tokio-rt-worker' (12345678) panicked` with
/// `thread 'tokio-rt-worker' panicked` so golden files don't need to
/// be updated on every run.
pub fn normalize_thread_ids(output: String) -> String {
    THREAD_ID_PATTERN.replace_all(&output, "$1 $2").to_string()
}

/// Replaces the position in a `[N of M ...]` execution index with `index`.
///
/// Only applied to tests that sort their output. Sorting is how a test declares that the
/// order its nodes run in is not fixed, and the position is assigned in that same order --
/// so `N` genuinely differs between runs and cannot be pinned in a golden file. `M` is left
/// alone: the total is stable, and it is what catches a node entering or leaving the
/// selection. Tests that run sequentially keep their positions asserted in full.
pub fn normalize_node_index(output: String) -> String {
    NODE_INDEX_PATTERN
        .replace_all(&output, "[index of $2")
        .to_string()
}

/// Strips absolute replay recording paths from error messages.
///
/// Replaces `(path: /abs/path/to/recordings)` with `(path: <path>)` so golden
/// files aren't tied to a specific worktree or username.
pub fn normalize_replay_paths(output: String) -> String {
    REPLAY_PATH_PATTERN
        .replace_all(&output, "(path: <path>)")
        .to_string()
}

/// Strips the platform-specific message text (and error code) for errors
/// that wrap a raw IO failure.
pub fn normalize_os_error_messages(output: String) -> String {
    OS_ERROR_PATTERN
        .replace_all(&output, "<os error>")
        .to_string()
}

/// Strips the full test name of the crate name and returns the test name.
pub fn strip_full_test_name(full_test_name: &str) -> String {
    full_test_name
        .split("::")
        .last()
        .expect("Fully qualified test name should contain `::`")
        .to_string()
}

/// Strips the leading relative path from a path.
pub fn strip_leading_relative(path: &Path) -> &Path {
    let mut components = path.components();

    // Skip over any leading CurDir (`./`) or ParentDir (`../`)
    while let Some(c) = components.clone().next() {
        match c {
            Component::CurDir | Component::ParentDir => {
                components.next(); // discard
            }
            _ => break,
        }
    }

    components.as_path()
}

// Util function to execute fusion commands in tests
#[allow(clippy::too_many_arguments)]
pub fn exec_fs<'a, Fut>(
    feature_stack_factory: Arc<FeatureStackFactory>,
    parser: &CliParser,
    cmd_vec: Vec<String>,
    project_dir: PathBuf,
    target_dir: PathBuf,
    stdout_file: File,
    stderr_file: File,
    execute_fs: impl FnOnce(SystemArgs, Box<Cli>, Arc<FeatureStack>, CancellationToken) -> Fut,
    from_lib: impl FnOnce(&Cli) -> SystemArgs,
    tracing_handle: TracingReloadHandle,
) -> Pin<Box<dyn Future<Output = FsResult<()>> + Send + 'a>>
where
    Fut: Future<Output = FsResult<()>> + Send + 'a,
{
    // Check if project_dir has a .env.conformance file
    // NOTE: this has to be done before we parse Cli
    let conformance_file = project_dir.join(".env.conformance");
    if conformance_file.exists() {
        // if so, load it
        dotenvy::from_path(conformance_file).unwrap();
    }

    let cli = parser.parse_from(cmd_vec);
    exec_cli(
        feature_stack_factory,
        cli,
        project_dir,
        target_dir,
        stdout_file,
        stderr_file,
        execute_fs,
        from_lib,
        tracing_handle,
    )
}

// Util function to execute fusion commands in tests from an already-parsed
// `Cli`. This is everything `exec_fs` does *after* argv parsing; `exec_fs`
// delegates to it. It exists so callers whose argv does not parse into the
// platform `Cli` directly can still reuse
// the redirect / ctrl-c / telemetry-shutdown plumbing.
//
// Unlike `exec_fs`, this does NOT load `.env.conformance`: that has to happen
// before argv parsing, which by definition is already done by the time a `Cli`
// exists. Callers that need it must load it before parsing.
#[allow(clippy::too_many_arguments)]
pub fn exec_cli<'a, Fut>(
    feature_stack_factory: Arc<FeatureStackFactory>,
    cli: Box<Cli>,
    project_dir: PathBuf,
    target_dir: PathBuf,
    stdout_file: File,
    stderr_file: File,
    execute_fs: impl FnOnce(SystemArgs, Box<Cli>, Arc<FeatureStack>, CancellationToken) -> Fut,
    from_lib: impl FnOnce(&Cli) -> SystemArgs,
    tracing_handle: TracingReloadHandle,
) -> Pin<Box<dyn Future<Output = FsResult<()>> + Send + 'a>>
where
    Fut: Future<Output = FsResult<()>> + Send + 'a,
{
    let arg = from_lib(&cli);
    // Equivalent to `CliParser::warn_error_options(&cli)`, computed directly so
    // this helper does not need a `CliParser`.
    let warn_error_options = Some(cli.common_args.get_cli_warn_error_options());
    let fail_fast_flag = cli.common_args.fail_fast;
    let trace_config = FsTraceConfigBuilder::from_io_args("dbt-tests", DBT_BRAND_NAME, &arg.io)
        .with_command(arg.command)
        .with_project_dir(Some(&project_dir))
        .with_target_path(Some(&target_dir))
        .with_query_log_enabled(true) // Always enable query log for now
        .with_warn_error_options(warn_error_options.as_ref().cloned().unwrap_or_default())
        .with_skip_fusion_only_upgrades(cli.common_args.skip_fusion_only_upgrades())
        .with_quiet(cli.common_args.get_quiet())
        .build();
    let tracing_config_provider = trace_config.create_config_provider();
    let (middlewares, consumer_layers, mut shutdown_items) =
        match trace_config.build_layers(Arc::clone(&tracing_config_provider)) {
            Ok(layers) => layers.into_parts(),
            Err(err) => {
                return Box::pin(async move { Err(err) });
            }
        };

    tracing_handle.with_tracing_consumer(middlewares, consumer_layers);

    let feature_stack = feature_stack_factory(tracing_config_provider);
    let cst = feature_stack.cli.cancellation_token_source.clone();
    let fail_fast = feature_stack.cli.fail_fast.clone();
    let token = cst.token();

    let future = Box::pin(execute_fs(arg, cli, feature_stack, token));
    Box::pin(async move {
        // Redirect stdout and stderr for the duration of the future.
        let stdout_guard = with_redirected_stdout(stdout_file);
        let stderr_guard = with_redirected_stderr(stderr_file);

        let result = AssertUnwindSafe(run_future_with_ctrlc_support(
            cst,
            future,
            fail_fast,
            fail_fast_flag,
        ))
        .catch_unwind()
        .await;
        let result = match result {
            Ok(result) => result,
            Err(payload) => {
                // The panic hook runs *before* the unwind drops these guards,
                // so its report went into the redirected file, and on an
                // unwind nobody reads that file back -- the test reported a
                // bare `FAILED` with no reason at all. Restore the real
                // descriptors and re-state the message, which is the half of
                // the report worth reading.
                drop(stderr_guard);
                drop(stdout_guard);
                report_panic_message(payload.as_ref());
                std::panic::resume_unwind(payload);
            }
        };

        let shutdown_errors: Vec<FsError> = shutdown_items
            .iter_mut()
            .filter_map(|item| item.shutdown().err().map(FsError::from))
            .collect();

        match result {
            Ok(_) if shutdown_errors.is_empty() => Ok(()),
            Err(ref err)
                if err.exit_status() == Some(0) // early-exit, but successful
            && shutdown_errors.is_empty() =>
            {
                Ok(())
            }
            // If the run itself failed - return it's error and ignore shutdown
            Err(err) => Err(err),
            _ => unexpected_err!("Failed to shutdown telemetry"),
        }
    })
}

/// Re-states a panicking command's message on the real stderr, so the test
/// failure carries a reason.
///
/// The captured stderr file cannot be read back for this: it is opened
/// write-only, and its path does not reach here. The payload does, and for a
/// `panic!("{..}")` it is the whole message. Anything the hook wrote --
/// notably a backtrace -- stays in the command's captured stderr.
///
/// Call only after the redirection guards are dropped; before that this would
/// just append to the same file. And call it as `report_panic_message(
/// payload.as_ref())`: `&payload` coerces the *box* into the trait object, so
/// every downcast below then fails and the message is lost again.
fn report_panic_message(payload: &(dyn std::any::Any + Send)) {
    use std::io::Write as _;

    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned());
    let mut stderr = std::io::stderr();
    match message {
        Some(message) => {
            let _ = writeln!(
                stderr,
                "\n---- the command panicked while its output was redirected ----\n\
                 {message}\n\
                 ---- end panic message; a backtrace, if any, is in the command's \
                 captured stderr ----"
            );
        }
        None => {
            let _ = writeln!(
                stderr,
                "\n---- the command panicked with a non-string payload ----"
            );
        }
    }
}

/// The purpose of this guard is two fold:
/// 1. it holds the file handle open for the duration of the redirection
/// 2. it restores the original stdout/stderr file descriptors when dropped
///
/// Restoring the original file descriptors is necessary to allow printing to
/// terminal code (e.g. `dbg!/println!`) to still function in test cases -- if
/// we don't restore here, then terminal output will be disabled after the first
/// time `exec_fs` gets called, which would be surprising for the test author.
struct FdRedirectionGuard {
    _file: File,
    #[cfg(not(target_os = "windows"))]
    target_fd: std::os::unix::io::RawFd,
    #[cfg(not(target_os = "windows"))]
    original_fd: std::os::unix::io::RawFd,
    #[cfg(target_os = "windows")]
    target_fd: usize, // Windows uses HANDLE, but we store it as usize for simplicity
    #[cfg(target_os = "windows")]
    original_fd: usize, // Windows uses HANDLE, but we store it as usize for simplicity
}

impl Drop for FdRedirectionGuard {
    fn drop(&mut self) {
        #[cfg(not(target_os = "windows"))]
        unsafe {
            // Restore the original stdout
            libc::dup2(self.original_fd, self.target_fd);
        }
        #[cfg(target_os = "windows")]
        unsafe {
            use winapi::um::processenv::SetStdHandle;

            // Restore the original stdout
            SetStdHandle(self.target_fd as _, self.original_fd as _);
        }
    }
}

#[cfg(target_os = "windows")]
/// Redirects stdout to `file`. Returns a scope guard that restores the original
/// stdout on drop.
fn with_redirected_stdout(file: File) -> FdRedirectionGuard {
    use std::os::windows::io::AsRawHandle as _;
    use winapi::um::processenv::SetStdHandle;
    use winapi::um::winbase::STD_OUTPUT_HANDLE;

    let original_fd = unsafe { winapi::um::processenv::GetStdHandle(STD_OUTPUT_HANDLE) as usize };

    let raw_handle = file.as_raw_handle();

    let success = unsafe { SetStdHandle(STD_OUTPUT_HANDLE, raw_handle as _) };
    if success == 0 {
        panic!("Failed to redirect stdout");
    }

    FdRedirectionGuard {
        _file: file,
        target_fd: STD_OUTPUT_HANDLE as usize,
        original_fd,
    }
}

#[cfg(target_os = "windows")]
/// Redirects stderr to `file`. Returns a scope guard that restores the original
/// stderr on drop.
fn with_redirected_stderr(file: File) -> FdRedirectionGuard {
    use std::os::windows::io::AsRawHandle as _;
    use winapi::um::processenv::SetStdHandle;
    use winapi::um::winbase::STD_ERROR_HANDLE;

    let original_fd = unsafe { winapi::um::processenv::GetStdHandle(STD_ERROR_HANDLE) as usize };

    let raw_handle = file.as_raw_handle();

    let success = unsafe { SetStdHandle(STD_ERROR_HANDLE, raw_handle as _) };
    if success == 0 {
        panic!("Failed to redirect stderr");
    }

    FdRedirectionGuard {
        _file: file,
        target_fd: STD_ERROR_HANDLE as usize,
        original_fd,
    }
}

#[cfg(not(target_os = "windows"))]
/// Redirects stdout to `file`. Returns a scope guard that restores the original
/// stdout on drop.
fn with_redirected_stdout(file: File) -> FdRedirectionGuard {
    use std::os::fd::AsRawFd as _;

    let original_fd = unsafe { libc::dup(libc::STDOUT_FILENO) };

    unsafe {
        // Redirect stdout to the file
        libc::dup2(file.as_raw_fd(), libc::STDOUT_FILENO);
    }

    FdRedirectionGuard {
        _file: file,
        target_fd: libc::STDOUT_FILENO,
        original_fd,
    }
}

#[cfg(not(target_os = "windows"))]
/// Redirects stderr to `file`. Returns a scope guard that restores the original
/// stderr on drop.
fn with_redirected_stderr(file: File) -> FdRedirectionGuard {
    use std::os::fd::AsRawFd as _;

    let original_fd = unsafe { libc::dup(libc::STDERR_FILENO) };

    unsafe {
        // Redirect stderr to the file
        libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
    }

    FdRedirectionGuard {
        _file: file,
        target_fd: libc::STDERR_FILENO,
        original_fd,
    }
}

/// The name of the git directory
pub const GIT_DIR: &str = ".git";

/// Given an absolute path, returns the relative path to the git root directory if it exists.
pub fn relative_to_git_root(path: &Path) -> Option<PathBuf> {
    let mut current = path;
    while let Some(parent) = current.parent() {
        if parent.join(GIT_DIR).is_dir() {
            return path.strip_prefix(parent).ok().map(|p| p.to_path_buf());
        }
        current = parent;
    }
    None
}

pub fn assert_str_in_log_messages(logs: &[JsonLogEvent], search_str: &str) -> FsResult<()> {
    if logs.iter().any(|log| {
        log.info()
            .map(|info| info.msg.as_str())
            .unwrap_or_default()
            .contains(search_str)
    }) {
        Ok(())
    } else {
        panic!("Log message containing '{search_str}' not found");
    }
}

/// Helper function to assert that two vectors are equal, ignoring order.
pub fn assert_vec_sorted_eq<T: PartialEq + Clone + Ord + Debug>(expected: Vec<T>, actual: Vec<T>) {
    let mut expected = expected;
    expected.sort();
    let mut actual = actual;
    actual.sort();
    assert_eq!(expected, actual);
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_maybe_normalize_schema_name() {
        let actual =
            maybe_normalize_schema_name("fusion_tests_schema__alex.dbt_something".to_string());
        assert_eq!(actual, "fusion_tests_schema__replaced.dbt_something");
    }
}
