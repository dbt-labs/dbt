use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use dbt_common::tracing::formatters::duration::format_duration_fixed_width;
use dbt_common::tracing::formatters::node::{format_node_action, format_node_type_fixed_width};

use dbt_adapter::load_store::ResultStore;
use dbt_adapter::{
    Adapter, AdapterType, convert_macro_result_to_record_batch,
    relation::{RelationObject, create_relation},
};
use dbt_clap_core::{
    Cli, Command, CompileArgs, CoreCommand, DocsServeArgs as ClapDocsServeArgs, DocsSubcommand,
    InternalCommand, LoginSubcommand, ProjectTemplate, ShowArgs, StateSubcommand,
};
use dbt_common::cancellation::CancellationToken;
use dbt_common::io_utils::determine_project_dir;
use dbt_common::{
    ErrorCode, FsResult,
    artifact_io::write_artifact_to_file,
    constants::{
        DBT_CATALOG_JSON, DBT_COMPILED_DIR_NAME, DBT_MANIFEST_JSON, DBT_PROJECT_YML, ERROR,
        INSTALLING, VALIDATING, default_index_dir, default_metadata_dir,
    },
    create_root_info_span, fs_err,
    io_args::{DisplayFormat, EvalArgs, ListOutputFormat, Phases, ShowOptions, SystemArgs},
    node_selector::IndirectSelection,
    path::get_target_write_path,
    pretty_string::{GREEN, RED, color_quotes},
    stdfs,
    tracing::{
        dbt_emit::{
            emit_error_log_from_fs_error, emit_error_log_message, emit_info_log_message,
            emit_info_progress_message, emit_warn_log_message,
        },
        dbt_metrics::{
            FusionMetricKey, NodeSubOutcome, OutcomeCountsKey, OutcomeKind, error_count_checkpoint,
            get_error_count, return_exit_code_from_error_counter,
        },
        emit::emit_info_event,
        invocation::create_invocation_attributes,
        metrics::get_metric,
        span_info::record_span_status,
    },
    warn_error_options::{SupportedLegacyWarnError, WarnErrorDecision},
};
use dbt_common::{FsError, io_args::FsCommand};
use dbt_dag::schedule::Schedule;
use dbt_dist::command::execute_get_distribution_info;
use dbt_docs_server::providers::Backend;
use dbt_features::feature_stack::FeatureStack;
use dbt_features::index::write_metadata_parquet;
use dbt_index_core::backend::DuckDbInfoSchemaBackend;
use dbt_index_core::ingest::ingest_state::IngestState;
use dbt_index_core::ingest::metadata_to_parquet::{
    apply_delta_direct, has_persisted_state, ingest_from_metadata_direct,
};
use dbt_index_core::{WriteSource, save_artifact_meta, write_info_schema};
use dbt_init::init;
use dbt_jinja_utils::{
    jinja_environment::JinjaEnv, listener::JinjaTypeCheckingEventListenerFactory,
    utils::get_catalog_by_relations,
};
use dbt_loader::{
    clean::execute_clean_command, execute_deps_command, upload_artifacts_ingest_if_enabled,
};
use dbt_login::{execute_login, execute_login_status};
use dbt_schema_store::{DataStoreTrait, SchemaStoreTrait};
use dbt_schemas::schemas::DbtCommandExecutionArtifacts;
use dbt_schemas::schemas::selection_override::{
    SAMPLE_CAP, format_sample, reconcile_reported_nodes, resolve_selection_override,
};
use dbt_schemas::{
    man::execute_man_command,
    schemas::legacy_catalog::{DbtCatalog, build_catalog},
};
use dbt_schemas::{
    schemas::{
        DbtModel, InternalDbtNodeAttributes, Nodes, RunResultsArtifact,
        common::{DbtMaterialization, ResolvedQuoting},
        relations::base::BaseRelation,
    },
    state::ResolverState,
};
use dbt_state::explain::{StateExplainOptions, execute_state_explain};
use dbt_tasks_core::{
    RunTaskResults,
    task_runner_hooks::TaskRunnerHooksFactory,
    utils::{build_run_results_artifact, write_run_results_json_or_warn},
};
use dbt_tasks_sa::base_context::build_base_context;
use dbt_telemetry::ArtifactType;
use dbt_telemetry::{
    CompiledCodeInline, GenericOpExecuted, NodeOutcome, NodeSkipReason, ProgressMessage,
    ShowDataOutput, ShowDataOutputFormat, ShowResult, TestOutcome,
};

use dbt_vortex::vortex_producer_is_running;
#[cfg(debug_assertions)]
use git_version::git_version;
use minijinja::Value;
use serde_json::{json, to_string_pretty};
use tracing::Instrument;
use vortex_events::{build_result_string, invocation_end_event};

use crate::{
    compilation::{
        DbtCustomScheduleDescription, DbtProjectCompilation, DbtProjectCompilationCacheChanges,
        DbtRunTasksResult, DbtScheduleDescription, show_queries_info_schema, update_manifest,
    },
    retry::{RETRIABLE_COMMANDS, RetryState},
    utils::{InvocationContext, write_catalog_stats_parquet, write_runtime_results_parquet},
    vars::{validate_engine_env_vars, warn_unused_engine_env_vars},
};

// ------------------------------------------------------------------------------------------------

/// A failed dbt invocation together with any artifacts produced before it failed.
pub struct DbtCommandExecutionFailure {
    pub error: Box<FsError>,
    pub artifacts: DbtCommandExecutionArtifacts,
}

impl std::fmt::Debug for DbtCommandExecutionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbtCommandExecutionFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for DbtCommandExecutionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for DbtCommandExecutionFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}

impl From<DbtCommandExecutionFailure> for Box<FsError> {
    fn from(failure: DbtCommandExecutionFailure) -> Self {
        failure.error
    }
}

impl From<Box<FsError>> for DbtCommandExecutionFailure {
    fn from(error: Box<FsError>) -> Self {
        Self {
            error,
            artifacts: DbtCommandExecutionArtifacts::default(),
        }
    }
}

pub type DbtCommandExecutionResult =
    Result<DbtCommandExecutionArtifacts, DbtCommandExecutionFailure>;

/// Runs a full invocation in a multi-invocation execution environment.
///
/// Flushes but doesn't shutdown telemtry if it is enabled.
///
/// Primary test entry point. Embedders that need the captured artifacts (the
/// Python binding) call [`setup_and_execute_fs`] directly instead.
pub async fn execute_fs(
    system_arg: SystemArgs,
    cli: Box<Cli>,
    feature_stack: Arc<FeatureStack>,
    token: CancellationToken,
) -> FsResult<()> {
    setup_and_execute_fs(system_arg, cli, false, feature_stack, token)
        .await
        .map(|_| ())
        .map_err(Into::into)
}

/// Runs a full invocation for one-off execution, forcing full shutdown & discarding returned artifacts.
///
/// Primary cli entrypoint.
pub async fn execute_fs_and_shutdown(
    system_arg: SystemArgs,
    cli: Box<Cli>,
    feature_stack: Arc<FeatureStack>,
    token: CancellationToken,
) -> FsResult<()> {
    setup_and_execute_fs(system_arg, cli, true, feature_stack, token)
        .await
        .map(|_| ())
        .map_err(Into::into)
}

pub async fn setup_and_execute_fs(
    system_arg: SystemArgs,
    cli: Box<Cli>,
    shutdown: bool,
    feature_stack: Arc<FeatureStack>,
    token: CancellationToken,
) -> DbtCommandExecutionResult {
    // Resolve EvalArgs from SystemArgs and Cli. This will create out folders,
    // for commands that need it and canonicalize the paths. May error on invalid paths.
    // If this fails (e.g., not in a dbt project directory), print a concise error and exit 1.
    let mut eval_arg = cli.to_eval_args(system_arg.clone()).map_err(|e| {
        // before the logger is initialized, so we print directly to stderr
        eprintln!(
            "{} {}",
            RED.apply_to(format!("[{ERROR}]")),
            color_quotes(e.pretty().as_str())
        );
        FsError::exit_with_status(1)
    })?;

    // --dirty without --select: synthesize a `seed_id+ seed_id+ ...` selector so the
    // scheduler runs only the dirty nodes and their descendants — ancestors are loaded
    // for dep closure but not scheduled.  Falls back to select=None (run all) when the
    // cache doesn't exist yet or nothing is dirty.
    if cli.common_args().dirty && eval_arg.select.is_none() {
        use dbt_metadata::partial_parse::dirty_select_expression;
        if let Some(expr) = dirty_select_expression(&eval_arg.io) {
            eval_arg.select = Some(expr);
        }
    }

    let invocation_id = eval_arg.io.invocation_id.to_string();
    let send_anonymous_usage_stats = eval_arg.io.send_anonymous_usage_stats;
    let dbt_distribution = feature_stack
        .instrumentation
        .event_emitter
        .dbt_distribution();

    // Capture invocation context now — eval_arg.metadata_dir() is correctly resolved here.
    // Written at exit so every path (success / error / warm-parse / Ctrl+C) is covered.
    let invocation_ctx = if eval_arg.write_metadata {
        let common = cli.common_args();
        Some(InvocationContext::new(
            eval_arg.metadata_dir(),
            &eval_arg.io,
            eval_arg.command,
            &common,
        ))
    } else {
        None
    };

    // Create the Invocation span as a new root
    let invocation_span = create_root_info_span(create_invocation_attributes("dbt", &eval_arg));

    // We are forced to use a mutable argument, because we want to recover artifcats
    // even when execution is short-circuited on Err and thus can't return it as the result type
    let mut artifacts_sink = DbtCommandExecutionArtifacts::default();
    let result = do_execute_fs(&eval_arg, cli, &mut artifacts_sink, feature_stack, &token)
        .instrument(invocation_span.clone())
        .await;

    // Record span run result
    let span_status = match &result {
        Ok(()) => None,
        Err(err) => match err.exit_status() {
            Some(0) => None,
            Some(_) => Some("Executed with errors".to_string()),
            None => Some(format!("Error: {}", err)),
        },
    };
    record_span_status(&invocation_span, span_status.as_deref());

    // Write invocation record — one place covers every exit path.
    if let Some(ctx) = invocation_ctx {
        // NoFilesChanged (warm parse, no-op) propagates via `?` as Err rather than Ok —
        // it's a "successful sentinel", not a real failure. exit_status() == Some(0)
        // catches it and ExitRepl; everything else with exit_status() == None is a real error.
        let status = match &result {
            Ok(()) => "success",
            Err(e) if e.exit_status() == Some(0) => "success",
            Err(_) => "error",
        };
        ctx.write(status);
    }

    // Shutdown must be called to ensure vortex flushes all events.
    // If any event is sent after this, it will be dropped.
    //
    // TODO: this part currently accounts for by far the largest portion of the
    // shutdown (aka "post-exec stall") time. We should optimize this by either:
    // 1) investigate whether vortex itself can be further optimized; or
    // 2) move vortex telemetry into a separate subprocess so it can run fully
    //    async without blocking the main process.
    if send_anonymous_usage_stats || (shutdown && vortex_producer_is_running()) {
        debug_assert!(
            send_anonymous_usage_stats,
            "Vortex producer is running, but send_anonymous_usage_stats \
is false. This should not happen."
        );
        let result_string = build_result_string(&result);
        tokio::task::spawn_blocking(move || {
            // This blocks on the worker thread until the final batch(es)
            // are sent, so we run it as a blocking tokio task.
            invocation_end_event(invocation_id, result_string, dbt_distribution, shutdown);
        })
        .instrument(invocation_span)
        .await
        .map_err(|e| {
            if e.is_cancelled() {
                Ok(()) // ignore cancellation
            } else {
                Err(e) // let JoinError::Panic cause a panic
            }
        })
        .unwrap();
    }

    // Hand the captured artifacts (if any) to the caller. Phase-checkpoint
    // commands (parse, list, ...) signal success via Err(exit_status == 0) — a
    // success sentinel — after the artifacts have been captured, so treat that
    // the same as Ok here. Real errors propagate unchanged.
    match result {
        Ok(()) => Ok(artifacts_sink),
        Err(e) if e.exit_status() == Some(0) => Ok(artifacts_sink),
        Err(error) => Err(DbtCommandExecutionFailure {
            error,
            artifacts: artifacts_sink,
        }),
    }
}

