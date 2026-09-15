use console::Style;
use dbt_common::collections::HashSet;
use dbt_common::io_utils::determine_project_dir;
use dbt_common::{ErrorCode, FsResult, fs_err, stdfs};
use dbt_yaml::Value as YValue;
use serde::{Deserialize, Serialize};

use std::any::Any;
use std::env;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fmt;
use std::io::Write;
use std::sync::LazyLock;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
};
use strum::IntoEnumIterator;
use strum_macros::Display;
use uuid::Uuid;

use clap::{
    ArgAction, Parser, ValueEnum,
    builder::{BoolishValueParser, TypedValueParser},
};
use clap_complete::Shell;
use dbt_common::constants::{
    DBT_DEFAULT_LOG_FILE_MAX_BYTES, DBT_PROJECT_YML, DBT_TARGET_DIR_NAME, NOOP,
};
use dbt_common::io_args::FsCommand;
use dbt_common::io_args::{
    ClapResourceType, ClapSchemaTypes, ComputeArg, EvalArgs, InternalPackageMode, IoArgs,
    LocalExecutionBackendKind, LogFormat, LogLevel, OptimizeTestsOptions, Phases, RunCacheMode,
    ShowOptions, SystemArgs, TimeMachineModeKind, TimeMachineReplayOrdering,
    check_key_value_cli_arg, check_key_value_cli_arg_with_recovery, check_selector, check_target,
    validate_project_name,
};
use dbt_common::io_args::{DisplayFormat, ListOutputFormat, StaticAnalysisKind};
use dbt_common::row_limit::RowLimit;
use dbt_common::warn_error_options::WarnErrorOptions;
use dbt_common::warn_error_options::parse_warn_error_options;

use dbt_common::node_selector::{
    IndirectSelection, MethodName, SelectionCriteria, parse_model_specifiers,
};

use crate::time_machine::*;
use crate::tracing::*;

pub mod commands;
mod time_machine;
mod tracing;

pub use crate::commands::{Command, CoreCommand};

use self::commands::{CommandParser, ExtensionCommandParser};

pub const DEFAULT_LIMIT: &str = "10";
pub const DEFAULT_FORMAT: DisplayFormat = DisplayFormat::Table;

/// `--help` heading labels used to group flags into sections.
///
/// Defined here as constants so a heading is named in exactly one place and
/// can be reused by both the core CLI and the extension CLI (`dbt-clap`).
pub mod help_headings {
    pub const PROJECT: &str = "Project";
    pub const SELECTION: &str = "Selection";
    pub const EXECUTION: &str = "Execution";
    pub const LOGGING: &str = "Logging";
    pub const ARTIFACTS: &str = "Artifacts";
    pub const EVENT_TIME: &str = "Microbatch Event Time";
    pub const SAMPLE: &str = "Sample";
    pub const ADVANCED: &str = "Advanced";
}
const MANAGE_STATE_ENV: &str = "DBT_ENGINE_MANAGE_STATE";
const USER_SETTINGS_YML: &str = ".dbt/user_settings.yml";

// defined in pretty string, but copied here to avoid cycle...
static BOLD: LazyLock<Style> = LazyLock::new(|| Style::new().bold());

// ----------------------------------------------------------------------------------------------
// Cli and its subcommands

const ABOUT: &str = "An ELT tool for managing your SQL transformations and data models. For more documentation on these commands, visit: docs.getdbt.com";
static AFTER_HELP: LazyLock<String> = LazyLock::new(|| {
    format!(
        "{}",
        BOLD.apply_to(
            "Use `dbt <COMMAND> --help` to learn more about the options for each command."
        )
    )
});

/// Custom help template that hides top-level options (they are shown per-subcommand).
const CLI_HELP_TEMPLATE: &str = "\
{before-help}{name} {version}: {about-with-newline}
{usage-heading} {usage}

{subcommands}{after-help}";

pub trait CliParserFactory: Send + Sync {
    fn create(&self, command_name: &'static str, version: &'static str) -> CliParser;
}

/// An equivalent of [clap::Parser] that produces a [Cli] instead of parsing into itself.
///
/// This allows us to programatically control the clap setup and parsing process as
/// it allows [clap::Parser::try_parse] to be implemented as a method with a receiver
/// that can carry dependencies instead of a static method.
pub struct CliParser {
    command_name: &'static str,
    version: &'static str,
    command_parser: CommandParser,
}

impl CliParser {
    pub fn new(
        command_name: &'static str,
        version: &'static str,
        extension_command_parser: Box<dyn ExtensionCommandParser>,
    ) -> Self {
        let command_parser = CommandParser::new(extension_command_parser);
        Self {
            command_name,
            version,
            command_parser,
        }
    }

    /// User-facing CLI brand name. Surfaced in clap help/version output,
    /// the version banner, and `--log-format json` log lines.
    pub fn command_name(&self) -> &'static str {
        self.command_name
    }

    /// Instantiate `Cli` from the `ArgMatches` that `clap` generated.
    fn try_parse_from_arg_matches_mut(
        &self,
        arg_matches: &mut clap::ArgMatches,
    ) -> Result<Cli, clap::Error> {
        let command = self.command_parser.from_arg_matches_mut(arg_matches)?;
        let common_args = <CommonArgs as clap::FromArgMatches>::from_arg_matches_mut(arg_matches)?;
        let cli = Cli {
            command,
            common_args,
        };
        Ok(cli)
    }

    /// Build the [clap::Command] for the CLI application.
    fn app(&self) -> clap::Command {
        let app = clap::Command::new(self.command_name);

        // -- Augment arguments
        let app = app.group(clap::ArgGroup::new("Cli").multiple(true).args({
            let members: [clap::Id; 0] = [];
            members
        }));
        let app = self
            .command_parser
            .augment_subcommands(app)
            .subcommand_required(true)
            .arg_required_else_help(true);
        let app = <CommonArgs as clap::Args>::augment_args(app);

        // -- Augment metadata
        app.author("dbt Labs <info@getdbt.com>")
            .version(self.version)
            .long_about(None)
            .about(ABOUT)
            .after_help(&**AFTER_HELP)
            .help_template(CLI_HELP_TEMPLATE)
    }

    /// Key used for the machine-readable `--format json --version` output, e.g.
    /// `"dbt-oss"` -> `"oss"`; `"dbt"` has no `dbt-` prefix so it falls through
    /// to itself as the key.
    fn version_json_key(&self) -> &str {
        self.command_name
            .strip_prefix("dbt-")
            .unwrap_or(self.command_name)
    }

    pub fn version_json(&self) -> String {
        serde_json::json!({ self.version_json_key(): self.version }).to_string()
    }

    fn version_json_error(&self) -> clap::Error {
        clap::Error::raw(clap::error::ErrorKind::DisplayVersion, self.version_json())
    }

    fn args_request_json_version<I, T>(itr: I) -> bool
    where
        I: IntoIterator<Item = T>,
        T: AsRef<OsStr>,
    {
        let mut version = false;
        let mut json_format = false;
        let mut expect_format_value = false;

        for arg in itr.into_iter().skip(1) {
            let arg = arg.as_ref();

            if expect_format_value {
                if arg != OsStr::new("json") {
                    return false;
                }
                json_format = true;
                expect_format_value = false;
            } else if arg == OsStr::new("--version") {
                version = true;
            } else if arg == OsStr::new("--format") || arg == OsStr::new("--output") {
                expect_format_value = true;
            } else if arg == OsStr::new("--format=json") || arg == OsStr::new("--output=json") {
                json_format = true;
            } else {
                return false;
            }
        }

        version && json_format && !expect_format_value
    }

    pub fn json_version_for_args<I, T>(&self, itr: I) -> Option<String>
    where
        I: IntoIterator<Item = T>,
        T: AsRef<OsStr>,
    {
        Self::args_request_json_version(itr).then(|| self.version_json())
    }

    pub fn print_json_version_and_exit_for_args<I, T>(&self, itr: I)
    where
        I: IntoIterator<Item = T>,
        T: AsRef<OsStr>,
    {
        if let Some(version) = self.json_version_for_args(itr) {
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "{version}");
            let _ = stdout.flush();
            std::process::exit(0);
        }
    }

    fn format_error(&self, err: clap::Error) -> clap::Error {
        let mut cmd = self.app();
        err.format(&mut cmd)
    }

    /// Write shell completion scripts for the given shell to `writer`.
    pub fn write_completions<W: Write>(&self, shell: Shell, writer: &mut W) {
        clap_complete::generate(shell, &mut self.app(), "dbt", writer);
    }

    /// Parse from `std::env::args_os()`, [exit][Error::exit] on error.
    pub fn parse(&self) -> Box<Cli> {
        let args = env::args_os().collect::<Vec<_>>();
        self.print_json_version_and_exit_for_args(&args);

        match self.try_parse_from(args) {
            Ok(s) => s,
            Err(e) => e.exit(),
        }
    }

    /// Parse from `std::env::args_os()`, return Err on error.
    pub fn try_parse(&self) -> Result<Box<Cli>, clap::Error> {
        self.try_parse_from(env::args_os())
    }

    /// Parse from iterator, [exit][clap::Error::exit] on error.
    pub fn parse_from<I, T>(&self, itr: I) -> Box<Cli>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let args = itr.into_iter().collect::<Vec<_>>();
        self.print_json_version_and_exit_for_args(args.iter().cloned().map(Into::into));

        match self.try_parse_from(args) {
            Ok(s) => s,
            Err(e) => e.exit(),
        }
    }

    /// Parse from iterator, return Err on error.
    pub fn try_parse_from<I, T>(&self, itr: I) -> Result<Box<Cli>, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let args = itr.into_iter().collect::<Vec<_>>();
        if Self::args_request_json_version(args.iter().cloned().map(Into::into)) {
            return Err(self.version_json_error());
        }

        let mut matches = self.app().try_get_matches_from(args)?;
        let cli = self
            .try_parse_from_arg_matches_mut(&mut matches)
            .map_err(|err| self.format_error(err))?;
        Ok(Box::new(cli))
    }

    /// Extract warn-error options from the parsed CLI.
    pub fn warn_error_options(&self, cli: &Cli) -> Option<WarnErrorOptions> {
        Some(cli.common_args.get_cli_warn_error_options())
    }
}

#[derive(Debug, Clone)]
pub struct Cli {
    pub command: Command,
    pub common_args: CommonArgs,
}

/// Determine in/out dir assuming the command requires project dir.
///
/// NOTE: Don't call for commands that don't need project directory.
pub fn in_out_dir(common_args: &CommonArgs) -> FsResult<(PathBuf, PathBuf)> {
    let in_dir = if let Some(project_dir) = common_args.project_dir.clone() {
        project_dir
    } else {
        // TODO: the first argument to this function is never used anywhere in the codebase,
        // possibly it should be removed or properly wired
        let node_targets = &[];
        determine_project_dir(node_targets, DBT_PROJECT_YML)?
    };
    let in_dir = stdfs::canonicalize(in_dir)?;

    let out_dir = common_args
        .target_path
        .clone()
        .map(|p| if p.is_relative() { in_dir.join(p) } else { p })
        .unwrap_or_else(|| in_dir.join(DBT_TARGET_DIR_NAME));
    stdfs::create_dir_all(&out_dir).map_err(|e| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to create output directory: {}",
            e
        )
    })?;
    let out_dir = stdfs::canonicalize(out_dir)?;

    Ok((in_dir, out_dir))
}

impl Cli {
    pub fn extension_command<T: Any + fmt::Debug>(&self) -> Option<&T> {
        match &self.command {
            Command::Extension(ext_cmd) => {
                let typed = ext_cmd.as_any().downcast_ref::<T>();
                debug_assert!(
                    typed.is_some(),
                    "failed to downcast extension command to the expected type; \
did you use the correct CliParser? \
got {:?}, expected an instance of {}",
                    ext_cmd.as_ref() as &dyn fmt::Debug,
                    std::any::type_name::<T>()
                );
                typed
            }
            _ => None,
        }
    }

    pub fn is_project_command(&self) -> bool {
        use CoreCommand::*;
        match &self.command {
            Command::Core(
                Man(_) | Init(_) | Login(_) | State(_) | Completions(_) | Internal(_),
            ) => {
                // These commands do not require a project directory
                false
            }
            // `docs generate` reads a project's target directory, so it resolves
            // `--project-dir` / `--target-path` the same way every project command
            // does. `docs serve` only serves a directory and is documented as not
            // requiring a project, so it stays out.
            Command::Core(Docs(args)) => {
                matches!(args.subcommand, Some(DocsSubcommand::Generate(_)))
            }
            Command::Core(_) => {
                // Assume all the other core commands require a project directory.
                true
            }
            Command::Extension(ext_cmd) => ext_cmd.is_project_command(),
        }
    }

