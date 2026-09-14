use crate::{
    tracing::dbt_convert::log_level_filter_to_tracing,
    warn_error_options::{WarnErrorOptions, project_flags_get_value},
};
use clap::{
    ValueEnum,
    builder::{BoolishValueParser, TypedValueParser},
};
use dbt_adapter_core::{AdapterType, STATIC_ANALYSIS_SUPPORTED_ADAPTERS};
use dbt_base::{HashMap, HashSet};
use dbt_telemetry::NodeType;
use dbt_yaml::{JsonSchema, Value};
use pathdiff::diff_paths;
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::str::FromStr;
use std::{
    collections::BTreeMap,
    fmt::{self, Display},
    path::{Path, PathBuf},
    sync::Arc,
};
use strum::EnumIter;
use strum_macros::Display;
use tracing::level_filters::LevelFilter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LocalExecutionBackendKind {
    #[default]
    /// Execute on the remote warehouse (Snowflake, BigQuery, etc.)
    Remote,
    /// Run models in the current process
    Inline,
    /// Run models in a separate worker process
    Worker,
    /// Run models in a service
    Service,
}

impl LocalExecutionBackendKind {
    /// Whether this backend can execute against the adapter.
    pub fn is_supported_for_adapter(self, adapter: AdapterType) -> bool {
        matches!(
            (adapter, self),
            (_, Self::Remote)
                | (AdapterType::Snowflake, _)
                | (AdapterType::Bigquery, Self::Worker)
                | (AdapterType::Datafusion, Self::Inline)
        )
    }
}

#[derive(
    Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, ValueEnum, Display, Default,
)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ComputeArg {
    #[default]
    /// Execute on the remote warehouse (Snowflake, BigQuery, etc.)
    Remote,
    /// Run computations in-process
    Inline,
    /// Run computations in a separate, ephemeral worker process
    // `local` is the other accepted spelling, as in `Execute::from_str`. It is
    // an alias, not a variant, so that one value reaches the code that matches
    // on `Sidecar`, and so that we always serialize `sidecar`.
    #[serde(alias = "local")]
    #[value(alias = "local")]
    Sidecar,
    /// Run via the remote compute service (persistent workers/cluster).
    Service,
}

// Hand-written because schemars 0.8 drops `#[serde(alias)]`. Editors validate
// YAML against this schema, so it must list `local`. If it does not, an editor
// rejects a value that dbt accepts. Keep the descriptions in sync with the
// variant docs above.
impl schemars::JsonSchema for ComputeArg {
    fn schema_name() -> String {
        "ComputeArg".to_string()
    }

    fn json_schema(_gen: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        let variants: [(&[&str], &str); 4] = [
            (
                &["remote"],
                "Execute on the remote warehouse (Snowflake, BigQuery, etc.)",
            ),
            (&["inline"], "Run computations in-process"),
            (
                &["sidecar", "local"],
                "Run computations in a separate, ephemeral worker process",
            ),
            (
                &["service"],
                "Run via the remote compute service (persistent workers/cluster).",
            ),
        ];

        schemars::schema::Schema::Object(schemars::schema::SchemaObject {
            subschemas: Some(Box::new(schemars::schema::SubschemaValidation {
                one_of: Some(
                    variants
                        .iter()
                        .map(|(values, description)| {
                            schemars::schema::Schema::Object(schemars::schema::SchemaObject {
                                metadata: Some(Box::new(schemars::schema::Metadata {
                                    description: Some((*description).to_string()),
                                    ..Default::default()
                                })),
                                instance_type: Some(schemars::schema::InstanceType::String.into()),
                                enum_values: Some(
                                    values.iter().map(|v| (*v).into()).collect::<Vec<_>>(),
                                ),
                                ..Default::default()
                            })
                        })
                        .collect(),
                ),
                ..Default::default()
            })),
            ..Default::default()
        })
    }
}

impl From<ComputeArg> for LocalExecutionBackendKind {
    fn from(arg: ComputeArg) -> Self {
        match arg {
            ComputeArg::Remote => LocalExecutionBackendKind::Remote,
            ComputeArg::Inline => LocalExecutionBackendKind::Inline,
            ComputeArg::Sidecar => LocalExecutionBackendKind::Worker,
            ComputeArg::Service => LocalExecutionBackendKind::Service,
        }
    }
}

use crate::constants::{
    DBT_INFO_SCHEMA_DIR_NAME, DBT_INFO_SCHEMA_STAGING_DIR_NAME, DBT_TARGET_DIR_NAME, WARNING,
    default_index_dir, default_metadata_dir,
};
use crate::pretty_string::YELLOW;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum InternalPackageMode {
    /// Load internal packages from RustEmbed directly, no disk I/O (default).
    #[default]
    Embedded,
    /// Write embedded assets to disk, then load from disk (legacy).
    ForceWrite,
    /// Assume already on disk, just read (skip writing).
    #[value(alias = "read")]
    ReadFromDisk,
}

impl Display for InternalPackageMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.to_possible_value()
            .expect("no values are skipped")
            .get_name()
            .fmt(f)
    }
}
use crate::{
    constants::{DBT_GENERIC_TESTS_DIR_NAME, DBT_SNAPSHOTS_DIR_NAME},
    io_utils::StatusReporter,
    node_selector::{
        IndirectSelection, SelectExpression, SelectionCriteria, conjoin_expression,
        parse_model_specifiers,
    },
    tracing::invocation::with_invocation_mut,
};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ValueEnum, Serialize, Copy, Default)]
pub enum LogFormat {
    Text,
    Json,
    #[default]
    Default,
    Otel,
}

impl Display for LogFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text => write!(f, "text"),
            Self::Json => write!(f, "json"),
            Self::Default => write!(f, "default"),
            Self::Otel => write!(f, "otel"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, ValueEnum, Serialize, Copy, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Off => write!(f, "OFF"),
            Self::Error => write!(f, "ERROR"),
            Self::Warn => write!(f, "WARN"),
            Self::Info => write!(f, "INFO"),
            Self::Debug => write!(f, "DEBUG"),
            Self::Trace => write!(f, "TRACE"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FsCommand {
    /// Special value indicating no command was provided.
    /// Used in places where a command is optional to avoid using Option<>
    #[default]
    Unset,
    /// Standard dbt commands
    Init,
    Deps,
    Parse,
    List, // aka: Ls
    Compile,
    Check,
    Run,
    RunOperation,
    Test,
    Seed,
    Snapshot,
    Show,
    Build,
    Clean,
    Source,
    Freshness,
    Clone,
    System,
    Man,
    Login,
    Debug,
    Retry,
    Docs,
    State,
    Completions,
    /// Undocumented plumbing commands (e.g. `dbt internal get-distribution-info`)
    Internal,
    /// All other commands provided by private cli's
    Extension(&'static str),
}

impl FsCommand {
    pub const fn is_freshness_command(&self) -> bool {
        matches!(self, FsCommand::Source | FsCommand::Freshness)
    }

    pub const fn is_sources_only_freshness(&self) -> bool {
        matches!(self, FsCommand::Source)
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            FsCommand::Unset => "",
            FsCommand::Init => "init",
            FsCommand::Deps => "deps",
            FsCommand::Parse => "parse",
            FsCommand::List => "list",
            FsCommand::Compile => "compile",
            FsCommand::Check => "check",
            FsCommand::Run => "run",
            FsCommand::RunOperation => "run-operation",
            FsCommand::Test => "test",
            FsCommand::Seed => "seed",
            FsCommand::Snapshot => "snapshot",
            FsCommand::Show => "show",
            FsCommand::Build => "build",
            FsCommand::Clean => "clean",
            FsCommand::Source => "freshness",
            FsCommand::Freshness => "freshness",
            FsCommand::Clone => "clone",
            FsCommand::System => "system",
            FsCommand::Man => "man",
            FsCommand::Login => "login",
            FsCommand::Debug => "debug",
            FsCommand::Retry => "retry",
            FsCommand::Docs => "docs",
            FsCommand::State => "state",
            FsCommand::Completions => "completions",
            FsCommand::Internal => "internal",
            FsCommand::Extension(s) => s,
        }
    }

    /// Whether this command compiles every node in the project (as opposed to a narrower
    /// operation like `test`, `seed`, or `snapshot`, which act on a subset). Per-node artifacts
    /// derived from a full compile are only meaningful for these commands.
    pub const fn compiles_project(&self) -> bool {
        matches!(
            self,
            FsCommand::Compile | FsCommand::Check | FsCommand::Build | FsCommand::Run
        )
    }
}

// ----------------------------------------------------------------------------------------------
// IO Args
#[derive(Default, Clone)]
pub struct IoArgs {
    pub invocation_id: uuid::Uuid,
    pub otel_parent_span_id: Option<u64>,
    pub show: HashSet<ShowOptions>,
    pub is_compile: bool,
    pub in_dir: PathBuf,
    pub out_dir: PathBuf,
    /// Root directory for sidecar/DuckDB state. Defaults to out_dir if not set.
    /// Structure: {db_root}/db/state/{catalog}.db (persistent)
    ///            {db_root}/db/sessions/{session-id}/... (ephemeral)
    pub db_root: Option<PathBuf>,
    pub log_path: Option<PathBuf>,
    pub otel_file_name: Option<String>,
    pub otel_parquet_file_name: Option<String>,
    pub export_to_otlp: bool,
    pub log_format: LogFormat,
    pub log_format_file: Option<LogFormat>,
    pub log_level: Option<LogLevel>,
    pub log_level_file: Option<LogLevel>,
    pub log_file_max_bytes: u64,
    pub debug: bool,

    // Flags influencing error/warning behavior
    pub show_all_deprecations: bool,

    // Flag for deps to use Fusion-compatible downloads from Package Hub
    pub use_v2_compatible_package_downloads: bool,

    // Flag for deps to require a sha1-verified download from Package Hub
    pub require_hub_verified_downloads: bool,

    /// Optional status reporter for reporting status messages during execution
    pub status_reporter: Option<Arc<dyn StatusReporter>>,
    pub send_anonymous_usage_stats: bool,

    // internal fields
    pub show_timings: bool, // whether to show timings in the status messages
    pub host: String,
    pub port: u16,
}
impl IoArgs {
    pub fn is_generated_file(&self, rel_path: &Path) -> bool {
        // Get last component of out_dir (as_os_str returns None if out_dir is empty)
        let out_dir_last = self.out_dir.components().next_back();
        let rel_first = rel_path.components().next();
        out_dir_last == rel_first
    }

    pub fn max_log_verbosity(&self) -> LevelFilter {
        self.log_level
            .map(|lf| log_level_filter_to_tracing(&lf))
            .unwrap_or(LevelFilter::INFO)
    }

    pub fn max_file_log_verbosity(&self) -> LevelFilter {
        self.log_level_file
            .map(|lf| log_level_filter_to_tracing(&lf))
            .unwrap_or(LevelFilter::DEBUG)
    }

    /// OTel Parquet tracing is only enabled when explicitly requested via
    /// --otel/-parquet-file-name. The write_metadata flag no longer auto-enables it.
    pub fn otel_parquet_file_name(&self) -> Option<&str> {
        self.otel_parquet_file_name.as_deref()
    }

    // -----------------------------------------------------------------------------------------
    // Sidecar/DuckDB path helpers
    // -----------------------------------------------------------------------------------------

    /// Returns the db_root, defaulting to out_dir if not explicitly set.
    pub fn db_root(&self) -> &Path {
        self.db_root.as_deref().unwrap_or(&self.out_dir)
    }

    /// Path to persistent DuckDB state directory: {db_root}/db/state/
    pub fn db_state_dir(&self) -> PathBuf {
        self.db_root().join("db").join("state")
    }

    /// Path to a specific catalog's DuckDB file: {db_root}/db/state/{catalog}.db
    pub fn db_catalog_path(&self, catalog: &str) -> PathBuf {
        self.db_state_dir()
            .join(format!("{}.db", catalog.to_ascii_lowercase()))
    }

    /// Path to session-scoped directory: {db_root}/db/sessions/{invocation_id}/
    pub fn db_session_dir(&self) -> PathBuf {
        self.db_root()
            .join("db")
            .join("sessions")
            .join(self.invocation_id.to_string())
    }

    /// Path to session logs: {db_root}/db/sessions/{invocation_id}/logs/
    pub fn db_session_logs_dir(&self) -> PathBuf {
        self.db_session_dir().join("logs")
    }

    /// Path to session pull data: {db_root}/db/sessions/{invocation_id}/pull/
    pub fn db_session_pull_dir(&self) -> PathBuf {
        self.db_session_dir().join("pull")
    }

    /// Path to session pull data for a specific table:
    /// {db_root}/db/sessions/{invocation_id}/pull/{catalog}/{schema}/{table}/
    pub fn db_session_table_dir(&self, catalog: &str, schema: &str, table: &str) -> PathBuf {
        self.db_session_pull_dir()
            .join(catalog.to_ascii_lowercase())
            .join(schema.to_ascii_lowercase())
            .join(table.to_ascii_lowercase())
    }
}

impl fmt::Debug for IoArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IoArgs")
            .field("invocation_id", &self.invocation_id)
            .field("show", &self.show)
            .field("in_dir", &self.in_dir)
            .field("out_dir", &self.out_dir)
            .field("db_root", &self.db_root)
            .field("log_path", &self.log_path)
            .field("log_file_max_bytes", &self.log_file_max_bytes)
            .field("otel_file_name", &self.otel_file_name)
            .field("status_reporter", &self.status_reporter.is_some())
            .finish()
    }
}

