//! The engine glue every dbt distribution's Python extension module shares.
//!
//! A distribution supplies its CLI surface and feature set via [`Distribution`]
//! and calls [`register`] from its `#[pymodule]`; everything else lives here.

use dbt_clap_core::commands::{Command, CoreCommand};
use dbt_clap_core::{Cli, CliParser, from_lib, from_main};
pub use dbt_common::FsResult;
use dbt_common::io_args::{FsCommand, SystemArgs};
use dbt_common::tracing::dbt_init::{
    InvocationTracingGuard, ProcessTracing, init_tracing_cli_reloadable,
};
use dbt_common::tracing::{FsTraceConfig, FsTraceConfigBuilder};
use dbt_features::feature_stack::FeatureStack;
use dbt_features::tracing::TracingFeature;
use dbt_main::{print_trimmed_error, run_cli_with_code};
use dbt_schemas::schemas::DbtCommandExecutionArtifacts;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use std::cell::Cell;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::level_filters::LevelFilter;

thread_local! {
    static DBT_RT_GUARD: Cell<Option<dbt_runtime::SetCurrentGuard>> =
        const { Cell::new(None) };
}

fn on_tokio_thread_start(handle: &dbt_runtime::Handle) {
    // SAFETY: cleanup is performed on the tokio thread stop callback.
    let guard = unsafe { handle.enter_owned() };
    DBT_RT_GUARD.set(Some(guard));
}

fn on_tokio_thread_stop() {
    DBT_RT_GUARD.set(None);
}

mod contracts;

// Re-exported so a distribution can write a [`CliTracingFactory`] without depending
// on the tracing crates directly.
pub use dbt_common::tracing::TracingConfigProvider;
pub use dbt_tracing::init::TelemetryHandle;

/// Builds this distribution's CLI surface. A factory rather than a built parser so
/// [`Distribution`] stays a plain `Sync` value.
pub type CliParserFactory = fn() -> CliParser;

/// Installs process tracing for the console-script entrypoint, and may adjust
/// [`SystemArgs`] for the command being run.
///
/// A distribution overrides this when one of its commands needs a different sink
/// stack than the CLI's — a language server speaking JSON-RPC over stdio cannot have
/// log lines on stdout. `None` uses the ordinary CLI tracing.
pub type CliTracingFactory = fn(
    &Cli,
    &CliParser,
    &mut SystemArgs,
) -> FsResult<(TelemetryHandle, Arc<dyn TracingConfigProvider>)>;

/// Builds this distribution's feature set from the invocation's tracing feature.
///
/// Takes the parsed `Cli` as well as the derived args: a distribution's feature set
/// can hinge on its own flags, which never reach [`SystemArgs`].
pub type FeatureStackFactory = fn(TracingFeature, &Cli, &SystemArgs) -> Arc<FeatureStack>;

/// All that separates one distribution's extension module from another.
pub struct Distribution {
    pub cli_parser: CliParserFactory,
    pub feature_stack: FeatureStackFactory,
    pub cli_tracing: Option<CliTracingFactory>,
}

/// Set once by [`register`]. Process-global because the classes below are
/// constructed by Python, which cannot hand them a distribution.
static DISTRIBUTION: OnceLock<Distribution> = OnceLock::new();

fn distribution() -> &'static Distribution {
    DISTRIBUTION
        .get()
        .expect("register() sets this at module import, before Python can call anything")
}

/// Adds this crate's classes and functions to a distribution's `_core` module.
pub fn register(m: &Bound<'_, PyModule>, distribution: Distribution) -> PyResult<()> {
    DISTRIBUTION
        .set(distribution)
        .map_err(|_| pyo3::exceptions::PyRuntimeError::new_err("`_core` initialized twice"))?;

    // No artifact classes; they are dataclasses under dbt/artifacts/schemas/.
    m.add_class::<DbtRunner>()?;
    m.add_class::<DbtRunnerResult>()?;
    m.add_function(wrap_pyfunction!(run_cli, m)?)?;
    Ok(())
}

/// Initialized once without consumer layers; each invocation installs its own.
static PROCESS_TRACING: OnceLock<ProcessTracing> = OnceLock::new();

/// `invoke` releases the GIL, so threads can race to initialize; the loser would fail as
/// already-initialized.
///
/// This lock is only necessary to gracefully handle tracing init errors and
/// pass them back as Python exceptions, rather than panics. Technically
/// not required, since these errors should never happend unless we made a
/// coding mistake, but the cost of one time init mutex is much lower than
/// Py overhead, so we can keep it.
static PROCESS_TRACING_INIT: Mutex<()> = Mutex::new(());

