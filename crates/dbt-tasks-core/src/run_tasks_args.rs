use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::PathBuf;

use dbt_common::fail_fast::FailFast;
use dbt_common::io_args::FsCommand;
use dbt_common::io_args::IoArgs;
use dbt_common::io_args::LocalExecutionBackendKind;
use dbt_common::io_args::OptimizeTestsOptions;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::io_args::{DisplayFormat, EvalArgs, ReplayMode};
use dbt_common::io_args::{Phases, RunCacheMode};
use dbt_common::node_selector::SelectExpression;
use dbt_common::static_analysis::{
    is_static_analysis_off_or_baseline, normalize_static_analysis_kind,
};
use dbt_common::warn_error_options::WarnErrorOptions;
use dbt_yaml::Spanned;

#[derive(Default)] // DO NOT ADD Clone HERE
pub struct RunTasksArgs {
    pub command: FsCommand,
    // The io args
    pub io: IoArgs,
    // The profile to use (user input)
    pub profile: Option<String>,
    // The profile directory to load the profiles from
    pub profiles_dir: Option<PathBuf>,
    // The directory to install packages
    pub packages_install_path: Option<PathBuf>,
    // The target within the profile to use for the dbt run (user input)
    pub target: Option<String>,
    // Resolved profile name (from DbtProfile after resolution)
    pub resolved_profile: String,
    // Resolved target name (from DbtProfile after resolution)
    pub resolved_target: String,
    // Resolved database name (from DbtProfile after resolution)
    pub resolved_database: String,
    // Resolved schema name (from DbtProfile after resolution)
    pub resolved_schema: String,
    // Whether to update dependencies
    pub update_deps: bool,
    // Vars to pass to the jinja environment
    pub vars: BTreeMap<String, dbt_yaml::Value>,
    // Display rows in different formats, this is .to_string on DisplayFormat; we use a string here to break dep. cycle
    pub format: DisplayFormat,
    /// Limiting number of shown rows. Run with --limit -1 to remove limit
    pub limit: Option<usize>,
    /// Whether to write json output
    pub write_json: bool,
    /// Whether to write metadata parquet epoch files during command execution.
    pub write_metadata: bool,
    /// Whether the Parquet index is being written (`--write-index`). Gates
    /// classifier propagation, which only has a consumer when the index is built.
    pub write_index: bool,
    /// Whether to seed classifier propagation with Snowflake column tags
    /// (Phase 1) and write propagated labels back to Snowflake on Run
    /// (Phase 3).  See `propagation_of_snowflake_tags.md` §3a.
    pub classify_with_warehouse_tags: bool,
    /// Whether to compute and write column-level lineage into compile/cll parquet.
    pub write_lineage: bool,
    /// Whether this is the main command or a subcommand
    pub from_main: bool,
    /// Number of threads: the cap on the `dbt-runtime` blocking pool, which
    /// bounds warehouse concurrency and parser rendering alike. Not used to
    /// force sequential task execution; see `no_parallel`.
    pub num_threads: usize,
    /// When true, the task graph is visited sequentially (one node at a time)
    /// regardless of `num_threads`. Use for deterministic test output.
    pub no_parallel: bool,
    /// Whether to replay from the execution cache
    pub replay: Option<ReplayMode>,
    /// Whether to compile only
    pub static_analysis: Spanned<StaticAnalysisKind>,
    /// Whether to skip the unreferenced tables
    pub skip_unreferenced_table_check: bool,

    /// Whether to favor state over current environment
    pub favor_state: bool,
    /// The mode to use for the run cache
    pub run_cache_mode: RunCacheMode,
    /// The url to use for the run cache
    pub task_cache_url: String,
    /// phase
    pub phase: Phases,
    /// Optional (resolved) sampling plan to locate local sampled data for sources
    pub sample_renaming: BTreeMap<String, (String, String, String)>,
    /// Parsed select expression from the invocation.
    pub select: Option<SelectExpression>,
    /// Parsed exclude expression from the invocation.
    pub exclude: Option<SelectExpression>,
    /// Backend used for local execution of runnable nodes
    pub local_execution_backend: LocalExecutionBackendKind,
    /// Sidecar/service should not time out. (Used by REPL to keep the runner alive across multiple commands.)
    pub long_living: bool,
    /// Whether to perform a full refresh (rebuild incremental models from scratch)
    pub full_refresh: bool,
    /// Bind without a catalog; assume referenced tables/columns exist and infer schemas from usage.
    pub infer_schemas_and_typeless: bool,
    /// Whether to run with `--empty` (creates relations with schema only, no data).
    pub empty: bool,
    /// If specified, the end datetime dbt uses to filter microbatch model inputs (exclusive).
    pub event_time_end: Option<String>,
    /// If specified, the start datetime dbt uses to filter microbatch model inputs (inclusive).
    pub event_time_start: Option<String>,
    /// The raw `--sample` spec (e.g. `"30 days"` or a `{start, end}` JSON range), if passed.
    pub sample: Option<String>,
    /// Per-invocation fail-fast signal.
    pub fail_fast: FailFast,
    /// Whether the user passed `--fail-fast`. The signal above is triggered on
    /// any error regardless of this flag; this records whether the flag itself
    /// was set so downstream policy can react (e.g. ignore `on_error: continue`).
    pub fail_fast_flag: bool,
    /// Whether to skip running post hook operations.
    pub skip_post_hooks: bool,
    /// Hidden test optimization flags.
    pub optimize_tests: dbt_common::collections::HashSet<OptimizeTestsOptions>,
    /// Whether the gRPC run-cache service is explicitly requested via CLI flag.
    pub run_cache_service: bool,
    /// Per-invocation warn-error options resolved before task execution.
    pub warn_error_options: WarnErrorOptions,
    /// Previous batch_results from run_results.json, populated during retry
    /// so that already-successful overloads can be skipped.
    pub previous_batch_results: HashMap<String, dbt_schemas::schemas::BatchResults>,
    /// Resolved metadata directory (`--metadata-dir` or `<out_dir>/metadata`). Carried here so a
    /// task can find it — `EvalArgs::metadata_dir()` is not reachable from the task layer, and
    /// deriving it from `out_dir` would silently ignore the override.
    pub metadata_dir: PathBuf,
    /// Resolved index directory (`--index-dir` or `<out_dir>/index`). See `metadata_dir`.
    pub index_dir: PathBuf,
    /// `show --query-id <id>`: fetch a previously completed LakeCompute query's
    /// result directly. See `EvalArgs::query_id`.
    pub query_id: Option<String>,
}