impl IoArgs {
    /// Given a path, returns a string representation of that path that is
    /// suitable for display in terminal status messages.
    pub fn format_display_path(&self, path: &Path) -> String {
        let in_dir = &self.in_dir;
        let out_dir = &self.out_dir;

        if path.starts_with(in_dir)
            && let Some(relative_path) = diff_paths(path, in_dir)
        {
            return relative_path.to_string_lossy().to_string();
        }
        if path.starts_with(out_dir)
            && let Some(relative_path) = diff_paths(path, out_dir)
        {
            return relative_path.to_string_lossy().to_string();
        }
        if path.is_relative() {
            let target_path = in_dir.join(DBT_TARGET_DIR_NAME).join(path);
            if target_path.exists() {
                return format!("target/{}", path.to_string_lossy());
            }
        }

        path.to_string_lossy().to_string()
    }

    /// This function takes an artifact path, which may either be a workspace
    /// resource, or some generated temp location, and returns a path to its
    /// corresponding location in the workspace
    pub fn map_to_workspace_path(&self, path: &Path, resource_type: NodeType) -> PathBuf {
        if resource_type == NodeType::UnitTest || resource_type == NodeType::Snapshot {
            let special_component_idx = path.components().position(|c| {
                c.as_os_str() == DBT_GENERIC_TESTS_DIR_NAME
                    || c.as_os_str() == DBT_SNAPSHOTS_DIR_NAME
            });
            if let Some(idx) = special_component_idx {
                // FIXME: this is really a hack, the proper thing to do is to have a
                // semantic representation for each artifact that can generate workspace or
                // temporary paths
                self.out_dir
                    .join(path.components().skip(idx).collect::<PathBuf>())
            } else {
                self.out_dir.join(path)
            }
        } else {
            self.in_dir.join(path)
        }
    }

    pub fn should_show(&self, option: ShowOptions) -> bool {
        (self.show.contains(&option) || option == ShowOptions::All)
            // TODO: temporary logic to avoid showing skipped nodes for compile.
            // Should be centralized across all commands, progress message types, and options.
            && (option != ShowOptions::Completed
                || !self.is_compile
                || self.debug)
    }
}
// ----------------------------------------------------------------------------------------------
// System Args
#[derive(Clone, Debug)]
pub struct SystemArgs {
    pub command: FsCommand,
    pub io: IoArgs,
    pub from_main: bool,
    pub exit_process_on_panic: bool,
    pub num_threads: Option<usize>,
    /// Request sequential task execution, decoupled from `num_threads`.
    /// Drives the binary entrypoint's single-worker tokio runtime and the
    /// task-scheduler's sequential visitor.
    pub no_parallel: bool,
    pub target: Option<String>,
}

// ----------------------------------------------------------------------------------------------
// Eval Args
#[derive(Clone, Default)]
pub struct EvalArgs {
    // The command to run
    pub command: FsCommand,
    // io
    pub io: IoArgs,
    // The profile directory to load the profiles from
    pub profiles_dir: Option<PathBuf>,
    // The directory to install packages
    pub packages_install_path: Option<PathBuf>,
    // A package to add to deps
    pub add_package: Option<String>,
    // Upgrade deps
    pub upgrade: bool,
    // Generate lock file only
    pub lock: bool,
    // The profile to use
    pub profile: Option<String>,
    // The target within the profile to use for the dbt run
    pub target: Option<String>,
    // Vars to pass to the jinja environment
    pub vars: BTreeMap<String, Value>,
    /// AI coding agent(s) to install package skills for. Unset means skills are
    /// discovered but not installed.
    pub ai_provider: Option<Vec<String>>,
    // Stop as soon as this stage is reached
    pub phase: Phases,
    // Display rows in different formats
    pub format: DisplayFormat,
    /// Limiting number of shown rows. None means no limit, run with --limit -1 to remove limit
    pub limit: Option<usize>,
    /// called as bin or as library
    pub from_main: bool,
    /// The max number of threads to use in the dbt-runtime thread-pool.
    ///
    /// Not used to force sequential task execution — use `no_parallel` for that.
    pub num_threads: Option<usize>,
    /// Force sequential task execution and sequential parser rendering without
    /// constraining the connection pool. Set by `--no-parallel`.
    pub no_parallel: bool,
    /// yaml selector
    pub selector: Option<String>,
    /// Select nodes to operate on
    pub select: Option<SelectExpression>,
    /// Select nodes to exclude from selected nodes
    pub exclude: Option<SelectExpression>,
    /// Indirect selection mode
    pub indirect_selection: Option<IndirectSelection>,
    /// Show output keys
    pub output_keys: Vec<String>,
    /// Resource types to filter by
    pub resource_types: Vec<ClapResourceType>,
    /// Exclude nodes of a specific type
    pub exclude_resource_types: Vec<ClapResourceType>,
    /// Debug flag
    pub debug: bool,
    /// Set log file format, overriding the default and --log-format setting.
    pub log_format_file: Option<LogFormat>,
    /// Set logging format
    pub log_format: LogFormat,
    /// Set minimum log file severity, overriding the default and --log-level setting.
    pub log_level_file: Option<LogLevel>,
    /// Set minimum severity for console/log file
    pub log_level: Option<LogLevel>,
    /// Set 'log-path' for the current run, overriding 'DBT_LOG_PATH'.
    pub log_path: Option<PathBuf>,
    /// The output directory for all produced assets
    pub target_path: Option<PathBuf>,
    /// The directory to load the dbt project from
    pub project_dir: Option<PathBuf>,
    /// Suppress all non-error logging to stdout
    pub quiet: bool,
    /// Write JSON artifacts to disk
    pub write_json: bool,
    /// Write a catalog.json file to the target directory
    pub write_catalog: bool,
    /// Show schema on the command line
    pub schema: Vec<JsonSchemaTypes>,
    /// Maximum size (MiB) for seed files whose contents are hashed
    /// 1 MiB default); `0` means "no limit".
    pub maximum_seed_size_mib: u64,