#[allow(clippy::cognitive_complexity)]
async fn do_execute_fs(
    eval_arg: &EvalArgs,
    cli: Box<Cli>,
    artifacts_sink: &mut DbtCommandExecutionArtifacts,
    feature_stack: Arc<FeatureStack>,
    token: &CancellationToken,
) -> FsResult<()> {
    use CoreCommand::*;

    warn_unused_engine_env_vars();

    // Current versions of rustls require us to explicitly install a default provider.
    // The default provider can only be installed once per process, so
    // be defensive here (tests may use the same process)
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .expect("Failed to install crypto provider");
    }

    feature_stack
        .cli
        .hooks
        .will_execute(&cli, eval_arg, &feature_stack)
        .await?;

    if let Command::Core(State(state_args)) = &cli.command {
        let StateSubcommand::Explain(explain_args) = &state_args.subcommand;
        if state_args.common_args.selector.is_some() {
            let err = fs_err!(
                ErrorCode::InvalidArgument,
                "`dbt state explain` does not support --selector. Use --select and --exclude to filter explain output."
            );
            emit_error_log_from_fs_error(*err);
            return Err(FsError::exit_with_status(1));
        }
        let project_dir = state_args
            .common_args
            .project_dir
            .clone()
            .map(Ok)
            .unwrap_or_else(|| {
                if explain_args.log_file.is_some() {
                    std::env::current_dir().map_err(Into::into)
                } else {
                    determine_project_dir(&[], DBT_PROJECT_YML)
                }
            })?;
        let manage_state = state_args.common_args.get_manage_state(&project_dir, true);
        let result = execute_state_explain(StateExplainOptions {
            project_dir,
            log_path: state_args.common_args.log_path.clone(),
            log_file: explain_args.log_file.clone(),
            select: state_args.common_args.select.clone(),
            exclude: state_args.common_args.exclude.clone(),
            manage_state,
            verbose: explain_args.verbose,
        })
        .await;
        return match result {
            Ok(()) => Ok(()),
            Err(err) if err.exit_status().is_some() => Err(err),
            Err(err) => {
                emit_error_log_from_fs_error(*err);
                Err(FsError::exit_with_status(1))
            }
        };
    }

    if let Command::Core(Man(_)) = &cli.command {
        return execute_man_command(eval_arg).await;
    } else if let Command::Core(Internal(internal_args)) = &cli.command {
        return match &internal_args.command {
            InternalCommand::GetDistributionInfo(args) => execute_get_distribution_info(
                args.path.as_deref(),
                args.all,
                feature_stack.cli.command_name,
            ),
        };
    } else if let Command::Core(Login(login_args)) = &cli.command {
        return match login_args.subcommand {
            Some(LoginSubcommand::Status) => execute_login_status().await,
            None => {
                execute_login(
                    Arc::clone(&feature_stack.login.hooks),
                    token,
                    &eval_arg.io.invocation_id,
                )
                .await
            }
        };
    } else if let Command::Core(Docs(docs_args)) = &cli.command {
        return match &docs_args.subcommand {
            Some(DocsSubcommand::Serve(serve_args)) => {
                run_docs_serve(
                    serve_args.clone(),
                    &feature_stack,
                    &eval_arg.io.in_dir,
                    &cli.common_args(),
                )
                .await
            }
            Some(DocsSubcommand::Generate(generate_args)) => {
                run_docs_generate(generate_args.clone(), eval_arg, &cli, feature_stack, token).await
            }
            // An unrecognized subcommand is a typo, not a request. Naming it beats
            // the silent success this arm used to return.
            Some(DocsSubcommand::Other(argv)) => {
                emit_error_log_message(
                    ErrorCode::Generic,
                    format!(
                        "unrecognized subcommand `{}`\n\nUsage: dbt docs <generate|serve>",
                        argv.join(" "),
                    ),
                );
                Err(FsError::exit_with_status(2))
            }
            None => {
                emit_error_log_message(
                    ErrorCode::Generic,
                    "`dbt docs` needs a subcommand.\n\n  \
                     dbt docs generate   Build a statically hostable docs site\n  \
                     dbt docs serve      Build if needed, then serve it locally",
                );
                Err(FsError::exit_with_status(2))
            }
        };
    } else if let Command::Core(Init(init_args)) = &cli.command {
        // Handle init command
        use dbt_init::init::run_init_workflow;

        // Notify the CLI extension hooks that we're initializing a project
        feature_stack
            .cli
            .hooks
            .will_init_project(eval_arg.io.invocation_id, &cli, init_args)
            .await?;

        emit_info_progress_message(ProgressMessage::new_from_action_and_target(
            INSTALLING.to_string(),
            "dbt project and profile setup".to_string(),
        ));

        let project_name = if init_args.project_name == "jaffle_shop" {
            None // Use default
        } else {
            Some(init_args.project_name.clone())
        };

        let project_template = match init_args.sample {
            ProjectTemplate::JaffleShop => init::assets::ProjectTemplateAsset::JaffleShop,
            ProjectTemplate::MomsFlowerShop => init::assets::ProjectTemplateAsset::MomsFlowerShop,
        };

        match run_init_workflow(
            project_name,
            init_args.skip_profile_setup,
            init_args.common_args.profile.clone(), // Get profile from common args
            &project_template,
        )
        .await
        {
            Ok(()) => {
                // If profile setup was not skipped, run debug to validate credentials
                if init_args.skip_profile_setup {
                    return Err(FsError::exit_with_status(0));
                }

                emit_info_log_message(format!(
                    "{} profile inputs, adapters, and connection\n", // Add empty line for spacing
                    GREEN.apply_to(VALIDATING)
                ));
            }
            Err(e) => {
                let code = e.exit_status().unwrap_or(1);
                emit_error_log_from_fs_error(*e);
                return Err(FsError::exit_with_status(code));
            }
        }
    } else if let Command::Core(Deps(deps_args)) = &cli.command {
        let command_name = feature_stack.cli.command_name;
        emit_info_progress_message(ProgressMessage::new_from_action_and_target(
            command_name.to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        ));

        return match execute_deps_command(
            eval_arg,
            deps_args.common_args.get_warn_error(),
            deps_args.common_args.warn_error_options.clone(),
            Some(feature_stack.tracing.config_provider.as_ref()),
            token,
            feature_stack.loader.private_package_resolver.clone(),
        )
        .await
        {
            Ok(()) => Ok(()),
            Err(e) if e.exit_status().is_some() => Err(e),
            Err(e) => {
                emit_error_log_from_fs_error(*e);
                Err(FsError::exit_with_status(1))
            }
        };
    } else if let Command::Core(Clean(clean_args)) = &cli.command {
        let command_name = feature_stack.cli.command_name;
        emit_info_progress_message(ProgressMessage::new_from_action_and_target(
            command_name.to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        ));

        return execute_clean_command(eval_arg, &clean_args.files, token).await;
    }
    // Handle project specific commands
    let hooks_factory = Arc::clone(&feature_stack.task_runner.hooks_factory);
    execute_setup_and_all_phases(
        eval_arg,
        &cli,
        artifacts_sink,
        feature_stack,
        hooks_factory,
        token,
    )
    .await
}

#[allow(clippy::cognitive_complexity)]
pub async fn execute_setup_and_all_phases(
    eval_arg: &EvalArgs,
    cli: &Cli,
    artifacts_sink: &mut DbtCommandExecutionArtifacts,
    feature_stack: Arc<FeatureStack>,
    task_runner_hooks_factory: Arc<dyn TaskRunnerHooksFactory>,
    token: &CancellationToken,
) -> FsResult<()> {
    emit_version_info(eval_arg, feature_stack.cli.command_name)?;

    check_options(cli);
    if let Err(e) = validate_engine_env_vars() {
        emit_error_log_from_fs_error(*e);
        return Err(FsError::exit_with_status(1));
    }

    let mut executor = {
        let arg = Cow::Borrowed(eval_arg);
        let cli = Cow::Borrowed(cli);
        AllPhasesExecutor::new(arg, cli, feature_stack, task_runner_hooks_factory)
    };

    let phases_result = executor.execute_all_phases(token).await;
    let result = match phases_result {
        Ok(()) => Ok(()),
        Err(e) if e.exit_status().is_some() => Err(e),
        Err(e) => {
            // Keep the rendered message for embedders: flattening below drops it,
            // and `exit_with_status` carries no context of its own.
            executor.captured_artifacts.error_message = Some(e.pretty());
            emit_error_log_from_fs_error(*e);
            Err(FsError::exit_with_status(1))
        }
    };

    // Surface an "update available" hint if the background version check
    // produced one. Shared between `dbt` and `dbt-repl` — for the REPL it
    // appears just before the prompt.
    let version_check_handle = executor.version_check_handle_mut().take();
    if let Some(handle) = version_check_handle
        && let Ok(Some(hint)) = handle.await
    {
        emit_info_progress_message(ProgressMessage::new_from_action_and_target(
            "New version available".to_string(),
            hint,
        ));
    }

    // Hand the captured artifacts (if any) up to the caller via the sink.
    *artifacts_sink = executor.captured_artifacts;

    result
}

/// Emits version information as a progress message.
/// In debug builds, includes additional details like git hash and build time.
fn emit_version_info(eval_arg: &EvalArgs, command_name: &str) -> FsResult<()> {
    // current_exe errors when running in dbt-cloud
    // https://github.com/rust-lang/rust/issues/46090
    #[cfg(debug_assertions)]
    {
        use chrono::{DateTime, Local};
        use std::env;
        let exe_path = env::current_exe()
            .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to get current exe path: {}", e))?;
        let modified_time = stdfs::last_modified(&exe_path)?;

        // Convert SystemTime to DateTime<Local>
        let datetime: DateTime<Local> = DateTime::from(modified_time);
        let formatted_time = datetime.format("%Y-%m-%d %H:%M:%S").to_string();
        if eval_arg.from_main {
            let git_hash = git_version!(fallback = "unknown");
            let build_time = format!(
                "{} ({} {})",
                env!("CARGO_PKG_VERSION"),
                git_hash,
                formatted_time
            );
            emit_info_progress_message(ProgressMessage::new_from_action_and_target(
                command_name.to_string(),
                build_time,
            ));
            return Ok(());
        };
    }

    // Show version (always shown in release builds, or in debug builds when not from_main)
    let current_version = env!("CARGO_PKG_VERSION");
    emit_info_progress_message(ProgressMessage::new_from_action_and_target(
        command_name.to_string(),
        current_version.to_string(),
    ));

    Ok(())
}

struct AllPhasesExecutor<'a> {
    arg: Cow<'a, EvalArgs>,
    cli: Cow<'a, Cli>,
    feature_stack: Arc<FeatureStack>,
    start: SystemTime,
    // simple support objects
    jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
    task_runner_hooks_factory: Arc<dyn TaskRunnerHooksFactory>,
    version_check_handle: Option<tokio::task::JoinHandle<Option<String>>>,
    captured_artifacts: DbtCommandExecutionArtifacts,
    /// Previous batch results from retry, to skip already-successful overloads
    previous_batch_results: HashMap<String, dbt_schemas::schemas::BatchResults>,
}

impl<'a> AllPhasesExecutor<'a> {
    pub fn new(
        arg: Cow<'a, EvalArgs>,
        cli: Cow<'a, Cli>,
        feature_stack: Arc<FeatureStack>,
        task_runner_hooks_factory: Arc<dyn TaskRunnerHooksFactory>,
    ) -> Self {
        let start = SystemTime::now();
        let jinja_type_checking_event_listener_factory = feature_stack
            .jinja
            .factory
            .create_type_checking_listener_factory();

        Self {
            arg,
            cli,
            feature_stack,
            start,
            jinja_type_checking_event_listener_factory,
            task_runner_hooks_factory,
            version_check_handle: None,
            captured_artifacts: DbtCommandExecutionArtifacts::default(),
            previous_batch_results: Default::default(),
        }
    }

    fn version_check_handle_mut(&mut self) -> &mut Option<tokio::task::JoinHandle<Option<String>>> {
        &mut self.version_check_handle
    }

    pub fn prepare_for_potential_retry(
        &mut self,
    ) -> FsResult<Option<DbtCustomScheduleDescription>> {
        use CoreCommand::*;

        // Handle retry command: load retry state and create custom schedule
        if let Command::Core(Retry(retry_args)) = &self.cli.command {
            debug_assert!(matches!(self.arg.command, FsCommand::Retry));

            // Load retry state
            let retry_state = {
                let run_results_path = self
                    .arg
                    .state
                    .clone()
                    .unwrap_or_else(|| self.arg.io.out_dir.clone())
                    .join("run_results.json");
                // Gate the retryable `warn` status on the *retry* invocation's
                // own --warn-error, matching dbt-core (which keys off the retry
                // flags, not the original run's). dbt-labs/fs#12417.
                RetryState::from_run_results(&run_results_path, retry_args.common_args.warn_error)
            }?;

            // Get the original command to execute
            let command_for_retry = retry_state.to_command(retry_args).map_err(|other| {
                let mut message = format!("Cannot retry command '{other}' - only ");
                message.push_str(RETRIABLE_COMMANDS.join("/").as_str());
                message += " supported";
                fs_err!(ErrorCode::InvalidArgument, "{message}")
            })?;
            let effective_sa = command_for_retry.static_analysis();

            // Emit info message when preserving non-default SA from original run
            if retry_args.static_analysis.is_none()
                && retry_state.original_static_analysis.is_some()
                && let Some(effective_sa) = effective_sa
            {
                emit_info_log_message(format!(
                    "Using static_analysis={} from original run (override with --static-analysis)",
                    effective_sa
                ));
            }

            emit_info_progress_message(ProgressMessage::new_from_action_and_target(
                "Retrying".to_string(),
                format!(
                    "{} nodes from previous {} command",
                    retry_state.retryable_node_ids.len(),
                    retry_state.original_command
                ),
            ));

            // Modify command in-place eval args with the original command and effective SA
            let arg_for_retry = self.arg.to_mut();
            arg_for_retry.command = command_for_retry.as_command();
            arg_for_retry.static_analysis = effective_sa;
            arg_for_retry.full_refresh = retry_state.original_full_refresh;

            // Create a custom schedule for the retryable nodes from the original run.
            // This keeps retry bounded to nodes recorded in run_results.json instead
            // of broadening the selection by expanding descendants.
            //
            // Retry executes exactly the node ids recorded in run_results.json, so
            // indirect selection must be `Empty`. The recorded set already contains
            // every node that has to re-run -- in particular, tests skipped behind a
            // failed model are recorded `skipped`, which is retryable -- so expanding
            // is never needed. Any expansion is by construction a node the original
            // command did not run: an already-passed unit test, or a test excluded by
            // the original --indirect-selection / --exclude. This matches dbt-core,
            // whose retry path replaces the graph queue outright and never consults
            // indirect selection (dbt-labs/dbt-core#14536).
            // Check ids are **not schedulable**: checks run before the task graph exists, so they
            // are not nodes in it. Putting one in a custom schedule produced a false green —
            // `dbt retry` after a failing check selected only that id, row scoping then found no
            // model to scope to, and every check reported `skipped`, so retry exited 0 on a project
            // whose check still fails. Split them: check ids become names to re-run, everything else
            // becomes the schedule.
            let (retry_check_ids, retry_node_ids): (Vec<String>, Vec<String>) = retry_state
                .retryable_node_ids
                .into_iter()
                .partition(|id| id.starts_with("check."));
            let failed_check_names = crate::retry::check_names_from_retry_ids(&retry_check_ids);

            // `dbt check` never needs a schedule: it compiles nothing. For build-shaped commands a
            // schedule of the failed *nodes* is still what limits the work.
            let custom_schedule = if matches!(&command_for_retry, Check(_)) {
                None
            } else {
                Some(DbtCustomScheduleDescription {
                    unique_ids: retry_node_ids,
                    include_parents: false,
                    include_children: false,
                    indirect_selection: IndirectSelection::Empty,
                })
            };

            self.previous_batch_results = retry_state.previous_batch_results;

            let common_args = command_for_retry.common_args().clone();
            let cli_for_retry = Cli {
                command: Command::Core(command_for_retry),
                common_args,
            };
            self.cli = Cow::Owned(cli_for_retry);
            // `self.arg` was built from the *original* `retry` command, so nothing the
            // reconstructed command carries reaches it automatically. `check_names` is what
            // narrows the gate to the checks that actually failed — for both `dbt check` and
            // a check-blocked `dbt build`. Empty means "run every check" (a model-only
            // failure, or a legacy artifact whose ids did not decode).
            self.arg.to_mut().check_names = failed_check_names;
            Ok(custom_schedule)
        } else {
            Ok(None)
        }
    }