    pub fn to_eval_args(&self, system_arg: SystemArgs) -> FsResult<EvalArgs> {
        validate_manage_state_env()?;

        use CoreCommand::*;
        let common_args = self.common_args();
        // Determine the input and output directories based on the command.
        let (in_dir, out_dir) = {
            if self.is_project_command() {
                in_out_dir(&common_args)?
            } else {
                (PathBuf::from("."), PathBuf::from("."))
            }
        };

        let from_main = system_arg.from_main;

        let mut arg = match &self.command {
            Command::Core(core_cmd) => match core_cmd {
                Init(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Deps(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                List(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Compile(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Check(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Parse(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Run(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                RunOperation(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Seed(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Snapshot(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Ls(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Test(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Build(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Clone(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Clean(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Source(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Freshness(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Show(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Man(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Debug(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Retry(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Docs(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Login(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                State(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Completions(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
                Internal(args) => args.to_eval_args(system_arg, &in_dir, &out_dir),
            },
            Command::Extension(ext_cmd) => ext_cmd.to_eval_args(&common_args, system_arg)?,
        };
        arg.from_main = from_main;
        // Must be derived from the caller's *original* `--static-analysis`
        // value, before the force-to-strict override just below can
        // overwrite it for local execution — otherwise `baseline` could
        // never survive to be observed here. `unwrap_or_default()` because
        // omitting `--static-analysis` leaves `arg.static_analysis` as
        // `None`, not `Some(Baseline)`, even though baseline is the actual
        // default behavior everywhere else it's consulted.
        arg.infer_schemas_and_typeless = arg.static_analysis.unwrap_or_default()
            == StaticAnalysisKind::Baseline
            && arg.write_lineage;
        if arg.local_execution_backend != LocalExecutionBackendKind::Remote {
            arg.static_analysis = Some(StaticAnalysisKind::Strict);
        }
        if (arg.write_index || arg.generate_info_schema)
            && arg.static_analysis == Some(StaticAnalysisKind::Strict)
        {
            arg.write_lineage = true;
        }
        // `build`, `run`, and `check` write the index by default.
        //
        // After the lineage block above so a defaulted index does not turn on `write_lineage`.
        // Explicit `--write-index` still does. Set on `EvalArgs`, not `Cli.common_args`:
        // `effective_partial_parse()` / `effective_partial_load()` read the raw flag, so
        // leaving it unset keeps partial parse/load off for a plain `dbt build`/`run`/`check`.
        // Skip if the caller already set metadata, index, or catalog so `--write-metadata`
        // stays epochs-without-index and `--write-catalog` stays explicit.
        if matches!(
            arg.command,
            FsCommand::Build | FsCommand::Run | FsCommand::Check
        ) && !common_args.write_index
            && !common_args.write_metadata
            && !common_args.write_catalog
            && !common_args.no_write_index
        {
            arg.write_metadata = true;
            arg.write_index = true;
            arg.write_index_implied = true;
        }
        Ok(arg)
    }

    // TODO: Box the CommonArgs because it's a 1Kb struct
    pub fn common_args(&self) -> CommonArgs {
        match &self.command {
            Command::Core(core_cmd) => core_cmd.common_args().clone(),
            Command::Extension(ext_cmd) => ext_cmd.common_args(),
        }
    }

    pub fn project_dir(&self) -> Option<PathBuf> {
        self.common_args().project_dir
    }

    pub fn target_path(&self) -> Option<PathBuf> {
        self.common_args().target_path
    }

    pub fn stage(&self) -> Phases {
        use CoreCommand::*;
        // todo: fix this: should take the minimum of the user selection and the default
        match &self.command {
            Command::Core(core_cmd) => match core_cmd {
                Init(_args) => unreachable!("Init command does not need a phase"),
                Deps(args) => args.common_args.phase.clone().unwrap_or(Phases::Deps),
                Parse(args) => args.common_args.phase.clone().unwrap_or(Phases::Parse),
                Ls(args) => args.common_args.phase.clone().unwrap_or(Phases::List),
                List(args) => args.common_args.phase.clone().unwrap_or(Phases::List),
                Compile(args) => args.common_args.phase.clone().unwrap_or(Phases::Compile),
                Check(args) => args.common_args.phase.clone().unwrap_or(Phases::Compile),
                Run(args) => args.common_args.phase.clone().unwrap_or(Phases::All),
                RunOperation(args) => args.common_args.phase.clone().unwrap_or(Phases::Parse),
                Seed(args) => args.common_args.phase.clone().unwrap_or(Phases::All),
                Snapshot(args) => args.common_args.phase.clone().unwrap_or(Phases::All),
                Test(args) => args.common_args.phase.clone().unwrap_or(Phases::All),
                Build(args) => args.common_args.phase.clone().unwrap_or(Phases::All),
                Clone(args) => args.common_args.phase.clone().unwrap_or(Phases::All),
                Clean(_args) => unreachable!("Clean command does not need a phase"),
                Source(args) => args
                    .common_args()
                    .phase
                    .clone()
                    .unwrap_or(Phases::Freshness),
                Freshness(args) => args.common_args.phase.clone().unwrap_or(Phases::Freshness),
                Show(args) => args.common_args.phase.clone().unwrap_or(Phases::Show),
                Man(_args) => unreachable!("Man command does not need a phase"),
                Debug(args) => args.common_args.phase.clone().unwrap_or(Phases::Debug),
                Retry(args) => args.common_args.phase.clone().unwrap_or(Phases::All),
                Docs(_args) => unreachable!("Docs command does not need a phase"),
                Login(_args) => unreachable!("Login command does not need a phase"),
                State(_args) => unreachable!("State command does not need a phase"),
                Completions(_args) => unreachable!("Completions command does not need a phase"),
                Internal(_args) => unreachable!("Internal command does not need a phase"),
            },
            Command::Extension(ext_cmd) => ext_cmd.stage(),
        }
    }

    pub fn as_command(&self) -> FsCommand {
        match &self.command {
            Command::Core(core_cmd) => core_cmd.as_command(),
            Command::Extension(ext_cmd) => ext_cmd.as_command(),
        }
    }

    pub fn cli_options(&self) -> Vec<String> {
        use CoreCommand::*;
        let mut options = Vec::new();
        // Add global/common args
        options.extend(struct_to_cli_options(&self.common_args));
        // Add subcommand-specific args
        match &self.command {
            Command::Core(core_cmd) => match core_cmd {
                Build(args) => options.extend(struct_to_cli_options(args)),
                Run(args) => options.extend(struct_to_cli_options(args)),
                Test(args) => options.extend(struct_to_cli_options(args)),
                Parse(args) => options.extend(struct_to_cli_options(args)),
                Compile(args) => options.extend(struct_to_cli_options(args)),
                _ => {}
            },
            Command::Extension(ext_cmd) => ext_cmd.extend_cli_options(&mut options),
        }
        options
    }

    pub fn sample_select(&self) -> Option<Vec<String>> {
        match &self.command {
            Command::Core(_) => None,
            Command::Extension(ext_cmd) => ext_cmd.sample_select(),
        }
    }

    pub fn sample_exclude(&self) -> Option<Vec<String>> {
        match &self.command {
            Command::Core(_) => None,
            Command::Extension(ext_cmd) => ext_cmd.sample_exclude(),
        }
    }
}

/// Converts a serializable struct to CLI options (e.g. --foo bar --baz qux)
pub fn struct_to_cli_options<T: Serialize>(s: &T) -> Vec<String> {
    let mut options = Vec::new();
    let value = serde_json::to_value(s).unwrap();
    if let serde_json::Value::Object(map) = value {
        for (k, v) in map {
            if k == "vars" {
                if let serde_json::Value::Object(vars_map) = v
                    && !vars_map.is_empty()
                {
                    // Serialize as JSON string for CLI
                    let vars_str = dbt_yaml::to_string(&vars_map).unwrap();
                    options.push("--vars".to_string());
                    options.push(vars_str);
                }
                continue;
            }
            match v {
                serde_json::Value::Bool(true) => options.push(format!("--{}", k.replace('_', "-"))),
                serde_json::Value::Bool(false) => {} // skip false flags
                serde_json::Value::Null => {}
                serde_json::Value::Array(arr) => {
                    for item in arr {
                        options.push(format!("--{}", k.replace('_', "-")));
                        options.push(item.to_string().trim_matches('"').to_string());
                    }
                }
                _ => {
                    options.push(format!("--{}", k.replace('_', "-")));
                    options.push(v.to_string().trim_matches('"').to_string());
                }
            }
        }
    }
    options
}
// ----------------------------------------------------------------------------------------------
// Build, run, test, compile subcommands

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct CleanArgs {
    /// Clean the target directory specified by file or --target-path
    #[arg(value_parser = check_target)]
    pub files: Vec<String>,

    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,
}

impl CleanArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        self.common_args.to_eval_args(arg, in_dir, out_dir)
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct DepsArgs {
    #[arg(long)]
    pub add_package: Option<String>,
    #[arg(long)]
    pub upgrade: bool,
    #[arg(long)]
    pub lock: bool,

    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,
}
impl DepsArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::Deps;
        eval_args.add_package = self.add_package.clone();
        eval_args.upgrade = self.upgrade;
        eval_args.lock = self.lock;
        eval_args
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct ParseArgs {
    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,
}
impl ParseArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::Parse;
        eval_args
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct CompileArgs {
    /// Compile the given nodes, identified by paths, and all its upstreams
    #[arg(value_parser = check_target)]
    pub node_targets: Vec<String>,

    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// Provide SQL content directly to compile as a temporary model
    #[arg(long, conflicts_with = "select", allow_hyphen_values = true)]
    pub inline: Option<String>,

    /// Select nodes of a specific type;
    #[arg(long, num_args(1..), value_delimiter = ' ', aliases = ["resource-types"], env = "DBT_RESOURCE_TYPES")]
    pub resource_type: Option<Vec<ClapResourceType>>,

    /// Exclude nodes of a specific type;
    #[arg(long, num_args(1..), value_delimiter = ' ', aliases = ["exclude-resource-types"], env = "DBT_EXCLUDE_RESOURCE_TYPES")]
    pub exclude_resource_type: Option<Vec<ClapResourceType>>,

    /// Limiting number of shown rows. Run with --limit -1 to remove limit [default: 10]
    #[arg(long, default_value=DEFAULT_LIMIT, allow_hyphen_values = true)]
    pub limit: RowLimit,

    /// Display rows in different formats
    #[arg(global = true, long, aliases = ["format"])]
    pub output: Option<DisplayFormat>,

    /// Flag to enable or disable SQL analysis, or to run SQL in unsafe mode, enabled by default
    #[arg(global = true, long, env = "DBT_STATIC_ANALYSIS")]
    pub static_analysis: Option<StaticAnalysisKind>,

    /// Drop incremental models and fully recalculate incremental tables.
    #[arg(global = true, long, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), short = 'f', env = "DBT_FULL_REFRESH")]
    pub full_refresh: bool,

    /// Use the samples as given in this YAML/JSON file.
    #[arg(
        long,
        value_name = "default|FILE",
        alias = "with-sample",
        help_heading = help_headings::SAMPLE
    )]
    pub with_sample: Option<String>,
    /// Add source selectors to sample (e.g., "source:raw.events"). Repeatable.
    #[arg(long, num_args(1..), value_delimiter = ' ', help_heading = help_headings::SAMPLE)]
    pub sampled: Vec<String>,
}

impl CompileArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::Compile;
        eval_args.introspect = self.common_args.get_introspect();
        eval_args.limit = self.limit.into();
        // If introspection is disabled, set static analysis to off
        eval_args.static_analysis = if eval_args.introspect {
            self.static_analysis
        } else {
            Some(StaticAnalysisKind::Off)
        };
        eval_args.full_refresh = self.full_refresh;
        eval_args.format = self.output.unwrap_or(DEFAULT_FORMAT);
        if let Some(resource_type) = &self.resource_type {
            eval_args.resource_types = resource_type.clone();
        }
        if let Some(exclude_resource_type) = &self.exclude_resource_type {
            eval_args.exclude_resource_types = exclude_resource_type.clone();
        }
        configure_run_cache(
            &mut eval_args,
            &self.common_args,
            false,
            &RunCacheMode::ReadWrite,
        );

        eval_args
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct CheckArgs {
    /// Names of specific checks to run (the file stem, without `.sql`). Runs all
    /// discovered checks if omitted.
    #[arg(value_name = "CHECK")]
    pub check_names: Vec<String>,

    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// Flag to enable or disable SQL analysis, or to run SQL in unsafe mode, enabled by default
    #[arg(global = true, long, env = "DBT_STATIC_ANALYSIS")]
    pub static_analysis: Option<StaticAnalysisKind>,
}

impl CheckArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::Compile;
        eval_args.introspect = self.common_args.get_introspect();
        // v1 is parse-time only. Do not force static analysis off the way
        // warehouse-introspecting commands do.
        eval_args.static_analysis = self.static_analysis;
        // Implied index is set on `EvalArgs` in `Cli::to_eval_args` (same as
        // `build` / `run`), not here: setting `write_index` on this struct
        // before that function's lineage block would turn on `write_lineage`,
        // and setting `Cli.common_args` would turn on partial parse/load.
        eval_args.check_names = self.check_names.clone();
        eval_args
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct SeedArgs {
    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// Force node selection
    #[arg(long, default_value = "false")]
    pub force_node_selection: bool,

    /// The mode to use for dbt State. Cannot be used with --force-node-selection
    #[arg(
        long,
        default_value = "read-write",
        conflicts_with = "force_node_selection"
    )]
    pub run_cache_mode: RunCacheMode,

    /// Flag to enable or disable SQL analysis, or to run SQL in unsafe mode, enabled by default
    #[arg(global = true, long, env = "DBT_STATIC_ANALYSIS")]
    pub static_analysis: Option<StaticAnalysisKind>,

    /// Drop incremental models and fully recalculate incremental tables.
    #[arg(global = true, long, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), short = 'f', env = "DBT_FULL_REFRESH")]
    pub full_refresh: bool,
}

impl SeedArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.resource_types = vec![ClapResourceType::Seed];
        configure_run_cache(
            &mut eval_args,
            &self.common_args,
            self.force_node_selection,
            &self.run_cache_mode,
        );
        eval_args.static_analysis = self.static_analysis;
        eval_args.full_refresh = self.full_refresh;
        eval_args
    }
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct SourceArgs {
    #[command(subcommand)]
    pub command: SourceCommand,
}

impl SourceArgs {
    pub fn common_args(&self) -> &CommonArgs {
        match &self.command {
            SourceCommand::Freshness(f) => &f.common_args,
        }
    }

    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args().to_eval_args(arg, in_dir, out_dir);
        let predicate = SelectionCriteria::new(
            MethodName::ResourceType,
            vec![],
            "source".to_string(),
            false,
            None,
            None,
            Some(IndirectSelection::default()),
            None,
        );
        eval_args.check_all = match &self.command {
            SourceCommand::Freshness(f) => f.check_all,
        };
        eval_args.phase = Phases::Freshness;
        eval_args.set_refined_node_selectors(Some(predicate))
    }
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
#[command()]
pub enum SourceCommand {
    /// Check the current freshness of the project's sources
    Freshness(SourceFreshnessArgs),
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct SourceFreshnessArgs {
    /// Check freshness of all sources
    #[arg(long, default_value = "false")]
    pub check_all: bool,

    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct FreshnessArgs {
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// Select nodes of a specific type;
    #[arg(long, num_args(1..), value_delimiter = ' ', aliases = ["resource-types"], env = "DBT_RESOURCE_TYPES")]
    pub resource_type: Option<Vec<ClapResourceType>>,

    /// Exclude nodes of a specific type;
    #[arg(long, num_args(1..), value_delimiter = ' ', aliases = ["exclude-resource-types"], env = "DBT_EXCLUDE_RESOURCE_TYPES")]
    pub exclude_resource_type: Option<Vec<ClapResourceType>>,
}

impl FreshnessArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::Freshness;
        if let Some(resource_type) = &self.resource_type {
            eval_args.resource_types = resource_type.clone();
        }
        if let Some(exclude_resource_type) = &self.exclude_resource_type {
            eval_args.exclude_resource_types = exclude_resource_type.clone();
        }
        eval_args
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct ShowArgs {
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// Show the given query
    #[arg(long, allow_hyphen_values = true, conflicts_with = "query_id")]
    pub inline: Option<String>,

    /// Query a dbt information schema view (e.g. `models`, `dag_nodes`) instead of
    /// the warehouse. Equivalent to `--inline "select * from {{ info_schema('<view>') }}"`.
    /// Requires a prior `dbt parse|compile|run|build --generate-info-schema`.
    #[arg(long, value_name = "VIEW", conflicts_with_all = ["inline", "adapter", "query_id"])]
    pub info: Option<String>,

    /// Fetch the result of a previously completed LakeCompute query directly,
    /// by query id, instead of compiling and running a query. No worker/compute
    /// round trip -- this reads the query's already-materialized result.
    /// Requires `--adapter lakecompute`.
    #[arg(long, conflicts_with_all = ["inline", "info"])]
    pub query_id: Option<String>,

    /// Select nodes of a specific type;
    #[arg(long, num_args(1..), value_delimiter = ' ', aliases = ["resource-types"], env = "DBT_RESOURCE_TYPES")]
    pub resource_type: Option<Vec<ClapResourceType>>,

    /// Exclude nodes of a specific type;
    #[arg(long, num_args(1..), value_delimiter = ' ', aliases = ["exclude-resource-types"], env = "DBT_EXCLUDE_RESOURCE_TYPES")]
    pub exclude_resource_type: Option<Vec<ClapResourceType>>,

    /// Limiting number of shown rows. Run with --limit -1 to remove limit [default: 10]
    #[arg(long, default_value=DEFAULT_LIMIT, allow_hyphen_values = true)]
    pub limit: RowLimit,

    /// Display rows in different formats
    #[arg(global = true, long, aliases = ["format"])]
    pub output: Option<DisplayFormat>,

    /// Flag to enable or disable SQL analysis, or to run SQL in unsafe mode, enabled by default
    #[arg(global = true, long, env = "DBT_STATIC_ANALYSIS")]
    pub static_analysis: Option<StaticAnalysisKind>,

    /// Do not perform any local type checking on the show target
    ///
    /// If this is set, any existing data in the remote warehouse will be
    /// displayed regardless of whether it matches the current state of the
    /// local workspace.
    ///
    /// @deprecated This is now the default behavior, command arg retained for
    /// backwards compatibility, will be removed in a future release.
    #[arg(long)]
    pub unchecked: bool,

    /// Use the samples as given in this YAML/JSON file.
    #[arg(
        long,
        value_name = "default|FILE",
        alias = "with-sample",
        help_heading = help_headings::SAMPLE
    )]
    pub with_sample: Option<String>,
    /// Add source selectors to sample (e.g., "source:raw.events"). Repeatable.
    #[arg(long, num_args(1..), value_delimiter = ' ', help_heading = help_headings::SAMPLE)]
    pub sampled: Vec<String>,

    /// Run against a non-default adapter the target declares (e.g. `lakecompute`),
    /// instead of the target's default adapter. Only meaningful for targets
    /// declaring more than one adapter, and only with `--inline`.
    #[arg(long)]
    pub adapter: Option<String>,
}

impl ShowArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::Show;
        eval_args.adapter_override = self.adapter.clone();
        eval_args.query_id = self.query_id.clone();
        if let Some(resource_type) = &self.resource_type {
            eval_args.resource_types = resource_type.clone();
        } else {
            eval_args.resource_types = vec![
                ClapResourceType::Model,
                ClapResourceType::Snapshot,
                ClapResourceType::Seed,
                ClapResourceType::Source,
                ClapResourceType::Analysis,
                ClapResourceType::Test,
            ];
        }
        eval_args.limit = self.limit.into();
        eval_args.static_analysis = self.static_analysis;
        eval_args.format = self.output.unwrap_or(DEFAULT_FORMAT);
        if let Some(exclude_resource_type) = &self.exclude_resource_type {
            eval_args.exclude_resource_types = exclude_resource_type.clone();
        }
        configure_run_cache(
            &mut eval_args,
            &self.common_args,
            false,
            &RunCacheMode::ReadWrite,
        );
        eval_args
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct SnapshotArgs {
    /// Snapshot the given nodes; same as --select node_1 ... node_n
    #[arg(value_parser = check_target)]
    pub node_targets: Vec<String>,

    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// Force node selection
    #[arg(long, default_value = "false")]
    pub force_node_selection: bool,

    /// The mode to use for dbt State. Cannot be used with --force-node-selection
    #[arg(
        long,
        default_value = "read-write",
        conflicts_with = "force_node_selection"
    )]
    pub run_cache_mode: RunCacheMode,

    /// Flag to enable or disable SQL analysis, or to run SQL in unsafe mode, enabled by default
    #[arg(global = true, long, env = "DBT_STATIC_ANALYSIS")]
    pub static_analysis: Option<StaticAnalysisKind>,
}

impl SnapshotArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);

        configure_run_cache(
            &mut eval_args,
            &self.common_args,
            self.force_node_selection,
            &self.run_cache_mode,
        );
        eval_args.resource_types = vec![ClapResourceType::Snapshot];
        eval_args.static_analysis = self.static_analysis;
        eval_args
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct TestArgs {
    ///Test the given nodes; same as --select node_1 ... node_n
    #[arg(value_parser = check_target)]
    pub node_targets: Vec<String>,

    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// Batch compatible generic tests.
    #[arg(long = "batch-tests", hide = true)]
    pub aggregate_tests: bool,

    /// Select nodes of a specific type;
    #[arg(long, num_args(1..), value_delimiter = ' ', aliases = ["resource-types"], env = "DBT_RESOURCE_TYPES")]
    pub resource_type: Option<Vec<ClapResourceType>>,

    /// Exclude nodes of a specific type;
    #[arg(long, num_args(1..), value_delimiter = ' ', aliases = ["exclude-resource-types"], env = "DBT_EXCLUDE_RESOURCE_TYPES")]
    pub exclude_resource_type: Option<Vec<ClapResourceType>>,

    /// Force node selection
    #[arg(long, default_value = "false")]
    pub force_node_selection: bool,

    /// The mode to use for dbt State. Cannot be used with --force-node-selection
    #[arg(
        long,
        default_value = "read-write",
        conflicts_with = "force_node_selection"
    )]
    pub run_cache_mode: RunCacheMode,

    /// Limiting number of shown rows. Run with --limit -1 to remove limit [default: 10]
    #[arg(long, default_value=DEFAULT_LIMIT, allow_hyphen_values = true)]
    pub limit: RowLimit,

    /// Display rows in different formats
    #[arg(global = true, long, aliases = ["format"])]
    pub output: Option<DisplayFormat>,

    /// Flag to enable or disable SQL analysis, or to run SQL in unsafe mode, enabled by default
    #[arg(global = true, long, env = "DBT_STATIC_ANALYSIS")]
    pub static_analysis: Option<StaticAnalysisKind>,

    /// Use the samples as given in this YAML/JSON file.
    #[arg(
        long,
        value_name = "default|FILE",
        alias = "with-sample",
        help_heading = help_headings::SAMPLE
    )]
    pub with_sample: Option<String>,
    /// Add source selectors to sample (e.g., "source:raw.events"). Repeatable.
    #[arg(long, num_args(1..), value_delimiter = ' ', help_heading = help_headings::SAMPLE)]
    pub sampled: Vec<String>,
}

impl TestArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        if self.aggregate_tests {
            eval_args
                .optimize_tests
                .insert(OptimizeTestsOptions::TestAggregation);
        }
        if let Some(resource_type) = &self.resource_type {
            eval_args.resource_types = resource_type.clone();
        } else {
            eval_args.resource_types = vec![ClapResourceType::Test, ClapResourceType::UnitTest];
        }
        if let Some(exclude_resource_type) = &self.exclude_resource_type {
            eval_args.exclude_resource_types = exclude_resource_type.clone();
        }
        configure_run_cache(
            &mut eval_args,
            &self.common_args,
            self.force_node_selection,
            &self.run_cache_mode,
        );
        eval_args.limit = self.limit.into();
        eval_args.static_analysis = self.static_analysis;
        eval_args.format = self.output.unwrap_or(DEFAULT_FORMAT);
        eval_args
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct BuildArgs {
    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// Select nodes of a specific type;
    #[arg(long, num_args(1..), value_delimiter = ' ', aliases = ["resource-types"], env = "DBT_RESOURCE_TYPES")]
    pub resource_type: Option<Vec<ClapResourceType>>,

    /// Exclude nodes of a specific type;
    #[arg(long, num_args(1..), value_delimiter = ' ', aliases = ["exclude-resource-types"], env = "DBT_EXCLUDE_RESOURCE_TYPES")]
    pub exclude_resource_type: Option<Vec<ClapResourceType>>,

    /// Batch compatible generic tests.
    #[arg(long = "batch-tests", hide = true)]
    pub aggregate_tests: bool,

    /// Skip data tests that are proven redundant.
    #[arg(long, hide = true)]
    pub skip_redundant_tests: bool,

    /// Force node selection
    #[arg(long, default_value = "false")]
    pub force_node_selection: bool,

    /// The mode to use for dbt State. Cannot be used with --force-node-selection
    #[arg(
        long,
        default_value = "read-write",
        conflicts_with = "force_node_selection"
    )]
    pub run_cache_mode: RunCacheMode,

    /// Drop incremental models and fully recalculate incremental tables.
    #[arg(global = true, long, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), short = 'f', env = "DBT_FULL_REFRESH")]
    pub full_refresh: bool,

    /// Skip parse-time project quality checks. Models still compile and run.
    #[arg(long, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new())]
    pub skip_checks: bool,

    /// Limiting number of shown rows. Run with --limit -1 to remove limit [default: 10]
    #[arg(long, default_value=DEFAULT_LIMIT, allow_hyphen_values = true)]
    pub limit: RowLimit,

    /// Display rows in different formats
    #[arg(global = true, long, aliases = ["format"])]
    pub output: Option<DisplayFormat>,

    /// Flag to enable or disable SQL analysis, or to run SQL in unsafe mode, enabled by default
    #[arg(global = true, long, env = "DBT_STATIC_ANALYSIS")]
    pub static_analysis: Option<StaticAnalysisKind>,

    /// Run models using time-based filters (only applicable to relations created via `ref` or `source`)
    /// reference: https://docs.getdbt.com/docs/build/sample-flag
    #[arg(global = true, long, hide = true, env = "DBT_SAMPLE")]
    pub sample: Option<String>,

    /// Use the samples as given in this YAML/JSON file.
    #[arg(
        long,
        value_name = "default|FILE",
        alias = "with-sample",
        help_heading = help_headings::SAMPLE
    )]
    pub with_sample: Option<String>,
    /// Add source selectors to sample (e.g., "source:raw.events"). Repeatable.
    #[arg(long, num_args(1..), value_delimiter = ' ', help_heading = help_headings::SAMPLE)]
    pub sampled: Vec<String>,
}

impl BuildArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        if self.aggregate_tests {
            eval_args
                .optimize_tests
                .insert(OptimizeTestsOptions::TestAggregation);
        }
        if self.skip_redundant_tests {
            eval_args
                .optimize_tests
                .insert(OptimizeTestsOptions::TestStaticAnalysis);
        }
        eval_args.phase = Phases::All;
        // Enable task cache
        if let Some(resource_type) = &self.resource_type {
            eval_args.resource_types = resource_type.clone();
        } else {
            eval_args.resource_types = vec![
                ClapResourceType::Model,
                ClapResourceType::Seed,
                ClapResourceType::Snapshot,
                ClapResourceType::Test,
                ClapResourceType::UnitTest,
                ClapResourceType::Function,
                ClapResourceType::Check,
            ];
            if eval_args.export_saved_queries {
                eval_args.resource_types.push(ClapResourceType::SavedQuery);
            }
        }
        if let Some(exclude_resource_type) = &self.exclude_resource_type {
            eval_args.exclude_resource_types = exclude_resource_type.clone();
        }
        configure_run_cache(
            &mut eval_args,
            &self.common_args,
            self.force_node_selection,
            &self.run_cache_mode,
        );
        eval_args.full_refresh = self.full_refresh;
        eval_args.static_analysis = self.static_analysis;
        eval_args.empty = self.common_args.empty;
        eval_args.sample = self.sample.clone();
        eval_args.skip_checks = self.skip_checks;
        eval_args
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct ListArgs {
    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// Limiting number of shown rows. Run with --limit -1 to remove limit [default: 10]
    // todo: still l;eft to be implemented..
    #[arg(long,default_value=DEFAULT_LIMIT, allow_hyphen_values = true, hide = true)]
    pub limit: RowLimit,

    /// Output format: either JSON or a newline-delimited list of selectors, paths, or names
    #[arg(global = true, long, aliases = ["format"], default_value = "selector")]
    pub output: ListOutputFormat,

    /// Space-delimited listing of node properties to include as custom keys for JSON output
    /// (e.g. `--output json --output-keys name resource_type description`)
    #[arg(long, num_args(1..), value_delimiter = ' ')]
    pub output_keys: Vec<String>,

    /// Select nodes of a specific type;
    #[arg(long, num_args(1..), value_delimiter = ' ', aliases = ["resource-types"], env = "DBT_RESOURCE_TYPES")]
    pub resource_type: Option<Vec<ClapResourceType>>,

    /// Exclude nodes of a specific type;
    #[arg(long, num_args(1..), value_delimiter = ' ', aliases = ["exclude-resource-types"], env = "DBT_EXCLUDE_RESOURCE_TYPES")]
    pub exclude_resource_type: Option<Vec<ClapResourceType>>,
}

impl ListArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::List;
        eval_args.io.show.insert(ShowOptions::Nodes);
        eval_args.output_keys = self.output_keys.clone();
        if let Some(resource_type) = &self.resource_type {
            eval_args.resource_types = resource_type.clone();
        }
        if let Some(exclude_resource_type) = &self.exclude_resource_type {
            eval_args.exclude_resource_types = exclude_resource_type.clone();
        }
        eval_args.limit = self.limit.into();
        // Convert ListOutputFormat to DisplayFormat for EvalArgs
        eval_args.format = DisplayFormat::from(self.output);
        eval_args
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct RunArgs {
    // Flattened IO args
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// Force node selection
    #[arg(long, default_value = "false")]
    pub force_node_selection: bool,

    /// The mode to use for dbt State. Cannot be used with --force-node-selection
    #[arg(
        long,
        default_value = "read-write",
        conflicts_with = "force_node_selection"
    )]
    pub run_cache_mode: RunCacheMode,

    /// Limiting number of shown rows. Run with --limit -1 to remove limit [default: 10]
    #[arg(long, default_value=DEFAULT_LIMIT, allow_hyphen_values = true)]
    pub limit: RowLimit,

    /// Display rows in different formats
    #[arg(global = true, long, aliases = ["format"])]
    pub output: Option<DisplayFormat>,

    /// Flag to enable or disable SQL analysis, or to run SQL in unsafe mode, enabled by default
    #[arg(global = true, long, env = "DBT_STATIC_ANALYSIS")]
    pub static_analysis: Option<StaticAnalysisKind>,

    /// Drop incremental models and fully recalculate incremental tables.
    #[arg(global = true, long, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), short = 'f', env = "DBT_FULL_REFRESH")]
    pub full_refresh: bool,

    /// Sample mode generates time-based filtered refs and sources
    #[arg(global = true, long, hide = true, env = "DBT_SAMPLE")]
    pub sample: Option<String>,

    /// Use the samples as given in this YAML/JSON file.
    #[arg(
        long,
        value_name = "default|FILE",
        alias = "with-sample",
        help_heading = help_headings::SAMPLE
    )]
    pub with_sample: Option<String>,
    /// Add source selectors to sample (e.g., "source:raw.events"). Repeatable.
    #[arg(long, num_args(1..), value_delimiter = ' ', help_heading = help_headings::SAMPLE)]
    pub sampled: Vec<String>,
}

impl RunArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::All;

        configure_run_cache(
            &mut eval_args,
            &self.common_args,
            self.force_node_selection,
            &self.run_cache_mode,
        );

        eval_args.resource_types = vec![ClapResourceType::Model];
        eval_args.limit = self.limit.into();
        eval_args.static_analysis = self.static_analysis;
        eval_args.full_refresh = self.full_refresh;
        eval_args.empty = self.common_args.empty;
        eval_args.sample = self.sample.clone();
        // Optional sampling plan (for local runs to locate sampled data)
        // if let Some(ss) = &self.with_sample {
        //     let plan = normalize_sample_plan_or_exit::<RunArgs>(&Some(ss.clone()));
        //     eval_args.sample_plan = Some(plan.to_macro_json().expect("Sampling plan to be serializable"));
        // } else if !self.sampled.is_empty() {
        //     let entries: Vec<serde_json::Value> = self
        //         .sampled
        //         .iter()
        //         .map(|p| Entry {
        //             select: Some(p.to_string()),
        //             strategy: Strategy::Clone , ..Default::default()
        //             })
        //         .collect();
        //     let plan = Plan { entries };
        //     eval_args.sample_plan = Some(serde_json::json!({ "entries": entries }));
        // }
        eval_args.format = self.output.unwrap_or(DEFAULT_FORMAT);
        eval_args
    }
}

fn configure_run_cache(
    eval_args: &mut EvalArgs,
    common_args: &CommonArgs,
    force_node_selection: bool,
    run_cache_mode: &RunCacheMode,
) {
    if common_args.no_manage_state {
        eval_args.run_cache_service = false;
        return;
    }

    let manage_state = common_args.get_manage_state(&eval_args.io.in_dir, false);
    if common_args.task_cache_url != NOOP || manage_state {
        eval_args.run_cache_service = manage_state;
        eval_args.run_cache_mode = if force_node_selection {
            RunCacheMode::WriteOnly
        } else {
            run_cache_mode.clone()
        };
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct CloneArgs {
    // Flattened IO args
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// Drop existing models and clone all tables.
    #[arg(global = true, long, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), short = 'f', env = "DBT_FULL_REFRESH")]
    pub full_refresh: bool,
}

impl CloneArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::All;

        eval_args.full_refresh = self.full_refresh;

        eval_args.run_cache_mode = RunCacheMode::Noop;
        eval_args.resource_types = vec![
            ClapResourceType::Model,
            ClapResourceType::Seed,
            ClapResourceType::Snapshot,
        ];
        eval_args.format = DEFAULT_FORMAT;

        eval_args
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct RunOperationArgs {
    #[arg(id = "MACRO", conflicts_with = "sql", required_unless_present = "sql")]
    pub macro_name: Option<String>,

    /// Supply arguments to the macro. This dictionary will be mapped to the keyword arguments defined in the selected macro. This argument should be a yml string.
    #[arg(long, value_parser = check_key_value_cli_arg, conflicts_with = "sql")]
    pub args: Option<BTreeMap<String, YValue>>,

    /// Execute an inline SQL/Jinja statement directly against the target database.
    /// Mutually exclusive with the named-macro form and `--args`.
    #[arg(long, conflicts_with_all = ["MACRO", "args"])]
    pub sql: Option<String>,

    /// Run against a non-default adapter the target declares (e.g. `lakecompute`),
    /// instead of the target's default adapter. Only meaningful for targets
    /// declaring more than one adapter.
    #[arg(long)]
    pub adapter: Option<String>,

    // Flattened IO args
    #[clap(flatten)]
    pub common_args: CommonArgs,
}

impl RunOperationArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::RunOperation;
        eval_args.macro_name = self.macro_name.clone();
        eval_args.macro_sql = self.sql.clone();
        eval_args.adapter_override = self.adapter.clone();
        if let Some(args) = &self.args {
            eval_args.macro_args = args.clone();
        }

        eval_args
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct ManArgs {
    // Flattened IO args
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// Show the post-Jinja-transformation form of JSON schema for the specified
    /// types on the command line. Repeatable to show multiple schema types.
    #[clap(long, num_args(0..))]
    pub schema: Vec<ClapSchemaTypes>,

    /// Show the pre-Jinja-transformation form of JSON schema for the specified
    /// types on the command line. Repeatable to show multiple schema types.
    #[clap(long, num_args(0..))]
    pub pre_schema: Vec<ClapSchemaTypes>,
}
// dbt man --schema selector --schema project

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct LoginArgs {
    #[clap(flatten)]
    pub common_args: CommonArgs,

    #[command(subcommand)]
    pub subcommand: Option<LoginSubcommand>,
}

impl LoginArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        self.common_args.to_eval_args(arg, in_dir, out_dir)
    }
}

#[derive(clap::Subcommand, Debug, Clone, Serialize, Deserialize)]
pub enum LoginSubcommand {
    /// Show current authentication status
    Status,
}

/// Undocumented plumbing commands, not intended for direct end-user use.
#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct InternalArgs {
    #[command(subcommand)]
    pub command: InternalCommand,
}

impl InternalArgs {
    pub fn common_args(&self) -> &CommonArgs {
        match &self.command {
            InternalCommand::GetDistributionInfo(args) => &args.common_args,
        }
    }

    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        self.common_args().to_eval_args(arg, in_dir, out_dir)
    }
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
#[command()]
pub enum InternalCommand {
    /// Resolve release-channel and distribution info for a dbt installation
    GetDistributionInfo(GetDistributionInfoArgs),
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct GetDistributionInfoArgs {
    /// Path to a dbt executable to inspect. Defaults to the currently running
    /// process. Mutually exclusive with `--all`.
    #[arg(value_name = "PATH", conflicts_with = "all")]
    pub path: Option<PathBuf>,

    /// Print distribution info for every dbt executable found on PATH.
    /// Mutually exclusive with the PATH argument.
    #[arg(short = 'a', long, conflicts_with = "path")]
    pub all: bool,

    #[clap(flatten)]
    pub common_args: CommonArgs,
}

/// Args for `dbt system upgrade-distribution`. No `common_args` flatten:
/// this is a `System` subcommand, and `SystemMgmtArgs` already flattens
/// `CommonArgs` once (same precedent as `SystemUpdateArgs`/`SystemUninstallArgs`).
#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct SystemUpgradeDistributionArgs {
    /// Skip the confirmation prompt and proceed automatically.
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// For a managed-project upgrade, the Python package manager to sync
    /// the rewritten manifest with (e.g. `uv`, `poetry`, `pdm`, `pipenv`,
    /// `conda`, `hatch`, `pip`, `pipx`, `rye`, `asdf`, `mise`, `pyenv`).
    /// Overrides automatic detection -- use this to answer non-interactively
    /// when detection can't determine the manager on its own (it would
    /// otherwise prompt for one interactively, or fail if run with --yes or
    /// outside a terminal).
    #[arg(long)]
    pub package_manager: Option<String>,
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct StateArgs {
    #[clap(flatten)]
    pub common_args: CommonArgs,

    #[command(subcommand)]
    pub subcommand: StateSubcommand,
}

impl StateArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        self.common_args.to_eval_args(arg, in_dir, out_dir)
    }
}

#[derive(clap::Subcommand, Debug, Clone, Serialize, Deserialize)]
pub enum StateSubcommand {
    Explain(StateExplainArgs),
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct StateExplainArgs {
    #[arg(short, long)]
    pub verbose: bool,

    #[arg(short = 'l', long = "log-file")]
    pub log_file: Option<PathBuf>,
}

impl ManArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.set_schema(
            self.schema
                .iter()
                .map(|s| s.to_json_schema_types(false))
                .chain(self.pre_schema.iter().map(|s| s.to_json_schema_types(true)))
                .collect(),
        )
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct DocsArgs {
    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,

    #[command(subcommand)]
    pub subcommand: Option<DocsSubcommand>,
}

impl DocsArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        self.common_args.to_eval_args(arg, in_dir, out_dir)
    }
}