    // -- fields from the private branch
    pub internal_packages_install_path: Option<PathBuf>,
    pub update_deps: bool,
    pub replay: Option<ReplayMode>,
    pub static_analysis: Option<StaticAnalysisKind>,
    /// A signal used to keep sidecar runners alive (no idle timeout).
    /// Set programmatically by `dbt-repl`; not exposed as a CLI flag on `dbt`.
    pub long_living: bool,
    pub skip_semantic_manifest_validation: bool,
    pub export_saved_queries: bool,
    pub task_cache_url: String,
    /// Whether dbt State management (auto-deferral) is enabled, resolved from
    /// `--manage-state`, `DBT_ENGINE_MANAGE_STATE`, or `flags.manage_state` in
    /// dbt_project.yml / user settings. Also surfaced on the invocation telemetry
    /// span as `manage_state`.
    pub run_cache_service: bool,
    pub run_cache_mode: RunCacheMode,
    pub optimize_tests: HashSet<OptimizeTestsOptions>,
    pub show_scans: bool,
    pub max_depth: usize,
    pub use_fqtn: bool,
    pub skip_unreferenced_table_check: bool,
    pub state: Option<PathBuf>,
    pub defer_state: Option<PathBuf>,
    pub connection: bool,
    pub macro_name: Option<String>,
    pub macro_args: BTreeMap<String, Value>,
    pub macro_sql: Option<String>,
    /// `run-operation --adapter <type>`: run against this non-default adapter
    /// instead of the target's default one.
    pub adapter_override: Option<String>,
    /// `show --query-id <id>`: fetch a previously completed LakeCompute query's
    /// result directly, instead of compiling/executing a query. Mutually
    /// exclusive with `inline`.
    pub query_id: Option<String>,
    pub warn_error: Option<bool>,
    pub warn_error_options: WarnErrorOptions,
    pub version_check: bool,
    pub introspect: bool,
    pub defer: bool,
    pub fail_fast: bool,
    pub empty: bool,
    pub sample: Option<String>,
    pub full_refresh: bool,
    /// Bind without a catalog; assume referenced tables/columns exist and
    /// infer schemas from usage. Set from `--infer-schemas` on `compile`.
    pub infer_schemas_and_typeless: bool,
    pub store_failures: bool,
    pub favor_state: bool,
    pub refresh_sources: bool,
    pub send_anonymous_usage_stats: bool,
    pub check_all: bool,
    // todo: temporary, until Sampling is public, maps (source) unique_id to renamed (database, schema, table)
    pub sample_renaming: BTreeMap<String, (String, String, String)>,
    pub local_execution_backend: LocalExecutionBackendKind,
    /// Does not apply to interactive checkpoints.
    pub skip_checkpoints: bool,
    /// Skip installation of private dependencies (useful for build conformance testing)
    pub skip_private_deps: bool,
    /// Override end datetime when generating microbatches
    pub event_time_end: Option<String>,
    /// Override start datetime when generating microbatches
    pub event_time_start: Option<String>,
    /// How to load internal (embedded) dbt packages
    pub internal_package_mode: InternalPackageMode,
    /// Whether to skip running post hook operations.
    /// Seed classifier propagation with Snowflake column tags and write
    /// propagated labels back to Snowflake post-Run.  Only meaningful on
    /// Snowflake adapter; combined with `write_index = true` to be valid.
    /// Mirrors the `--classify-with-warehouse-tags` CLI flag.
    pub classify_with_warehouse_tags: bool,
    pub skip_post_hooks: bool,
    /// Write metadata parquet epoch files (parse/nodes, compile/nodes, compile/columns, etc.)
    pub write_metadata: bool,
    /// Also write snapshot index parquet to target/private/index/ (implies write_metadata)
    pub write_index: bool,
    /// True when `write_index` came from a command's default rather than from the command
    /// line. The index itself is identical either way; this only suppresses the advisory
    /// naming the extra flags that would enrich it, which is noise for a user who never
    /// asked for an index (and fails the command under `--warn-error`).
    pub write_index_implied: bool,
    /// Directory for index parquet output (default: <target>/private/index/)
    pub index_dir: Option<PathBuf>,
    /// Directory for metadata parquet output (default: <target>/private/metadata/)
    pub metadata_dir: Option<PathBuf>,
    /// Write the dbt information schema to target/info_schema/ (implies write_metadata)
    pub generate_info_schema: bool,
    /// Directory for information schema parquet output (default: <target>/info_schema/)
    pub info_schema_dir: Option<PathBuf>,
    /// Whether to skip creating generic tests
    pub skip_creating_generic_tests: bool,
    /// Compute and write column-level lineage into compile/cll parquet (requires --write-metadata and --static-analysis strict)
    pub write_lineage: bool,
    /// Positional check names from `dbt check <name>...`. Empty means every check.
    ///
    /// A filter on which checks run, not a node selection: the parse-time gate still
    /// evaluates against the whole project's index. Narrowing the schedule to the check
    /// node instead would leave it querying an empty index and passing vacuously.
    pub check_names: Vec<String>,
    /// Skip the parse-time check gate (`dbt build --skip-checks`). Opt-out, no
    /// warning: the user asked to skip. The index is still written.
    pub skip_checks: bool,
    /// Always enable the linter.
    pub force_enable_linter: bool,
    /// Always enable formatter-fix diagnostics.
    pub force_enable_formatter_diagnostics: bool,
    /// Command that originated the execution.
    /// Used for extension commands that execute core commands like compilation.
    pub command_entrypoint: FsCommand,
}
impl fmt::Debug for EvalArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvalArgs")
            .field("in_dir", &self.io.in_dir)
            .field("out_dir", &self.io.out_dir)
            .field("profiles_dir", &self.profiles_dir)
            .field("packages_install_path", &self.packages_install_path)
            .field("target", &self.target)
            .field("vars", &self.vars)
            .field("show", &self.io.show)
            .field("optimize_tests", &self.optimize_tests)
            .field("store_failures", &self.store_failures)
            .field("stage", &self.phase)
            .field("format", &self.format)
            .field("limit", &self.limit)
            .field("invocation_id", &self.io.invocation_id)
            .field("select", &self.select)
            .field("exclude", &self.exclude)
            .field("command", &self.command)
            .field("from_main", &self.from_main)
            .field("num_threads", &self.num_threads)
            .field("output_keys", &self.output_keys)
            .field("indirect_selection", &self.indirect_selection)
            .field("command_entrypoint", &self.command_entrypoint)
            .finish()
    }
}

pub struct EvalArgsBuilder {
    pub args: EvalArgs,
}

impl EvalArgsBuilder {
    pub fn from_eval_args(args: &EvalArgs) -> Self {
        Self { args: args.clone() }
    }
}

impl EvalArgsBuilder {
    /// Configure additional arguments
    pub fn with_additional(
        self,
        target: String,
        threads: Option<usize>,
        adapter_type: AdapterType,
    ) -> Self {
        self.with_target(target)
            .with_threads(threads)
            .disable_static_analysis_if_not_supported(adapter_type)
    }

    fn with_target(mut self, target: String) -> Self {
        // Update span info as it is used in telemetry & TUI
        with_invocation_mut(|invocation| {
            if let Some(args) = invocation.eval_args.as_mut() {
                args.target = Some(target.clone());
            };
        });

        self.args.target = Some(target);
        self
    }

    pub fn with_threads(mut self, num_threads: Option<usize>) -> Self {
        // Update span info as it is used in telemetry & TUI
        with_invocation_mut(|invocation| {
            if let Some(args) = invocation.eval_args.as_mut() {
                args.num_threads = num_threads.map(|l| l as u64);
            };
        });

        self.args.num_threads = num_threads;
        self
    }

    /// Disable the static analysis for a specific adapter if the relevant dialect is unsupported.
    /// Otherwise, it's a noop
    pub fn disable_static_analysis_if_not_supported(mut self, adapter_type: AdapterType) -> Self {
        let supported = STATIC_ANALYSIS_SUPPORTED_ADAPTERS.contains(&adapter_type);

        // FIXME(serramatutu): there is a bug in Postgres' frontend parser that makes
        // all our recordings invalid if enable it, but we can't disable it otherwise
        // the recordings will break too. This line should be removed when we fix the
        // following for Postgres:
        // dbt1058: Column 'id' in node 'model.test.a' has a type mismatch. Overriding
        // 'int' with 'integer'.
        let skip = adapter_type == AdapterType::Postgres;

        if !supported && !skip {
            #[cfg(debug_assertions)]
            {
                println!(
                    "debug:warning=static analysis for adapter: {:?} is disabled",
                    adapter_type
                );
            }
            self.args.static_analysis = Some(StaticAnalysisKind::Off);
        }

        self
    }

    pub fn with_show_scans(mut self, show_scans: bool) -> Self {
        self.args.show_scans = show_scans;
        self
    }

    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.args.max_depth = max_depth;
        self
    }

    pub fn with_use_fqtn(mut self, use_fqtn: bool) -> Self {
        self.args.use_fqtn = use_fqtn;
        self
    }

    pub fn with_warn_error_options(
        mut self,
        warn_error: bool,
        warn_error_options: WarnErrorOptions,
    ) -> Self {
        self.args.warn_error = Some(warn_error);
        self.args.warn_error_options = warn_error_options;
        self
    }

    pub fn build(self) -> EvalArgs {
        self.args
    }
}

impl EvalArgs {
    /// Resolves the metadata output directory: `--metadata-dir` if set, else
    /// `<out_dir>/private/metadata`.
    pub fn metadata_dir(&self) -> PathBuf {
        self.metadata_dir
            .clone()
            .unwrap_or_else(|| default_metadata_dir(&self.io.out_dir))
    }

    /// Resolves the index output directory: `--index-dir` if set, else
    /// `<out_dir>/private/index`.
    pub fn index_dir(&self) -> PathBuf {
        self.index_dir
            .clone()
            .unwrap_or_else(|| default_index_dir(&self.io.out_dir))
    }

    /// Resolves the information schema output directory: `--info-schema-dir` if
    /// set, else `<out_dir>/info_schema`.
    ///
    /// The intermediate for building it is the flat index at [`index_dir`] when one
    /// is already present; otherwise the fallback at [`info_schema_staging_dir`].
    ///
    /// [`index_dir`]: Self::index_dir
    /// [`info_schema_staging_dir`]: Self::info_schema_staging_dir
    pub fn info_schema_dir(&self) -> PathBuf {
        self.info_schema_dir
            .clone()
            .unwrap_or_else(|| self.io.out_dir.join(DBT_INFO_SCHEMA_DIR_NAME))
    }

