use std::any::Any;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use clap::{Parser, Subcommand};
use minijinja::Value as MinijinjaValue;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use dbt_adapter::Adapter;
use dbt_clap_core::commands::{AbstractExtensionCommand, ExtensionCommandParser};
use dbt_clap_core::{
    Cli, CliParser, CliParserFactory, CommonArgs, InitArgs, SystemUpgradeDistributionArgs,
    in_out_dir,
};
use dbt_cloud_config::ResolvedCloudConfig;
use dbt_common::cancellation::{CancellationToken, CancellationTokenSource};
use dbt_common::fail_fast::FailFast;
use dbt_common::io_args::{EvalArgs, FsCommand, IoArgs, Phases, SystemArgs};
use dbt_common::pretty_string::CYAN;
use dbt_common::tracing::dbt_emit::{
    emit_error_log_from_fs_error, emit_error_log_message, emit_warn_log_message,
};
use dbt_common::{ErrorCode, FsError, FsResult, fs_err};
use dbt_compilation::config::CompilationConfig;
use dbt_dag::schedule::Schedule;
use dbt_dist::upgrade::exec_upgrade_distribution;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schema_store::{CanonicalFqn, DataStoreTrait, SchemaStoreTrait};
use dbt_schemas::schemas::{Nodes, StateArtifacts};
use dbt_schemas::state::{DbtState, ResolverState};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::{PreTaskRunData, RunTaskResults};

use crate::feature_stack::FeatureStack;
use crate::metricflow::MetricflowClient;

pub struct CliFeature {
    pub command_name: &'static str,
    pub version_check_enabled: bool,
    pub hooks: Box<dyn CliExtensionHooks>,
    pub cli_parser_factory: Arc<dyn CliParserFactory>,
    /// Global [CancelltionTokenSource] that can be used to signal cancellation to
    /// tasks running in other threads from a signal handler (e.g. Ctrl+C).
    pub cancellation_token_source: CancellationTokenSource,
    /// Per CLI invocation fail-fast signal.
    ///
    /// Each invocation of the CLI (or test) gets its own isolated signal
    /// so concurrent runs don't interfere with each other.
    pub fail_fast: FailFast,
}

pub struct CliFeatureBuilder {
    command_name: &'static str,
    version_check_enabled: bool,
    hooks: Option<Box<dyn CliExtensionHooks>>,
    cli_parser_factory: Option<Arc<dyn CliParserFactory>>,
}

impl CliFeatureBuilder {
    pub fn new(command_name: &'static str) -> Self {
        Self {
            command_name,
            version_check_enabled: false,
            hooks: None,
            cli_parser_factory: None,
        }
    }

    pub fn version_check_enabled(mut self, enabled: bool) -> Self {
        self.version_check_enabled = enabled;
        self
    }

    pub fn hooks(mut self, hooks: Box<dyn CliExtensionHooks>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    pub fn cli_parser_factory(mut self, factory: Arc<dyn CliParserFactory>) -> Self {
        self.cli_parser_factory = Some(factory);
        self
    }

    pub fn build(self) -> CliFeature {
        let hooks = self
            .hooks
            .unwrap_or_else(|| Box::new(DefaultCliExtensionHooks));

        let cli_parser_factory = self
            .cli_parser_factory
            .unwrap_or_else(|| Arc::new(DefaultCliParserFactory));

        CliFeature {
            command_name: self.command_name,
            version_check_enabled: self.version_check_enabled,
            hooks,
            cli_parser_factory,
            cancellation_token_source: CancellationTokenSource::new(),
            fail_fast: FailFast::new(),
        }
    }
}

#[derive(clap::Parser, Debug, Clone, Serialize, Deserialize)]
#[command()]
pub enum SystemCommand {
    /// Informative-only update command for dbt v2 OSS.
    #[clap(hide = true)]
    Update,
    /// Informative-only uninstall command for dbt v2 OSS.
    #[clap(hide = true)]
    Uninstall,
    /// Preinstall all supported database drivers into the local cache
    InstallDrivers,
    /// Upgrade a dbt v2 OSS installation to dbt v2 proprietary
    UpgradeDistribution(SystemUpgradeDistributionArgs),
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct SystemMgmtArgs {
    #[command(subcommand)]
    pub command: SystemCommand,
    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,
}

impl SystemMgmtArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::Deps;
        eval_args
    }
}

