use std::cell::Cell;
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::Arc;

use clap::error::ErrorKind;

use dbt_clap_core::Cli;
use dbt_clap_core::CliParser;
use dbt_clap_core::commands::CoreCommand;
use dbt_common::FsResult;
use dbt_common::cancellation::CancellationReport;
use dbt_common::io_args::SystemArgs;
use dbt_common::{
    constants::{ERROR, PANIC},
    pretty_string::{GREEN, RED},
};
use dbt_error::FsError;
use dbt_features::feature_stack::FeatureStack;

use crate::ctrl_c::run_future_with_ctrlc_support;

use crate::dbt_lib::execute_fs_and_shutdown;
use crate::vars::{apply_color_env_overrides, apply_engine_env_var_aliases};

const FS_DEFAULT_STACK_SIZE: usize = 8 * 1024 * 1024;

/// Maximum number of threads used for running blocking operations (based on the tokio runtime
/// default).
///
/// These threads are used mostly for blocking I/O operations, so they don't really
/// consume CPU resources. That's why we can afford and should have a lot of them.
const FS_DEFAULT_MAX_BLOCKING_THREADS: usize = 512;

thread_local! {
    static DBT_RT_GUARD: Cell<Option<dbt_runtime::SetCurrentGuard>> =
        const { Cell::new(None) };
}

fn on_tokio_thread_start(handle: &dbt_runtime::Handle) {
    // SAFETY: this is called only from the tokio thread start callback.
    // Cleanup is performed on the tokio thread stop callback.
    let guard = unsafe { handle.enter_owned() };
    DBT_RT_GUARD.set(Some(guard));
}

fn on_tokio_thread_stop() {
    DBT_RT_GUARD.set(None);
}

/// Load environment variables from .env file in the current working directory.
///
/// Does not override existing env vars.
///
/// SAFETY: This modifies the process environment. Must be called before spawning
/// any threads and before CLI parsing.
fn maybe_load_dotenv() {
    if let Ok(cwd) = std::env::current_dir() {
        let env_path = cwd.join(".env");
        if env_path.is_file() {
            // from_path does NOT override existing env vars, which is exactly what we want:
            // shell env vars take precedence over .env file values
            let _ = dotenvy::from_path(&env_path);
        }
    }
}

pub fn init_env_before_parse() {
    // Find project root and load .env BEFORE CLI parsing so that environment
    // variables from .env are available for clap's `env = "VAR"` attributes.
    maybe_load_dotenv();

    // Apply DBT_ENGINE_* -> DBT_* aliases before CLI parsing.
    // This allows users to use DBT_ENGINE_FAIL_FAST instead of DBT_FAIL_FAST.
    apply_engine_env_var_aliases();

    // Translate FORCE_COLOR before CLI parsing, so clap's own usage/error text
    // is styled consistently with the rest of the output.
    apply_color_env_overrides();
}

fn parse_cli_or_exit(cli_parser: &CliParser) -> Box<Cli> {
    let args = std::env::args_os().collect::<Vec<_>>();
    cli_parser.print_json_version_and_exit_for_args(&args);

    match cli_parser.try_parse_from(args) {
        Ok(cli) => {
            // Continue as normal
            cli
        }
        Err(e) => {
            if e.kind() == ErrorKind::UnknownArgument {
                // todo make this for more than just unknown arguments
                // Only show the actual error message
                let msg = e.to_string(); // includes both "error:" and possibly "tip:"
                print_trimmed_error(msg); // prints to stderr
                std::process::exit(1);
            } else {
                // For other errors, show full help as usual
                e.exit();
            }
        }
    }
}

pub fn prepare_cli_or_exit(cli_parser: &CliParser) -> Box<Cli> {
    init_env_before_parse();
    let cli = parse_cli_or_exit(cli_parser);

    // Handle completions before any project/runtime setup.
    if let dbt_clap_core::commands::Command::Core(CoreCommand::Completions(args)) = &cli.command {
        cli_parser.write_completions(args.shell, &mut io::stdout());
        std::process::exit(0);
    }

    cli
}

pub fn run_cli(cli: Box<Cli>, arg: SystemArgs, feature_stack: Arc<FeatureStack>) -> ExitCode {
    ExitCode::from(run_cli_with_code(cli, arg, feature_stack))
}