#[derive(clap::Subcommand, Debug, Clone, Serialize, Deserialize)]
pub enum DocsSubcommand {
    /// Generate a self-contained, statically hostable docs site.
    ///
    /// Compiles the project, then writes the site that compile describes to
    /// `--output-dir`. The result is a plain directory of files: host it anywhere,
    /// no server process required.
    ///
    /// Pass `--no-compile` to skip the compile and export an existing index,
    /// which is then an error if there is none.
    Generate(DocsGenerateArgs),
    /// Start the dbt docs v2 server backed by parquet artifacts in the target directory.
    ///
    /// Reads parquet artifacts from a previous `build` / `run` / `docs generate`
    /// and exposes a local HTTP server. Does not require a dbt project.
    Serve(DocsServeArgs),
    #[command(external_subcommand)]
    Other(Vec<String>),
}

#[derive(Parser, Debug, Clone, Serialize, Deserialize)]
pub struct DocsServeArgs {
    /// Path to the dbt target directory containing the `private/index/` subdirectory of
    /// parquet artifacts. Defaults to the project's target directory, resolved from
    /// `--project-dir` and `--target-path` like any other project command.
    #[arg(long, value_name = "DIR", env = "DBT_DOCS_TARGET_PATH")]
    pub target_path: Option<PathBuf>,