    /// Initializes a new compilation.
    /// The resulting compilation is based on the state of the file system.
    pub async fn load_and_resolve_state(
        &mut self,
        token: &CancellationToken,
    ) -> FsResult<(
        DbtProjectCompilation,
        JinjaEnv,
        Option<DbtProjectCompilationCacheChanges>,
    )> {
        let event_emitter = self
            .feature_stack
            .as_ref()
            .instrumentation
            .event_emitter
            .as_ref();

        DbtProjectCompilation::initialize_cli(
            &self.feature_stack,
            self.arg.as_ref(),
            self.cli.as_ref(),
            Some(event_emitter),
            Arc::clone(&self.jinja_type_checking_event_listener_factory)
                as Arc<dyn JinjaTypeCheckingEventListenerFactory>,
            token,
            &mut self.version_check_handle,
            // TODO: Same ugly pattern as artifacts sink. `initialize` needs to be refactored
            // to avoid early exit with side-effect path within it and instead always return
            // artifacts to be written by executor at the end. Then this may be removed
            &mut self.captured_artifacts,
        )
        .await
    }

    /// Run tasks based on the arguments.
    /// This can be called multiple times on the same compilation.
    async fn run_tasks(
        &mut self,
        compilation: &mut DbtProjectCompilation,
        jinja_env: JinjaEnv,
        compilation_cache_changes: Option<&DbtProjectCompilationCacheChanges>,
        schedule: Schedule<String>,
        token: &CancellationToken,
    ) -> FsResult<DbtRunTasksResult> {
        compilation
            .run_tasks(
                self.arg.as_ref(),
                self.cli.as_ref(),
                self.start,
                jinja_env,
                Arc::clone(&self.feature_stack),
                schedule,
                compilation_cache_changes,
                None,
                Arc::clone(&self.jinja_type_checking_event_listener_factory)
                    as Arc<dyn JinjaTypeCheckingEventListenerFactory>,
                self.task_runner_hooks_factory.as_ref(),
                token,
                self.previous_batch_results.clone(),
                &mut self.captured_artifacts,
            )
            .await
    }

    /// Handle inline compile - print the compiled SQL
    async fn run_inline_compile(&self, resolved_state: &ResolverState) -> FsResult<()> {
        debug_assert!(matches!(
            &self.cli.command,
            Command::Core(CoreCommand::Compile(CompileArgs {
                inline: Some(_),
                ..
            })),
        ));
        // Find the inline model in the compiled nodes
        let inline_model = resolved_state
            .nodes
            .models
            .values()
            .find(|model| model.materialized() == DbtMaterialization::Inline)
            .ok_or_else(|| {
                fs_err!(
                    ErrorCode::Unexpected,
                    "Failed to find inline model after compilation"
                )
            })?;

        Self::emit_inline_compiled_sql(inline_model, self.arg.as_ref()).await
    }

    /// Read the compiled inline SQL from the target directory and emit a
    /// `CompiledCodeInline` telemetry event.
    async fn emit_inline_compiled_sql(
        inline_model: &Arc<DbtModel>,
        arg: &EvalArgs,
    ) -> FsResult<()> {
        let absolute_compiled_path = get_target_write_path(
            &arg.io.in_dir,
            &arg.io.out_dir.join(DBT_COMPILED_DIR_NAME),
            &inline_model.__common_attr__.package_name,
            &inline_model.__common_attr__.path,
            &inline_model.__common_attr__.original_file_path,
        );

        let compiled_sql = dbt_common::tokiofs::read_to_string(&absolute_compiled_path)
            .await
            .map_err(|_| {
                fs_err!(
                    ErrorCode::Unexpected,
                    "Failed to read compiled inline SQL at {}",
                    absolute_compiled_path.display()
                )
            })?;

        emit_info_event(CompiledCodeInline { sql: compiled_sql }, None);

        Ok(())
    }

    fn emit_selected_compile_output(
        &self,
        resolved_state: &ResolverState,
        schedule: &Schedule<String>,
        map_compiled_sql: &HashMap<&str, Option<&str>>,
    ) -> FsResult<()> {
        if !matches!(
            &self.cli.command,
            Command::Core(CoreCommand::Compile(CompileArgs { inline: None, .. }))
        ) || self.arg.select.is_none()
            || schedule.all_selected_nodes.len() != 1
        {
            return Ok(());
        }

        let unique_id = schedule
            .all_selected_nodes
            .iter()
            .next()
            .expect("all_selected_nodes has exactly one entry");
        if !schedule.selected_nodes.contains(unique_id) {
            return Ok(());
        }

        let Some(model) = resolved_state.nodes.models.get(unique_id) else {
            return Ok(());
        };

        let has_only_progress_show_options = !self.arg.io.show.is_empty()
            && self.arg.io.show.iter().all(|option| {
                matches!(
                    option,
                    ShowOptions::Progress
                        | ShowOptions::ProgressParse
                        | ShowOptions::ProgressHydrate
                        | ShowOptions::ProgressRender
                        | ShowOptions::ProgressAnalyze
                        | ShowOptions::ProgressRun
                        | ShowOptions::Completed
                )
            });
        if get_error_count() > 0
            || !has_only_progress_show_options
            || !model
                .__common_attr__
                .language
                .as_deref()
                .is_some_and(|language| language.eq_ignore_ascii_case("sql"))
        {
            return Ok(());
        }

        let compiled_sql: Cow<str> = map_compiled_sql
            .get(unique_id.as_str())
            .and_then(Option::as_deref)
            .map(|s| s.into())
            .or_else(|| {
                let path = get_target_write_path(
                    &self.arg.io.in_dir,
                    &self.arg.io.out_dir.join(DBT_COMPILED_DIR_NAME),
                    &model.__common_attr__.package_name,
                    &model.__common_attr__.path,
                    &model.__common_attr__.original_file_path,
                );
                stdfs::read_to_string(&path).ok().map(Cow::Owned)
            })
            .ok_or_else(|| {
                fs_err!(
                    ErrorCode::Unexpected,
                    "Failed to find compiled SQL for {}",
                    unique_id
                )
            })?;

        let node_name = model.__common_attr__.name.clone();
        let (output_format, content) = if self.arg.format == DisplayFormat::Json {
            (
                ShowDataOutputFormat::Json,
                to_string_pretty(&json!({
                    "node": node_name,
                    "compiled": compiled_sql,
                }))?,
            )
        } else {
            (
                ShowDataOutputFormat::Text,
                format!("Compiled node '{node_name}' is:\n{compiled_sql}"),
            )
        };

        emit_info_event(
            ShowDataOutput::new_with_default_code(
                output_format,
                content,
                node_name,
                false,
                Some(unique_id.clone()),
                vec![],
            ),
            None,
        );

        Ok(())
    }

    fn should_exit_with_run_result_warning(&self) -> bool {
        let should_upgrade_warning = [
            SupportedLegacyWarnError::RunResultWarning,
            SupportedLegacyWarnError::RunResultWarningMessage,
        ]
        .into_iter()
        .any(|legacy| {
            self.arg
                .warn_error_options
                .decision_for_supported_legacy(legacy)
                == WarnErrorDecision::UpgradeToError
        });

        should_upgrade_warning
            && get_error_count() == 0
            && [
                NodeSubOutcome::TestWarned,
                NodeSubOutcome::FreshnessWarned,
                NodeSubOutcome::NodeWarned,
            ]
            .into_iter()
            .any(|sub_outcome| {
                get_metric(FusionMetricKey::OutcomeCounts(OutcomeCountsKey::new(
                    OutcomeKind::Node(NodeOutcome::Success),
                    NodeSkipReason::Unspecified,
                    Some(sub_outcome),
                ))) > 0
            })
    }

    async fn write_json(
        &self,
        run_task_results: &RunTaskResults,
        compilation: &DbtProjectCompilation,
        jinja_env: &Arc<JinjaEnv>,
        base_context: &BTreeMap<String, Value>,
        resolved_state: &ResolverState,
        adapter: &Arc<Adapter>,
    ) -> FsResult<Option<DbtCatalog>> {
        debug_assert!(self.arg.write_json);

        if self.arg.write_catalog && !self.arg.write_metadata {
            let metadata_adapter = adapter
                .metadata_adapter()
                .expect("Expected implements MetadataAdapter");
            let relations = metadata_adapter
                .create_relations_from_executed_nodes(resolved_state, &run_task_results.stats.run);
            // Returned, not discarded: this is the only fetch on the --write-catalog
            // without --write-metadata path, so a library caller has no other source.
            return Ok(Some(
                write_catalog_json(
                    adapter,
                    resolved_state,
                    relations,
                    jinja_env,
                    compilation.root_project_name(),
                    base_context,
                    self.arg.as_ref(),
                    20,
                )
                .await?,
            ));
        }

        Ok(None)
    }