/// Same as [`run_cli`] but yields the raw exit code instead of an [`ExitCode`],
/// so embedders (e.g. the `dbt-core` Python extension's console entrypoint) can
/// pass it to `std::process::exit`.
pub fn run_cli_with_code(cli: Box<Cli>, arg: SystemArgs, feature_stack: Arc<FeatureStack>) -> u8 {
    let event_emitter = feature_stack.instrumentation.event_emitter.as_ref();
    let fail_fast_flag = cli.common_args.fail_fast;

    if arg.io.send_anonymous_usage_stats {
        event_emitter.invocation_start_event(
            &arg.io.invocation_id,
            "",   // root project name is not known at this point
            None, // profile path is not known at this point
            cli.command.to_string(),
        );
    }

    // Apply the global ANTLR parser configuration
    feature_stack
        .antlr_parser
        .config
        .apply_configuration(&cli.common_args());

    // See also: `set_runtime_max_parallelism`.
    let dbt_rt = dbt_runtime::builder::Builder::new()
        .max_blocking_threads(if arg.no_parallel {
            1
        } else {
            dbt_runtime::builder::DEFAULT_MAX_BLOCKING_THREADS
        })
        .thread_stack_size(FS_DEFAULT_STACK_SIZE)
        .build();

    // Setup tokio runtime and set stack-size to 8MB
    // DO NOT USE Rayon, it is not compatible with Tokio

    // Only `--no-parallel` pins the tokio runtime to a single worker. The
    // blocking pool above starts with a default cap because `threads` is not
    // resolved yet; `set_runtime_max_parallelism` lowers it to `threads` once
    // the profile is loaded.
    let tokio_rt = if arg.no_parallel {
        let dbt_rt_handle = dbt_rt.handle().clone();
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_stack_size(FS_DEFAULT_STACK_SIZE)
            .worker_threads(1)
            .max_blocking_threads(1)
            .on_thread_start(move || on_tokio_thread_start(&dbt_rt_handle))
            .on_thread_stop(on_tokio_thread_stop)
            .build()
            .expect("failed to initialize 'single-worker' tokio runtime")
    } else {
        let dbt_rt_handle = dbt_rt.handle().clone();
        // Multi-threaded runtime: use default (max parallelism)
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .max_blocking_threads(FS_DEFAULT_MAX_BLOCKING_THREADS)
            .thread_stack_size(FS_DEFAULT_STACK_SIZE)
            .on_thread_start(move || on_tokio_thread_start(&dbt_rt_handle))
            .on_thread_stop(on_tokio_thread_stop)
            .build()
            .expect("failed to initialize default multi-threaded tokio runtime")
    };

    // If execution panics, exit with a status 2 (but not if RUST_BACKTRACE is
    // set to 1, in which case we want to see the backtrace):
    if arg.exit_process_on_panic && std::env::var("RUST_BACKTRACE").unwrap_or_default() != "1" {
        std::panic::set_hook(Box::new(|info| {
            eprintln!("{} {}", RED.apply_to(format!("{PANIC}:")), info);
            let _ = io::stdout().flush();
            let _ = io::stderr().flush();

            std::process::exit(2);
        }));
    }

    let cst = feature_stack.cli.cancellation_token_source.clone();
    let fail_fast = feature_stack.cli.fail_fast.clone();
    let token = cst.token();

    let future = tokio_rt.spawn(execute_fs_and_shutdown(
        arg,
        cli,
        Arc::clone(&feature_stack),
        token,
    ));
    let future = Box::pin(async {
        // JoinError is produced if future panics, so we .unwrap()
        future.await.unwrap()
    });

    let result: FsResult<Option<CancellationReport>> = tokio_rt.block_on(
        run_future_with_ctrlc_support(cst, future, fail_fast, fail_fast_flag),
    );

    // Shut down telemetry file writers (fast: ~300µs, must happen before exit).
    if let Err(errors) = feature_stack.tracing.shutdown_once() {
        for err in errors {
            let err = FsError::from(err);
            eprintln!("{}", err.pretty());
        }
    }

    // Remove the panic hook
    let _ = std::panic::take_hook();

    // Handle regular execution
    match result {
        Ok(cancellation_report) => {
            if let Some(_report) = cancellation_report {
                // TODO(felipecrv): move the Vortex shutdown logging here so we can
                // also log the cancellation report.

                // TODO(felipecrv): Use the UNIX convention of returning: 128 + signal.
                // SIGINIT is 2, so we should return 130.
                let exit_code = 1;

                // When we get a Ctrl+C (or process cancellation request in some other way),
                // the process may behave weirdly (e.g. some threads deadlocked), so we
                // must perform a "hard" exit that doesn't wait for threads to clean up.
                // This entails two thing:
                //
                // 1. we must not allow the tokio runtime destructor to run.
                //    (technically, this is implied by the 2nd step, but
                //    nonetheless we make it explicit here in case this function
                //    gets refactored in the future)
                std::mem::forget(dbt_rt);
                std::mem::forget(tokio_rt);
                // 2. Call a platform-specific function to hard terminate the
                //    current process
                hard_exit(exit_code);
            }

            // Otherwise, allow graceful shutdown and normal exit:
            0
        }
        Err(err) => {
            // If any step failed, assume error is already printed, just exit
            // with a corresponding exit code:
            err.exit_status().unwrap_or(1) as u8
        }
    }
}

pub fn print_trimmed_error(msg: String) {
    let mut stderr = io::stderr();

    let mut lines = msg.lines();
    let mut command = String::new();

    for line in lines.by_ref() {
        if let Some(rest) = line.strip_prefix("error:") {
            let _ = write!(stderr, "{}:", RED.apply_to(ERROR));
            let _ = writeln!(stderr, "{rest}");
        } else if let Some(rest) = line.trim_start().strip_prefix("tip:") {
            let prefix = if line.starts_with("tip:") { "" } else { "  " };
            let _ = write!(stderr, "{}{}", prefix, GREEN.apply_to("tip"));
            let _ = writeln!(stderr, ":{rest}");
        } else if line.trim().starts_with("Usage:") {
            //let command = drop "Usage:"; take everything until the first '<'; trim
            command = line.strip_prefix("Usage:").unwrap_or(line).to_string();
            command = command
                .split_once('<')
                .unwrap_or(("", ""))
                .0
                .trim()
                .to_string();
            break; // stop before dumping giant usage block
        } else {
            let _ = writeln!(stderr, "{line}");
        }
    }

    // Always print this footer
    let _ = writeln!(stderr, "\nFor more information, try '{command} --help'.");
}

#[cfg(target_os = "windows")]
/// Terminates the current process with the given exit code, without waiting for
/// threads to clean up
fn hard_exit(exit_code: i32) -> ! {
    use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};

    unsafe {
        let _ = TerminateProcess(GetCurrentProcess(), exit_code as u32);

        // Since we've terminated the current process, control should never
        // reach this point. This is just to satisfy the `-> !`:
        std::process::exit(exit_code)
    }
}

#[cfg(not(target_os = "windows"))]
/// Terminates the current process with the given exit code, without waiting for
/// threads to clean up
fn hard_exit(exit_code: i32) -> ! {
    std::process::exit(exit_code)
}