    /// Host to bind the HTTP server to.
    #[arg(long, default_value = "127.0.0.1", env = "DBT_DOCS_HOST")]
    pub host: String,

    /// Port to listen on.
    #[arg(long, default_value_t = 8580, env = "DBT_DOCS_PORT")]
    pub port: u16,

    /// Don't auto-open a browser tab on startup.
    #[arg(long, default_value_t = false)]
    pub no_open: bool,
}

impl Default for DocsServeArgs {
    fn default() -> Self {
        Self {
            target_path: None,
            host: "127.0.0.1".to_string(),
            port: 8580,
            no_open: false,
        }
    }
}

/// Args for `dbt docs generate`.
///
/// Mirrors [`DocsServeArgs`] on `--target-path`, including the env var, so both
/// docs subcommands locate the index identically. `generate` compiles and
/// exports the index that compile produces, so it needs a project directory
/// and a warehouse connection — unless `--no-compile` reduces it to the
/// exporter it used to be.
#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct DocsGenerateArgs {
    /// Path to the dbt target directory containing the `private/index/` subdirectory of
    /// parquet artifacts. Defaults to `./target` in the current working directory.
    #[arg(long, value_name = "DIR", env = "DBT_DOCS_TARGET_PATH")]
    pub target_path: Option<PathBuf>,

    /// Directory to write the static site to. Defaults to the target directory
    /// itself, so `index.html` lands where core v1 wrote it.
    #[arg(long, value_name = "DIR", env = "DBT_DOCS_OUTPUT_DIR")]
    pub output_dir: Option<PathBuf>,

    /// Base URL the browser loads DuckDB-WASM from at runtime.
    ///
    /// Defaults to the pinned jsDelivr path. Point this at a mirror to serve the
    /// wasm from your own infrastructure; the site never bundles it.
    #[arg(long, value_name = "URL", env = "DBT_DOCS_DUCKDB_CDN_BASE")]
    pub duckdb_cdn_base: Option<String>,

    /// Export only what is already indexed; never compile.
    ///
    /// Without this, a compile runs first, so the site always reflects the
    /// project as it is now. With it, the export reads the index a previous
    /// `build` / `run` / `docs generate` wrote and a missing one is an error
    /// naming that remedy — useful where the warehouse is unreachable, or to
    /// keep the command cheap and read-only.
    #[arg(long, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), env = "DBT_DOCS_NO_COMPILE", overrides_with = "compile")]
    pub no_compile: bool,

    /// Accepted for dbt v1 compatibility, where it was the default. Compiling is
    /// the default here too, so this is a no-op; it exists so v1 scripts that pass
    /// it keep working.
    #[arg(long, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), hide = true, overrides_with = "no_compile")]
    pub compile: bool,
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct DebugArgs {
    // Flattened IO args
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// When set, skip any non-connection related debug steps
    #[arg(long, default_value_t = false)]
    pub connection: bool,
}

impl DebugArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::Debug;
        eval_args.set_connection(self.connection)
    }
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct RetryArgs {
    // Flattened Common args (includes --state global flag)
    #[clap(flatten)]
    pub common_args: CommonArgs,

    /// Override static analysis setting for retry. If not specified, uses the setting from the
    /// original run (if available) or defaults to "on".
    #[arg(long, env = "DBT_STATIC_ANALYSIS")]
    pub static_analysis: Option<StaticAnalysisKind>,
}

impl RetryArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::All;
        // Note: static_analysis is applied later in retry handling based on original run settings
        eval_args.static_analysis = self.static_analysis;
        eval_args
    }
}