    #[allow(clippy::cognitive_complexity)]
    pub async fn execute_all_phases(&mut self, token: &CancellationToken) -> FsResult<()> {
        use CoreCommand::*;

        // `dbt show --info` / `--inline` with `{{ info_schema() }}` only reads
        // `target/info_schema/`. Do not parse, compile, or write artifacts.
        if let Command::Core(Show(show_args)) = &self.cli.command
            && show_queries_info_schema(show_args)
        {
            let info = show_args.info.clone();
            let inline = show_args.inline.clone();
            let metadata_dir = self.arg.metadata_dir();
            let format = self.arg.format;
            let limit = self.arg.limit;
            let token = token.clone();
            // Reads the parse metadata through a DuckDB adapter, so it runs on
            // a `dbt-runtime` worker: every database connection must be created
            // by one.
            dbt_runtime::spawn_blocking(move || {
                dbt_tasks_sa::show_info::run_show_info_schema(
                    info.as_deref(),
                    inline.as_deref(),
                    &metadata_dir,
                    format,
                    limit,
                    token,
                )
            })
            .await
            .map_err(|e| fs_err!(ErrorCode::Generic, "spawn_blocking join error: {e}"))??;
            return Ok(());
        }

        let type_ops_factory = Arc::clone(&self.feature_stack.adapter.type_ops_factory);

        let retry_schedule = self.prepare_for_potential_retry()?;

        // Implied index for `build`/`run`/`check` is EvalArgs-only (`Cli.to_eval_args`).
        // `dbt retry` reconstructs `command` but keeps the `EvalArgs` produced from
        // `Retry`, which did not get that default. Without it the parse-time check gate
        // reads the previous invocation's index. Honor `--no-write-index` the same way
        // the default does. Do not set `Cli.common_args.write_index`: that turns partial
        // parse/load on, and a warm `dbt check --select <model>` unique_id-filters checks
        // out of `ResolverState` (empty success).
        if matches!(self.arg.command, FsCommand::Build | FsCommand::Check)
            && !self.arg.write_index
            && !self.cli.common_args.no_write_index
        {
            let arg = self.arg.to_mut();
            arg.write_metadata = true;
            arg.write_index = true;
            arg.write_index_implied = true;
        }

        let (mut compilation, jinja_env, compilation_cache_changes) =
            self.load_and_resolve_state(token).await?;

        // Inform the user that schemas and CLL require --static-analysis strict.
        // CLL is auto-enabled when generate-info-schema (or write-index) meets
        // strict SA; this advisory covers the remaining epochs-only path.
        // Emitted after load_and_resolve_state so the project's
        // warn_error_options (applied during loading) can silence/upgrade these warnings.
        //
        // Not when `docs generate` drove this compile: the user passed none of these flags,
        // so advising them to add some to a command they did not run is noise — and under
        // `--warn-error` it would fail the export. The export prints its own lineage hint,
        // which names the command that would produce it.
        //
        // Nor when the index came from a command default (`write_index_implied`), for exactly
        // the same reason: a plain `dbt build` never mentioned the index, so telling it which
        // flags would enrich one is noise on every invocation, and fails under `--warn-error`.
        let advise_index_flags = self.arg.write_metadata
            && matches!(
                self.arg.command,
                FsCommand::Compile | FsCommand::Build | FsCommand::Run
            )
            && self.arg.command_entrypoint != FsCommand::Docs
            && !self.arg.write_index_implied;
        let strict_static_analysis = self
            .arg
            .static_analysis
            .is_some_and(dbt_common::static_analysis::is_strict_static_analysis);
        // Name the public flag when the user passed one, so the advice is actionable.
        // `--write-index` / `--write-metadata` are hidden; still name them if that is
        // what the invocation actually set.
        let advised_flag = if self.arg.generate_info_schema {
            "--generate-info-schema"
        } else if self.arg.write_index {
            "--write-index"
        } else {
            "--write-metadata"
        };
        if advise_index_flags && !strict_static_analysis {
            emit_warn_log_message(
                ErrorCode::Generic,
                format!(
                    "{advised_flag}: column types will not be populated without `--static-analysis strict`, which also enables column-level lineage."
                ),
            );
        } else if advise_index_flags && strict_static_analysis && !self.arg.write_lineage {
            emit_warn_log_message(
                ErrorCode::Generic,
                format!(
                    "{advised_flag}: column-level lineage is not written. Pass `--generate-info-schema` with `--static-analysis strict` to include it."
                ),
            );
        }

        let schedule_desc = retry_schedule
            .as_ref()
            .map(DbtScheduleDescription::Custom)
            .unwrap_or(DbtScheduleDescription::Default);

        let schedule = compilation
            .create_schedule(
                self.cli.as_ref(),
                self.arg.as_ref(),
                schedule_desc,
                Default::default(),
                token,
            )
            .await?;

        // Check rows for `run_results.json`. Populated by the gate below and merged into the
        // artifact after the graph runs, so a passing check still appears in the results.
        let mut check_result_rows: Vec<dbt_schemas::schemas::RunResultOutput> = Vec::new();

        // Parse-time checks run here: parse has finished, the parse layer is in the index, and the
        // task graph has not been built yet. A failing check therefore needs no graph edges to stop
        // anything — we simply do not proceed. `dbt check` and `dbt build` share this one call, so
        // they cannot disagree about scoping or about what counts as a violation.
        //
        // Checks do not run on `dbt run` (or `test`, `compile`, `seed`, …): those commands share
        // this same `execute_all_phases` pipeline, but a check is a project-quality gate on the
        // build as a whole, not something a narrower command implicitly opts into. `command`, not
        // `command_entrypoint`, because a retry rewrites `command` back to the original command
        // (see the retry comment on `retrying` below), so a retried build/check still gates here.
        //
        // `--skip-checks` is the one opt-out: the user asked to skip, so do not warn. Nothing
        // about the index enters into it any more -- the views a check queries are declared over
        // the parse metadata epochs, so neither `--no-write-index` nor a failed index write
        // decides whether the gate runs. That was the whole point of reading the metadata: the
        // gate no longer depends on a conversion having succeeded first.
        if matches!(self.arg.command, FsCommand::Build | FsCommand::Check) && !self.arg.skip_checks
        {
            use dbt_tasks_sa::check::run_parse_time_checks;

            let checks: Vec<_> = compilation
                .resolved_state()
                .nodes
                .checks
                .values()
                .cloned()
                .collect();
            // A selector scopes a check's *rows*, never whether it runs, so `dbt check <name>`
            // narrowing is applied here rather than by the scheduler.
            //
            // `dbt retry` after either `check` or `build` re-runs only the checks recorded as
            // failed, via `check_names`. A retried build still materializes the skipped/failed
            // nodes from the original run if those checks then pass.
            //
            // `command` is rewritten to the original command during retry, so
            // `command_entrypoint` is what still says how we were invoked.
            let retrying = self.arg.command_entrypoint == FsCommand::Retry;
            let named: Vec<_> = if self.arg.check_names.is_empty() {
                checks
            } else {
                // A requested name matching no check at all (typo, or a check renamed since a
                // failing run recorded it) must not be silently dropped: with every name unmatched
                // this would otherwise leave `named` empty and skip the entire gate below, exiting
                // 0 having verified nothing. A *disabled* check is a different case — naming one
                // stays a no-op success, so it has to be told apart from a name that resolves to
                // nothing, which is why the resolver records disabled checks rather than dropping
                // them.
                let known_names: HashSet<&str> = checks
                    .iter()
                    .map(|c| c.__common_attr__.name.as_str())
                    .chain(
                        compilation
                            .resolved_state()
                            .disabled_nodes
                            .checks
                            .values()
                            .map(|c| c.__common_attr__.name.as_str()),
                    )
                    .collect();
                let unknown_names: Vec<&str> = self
                    .arg
                    .check_names
                    .iter()
                    .map(|n| n.as_str())
                    .filter(|n| !known_names.contains(n))
                    .collect();
                if !unknown_names.is_empty() {
                    return Err(fs_err!(
                        ErrorCode::InvalidArgument,
                        "no check named '{}'",
                        unknown_names.join("', '")
                    ));
                }
                checks
                    .into_iter()
                    .filter(|c| {
                        self.arg
                            .check_names
                            .iter()
                            .any(|n| *n == c.__common_attr__.name)
                    })
                    .collect()
            };
            if !named.is_empty() {
                if let Some(reason) =
                    dbt_tasks_sa::check::metadata_unavailable_reason(&self.arg.metadata_dir())
                {
                    emit_warn_log_message(
                        ErrorCode::CheckMetadataUnavailable,
                        format!("{reason}. Skipping checks..."),
                    );
                } else {
                    // Naming checks and scoping their rows are orthogonal: `check_names` chooses
                    // which checks run, a selector chooses which of their rows are reported. So
                    // `dbt check <name> --select <subset>` composes -- run that check, report only
                    // the subset's violations.
                    //
                    // Never on a retry, and that exclusion is load-bearing rather than tidy:
                    // `check_names` is set from the retry artifact, and the accompanying schedule
                    // holds only the failed nodes. Scoping to it would verify a fraction of the gate
                    // and report the rest as a green `skipped`, which is how a retry of a failing
                    // check once came to exit 0.
                    let selection_active =
                        !retrying && (schedule.select.is_some() || schedule.exclude.is_some());
                    let scope = dbt_tasks_sa::check::scope_for_selection(
                        selection_active,
                        schedule.selected_nodes.iter(),
                    );
                    // Checks query the parse metadata through a DuckDB
                    // adapter, so they run on a `dbt-runtime` worker: every
                    // database connection must be created by one.
                    let outcome = {
                        let metadata_dir = self.arg.metadata_dir();
                        let scope = scope.clone();
                        dbt_runtime::spawn_blocking(move || {
                            run_parse_time_checks(&named, &metadata_dir, scope.as_ref(), 5)
                        })
                        .await
                        .map_err(|e| {
                            fs_err!(ErrorCode::Generic, "spawn_blocking join error: {e}")
                        })?
                    };

                    // A selector that matched nothing is one fact about the invocation, not one
                    // fact per check: twenty checks produced twenty identical `CheckSkipped`
                    // warnings, none of which said why. Say it once, in the words every other
                    // command uses for an empty selection, and drop the per-check lines it
                    // explains -- their `skipped` *status* still goes to `run_results.json`,
                    // which is what `dbt retry` and any honest reading of the run depend on.
                    //
                    // `check` is the command that has to say it here, because it is the one
                    // deliberately exempt from the schedule phase's own empty-selection warning
                    // (its schedule is emptied on purpose once the gate passes, so "nothing to
                    // do" would be a lie). `build` reaches that warning normally; repeating it
                    // is exactly what this is trying to stop.
                    let empty_selection = scope.as_ref().is_some_and(|s| s.is_empty());
                    if empty_selection && self.arg.command == FsCommand::Check {
                        if let Some(select_expr) = &schedule.select {
                            emit_warn_log_message(
                                ErrorCode::NoNodesForSelectionCriteria,
                                format!(
                                    "The selection criterion '{select_expr}' does not match any enabled nodes"
                                ),
                            );
                        }
                    }
                    // Suppress only where the reason does get stated. An `--exclude`-only
                    // `dbt check` has no criterion to name and no schedule-phase warning coming,
                    // so silence there would be worse than repetition: it keeps its lines.
                    let empty_selection_explained = empty_selection
                        && (schedule.select.is_some() || self.arg.command != FsCommand::Check);

                    for r in &outcome.results {
                        // One result line per check, in the shape a test's takes:
                        // right-aligned verdict, fixed-width duration, node type, name.
                        // Built from the same helpers the node formatter uses, so the
                        // columns line up with the models and tests printed around it
                        // on `dbt build`. Checks are not tasks, so they do not flow
                        // through `NodeProcessed` -- borrowing the formatting is what
                        // keeps them from looking like a different program's output.
                        //
                        // The verdict comes from `format_node_action` rather than a
                        // local match, so a check's label and colour are a test's by
                        // construction: green Passed, yellow Warned, red Failed. The
                        // file and JSON layers strip ANSI from message bodies, so
                        // colouring here reaches the terminal only.
                        let (outcome, test_outcome, skip_reason) = match r.status {
                            "pass" => (NodeOutcome::Success, Some(TestOutcome::Passed), None),
                            "warn" => (NodeOutcome::Success, Some(TestOutcome::Warned), None),
                            "fail" => (NodeOutcome::Success, Some(TestOutcome::Failed), None),
                            "skipped" => (
                                NodeOutcome::Skipped,
                                None,
                                Some(NodeSkipReason::Unspecified),
                            ),
                            // Not evaluated at all: red Failed, and the error below says
                            // why. Reporting it as a test failure would claim the check
                            // ran and disagreed with the project.
                            _ => (NodeOutcome::Error, None, None),
                        };
                        emit_info_log_message(format!(
                            "{} [{}] {} {}",
                            format_node_action(
                                outcome,
                                skip_reason,
                                test_outcome,
                                None,
                                false,
                                true
                            ),
                            format_duration_fixed_width(Duration::from_secs_f64(r.execution_time)),
                            format_node_type_fixed_width("check", true),
                            r.name,
                        ));

                        match r.status {
                            // The line above is the whole report for a pass.
                            "pass" => {}
                            // And for a skip whose reason was already given once, above.
                            "skipped" if empty_selection_explained => {}
                            "skipped" => emit_warn_log_message(
                                ErrorCode::CheckSkipped,
                                format!(
                                    "check '{}' skipped: {}",
                                    r.name,
                                    r.message.as_deref().unwrap_or("nothing in scope")
                                ),
                            ),
                            _ => {
                                // Show the rows the check's SQL returned, the way
                                // `dbt show` shows a query's rows. `message` keeps the
                                // compact `col=value` form for `run_results.json`,
                                // which is read by tools rather than by people.
                                let detail = r
                                    .rows
                                    .as_deref()
                                    .map(|t| format!("\n{t}"))
                                    .or_else(|| r.message.as_deref().map(|m| format!("\n  {m}")))
                                    .unwrap_or_default();
                                let count = r
                                    .violations
                                    .map(|v| format!(" with {v} violation(s)"))
                                    .unwrap_or_default();
                                // Distinct codes per outcome so `warn_error_options` can target a
                                // check without also catching every unrelated `Generic` warning.
                                // Error-severity violations are errors; warn-severity stays a
                                // warning so it can still be promoted.
                                //
                                // A check that could not be evaluated at all is an error whatever
                                // its severity, and the level here is what makes that stick: the
                                // exit code comes off the error counter, so emitting this as a
                                // warning exited 0 — the same as a check that ran and found
                                // nothing. A check that has quietly stopped working is exactly the
                                // one CI must not wave through.
                                match r.status {
                                    "fail" => emit_error_log_message(
                                        ErrorCode::CheckFailed,
                                        format!("check '{}' failed{count}{detail}", r.name),
                                    ),
                                    "warn" => emit_warn_log_message(
                                        ErrorCode::CheckWarned,
                                        format!("check '{}' found{count}{detail}", r.name),
                                    ),
                                    _ => emit_error_log_message(
                                        ErrorCode::CheckEvaluationFailed,
                                        format!(
                                            "check '{}' could not be evaluated:{count}{detail}",
                                            r.name
                                        ),
                                    ),
                                }
                            }
                        }
                    }

                    // `run_results.json` rows keyed `check.<project>.<name>`, which is what `dbt retry`
                    // reads to decide what to re-run.
                    check_result_rows = outcome
                        .results
                        .iter()
                        .map(|r| {
                            dbt_schemas::schemas::RunResultOutput::synthetic(
                                r.unique_id.clone(),
                                r.status,
                                r.message.clone(),
                                r.violations.map(|v| v as i64),
                            )
                        })
                        .collect();

                    if outcome.failed > 0 {
                        // Persist before bailing: the usual artifact write happens after the task graph,
                        // which never runs. Without this `dbt retry` has nothing to read and would
                        // silently re-run everything instead of the checks that failed.
                        if self.arg.write_json {
                            let empty = dbt_schemas::stats::Stats {
                                stats: Vec::new(),
                                nodes: None,
                                batch_results: Default::default(),
                                compiled_code: Default::default(),
                            };
                            let mut artifact = build_run_results_artifact(
                                &empty,
                                &HashMap::new(),
                                self.arg.as_ref(),
                            );
                            artifact.results = std::mem::take(&mut check_result_rows);
                            // The graph never runs, so nothing else reports itself. Record what it
                            // would have processed as `skipped`, which is retryable: without these
                            // rows `dbt retry` sees only the check, and once the check is fixed it
                            // reports "nothing to do" and exits 0 having built none of the models.
                            // Only `build`: a plain `dbt check` materializes nothing even when it
                            // passes, so there is nothing to mark skipped, and its own retry narrows
                            // via `check_names` instead (see `retrying` above).
                            if self.arg.command == FsCommand::Build {
                                artifact.results.extend(
                                    schedule
                                        .selected_nodes
                                        .iter()
                                        .filter(|id| !id.starts_with("check."))
                                        .map(|unique_id| {
                                            dbt_schemas::schemas::RunResultOutput::synthetic(
                                                unique_id.clone(),
                                                "skipped",
                                                Some(
                                                    "skipped because a parse-time check failed"
                                                        .to_string(),
                                                ),
                                                None,
                                            )
                                        }),
                                );
                            }
                            write_run_results_json_or_warn(&artifact, self.arg.as_ref());
                            self.captured_artifacts.run_results = Some(artifact);
                        }

                        // Stop before the graph is built. Per-check CheckFailed errors are already
                        // on the counter; ExitWithStatus fails the command without a second coded
                        // error. "No models compiled" is recorded on skipped run_results rows.
                        return Err(return_exit_code_from_error_counter());
                    }
                }
            }
        }

        // `dbt check` never needs the task graph: a check's rows are already scoped above, and
        // nothing else in the project needs rendering, analyzing, or running to answer "did the
        // checks pass". Emptying the schedule here — after create_schedule's own "nothing to do"
        // warning has already had its one chance to fire against the *real* selection — means
        // run_tasks does no Render/Analyze work and that warning path never re-runs to complain
        // about the emptiness we are about to introduce.
        let schedule = if self.arg.command == FsCommand::Check {
            Schedule::default()
        } else {
            schedule
        };

        // Validate show command selection before running any tasks.
        // Nodes present only because they were pulled in by ephemeral-ancestor expansion
        // (i.e. in `selected_nodes` but not `all_selected_nodes`) don't count as user-selected
        // targets here.
        if let Command::Core(Show(ShowArgs { inline: None, .. })) = &self.cli.command {
            let user_selected_nodes: Vec<&String> = schedule
                .selected_nodes
                .iter()
                .filter(|n| schedule.all_selected_nodes.contains(*n))
                .collect();
            if user_selected_nodes.len() > 1 {
                return Err(fs_err!(
                    ErrorCode::InvalidArgument,
                    "Only one node can be selected for show: {}, {}, and {} more",
                    user_selected_nodes[0],
                    user_selected_nodes[1],
                    user_selected_nodes.len() - 1
                ));
            }
        }

        if self.arg.command == FsCommand::List {
            self.captured_artifacts.list_items = Some(
                schedule
                    .show_dbt_nodes(
                        &compilation.resolved_state.nodes,
                        ListOutputFormat::Selector,
                        &[],
                    )
                    .into_iter()
                    .map(|item| item.content)
                    .collect(),
            );
        }

        let run_tasks_result = self
            .run_tasks(
                &mut compilation,
                jinja_env,
                compilation_cache_changes.as_ref(),
                schedule.clone(),
                token,
            )
            .await;
        let (run_task_args, run_task_results, jinja_env, adapter, compilation_cache_state) =
            match run_tasks_result {
                Ok(result) => result,
                Err(err) => {
                    // The manifest represents the parsed project state and is always valid after
                    // load/resolve completes. Writing it unconditionally ensures downstream
                    // systems (codex ingestion, job.run.completed webhooks) can process failed
                    // runs regardless of failure mode — matching dbt-core behaviour.
                    // Parse already writes it during load/resolve.
                    if self.arg.write_json && self.arg.command != FsCommand::Parse {
                        // Write run_results.json with error status for all selected nodes
                        // so that `dbt retry` can pick them up after compilation failures.
                        // Only write for real errors (no exit_status); normal phase-checkpoint
                        // exits (list, format, lint, schedule, source freshness) carry an
                        // exit_status and must not produce spurious "Compilation Error" results.
                        if err.exit_status().is_none() {
                            let now = SystemTime::now();
                            let error_stats = dbt_schemas::stats::Stats {
                                stats: schedule
                                    .selected_nodes
                                    .iter()
                                    .map(|uid| dbt_common::stats::Stat {
                                        unique_id: uid.clone(),
                                        num_rows: None,
                                        rows_affected: None,
                                        start_time: now,
                                        end_time: now,
                                        status: dbt_common::stats::NodeStatus::Errored,
                                        thread_id: "main".to_string(),
                                        message: Some("Compilation Error".to_string()),
                                    })
                                    .collect(),
                                nodes: Some(compilation.nodes().clone()),
                                batch_results: Default::default(),
                                compiled_code: Default::default(),
                            };

                            // Prepare artifact
                            let run_results_artifact = build_run_results_artifact(
                                &error_stats,
                                // Adapter responses not available since we errored at compilation
                                &HashMap::new(),
                                self.arg.as_ref(),
                            );

                            write_run_results_json_or_warn(
                                &run_results_artifact,
                                self.arg.as_ref(),
                            );

                            // Save artifact for callers
                            self.captured_artifacts.run_results = Some(run_results_artifact);

                            if self.arg.write_metadata {
                                write_runtime_results_parquet(
                                    &error_stats,
                                    &HashMap::new(),
                                    self.arg.as_ref(),
                                );
                            }
                        }

                        let dbt_manifest = compilation.take_dbt_manifest();
                        if let Err(e) = write_artifact_to_file(
                            &dbt_manifest,
                            ArtifactType::Manifest,
                            &self.arg.io.out_dir,
                            DBT_MANIFEST_JSON,
                            &self.arg.io.in_dir,
                        ) {
                            self.captured_artifacts.manifest = Some(dbt_manifest);
                            return Err(e);
                        };

                        self.captured_artifacts.manifest = Some(dbt_manifest);
                    }
                    return Err(err);
                }
            };

        let resolved_state = Arc::clone(&run_task_results.resolved_state);

        // Prepare artifact
        let run_results_artifact = build_run_results_artifact(
            &run_task_results.stats.run,
            &run_task_results.adapter_responses,
            self.arg.as_ref(),
        );

        // Write run_results.json eagerly from real stats so that it persists
        // even if post-execution steps (did_run_tasks, update_manifest,
        // save_build_cache, did_compile, etc.) fail before the late write_json() call.
        //
        // Checks are not tasks, so their verdicts are not in `stats.run` — merge the rows the gate
        // produced, keyed `check.<project>.<name>`, which is what `dbt retry` reads.
        let mut run_results_artifact = run_results_artifact;
        run_results_artifact.results.append(&mut check_result_rows);

        if self.arg.write_json && self.arg.command != FsCommand::Parse {
            write_run_results_json_or_warn(&run_results_artifact, self.arg.as_ref());
        }

        report_selection_override_reconciliation(
            self.arg.as_ref(),
            &run_results_artifact,
            &resolved_state.nodes,
        );

        // Save artifact for callers
        self.captured_artifacts.run_results = Some(run_results_artifact);

        if self.arg.write_metadata {
            write_runtime_results_parquet(
                &run_task_results.stats.run,
                &run_task_results.adapter_responses,
                self.arg.as_ref(),
            );
        }

        for s in &run_task_results.storeables {
            let path = self.arg.io.out_dir.join(s.out_dir_relpath());
            let mut output = stdfs::File::create(&path).inspect_err(|_| {
                self.captured_artifacts
                    .manifest
                    .get_or_insert_with(|| compilation.take_dbt_manifest());
            })?;
            s.write_results(resolved_state.as_ref(), &mut output)
                .inspect_err(|_| {
                    self.captured_artifacts
                        .manifest
                        .get_or_insert_with(|| compilation.take_dbt_manifest());
                })?;
        }

        self.feature_stack
            .cli
            .hooks
            .did_schedule_and_run_tasks(
                self.arg.as_ref(),
                self.cli.as_ref(),
                compilation.previous_state.as_deref(),
                &run_task_results,
                resolved_state.as_ref(),
                token,
            )
            .await
            .inspect_err(|_| {
                self.captured_artifacts
                    .manifest
                    .get_or_insert_with(|| compilation.take_dbt_manifest());
            })?;

        let mut dbt_manifest = compilation.take_dbt_manifest();
        // update_manifest clones the full ResolverState (~3GB for 6k nodes) to merge
        // compiled SQL + inferred schemas into manifest nodes. Only needed for --write-json.
        // For --write-metadata we keep the Arc and borrow — no clone.
        let resolved_state: Arc<ResolverState> = if self.arg.write_json {
            let schema_store =
                Arc::clone(&compilation_cache_state.schema_store) as Arc<dyn SchemaStoreTrait>;

            let macro_depends_on = self
                .jinja_type_checking_event_listener_factory
                .all_macro_depends_on();

            match update_manifest(
                &run_task_args,
                &type_ops_factory,
                &schema_store,
                resolved_state,
                &macro_depends_on,
                &mut dbt_manifest,
            ) {
                Err(e) => {
                    self.captured_artifacts.manifest.get_or_insert(dbt_manifest);
                    return Err(e);
                }
                Ok(resolved_state) => Arc::new(resolved_state),
            }
        } else {
            resolved_state
        };

        // Save updated manifest, overwriting previous one even if it was there (which would be a coding error really)
        // TODO: above where we do `self.captured_artifacts.manifest.get_or_insert(dbt_manifest)` - this is
        // defensive tactic to avoid reasoning through all possible control flows. Logically it should never be set if
        // we reached here, since only parse pahse, which short-circuits writes manifest early. Maybe worth using
        // debug_aserts! to check for this invariant everwhere we set it in this function...
        self.captured_artifacts.manifest = Some(dbt_manifest);

        // Single warehouse INFORMATION_SCHEMA fetch shared by all catalog consumers.
        // Gated on write_metadata && (write_catalog || write_index): --write-metadata alone
        // must not hit the warehouse. For --write-catalog without --write-metadata, Block 0
        // (write_json path) fetches independently via write_catalog_json — adding write_catalog
        // here would introduce a second DWH round-trip for that path.
        //
        // Skipped entirely for an implied build/run: a plain `dbt build`/`dbt run` never asked
        // for the index or the catalog, so it shouldn't pay for an extra warehouse round-trip,
        // inherit every adapter's catalog-fetch code path, or risk a new --warn-error-eligible
        // failure mode on every invocation. The defaulted index is therefore missing
        // catalog_type/catalog_comment on node_columns and has empty catalog_tables/
        // catalog_stats layers, same as it would be if the fetch failed outright — an explicit
        // --write-index/--write-metadata/--write-catalog still gets the full fetch.
        //
        // NOTE on resolved_state: for --write-index, write_json=true (clap-core forces
        // write_json=false only when self.write_metadata=true AND self.write_index=false;
        // --write-index has self.write_metadata=false raw, so write_json stays true and
        // update_manifest IS called above). This is safe: catalog queries use resolved_state
        // only for relation identity (database/schema/table), not for compiled SQL or inferred
        // schemas added by update_manifest.
        // `--generate-info-schema` deliberately does NOT force a catalog fetch: that
        // stays explicit-only (#13295), so a default/info-schema build never pays the
        // warehouse round-trip. The information schema then carries no catalog_type and
        // `data_type` falls back to inferred_type — pass `--write-catalog` for the full
        // warehouse-backed types.
        let catalog_data: Option<DbtCatalog> = if self.arg.write_metadata
            && (self.arg.write_catalog || self.arg.write_index)
            && !self.arg.write_index_implied
            && matches!(self.arg.command, FsCommand::Run | FsCommand::Build)
        {
            try_fetch_catalog(
                &adapter,
                &resolved_state,
                &run_task_results,
                &compilation,
                &jinja_env,
                self.arg.as_ref(),
            )
            .await
        } else {
            None
        };

        // Produce parquet metadata epoch files (compile/nodes, compile/columns, cll, etc.).
        // Must happen before the manifest is consumed below.
        if self.arg.write_metadata && self.arg.command != FsCommand::Show {
            // Catalog epochs fire whenever catalog_data is Some — catalog_data is non-None
            // only when write_metadata && (write_catalog || write_index) && Run|Build,
            // so the if-let is sufficient; catalog.json is separately gated below.
            if let Some(ref catalog) = catalog_data {
                write_catalog_stats_parquet(catalog, self.arg.as_ref()).await;
                write_catalog_columns_epoch(catalog, self.arg.as_ref());
            }

            let schema_store =
                Arc::clone(&compilation_cache_state.schema_store) as Arc<dyn SchemaStoreTrait>;

            let grain_infos = self
                .feature_stack
                .index
                .hooks
                .lineage_grain_infos(&run_task_results)
                .await?;

            // Classifier propagation results (proprietary hook; empty in OSS).
            // Merged with manifest-declared classifiers inside write_metadata_parquet.
            let (node_classifiers, column_classifiers) = self
                .feature_stack
                .index
                .hooks
                .classifier_results(&run_task_results)
                .await?;

            let recomputed_targets: HashSet<String> = if self.arg.command.compiles_project() {
                run_task_results
                    .stats
                    .compile
                    .stats
                    .iter()
                    .map(|s| s.unique_id.clone())
                    .collect()
            } else {
                HashSet::new()
            };

            if recomputed_targets.is_empty() {
                write_metadata_parquet(
                    self.arg.as_ref(),
                    self.captured_artifacts
                        .manifest
                        .as_ref()
                        .expect("Unconditionally set earlier"),
                    Some(resolved_state.as_ref()),
                    Some(schema_store.as_ref()),
                    None,
                    &recomputed_targets,
                    &grain_infos,
                    &node_classifiers,
                    &column_classifiers,
                );
            } else if !self.arg.write_lineage {
                write_metadata_parquet(
                    self.arg.as_ref(),
                    self.captured_artifacts
                        .manifest
                        .as_ref()
                        .expect("Unconditionally set earlier"),
                    Some(resolved_state.as_ref()),
                    Some(schema_store.as_ref()),
                    Some(&[]),
                    &recomputed_targets,
                    &grain_infos,
                    &node_classifiers,
                    &column_classifiers,
                );
            } else {
                let t_ble = {
                    let timing = std::env::var_os("DBT_LINEAGE_TIMING").is_some();
                    if timing {
                        eprintln!("[lineage] column_lineage hook start");
                    }
                    Instant::now()
                };
                match self
                    .feature_stack
                    .index
                    .hooks
                    .column_lineage(resolved_state.as_ref(), &run_task_results)
                    .await
                {
                    Ok(column_lineage) => {
                        if std::env::var_os("DBT_LINEAGE_TIMING").is_some() {
                            eprintln!(
                                "[lineage] {:>8.1}ms  cll_edges_from_lineage_results ({})",
                                t_ble.elapsed().as_secs_f64() * 1000.0,
                                column_lineage.len()
                            );
                        }
                        if column_lineage.is_empty() {
                            emit_warn_log_message(
                                ErrorCode::Generic,
                                "column-level lineage requires --static-analysis strict; no column lineage written.",
                            );
                        }
                        write_metadata_parquet(
                            self.arg.as_ref(),
                            self.captured_artifacts
                                .manifest
                                .as_ref()
                                .expect("Unconditionally set earlier"),
                            Some(resolved_state.as_ref()),
                            Some(schema_store.as_ref()),
                            Some(&column_lineage),
                            &recomputed_targets,
                            &grain_infos,
                            &node_classifiers,
                            &column_classifiers,
                        );
                    }
                    Err(e) => {
                        emit_warn_log_message(
                            ErrorCode::Generic,
                            format!("dbt-index: column_lineage: {e}"),
                        );
                        let empty_targets: HashSet<String> = HashSet::new();
                        write_metadata_parquet(
                            self.arg.as_ref(),
                            self.captured_artifacts
                                .manifest
                                .as_ref()
                                .expect("Unconditionally set earlier"),
                            Some(resolved_state.as_ref()),
                            Some(schema_store.as_ref()),
                            Some(&[]),
                            &empty_targets,
                            &grain_infos,
                            &node_classifiers,
                            &column_classifiers,
                        );
                    }
                }
            }

            // Write catalog.json from pre-fetched catalog — no second warehouse query.
            // Epochs already written unconditionally above; this block is catalog.json only.
            // Only for Run/Build: need executed nodes to populate relations.
            if self.arg.write_catalog
                && matches!(self.arg.command, FsCommand::Run | FsCommand::Build)
            {
                if let Some(ref catalog) = catalog_data {
                    match write_artifact_to_file(
                        catalog,
                        ArtifactType::Catalog,
                        &self.arg.io.out_dir,
                        DBT_CATALOG_JSON,
                        &self.arg.io.in_dir,
                    ) {
                        Ok(()) => {
                            emit_info_log_message("Successfully wrote catalog.json");
                        }
                        Err(e) => {
                            emit_warn_log_message(ErrorCode::Generic, format!("catalog: {e}"));
                        }
                    }
                }
            }

            // Save catalog
            self.captured_artifacts.catalog = catalog_data;

            // When --write-index is active, convert metadata epochs → snapshot index parquet.
            if self.arg.write_index {
                let metadata_dir = self.arg.metadata_dir();
                let index_dir = self.arg.index_dir();
                let mut state = IngestState::default();
                match ingest_from_metadata_direct(&metadata_dir, &index_dir, &mut state) {
                    Ok(_) => {
                        if let Err(e) = save_artifact_meta(
                            &index_dir,
                            &self.arg.io.out_dir,
                            WriteSource::DirectWrite,
                            None,
                        ) {
                            emit_warn_log_message(
                                ErrorCode::IndexWriteFailed,
                                format!("dbt-index: save_artifact_meta: {e}"),
                            );
                        }
                    }
                    Err(e) => emit_warn_log_message(
                        ErrorCode::IndexWriteFailed,
                        format!("dbt-index: write-index: {e}"),
                    ),
                }

                // Post-index hook: ingest the classifier registry and run the
                // classifier "checks" gate. No-op in OSS.
                //
                // Recorded rather than propagated: a failing index write should not
                // abort a build whose models already succeeded.
                if let Err(e) = self
                    .feature_stack
                    .index
                    .hooks
                    .did_write_index(
                        self.arg.as_ref(),
                        &index_dir,
                        &run_task_results,
                        resolved_state.as_ref(),
                    )
                    .await
                {
                    emit_error_log_from_fs_error(*e);
                }
            }

            // The information schema is written independently of the index:
            // either, neither, or both may be requested. Its intermediate is the
            // flat index at `target/private/index` when one is present — the same ingest
            // builds both, so an index written by the block just above (or by a
            // prior run) is reused via the delta path rather than re-ingested. With
            // no index to reuse — e.g. `--no-write-index` — it stages privately, so
            // requesting the information schema never materialises an index the
            // caller opted out of.
            if self.arg.generate_info_schema {
                let metadata_dir = self.arg.metadata_dir();
                let info_schema_dir = self.arg.info_schema_dir();
                let index_dir = self.arg.index_dir();
                let staging_dir = if has_persisted_state(&index_dir) {
                    index_dir
                } else {
                    self.arg.info_schema_staging_dir()
                };
                if let Err(e) = write_info_schema(&metadata_dir, &info_schema_dir, &staging_dir) {
                    emit_warn_log_message(
                        ErrorCode::InfoSchemaWriteFailed,
                        format!("dbt: generate-info-schema: {e}"),
                    );
                }
            }
        }

        let map_compiled_sql = self
            .captured_artifacts
            .manifest
            .as_ref()
            .expect("Unconditionally set earlier")
            .into_map_compiled_sql();

        if self.arg.io.should_show(ShowOptions::Stats) {
            emit_info_event(
                ShowResult::new_text(
                    run_task_results.stats.compile.to_string(),
                    "stats",
                    "Compile time stats",
                ),
                None,
            );
        }

        if let Command::Core(Compile(CompileArgs {
            inline: Some(_), ..
        })) = &self.cli.command
        {
            return self.run_inline_compile(&resolved_state).await;
        }

        self.emit_selected_compile_output(&resolved_state, &schedule, &map_compiled_sql)?;

        let schema_store =
            Arc::clone(&compilation_cache_state.schema_store) as Arc<dyn SchemaStoreTrait>;
        let data_store = Arc::clone(&compilation_cache_state.data_store) as Arc<dyn DataStoreTrait>;
        self.feature_stack
            .cli
            .hooks
            .did_emit_selected_compile_output(
                self.arg.as_ref(),
                &resolved_state,
                &jinja_env,
                run_task_results.task_runner_ctx.as_ref(),
                &schema_store,
                &data_store,
                &map_compiled_sql,
                &self.feature_stack,
                token,
            )
            .await?;

        // Phase-only checkpoint at Compile: exit if the requested phase ends
        // at or before Compile, but otherwise continue regardless of the
        // current error count. (We deliberately do NOT consult the error
        // counter here — that's what `checkpoint_maybe_exit` does for phase
        // boundaries that are themselves the unit of work. Subsequent steps,
        // including runtime stats output, still need to run on test/run
        // failures.)
        if !self.arg.skip_checkpoints && self.arg.phase <= Phases::Compile {
            return Err(return_exit_code_from_error_counter());
        }

        self.feature_stack
            .cli
            .hooks
            .did_compile(
                self.arg.as_ref(),
                self.cli.as_ref(),
                &resolved_state,
                &schedule,
                token,
            )
            .await?;

        for showable in &run_task_results.showables {
            showable.show(self.arg.as_ref(), &resolved_state, &schedule, token)?;
        }

        if !self.arg.skip_checkpoints && self.arg.phase <= Phases::Lineage {
            return Err(return_exit_code_from_error_counter());
        }

        assert!(self.arg.phase == Phases::All);

        let should_exit_with_warning = self.should_exit_with_run_result_warning();

        // Write run_results.json
        if self.arg.write_json {
            let base_context = build_base_context(&resolved_state, &jinja_env);
            let refetched_catalog = self
                .write_json(
                    &run_task_results,
                    &compilation,
                    &jinja_env,
                    &base_context,
                    &resolved_state,
                    &adapter,
                )
                .await?;
            if refetched_catalog.is_some() {
                self.captured_artifacts.catalog = refetched_catalog;
            }

            if matches!(self.arg.command, FsCommand::Run | FsCommand::Build) {
                upload_artifacts_ingest_if_enabled(
                    &compilation.dbt_cloud_config().cloned(),
                    &self.arg.io,
                    self.arg.write_catalog,
                )
                .await?;
            }
        }
        if self.arg.io.should_show(ShowOptions::Stats) {
            emit_info_event(
                ShowResult::new_text(
                    run_task_results.stats.run.to_string(),
                    "stats",
                    "Runtime stats",
                ),
                None,
            );
        }

        match error_count_checkpoint() {
            Ok(()) if should_exit_with_warning => Err(FsError::exit_with_status(2)),
            result => result,
        }
    }
}