    /// Fallback intermediate for building the information schema, used only when
    /// there is no flat index at [`index_dir`] to reuse — e.g. under
    /// `--no-write-index`, or a `parse` that never builds one. Held beside the
    /// output directory rather than inside it, because it holds files in a
    /// different shape and must never be picked up by a caller globbing the
    /// information schema. When an index *is* present it is reused directly and
    /// this directory is never created.
    ///
    /// [`index_dir`]: Self::index_dir
    pub fn info_schema_staging_dir(&self) -> PathBuf {
        match &self.info_schema_dir {
            Some(dir) => dir.with_file_name(DBT_INFO_SCHEMA_STAGING_DIR_NAME),
            None => self.io.out_dir.join(DBT_INFO_SCHEMA_STAGING_DIR_NAME),
        }
    }

    // this could accept a SelectExpression in case we want to join more complex selections together.
    pub fn set_refined_node_selectors(mut self, predicate: Option<SelectionCriteria>) -> EvalArgs {
        // Convert SelectionCriteria to SelectExpression::Atom first
        let predicate_expr = predicate.map(SelectExpression::Atom);

        self.select = conjoin_expression(self.select.clone(), predicate_expr.clone());
        if self.exclude.is_some() {
            self.exclude = conjoin_expression(self.exclude.clone(), predicate_expr);
        }

        // Update span info as it is used in telemetry & TUI
        with_invocation_mut(|invocation| {
            if let Some(args) = invocation.eval_args.as_mut() {
                args.select = self.select.iter().map(|s| s.to_string()).collect();
                args.exclude = self.exclude.iter().map(|s| s.to_string()).collect();
            };
        });

        self
    }

    pub fn set_schema(mut self, schema: Vec<JsonSchemaTypes>) -> Self {
        self.schema = schema;
        self
    }

    pub fn set_connection(mut self, connection: bool) -> Self {
        self.connection = connection;
        self
    }

    pub fn invocation_source(&self) -> &'static str {
        match self.command_entrypoint {
            FsCommand::Extension(s) => s,
            _ => "cli",
        }
    }
}

// ----------------------------------------------------------------------------------------------
// Enums

#[derive(
    Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Default, ValueEnum, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[clap(rename_all = "snake_case")]
pub enum ClapResourceType {
    #[default]
    Model,
    Source,
    Seed,
    Snapshot,
    Test,
    UnitTest,
    Analysis,
    Function,
    SemanticModel,
    Metric,
    SavedQuery,
    Check,
}

impl Display for ClapResourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ClapResourceType::Model => "model",
            ClapResourceType::Source => "source",
            ClapResourceType::Seed => "seed",
            ClapResourceType::Snapshot => "snapshot",
            ClapResourceType::Test => "test",
            ClapResourceType::UnitTest => "unit_test",
            ClapResourceType::Analysis => "analysis",
            ClapResourceType::Function => "function",
            ClapResourceType::SemanticModel => "semantic_model",
            ClapResourceType::Metric => "metric",
            ClapResourceType::SavedQuery => "saved_query",
            ClapResourceType::Check => "check",
        };
        write!(f, "{s}")
    }
}

impl From<&ClapResourceType> for NodeType {
    fn from(value: &ClapResourceType) -> Self {
        match value {
            ClapResourceType::Model => NodeType::Model,
            ClapResourceType::Source => NodeType::Source,
            ClapResourceType::Seed => NodeType::Seed,
            ClapResourceType::Snapshot => NodeType::Snapshot,
            ClapResourceType::Test => NodeType::Test,
            ClapResourceType::UnitTest => NodeType::UnitTest,
            ClapResourceType::Analysis => NodeType::Analysis,
            ClapResourceType::Function => NodeType::Function,
            ClapResourceType::SemanticModel => NodeType::SemanticModel,
            ClapResourceType::Metric => NodeType::Metric,
            ClapResourceType::SavedQuery => NodeType::SavedQuery,
            ClapResourceType::Check => NodeType::Check,
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
    Hash,
    Eq,
    Ord,
    ValueEnum,
    Display,
    Default,
)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum Phases {
    Debug, // dbt debug
    Deps,  // dbt deps
    Parse, // dbt parse
    Format,
    Lint,
    Schedule,
    List, // dbt list
    Freshness,
    JinjaCheck, // dbt jinja-check
    Compile,    // dbt compile
    Show,       // dbt show
    Compare,    // dbt compare
    Sample,     // dbt sample
    Lineage,
    RunOperation,
    #[default]
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Hash, Eq, Display, EnumIter)]
#[serde(rename_all = "lowercase")]
pub enum JsonSchemaTypes {
    Selector(bool),
    Schema(bool),
    Project(bool),
    Profile(bool),
    #[serde(rename = "dbt_cloud")]
    #[strum(serialize = "dbt_cloud")]
    DbtCloud(bool),
    Packages(bool),
    Dependencies(bool),
    Telemetry(bool),
    Catalogs(bool),
}

impl JsonSchemaTypes {
    pub fn is_pre(&self) -> bool {
        match self {
            JsonSchemaTypes::Selector(is_pre)
            | JsonSchemaTypes::Schema(is_pre)
            | JsonSchemaTypes::Project(is_pre)
            | JsonSchemaTypes::Profile(is_pre)
            | JsonSchemaTypes::DbtCloud(is_pre)
            | JsonSchemaTypes::Packages(is_pre)
            | JsonSchemaTypes::Dependencies(is_pre)
            | JsonSchemaTypes::Telemetry(is_pre)
            | JsonSchemaTypes::Catalogs(is_pre) => *is_pre,
        }
    }

    pub fn get_schema_settings(&self) -> schemars::r#gen::SchemaSettings {
        match self {
            JsonSchemaTypes::Selector(_)
            | JsonSchemaTypes::Schema(_)
            | JsonSchemaTypes::Project(_)
            | JsonSchemaTypes::Profile(_)
            | JsonSchemaTypes::DbtCloud(_)
            | JsonSchemaTypes::Packages(_)
            | JsonSchemaTypes::Dependencies(_)
            | JsonSchemaTypes::Catalogs(_) => schemars::r#gen::SchemaSettings::default(),
            JsonSchemaTypes::Telemetry(_) => schemars::r#gen::SchemaSettings::draft07(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum ClapSchemaTypes {
    Selector,
    Schema,
    Project,
    Profile,
    #[value(name = "dbt_cloud")]
    DbtCloud,
    Packages,
    Dependencies,
    Telemetry,
    Catalogs,
}

impl ClapSchemaTypes {
    pub fn to_json_schema_types(&self, is_pre: bool) -> JsonSchemaTypes {
        match self {
            ClapSchemaTypes::Selector => JsonSchemaTypes::Selector(is_pre),
            ClapSchemaTypes::Schema => JsonSchemaTypes::Schema(is_pre),
            ClapSchemaTypes::Project => JsonSchemaTypes::Project(is_pre),
            ClapSchemaTypes::Profile => JsonSchemaTypes::Profile(is_pre),
            ClapSchemaTypes::DbtCloud => JsonSchemaTypes::DbtCloud(is_pre),
            ClapSchemaTypes::Packages => JsonSchemaTypes::Packages(is_pre),
            ClapSchemaTypes::Dependencies => JsonSchemaTypes::Dependencies(is_pre),
            ClapSchemaTypes::Telemetry => JsonSchemaTypes::Telemetry(is_pre),
            ClapSchemaTypes::Catalogs => JsonSchemaTypes::Catalogs(is_pre),
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
    Hash,
    Eq,
    ValueEnum,
    Default,
    EnumIter,
    Display,
)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum DisplayFormat {
    #[default]
    Table,
    Csv,
    Tsv,
    Json,
    NdJson,
    Yml,
    /// Output nodes as selector strings (e.g. "source:pkg.source_name.table_name")
    Selector,
    /// Output nodes as search names (node.search_name)
    Name,
    /// Output nodes as file paths (node.original_file_path)
    Path,
}

/// Output format for the list command. This is a subset of DisplayFormat
/// that only includes formats supported by the list command.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
    Hash,
    Eq,
    ValueEnum,
    Default,
    EnumIter,
    strum_macros::IntoStaticStr,
)]
#[strum(serialize_all = "lowercase")]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum ListOutputFormat {
    /// Output nodes as JSON objects with customizable keys
    Json,
    /// Output nodes as selector strings (e.g. "source:pkg.source_name.table_name")
    #[default]
    Selector,
    /// Output nodes as search names (node.search_name)
    Name,
    /// Output nodes as file paths (node.original_file_path)
    Path,
}

impl From<ListOutputFormat> for DisplayFormat {
    fn from(format: ListOutputFormat) -> Self {
        match format {
            ListOutputFormat::Json => DisplayFormat::Json,
            ListOutputFormat::Selector => DisplayFormat::Selector,
            ListOutputFormat::Name => DisplayFormat::Name,
            ListOutputFormat::Path => DisplayFormat::Path,
        }
    }
}

impl TryFrom<DisplayFormat> for ListOutputFormat {
    type Error = ();

    fn try_from(format: DisplayFormat) -> Result<Self, Self::Error> {
        match format {
            DisplayFormat::Json => Ok(ListOutputFormat::Json),
            DisplayFormat::Selector => Ok(ListOutputFormat::Selector),
            DisplayFormat::Name => Ok(ListOutputFormat::Name),
            DisplayFormat::Path => Ok(ListOutputFormat::Path),
            _ => Err(()),
        }
    }
}

impl From<ListOutputFormat> for dbt_telemetry::ListOutputFormat {
    fn from(format: ListOutputFormat) -> Self {
        match format {
            ListOutputFormat::Json => Self::Json,
            ListOutputFormat::Selector => Self::Selector,
            ListOutputFormat::Name => Self::Name,
            ListOutputFormat::Path => Self::Path,
        }
    }
}

impl ListOutputFormat {
    pub fn supported_formats() -> impl Iterator<Item = &'static str> {
        use strum::IntoEnumIterator;