// reference: https://docs.getdbt.com/reference/global-configs/about-global-configs
#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct CommonArgs {
    /// The target to execute
    #[arg(
        global = true,
        short = 't',
        long,
        env = "DBT_TARGET",
        help_heading = help_headings::PROJECT,
        hide_short_help = true
    )]
    pub target: Option<String>,

    /// The directory to load the dbt project from
    #[arg(
        global = true,
        long,
        env = "DBT_PROJECT_DIR",
        help_heading = help_headings::PROJECT,
        hide_short_help = true
    )]
    pub project_dir: Option<PathBuf>,

    /// The profile to use
    #[arg(
        global = true,
        long,
        env = "DBT_PROFILE",
        help_heading = help_headings::PROJECT,
        hide_short_help = true
    )]
    pub profile: Option<String>,

    /// The directory to load the profiles from
    #[arg(
        global = true,
        long,
        env = "DBT_PROFILES_DIR",
        help_heading = help_headings::PROJECT,
        hide_short_help = true
    )]
    pub profiles_dir: Option<PathBuf>,

    /// The directory to install packages
    #[arg(
        global = true,
        long,
        env = "DBT_PACKAGES_INSTALL_PATH",
        help_heading = help_headings::PROJECT,
        hide_short_help = true
    )]
    pub packages_install_path: Option<PathBuf>,

    /// The output directory for all produced assets
    #[arg(
        global = true,
        long,
        env = "DBT_TARGET_PATH",
        help_heading = help_headings::PROJECT,
        hide_short_help = true
    )]
    pub target_path: Option<PathBuf>,

    /// Supply var bindings in yml format e.g. '{key: value}' or as separate key: value pairs
    // has no ENV_VAR
    #[arg(global = true, long, value_parser = check_key_value_cli_arg_with_recovery, help_heading = help_headings::PROJECT, hide_short_help = true)]
    pub vars: Option<BTreeMap<String, YValue>>,

    /// The AI coding agent(s) to install package skills for, e.g. 'claude'.
    /// Accepts a comma-separated list. Skills are only installed when this is set.
    #[arg(
        global = true,
        long,
        env = "DBT_AI_PROVIDER",
        num_args(1..),
        value_delimiter = ',',
        help_heading = help_headings::PROJECT,
        hide_short_help = true
    )]
    pub ai_provider: Option<Vec<String>>,

    /// Select nodes to run
    // has no ENV_VAR
    #[arg(global = true, long, short = 's', value_parser = check_selector, num_args(1..), value_delimiter = ' ', group = "selector_or_select", help_heading = help_headings::SELECTION, hide_short_help = true)]
    // This is a deprecated legacy alias for '--select'. It is not visible in the help and should be removed (eventually).
    #[clap(alias("models"), short_alias('m'))]
    pub select: Option<Vec<String>>,

    /// Select nodes to exclude
    // has no ENV_VAR
    #[arg(global = true, long, value_parser = check_selector, num_args(1..), value_delimiter = ' ', help_heading = help_headings::SELECTION, hide_short_help = true)]
    pub exclude: Option<Vec<String>>,

    /// The name of the yml defined selector to use
    #[arg(
        global = true,
        long,
        group = "selector_or_select",
        help_heading = help_headings::SELECTION,
        hide_short_help = true
    )]
    pub selector: Option<String>,

    /// Choose which tests to select adjacent to resources: eager (most inclusive), cautious (most exclusive), buildable (inbetween) or empty.
    #[arg(
        global = true,
        long,
        env = "DBT_INDIRECT_SELECTION",
        help_heading = help_headings::SELECTION,
        hide_short_help = true
    )]
    pub indirect_selection: Option<IndirectSelection>,

    /// Suppress all non-error logging to stdout. Does not affect {{ print() }} macro calls.
    #[arg(global = true, long, env = "DBT_QUIET", short = 'q', default_value = "false", action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), help_heading = help_headings::LOGGING, hide_short_help = true)]
    pub quiet: bool,
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), hide = true)]
    pub no_quiet: bool,

    /// The number of threads to use [Run with --threads 0 to use max_cpu [default: max_cpu]]
    // has no ENV_VAR
    #[arg(
        global = true,
        long,
        help_heading = help_headings::EXECUTION,
        hide_short_help = true
    )]
    pub threads: Option<usize>,

    /// Force sequential task execution and sequential parser rendering. Does
    /// not affect the adapter connection pool — use `--threads` for that.
    /// Hidden because it is primarily a test/debug knob.
    #[arg(global = true, long = "no-parallel", action = ArgAction::SetTrue, env = "DBT_NO_PARALLEL", value_parser = BoolishValueParser::new(), hide = true)]
    pub no_parallel: bool,

    /// Execution backend to use
    #[arg(
        global = true,
        long = "compute",
        value_enum,
        env = "DBT_COMPUTE",
        default_value = "remote",
        help_heading = help_headings::EXECUTION,
        hide_short_help = true
    )]
    pub compute: ComputeArg,

    /// Host address
    #[arg(
        long,
        default_value = "127.0.0.1",
        value_name = "HOST",
        help_heading = help_headings::EXECUTION,
        hide_short_help = true
    )]
    pub host: String,

    /// Port number
    #[arg(
        long,
        default_value_t = 8000,
        value_name = "PORT",
        help_heading = help_headings::EXECUTION,
        hide_short_help = true
    )]
    pub port: u16,

    /// Warn on error
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, env = "DBT_WARN_ERROR",hide = true, value_parser = BoolishValueParser::new())]
    pub warn_error: bool,
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, hide = true, value_parser = BoolishValueParser::new())]
    pub no_warn_error: bool,

    /// Warning error options
    #[arg(global = true, long, value_parser = parse_warn_error_options,
        env = "DBT_WARN_ERROR_OPTIONS",
        hide = true )]
    pub warn_error_options: Option<WarnErrorOptions>,

    // TODO: currently only used to avoid suppressing warnings/errors from dependencies
    /// Show all deprecations warnings/errors instead of one per package
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, env = "DBT_SHOW_ALL_DEPRECATIONS",hide = true, value_parser = BoolishValueParser::new())]
    pub show_all_deprecations: bool,

    /// Display debug logging during dbt execution. Useful for debugging and making bug reports.
    #[arg(global = true, long, short = 'd', default_value = "false", action = ArgAction::SetTrue, env = "DBT_DEBUG", value_parser = BoolishValueParser::new(), help_heading = help_headings::LOGGING, hide_short_help = true)]
    pub debug: bool,
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, env = "DBT_DEBUG", value_parser = BoolishValueParser::new(), hide = true)]
    pub no_debug: bool,

    /// Introspect flag
    #[arg(global = true, long,  default_value = "false", action = ArgAction::SetTrue,  env = "DBT_INTROSPECT", value_parser = BoolishValueParser::new(),hide = true)]
    pub introspect: bool,

    /// Seed classifier propagation with column tags pulled from the Snowflake
    /// warehouse before the Analyze phase, and write the propagated labels
    /// back as column tags on materialized downstream tables/views after Run.
    /// Only valid for the Snowflake adapter, and requires --write-index.
    #[arg(global = true, long = "classify-with-warehouse-tags", default_value_t = false, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), hide = true)]
    pub classify_with_warehouse_tags: bool,

    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(),hide = true)]
    pub no_introspect: bool,

    /// Write JSON artifacts to disk [env: DBT_WRITE_JSON=]. Use --no-write-json to suppress writing JSON artifacts.
    #[arg(global = true, long, default_value_t=true, action = ArgAction::SetTrue, env = "DBT_WRITE_JSON", value_parser = BoolishValueParser::new(), help_heading = help_headings::ARTIFACTS, hide_short_help = true)]
    pub write_json: bool,
    #[arg(global = true,long,action = ArgAction::SetTrue,  default_value_t=false, value_parser = BoolishValueParser::new(),hide = true)]
    pub no_write_json: bool,

    /// Write a catalog.json file to the target directory
    #[arg(global = true, long, default_value_t=false, action = ArgAction::SetTrue, env = "DBT_WRITE_CATALOG", value_parser = BoolishValueParser::new(), help_heading = help_headings::ARTIFACTS, hide_short_help = true)]
    pub write_catalog: bool,

    /// Enable full metadata output: incremental parse cache, epoch parquet state, and no JSON
    /// artifacts. Implies --partial-parse --no-write-json.
    ///
    /// Not user-facing: kept for backwards compatibility with programmatic callers.
    #[arg(global = true, long, default_value_t=false, action = ArgAction::SetTrue, env = "DBT_WRITE_METADATA", value_parser = BoolishValueParser::new(), hide = true)]
    pub write_metadata: bool,

    /// Internal engine index at target/private/index/. Not a user API — `build` / `run` /
    /// `check` write it by default. Hidden; still accepted for programmatic callers.
    #[arg(global = true, long = "write-index", alias = "use-index", default_value_t=false, action = ArgAction::SetTrue, env = "DBT_USE_INDEX", value_parser = BoolishValueParser::new(), hide = true)]
    pub write_index: bool,
    /// Opt out of the index that `build` writes by default. `--write-index` is `SetTrue` with
    /// no negation, so without this flag defaulting it on would leave no way back.
    /// Hidden; still accepted for existing scripts.
    #[arg(global = true, long, action = ArgAction::SetTrue, default_value_t=false, value_parser = BoolishValueParser::new(), hide = true)]
    pub no_write_index: bool,

    /// Directory for metadata parquet output (default: <target>/private/metadata/)
    #[arg(
        global = true,
        long,
        env = "DBT_METADATA_DIR",
        help_heading = help_headings::ARTIFACTS,
        hide_short_help = true
    )]
    pub metadata_dir: Option<PathBuf>,

    /// Directory for index parquet output (default: <target>/private/index/)
    #[arg(global = true, long, env = "DBT_INDEX_DIR", hide = true)]
    pub index_dir: Option<PathBuf>,

    /// Write the dbt information schema to target/info_schema/: a queryable
    /// parquet layer over your project's metadata. With --static-analysis strict,
    /// also writes column types and column-level lineage.
    #[arg(global = true, long = "generate-info-schema", default_value_t=false, action = ArgAction::SetTrue, env = "DBT_GENERATE_INFO_SCHEMA", value_parser = BoolishValueParser::new(), help_heading = help_headings::ARTIFACTS)]
    pub generate_info_schema: bool,

    /// Directory for information schema parquet output (default: <target>/info_schema/)
    #[arg(
        global = true,
        long,
        env = "DBT_INFO_SCHEMA_DIR",
        help_heading = help_headings::ARTIFACTS,
        hide_short_help = true
    )]
    pub info_schema_dir: Option<PathBuf>,

    /// Compute and write column-level lineage into compile/cll parquet.
    ///
    /// Hidden. Auto-enabled when `--generate-info-schema` (or the hidden
    /// `--write-index`) is combined with `--static-analysis strict`.
    #[arg(global = true, long, default_value_t=false, action = ArgAction::SetTrue, env = "DBT_WRITE_LINEAGE", value_parser = BoolishValueParser::new(), hide = true)]
    pub write_lineage: bool,

    // Support for query cache
    #[arg(global = true, long, env = "DBT_BETA_USE_QUERY_CACHE", hide = true)]
    pub beta_use_query_cache: bool,

    //
    // NOTE: The arguments below were generated by a script to temporarily fill gaps between fs and
    // dbt cli parsing. They may not actually be implemented, yet. If you implement them, move them
    // above this comment. Script at: https://github.com/dbt-labs/dbt-mantle/blob/7fff1e9b9ed1203454447e68cf298be788255d9f/scripts/cli-click-to-clap.py
    //
    // At start of run, populate relational cache only for schemas containing selected nodes, or for all schemas of interest.
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, env = "DBT_CACHE_SELECTED_ONLY", value_parser = BoolishValueParser::new(),hide = true)]
    pub cache_selected_only: bool,
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, env = "DBT_CACHE_ALL_SCHEMAS", value_parser = BoolishValueParser::new(),hide = true)]
    pub no_cache_selected_only: bool,

    /// Skip writing msgpack files if they already exist, deprecated
    #[arg(global = true, long = "skip-write-msgpack-if-exist", action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), hide = true)]
    pub skip_write_msgpack_if_exist: bool,

    // If set, resolve unselected nodes by deferring to the manifest within the --state directory.
    #[arg(global = true, long = "defer", action = ArgAction::SetTrue, env = "DBT_DEFER", value_parser = BoolishValueParser::new(), help_heading = help_headings::EXECUTION, hide_short_help = true)]
    pub defer: bool,
    #[arg(global = true, long= "no-defer", default_value_t=false, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), hide = true)]
    pub no_defer: bool,

    /// Override the state directory for deferral only.
    #[arg(global = true, long, env = "DBT_DEFER_STATE", hide = true)]
    pub defer_state: Option<PathBuf>,

    /// Unless overridden, use this state directory for both state comparison and deferral.
    #[arg(
        global = true,
        long,
        env = "DBT_STATE",
        help_heading = help_headings::EXECUTION,
        hide_short_help = true
    )]
    pub state: Option<PathBuf>,

    // Stop execution on first failure.
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, env = "DBT_FAIL_FAST", short = 'x', value_parser = BoolishValueParser::new(),hide = true)]
    pub fail_fast: bool,
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, env = "DBT_NO_FAIL_FAST", value_parser = BoolishValueParser::new(),hide = true)]
    pub no_fail_fast: bool,

    // If set, defer to the argument provided to the state flag for resolving unselected nodes, even if the node(s) exist as a database object in the current environment.
    #[arg(global = true, long = "favor-state", default_value = "false",action = ArgAction::SetTrue, env = "DBT_FAVOR_STATE", value_parser = BoolishValueParser::new(),hide = true)]
    pub favor_state: bool,
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, env = "DBT_NO_FAVOR_STATE", value_parser = BoolishValueParser::new(),hide = true)]
    pub no_favor_state: bool,

    // Enable verbose logging for relational cache events to help when debugging.
    #[arg(global = true, long = "log-cache-events", default_value = "false", action = ArgAction::SetTrue, env = "DBT_LOG_CACHE_EVENTS", value_parser = BoolishValueParser::new(),hide = true)]
    pub log_cache_events: bool,
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, env = "DBT_NO_LOG_CACHE_EVENTS", value_parser = BoolishValueParser::new(),hide = true)]
    pub no_log_cache_events: bool,

    // logging
    //
    /// Set 'log-path' for the current run, overriding 'DBT_LOG_PATH'.
    #[arg(
        global = true,
        long,
        env = "DBT_LOG_PATH",
        help_heading = help_headings::LOGGING,
        hide_short_help = true
    )]
    pub log_path: Option<PathBuf>,

    /// Set 'otel-file-name' for the current run, overriding 'DBT_OTEL_FILE_NAME'.
    /// If set, OTEL telemetry will be written to `$log_path/otel-file-name`.
    #[arg(
        global = true,
        long = "otel-file-name",
        env = "DBT_OTEL_FILE_NAME",
        help_heading = help_headings::LOGGING,
        hide_short_help = true
    )]
    pub otel_file_name: Option<String>,

    /// Set 'otel-parquet-file-name' for the current run, overriding 'DBT_OTEL_PARQUET_FILE_NAME'.
    /// If set, OTEL telemetry will be written to `$target_path/private/metadata/otel-parquet-file-name` in Parquet format.
    #[arg(
        global = true,
        long = "otel-parquet-file-name",
        env = "DBT_OTEL_PARQUET_FILE_NAME",
        help_heading = help_headings::LOGGING,
        hide_short_help = true
    )]
    pub otel_parquet_file_name: Option<String>,

    /// Configure the max file size in bytes for a single dbt.log file, before rolling over. 0 means no limit.
    #[arg(global = true, long, default_value_t = DBT_DEFAULT_LOG_FILE_MAX_BYTES, env = "DBT_LOG_FILE_MAX_BYTES", hide = true)]
    pub log_file_max_bytes: u64,

    /// Set logging format; use --log-format-file to override.
    #[arg(global = true, long, env = "DBT_LOG_FORMAT", default_value_t = LogFormat::Default, help_heading = help_headings::LOGGING, hide_short_help = true)]
    pub log_format: LogFormat,

    /// Set log file format, overriding the default and --log-format setting.
    #[arg(
        global = true,
        long,
        env = "DBT_LOG_FORMAT_FILE",
        help_heading = help_headings::LOGGING,
        hide_short_help = true
    )]
    pub log_format_file: Option<LogFormat>,

    /// Set minimum severity for console/log file; use --log-level-file to set log file severity separately.
    #[arg(
        global = true,
        long,
        env = "DBT_LOG_LEVEL",
        ignore_case = true,
        help_heading = help_headings::LOGGING,
        hide_short_help = true
    )]
    pub log_level: Option<LogLevel>,
    /// Set minimum log file severity, overriding the default and --log-level setting.
    #[arg(
        global = true,
        long,
        env = "DBT_LOG_LEVEL_FILE",
        ignore_case = true,
        help_heading = help_headings::LOGGING,
        hide_short_help = true
    )]
    pub log_level_file: Option<LogLevel>,

    #[arg(global = true, long, default_value_t = false, action = ArgAction::SetTrue, env = "DBT_MACRO_DEBUGGING", value_parser = BoolishValueParser::new(),hide = true)]
    pub macro_debugging: bool,
    #[arg(global = true, long, default_value_t = false, action = ArgAction::SetTrue, env = "DBT_MACRO_DEBUGGING", value_parser = BoolishValueParser::new(),hide = true)]
    pub no_macro_debugging: bool,

    // Allow for partial parsing by looking for and writing to a pickle file in the target directory. This overrides the user configuration file.
    #[arg(global = true, long , default_value_t = false,  action = ArgAction::SetTrue, env = "DBT_PARTIAL_PARSE", value_parser = BoolishValueParser::new(), hide = true)]
    pub partial_parse: bool,
    #[arg(global = true, long, default_value_t = false,  action = ArgAction::SetTrue, env = "DBT_PARTIAL_PARSE", value_parser = BoolishValueParser::new(), hide = true)]
    pub no_partial_parse: bool,

    #[arg(global = true, long ,default_value_t = false, action = ArgAction::SetTrue, env = "DBT_PARTIAL_PARSE_FILE_DIFF", value_parser = BoolishValueParser::new(), hide = true)]
    pub partial_parse_file_diff: bool,
    #[arg(global = true, long,default_value_t = false, action = ArgAction::SetTrue, env = "DBT_PARTIAL_PARSE_FILE_DIFF", value_parser = BoolishValueParser::new(), hide = true)]
    pub no_partial_parse_file_diff: bool,

    #[arg(global = true, long, env = "DBT_PARTIAL_PARSE_FILE_PATH", hide = true)]
    pub partial_parse_file_path: Option<PathBuf>,

    #[arg(global = true, long, default_value_t = false, action = ArgAction::SetTrue, env = "DBT_PARTIAL_LOAD", value_parser = BoolishValueParser::new(), hide = true)]
    pub partial_load: bool,
    #[arg(global = true, long, default_value_t = false, action = ArgAction::SetTrue, env = "DBT_PARTIAL_LOAD", value_parser = BoolishValueParser::new(), hide = true)]
    pub no_partial_load: bool,

    /// Select only nodes whose source files have changed since the last --partial-parse run,
    /// plus all their downstream dependents. Implies --partial-parse.
    /// Use with --partial-load for full speed: --partial-load --dirty
    #[arg(global = true, long, default_value_t = false, action = ArgAction::SetTrue, env = "DBT_DIRTY", value_parser = BoolishValueParser::new(), help_heading = help_headings::EXECUTION, hide_short_help = true)]
    pub dirty: bool,

    #[arg(global = true, long, default_value_t = false, action = ArgAction::SetTrue, env = "DBT_VERIFY_PARTIAL_PARSE", value_parser = BoolishValueParser::new(), hide = true)]
    pub verify_partial_parse: bool,

    #[arg(global = true, long, default_value_t = false, action = ArgAction::SetTrue, env = "DBT_VERIFY_PARTIAL_LOAD", value_parser = BoolishValueParser::new(), hide = true)]
    pub verify_partial_load: bool,

    // At start of run, use `show` or `information_schema` queries to populate a relational cache, which can speed up subsequent materializations.
    #[arg(global = true, long, default_value_t = false, action = ArgAction::SetTrue, env = "DBT_POPULATE_CACHE", value_parser = BoolishValueParser::new(), hide = true)]
    pub populate_cache: bool,
    #[arg(global = true, long, default_value_t = false, action = ArgAction::SetTrue, env = "DBT_POPULATE_CACHE", value_parser = BoolishValueParser::new(), hide = true)]
    pub no_populate_cache: bool,

    // Output all {{ print() }} macro calls.
    #[arg(global = true, long, default_value_t = false, action = ArgAction::SetTrue, env = "DBT_PRINT", value_parser = BoolishValueParser::new(), hide = true)]
    pub print: bool,
    #[arg(global = true, long, default_value_t = false, action = ArgAction::SetTrue, env = "DBT_PRINT", value_parser = BoolishValueParser::new(), hide = true)]
    pub no_print: bool,

    // Sets the width of terminal output
    #[arg(global = true, long, env = "DBT_PRINTER_WIDTH", value_parser = u32::from_str, default_value_t = 120, hide = true)]
    pub printer_width: u32,

    /// When this option is passed, dbt will output low-level timing stats to the specified file. Example: `--record-timing-info output.profile`
    #[arg(global = true, long, short = 'r', hide = true)]
    pub record_timing_info: Option<PathBuf>,

    // Send anonymous usage stats to dbt Labs.
    #[arg(global = true, long, default_value_t=true, action = ArgAction::SetTrue, env = "DBT_SEND_ANONYMOUS_USAGE_STATS", value_parser = BoolishValueParser::new(), help_heading = help_headings::ADVANCED, hide_short_help = true)]
    pub send_anonymous_usage_stats: bool,
    #[arg(global = true, long, default_value_t=false, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), help_heading = help_headings::ADVANCED, hide_short_help = true)]
    pub no_send_anonymous_usage_stats: bool,

    // Use the static parser.
    #[arg(global = true, long, default_value_t=false,  action = ArgAction::SetTrue, env = "DBT_STATIC_PARSER", value_parser = BoolishValueParser::new(), hide = true)]
    pub static_parser: bool,
    #[arg(global = true, long, default_value_t=false,  action = ArgAction::SetTrue, env = "DBT_STATIC_PARSER", value_parser = BoolishValueParser::new(), hide = true)]
    pub no_static_parser: bool,

    // Specify whether log output is colorized in the console and the log file. Use --use-colors-file/--no-use-colors-file to colorize the log file differently than the console.
    #[arg(global = true, long, default_value_t=false,  action = ArgAction::SetTrue, env = "DBT_USE_COLORS", value_parser = BoolishValueParser::new(), hide = true)]
    pub use_colors: bool,
    #[arg(global = true, long, default_value_t=false,  action = ArgAction::SetTrue, env = "DBT_USE_COLORS", value_parser = BoolishValueParser::new(), hide = true)]
    pub no_use_colors: bool,

    // Specify whether log file output is colorized by overriding the default value and the general --use-colors/--no-use-colors setting.
    #[arg(global = true, long, default_value_t=false, action = ArgAction::SetTrue, env = "DBT_USE_COLORS_FILE", value_parser = BoolishValueParser::new(), hide = true)]
    pub use_colors_file: bool,
    #[arg(global = true, long, default_value_t=false, action = ArgAction::SetTrue, env = "DBT_USE_COLORS_FILE", value_parser = BoolishValueParser::new(), hide = true)]
    pub no_use_colors_file: bool,

    // Enable experimental parsing features.
    #[arg(global = true, long, default_value_t=false, action = ArgAction::SetTrue, env = "DBT_USE_EXPERIMENTAL_PARSER", value_parser = BoolishValueParser::new(), hide = true)]
    pub use_experimental_parser: bool,
    #[arg(global = true, long, default_value_t=false, action = ArgAction::SetTrue, env = "DBT_USE_EXPERIMENTAL_PARSER", value_parser = BoolishValueParser::new(), hide = true)]
    pub no_use_experimental_parser: bool,

    // Use the v2 parser. Accepted as a no-op for compatibility; fusion always uses its own parser.
    #[arg(global = true, long, default_value_t=false, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), hide = true)]
    pub use_v2_parser: bool,
    #[arg(global = true, long, default_value_t=false, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), hide = true)]
    pub no_use_v2_parser: bool,

    #[arg(global = true, long, default_value_t=false,  action = ArgAction::SetTrue, env = "DBT_USE_FAST_TEST_EDGES", value_parser = BoolishValueParser::new(), hide = true)]
    pub use_fast_test_edges: bool,
    #[arg(global = true, long, default_value_t=false,  action = ArgAction::SetTrue, env = "DBT_USE_FAST_TEST_EDGES", value_parser = BoolishValueParser::new(), hide = true)]
    pub no_use_fast_test_edges: bool,

    // If set, ensure the installed dbt version matches the require-dbt-version specified in the dbt_project.yml file (if any). Otherwise, allow them to differ.
    #[arg(global = true, long , default_value_t=true,  action = ArgAction::SetTrue, env = "DBT_VERSION_CHECK", value_parser = BoolishValueParser::new(), hide=true)]
    pub version_check: bool,
    /// Disable online version check for dbt updates
    #[arg(global = true, long = "no-version-check", default_value_t=false,  action = ArgAction::SetTrue, env = "DBT_DISABLE_VERSION_CHECK", value_parser = BoolishValueParser::new(), help_heading = help_headings::EXECUTION, hide_short_help = true)]
    pub no_version_check: bool,

    // If set, run models in a project while building only the schemas
    // reference: https://docs.getdbt.com/docs/build/empty-flag
    #[arg(global = true, long , default_value_t=false,  action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), hide=true, overrides_with = "_no_empty")]
    pub empty: bool,
    #[arg(global = true, long = "no-empty", default_value_t=false,  action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), hide=true)]
    pub _no_empty: bool,

    /// Store test failures in the database (equivalent to dbt-core `--store-failures`).
    #[arg(
        global = true,
        long = "store-failures",
        default_value_t = false,
        action = ArgAction::SetTrue,
        env = "DBT_STORE_FAILURES",
        value_parser = BoolishValueParser::new(),
        help_heading = help_headings::EXECUTION,
        hide_short_help = true
    )]
    pub store_failures: bool,

    /// Maximum size (MiB) for seed files whose contents are hashed.
    /// Larger seeds fall back to a path-based checksum. Set to 0 to
    /// disable the limit (always hash contents). Defaults to 1 MiB when unset.
    #[arg(global = true, long, env = "DBT_MAXIMUM_SEED_SIZE_MIB")]
    pub maximum_seed_size_mib: Option<u64>,

    // --------------------------------------------------------------------------------------------
    // fs specific public options
    #[clap(
    long,
    num_args(0..),
    help = "Show produced artifacts [default: 'progress']\n",
    help_heading = help_headings::ADVANCED,
    hide_short_help = true
)]
    pub show: Vec<ShowOptions>,

    /// Run the following phases [default: derived from command]
    #[arg(global = true, long, hide = true)]
    pub phase: Option<Phases>,

    // --------------------------------------------------------------------------------------------
    // fs specific public options
    // Task cache coordination URL. Supports:
    // - `noop` for no coordination (single-process only)
    // - `file://<path>` for local file-based cache
    // - `redis://<host>` for shared Redis coordination
    //
    /// Task cache coordination URL. Use `redis://<host>` for shared Redis coordination
    #[clap(long, env = "DBT_TASK_CACHE_URL", default_value = "noop", hide = true)]
    pub task_cache_url: String,

    /// Enable service-backed dbt State without legacy task-cache coordination
    #[arg(global = true, long = "manage-state", default_value_t = false, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), help_heading = help_headings::EXECUTION, hide_short_help = true)]
    pub manage_state: bool,
    /// Disable service-backed dbt State
    #[arg(global = true, long = "no-manage-state", default_value_t = false, action = ArgAction::SetTrue, value_parser = BoolishValueParser::new())]
    pub no_manage_state: bool,

    // --------------------------------------------------------------------------------------------
    // internal only
    /// The directory to install fs_internal packages
    #[arg(
        global = true,
        long,
        env = "DBT_FS_INTERNAL_PACKAGES_INSTALL_PATH",
        hide = true
    )]
    pub internal_packages_install_path: Option<PathBuf>,

    /// Enable OTLP export tracing. Use OTEL_EXPORTER_OTLP_ENDPOINT to set the endpoint.
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue,  env = "DBT_EXPORT_TO_OTLP", value_parser = BoolishValueParser::new(), hide = true)]
    pub export_to_otlp: bool,

    /// Path for dbt replay functionality (Using _DBT_REPLAY env var for Shadow Scenarios)
    #[arg(
        global = true,
        long,
        group = "replay_mode",
        env = "_DBT_REPLAY",
        hide = true
    )]
    pub dbt_replay: Option<PathBuf>,

    /// Path for recording SQL queries
    #[arg(global = true, long, group = "replay_mode", hide = true)]
    pub fs_record: Option<PathBuf>,

    /// Path for replaying SQL queries
    #[arg(global = true, long, group = "replay_mode", hide = true)]
    pub fs_replay: Option<PathBuf>,

    // -------------------------------------------------------------------------
    // Time Machine arguments
    // -------------------------------------------------------------------------
    /// Time Machine mode: record adapter calls for later replay, or replay from artifact.
    /// Use with --time-machine-path to specify the recording/artifact location.
    #[arg(
        global = true,
        long = "time-machine",
        env = "DBT_TIME_MACHINE_MODE",
        value_enum,
        hide = true
    )]
    pub time_machine_mode: Option<TimeMachineModeKind>,

    /// Path for Time Machine recording output or replay artifact input.
    /// In record mode: directory to write the recording (default: <target>/time_machine/{run_id})
    /// In replay mode: path to the recorded artifact directory (default: <target>/time_machine)
    #[arg(
        global = true,
        long = "time-machine-path",
        env = "DBT_TIME_MACHINE_PATH",
        hide = true
    )]
    pub time_machine_path: Option<PathBuf>,

    /// [Replay mode] Replay ordering mode: strict (exact sequence) or semantic (flexible reads).
    #[arg(
        global = true,
        long = "time-machine-ordering",
        env = "DBT_TIME_MACHINE_ORDERING",
        value_enum,
        default_value = "strict",
        hide = true
    )]
    pub time_machine_ordering: TimeMachineReplayOrdering,

    /// TEMPORARY same as sql-analysis=none.
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, hide = true)]
    pub legacy_compile: bool,

    /// Flag for semantic manifest validation
    #[arg(global = true, env = "DBT_SKIP_SEMANTIC_MANIFEST_VALIDATION", long, default_value = "false", action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), hide= true)]
    pub skip_semantic_manifest_validation: bool,

    /// Flag to enable Semantic Layer exports
    #[arg(global = true, env = "DBT_EXPORT_SAVED_QUERIES", long, default_value = "false", action = ArgAction::SetTrue, value_parser = BoolishValueParser::new(), hide= true)]
    pub export_saved_queries: bool,

    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, hide = true)]
    pub skip_unreferenced_table_check: bool,

    /// Override the invocation ID with a UUID string or integer.
    #[arg(global = true, env = "DBT_INVOCATION_ID", long, value_parser = parse_invocation_id, help_heading = help_headings::LOGGING, hide_short_help = true)]
    pub invocation_id: Option<Uuid>,

    /// Set the parent span ID for trace correlation (16-character hex or u64).
    #[arg(global = true, env = "DBT_PARENT_SPAN_ID", long, value_parser = parse_parent_span_id, help_heading = help_headings::LOGGING, hide_short_help = true)]
    pub parent_span_id: Option<u64>,

    /// Skip installation of private dependencies (useful for build conformance testing)
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, hide = true)]
    pub skip_private_deps: bool,

    /// If specified, the end datetime dbt uses to filter microbatch model inputs (exclusive).
    #[arg(
        global = true,
        long,
        env = "DBT_EVENT_TIME_END",
        help_heading = help_headings::EVENT_TIME,
        hide_short_help = true
    )]
    pub event_time_end: Option<String>,

    /// If specified, the start datetime dbt uses to filter microbatch model inputs (inclusive).
    #[arg(
        global = true,
        long,
        env = "DBT_EVENT_TIME_START",
        help_heading = help_headings::EVENT_TIME,
        hide_short_help = true
    )]
    pub event_time_start: Option<String>,

    /// How to load internal (embedded) dbt packages: embedded (default), forcewrite, readfromdisk
    #[arg(
        global = true,
        long = "internal-package",
        env = "DBT_INTERNAL_PACKAGE_MODE",
        hide = true,
        default_value_t
    )]
    pub internal_package_mode: InternalPackageMode,

    /// When installing packages from Package Hub, use v2-compatible downloads if available
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, env = "DBT_USE_V2_COMPATIBLE_PACKAGE_DOWNLOADS", value_parser = BoolishValueParser::new(), help_heading = help_headings::ADVANCED, hide_short_help = true)]
    pub use_v2_compatible_package_downloads: bool,

    /// When installing packages from Package Hub, require a Hub-provided sha1 checksum and verify it, failing the install if Hub offers neither
    #[arg(global = true, long, default_value = "false", action = ArgAction::SetTrue, env = "DBT_REQUIRE_HUB_VERIFIED_DOWNLOADS", value_parser = BoolishValueParser::new(), help_heading = help_headings::ADVANCED, hide_short_help = true)]
    pub require_hub_verified_downloads: bool,

    /// If set, the maximum number of bytes that the ANTLR parser is allowed to
    /// allocate in its (per-dialect) global cache before it aborts with an
    /// error. USE WITH CAUTION: as setting this too low may cause parsing to
    /// fail on large SQL inputs. Default is 0 (unlimited). [env:
    /// DBT_ANTLR_PARSER_CACHE_HARD_LIMIT_BYTES]
    #[arg(
        global = true,
        long,
        env = "DBT_ANTLR_PARSER_CACHE_HARD_LIMIT_BYTES",
        hide = true
    )]
    pub antlr_parser_cache_hard_limit_bytes: Option<usize>,

    /// If set, places a hard limit on the size of each Antlr AST -- if the
    /// limit is exceeded, the parser will abort with an error. The purpose of
    /// this limit is to prevent excessively large inputs (e.g.
    /// machine-generated SQL) causing the whole process to be OOM-killed.
    /// Default is 0 (unlimited). [env: DBT_ANTLR_ARENA_LIMIT_BYTES]
    #[arg(global = true, long, env = "DBT_ANTLR_ARENA_LIMIT_BYTES", hide = true)]
    pub antlr_arena_limit_bytes: Option<usize>,

    /// If set, the number of bytes that the ANTLR parser is allowed to allocate
    /// in its (per-dialect) global cache before it attempts to reset the whole
    /// cache to free up memory. This setting is a "soft" limit that is checked
    /// at the end of each parse (since cache resets can not be performed while
    /// a parse is in progress), and if exceeded, triggers a cache reset. USE
    /// WITH CAUTION: setting this too low may cause excessive cache resets and
    /// degrade parsing performance on large SQL inputs. Default is 0
    /// (unlimited). [env: DBT_ANTLR_CACHE_THRESHOLD_BYTES]
    #[arg(
        global = true,
        long,
        env = "DBT_ANTLR_CACHE_THRESHOLD_BYTES",
        hide = true
    )]
    pub antlr_parser_cache_threshold_bytes: Option<usize>,

    /// If set, the maximum depth of call stack that the ANTLR parser is allowed
    /// to reach before it aborts with an error. This setting is intended to
    /// prevent inputs with excessively deep nesting (e.g. from maliciously
    /// crafted SQL) causing a denial-of-service on the host. Note that the this
    /// value is usually 2x-10x the actual level of nesting ANTLR can handle,
    /// depending on the grammar. Default is 20000. [env:
    /// DBT_ANTLR_PARSER_RECURSION_LIMIT]
    #[arg(
        global = true,
        long,
        env = "DBT_ANTLR_PARSER_RECURSION_LIMIT",
        hide = true
    )]
    pub antlr_parser_recursion_limit: Option<u32>,
}