/// Installing consumer layers replaces the previous set globally, so overlapping invocations
/// would cross-write logs and the first to finish would silently unlog the rest.
static INVOCATION: Mutex<()> = Mutex::new(());

/// Initializes process-wide tracing on first call. The cap is fixed for the process, so only
/// the first caller's value takes effect and anything it excludes reaches no later layer.
fn process_tracing(max_log_verbosity: LevelFilter) -> PyResult<&'static ProcessTracing> {
    if let Some(tracing) = PROCESS_TRACING.get() {
        return Ok(tracing);
    }

    let _guard = PROCESS_TRACING_INIT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    if let Some(tracing) = PROCESS_TRACING.get() {
        return Ok(tracing);
    }

    let tracing = init_tracing_cli_reloadable("dbt", max_log_verbosity)
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    Ok(PROCESS_TRACING.get_or_init(|| tracing))
}

fn trace_config(cli: &Cli, cli_parser: &CliParser, arg: &SystemArgs) -> FsTraceConfig {
    let brand_name = cli_parser.command_name();
    FsTraceConfigBuilder::from_io_args("dbt", brand_name, &arg.io)
        .with_command(arg.command)
        .with_project_dir(cli.project_dir().as_ref())
        .with_target_path(cli.target_path().as_ref())
        .with_query_log_enabled(true) // Always enable query log for now
        .with_warn_error_options(cli.common_args().get_cli_warn_error_options())
        .with_skip_fusion_only_upgrades(cli.common_args().skip_fusion_only_upgrades())
        .with_quiet(cli.common_args().get_quiet())
        .build()
}

/// Installs this invocation's tracing layers and points `arg.io.log_path` at its log file.
///
/// The guard flushes and detaches the layers on `finish`, so it must outlive the engine call.
fn begin_invocation(
    config: FsTraceConfig,
    max_log_verbosity: LevelFilter,
    arg: &mut SystemArgs,
) -> PyResult<(InvocationTracingGuard, TracingFeature)> {
    let config_provider = config.create_config_provider();
    let guard = process_tracing(max_log_verbosity)?
        .begin_invocation(config, Arc::clone(&config_provider))
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    if let Some(log_path) = config_provider.get_file_log_path() {
        arg.io.log_path = Some(log_path);
    }

    // No shutdown handle: it would let the engine close the process span, ending tracing for
    // every later invocation. The guard owns teardown instead.
    Ok((
        guard,
        TracingFeature::default().with_config_provider(config_provider),
    ))
}

/// Transport only; `dbt/cli/main.py` decodes into the artifact dataclasses.
#[pyclass(get_all)]
struct DbtRunnerResult {
    success: bool,
    exit_code: u8,
    /// `manifest`, `list`, `sources` or `run_results`; `None` if nothing was captured.
    result_kind: Option<String>,
    result_msgpack: Option<Py<PyBytes>>,
    /// Kept off `result` so that stays dbt-core-compatible; `--write-catalog` is
    /// global, so any command can populate this.
    catalog_msgpack: Option<Py<PyBytes>>,
    /// Engine error message, else `None`. Handled failures with run results have
    /// no message.
    exception: Option<String>,
}

#[pymethods]
impl DbtRunnerResult {
    fn __repr__(&self) -> String {
        format!(
            "DbtRunnerResult(success={}, exit_code={})",
            self.success, self.exit_code
        )
    }
}

/// Tagged so Python knows which dataclass to decode into. Keyed on the command
/// rather than on capture order, so the mapping is deterministic.
fn build_result_msgpack(
    py: Python<'_>,
    command: FsCommand,
    exec: &mut DbtCommandExecutionArtifacts,
) -> PyResult<Option<(&'static str, Py<PyBytes>)>> {
    let tagged = match command {
        FsCommand::Parse => exec
            .manifest
            .take()
            .map(|m| contracts::to_msgpack(py, &m).map(|b| ("manifest", b))),
        FsCommand::List => exec
            .list_items
            .take()
            .map(|items| contracts::to_msgpack(py, &items).map(|b| ("list", b))),
        // `source freshness` writes sources.json instead of run_results.json, so
        // that artifact is what it reports.
        FsCommand::Source => exec
            .sources
            .take()
            .map(|s| contracts::to_msgpack(py, &s).map(|b| ("sources", b))),
        // Everything else reports run_results, as dbt-core does; the catalog is
        // surfaced separately.
        _ => exec
            .run_results
            .take()
            .map(|rr| contracts::to_msgpack(py, &rr).map(|b| ("run_results", b))),
    };
    tagged.transpose()
}