        Self::iter().map(|f| f.into())
    }

    pub fn supported_formats_display() -> String {
        Self::supported_formats().collect::<Vec<_>>().join(", ")
    }
}

#[derive(Debug, Clone)]
pub enum ReplayMode {
    /// Replay recordings generated from Mantle
    MantleReplay(PathBuf),
    /// Make recordings at the driver level
    FsRecord(PathBuf),
    /// Replay at the driver level
    FsReplay(PathBuf),
    /// Time Machine mode for cross-version compatibility testing
    FsTimeMachine(TimeMachineMode),
}

impl ReplayMode {
    /// Returns the time machine mode if this is a TimeMachine variant.
    pub fn as_time_machine(&self) -> Option<&TimeMachineMode> {
        match self {
            ReplayMode::FsTimeMachine(mode) => Some(mode),
            _ => None,
        }
    }

    /// Returns true when replaying a Time Machine recording.
    pub fn is_time_machine_replay(&self) -> bool {
        matches!(self, ReplayMode::FsTimeMachine(TimeMachineMode::Replay(_)))
    }
}

// -----------------------------------------------------------------------------
// Time Machine Args
// -----------------------------------------------------------------------------

/// Time Machine operating mode for cross-version compatibility testing.
///
/// This system records adapter-level behavior during a run and can replay
/// those recordings against newer versions to detect breaking changes.
#[derive(Debug, Clone)]
pub enum TimeMachineMode {
    /// Record adapter calls to a directory for later replay
    Record(TimeMachineRecordConfig),
    /// Replay adapter calls from a recorded artifact
    Replay(TimeMachineReplayConfig),
}

/// Configuration for Time Machine record mode.
#[derive(Debug, Clone)]
pub struct TimeMachineRecordConfig {
    /// Output directory for the recording.
    /// The CLI defaults this to `<project_target_dir>/time_machine`.
    pub output_path: PathBuf,
    /// The invocation ID for this run (used as the subdirectory name).
    pub invocation_id: uuid::Uuid,
    /// The invocation command that was executed (e.g., "dbt build --select ...")
    /// Stored in header.json for disambiguation when multiple recordings exist.
    pub invocation_command: Option<String>,
}

/// Configuration for Time Machine replay mode.
#[derive(Debug, Clone, Default)]
pub struct TimeMachineReplayConfig {
    /// Path to the recorded artifact directory
    pub artifact_path: PathBuf,
    /// Replay ordering mode (strict vs semantic)
    pub ordering: TimeMachineReplayOrdering,
}

/// Replay ordering mode for Time Machine.
///
/// Controls how recorded events are matched during replay.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum, Display, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum TimeMachineReplayOrdering {
    /// Events must match in exact recorded sequence order.
    /// This is the most restrictive mode and ensures deterministic replay.
    #[default]
    Strict,

    /// Events are matched based on semantic constraints.
    /// Write operations must match in sequence
    /// Read operations between writes can match flexibly
    /// This mode tolerates minor ordering variations between code versions.
    Semantic,
}

/// Parse TIME_MACHINE_MODE from string (for env var parsing).
impl FromStr for TimeMachineModeKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "record" => Ok(TimeMachineModeKind::Record),
            "replay" => Ok(TimeMachineModeKind::Replay),
            "off" | "" => Ok(TimeMachineModeKind::Off),
            _ => Err(format!(
                "Invalid TIME_MACHINE_MODE: '{}'. Expected: record, replay, or off",
                s
            )),
        }
    }
}

/// Simple enum for CLI/env var parsing (the full config is built from multiple args).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum, Display, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum TimeMachineModeKind {
    #[default]
    Off,
    Record,
    Replay,
}

#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Default,
    ValueEnum,
    Display,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum Runtime {
    #[default]
    Local,
    Remote,
}

#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Default,
    ValueEnum,
    Display,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum StaticAnalysisKind {
    #[value(hide = true)]
    Unsafe,
    #[serde(alias = "False", alias = "false", alias = "FALSE")]
    Off,
    Strict,
    #[default]
    Baseline,
    #[value(hide = true)]
    #[serde(alias = "True", alias = "true", alias = "TRUE")]
    On,
}

#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    ValueEnum,
    Display,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum StaticAnalysisOffReason {
    ConfiguredOff,
    UnableToFetchSchema,
    NoDownstream,
    /// No longer produced: a model with a custom materialization now honors its
    /// configured `static_analysis` (dbt-labs/fs#14357). Retained because
    /// `RunResultOutput` deserializes this field, so a `run_results.json`
    /// written by an older Fusion must still parse (e.g. `dbt retry`).
    CustomMaterialization,
}

impl FromStr for StaticAnalysisKind {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "unsafe" => Ok(StaticAnalysisKind::Unsafe),
            "off" => Ok(StaticAnalysisKind::Off),
            "baseline" => Ok(StaticAnalysisKind::Baseline),
            "strict" => Ok(StaticAnalysisKind::Strict),
            "on" => Ok(StaticAnalysisKind::On),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Debug, Display, Serialize, Deserialize, ValueEnum, Default)]
pub enum RunCacheMode {
    #[default]
    Noop,
    ReadWrite,
    WriteOnly,
}

impl RunCacheMode {
    pub fn use_cache(&self) -> bool {
        match self {
            RunCacheMode::ReadWrite => true,
            RunCacheMode::WriteOnly => false,
            RunCacheMode::Noop => false,
        }
    }

    pub fn write_cache(&self) -> bool {
        matches!(self, RunCacheMode::ReadWrite | RunCacheMode::WriteOnly)
    }
}

impl FromStr for RunCacheMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "noop" => Ok(RunCacheMode::Noop),
            "read-write" => Ok(RunCacheMode::ReadWrite),
            "write-only" => Ok(RunCacheMode::WriteOnly),
            _ => Err(format!("Invalid RunCacheMode: {s}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ValueEnum, EnumIter)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum OptimizeTestsOptions {
    TestAggregation,
    TestStaticAnalysis,
}

pub const SKIP_REDUNDANT_TESTS_ENV: &str = "DBT_ENGINE_SKIP_REDUNDANT_TESTS";
pub const BATCH_TESTS_ENV: &str = "DBT_ENGINE_BATCH_TESTS";

pub fn optimize_test_defaults_from_project_flags(
    project_flags: Option<&Value>,
) -> HashSet<OptimizeTestsOptions> {
    let mut optimize_tests = HashSet::default();
    insert_project_default_if_enabled(
        &mut optimize_tests,
        project_flags,
        OptimizeTestsOptions::TestStaticAnalysis,
        "skip_redundant_tests",
    );
    insert_project_default_if_enabled(
        &mut optimize_tests,
        project_flags,
        OptimizeTestsOptions::TestAggregation,
        "batch_tests",
    );
    optimize_tests
}

fn insert_project_default_if_enabled(
    optimize_tests: &mut HashSet<OptimizeTestsOptions>,
    project_flags: Option<&Value>,
    option: OptimizeTestsOptions,
    project_flag: &str,
) {
    if project_flags
        .and_then(|flags| project_flags_get_value(flags, project_flag))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        optimize_tests.insert(option);
    }
}

pub fn resolve_effective_optimize_tests(
    command: FsCommand,
    explicit_cli: &HashSet<OptimizeTestsOptions>,
    project_defaults: &HashSet<OptimizeTestsOptions>,
) -> HashSet<OptimizeTestsOptions> {
    resolve_effective_optimize_tests_with_env_lookup(
        command,
        explicit_cli,
        project_defaults,
        |name| std::env::var_os(name),
    )
}

fn resolve_effective_optimize_tests_with_env_lookup(
    command: FsCommand,
    explicit_cli: &HashSet<OptimizeTestsOptions>,
    project_defaults: &HashSet<OptimizeTestsOptions>,
    get_env: impl Fn(&str) -> Option<OsString>,
) -> HashSet<OptimizeTestsOptions> {
    let mut optimize_tests = explicit_cli.clone();
    // Not gated on Build: batching applies to every command that schedules generic
    // tests. Graph construction filters by command, so it is inert elsewhere.
    insert_effective_optimize_test_option(
        &mut optimize_tests,
        explicit_cli,
        project_defaults,
        &get_env,
        OptimizeTestsOptions::TestAggregation,
        BATCH_TESTS_ENV,
    );
    if command == FsCommand::Build {
        insert_effective_optimize_test_option(
            &mut optimize_tests,
            explicit_cli,
            project_defaults,
            &get_env,
            OptimizeTestsOptions::TestStaticAnalysis,
            SKIP_REDUNDANT_TESTS_ENV,
        );
    } else {
        optimize_tests.remove(&OptimizeTestsOptions::TestStaticAnalysis);
    }
    optimize_tests
}

fn insert_effective_optimize_test_option(
    optimize_tests: &mut HashSet<OptimizeTestsOptions>,
    explicit_cli: &HashSet<OptimizeTestsOptions>,
    project_defaults: &HashSet<OptimizeTestsOptions>,
    get_env: impl Fn(&str) -> Option<OsString>,
    option: OptimizeTestsOptions,
    env_var: &str,
) {
    if explicit_cli.contains(&option) {
        optimize_tests.insert(option);
        return;
    }

    if let Some(value) = get_env(env_var) {
        if parse_boolish_env(value.as_ref()).unwrap_or(false) {
            optimize_tests.insert(option);
        }
        return;
    }

    if project_defaults.contains(&option) {
        optimize_tests.insert(option);
    }
}

fn parse_boolish_env(value: &OsStr) -> Option<bool> {
    BoolishValueParser::new()
        .parse_ref(&clap::Command::new("dbt"), None, value)
        .ok()
}