fn resolve_show_arg(show_arg: &[ShowOptions], quiet: bool) -> HashSet<ShowOptions> {
    let mut show = if show_arg.contains(&ShowOptions::All) {
        ShowOptions::iter().collect()
    } else if show_arg.contains(&ShowOptions::None) {
        HashSet::default()
    } else if show_arg.is_empty() {
        [ShowOptions::Progress, ShowOptions::Completed]
            .into_iter()
            .collect()
    } else {
        show_arg
            .iter()
            .cloned()
            .flat_map(|opt| {
                if opt == ShowOptions::Progress {
                    vec![
                        ShowOptions::Progress,
                        ShowOptions::ProgressParse,
                        ShowOptions::ProgressHydrate,
                        ShowOptions::ProgressRender,
                        ShowOptions::ProgressAnalyze,
                        ShowOptions::ProgressRun,
                        ShowOptions::Completed,
                    ]
                } else {
                    vec![opt]
                }
            })
            .collect()
    };

    // quiet overrules all show options
    if quiet {
        show = HashSet::default();
    }

    show
}

#[derive(Debug, Default, Deserialize)]
struct FlagsFile {
    #[serde(default)]
    flags: Option<FlagsBlock>,
}

#[derive(Debug, Default, Deserialize)]
struct FlagsBlock {
    manage_state: Option<bool>,
    maximum_seed_size_mib: Option<u64>,
    send_anonymous_usage_stats: Option<bool>,
}

fn manage_state_from_yaml(path: &Path) -> Option<bool> {
    let content = stdfs::read_to_string(path).ok()?;
    let file = dbt_yaml::from_str::<FlagsFile>(&content).ok()?;
    file.flags.and_then(|flags| flags.manage_state)
}

fn parse_manage_state_env(value: Option<&OsStr>) -> FsResult<Option<bool>> {
    let Some(value) = value.filter(|value| !value.to_string_lossy().trim().is_empty()) else {
        return Ok(None);
    };
    BoolishValueParser::new()
        .parse_ref(&clap::Command::new("dbt"), None, value)
        .map(Some)
        .map_err(|_| {
            fs_err!(
                ErrorCode::InvalidConfig,
                "Invalid value for {}: '{}'. Expected a boolean.",
                MANAGE_STATE_ENV,
                value.to_string_lossy()
            )
        })
}

fn validate_manage_state_env() -> FsResult<()> {
    parse_manage_state_env(env::var_os(MANAGE_STATE_ENV).as_deref()).map(|_| ())
}

fn maximum_seed_size_mib_from_yaml(path: &Path) -> Option<u64> {
    let content = stdfs::read_to_string(path).ok()?;
    let file = dbt_yaml::from_str::<FlagsFile>(&content).ok()?;
    file.flags.and_then(|flags| flags.maximum_seed_size_mib)
}

fn send_anonymous_usage_stats_from_yaml(path: &Path) -> Option<bool> {
    let content = stdfs::read_to_string(path).ok()?;
    let file = dbt_yaml::from_str::<FlagsFile>(&content).ok()?;
    file.flags
        .and_then(|flags| flags.send_anonymous_usage_stats)
}

const DEFAULT_MAXIMUM_SEED_SIZE_MIB: u64 = 1;

fn user_settings_path() -> Option<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(USER_SETTINGS_YML))
}

impl CommonArgs {
    /// `default` applies only when neither the flags, `DBT_ENGINE_MANAGE_STATE`,
    /// `dbt_project.yml`, nor the user settings say anything.
    pub fn get_manage_state(&self, project_dir: &Path, default: bool) -> bool {
        self.get_manage_state_with(
            project_dir,
            env::var_os(MANAGE_STATE_ENV),
            user_settings_path(),
            default,
        )
    }

    fn resolve_maximum_seed_size_mib(&self, project_dir: &Path) -> u64 {
        self.maximum_seed_size_mib
            .or_else(|| maximum_seed_size_mib_from_yaml(&project_dir.join(DBT_PROJECT_YML)))
            .unwrap_or(DEFAULT_MAXIMUM_SEED_SIZE_MIB)
    }

    /// Precedence: CLI flag, then an explicit config opt-out, then env var, then
    /// config. Callers get an authoritative answer and must not re-read the env.
    fn get_manage_state_with(
        &self,
        project_dir: &Path,
        manage_state_env: Option<OsString>,
        user_settings_path: Option<PathBuf>,
        default: bool,
    ) -> bool {
        if self.no_manage_state {
            return false;
        }
        if self.manage_state {
            return true;
        }

        let configured = manage_state_from_yaml(&project_dir.join(DBT_PROJECT_YML))
            .or_else(|| user_settings_path.and_then(|path| manage_state_from_yaml(&path)));

        if configured == Some(false) {
            return false;
        }

        let env_value = parse_manage_state_env(manage_state_env.as_deref())
            .ok()
            .flatten();
        env_value.or(configured).unwrap_or(default)
    }

    pub fn get_warn_error(&self) -> Option<bool> {
        if self.warn_error {
            Some(true)
        } else if self.no_warn_error {
            Some(false)
        } else {
            env::var_os("DBT_WARN_ERROR").and_then(|value| {
                BoolishValueParser::new()
                    .parse_ref(&clap::Command::new("dbt"), None, OsStr::new(&value))
                    .ok()
            })
        }
    }

    /// `--verify-partial-load` implies `--partial-load` implies `--partial-parse`.
    /// `--dirty` implies `--partial-parse`.
    /// `--verify-partial-load` → `--partial-load` → `--partial-parse`
    /// `--verify-partial-parse` → `--partial-parse`
    /// `--dirty` → `--partial-parse`
    /// `--write-index` / `--generate-info-schema` → `--write-metadata` → `--partial-parse`
    pub fn effective_partial_parse(&self) -> bool {
        self.write_metadata
            || self.effective_write_index()
            || self.generate_info_schema
            || self.partial_parse
            || self.partial_load
            || self.verify_partial_load
            || self.verify_partial_parse
            || self.dirty
    }

    /// `--verify-partial-load` implies `--partial-load`. `--write-metadata` implies both.
    /// `--write-index` and `--generate-info-schema` imply `--write-metadata`, hence both.
    pub fn effective_partial_load(&self) -> bool {
        self.write_metadata
            || self.effective_write_index()
            || self.generate_info_schema
            || self.partial_load
            || self.verify_partial_load
    }

    /// `--write-index` after `--no-write-index` cancels it.
    ///
    /// Both `effective_partial_*` gates go through this rather than reading `write_index`
    /// directly, because those implications exist only to serve the index. Cancelling the index
    /// without cancelling them would leave the caller with no index *and* the partial-load fast
    /// path, which cannot see files it has not already recorded — the worst of both. An explicit
    /// `--write-metadata` still implies both on its own; `--no-write-index` only cancels the index.
    /// (`--generate-info-schema` implies `--write-metadata` independently, so it stays in the gates
    /// above regardless of `--no-write-index`.)
    pub fn effective_write_index(&self) -> bool {
        self.write_index && !self.no_write_index
    }

    /// Resolve the effective value of `--quiet` / `--no-quiet`.
    ///
    /// `--no-quiet` always wins over `DBT_QUIET` or `--quiet`, allowing callers to
    /// override an ambient `DBT_QUIET=true` from the command line.
    pub fn get_quiet(&self) -> bool {
        if self.no_quiet { false } else { self.quiet }
    }

    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let select_option = self.select.clone().map(|selectors| {
            let mut expr = parse_model_specifiers(&selectors).unwrap();
            // Apply indirect selection from CLI if specified (matching dbt-core behavior)
            if let Some(mode) = self.indirect_selection {
                expr.set_indirect_selection(mode);
            }
            expr
        });

        let exclude_option = self.exclude.clone().map(|selectors| {
            let mut expr = parse_model_specifiers(&selectors).unwrap();
            // Apply indirect selection from CLI if specified (matching dbt-core behavior)
            if let Some(mode) = self.indirect_selection {
                expr.set_indirect_selection(mode);
            }
            expr
        });
        let show = resolve_show_arg(self.show.as_slice(), self.get_quiet());
        let replay = pick_replay_mode(self, arg.io.invocation_id, out_dir);
        EvalArgs {
            command: arg.command,
            io: IoArgs {
                show,
                is_compile: arg.command == FsCommand::Compile,
                debug: self.debug,
                invocation_id: arg.io.invocation_id,
                otel_parent_span_id: arg.io.otel_parent_span_id,
                in_dir: in_dir.to_path_buf(),
                out_dir: out_dir.to_path_buf(),
                db_root: None,
                send_anonymous_usage_stats: self.get_send_anonymous_usage_stats(),
                status_reporter: arg.io.status_reporter.clone(),
                log_format: self.log_format,
                log_format_file: self.log_format_file,
                log_level: self.log_level,
                log_level_file: self.log_level_file,
                log_file_max_bytes: self.log_file_max_bytes,
                log_path: self.log_path.clone(),
                otel_file_name: self.otel_file_name.clone(),
                otel_parquet_file_name: self.otel_parquet_file_name.clone(),
                export_to_otlp: self.export_to_otlp,
                show_all_deprecations: self.show_all_deprecations,
                show_timings: arg.io.show_timings,
                host: self.host.clone(),
                port: self.port,
                use_v2_compatible_package_downloads: self.use_v2_compatible_package_downloads,
                require_hub_verified_downloads: self.require_hub_verified_downloads,
            },
            profiles_dir: self.profiles_dir.clone(),
            packages_install_path: self.packages_install_path.clone(),
            internal_packages_install_path: self.internal_packages_install_path.clone(),
            add_package: None, // doesn't come from CommonArgs, comes from DepsArgs
            upgrade: false,    // comes from DepsArgs
            lock: false,       // comes from DepsArgs
            profile: self.profile.clone(),
            target: self.target.clone(),
            update_deps: false,
            vars: self.vars.clone().unwrap_or_default(),
            ai_provider: self.ai_provider.clone(),
            phase: self.phase.clone().unwrap_or(Phases::All),
            format: DEFAULT_FORMAT,
            limit: if arg.command == FsCommand::Extension("sl") {
                Some(100)
            } else {
                Some(10)
            },
            from_main: false,
            // `threads` caps the `dbt-runtime` blocking pool, and with it both
            // warehouse concurrency and rendering parallelism. Sequential task
            // execution is requested via the separate `no_parallel` flag below.
            num_threads: self.threads,
            no_parallel: self.no_parallel,
            select: select_option,
            exclude: exclude_option,
            indirect_selection: self.indirect_selection,
            replay,
            long_living: false,
            skip_semantic_manifest_validation: self.skip_semantic_manifest_validation,
            export_saved_queries: self.export_saved_queries,
            max_depth: 0,
            show_scans: false,
            use_fqtn: false,
            schema: vec![],
            output_keys: vec![],
            skip_unreferenced_table_check: self.skip_unreferenced_table_check,
            state: self.state.clone(),
            defer_state: self.defer_state.clone(),
            connection: false,
            macro_name: None,
            macro_args: BTreeMap::new(),
            macro_sql: None,
            adapter_override: None,
            query_id: None,
            selector: self.selector.clone(),
            resource_types: vec![],
            exclude_resource_types: vec![],
            //flags
            warn_error: self.get_warn_error(),
            warn_error_options: self.warn_error_options.clone().unwrap_or_default(),
            version_check: if self.no_version_check {
                false
            } else {
                self.version_check
            },
            introspect: true,
            defer: if self.no_defer {
                false
            } else if self.defer {
                true
            } else {
                // default to defer if neither flag is passed
                true
            },
            debug: self.debug,
            log_format_file: self.log_format_file,
            log_format: self.log_format,
            log_level_file: match (self.debug, self.log_level_file) {
                (true, Some(LogLevel::Trace)) => Some(LogLevel::Trace),
                (true, _) => Some(LogLevel::Debug),
                (false, _) => self.log_level_file,
            },
            log_level: match (self.debug, self.log_level) {
                (true, Some(LogLevel::Trace)) => Some(LogLevel::Trace),
                (true, _) => Some(LogLevel::Debug),
                (false, _) => self.log_level,
            },
            log_path: self.log_path.clone(),
            project_dir: self.project_dir.clone(),
            quiet: self.get_quiet(),
            send_anonymous_usage_stats: self.get_send_anonymous_usage_stats(),
            write_json: if (self.write_metadata && !self.write_index) || self.no_write_json {
                false
            } else {
                self.write_json
            },
            write_catalog: self.write_catalog,
            // `--no-write-index` is authoritative over `--write-index`, the way
            // `--no-write-json` is over `--write-json`. It cancels only the index, so
            // `--write-metadata --no-write-index` still means epochs-without-index.
            // `--generate-info-schema` implies `--write-metadata` on its own.
            write_metadata: self.write_metadata
                || self.effective_write_index()
                || self.generate_info_schema,
            write_index: self.effective_write_index(),
            // Set by `Cli::to_eval_args` for the commands that default the index on; a
            // per-command conversion cannot know it was a default rather than a request.
            write_index_implied: false,
            check_names: Vec::new(),
            skip_checks: false,
            generate_info_schema: self.generate_info_schema,
            info_schema_dir: self.info_schema_dir.clone(),
            classify_with_warehouse_tags: self.classify_with_warehouse_tags,
            index_dir: self.index_dir.clone(),
            metadata_dir: self.metadata_dir.clone(),
            fail_fast: self.fail_fast,
            target_path: self.target_path.clone(),
            empty: self.empty,
            sample: None,
            favor_state: if self.no_favor_state {
                false
            } else {
                self.favor_state
            },
            refresh_sources: false,
            run_cache_service: false,
            run_cache_mode: RunCacheMode::Noop,
            task_cache_url: self.task_cache_url.clone(),
            static_analysis: None,
            full_refresh: false,
            // Derived later in `Cli::to_eval_args` from the caller's
            // original `--static-analysis`/`--write-lineage` values.
            infer_schemas_and_typeless: false,
            store_failures: self.store_failures,
            check_all: false,
            sample_renaming: BTreeMap::new(),
            optimize_tests: Default::default(),
            local_execution_backend: self.compute.into(),
            skip_checkpoints: false,
            skip_private_deps: self.skip_private_deps,
            event_time_end: self.event_time_end.clone(),
            event_time_start: self.event_time_start.clone(),
            internal_package_mode: self.internal_package_mode.clone(),
            skip_post_hooks: false,
            skip_creating_generic_tests: false,
            write_lineage: self.write_lineage,
            force_enable_linter: false,
            force_enable_formatter_diagnostics: false,
            maximum_seed_size_mib: self.resolve_maximum_seed_size_mib(in_dir),
            command_entrypoint: arg.command,
        }
    }

    pub fn get_send_anonymous_usage_stats(&self) -> bool {
        if self.no_send_anonymous_usage_stats {
            false
        } else {
            self.send_anonymous_usage_stats
        }
    }

    /// Resolve send_anonymous_usage_stats with precedence:
    /// `--no-send-anonymous-usage-stats` > DBT_SEND_ANONYMOUS_USAGE_STATS env > project yaml flags > true
    ///
    /// Note: the clap flag has `default_value_t = true`, so an explicit
    /// `--send-anonymous-usage-stats=true` is indistinguishable from the default and will
    /// NOT override a YAML `false`. Only the explicit opt-out (`--no-`) and the env var are
    /// detectable here.
    pub fn get_send_anonymous_usage_stats_for_project(&self, project_dir: &Path) -> bool {
        if self.no_send_anonymous_usage_stats {
            return false;
        }
        if let Some(value) = env::var_os("DBT_SEND_ANONYMOUS_USAGE_STATS") {
            return BoolishValueParser::new()
                .parse_ref(&clap::Command::new("dbt"), None, value.as_ref())
                .unwrap_or(true);
        }
        send_anonymous_usage_stats_from_yaml(&project_dir.join(DBT_PROJECT_YML)).unwrap_or(true)
    }

    pub fn get_introspect(&self) -> bool {
        !self.no_introspect
    }

    /// Returns warn_error_options resolved purely from cli args (including `--warn-error`)
    pub fn get_cli_warn_error_options(&self) -> WarnErrorOptions {
        let mut options = self.warn_error_options.clone().unwrap_or_default();

        if self.warn_error {
            options.add_all_to_error();
        }

        options
    }

    /// True when replaying a prior recorded dbt run via `--dbt-replay`, since those recordings predate fusion-only warnings.
    pub fn skip_fusion_only_upgrades(&self) -> bool {
        self.dbt_replay.is_some()
    }
}