/// `dbt docs generate` — compile the project, then write a statically hostable site.
///
/// Runs `compile --write-index` ([`build_index_for_docs`]) and turns the index that
/// produces into a directory of files any host can serve. The compile is unconditional,
/// exactly as in v1: `--no-compile` is how a user says they want the index that is
/// already on disk. Inferring that from whether an index happens to exist looked like a
/// saving and behaved like a bug — the second `docs generate` of the day silently
/// published whatever the first one had compiled.
///
/// The compile is a synthesized invocation handed to the ordinary pipeline, not a
/// docs-aware code path in the pipeline: the phase pipeline branches on
/// `FsCommand::Compile | Build | Run` in roughly two dozen places, and threading a
/// docs command through every one of them was tried once and abandoned.
async fn run_docs_generate(
    generate_args: dbt_clap_core::DocsGenerateArgs,
    eval_arg: &EvalArgs,
    cli: &Cli,
    feature_stack: Arc<FeatureStack>,
    token: &CancellationToken,
) -> FsResult<()> {
    // `docs generate` is a project command, so `out_dir` is the project's target
    // directory, resolved the standard way: `--project-dir` or discovery, then
    // `--target-path` or `dbt_project.yml`'s. The subcommand's own `--target-path`
    // stays as an explicit override, and `--index-dir` / `--metadata-dir` still win
    // over both, as they do everywhere else.
    //
    // A relative override resolves against the project directory, which is what
    // `in_out_dir` does for `--target-path` everywhere else. Resolving it against the
    // working directory instead would put the index the compile writes and the index
    // the export reads in two different places whenever `--project-dir` is not `.`.
    let target_dir = generate_args
        .target_path
        .clone()
        .map(|path| {
            if path.is_relative() {
                eval_arg.io.in_dir.join(path)
            } else {
                path
            }
        })
        .unwrap_or_else(|| eval_arg.io.out_dir.clone());

    let index_dir = eval_arg
        .index_dir
        .clone()
        .unwrap_or_else(|| default_index_dir(&target_dir));
    // The site's data directory. Public under `target/`, unlike the index, because
    // the browser fetches it over HTTP. An explicit `--info-schema-dir` still wins,
    // for the same reason `--index-dir` does.
    let info_schema_root = eval_arg
        .info_schema_dir
        .clone()
        .unwrap_or_else(|| target_dir.join(dbt_common::constants::DBT_INFO_SCHEMA_DIR_NAME));
    let info_schema_dir = dbt_index_core::versioned_dir(&info_schema_root);
    let output_dir = generate_args
        .output_dir
        .clone()
        .unwrap_or_else(|| target_dir.clone());

    // Roll any newer metadata epochs into the index, the same opportunistic
    // catch-up `docs serve` does, so a run since the last `--write-index` is picked up.
    // Under `--no-compile` this is the only thing that can advance the index; otherwise
    // `build_index_for_docs` runs it again once its compile has finished.
    let metadata_dir = eval_arg
        .metadata_dir
        .clone()
        .unwrap_or_else(|| default_metadata_dir(&target_dir));
    ingest_metadata_into_index(&metadata_dir, &index_dir, "dbt docs generate");

    // Compile unless the user said not to, the way v1 does. `--no-compile` falls through
    // to the export, and to the error below when there is nothing to export.
    if !generate_args.no_compile {
        emit_info_log_message(
            "Running compile; pass `--no-compile` to export the existing index instead.",
        );
        build_index_for_docs(
            &target_dir,
            &index_dir,
            &metadata_dir,
            eval_arg,
            cli,
            Arc::clone(&feature_stack),
            token,
        )
        .await?;
    }

    // Build the information schema last, from whatever the metadata directory now
    // holds. It has to be here rather than left to the compile's own
    // `--generate-info-schema`: the invocation record is written *after* that point,
    // so an artifact set built during the compile would miss it and the site's
    // timings and status surfaces would come up empty — the same reason the ingest
    // above runs twice. Under `--no-compile` this is the only thing that advances it.
    build_info_schema_for_docs(&metadata_dir, &index_dir, &info_schema_root);

    // Nothing to export. Under `--no-compile` that is the expected way to get here;
    // otherwise the compile above ran but wrote nothing. Checked before opening the
    // backend so the user gets the export's message — which names both commands that
    // produce the artifacts — rather than the backend's generic "does not exist".
    if !dbt_docs_server::has_artifacts(&info_schema_dir) {
        emit_error_log_message(
            ErrorCode::Generic,
            format!(
                "dbt docs generate: {}",
                dbt_docs_server::ExportError::NoIndex { info_schema_dir }
            ),
        );
        return Err(FsError::exit_with_status(1));
    }

    let backend: Arc<dyn Backend> =
        Arc::new(match DuckDbInfoSchemaBackend::open(&info_schema_dir) {
            Ok(backend) => backend,
            Err(err) => {
                emit_error_log_message(ErrorCode::Generic, format!("dbt docs generate: {err}"));
                return Err(FsError::exit_with_status(1));
            }
        });
    let providers = (feature_stack.index.providers_factory)(backend);

    let project_dir = &eval_arg.io.in_dir;
    let options = dbt_docs_server::ExportOptions {
        info_schema_dir,
        output_dir,
        duckdb_cdn_base: generate_args.duckdb_cdn_base,
        // Consent is resolved here because the project and profile are only
        // readable on this machine; the browser reads the answer, not the inputs.
        analytics_enabled: std::env::var("DO_NOT_TRACK").as_deref() != Ok("1")
            && cli
                .common_args()
                .get_send_anonymous_usage_stats_for_project(project_dir),
    };

    let summary = match dbt_docs_server::export_site(&providers, &options) {
        Ok(summary) => summary,
        Err(err) => {
            emit_error_log_message(ErrorCode::Generic, format!("dbt docs generate: {err}"));
            return Err(FsError::exit_with_status(1));
        }
    };

    emit_info_progress_message(ProgressMessage::new_from_action_and_target(
        "Generated".to_string(),
        format!(
            "{}{}{}",
            summary.output_dir.display(),
            if summary.copied_artifacts > 0 {
                format!(" ({} artifacts copied)", summary.copied_artifacts)
            } else {
                // The common case: the site reads the index where it already lies.
                format!(" (reading {})", summary.data_dir)
            },
            if summary.has_column_lineage {
                ""
            } else {
                " — no column lineage; rerun with `--static-analysis strict` to include it"
            },
        ),
    ));
    // The caveat only applies when the site was written into the target directory,
    // which holds plenty besides the site. An explicit `--output-dir` gets a fresh
    // directory containing nothing else.
    let wrote_into_target = summary.output_dir == target_dir;
    emit_info_log_message(format!(
        "Host the contents of {} on any static file server.{}",
        summary.output_dir.display(),
        if wrote_into_target {
            " Note that this publishes everything else in the target directory too, \
             including compiled SQL and any stored test failures."
        } else {
            ""
        },
    ));

    Ok(())
}