/// Read a boolean environment variable, using the same value grammar as the boolean CLI flags
/// (`1`/`0`, `true`/`false`, `yes`/`no`, …).
///
/// Unset reads as `false`; set-but-unparseable is an error rather than a silent `false`, so a typo
/// cannot quietly turn a feature off.
pub fn env_flag_enabled(name: &str) -> crate::FsResult<bool> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(false);
    };
    parse_boolish_env(value.as_ref()).ok_or_else(|| {
        crate::fs_err!(
            crate::ErrorCode::InvalidConfig,
            "{name} must be a boolean (1/0, true/false, yes/no), got '{}'",
            value.to_string_lossy()
        )
    })
}

/// Read an environment variable holding a path, treating empty as unset.
pub fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Opts in to selecting the adapter a node runs on: the `+adapter` config and `--adapter`.
pub const MULTI_ADAPTER_ENV: &str = "DBT_ENGINE_EXPERIMENTAL_MULTI_ADAPTER";

/// Opts in to `+compute: local` (or its `sidecar` spelling) on an individual unit test. The
/// run-wide `--compute` flag is a separate, older knob and is not covered by this gate.
pub const LOCAL_UNIT_TESTS_ENV: &str = "DBT_ENGINE_EXPERIMENTAL_LOCAL_UNIT_TESTS";

/// Whether an experimental feature's opt-in environment variable is set. Unset or malformed
/// reads as off; a malformed value also warns.
///
/// Names passed here must be registered in `USED_ENGINE_ENV_VARS` (`dbt-main/src/vars.rs`),
/// or `validate_engine_env_vars` rejects them at startup.
fn experimental_gate_enabled(name: &str) -> bool {
    match env_flag_enabled(name) {
        Ok(enabled) => enabled,
        Err(e) => {
            crate::tracing::dbt_emit::emit_warn_log_message(
                crate::ErrorCode::InvalidConfig,
                e.to_string(),
            );
            false
        }
    }
}

/// Whether multi-adapter support is opted in to. See [`MULTI_ADAPTER_ENV`].
pub fn multi_adapter_enabled() -> bool {
    experimental_gate_enabled(MULTI_ADAPTER_ENV)
}

/// Whether per-unit-test local execution is opted in to. See [`LOCAL_UNIT_TESTS_ENV`].
pub fn local_unit_tests_enabled() -> bool {
    experimental_gate_enabled(LOCAL_UNIT_TESTS_ENV)
}

pub const LATEST_VERSION_POINTER_ENABLED_BY_DEFAULT_ENV: &str =
    "DBT_LATEST_VERSION_POINTER_ENABLED_BY_DEFAULT";

pub fn resolve_latest_version_pointer_enabled_by_default(project_flags: Option<&Value>) -> bool {
    resolve_latest_version_pointer_enabled_by_default_with_env_lookup(project_flags, |name| {
        std::env::var_os(name)
    })
}

fn resolve_latest_version_pointer_enabled_by_default_with_env_lookup(
    project_flags: Option<&Value>,
    get_env: impl Fn(&str) -> Option<OsString>,
) -> bool {
    if let Some(value) = get_env(LATEST_VERSION_POINTER_ENABLED_BY_DEFAULT_ENV) {
        if let Some(resolved) = parse_boolish_env(value.as_ref()) {
            return resolved;
        }
    }

    if let Some(enabled) = project_flags
        .and_then(|flags| {
            project_flags_get_value(flags, "latest_version_pointer_enabled_by_default")
        })
        .and_then(Value::as_bool)
    {
        return enabled;
    }

    true
}

pub const REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT_ENV: &str =
    "DBT_ENGINE_REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT";

/// Whether an unqualified `ref()` / `source()` / `function()` from a node inside an
/// installed package searches that package before the root project.
///
/// Defaults to `true`, which is Fusion's established behavior. Set it to `false` for
/// dbt-core's candidate order, where a root-project definition wins instead.
pub fn resolve_require_ref_searches_node_package_before_root(
    project_flags: Option<&Value>,
) -> bool {
    resolve_require_ref_searches_node_package_before_root_with_env_lookup(project_flags, |name| {
        std::env::var_os(name)
    })
}

fn resolve_require_ref_searches_node_package_before_root_with_env_lookup(
    project_flags: Option<&Value>,
    get_env: impl Fn(&str) -> Option<OsString>,
) -> bool {
    if let Some(value) = get_env(REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT_ENV) {
        if let Some(resolved) = parse_boolish_env(value.as_ref()) {
            return resolved;
        }
    }

    if let Some(enabled) = project_flags
        .and_then(|flags| {
            project_flags_get_value(flags, "require_ref_searches_node_package_before_root")
        })
        .and_then(Value::as_bool)
    {
        return enabled;
    }

    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ValueEnum, EnumIter)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lowercase")]
pub enum ShowOptions {
    Progress,
    ProgressHydrate,
    ProgressParse,
    ProgressRender,
    ProgressAnalyze,
    ProgressRun,
    Completed,
    InputFiles,
    Manifest,
    Schedule,
    Nodes,
    Instructions,
    SourcedSchemas,
    Schema,
    Data,
    Verdict,
    Stats,
    Lineage,
    All,
    None,
    // hidden internal-only:
    RawLineage,
    TaskGraph,
}

#[derive(ValueEnum, Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PersistTarget {
    #[default]
    Warehouse,
    Local,
}
impl Display for PersistTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            PersistTarget::Warehouse => "warehouse",
            PersistTarget::Local => "local",
        };
        write!(f, "{s}")
    }
}
// ----------------------------------------------------------------------------------------------
pub fn check_selector(selector: &str) -> Result<String, String> {
    // Parity with dbt-core: a delimiter-only value (e.g. "," or ",,") must not
    // hard-error at CLI-parse time. Accept it here; parse_model_specifiers
    // handles the assembled list — either skipping the token when real selectors
    // are present, or synthesizing a literal no-match criterion when every
    // token is delimiter-only (runtime then emits dbt1092 + dbt1601).
    if selector.chars().all(|c| c == ',' || c.is_whitespace()) {
        return Ok(selector.to_string());
    }
    let query = vec![selector.to_string()];
    match parse_model_specifiers(&query) {
        Ok(_) => Ok(selector.to_string()),
        Err(e) => Err(e.pretty()),
    }
}

pub fn check_target(filename: &str) -> Result<String, String> {
    let path = Path::new(filename);
    let err = Err(format!(
        "Input file '{filename}' must have .sql, or .yml extension"
    ));
    // TODO check that this test is universal for all inputs...
    if path.is_dir() {
        Ok(filename.to_owned())
    } else if path.is_file() {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("yml") | Some("sql") => Ok(filename.to_owned()),
            Some(_) => err,
            None => err,
        }
    } else {
        err
    }
}

/// `{key:value}` (no space) parses in YAML as one scalar key with a null value, not
/// a pair — a colon-containing key with null is that collapse's unambiguous signature
/// (a real key never has a null value), so we split and re-parse it here (#12873).
///
/// This is only called for flow-style inputs (wrapped in {}). Block-style YAML is
/// unambiguous and doesn't need recovery; the YAML parser handles it correctly.
fn recover_unspaced_colon_pairs(mut btree: BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    let ambiguous_keys: Vec<String> = btree
        .iter()
        .filter(|(key, val)| key.contains(':') && val.is_null())
        .map(|(key, _)| key.clone())
        .collect();

    for full_key in ambiguous_keys {
        let Some((real_key, raw_value)) = full_key.split_once(':') else {
            continue;
        };
        let raw_value = raw_value.trim();
        let mut stderr = std::io::stderr();
        let _ = writeln!(
            stderr,
            "{}: --vars key '{full_key}' has no space after ':', so YAML treats it as a single \
             key with a null value rather than a 'key: value' pair. Recovered it as \
             '{real_key}: {raw_value}' — add a space after the colon to avoid relying on this.",
            YELLOW.apply_to(WARNING)
        );
        btree.remove(&full_key);
        btree.insert(
            real_key.to_string(),
            dbt_yaml::from_str::<Value>(raw_value).unwrap_or_else(|_| Value::from(raw_value)),
        );
    }

    btree
}

pub fn check_key_value_cli_arg(value: &str) -> Result<BTreeMap<String, Value>, String> {
    // Handle empty input
    if value.trim().is_empty() {
        return Err("Empty input is not valid".into());
    }

    // Strip outer quotes if present
    let vars = value.trim().trim_matches('\'');

    // Try parsing as YAML first. Both brace-wrapped flow style ({ key: value, ... })
    // and bare block style (key1: value1\nkey2: value2) are valid YAML mappings.
    // We rely on the YAML parser itself to reject non-mapping inputs (bare strings,
    // lists, malformed syntax).
    let yaml_str = vars.to_string();

    match dbt_yaml::from_str::<BTreeMap<String, Value>>(&yaml_str) {
        Ok(btree) => Ok(btree),
        Err(_) => {
            // If YAML parsing fails, try JSON
            match serde_json::from_str(&yaml_str) {
                Ok(btree) => Ok(btree),
                Err(_) => Err(
                    "Invalid YAML/JSON format. Expected format: 'key: value' or '{key: value, ..}'. Note both argument forms must be just one shell token"
                        .to_string(),
                ),
            }
        }
    }
}

pub fn check_key_value_cli_arg_with_recovery(
    value: &str,
) -> Result<BTreeMap<String, Value>, String> {
    let btree = check_key_value_cli_arg(value)?;
    // Only apply recovery for flow-style inputs (wrapped in {}). Block-style YAML
    // is unambiguous and doesn't need recovery; the YAML parser handles it correctly.
    let is_flow_style = value.trim().trim_matches('\'').starts_with('{');
    Ok(if is_flow_style {
        recover_unspaced_colon_pairs(btree)
    } else {
        btree
    })
}