/// Generate shell completion scripts
#[derive(Parser, Debug, Clone)]
pub struct CompletionsArgs {
    /// The shell to generate completions for
    #[arg(value_enum)]
    pub shell: Shell,
    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,
}

impl CompletionsArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let mut eval_args = self.common_args.to_eval_args(arg, in_dir, out_dir);
        eval_args.phase = Phases::Deps;
        eval_args
    }
}

#[derive(
    Debug, Copy, Clone, PartialEq, Eq, Default, Display, ValueEnum, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ProjectTemplate {
    #[default]
    JaffleShop,
    MomsFlowerShop,
}

#[derive(Parser, Debug, Default, Clone, Serialize, Deserialize)]
pub struct InitArgs {
    /// The name of the project to create
    #[arg(long, default_value = "jaffle_shop", value_parser = validate_project_name)]
    pub project_name: String,

    /// Skip interactive profile setup
    #[arg(long, default_value = "false")]
    pub skip_profile_setup: bool,

    /// Run the dbt v2 onboarding/upgrade flow
    #[arg(long = "v2-upgrade", alias = "fusion-upgrade", default_value = "false")]
    #[serde(alias = "fusion_upgrade")]
    pub v2_upgrade: bool,

    /// The sample project to initialize with
    #[arg(long, default_value = "jaffle-shop")]
    pub sample: ProjectTemplate,

    // Flattened Common args
    #[clap(flatten)]
    pub common_args: CommonArgs,
}

impl InitArgs {
    pub fn to_eval_args(&self, arg: SystemArgs, in_dir: &Path, out_dir: &Path) -> EvalArgs {
        let show = if arg.io.show.contains(&ShowOptions::All) {
            ShowOptions::iter().collect()
        } else if arg.io.show.is_empty() {
            [ShowOptions::Progress, ShowOptions::Completed]
                .into_iter()
                .collect()
        } else {
            arg.io.show.iter().cloned().collect()
        };
        EvalArgs {
            command: arg.command,
            from_main: arg.from_main,
            io: IoArgs {
                in_dir: in_dir.to_path_buf(),
                out_dir: out_dir.to_path_buf(),
                db_root: None,
                show,
                is_compile: arg.command == FsCommand::Compile,
                debug: arg.io.debug,
                invocation_id: arg.io.invocation_id,
                otel_parent_span_id: arg.io.otel_parent_span_id,
                send_anonymous_usage_stats: self.common_args.get_send_anonymous_usage_stats(),
                status_reporter: arg.io.status_reporter.clone(),
                log_format: self.common_args.log_format,
                log_format_file: self.common_args.log_format_file,
                log_level: self.common_args.log_level,
                log_level_file: self.common_args.log_level_file,
                log_file_max_bytes: self.common_args.log_file_max_bytes,
                log_path: self.common_args.log_path.clone(),
                otel_file_name: self.common_args.otel_file_name.clone(),
                otel_parquet_file_name: self.common_args.otel_parquet_file_name.clone(),
                show_all_deprecations: self.common_args.show_all_deprecations,
                show_timings: arg.from_main,
                export_to_otlp: self.common_args.export_to_otlp,
                host: self.common_args.host.clone(),
                port: self.common_args.port,
                use_v2_compatible_package_downloads: self
                    .common_args
                    .use_v2_compatible_package_downloads,
                require_hub_verified_downloads: self.common_args.require_hub_verified_downloads,
            },
            task_cache_url: "noop".to_string(),
            favor_state: self.common_args.favor_state,
            phase: Phases::Debug,
            send_anonymous_usage_stats: self.common_args.get_send_anonymous_usage_stats(),
            // Propagate profile-resolution inputs so post-init validation reads the same
            // profiles.yml the setup step used (e.g. --profiles-dir / DBT_PROFILES_DIR).
            profiles_dir: self.common_args.profiles_dir.clone(),
            profile: self.common_args.profile.clone(),
            target: self.common_args.target.clone(),
            vars: self.common_args.vars.clone().unwrap_or_default(),
            ..Default::default()
        }
    }
}

pub fn from_main(cli: &Cli) -> SystemArgs {
    let common_args = cli.common_args();
    let command = cli.as_command();

    SystemArgs {
        command,
        io: IoArgs {
            invocation_id: common_args.invocation_id.unwrap_or_else(Uuid::now_v7),
            otel_parent_span_id: common_args.parent_span_id,
            show: resolve_show_arg(common_args.show.as_slice(), common_args.get_quiet()),
            is_compile: command == FsCommand::Compile,
            debug: common_args.debug,
            in_dir: PathBuf::new(),
            out_dir: PathBuf::new(),
            db_root: None,
            send_anonymous_usage_stats: common_args.get_send_anonymous_usage_stats(),
            status_reporter: None,
            log_format: common_args.log_format,
            log_format_file: common_args.log_format_file,
            log_level: match (common_args.debug, common_args.log_level) {
                (true, Some(LogLevel::Trace)) => Some(LogLevel::Trace),
                (true, _) => Some(LogLevel::Debug),
                (false, _) => common_args.log_level,
            },
            log_level_file: match (common_args.debug, common_args.log_level_file) {
                (true, Some(LogLevel::Trace)) => Some(LogLevel::Trace),
                (true, _) => Some(LogLevel::Debug),
                (false, _) => common_args.log_level_file,
            },
            log_file_max_bytes: common_args.log_file_max_bytes,
            log_path: common_args.log_path,
            export_to_otlp: common_args.export_to_otlp,
            otel_file_name: common_args.otel_file_name,
            otel_parquet_file_name: common_args.otel_parquet_file_name,
            show_all_deprecations: common_args.show_all_deprecations,
            show_timings: true,
            host: common_args.host,
            port: common_args.port,
            use_v2_compatible_package_downloads: common_args.use_v2_compatible_package_downloads,
            require_hub_verified_downloads: common_args.require_hub_verified_downloads,
        },
        from_main: true,
        exit_process_on_panic: true,

        target: common_args.target,
        num_threads: common_args.threads,
        no_parallel: common_args.no_parallel,
    }
}