/// Roll metadata epochs newer than the index into it, best-effort.
///
/// A missing metadata directory is nothing to do, and a failed ingest is worth a
/// warning but not the command: whatever the index already holds is still exportable.
fn ingest_metadata_into_index(
    metadata_dir: &std::path::Path,
    index_dir: &std::path::Path,
    context: &str,
) {
    if !metadata_dir.exists() {
        return;
    }
    let mut state = IngestState::default();
    if let Err(err) = apply_delta_direct(metadata_dir, index_dir, &mut state) {
        emit_warn_log_message(
            ErrorCode::Generic,
            format!("{context}: failed to ingest metadata: {err}"),
        );
    }
}

/// Build the information schema the site reads, best-effort.
///
/// Mirrors [`ingest_metadata_into_index`]'s contract: a missing metadata directory is
/// nothing to do, and a failure is worth a warning but not the command — whatever is
/// already on disk is still exportable, and the export's own emptiness check is what
/// decides whether the site can be written.
///
/// The flat index is reused as the intermediate when one is present, which it is
/// whenever a compile has run: the same ingest builds both, so this takes the delta
/// path instead of re-reading every epoch. Without one it stages privately.
fn build_info_schema_for_docs(
    metadata_dir: &std::path::Path,
    index_dir: &std::path::Path,
    info_schema_root: &std::path::Path,
) {
    if !metadata_dir.exists() {
        return;
    }
    let staging = if has_persisted_state(index_dir) {
        index_dir.to_path_buf()
    } else {
        info_schema_root.with_file_name(dbt_common::constants::DBT_INFO_SCHEMA_STAGING_DIR_NAME)
    };
    if let Err(err) = write_info_schema(metadata_dir, info_schema_root, &staging) {
        emit_warn_log_message(
            ErrorCode::InfoSchemaWriteFailed,
            format!("dbt docs: could not build the information schema: {err}"),
        );
    }
}