/// Arguments for a command that requires the full dbt distribution and is
/// stubbed out in dbt OSS. Any arguments are accepted and ignored, since the
/// command never executes — it only reports that the full distribution is
/// required.
#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct MissingDistributionStubArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    pub args: Vec<String>,
}

impl MissingDistributionStubArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        CommonArgs::default().to_eval_args(arg, in_dir, out_dir)
    }
}

#[derive(clap::Subcommand, Debug, Clone)]
#[allow(clippy::large_enum_variant)] // System is expected to be much larger than the stub variants.
pub enum OSSExtensionCommand {
    /// dbt v2 OSS system subcommand
    System(SystemMgmtArgs),
    /// Lint models (requires the full dbt distribution)
    Lint(MissingDistributionStubArgs),
}

impl AbstractExtensionCommand for OSSExtensionCommand {
    fn name(&self) -> &'static str {
        match self {
            OSSExtensionCommand::System(_) => "system",
            OSSExtensionCommand::Lint(_) => "lint",
        }
    }

    fn clone_box(&self) -> Box<dyn AbstractExtensionCommand> {
        Box::new(self.clone())
    }

    fn display_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        <Self as fmt::Debug>::fmt(self, f)
    }

    fn as_any(&self) -> &dyn Any {
        self as &dyn Any
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self as &mut dyn Any
    }
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }

    fn is_project_command(&self) -> bool {
        use OSSExtensionCommand::*;
        !matches!(self, System(_) | Lint(_))
    }

    fn to_eval_args(&self, common_args: &CommonArgs, system_arg: SystemArgs) -> FsResult<EvalArgs> {
        use OSSExtensionCommand::*;
        // Determine the input and output directories based on the command.
        // Some commands operate without project context, while others must be run in a project directory.
        let (in_dir, out_dir) = if self.is_project_command() {
            in_out_dir(common_args)?
        } else {
            (PathBuf::from("."), PathBuf::from("."))
        };

        let from_main = system_arg.from_main;
        let mut arg = match self {
            System(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
            Lint(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
        };
        arg.from_main = from_main;

        Ok(arg)
    }

    fn common_args(&self) -> CommonArgs {
        use OSSExtensionCommand::*;
        match self {
            System(args) => args.common_args.clone(),
            Lint(_) => CommonArgs::default(),
        }
    }

    fn stage(&self) -> Phases {
        use OSSExtensionCommand::*;
        match self {
            System(_) | Lint(_) => {
                unreachable!("stub command does not need a phase")
            }
        }
    }

    fn as_command(&self) -> FsCommand {
        use OSSExtensionCommand::*;
        match self {
            System(_) => FsCommand::System,
            Lint(_) => FsCommand::Extension("lint"),
        }
    }

    fn extend_cli_options(&self, _options: &mut Vec<String>) {
        // no-op
    }

    fn sample_select(&self) -> Option<Vec<String>> {
        None
    }

    fn sample_exclude(&self) -> Option<Vec<String>> {
        None
    }
}

pub struct DefaultCliParserFactory;

impl CliParserFactory for DefaultCliParserFactory {
    fn create(&self, command_name: &'static str, version: &'static str) -> CliParser {
        CliParser::new(command_name, version, Box::new(OSSExtensionCommandParser))
    }
}

struct OSSExtensionCommandParser;

impl ExtensionCommandParser for OSSExtensionCommandParser {
    fn from_arg_matches_mut(
        &self,
        arg_matches: &mut clap::ArgMatches,
    ) -> Result<Box<dyn AbstractExtensionCommand>, clap::Error> {
        let cmd = <OSSExtensionCommand as clap::FromArgMatches>::from_arg_matches_mut(arg_matches)?;
        Ok(Box::new(cmd) as Box<dyn AbstractExtensionCommand>)
    }

    fn augment_subcommands(&self, app: clap::Command) -> clap::Command {
        OSSExtensionCommand::augment_subcommands(app)
    }

    fn has_subcommand(&self, name: &str) -> bool {
        OSSExtensionCommand::has_subcommand(name)
    }
}

#[async_trait]
pub trait CliExtensionHooks: Send + Sync {
    /// Called before CLI compilation argument validation.
    ///
    /// Allowing extensions to inspect or reject arguments before any execution begins.
    fn will_validate_compilation_cli_args(
        &self,
        cli: &Cli,
        eval_arg: &mut Cow<EvalArgs>,
        dbt_state: &Arc<DbtState>,
        config: &CompilationConfig,
    ) -> FsResult<()>;

    /// Called when `dbt init` is invoked, before project initialization begins.
    async fn will_init_project(
        &self,
        invocation_id: Uuid,
        cli: &Cli,
        init_args: &InitArgs,
    ) -> FsResult<()>;

    /// Called early in execution, before any tasks are scheduled or run.
    async fn will_execute(
        &self,
        cli: &Cli,
        _eval_arg: &EvalArgs,
        feature_stack: &Arc<FeatureStack>,
    ) -> FsResult<()>;

    /// Called after the project has been resolved, before task scheduling
    /// and execution.
    ///
    /// This is the earliest point where `ResolverState` (including nodes,
    /// groups, and other resolved project data) is available.
    async fn did_resolve_project(
        &self,
        cli: &Cli,
        arg: &EvalArgs,
        resolved_state: &ResolverState,
        jinja_env: &JinjaEnv,
    ) -> FsResult<()>;

    /// Called just before tasks are scheduled and run.
    fn will_run_tasks(
        &self,
        cli: &Cli,
        arg: &EvalArgs,
        resolved_state: &ResolverState,
        token: &CancellationToken,
    ) -> FsResult<()>;

    /// Called after the schedule has been built, before task-scheduling
    /// checkpoints. Gives an implementation access to the resolved selection
    /// (`schedule.selected_nodes`) before any tasks run. An implementation may
    /// fully handle the command here and terminate execution by returning
    /// `Err(FsError::exit_with_status(n))`.
    fn did_build_schedule(
        &self,
        _cli: &Cli,
        _arg: &EvalArgs,
        _resolved_state: &ResolverState,
        _schedule: &Schedule<String>,
        _jinja_env: &JinjaEnv,
        _token: &CancellationToken,
    ) -> FsResult<()> {
        Ok(())
    }

    /// Called after tasks have been scheduled and run, but before manifest
    /// update and further phases.
    ///
    /// Return `Ok(())` if execution was not fully handled by this hook and
    /// should continue normally. To signal that a command was handled and
    /// execution should terminate, return `Err(FsError::exit_with_status(0))`
    /// for success or `Err(FsError::exit_with_status(n))` for failure.
    async fn did_schedule_and_run_tasks(
        &self,
        arg: &EvalArgs,
        cli: &Cli,
        previous_state: Option<&StateArtifacts>,
        run_task_results: &RunTaskResults,
        resolved_state: &ResolverState,
        token: &CancellationToken,
    ) -> FsResult<()>;

    /// Called after compile output has been emitted, providing the full
    /// compilation state for consumers that need post-compile access
    /// (e.g. REPL bootstrap).
    async fn did_emit_selected_compile_output(
        &self,
        arg: &EvalArgs,
        resolved_state: &ResolverState,
        jinja_env: &Arc<JinjaEnv>,
        task_runner_ctx: Option<&TaskRunnerCtx>,
        schema_store: &Arc<dyn SchemaStoreTrait>,
        data_store: &Arc<dyn DataStoreTrait>,
        map_compiled_sql: &HashMap<&str, Option<&str>>,
        feature_stack: &Arc<FeatureStack>,
        token: &CancellationToken,
    ) -> FsResult<()>;

    /// Called after compilation and manifest update, once the full schedule
    /// and lineage information are available.
    ///
    /// This is called after `did_schedule_and_run_tasks` and compilation
    /// happened without errors.
    ///
    /// Return `Ok(())` if execution was not fully handled by this hook and
    /// should continue normally. To signal that a command was handled and
    /// execution should terminate, return `Err(FsError::exit_with_status(0))`
    /// for success or `Err(FsError::exit_with_status(n))` for failure.
    async fn did_compile(
        &self,
        arg: &EvalArgs,
        cli: &Cli,
        resolved_state: &ResolverState,
        schedule: &Schedule<String>,
        token: &CancellationToken,
    ) -> FsResult<()>;

    async fn did_defer_nodes(
        &self,
        arg: &EvalArgs,
        deferred_unique_ids: &HashMap<CanonicalFqn, String>,
        resolved_state: &mut ResolverState,
        nodes: &Nodes,
        adapter: Arc<Adapter>,
    ) -> FsResult<()>;

    /// Called after all compile-time setup (adapter init, defer, schema hydration)
    /// and before the task runner starts.
    ///
    /// Returns per-node data that the task runner consumes, or `None` if this
    /// hook has nothing to contribute. An `Err` with an exit status signals that
    /// the hook handled the command fully and execution should terminate.
    async fn did_pre_run(
        &self,
        arg: &EvalArgs,
        cli: &Cli,
        jinja_env: Arc<JinjaEnv>,
        augmented_resolved_state: &ResolverState,
        schedule: &Schedule<String>,
        adapter: Arc<Adapter>,
        base_context: &BTreeMap<String, MinijinjaValue>,
        token: &CancellationToken,
    ) -> FsResult<Option<Box<dyn PreTaskRunData>>>;

    async fn did_handle_defer(
        &self,
        arg: &EvalArgs,
        cli: &Cli,
        jinja_env: Cow<'_, JinjaEnv>,
        augmented_resolved_state: &ResolverState,
        schedule: &Schedule<String>,
        metricflow_client: Option<Arc<dyn MetricflowClient>>,
        token: &CancellationToken,
    ) -> FsResult<()>;

    /// Called before deferred state is loaded. Implementations may populate
    /// `manifest_path` with a locally cached manifest to use for deferral.
    async fn will_load_deferred_state(
        &self,
        io: &IoArgs,
        cloud_config: Option<&ResolvedCloudConfig>,
        manifest_path: &mut Option<PathBuf>,
    ) -> FsResult<()>;
}

pub(crate) struct DefaultCliExtensionHooks;

#[async_trait]
impl CliExtensionHooks for DefaultCliExtensionHooks {
    fn will_validate_compilation_cli_args(
        &self,
        _cli: &Cli,
        _eval_arg: &mut Cow<EvalArgs>,
        _dbt_state: &Arc<DbtState>,
        _config: &CompilationConfig,
    ) -> FsResult<()> {
        Ok(())
    }

    async fn will_init_project(
        &self,
        _invocation_id: Uuid,
        _cli: &Cli,
        _init_args: &InitArgs,
    ) -> FsResult<()> {
        Ok(())
    }

    async fn will_execute(
        &self,
        cli: &Cli,
        _eval_arg: &EvalArgs,
        feature_stack: &Arc<FeatureStack>,
    ) -> FsResult<()> {
        use OSSExtensionCommand::*;
        match cli.extension_command::<OSSExtensionCommand>() {
            Some(Lint(_)) => {
                emit_warn_log_message(
                    ErrorCode::NotSupported,
                    format!(
                        "This command requires the full dbt distribution, which is not installed.\n\
                         install it with: {}",
                        CYAN.apply_to("dbt system upgrade-distribution")
                    ),
                );
                Err(FsError::exit_with_status(1))
            }
            Some(System(args)) => {
                match &args.command {
                    SystemCommand::Update => {
                        let e = fs_err!(
                            ErrorCode::NotSupported,
                            "`dbt system update` is not supported for this distribution. Upgrade \
             dbt-core with the package manager you installed it with:\
             \n\n    pip install --pre --upgrade dbt-core\
             \n    brew upgrade dbt-core\
             \n    winget upgrade --id dbtLabs.dbt-core --exact"
                        );
                        emit_error_log_from_fs_error(*e);
                        Err(FsError::exit_with_status(1))
                    }
                    SystemCommand::Uninstall => {
                        let e = fs_err!(
                            ErrorCode::NotSupported,
                            "`dbt system uninstall` is not supported for this distribution. Remove \
             dbt-core with the package manager you installed it with:\
             \n\n    pip uninstall dbt-core\
             \n    brew uninstall dbt-core\
             \n    winget uninstall --id dbtLabs.dbt-core"
                        );
                        emit_error_log_from_fs_error(*e);
                        Err(FsError::exit_with_status(1))
                    }
                    SystemCommand::InstallDrivers => {
                        dbt_adbc::pre_install_all_drivers().map_err(|install_err| {
                            emit_error_log_message(
                                ErrorCode::Generic,
                                format!("Failed to install drivers: {}", install_err).as_str(),
                            );
                            FsError::exit_with_status(1)
                        })
                    }
                    SystemCommand::UpgradeDistribution(args) => exec_upgrade_distribution(
                        args.yes,
                        args.package_manager.clone(),
                        feature_stack.cli.command_name,
                    )
                    .await
                    .map_err(|e| {
                        emit_error_log_from_fs_error(*e);
                        FsError::exit_with_status(1)
                    }),
                }?;
                // handled the System command, signal to exit with success
                Err(FsError::exit_with_status(0))
            }
            _ => Ok(()), // nothing handled, continue normal execution
        }
    }

    async fn did_resolve_project(
        &self,
        _cli: &Cli,
        _arg: &EvalArgs,
        _resolved_state: &ResolverState,
        _jinja_env: &JinjaEnv,
    ) -> FsResult<()> {
        Ok(())
    }

    fn will_run_tasks(
        &self,
        _cli: &Cli,
        _arg: &EvalArgs,
        _resolved_state: &ResolverState,
        _token: &CancellationToken,
    ) -> FsResult<()> {
        Ok(())
    }

    async fn did_schedule_and_run_tasks(
        &self,
        _arg: &EvalArgs,
        _cli: &Cli,
        _previous_state: Option<&StateArtifacts>,
        _run_task_results: &RunTaskResults,
        _resolved_state: &ResolverState,
        _token: &CancellationToken,
    ) -> FsResult<()> {
        Ok(())
    }

    async fn did_emit_selected_compile_output(
        &self,
        _arg: &EvalArgs,
        _resolved_state: &ResolverState,
        _jinja_env: &Arc<JinjaEnv>,
        _task_runner_ctx: Option<&TaskRunnerCtx>,
        _schema_store: &Arc<dyn SchemaStoreTrait>,
        _data_store: &Arc<dyn DataStoreTrait>,
        _map_compiled_sql: &HashMap<&str, Option<&str>>,
        _feature_stack: &Arc<FeatureStack>,
        _token: &CancellationToken,
    ) -> FsResult<()> {
        Ok(())
    }

    async fn did_defer_nodes(
        &self,
        _arg: &EvalArgs,
        _deferred_unique_ids: &HashMap<CanonicalFqn, String>,
        _resolved_state: &mut ResolverState,
        _nodes: &Nodes,
        _adapter: Arc<Adapter>,
    ) -> FsResult<()> {
        Ok(())
    }

    async fn did_compile(
        &self,
        _arg: &EvalArgs,
        _cli: &Cli,
        _resolved_state: &ResolverState,
        _schedule: &Schedule<String>,
        _token: &CancellationToken,
    ) -> FsResult<()> {
        Ok(())
    }

    async fn did_pre_run(
        &self,
        _arg: &EvalArgs,
        _cli: &Cli,
        _jinja_env: Arc<JinjaEnv>,
        _augmented_resolved_state: &ResolverState,
        _schedule: &Schedule<String>,
        _adapter: Arc<Adapter>,
        _base_context: &BTreeMap<String, MinijinjaValue>,
        _token: &CancellationToken,
    ) -> FsResult<Option<Box<dyn PreTaskRunData>>> {
        Ok(None)
    }

    async fn did_handle_defer(
        &self,
        _arg: &EvalArgs,
        _cli: &Cli,
        _jinja_env: Cow<'_, JinjaEnv>,
        _augmented_resolved_state: &ResolverState,
        _schedule: &Schedule<String>,
        _metricflow_client: Option<Arc<dyn MetricflowClient>>,
        _token: &CancellationToken,
    ) -> FsResult<()> {
        Ok(())
    }

    async fn will_load_deferred_state(
        &self,
        _io: &IoArgs,
        _cloud_config: Option<&ResolvedCloudConfig>,
        _manifest_path: &mut Option<PathBuf>,
    ) -> FsResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system_help(parser: &CliParser) -> String {
        parser
            .try_parse_from(["dbt", "system", "--help"])
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn hidden_self_management_commands_are_absent_from_help_but_parseable() {
        let version = "2.x";
        let parser = CliParser::new("dbt-core", version, Box::new(OSSExtensionCommandParser));

        // Hidden from help, but unaffected commands remain.
        let help = system_help(&parser);
        assert!(!help.contains("Update dbt in place"), "got:\n{help}");
        assert!(!help.contains("Uninstall dbt"), "got:\n{help}");
        assert!(help.contains("install-drivers"), "got:\n{help}");

        // Still parseable, so the runtime message fires instead of a parse error.
        let cli = parser.try_parse_from(["dbt", "system", "update"]).unwrap();
        let oss_ext_cmd = cli.extension_command::<OSSExtensionCommand>().unwrap();
        assert!(matches!(
            oss_ext_cmd,
            OSSExtensionCommand::System(SystemMgmtArgs {
                command: SystemCommand::Update,
                ..
            })
        ));
    }

    #[test]
    fn system_is_not_a_project_command() {
        let parser = CliParser::new("dbt-core", "2.x", Box::new(OSSExtensionCommandParser));
        let cli = parser.try_parse_from(["dbt", "system", "update"]).unwrap();

        let oss_ext_cmd = cli.extension_command::<OSSExtensionCommand>().unwrap();
        assert!(!oss_ext_cmd.is_project_command());

        // `Cli::is_project_command` should delegate to the extension command.
        assert!(!cli.is_project_command());
    }

    fn root_help(parser: &CliParser) -> String {
        parser
            .try_parse_from(["dbt", "--help"])
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn distribution_stub_commands_are_visible_in_help_and_parseable() {
        let parser = CliParser::new("dbt-core", "2.x", Box::new(OSSExtensionCommandParser));

        let help = root_help(&parser);
        assert!(help.contains("lint"), "got:\n{help}");

        parser
            .try_parse_from(["dbt", "lint"])
            .expect("expected `[\"dbt\", \"lint\"]` to parse");

        // Arguments are accepted and ignored, not rejected as unknown flags/files.
        parser
            .try_parse_from(["dbt", "lint", "models/foo.sql", "--fix"])
            .expect("lint should accept and ignore trailing args");
    }

    #[test]
    fn distribution_stub_commands_are_not_project_commands() {
        let parser = CliParser::new("dbt-core", "2.x", Box::new(OSSExtensionCommandParser));

        let cli = parser.try_parse_from(["dbt", "lint"]).unwrap();
        assert!(
            !cli.is_project_command(),
            "lint should not require a project dir"
        );
    }
}