pub fn check_env_var(vars: &str) -> Result<HashMap<String, String>, String> {
    let config = vars;
    if config.starts_with('{') {
        let yaml_hashmap: Result<HashMap<String, String>, dbt_yaml::Error> =
            dbt_yaml::from_str(config);

        match yaml_hashmap {
            Ok(x) => Ok(x),
            Err(err) => Err(err.to_string()),
        }
    } else {
        let path = Path::new(config);
        if path.is_file() {
            if path.extension().unwrap() == "yml" {
                match fs::read_to_string(path) {
                    Ok(yaml_data) => {
                        let yaml_hashmap: Result<HashMap<String, String>, dbt_yaml::Error> =
                            dbt_yaml::from_str(&yaml_data);

                        match yaml_hashmap {
                            Ok(x) => Ok(x),
                            Err(err) => Err(err.to_string()),
                        }
                    }
                    Err(err) => Err(err.to_string()),
                }
            } else {
                Err("File must have a .yml extension".into())
            }
        } else {
            Err("Value must be a .yml file or a yml string like so: '{ dialect: trino }'".into())
        }
    }
}

pub fn validate_project_name(name: &str) -> Result<String, String> {
    // Check if the name contains only letters, digits, and underscores
    if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        Ok(name.to_string())
    } else {
        Err(format!(
            "{name} is not a valid project name. Only letters, digits and underscore are valid characters in a project name."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_execution_backend_support_matrix() {
        let cases: &[(LocalExecutionBackendKind, &[AdapterType])] = &[
            (
                LocalExecutionBackendKind::Inline,
                &[AdapterType::Snowflake, AdapterType::Datafusion],
            ),
            (
                LocalExecutionBackendKind::Worker,
                &[AdapterType::Snowflake, AdapterType::Bigquery],
            ),
            (
                LocalExecutionBackendKind::Service,
                &[AdapterType::Snowflake],
            ),
        ];

        for (adapter, _) in AdapterType::iter_with_names() {
            assert!(
                LocalExecutionBackendKind::Remote.is_supported_for_adapter(adapter),
                "remote must support {adapter}"
            );
            for (backend, supported_adapters) in cases {
                assert_eq!(
                    backend.is_supported_for_adapter(adapter),
                    supported_adapters.contains(&adapter),
                    "unexpected {backend:?} support for {adapter}"
                );
            }
        }
    }

    fn optimize_tests_with_env(
        command: FsCommand,
        explicit_cli: &HashSet<OptimizeTestsOptions>,
        project_defaults: &HashSet<OptimizeTestsOptions>,
        env: &[(&str, &str)],
    ) -> HashSet<OptimizeTestsOptions> {
        resolve_effective_optimize_tests_with_env_lookup(
            command,
            explicit_cli,
            project_defaults,
            |name| {
                env.iter()
                    .find_map(|(key, value)| (*key == name).then(|| OsString::from(*value)))
            },
        )
    }

    fn project_defaults(yaml: &str) -> HashSet<OptimizeTestsOptions> {
        let project_flags: Value = dbt_yaml::from_str(yaml).unwrap();
        optimize_test_defaults_from_project_flags(Some(&project_flags))
    }

    fn latest_version_pointer_with_env(
        project_flags_yaml: Option<&str>,
        env: &[(&str, &str)],
    ) -> bool {
        let project_flags =
            project_flags_yaml.map(|yaml| dbt_yaml::from_str::<Value>(yaml).unwrap());
        resolve_latest_version_pointer_enabled_by_default_with_env_lookup(
            project_flags.as_ref(),
            |name| {
                env.iter()
                    .find_map(|(key, value)| (*key == name).then(|| OsString::from(*value)))
            },
        )
    }

    fn require_ref_searches_node_package_before_root_with_env(
        project_flags_yaml: Option<&str>,
        env: &[(&str, &str)],
    ) -> bool {
        let project_flags =
            project_flags_yaml.map(|yaml| dbt_yaml::from_str::<Value>(yaml).unwrap());
        resolve_require_ref_searches_node_package_before_root_with_env_lookup(
            project_flags.as_ref(),
            |name| {
                env.iter()
                    .find_map(|(key, value)| (*key == name).then(|| OsString::from(*value)))
            },
        )
    }

    #[test]
    fn require_ref_searches_node_package_before_root_defaults_to_fusion_order() {
        assert!(require_ref_searches_node_package_before_root_with_env(
            None,
            &[]
        ));
        assert!(require_ref_searches_node_package_before_root_with_env(
            Some("some_other_flag: true\n"),
            &[]
        ));
    }

    #[test]
    fn require_ref_searches_node_package_before_root_reads_project_flag() {
        assert!(require_ref_searches_node_package_before_root_with_env(
            Some("require_ref_searches_node_package_before_root: true\n"),
            &[]
        ));
        assert!(!require_ref_searches_node_package_before_root_with_env(
            Some("require_ref_searches_node_package_before_root: false\n"),
            &[]
        ));
    }

    #[test]
    fn require_ref_searches_node_package_before_root_env_overrides_project_flag() {
        // Env wins in both directions
        assert!(require_ref_searches_node_package_before_root_with_env(
            Some("require_ref_searches_node_package_before_root: false\n"),
            &[(REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT_ENV, "1")]
        ));
        assert!(!require_ref_searches_node_package_before_root_with_env(
            Some("require_ref_searches_node_package_before_root: true\n"),
            &[(REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT_ENV, "false")]
        ));
    }

    #[test]
    fn require_ref_searches_node_package_before_root_env_accepts_boolish_values() {
        for truthy in ["1", "true", "TRUE", "yes", "on"] {
            assert!(
                require_ref_searches_node_package_before_root_with_env(
                    None,
                    &[(REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT_ENV, truthy)]
                ),
                "expected {truthy} to enable the flag"
            );
        }
        for falsy in ["0", "false", "no", "off"] {
            assert!(
                !require_ref_searches_node_package_before_root_with_env(
                    None,
                    &[(REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT_ENV, falsy)]
                ),
                "expected {falsy} to disable the flag"
            );
        }
    }

    #[test]
    fn require_ref_searches_node_package_before_root_unparseable_env_falls_back() {
        // A typo must not silently flip the behavior; the project flag still wins
        assert!(!require_ref_searches_node_package_before_root_with_env(
            Some("require_ref_searches_node_package_before_root: false\n"),
            &[(REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT_ENV, "yep")]
        ));
    }

    #[test]
    fn optimize_tests_env_enables_skip_redundant_tests() {
        let resolved = optimize_tests_with_env(
            FsCommand::Build,
            &HashSet::default(),
            &HashSet::default(),
            &[(SKIP_REDUNDANT_TESTS_ENV, "1")],
        );

        assert!(resolved.contains(&OptimizeTestsOptions::TestStaticAnalysis));
        assert!(!resolved.contains(&OptimizeTestsOptions::TestAggregation));
    }

    #[test]
    fn optimize_tests_project_flags_enable_skip_redundant_tests() {
        let project_defaults = project_defaults("skip_redundant_tests: true\n");

        assert!(project_defaults.contains(&OptimizeTestsOptions::TestStaticAnalysis));
        assert!(!project_defaults.contains(&OptimizeTestsOptions::TestAggregation));
    }

    #[test]
    fn optimize_tests_absent_env_uses_project_defaults() {
        let project_defaults = project_defaults("skip_redundant_tests: true\n");

        let resolved = optimize_tests_with_env(
            FsCommand::Build,
            &HashSet::default(),
            &project_defaults,
            &[],
        );

        assert!(resolved.contains(&OptimizeTestsOptions::TestStaticAnalysis));
        assert!(!resolved.contains(&OptimizeTestsOptions::TestAggregation));
    }

    #[test]
    fn optimize_tests_env_false_overrides_project_true() {
        let project_defaults = project_defaults("skip_redundant_tests: true\n");

        let resolved = optimize_tests_with_env(
            FsCommand::Build,
            &HashSet::default(),
            &project_defaults,
            &[(SKIP_REDUNDANT_TESTS_ENV, "0")],
        );

        assert!(!resolved.contains(&OptimizeTestsOptions::TestStaticAnalysis));
    }

    #[test]
    fn optimize_tests_cli_true_wins_over_env_false() {
        let mut explicit_cli = HashSet::default();
        explicit_cli.insert(OptimizeTestsOptions::TestStaticAnalysis);

        let resolved = optimize_tests_with_env(
            FsCommand::Build,
            &explicit_cli,
            &HashSet::default(),
            &[(SKIP_REDUNDANT_TESTS_ENV, "false")],
        );

        assert!(resolved.contains(&OptimizeTestsOptions::TestStaticAnalysis));
    }

    #[test]
    fn optimize_tests_skip_redundant_tests_is_build_only() {
        let mut explicit_cli = HashSet::default();
        explicit_cli.insert(OptimizeTestsOptions::TestAggregation);
        explicit_cli.insert(OptimizeTestsOptions::TestStaticAnalysis);
        let project_defaults = project_defaults("skip_redundant_tests: true\n");

        let resolved = optimize_tests_with_env(
            FsCommand::Test,
            &explicit_cli,
            &project_defaults,
            &[(SKIP_REDUNDANT_TESTS_ENV, "1")],
        );

        assert!(resolved.contains(&OptimizeTestsOptions::TestAggregation));
        assert!(!resolved.contains(&OptimizeTestsOptions::TestStaticAnalysis));
    }

    #[test]
    fn optimize_tests_batch_tests_project_flag_applies_to_test_command() {
        let project_defaults = project_defaults("batch_tests: true\n");

        let resolved =
            optimize_tests_with_env(FsCommand::Test, &HashSet::default(), &project_defaults, &[]);

        assert!(resolved.contains(&OptimizeTestsOptions::TestAggregation));
    }

    #[test]
    fn optimize_tests_batch_tests_env_false_overrides_project_true() {
        let project_defaults = project_defaults("batch_tests: true\n");

        let resolved = optimize_tests_with_env(
            FsCommand::Build,
            &HashSet::default(),
            &project_defaults,
            &[(BATCH_TESTS_ENV, "0")],
        );

        assert!(!resolved.contains(&OptimizeTestsOptions::TestAggregation));
    }

    #[test]
    fn optimize_tests_batch_tests_cli_true_wins_over_env_false() {
        let mut explicit_cli = HashSet::default();
        explicit_cli.insert(OptimizeTestsOptions::TestAggregation);

        let resolved = optimize_tests_with_env(
            FsCommand::Build,
            &explicit_cli,
            &HashSet::default(),
            &[(BATCH_TESTS_ENV, "false")],
        );

        assert!(resolved.contains(&OptimizeTestsOptions::TestAggregation));
    }

    #[test]
    fn test_check_single_var() {
        let result = check_key_value_cli_arg("key: value").unwrap();
        let expected_result =
            BTreeMap::from([("key".to_string(), dbt_yaml::from_str("value").unwrap())]);

        assert_eq!(result, expected_result);
    }

    #[test]
    fn test_check_single_bracket_var() {
        let result = check_key_value_cli_arg("{key: value}").unwrap();
        let expected_result =
            BTreeMap::from([("key".to_string(), dbt_yaml::from_str("value").unwrap())]);

        assert_eq!(result, expected_result);
    }

    #[test]
    fn test_check_multiple_bracket_var() {
        let result = check_key_value_cli_arg("{key: value, key2: value2}").unwrap();
        let expected_result = BTreeMap::from([
            ("key".to_string(), dbt_yaml::from_str("value").unwrap()),
            ("key2".to_string(), dbt_yaml::from_str("value2").unwrap()),
        ]);

        assert_eq!(result, expected_result);
    }

    #[test]
    fn test_check_var_invalid() {
        let invalid_vars = vec![
            "key",       // Missing colon — YAML returns scalar string, not dict
            "key:value", // No space after colon — YAML returns scalar string, not dict
        ];

        for var in invalid_vars {
            assert!(
                check_key_value_cli_arg(var).is_err(),
                "Should have failed: {var}"
            );
        }
    }

    #[test]
    fn test_check_var_flow_style_without_space_after_colon() {
        // Flow-style: {key:value} without space after ':' collapses to a single null-valued key.
        // Recovery splits it back to the intended pair {key: value} (#12873).
        let result = check_key_value_cli_arg_with_recovery("{key:value}").unwrap();
        let expected = BTreeMap::from([("key".to_string(), dbt_yaml::from_str("value").unwrap())]);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_check_var_flow_style_complex_unspaced() {
        // Real example (#12873): {adf_run_id:<uuid>, start_date: X, end_date: Y}
        // Unspaced pair collapses; recovery extracts it while preserving spaced pairs.
        let result = check_key_value_cli_arg_with_recovery(
            "{adf_run_id:30578aee-7913-47b7-a36c-549a2ede5210, start_date: 12.07.2026, end_date: 09.08.2026}"
        ).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(
            result["adf_run_id"],
            dbt_yaml::from_str::<Value>("30578aee-7913-47b7-a36c-549a2ede5210").unwrap()
        );
        assert!(result.contains_key("start_date"));
        assert!(result.contains_key("end_date"));
    }

    #[test]
    fn test_check_var_flow_style_unspaced_numeric_value() {
        // The recovered value is re-parsed as a YAML scalar (not forced to a string), so
        // typed values still come through correctly, e.g. an unspaced numeric pair.
        // Uses _with_recovery since recovery now happens in the vars-specific handler.
        let result = check_key_value_cli_arg_with_recovery("{count:5}").unwrap();
        let expected = BTreeMap::from([("count".to_string(), dbt_yaml::from_str("5").unwrap())]);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_check_var_flow_style_key_with_colon_and_real_value_untouched() {
        // A deliberately colon-containing key with a real (non-null) value is NOT the
        // ambiguous collapse case — it already has an unambiguous ': ' separator — so
        // recovery must leave it alone rather than re-splitting it.
        let result = check_key_value_cli_arg("{ns:key: value}").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("ns:key"));
        assert_eq!(
            result["ns:key"],
            dbt_yaml::from_str::<Value>("value").unwrap()
        );
    }

    #[test]
    fn test_check_var_block_yaml_multikey() {
        // The primary bug fix for issue #402: multi-key block YAML without surrounding braces
        let result = check_key_value_cli_arg("key1: value1\nkey2: value2").unwrap();
        let expected = BTreeMap::from([
            ("key1".to_string(), dbt_yaml::from_str("value1").unwrap()),
            ("key2".to_string(), dbt_yaml::from_str("value2").unwrap()),
        ]);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_check_var_value_with_colons() {
        // Values containing colons are valid YAML (and valid in dbt-core / PyYAML).
        // The colon-count pre-check that was removed in the fix for issue #402
        // incorrectly rejected these.
        let result = check_key_value_cli_arg("key: value:with:colons").unwrap();
        let expected = BTreeMap::from([(
            "key".to_string(),
            dbt_yaml::from_str("value:with:colons").unwrap(),
        )]);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_check_var_block_yaml_key_with_double_colon() {
        // Block-style: keys can contain colons since the last ': ' is the separator.
        // Example: 'set_var::something: value' parses as key 'set_var::something'.
        let result = check_key_value_cli_arg("set_var::something: value").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("set_var::something"));
    }

    #[test]
    fn test_check_var_flow_style_key_with_double_colon() {
        // Same as above but in flow-map ('{...}') form; the spaced ': ' still acts as
        // the separator even though the key itself contains '::'.
        let result = check_key_value_cli_arg("{key::with::colons: value}").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("key::with::colons"));
    }

    #[test]
    fn test_validate_project_name_valid() {
        let valid_names = vec![
            "my_project",
            "project123",
            "Project_Name",
            "test_project_1",
            "a",
            "project_with_underscores_and_numbers123",
        ];

        for name in valid_names {
            assert_eq!(validate_project_name(name).unwrap(), name);
        }
    }

    #[test]
    fn test_validate_project_name_invalid() {
        let invalid_names = vec![
            "my-cool-project",      // Contains hyphen
            "project with spaces",  // Contains spaces
            "project.with.dots",    // Contains dots
            "project/with/slashes", // Contains slashes
            "project@symbol",       // Contains @ symbol
            "project#hash",         // Contains # symbol
        ];

        for name in invalid_names {
            let result = validate_project_name(name);
            assert!(result.is_err(), "Should have failed: {name}");
            assert_eq!(
                result.unwrap_err(),
                format!(
                    "{name} is not a valid project name. Only letters, digits and underscore are valid characters in a project name."
                )
            );
        }
    }

    // Parity with dbt-core on delimiter-only --select values: the CLI validator
    // must not hard-error on a per-value token like "," or ",,". Downstream
    // selection resolves this to zero matches + dbt1601 "Nothing to do".
    #[test]
    fn test_check_selector_allows_comma_only() {
        let result = check_selector(",,");
        assert!(
            result.is_ok(),
            "check_selector(\",,\") must be Ok to match dbt-core behavior, got: {result:?}"
        );
    }

    // Regression pin: normal selectors must keep passing the validator unchanged.
    #[test]
    fn test_check_selector_preserves_normal_selector() {
        let result = check_selector("tag:foo");
        assert!(
            result.is_ok(),
            "check_selector(\"tag:foo\") must remain Ok, got: {result:?}"
        );
    }

    #[test]
    fn test_static_analysis_deserializes_legacy_bool_strings() {
        assert_eq!(
            dbt_yaml::from_str::<StaticAnalysisKind>("\"False\"").unwrap(),
            StaticAnalysisKind::Off
        );
        assert_eq!(
            dbt_yaml::from_str::<StaticAnalysisKind>("\"True\"").unwrap(),
            StaticAnalysisKind::On
        );
    }

    #[test]
    fn latest_version_pointer_env_true_overrides_project_false() {
        let resolved = latest_version_pointer_with_env(
            Some("latest_version_pointer_enabled_by_default: false\n"),
            &[(LATEST_VERSION_POINTER_ENABLED_BY_DEFAULT_ENV, "true")],
        );

        assert!(resolved);
    }

    #[test]
    fn latest_version_pointer_env_false_overrides_project_true() {
        let resolved = latest_version_pointer_with_env(
            Some("latest_version_pointer_enabled_by_default: true\n"),
            &[(LATEST_VERSION_POINTER_ENABLED_BY_DEFAULT_ENV, "false")],
        );

        assert!(!resolved);
    }

    #[test]
    fn latest_version_pointer_absent_env_uses_project_defaults() {
        let resolved = latest_version_pointer_with_env(
            Some("latest_version_pointer_enabled_by_default: false\n"),
            &[],
        );

        assert!(!resolved);
    }

    #[test]
    fn latest_version_pointer_absent_env_and_project_defaults_true() {
        let resolved = latest_version_pointer_with_env(None, &[]);

        assert!(resolved);
    }

    #[test]
    fn latest_version_pointer_malformed_env_falls_back_to_project() {
        let resolved = latest_version_pointer_with_env(
            Some("latest_version_pointer_enabled_by_default: false\n"),
            &[(LATEST_VERSION_POINTER_ENABLED_BY_DEFAULT_ENV, "maybe")],
        );

        assert!(!resolved);
    }

    #[test]
    fn test_check_var_straightforward_plain_pair() {
        // Simple block-style: 'plain: valid' parses correctly as-is.
        let result = check_key_value_cli_arg_with_recovery("plain: valid").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("plain"));
        assert_eq!(
            result["plain"],
            dbt_yaml::from_str::<Value>("valid").unwrap()
        );
    }

    #[test]
    fn test_check_var_flow_style_explicit_null_value() {
        // Flow-style: {key: null} (no colon in key)
        // Recovery does not trigger; null is preserved correctly.
        let result = check_key_value_cli_arg("{key: null}").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("key"));
        assert!(result["key"].is_null());
    }

    #[test]
    fn test_check_var_flow_style_colon_key_explicit_null() {
        // Flow-style: {ns:key: null} recovers to {ns: key}.
        // Recovery splits on first ':' for unspaced collapse detection.
        let result = check_key_value_cli_arg_with_recovery("{ns:key: null}").unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("ns"));
        assert_eq!(result["ns"], dbt_yaml::from_str::<Value>("key").unwrap());
    }
}