pub fn from_lib(cli: &Cli) -> SystemArgs {
    let common_args = cli.common_args();
    let command = cli.as_command();

    SystemArgs {
        command,
        io: IoArgs {
            invocation_id: common_args.invocation_id.unwrap_or_else(Uuid::now_v7),
            otel_parent_span_id: common_args.parent_span_id,
            show: resolve_show_arg(common_args.show.as_slice(), common_args.get_quiet()),
            is_compile: command == FsCommand::Compile,
            debug: common_args.debug,
            in_dir: PathBuf::new(),
            out_dir: PathBuf::new(),
            db_root: None,
            send_anonymous_usage_stats: common_args.get_send_anonymous_usage_stats(),
            status_reporter: None,
            // should_cancel_compilation: None,
            log_format: common_args.log_format,
            log_format_file: common_args.log_format_file,
            log_level: common_args.log_level,
            log_level_file: common_args.log_level_file,
            log_file_max_bytes: common_args.log_file_max_bytes,
            log_path: common_args.log_path,
            export_to_otlp: common_args.export_to_otlp,
            otel_file_name: common_args.otel_file_name,
            otel_parquet_file_name: common_args.otel_parquet_file_name,
            show_all_deprecations: common_args.show_all_deprecations,
            show_timings: false,
            host: common_args.host,
            port: common_args.port,
            use_v2_compatible_package_downloads: common_args.use_v2_compatible_package_downloads,
            require_hub_verified_downloads: common_args.require_hub_verified_downloads,
        },
        from_main: false,
        exit_process_on_panic: true,
        target: common_args.target,
        num_threads: common_args.threads,
        no_parallel: common_args.no_parallel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoopParser;

    impl ExtensionCommandParser for NoopParser {
        fn has_subcommand(&self, _name: &str) -> bool {
            false
        }
    }

    fn get_manage_state_with_env(
        common_args: &CommonArgs,
        project_dir: &Path,
        env_value: Option<&str>,
        user_settings_path: Option<PathBuf>,
        default: bool,
    ) -> bool {
        common_args.get_manage_state_with(
            project_dir,
            env_value.map(OsString::from),
            user_settings_path,
            default,
        )
    }

    #[test]
    fn no_write_index_cancels_the_index_and_its_implications() {
        // `--write-index` alone: index on, and it implies partial parse + partial load.
        let mut args = CommonArgs {
            write_index: true,
            ..Default::default()
        };
        assert!(args.effective_write_index());
        assert!(args.effective_partial_parse());
        assert!(args.effective_partial_load());

        // Adding `--no-write-index` must cancel all three together. Cancelling only the index
        // would leave the caller with no index *and* the partial-load fast path, which cannot
        // see files it has not already recorded.
        args.no_write_index = true;
        assert!(!args.effective_write_index());
        assert!(!args.effective_partial_parse());
        assert!(!args.effective_partial_load());
    }

    #[test]
    fn no_write_index_leaves_explicit_write_metadata_alone() {
        // `--write-metadata` implies partial parse/load on its own, so `--no-write-index`
        // cancels only the index: this stays epochs-without-index.
        let args = CommonArgs {
            write_metadata: true,
            no_write_index: true,
            ..Default::default()
        };
        assert!(!args.effective_write_index());
        assert!(args.effective_partial_parse());
        assert!(args.effective_partial_load());
    }

    #[test]
    fn hidden_artifact_flags_are_absent_from_long_help_but_still_parse() {
        use clap::CommandFactory;

        let help = CommonArgs::command().render_long_help().to_string();
        for flag in [
            "--write-lineage",
            "--write-index",
            "--write-metadata",
            "--lineage",
        ] {
            assert!(
                !help.contains(flag),
                "{flag} must be hidden from --help, got:\n{help}"
            );
        }
        assert!(
            help.contains("--generate-info-schema"),
            "public artifacts flag must remain in --help, got:\n{help}"
        );
        assert!(
            help.contains("--write-catalog"),
            "--write-catalog stays in --help, got:\n{help}"
        );

        let parsed = CommonArgs::try_parse_from([
            "dbt",
            "--write-catalog",
            "--write-lineage",
            "--write-index",
            "--write-metadata",
        ])
        .expect("hidden artifact flags must remain accepted");
        assert!(parsed.write_catalog);
        assert!(parsed.write_lineage);
        assert!(parsed.write_index);
        assert!(parsed.write_metadata);

        assert!(
            CommonArgs::try_parse_from(["dbt", "--lineage"]).is_err(),
            "--lineage is not a clap flag"
        );
    }

    #[test]
    fn show_info_parses_and_conflicts_with_inline() {
        let parsed = ShowArgs::try_parse_from(["show", "--info", "models"])
            .expect("--info must parse on show");
        assert_eq!(parsed.info.as_deref(), Some("models"));

        assert!(
            ShowArgs::try_parse_from(["show", "--info", "models", "--inline", "select 1"]).is_err(),
            "--info must conflict with --inline"
        );
        assert!(
            ShowArgs::try_parse_from(["show", "--info", "models", "--adapter", "lakecompute"])
                .is_err(),
            "--info must conflict with --adapter"
        );
    }

    #[test]
    fn manage_state_defaults_to_false() {
        let project_dir = tempfile::tempdir().unwrap();
        let common_args = CommonArgs::default();

        assert!(!get_manage_state_with_env(
            &common_args,
            project_dir.path(),
            None,
            None,
            false
        ));
    }

    #[test]
    fn manage_state_uses_supplied_default() {
        let project_dir = tempfile::tempdir().unwrap();
        let common_args = CommonArgs::default();

        assert!(get_manage_state_with_env(
            &common_args,
            project_dir.path(),
            None,
            None,
            true
        ));
    }

    #[test]
    fn manage_state_reads_project_flags() {
        let project_dir = tempfile::tempdir().unwrap();
        stdfs::write(
            project_dir.path().join(DBT_PROJECT_YML),
            "flags:\n  manage_state: true\n",
        )
        .unwrap();
        let common_args = CommonArgs::default();

        assert!(get_manage_state_with_env(
            &common_args,
            project_dir.path(),
            None,
            None,
            false
        ));
    }

    #[test]
    fn manage_state_reads_user_settings() {
        let project_dir = tempfile::tempdir().unwrap();
        let home_dir = tempfile::tempdir().unwrap();
        stdfs::create_dir_all(home_dir.path().join(".dbt")).unwrap();
        let user_settings = home_dir.path().join(USER_SETTINGS_YML);
        stdfs::write(&user_settings, "flags:\n  manage_state: true\n").unwrap();
        let common_args = CommonArgs::default();

        assert!(get_manage_state_with_env(
            &common_args,
            project_dir.path(),
            None,
            Some(user_settings),
            false
        ));
    }

    #[test]
    fn manage_state_cli_overrides_env_false() {
        let project_dir = tempfile::tempdir().unwrap();
        let common_args = CommonArgs {
            manage_state: true,
            ..Default::default()
        };

        assert!(get_manage_state_with_env(
            &common_args,
            project_dir.path(),
            Some("false"),
            None,
            false
        ));
    }

    #[test]
    fn no_manage_state_overrides_project_flags() {
        let project_dir = tempfile::tempdir().unwrap();
        stdfs::write(
            project_dir.path().join(DBT_PROJECT_YML),
            "flags:\n  manage_state: true\n",
        )
        .unwrap();
        let common_args = CommonArgs {
            no_manage_state: true,
            ..Default::default()
        };

        assert!(!get_manage_state_with_env(
            &common_args,
            project_dir.path(),
            None,
            None,
            false
        ));
    }

    /// The dbt platform path: the injected env var alone still enables dbt State.
    #[test]
    fn env_true_enables_manage_state() {
        let project_dir = tempfile::tempdir().unwrap();
        let common_args = CommonArgs::default();

        assert!(get_manage_state_with_env(
            &common_args,
            project_dir.path(),
            Some("true"),
            None,
            false
        ));
    }

    /// dbt platform injects `DBT_ENGINE_MANAGE_STATE=true` on production
    /// deployments, so an explicit opt-out has to beat it or users have no way to
    /// turn dbt State off.
    #[test]
    fn no_manage_state_overrides_env_true() {
        let project_dir = tempfile::tempdir().unwrap();
        let common_args = CommonArgs {
            no_manage_state: true,
            ..Default::default()
        };

        assert!(!get_manage_state_with_env(
            &common_args,
            project_dir.path(),
            Some("true"),
            None,
            false
        ));
    }

    #[test]
    fn project_flag_false_overrides_env_true() {
        let project_dir = tempfile::tempdir().unwrap();
        stdfs::write(
            project_dir.path().join(DBT_PROJECT_YML),
            "flags:\n  manage_state: false\n",
        )
        .unwrap();
        let common_args = CommonArgs::default();

        assert!(!get_manage_state_with_env(
            &common_args,
            project_dir.path(),
            Some("true"),
            None,
            false
        ));
    }

    #[test]
    fn user_settings_false_overrides_env_true() {
        let project_dir = tempfile::tempdir().unwrap();
        let home_dir = tempfile::tempdir().unwrap();
        stdfs::create_dir_all(home_dir.path().join(".dbt")).unwrap();
        let user_settings = home_dir.path().join(USER_SETTINGS_YML);
        stdfs::write(&user_settings, "flags:\n  manage_state: false\n").unwrap();
        let common_args = CommonArgs::default();

        assert!(!get_manage_state_with_env(
            &common_args,
            project_dir.path(),
            Some("true"),
            Some(user_settings),
            false
        ));
    }

    /// Guards the clap wiring itself: if `manage_state` regains `env = ...`, the env
    /// var becomes indistinguishable from the flag and a config opt-out stops working.
    #[test]
    fn manage_state_flag_is_not_populated_from_env() {
        use clap::CommandFactory;

        let reads_env = CompileArgs::command()
            .get_arguments()
            .find(|arg| arg.get_id() == "manage_state")
            .map(|arg| arg.get_env().is_some());

        assert_eq!(
            reads_env,
            Some(false),
            "--manage-state must not read DBT_ENGINE_MANAGE_STATE via clap"
        );
    }

    #[test]
    fn invalid_env_value_is_rejected() {
        let err = parse_manage_state_env(Some(OsStr::new("treu"))).unwrap_err();

        assert!(
            err.to_string().contains(MANAGE_STATE_ENV) && err.to_string().contains("treu"),
            "error must name the variable and the bad value, got: {err}"
        );
    }

    #[test]
    fn valid_and_unset_env_values_parse() {
        assert_eq!(parse_manage_state_env(None).unwrap(), None);
        assert_eq!(
            parse_manage_state_env(Some(OsStr::new("   "))).unwrap(),
            None
        );
        assert_eq!(
            parse_manage_state_env(Some(OsStr::new("true"))).unwrap(),
            Some(true)
        );
        assert_eq!(
            parse_manage_state_env(Some(OsStr::new("false"))).unwrap(),
            Some(false)
        );
    }

    #[test]
    fn empty_env_value_is_treated_as_unset() {
        let project_dir = tempfile::tempdir().unwrap();
        stdfs::write(
            project_dir.path().join(DBT_PROJECT_YML),
            "flags:\n  manage_state: true\n",
        )
        .unwrap();

        assert!(get_manage_state_with_env(
            &CommonArgs::default(),
            project_dir.path(),
            Some("   "),
            None,
            false
        ));
    }

    #[test]
    fn project_flag_true_beats_user_settings_false() {
        let project_dir = tempfile::tempdir().unwrap();
        stdfs::write(
            project_dir.path().join(DBT_PROJECT_YML),
            "flags:\n  manage_state: true\n",
        )
        .unwrap();
        let home_dir = tempfile::tempdir().unwrap();
        stdfs::create_dir_all(home_dir.path().join(".dbt")).unwrap();
        let user_settings = home_dir.path().join(USER_SETTINGS_YML);
        stdfs::write(&user_settings, "flags:\n  manage_state: false\n").unwrap();

        assert!(get_manage_state_with_env(
            &CommonArgs::default(),
            project_dir.path(),
            None,
            Some(user_settings),
            false
        ));
    }

    /// An explicit `--manage-state` is the user acting right now, so it still wins
    /// over a stale project-level opt-out.
    #[test]
    fn manage_state_flag_overrides_project_flag_false() {
        let project_dir = tempfile::tempdir().unwrap();
        stdfs::write(
            project_dir.path().join(DBT_PROJECT_YML),
            "flags:\n  manage_state: false\n",
        )
        .unwrap();
        let common_args = CommonArgs {
            manage_state: true,
            ..Default::default()
        };

        assert!(get_manage_state_with_env(
            &common_args,
            project_dir.path(),
            None,
            None,
            false
        ));
    }

    #[test]
    fn compile_resolves_manage_state_from_project_flags() {
        let project_dir = tempfile::tempdir().unwrap();
        stdfs::write(
            project_dir.path().join(DBT_PROJECT_YML),
            "flags:\n  manage_state: true\n",
        )
        .unwrap();

        let compile_args = CompileArgs {
            common_args: CommonArgs {
                manage_state: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let system_args = SystemArgs {
            command: FsCommand::Compile,
            io: IoArgs::default(),
            from_main: false,
            exit_process_on_panic: false,
            num_threads: None,
            no_parallel: false,
            target: None,
        };
        let eval_args = compile_args.to_eval_args(
            system_args,
            project_dir.path(),
            &project_dir.path().join("target"),
        );

        assert!(
            eval_args.run_cache_service,
            "compile must resolve dbt State like the other auto-defer commands"
        );
    }

    #[test]
    fn env_false_overrides_project_flags() {
        let project_dir = tempfile::tempdir().unwrap();
        stdfs::write(
            project_dir.path().join(DBT_PROJECT_YML),
            "flags:\n  manage_state: true\n",
        )
        .unwrap();
        let common_args = CommonArgs::default();

        assert!(!get_manage_state_with_env(
            &common_args,
            project_dir.path(),
            Some("false"),
            None,
            false
        ));
        assert!(!get_manage_state_with_env(
            &common_args,
            project_dir.path(),
            Some("false"),
            None,
            true
        ));
    }

    fn core_cli(command: CoreCommand) -> Cli {
        Cli {
            command: Command::Core(command),
            common_args: CommonArgs::default(),
        }
    }

    #[test]
    fn detects_top_level_json_version() {
        assert!(CliParser::args_request_json_version([
            "dbt",
            "--format",
            "json",
            "--version"
        ]));
        assert!(CliParser::args_request_json_version([
            "dbt",
            "--version",
            "--format=json",
        ]));
        assert!(CliParser::args_request_json_version([
            "dbt",
            "--output=json",
            "--version",
        ]));
        assert!(!CliParser::args_request_json_version([
            "dbt",
            "list",
            "--format",
            "json",
            "--version"
        ]));
        assert!(!CliParser::args_request_json_version([
            "dbt",
            "--format",
            "text",
            "--version"
        ]));
    }

    #[test]
    fn version_json_uses_dbt_key() {
        let parser = CliParser::new("dbt", "2.0.0-preview.92", Box::new(NoopParser));

        assert_eq!(parser.version_json(), r#"{"dbt":"2.0.0-preview.92"}"#);
        assert_eq!(
            parser.json_version_for_args(["dbt", "--format", "json", "--version"]),
            Some(r#"{"dbt":"2.0.0-preview.92"}"#.to_string())
        );
    }

    fn test_system_args(command: FsCommand) -> SystemArgs {
        SystemArgs {
            command,
            io: IoArgs::default(),
            from_main: false,
            exit_process_on_panic: false,
            num_threads: None,
            no_parallel: false,
            target: None,
        }
    }

    fn parse_core_command(args: &[&str]) -> CoreCommand {
        let cli = CliParser::new("dbt", "2.0.0", Box::new(NoopParser))
            .try_parse_from(std::iter::once("dbt").chain(args.iter().copied()))
            .expect("args should parse");
        match cli.command {
            Command::Core(cmd) => cmd,
            Command::Extension(_) => panic!("expected a core command"),
        }
    }

    #[test]
    fn top_level_freshness_command_parses_and_is_not_sources_only() {
        let cmd = parse_core_command(&["freshness", "--select", "stg_orders+"]);

        let CoreCommand::Freshness(args) = &cmd else {
            panic!("expected CoreCommand::Freshness, got {cmd:?}");
        };
        assert_eq!(
            args.common_args.select.as_deref(),
            Some(["stg_orders+".to_string()].as_slice())
        );
        assert!(!cmd.as_command().is_sources_only_freshness());
        assert_eq!(cmd.as_command(), FsCommand::Freshness);
    }

    #[test]
    fn top_level_freshness_command_supports_resource_type_filters() {
        let cmd = parse_core_command(&[
            "freshness",
            "--resource-type",
            "source",
            "--exclude-resource-type",
            "model",
        ]);

        let CoreCommand::Freshness(args) = &cmd else {
            panic!("expected CoreCommand::Freshness, got {cmd:?}");
        };
        assert_eq!(args.resource_type, Some(vec![ClapResourceType::Source]));
        assert_eq!(
            args.exclude_resource_type,
            Some(vec![ClapResourceType::Model])
        );

        let eval_args = args.to_eval_args(
            test_system_args(FsCommand::Freshness),
            Path::new("/tmp/in"),
            Path::new("/tmp/out"),
        );
        assert_eq!(eval_args.resource_types, vec![ClapResourceType::Source]);
        assert_eq!(
            eval_args.exclude_resource_types,
            vec![ClapResourceType::Model]
        );
    }

    #[test]
    fn nested_source_freshness_command_is_sources_only() {
        let cmd = parse_core_command(&["source", "freshness", "--select", "raw_orders"]);

        let CoreCommand::Source(args) = &cmd else {
            panic!("expected CoreCommand::Source, got {cmd:?}");
        };
        let SourceCommand::Freshness(freshness) = &args.command;
        assert_eq!(
            freshness.common_args.select.as_deref(),
            Some(["raw_orders".to_string()].as_slice())
        );
        assert!(cmd.as_command().is_sources_only_freshness());
        assert_eq!(cmd.as_command(), FsCommand::Source);
    }

    #[test]
    fn nested_source_freshness_still_injects_resource_type_source() {
        let cmd = parse_core_command(&["source", "freshness"]);
        let CoreCommand::Source(args) = &cmd else {
            panic!("expected CoreCommand::Source");
        };

        let eval_args = args.to_eval_args(
            test_system_args(FsCommand::Source),
            Path::new("/tmp/in"),
            Path::new("/tmp/out"),
        );

        assert_eq!(eval_args.phase, Phases::Freshness);
        assert_eq!(
            eval_args.select.as_ref().map(|s| s.to_string()),
            Some("resource_type:source".to_string())
        );
    }

    #[test]
    fn version_json_key_derives_from_command_name() {
        let parser = CliParser::new("dbt-oss", "1.9.0", Box::new(NoopParser));

        assert_eq!(parser.version_json(), r#"{"oss":"1.9.0"}"#);
    }

    #[test]
    fn json_version_returns_display_version_error() {
        let err = CliParser::new("dbt", "2.0.0-preview.92", Box::new(NoopParser))
            .try_parse_from(["dbt", "--format", "json", "--version"])
            .expect_err("json version should short-circuit parsing");

        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
        assert!(!err.use_stderr());
        assert_eq!(err.exit_code(), 0);
    }

    #[test]
    fn non_project_core_commands_do_not_require_a_project() {
        use CoreCommand::*;
        let non_project = [
            Init(InitArgs::default()),
            Man(ManArgs::default()),
            Docs(DocsArgs::default()),
            Login(LoginArgs::default()),
            State(StateArgs {
                common_args: CommonArgs::default(),
                subcommand: StateSubcommand::Explain(StateExplainArgs::default()),
            }),
            Completions(CompletionsArgs {
                shell: Shell::Bash,
                common_args: CommonArgs::default(),
            }),
            Internal(InternalArgs {
                command: InternalCommand::GetDistributionInfo(GetDistributionInfoArgs::default()),
            }),
        ];
        for command in non_project {
            let name = command.name();
            assert!(
                !core_cli(command).is_project_command(),
                "expected `{name}` to not be a project command"
            );
        }
    }

    #[test]
    fn project_core_commands_require_a_project() {
        use CoreCommand::*;
        // A representative sample of the core commands that fall under the
        // `Command::Core(_)` catch-all and therefore require a project.
        let project = [
            Deps(DepsArgs::default()),
            Parse(ParseArgs::default()),
            List(ListArgs::default()),
            Ls(ListArgs::default()),
            Compile(CompileArgs::default()),
            Run(RunArgs::default()),
            RunOperation(RunOperationArgs::default()),
            Test(TestArgs::default()),
            Seed(SeedArgs::default()),
            Snapshot(SnapshotArgs::default()),
            Show(ShowArgs::default()),
            Build(BuildArgs::default()),
            Clean(CleanArgs::default()),
            Clone(CloneArgs::default()),
            Debug(DebugArgs::default()),
            Retry(RetryArgs::default()),
        ];
        for command in project {
            let name = command.name();
            assert!(
                core_cli(command).is_project_command(),
                "expected `{name}` to be a project command"
            );
        }
    }

    #[test]
    fn send_anonymous_usage_stats_missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dbt_project.yml");
        assert!(send_anonymous_usage_stats_from_yaml(&path).is_none());
    }

    #[test]
    fn send_anonymous_usage_stats_opt_out_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dbt_project.yml");
        std::fs::write(&path, "flags:\n  send_anonymous_usage_stats: false\n").unwrap();
        assert_eq!(send_anonymous_usage_stats_from_yaml(&path), Some(false));
    }

    #[test]
    fn send_anonymous_usage_stats_opt_in_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dbt_project.yml");
        std::fs::write(&path, "flags:\n  send_anonymous_usage_stats: true\n").unwrap();
        assert_eq!(send_anonymous_usage_stats_from_yaml(&path), Some(true));
    }

    #[test]
    fn send_anonymous_usage_stats_malformed_yaml_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dbt_project.yml");
        std::fs::write(&path, "flags: ][[\n").unwrap();
        assert!(send_anonymous_usage_stats_from_yaml(&path).is_none());
    }

    #[test]
    fn send_anonymous_usage_stats_no_flags_key_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dbt_project.yml");
        std::fs::write(&path, "name: my_project\nversion: 1.0.0\n").unwrap();
        assert!(send_anonymous_usage_stats_from_yaml(&path).is_none());
    }

    /// Serializes tests that mutate `DBT_SEND_ANONYMOUS_USAGE_STATS`, since env vars are
    /// process-global and Rust runs tests in parallel within a binary.
    static SEND_STATS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn write_yaml_with_stats(dir: &Path, value: bool) -> PathBuf {
        let path = dir.join("dbt_project.yml");
        std::fs::write(
            &path,
            format!("flags:\n  send_anonymous_usage_stats: {value}\n"),
        )
        .unwrap();
        path
    }

    #[test]
    fn get_send_anonymous_usage_stats_for_project_opt_out_wins() {
        let dir = tempfile::tempdir().unwrap();
        // yaml says true, but the explicit `--no-` opt-out wins.
        write_yaml_with_stats(dir.path(), true);
        let args = CommonArgs {
            no_send_anonymous_usage_stats: true,
            ..Default::default()
        };
        assert!(!args.get_send_anonymous_usage_stats_for_project(dir.path()));
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn get_send_anonymous_usage_stats_for_project_env_false_overrides_yaml_true() {
        let _guard = SEND_STATS_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        write_yaml_with_stats(dir.path(), true);
        let args = CommonArgs::default();
        unsafe { env::set_var("DBT_SEND_ANONYMOUS_USAGE_STATS", "false") };
        let result = args.get_send_anonymous_usage_stats_for_project(dir.path());
        unsafe { env::remove_var("DBT_SEND_ANONYMOUS_USAGE_STATS") };
        assert!(!result);
    }

    #[test]
    #[allow(clippy::disallowed_methods)]
    fn get_send_anonymous_usage_stats_for_project_env_true_overrides_yaml_false() {
        let _guard = SEND_STATS_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        write_yaml_with_stats(dir.path(), false);
        let args = CommonArgs::default();
        unsafe { env::set_var("DBT_SEND_ANONYMOUS_USAGE_STATS", "true") };
        let result = args.get_send_anonymous_usage_stats_for_project(dir.path());
        unsafe { env::remove_var("DBT_SEND_ANONYMOUS_USAGE_STATS") };
        assert!(result);
    }

    #[test]
    fn get_send_anonymous_usage_stats_for_project_yaml_only_fallback() {
        let _guard = SEND_STATS_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Ensure no ambient env var leaks in.
        unsafe { env::remove_var("DBT_SEND_ANONYMOUS_USAGE_STATS") };
        let dir = tempfile::tempdir().unwrap();
        write_yaml_with_stats(dir.path(), false);
        let args = CommonArgs::default();
        assert!(!args.get_send_anonymous_usage_stats_for_project(dir.path()));
    }

    #[test]
    fn get_send_anonymous_usage_stats_for_project_defaults_true() {
        let _guard = SEND_STATS_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        unsafe { env::remove_var("DBT_SEND_ANONYMOUS_USAGE_STATS") };
        // No yaml file, no env, no opt-out → default true.
        let dir = tempfile::tempdir().unwrap();
        let args = CommonArgs::default();
        assert!(args.get_send_anonymous_usage_stats_for_project(dir.path()));
    }

    fn parse_internal_args(args: &[&str]) -> Result<InternalArgs, clap::Error> {
        InternalArgs::try_parse_from(std::iter::once("internal").chain(args.iter().copied()))
    }

    #[test]
    fn get_distribution_info_defaults_to_no_path_and_not_all() {
        let parsed = parse_internal_args(&["get-distribution-info"]).unwrap();
        let InternalCommand::GetDistributionInfo(args) = parsed.command;
        assert_eq!(args.path, None);
        assert!(!args.all);
    }

    #[test]
    fn get_distribution_info_accepts_a_path() {
        let parsed = parse_internal_args(&["get-distribution-info", "/some/dbt"]).unwrap();
        let InternalCommand::GetDistributionInfo(args) = parsed.command;
        assert_eq!(args.path, Some(PathBuf::from("/some/dbt")));
        assert!(!args.all);
    }

    #[test]
    fn get_distribution_info_accepts_all_flag() {
        let parsed = parse_internal_args(&["get-distribution-info", "--all"]).unwrap();
        let InternalCommand::GetDistributionInfo(args) = parsed.command;
        assert_eq!(args.path, None);
        assert!(args.all);

        let parsed = parse_internal_args(&["get-distribution-info", "-a"]).unwrap();
        let InternalCommand::GetDistributionInfo(args) = parsed.command;
        assert!(args.all);
    }

    #[test]
    fn get_distribution_info_path_and_all_are_mutually_exclusive() {
        let result = parse_internal_args(&["get-distribution-info", "--all", "/some/dbt"]);
        assert!(result.is_err());
    }

    /// Regression test for dbt-core#15685: both entrypoints carry
    /// `--log-format-file` from `CommonArgs` into `IoArgs`.
    #[test]
    fn cli_entrypoints_carry_log_format_file_into_io_args() {
        // `Cli::common_args()` reads the subcommand's args, not the top-level field.
        let cli = Cli {
            command: Command::Core(CoreCommand::Run(RunArgs {
                common_args: CommonArgs {
                    log_format: LogFormat::Text,
                    log_format_file: Some(LogFormat::Json),
                    ..Default::default()
                },
                ..Default::default()
            })),
            common_args: CommonArgs::default(),
        };

        for (entrypoint, args) in [("from_main", from_main(&cli)), ("from_lib", from_lib(&cli))] {
            assert_eq!(
                args.io.log_format_file,
                Some(LogFormat::Json),
                "expected `{entrypoint}` to carry log_format_file"
            );
            assert_eq!(
                args.io.log_format,
                LogFormat::Text,
                "expected `{entrypoint}` to leave log_format alone"
            );
        }
    }
}