/// Run `compile --write-index` in-process so `docs generate` has an index to export.
///
/// Synthesizes the compile's own args and hands them to the ordinary pipeline, which
/// is how `dbt retry` reaches the command it is retrying (`prepare_for_potential_retry`)
/// and how `dbt-repl` runs its bootstrap compile. `EvalArgs::command` therefore reads
/// `Compile` — the pipeline branches on it in roughly two dozen places, and anything
/// else silently produces an empty index — while `command_entrypoint` keeps `Docs` as
/// the origin, the same way the LSP labels its internal compiles.
async fn build_index_for_docs(
    target_dir: &std::path::Path,
    index_dir: &std::path::Path,
    metadata_dir: &std::path::Path,
    docs_arg: &EvalArgs,
    docs_cli: &Cli,
    feature_stack: Arc<FeatureStack>,
    token: &CancellationToken,
) -> FsResult<()> {
    let mut common_args = docs_cli.common_args();
    // `write_index` is the only flag to set — `CommonArgs::to_eval_args` derives
    // `write_metadata` from it — and deliberately the only one. Static analysis is left
    // at its default, so this is a plain `compile --write-index` and nothing more: an
    // index built here is exactly what the two-command flow produces, no better and no
    // worse. That means no column lineage, which needs `--static-analysis strict`; the
    // export says so, naming the compile that would include it.
    common_args.write_index = true;
    // Pin the directories `run_docs_generate` resolved, so the compile writes exactly
    // where the export reads. `docs generate --target-path` is a subcommand-level
    // override that does not exist in `CommonArgs`, so without this the compile would
    // write into the project's default target directory instead.
    common_args.target_path = Some(target_dir.to_path_buf());
    common_args.index_dir = Some(index_dir.to_path_buf());
    common_args.metadata_dir = Some(metadata_dir.to_path_buf());
    // `generate_info_schema` carries over from the docs invocation on its own,
    // being a global flag; pin its directory too so it lands beside the index
    // rather than in the project's default target directory. An explicit
    // `--info-schema-dir` still wins.
    if common_args.generate_info_schema && common_args.info_schema_dir.is_none() {
        common_args.info_schema_dir =
            Some(target_dir.join(dbt_common::constants::DBT_INFO_SCHEMA_DIR_NAME));
    }

    let compile_cli = Cli {
        command: Command::Core(CoreCommand::Compile(CompileArgs {
            common_args: common_args.clone(),
            ..CompileArgs::default()
        })),
        common_args,
    };

    // Built from the docs invocation rather than `from_main`, so an embedder is not
    // told this came from the binary, and so the compile shares the docs invocation id
    // and log configuration. `to_eval_args` overwrites `in_dir` / `out_dir`, and
    // `exit_process_on_panic` is not one of the fields it reads.
    let system_args = SystemArgs {
        command: FsCommand::Compile,
        io: docs_arg.io.clone(),
        from_main: docs_arg.from_main,
        exit_process_on_panic: false,
        num_threads: docs_arg.num_threads,
        no_parallel: docs_arg.no_parallel,
        target: docs_arg.target.clone(),
    };
    let mut compile_arg = compile_cli.to_eval_args(system_args)?;
    compile_arg.command_entrypoint = FsCommand::Docs;

    // Started before the run so the recorded duration is the compile's.
    let invocation_ctx = InvocationContext::new(
        compile_arg.metadata_dir(),
        &compile_arg.io,
        FsCommand::Compile,
        &compile_cli.common_args(),
    );

    let hooks_factory = Arc::clone(&feature_stack.task_runner.hooks_factory);
    // Boxed: this nests the whole phase pipeline's future inside the one `do_execute_fs`
    // already returns, and the combined layout overflows rustc's query depth limit in
    // downstream crates. `Box::pin` keeps the inner future off the outer one's layout.
    let result = Box::pin(execute_setup_and_all_phases(
        &compile_arg,
        &compile_cli,
        &mut DbtCommandExecutionArtifacts::default(),
        feature_stack,
        hooks_factory,
        token,
    ))
    .await;

    // Compile ends at the `Phases::Compile` checkpoint, which reports success as
    // `Err` carrying exit status 0 — a sentinel, not a failure. Unwrapped the same way
    // `setup_and_execute_fs` does for the top-level invocation.
    let failure = match result {
        Ok(()) => None,
        Err(err) if err.exit_status() == Some(0) => None,
        Err(err) => Some(err),
    };
    invocation_ctx.write(if failure.is_some() {
        "error"
    } else {
        "success"
    });
    if let Some(err) = failure {
        return Err(err);
    }

    // The invocation record is written after the compile's own `--write-index` ingest,
    // so without this second pass it never reaches `dbt_rt.invocations` and the site's
    // timings and status surfaces come up empty.
    ingest_metadata_into_index(metadata_dir, index_dir, "dbt docs generate");

    Ok(())
}

async fn run_docs_serve(
    serve_args: ClapDocsServeArgs,
    feature_stack: &Arc<FeatureStack>,
    project_dir: &std::path::Path,
    common_args: &dbt_clap_core::CommonArgs,
) -> FsResult<()> {
    let has_dbt_state = common_args.get_manage_state(project_dir, false);
    let send_anonymous_usage_stats =
        common_args.get_send_anonymous_usage_stats_for_project(project_dir);
    let args = dbt_docs_server::DocsServeArgs {
        target_path: serve_args.target_path,
        host: serve_args.host,
        port: serve_args.port,
        no_open: serve_args.no_open,
        has_dbt_state,
        send_anonymous_usage_stats,
        // Filled in below, once the site has been generated.
        site_dir: None,
    };
    let info_schema_dir = dbt_docs_server::resolve_info_schema_dir(&args);

    let target = args
        .target_path
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("./target"));
    let metadata_dir = default_metadata_dir(&target);
    let index_dir = default_index_dir(&target);
    let info_schema_root = target.join(dbt_common::constants::DBT_INFO_SCHEMA_DIR_NAME);

    if !info_schema_dir.exists() && !metadata_dir.exists() {
        emit_error_log_message(
            ErrorCode::Generic,
            format!(
                "dbt docs serve: no data to serve\n\n\
                 Information schema not found: {}\n\
                 Run `dbt build` or `dbt docs generate` to write it,\n\
                 or pass `--target-path <DIR>` pointing at a directory that contains it.",
                info_schema_dir.display(),
            ),
        );
        return Err(FsError::exit_with_status(1));
    }

    // Opportunistic catch-up, the same one `docs generate` does: roll any newer
    // metadata epochs into the index and rebuild the information schema from them,
    // so a run since the last `docs generate` is picked up without one.
    ingest_metadata_into_index(&metadata_dir, &index_dir, "dbt docs serve");
    build_info_schema_for_docs(&metadata_dir, &index_dir, &info_schema_root);

    let backend: Arc<dyn Backend> =
        Arc::new(match DuckDbInfoSchemaBackend::open(&info_schema_dir) {
            Ok(b) => b,
            Err(err) => {
                emit_error_log_message(ErrorCode::Generic, format!("dbt docs serve: {err}"));
                return Err(FsError::exit_with_status(1));
            }
        });
    let providers = (feature_stack.index.providers_factory)(backend);

    // Serve the same static site `docs generate` produces rather than the
    // embedded bundle, so local preview exercises the artifact the user will
    // actually host. Regenerated when missing or older than the index; a failure
    // here is not fatal, because the embedded bundle is still a usable fallback.
    let site_dir = target.clone();
    if site_needs_regenerating(&site_dir, &info_schema_dir) {
        let options = dbt_docs_server::ExportOptions {
            info_schema_dir: info_schema_dir.clone(),
            output_dir: site_dir.clone(),
            duckdb_cdn_base: None,
            analytics_enabled: std::env::var("DO_NOT_TRACK").as_deref() != Ok("1")
                && send_anonymous_usage_stats,
        };
        match dbt_docs_server::export_site(&providers, &options) {
            Ok(summary) => eprintln!(
                "dbt docs serve: generated {} (reading {})",
                summary.output_dir.display(),
                summary.data_dir
            ),
            Err(err) => emit_warn_log_message(
                ErrorCode::Generic,
                format!("dbt docs serve: could not generate the site: {err}"),
            ),
        }
    }

    let args = dbt_docs_server::DocsServeArgs {
        site_dir: site_dir.exists().then_some(site_dir),
        ..args
    };

    dbt_docs_server::run_with_args(Arc::new(args), providers)
        .await
        .map_err(|err| {
            emit_error_log_message(ErrorCode::Generic, err.to_string());
            FsError::exit_with_status(1)
        })
}

/// Whether `site_dir` is missing or predates the data it was built from.
///
/// Compares against the data directory's mtime, the same staleness signal
/// `AppState::compute_generation` reports to the UI. Any unreadable timestamp
/// means regenerate: cheap, and being wrong the other way serves stale docs.
fn site_needs_regenerating(site_dir: &std::path::Path, data_dir: &std::path::Path) -> bool {
    let Ok(site_mtime) = std::fs::metadata(site_dir.join("index.html")).and_then(|m| m.modified())
    else {
        return true;
    };
    let Ok(index_mtime) = std::fs::metadata(data_dir).and_then(|m| m.modified()) else {
        return true;
    };
    site_mtime < index_mtime
}

#[allow(clippy::cognitive_complexity)]
pub fn check_options(cli: &Cli) {
    let common_args = cli.common_args();

    if common_args.no_debug {
        emit_warn_log_message(
            ErrorCode::NotYetSupportedOption,
            "--no-debug is no longer supported",
        );
    }
    if common_args.cache_selected_only || common_args.no_cache_selected_only {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--cache-selected is no longer supported",
        );
    }

    if common_args.skip_write_msgpack_if_exist {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--skip-write-msgpack-if-exist is no longer supported",
        );
    }

    if common_args.log_cache_events || common_args.no_log_cache_events {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--log-cache-events is no longer supported",
        );
    }
    if common_args.macro_debugging || common_args.no_macro_debugging {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--macro-debugging is no longer supported",
        );
    }

    if common_args.partial_parse_file_diff || common_args.no_partial_parse_file_diff {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--partial-parse-file-diff is no longer supported",
        );
    }
    if common_args.partial_parse_file_path.is_some() {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--partial-parse-file-path is no longer supported",
        );
    }
    if common_args.populate_cache || common_args.no_populate_cache {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--populate-cache is no longer supported",
        );
    }
    if common_args.print || common_args.no_print {
        emit_warn_log_message(
            ErrorCode::NotYetSupportedOption,
            "--print is not supported yet",
        );
    }
    if common_args.printer_width != 120 {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--printer-width is no longer supported",
        );
    }
    if common_args.record_timing_info.is_some() {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--record-timing-info is no longer supported",
        );
    }
    if common_args.static_parser || common_args.no_static_parser {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--static_parser is no longer supported",
        );
    }
    if common_args.use_colors || common_args.no_use_colors {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--use-colors is no longer supported; use the FORCE_COLOR or NO_COLOR environment variables instead",
        );
    }
    if common_args.use_colors_file || common_args.no_use_colors_file {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--use-colors-file is no longer supported",
        );
    }
    if common_args.use_experimental_parser || common_args.no_use_experimental_parser {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--use-experimental-parser is no longer supported",
        );
    }
    if common_args.use_fast_test_edges || common_args.no_use_fast_test_edges {
        emit_warn_log_message(
            ErrorCode::NoLongerSupportedOption,
            "--use-fast-test-edges is no longer supported",
        );
    }
}

/// Fetches catalog data at most once, returning None if the adapter has no
/// metadata support, no executed relations, or the fetch fails (warn-logged).
/// Callers should treat None as "skip all catalog writes".
async fn try_fetch_catalog(
    adapter: &Arc<Adapter>,
    resolved_state: &Arc<ResolverState>,
    run_task_results: &RunTaskResults,
    compilation: &DbtProjectCompilation,
    jinja_env: &Arc<JinjaEnv>,
    arg: &EvalArgs,
) -> Option<DbtCatalog> {
    let metadata_adapter = adapter.metadata_adapter()?;
    let relations = metadata_adapter
        .create_relations_from_executed_nodes(resolved_state, &run_task_results.stats.run);
    if relations.is_empty() {
        return None;
    }
    let base_context = build_base_context(resolved_state, jinja_env);
    match fetch_catalog_data(
        adapter,
        resolved_state,
        relations,
        jinja_env,
        compilation.root_project_name(),
        &base_context,
        arg,
        20,
    )
    .await
    {
        Ok(catalog) => Some(catalog),
        Err(e) => {
            // Only reached for an explicit --write-index/--write-metadata/--write-catalog
            // (an implied build/run never calls this at all, see the call site), so a failed
            // fetch is worth a warning --warn-error can see, not a build-ending error: the
            // caller asked for a nice-to-have index/catalog enrichment, not for the fetch
            // itself to be load-bearing.
            emit_warn_log_message(
                ErrorCode::Generic,
                format!("Failed to fetch catalog data: {e}"),
            );
            None
        }
    }
}