fn build_catalog_msgpack(
    py: Python<'_>,
    exec: &mut DbtCommandExecutionArtifacts,
) -> PyResult<Option<Py<PyBytes>>> {
    exec.catalog
        .take()
        .map(|c| contracts::to_msgpack(py, &c))
        .transpose()
}

/// Runs dbt in-process — no subprocess fork.
#[pyclass]
struct DbtRunner {
    /// Built once and reused across `invoke` calls.
    cli_parser: CliParser,
}

#[pymethods]
impl DbtRunner {
    #[new]
    fn new() -> Self {
        DbtRunner {
            cli_parser: (distribution().cli_parser)(),
        }
    }

    /// Run dbt with CLI args, e.g. `["run", "--select", "my_model"]`; drops the
    /// GIL for the duration.
    fn invoke(&self, py: Python<'_>, args: Vec<String>) -> PyResult<DbtRunnerResult> {
        let mut argv = vec!["dbt".to_string()];
        argv.extend(args);

        // invoke_inner is pure Rust; run it GIL-released, serialize after.
        let cli_parser = &self.cli_parser;
        let (exit_code, command, exec, exception) =
            py.detach(|| invoke_inner(argv, cli_parser, distribution().feature_stack))?;
        let (result, catalog_msgpack) = match exec {
            Some(mut exec) => (
                build_result_msgpack(py, command, &mut exec)?,
                build_catalog_msgpack(py, &mut exec)?,
            ),
            None => (None, None),
        };
        let (result_kind, result_msgpack) = match result {
            Some((kind, bytes)) => (Some(kind.to_string()), Some(bytes)),
            None => (None, None),
        };
        Ok(DbtRunnerResult {
            success: exit_code == 0,
            exit_code,
            result_kind,
            result_msgpack,
            catalog_msgpack,
            exception,
        })
    }
}

/// Message for `exception`. An error raised with real context renders itself;
/// `exit_with_status` carries none, so `pretty()` would yield a bare "exit code
/// N" while the diagnostics went only to the log.
fn describe_engine_error(captured: Option<&str>, e: &dbt_common::FsError) -> String {
    // The engine flattens a real error to a bare exit status for CLI callers,
    // stashing the rendered message on the artifacts first; prefer that.
    if let Some(message) = captured {
        return message.to_string();
    }
    match e.exit_status() {
        Some(code) => format!(
            "dbt reported errors and exited with status {code}; the diagnostics \
             are in the invocation log"
        ),
        None => e.pretty(),
    }
}

fn invoke_inner<F>(
    argv: Vec<String>,
    cli_parser: &CliParser,
    feature_stack_builder: F,
) -> PyResult<(
    u8,
    FsCommand,
    Option<DbtCommandExecutionArtifacts>,
    Option<String>,
)>
where
    F: FnOnce(TracingFeature, &Cli, &SystemArgs) -> Arc<FeatureStack>,
{
    let cli = cli_parser
        .try_parse_from(argv)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

    let mut arg = from_lib(&cli);
    let command = arg.command;

    // As of today we serialize all invocations to simplify tracing setup and teardown.
    // Each invocation reloads tracing, even if config hasn't changed and fully
    // shuts it down at the end, flushing all sinks. Sine free-threaded python
    // is not widely used yet, that should not be a major issue, but may be
    // imporved to support genuine homogenous concurrent invocations.
    let _invocation = INVOCATION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // The cap is process-wide, so trace spans would slow every later invocation too.
    // `--log-level trace` therefore acts as `debug`.
    let (tracing_guard, tracing) = begin_invocation(
        trace_config(&cli, cli_parser, &arg),
        LevelFilter::DEBUG,
        &mut arg,
    )?;

    let feature_stack = feature_stack_builder(tracing, &cli, &arg);

    // Apply ANTLR parser config from common args, as run_cli does (main_impl.rs);
    // skipping it diverges in-process parsing from the CLI.
    feature_stack
        .antlr_parser
        .config
        .apply_configuration(&cli.common_args());

    // Ctrl+C is Python's job; the engine gets a never-cancel token.
    let token = dbt_base::cancel::never_cancels();

    let dbt_rt = dbt_runtime::builder::Builder::new()
        .max_blocking_threads(48)
        .thread_stack_size(8 * 1024 * 1024)
        .build();

    // Big stack for the recursive parser/compiler; blocking-thread headroom for adapters.
    let dbt_rt_handle = dbt_rt.handle().clone();
    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
        .max_blocking_threads(512)
        .on_thread_start(move || on_tokio_thread_start(&dbt_rt_handle))
        .on_thread_stop(on_tokio_thread_stop)
        .build()
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;

    // setup_and_execute_fs, not run_cli: run_cli process::exits on panic, killing
    // the interpreter. The library path keeps the global Vortex producer alive so
    // later invokes still log.
    //
    // spawn, not block_on: block_on runs the future on Python's main thread
    // (~1 MB stack on Windows), which the recursive parser overflows; spawn uses
    // a worker with the 8 MB stack above.
    let handle = tokio_rt.spawn(dbt_main::dbt_lib::setup_and_execute_fs(
        arg,
        cli,
        false,
        feature_stack,
        token,
    ));
    let (exit_code, exec, exception) = match tokio_rt.block_on(handle) {
        Ok(Ok(exec)) => (0, Some(exec), None),
        Ok(Err(failure)) => {
            let error = failure.error;
            let artifacts = failure.artifacts;
            let exit_code = error.exit_status().unwrap_or(1) as u8;
            // A handled failure (a failing test, an errored node, a stale source) is
            // fully accounted for by the command's result artifact, so it carries no
            // exception.
            let exception = if artifacts.run_results.is_some() || artifacts.sources.is_some() {
                None
            } else {
                Some(describe_engine_error(
                    artifacts.error_message.as_deref(),
                    error.as_ref(),
                ))
            };
            (exit_code, Some(artifacts), exception)
        }
        // Engine task panicked; surface it instead of an opaque JoinError.
        Err(join_err) => (2, None, Some(format!("dbt engine panicked: {join_err}"))),
    };

    // Torn down explicitly so failures are reported rather than dropped, and before
    // `_invocation` unlocks so the next invocation cannot install its layers first.
    //
    // Printed, not returned on `exception`: that field means "the run errored", and callers
    // reconstruct a dbt-core exit code from it. Telemetry is not the run's verdict.
    if let Err(errors) = tracing_guard.finish() {
        for error in &errors {
            eprintln!("{}", error.pretty());
        }
    }

    Ok((exit_code, command, exec, exception))
}