impl RunTasksArgs {
    pub fn from_eval_args(arg: &EvalArgs, fail_fast: FailFast) -> Box<Self> {
        let run_tasks_args = Self {
            command: arg.command,
            io: arg.io.clone(),
            metadata_dir: arg.metadata_dir(),
            index_dir: arg.index_dir(),
            query_id: arg.query_id.clone(),
            profile: arg.profile.clone(),
            profiles_dir: arg.profiles_dir.clone(),
            packages_install_path: arg.packages_install_path.clone(),
            target: arg.target.clone(),
            resolved_profile: String::new(),
            resolved_target: String::new(),
            resolved_database: String::new(),
            resolved_schema: String::new(),
            update_deps: arg.update_deps,
            vars: arg.vars.clone(),
            classify_with_warehouse_tags: arg.classify_with_warehouse_tags,
            format: arg.format,
            limit: arg.limit,
            write_json: arg.write_json,
            write_metadata: arg.write_metadata,
            write_index: arg.write_index,
            write_lineage: arg.write_lineage,
            from_main: arg.from_main,
            num_threads: arg.num_threads.unwrap_or(0),
            no_parallel: arg.no_parallel,
            replay: arg.replay.clone(),
            static_analysis: Spanned::new(normalize_static_analysis_kind(
                arg.static_analysis.unwrap_or_default(),
            )),
            skip_unreferenced_table_check: arg.skip_unreferenced_table_check,
            task_cache_url: arg.task_cache_url.clone(),
            favor_state: arg.favor_state,
            run_cache_mode: arg.run_cache_mode.clone(),
            sample_renaming: arg.sample_renaming.clone(),
            select: arg.select.clone(),
            exclude: arg.exclude.clone(),
            phase: arg.phase.clone(),
            local_execution_backend: arg.local_execution_backend,
            long_living: arg.long_living,
            full_refresh: arg.full_refresh,
            infer_schemas_and_typeless: arg.infer_schemas_and_typeless,
            event_time_start: arg.event_time_start.clone(),
            event_time_end: arg.event_time_end.clone(),
            sample: arg.sample.clone(),
            fail_fast,
            fail_fast_flag: arg.fail_fast,
            skip_post_hooks: arg.skip_post_hooks,
            optimize_tests: arg.optimize_tests.clone(),
            run_cache_service: arg.run_cache_service,
            warn_error_options: arg.warn_error_options.clone(),
            empty: arg.empty,
            previous_batch_results: Default::default(),
        };
        Box::new(run_tasks_args)
    }

    /// Populate resolved profile fields from DbtProfile
    pub fn with_resolved_profile(mut self, profile: &dbt_schemas::state::DbtProfile) -> Self {
        self.resolved_profile = profile.profile.clone();
        self.resolved_target = profile.target.clone();
        self.resolved_database = profile.database.clone();
        self.resolved_schema = profile.schema.clone();
        self
    }

    pub fn static_analysis_off(&self) -> Spanned<bool> {
        self.static_analysis
            .clone()
            .map(|static_analysis| static_analysis == StaticAnalysisKind::Off)
    }

    pub fn static_analysis_off_or_baseline(&self) -> Spanned<bool> {
        self.static_analysis
            .clone()
            .map(is_static_analysis_off_or_baseline)
    }

    pub fn is_runnable(&self) -> bool {
        matches!(
            self.command,
            FsCommand::Run
                | FsCommand::Test
                | FsCommand::Build
                | FsCommand::Seed
                | FsCommand::Snapshot
        )
    }
}

impl fmt::Debug for RunTasksArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompilerArgs")
            .field("command", &self.command)
            .field("in_dir", &self.io.in_dir)
            .field("out_dir", &self.io.out_dir)
            .field("profile", &self.profile)
            .field("profiles_dir", &self.profiles_dir)
            .field("packages_install_path", &self.packages_install_path)
            .field("target", &self.target)
            .field("update_deps", &self.update_deps)
            .field("show", &self.io.show)
            .field("vars", &self.vars)
            .field("from_main", &self.from_main)
            .field("invocation_id", &self.io.invocation_id)
            .field("num_threads", &self.num_threads)
            .field("replay", &self.replay)
            .field("format", &self.format)
            .field("limit", &self.limit)
            .field("run_cache_url", &self.task_cache_url)
            .field("run_cache_mode", &self.run_cache_mode)
            .finish()
    }
}