/// Fetches catalog data from the warehouse without writing any artifact.
/// Returns the populated `DbtCatalog` (nodes keyed by unique_id with stats).
///
/// Extracted from `write_catalog_json` so both the JSON path and the parquet
/// metadata epoch path can share the same warehouse query logic.
#[allow(clippy::too_many_arguments)]
async fn fetch_catalog_data(
    adapter: &Arc<Adapter>,
    resolved_state: &ResolverState,
    relations: Vec<Arc<dyn BaseRelation>>,
    jinja_env: &JinjaEnv,
    project_name: &str,
    context: &BTreeMap<String, Value>,
    arg: &EvalArgs,
    batches: usize,
) -> FsResult<DbtCatalog> {
    // Only reached for an explicit --write-index/--write-metadata/--write-catalog (an implied
    // build/run skips catalog fetch entirely, see the call site), so progress/failure reporting
    // here is expected feedback, not noise.
    emit_info_log_message("Fetching catalog from warehouse");
    let metadata_adapter = adapter
        .metadata_adapter()
        .expect("Expected implements MetadataAdapter");
    let maybe_region = (adapter.adapter_type() == AdapterType::Bigquery).then(|| {
        adapter
            .engine()
            .config("location")
            .map_or_else(|| "us".to_string(), |cfg| cfg.to_lowercase())
    });
    // Build relations_by_schema map and task queue for worker pool
    let mut relations_by_schema = BTreeMap::new();
    let mut tasks = Vec::new();

    for rel in relations {
        let database = rel.database().unwrap_or_default();
        let schema = rel.schema().unwrap_or_default().to_string();
        let key = (database.to_string(), schema.clone());

        relations_by_schema
            .entry(key.clone())
            .or_insert_with(Vec::new)
            .push(rel);

        // Add (database, schema) task if not already present
        if !tasks.contains(&key) {
            tasks.push(key);
        }
    }

    // Create shared, thread-safe structures for worker coordination
    let task_queue = Arc::new(Mutex::new(tasks));
    let relations_map = Arc::new(relations_by_schema);

    // Shared collection for macro results
    let shared_results: Arc<Mutex<Vec<Arc<arrow::array::RecordBatch>>>> =
        Arc::new(Mutex::new(Vec::new()));

    // Shared collection for catalog errors
    let shared_errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // Progress tracker: worker_id -> (schema_name, task_start_time)
    // Entries are updated whenever a worker picks up a task
    // Entries are removed when a worker has no remaining work and exits
    let progress_tracker: Arc<Mutex<HashMap<usize, (String, Instant)>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Timeout threshold for detecting hung workers (1 minute)
    const WORKER_TIMEOUT: Duration = Duration::from_secs(60);
    // Polling interval for checking worker progress
    const POLL_INTERVAL: Duration = Duration::from_millis(500);

    let mut node_stats_and_stuff = BTreeMap::new();
    let mut node_columns = BTreeMap::new();

    // Spawn worker pool
    let concurrency = batches.max(1); // this means max of (1 or batches)
    let mut handles = Vec::new();

    // An `Id`, not a `Span`: holds no refcount, so it cannot keep the invocation alive.
    // Only used for the `follows_from` link below; a stale `Id` is a no-op there.
    let invocation_span_id = tracing::Span::current().id();

    for worker_id in 0..concurrency {
        let task_queue_clone = task_queue.clone();
        let relations_map_clone = relations_map.clone();
        // Clone the JinjaEnv for each worker to create isolated execution environment
        let jinja_env_clone = jinja_env.clone();
        let adapter_type = resolved_state.adapter_type;
        let maybe_region_clone = maybe_region.clone();
        let project_name_owned = project_name.to_string();
        // Clone the base context - we'll create fresh ResultStore for each schema iteration
        let base_context = context.clone();
        // Clone shared structures for progress tracking, result collection, and error accumulation
        let shared_results_clone = shared_results.clone();
        let shared_errors_clone = shared_errors.clone();
        let progress_tracker_clone = progress_tracker.clone();
        let invocation_span_id_clone = invocation_span_id.clone();

        // `Handle::spawn_blocking`, not the free `dbt_runtime::spawn_blocking`,
        // and deliberately NOT `Span::current().enter()`ed inside the worker.
        // The free function clones and enters the current span for the task's
        // whole life; the `Handle` method captures none. The poll loop below
        // abandons workers that blow past `WORKER_TIMEOUT` without joining them,
        // so a worker can outlive this function. A live worker holding a handle
        // to the invocation span (or any of its descendants) keeps that span's
        // refcount above zero, so the subscriber never fires `on_close` for it
        // -- and the end-of-invocation Execution Summary, which is emitted from
        // `handle_invocation_end`, is silently dropped. Users saw a
        // `--write-index`/`--write-catalog` build print "Fetched partial
        // catalog.json results" and then simply stop, with no summary and no
        // result counts (dbt-labs/fs#14424). Abandonment has to be total: these
        // workers must not participate in the invocation's span lifetime.
        //
        // Dropping a blocking `JoinHandle` detaches rather than cancels, so
        // abandonment still works. The pool's threads are built with
        // `FS_DEFAULT_STACK_SIZE` (8 MiB, `main_impl.rs`), which is the stack
        // these workers used to ask for themselves.
        //
        // ############################################################################
        // # DO NOT COPY THIS PATTERN.                                                #
        // #                                                                          #
        // # CLI code running outside the invocation span is NOT allowed -- a lot of  #
        // # things depend on that invariant. These workers are a deliberate, short-  #
        // # term exception, tolerated only because an abandoned one would otherwise  #
        // # pin the invocation span open (dbt-labs/fs#14424).                        #
        // #                                                                          #
        // # TODO(dbt-labs/fs#14598): once dbt-tracing can force a root span to shut  #
        // # down, put these workers back under the invocation and delete the         #
        // # independent root below.                                                  #
        // ############################################################################
        //
        // A root rather than nothing at all: with no span context, the adapter's first
        // span becomes an unnamed root and trips the data layer's `debug_assert!`, which
        // panics the worker in debug builds.
        let handle = dbt_runtime::Handle::current().spawn_blocking(move || -> FsResult<()> {
            // Worker loop: process tasks until queue is empty
            loop {
                let task = task_queue_clone.lock().unwrap().pop();
                let Some((database, schema)) = task else {
                    // No available work - remove from progress tracker and exit
                    progress_tracker_clone.lock().unwrap().remove(&worker_id);
                    break;
                };
                let schema_clone = schema.clone();

                // Update progress tracker with current schema and timestamp
                progress_tracker_clone
                    .lock()
                    .unwrap()
                    .insert(worker_id, (schema_clone.clone(), Instant::now()));

                // CRITICAL: Create a fresh ResultStore for EACH schema iteration to ensure
                // complete isolation. The `run_query` macro uses a hardcoded name "run_query_statement"
                // for store_result/load_result. By creating a fresh ResultStore for each schema,
                // we ensure that:
                // 1. No state leaks between different schemas processed by the same worker
                // 2. No possibility of race conditions with other workers
                // 3. Each macro invocation has a completely clean ResultStore
                let iteration_result_store = ResultStore::default();
                let mut iteration_context = base_context.clone();
                iteration_context.insert(
                    "store_result".to_owned(),
                    Value::from_function(iteration_result_store.store_result()),
                );
                iteration_context.insert(
                    "load_result".to_owned(),
                    Value::from_function(iteration_result_store.load_result()),
                );
                iteration_context.insert(
                    "store_raw_result".to_owned(),
                    Value::from_function(iteration_result_store.store_raw_result()),
                );

                // Lookup relations for this schema
                let rels = relations_map_clone
                    .get(&(database.clone(), schema.clone()))
                    .expect("schema must exist in relations map");
                let relation_as_values = rels
                    .iter()
                    .map(|r| RelationObject::new(Arc::clone(r)).into_value())
                    .collect::<Vec<Value>>();

                let db_schema = RelationObject::new(Arc::from(
                    create_relation(
                        adapter_type,
                        database.to_string(),
                        schema.clone(),
                        maybe_region_clone.clone(), // hack for BQ
                        None,
                        ResolvedQuoting::default(),
                    )?,
                ))
                .into_value();

                // Independent root -- see the DO NOT COPY banner on the spawn above.
                // Dropped at the end of this iteration so tasks do not nest in each other.
                let catalog_fetch_span = create_root_info_span(GenericOpExecuted::new(
                    "catalog_fetch".to_string(),
                    "fetching catalog".to_string(),
                    Some(relation_as_values.len() as u64),
                ));
                // A link, not a parent: keeps the span correlatable to the run without
                // pinning it open.
                catalog_fetch_span.follows_from(invocation_span_id_clone.clone());
                let _catalog_fetch_span_guard = catalog_fetch_span.clone().entered();

                // To avoid blowing up the query, we use the get_catalog macro for batches with more than 50 relations
                let jinja_result: FsResult<Value> = if relation_as_values.len() > 50 {
                    let args = vec![
                        Value::from_serialize(db_schema),
                        Value::from_serialize(vec![schema.clone()]),
                    ];
                    get_catalog_by_relations(
                        &jinja_env_clone,
                        "get_catalog",
                        &project_name_owned,
                        &project_name_owned,
                        &iteration_context,
                        &args,
                    )
                } else {
                    let args = vec![
                        Value::from_serialize(db_schema),
                        Value::from_serialize(relation_as_values.clone()),
                    ];
                    get_catalog_by_relations(
                        &jinja_env_clone,
                        "get_catalog_relations",
                        &project_name_owned,
                        &project_name_owned,
                        &iteration_context,
                        &args,
                    )
                };
                match jinja_result {
                    Ok(v) => match convert_macro_result_to_record_batch(&v) {
                        Ok(record_batch) => {
                            record_span_status(&catalog_fetch_span, None);
                            shared_results_clone.lock().unwrap().push(record_batch);
                        }
                        Err(e) => {
                            let msg = format!("[Non-critical] Issue processing catalog for schema '{database}.{schema}': {e}");
                            record_span_status(&catalog_fetch_span, Some(&msg));
                            emit_info_log_message(&msg);
                            shared_errors_clone.lock().unwrap().push(msg);
                        }
                    },
                    Err(e) => {
                        let msg = format!("[Non-critical] Issue fetching catalog for schema '{database}.{schema}': {e}");
                        record_span_status(&catalog_fetch_span, Some(&msg));
                        emit_info_log_message(&msg);
                        shared_errors_clone.lock().unwrap().push(msg);
                    }
                }
            }
            Ok(())
        });
        handles.push(handle);
    }

    // Do this so that handles are not abandoned immediately while we poll
    let _handles = handles;

    // Do not await workers directly as they may lock due to ADBC issues
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;

        let tracker_snapshot = progress_tracker.lock().unwrap().clone();

        // All workers finished normally.
        //
        // The queue check is not redundant with the tracker: a worker only
        // appears in the tracker once it has claimed a task, and workers are now
        // pool tasks, so with fewer pool threads than workers the ones still
        // queued have claimed nothing. An empty tracker alone would read that as
        // "everything finished" on the first tick and report a full catalog
        // built from zero batches.
        if tracker_snapshot.is_empty() && task_queue.lock().unwrap().is_empty() {
            emit_info_log_message("Fetched full catalog.json results");
            break;
        }

        let now = Instant::now();
        let all_stale = tracker_snapshot
            .iter()
            .all(|(_, (_, start_time))| now.duration_since(*start_time) > WORKER_TIMEOUT);

        if all_stale {
            emit_info_log_message("Fetched partial catalog.json results");

            // Record errors for hung workers
            for (schema, start_time) in tracker_snapshot.values() {
                let elapsed = now.duration_since(*start_time).as_secs();
                shared_errors
                    .lock()
                    .unwrap()
                    .push(format!("Timed out on schema '{schema}' after {elapsed}s"));
            }

            // Record errors for any remaining unprocessed tasks in the queue
            let remaining_tasks = std::mem::take(&mut *task_queue.lock().unwrap());
            for (database, schema) in remaining_tasks {
                shared_errors
                    .lock()
                    .unwrap()
                    .push(format!("Schema '{database}.{schema}' was never processed"));
            }

            break;
        }
    }

    // Collect results from shared structure
    let record_batches = std::mem::take(&mut *shared_results.lock().unwrap());

    for record_batch in record_batches.iter() {
        node_stats_and_stuff
            .extend(metadata_adapter.build_schemas_from_stats_sql(Arc::clone(record_batch))?);
        node_columns
            .extend(metadata_adapter.build_columns_from_get_columns(Arc::clone(record_batch))?);
    }
    let catalog_errors = std::mem::take(&mut *shared_errors.lock().unwrap());
    let mut catalog = build_catalog(
        &arg.io.invocation_id.to_string(),
        resolved_state,
        node_stats_and_stuff,
        node_columns,
    );
    if !catalog_errors.is_empty() {
        emit_info_log_message(
            "Encountered some issues building catalog.json, these did not affect job execution",
        );
        catalog.errors = Some(catalog_errors);
    }
    Ok(catalog)
}

/// Fetches catalog data from the warehouse and writes `catalog.json`.
#[allow(clippy::too_many_arguments)]
pub async fn write_catalog_json(
    adapter: &Arc<Adapter>,
    resolved_state: &ResolverState,
    relations: Vec<Arc<dyn BaseRelation>>,
    jinja_env: &JinjaEnv,
    project_name: &str,
    context: &BTreeMap<String, Value>,
    arg: &EvalArgs,
    batches: usize,
) -> FsResult<DbtCatalog> {
    let catalog = fetch_catalog_data(
        adapter,
        resolved_state,
        relations,
        jinja_env,
        project_name,
        context,
        arg,
        batches,
    )
    .await?;
    write_artifact_to_file(
        &catalog,
        ArtifactType::Catalog,
        &arg.io.out_dir,
        DBT_CATALOG_JSON,
        &arg.io.in_dir,
    )?;
    emit_info_log_message("Successfully wrote catalog.json");
    Ok(catalog)
}

/// Report how the nodes this run reported compare against the externally supplied node set.
///
/// This is the actual invariant check, and the primary signal. The counters emitted when the
/// schedule was built describe a different level: they can look clean while this one fails, because
/// a node can be scheduled correctly and then never produce a result row.
///
/// Re-resolves the supplied set rather than threading it down from schedule time; the artifact is
/// small, and a read failure here cannot happen without the run having already failed at schedule
/// time.
fn report_selection_override_reconciliation(
    arg: &EvalArgs,
    run_results: &RunResultsArtifact,
    nodes: &Nodes,
) {
    let Ok(Some(over)) = resolve_selection_override(arg) else {
        return;
    };

    let reported: BTreeSet<String> = run_results
        .results
        .iter()
        .map(|result| result.unique_id.clone())
        .collect();
    let report = reconcile_reported_nodes(over.ids(), &reported, nodes);

    let mut message = format!(
        "Nodes reported by this run against the externally supplied node set: \
         reported={} injected={} ran_not_injected={} injected_not_ran={}",
        report.reported,
        report.injected,
        report.ran_not_injected.len(),
        report.injected_not_ran.len(),
    );
    if !report.ran_not_injected.is_empty() {
        message.push_str(&format!(
            "; ran but not supplied: {}",
            format_sample(
                &report.ran_not_injected[..report.ran_not_injected.len().min(SAMPLE_CAP)],
                report.ran_not_injected.len()
            )
        ));
    }
    if !report.injected_not_ran.is_empty() {
        message.push_str(&format!(
            "; supplied but not run: {}",
            format_sample(
                &report.injected_not_ran[..report.injected_not_ran.len().min(SAMPLE_CAP)],
                report.injected_not_ran.len()
            )
        ));
    }

    if report.is_clean() {
        emit_info_log_message(message);
    } else {
        emit_warn_log_message(ErrorCode::SelectionOverrideDivergence, message);
    }
}

fn write_catalog_columns_epoch(catalog: &DbtCatalog, arg: &EvalArgs) {
    use chrono::Utc;
    use dbt_metadata_parquet::catalog_columns::CatalogColumnRow;

    let ingested_at = Utc::now().timestamp_micros();
    let mut rows = Vec::new();

    for (unique_id, table) in catalog.nodes.iter().chain(catalog.sources.iter()) {
        for (idx, (_col_name, col)) in table.columns.iter().enumerate() {
            rows.push(CatalogColumnRow {
                unique_id: unique_id.clone(),
                column_name: col.name.clone(),
                column_index: idx as i32,
                catalog_type: Some(col.data_type.clone()),
                catalog_comment: col.comment.clone(),
                ingested_at,
            });
        }
    }

    let dir = arg.metadata_dir().join("catalog").join("columns");
    if let Err(e) =
        dbt_metadata_parquet::catalog_columns::write_catalog_columns(&dir, rows, None, None, None)
    {
        emit_warn_log_message(
            ErrorCode::Generic,
            format!("metadata: catalog_columns: {e}"),
        );
    }
}