/// Console-script entrypoint (the `dbt` command). Runs dbt as a CLI in-process
/// and exits — never returns to Python. Unlike [`DbtRunner::invoke`], it mirrors
/// the standalone `dbt` binary: clap and the engine drive the process exit code.
#[pyfunction]
fn run_cli(py: Python<'_>, argv: Vec<String>) -> PyResult<()> {
    let distribution = distribution();
    let code = py.detach(|| {
        run_cli_inner(
            argv,
            &(distribution.cli_parser)(),
            distribution.feature_stack,
        )
    });
    std::process::exit(code as i32);
}

fn run_cli_inner<F>(argv: Vec<String>, cli_parser: &CliParser, feature_stack_builder: F) -> u8
where
    F: FnOnce(TracingFeature, &Cli, &SystemArgs) -> Arc<FeatureStack>,
{
    // argv is Python's sys.argv; parse it explicitly since the process is the
    // interpreter, not `dbt`. Load .env first, same as the standalone binary's
    // `prepare_cli_or_exit`, so clap's `env = "VAR"` attributes see it too.
    dbt_main::init_env_before_parse();
    let cli = match cli_parser.try_parse_from(argv) {
        Ok(cli) => cli,
        // clap printed help/version/usage with the right code; honor it.
        Err(e) => e.exit(),
    };

    // Handle completions before any runtime setup, mirroring prepare_cli_or_exit.
    if let Command::Core(CoreCommand::Completions(args)) = &cli.command {
        cli_parser.write_completions(args.shell, &mut std::io::stdout());
        std::process::exit(0);
    }

    let mut arg = from_main(&cli);

    let init_tracing = match distribution().cli_tracing {
        Some(factory) => factory(&cli, cli_parser, &mut arg),
        None => {
            let config = trace_config(&cli, cli_parser, &arg);
            let config_provider = config.create_config_provider();
            config
                .init(Arc::clone(&config_provider))
                .map(|handle| (handle, config_provider))
        }
    };
    let (telemetry_handle, tracing_config_provider) = match init_tracing {
        Ok(handle) => handle,
        Err(e) => {
            let msg = e.to_string();
            print_trimmed_error(msg);
            std::process::exit(1);
        }
    };

    let tracing = TracingFeature::default()
        .with_config_provider(tracing_config_provider)
        .with_shutdown_handle(telemetry_handle);

    if let Some(resolved_file_log_path) = tracing.config_provider.get_file_log_path() {
        arg.io.log_path = Some(resolved_file_log_path);
    }

    let feature_stack = feature_stack_builder(tracing, &cli, &arg);

    run_cli_with_code(cli, arg, feature_stack)
}
