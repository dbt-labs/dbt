use std::sync::Arc;
use std::{collections::BTreeMap, path::PathBuf};

pub use dbt_common::io_args::IoArgs;
use dbt_common::io_args::{EvalArgs, FsCommand, InternalPackageMode, Phases};
use dbt_common::warn_error_options::WarnErrorOptions;
use dbt_schemas::state::DbtState;

#[derive(Clone, Default)]
pub struct LoadArgs {
    pub command: FsCommand,
    pub io: IoArgs,
    // The profile directory to load the profiles from
    pub profiles_dir: Option<PathBuf>,
    // The directory to install packages
    pub packages_install_path: Option<PathBuf>,
    // The directory to install fs_internal packages
    pub internal_packages_install_path: Option<PathBuf>,
    // The profile to use
    pub profile: Option<String>,
    // The target within the profile to use for the dbt run
    pub target: Option<String>,
    // Whether to update dependencies
    pub update_deps: bool,
    // The directory to load the dbt project from
    pub vars: BTreeMap<String, dbt_yaml::Value>,
    /// AI coding agent(s) to install package skills for, from the CLI or
    /// `DBT_AI_PROVIDER`. `None` means fall back to the project's `flags:`.
    pub ai_provider: Option<Vec<String>>,
    /// Vars loaded from `vars.yml` at the root project (populated after the
    /// initial project load). Empty when no `vars.yml` is present.
    pub root_vars_from_file: BTreeMap<String, dbt_yaml::Value>,
    /// Whether this is the main command or a subcommand
    pub from_main: bool,
    /// threads
    pub threads: Option<usize>,
    /// Install dependencies
    pub install_deps: bool,
    /// add_package cli option
    pub add_package: Option<String>,
    /// upgrade dependencies
    pub upgrade: bool,
    /// generate lock file only
    pub lock: bool,
    // Whether to load only profiles
    pub debug_profile: bool,
    /// Whether to check package version requirements
    pub version_check: bool,
    /// Inline SQL to compile (from --inline flag)
    pub inline_sql: Option<String>,
    /// The raw CLI `warn_error`/`no_warn_error` value before flags are resolved.
    pub cli_warn_error: Option<bool>,
    /// The raw CLI/env `warn_error_options` value before flags are resolved.
    pub cli_warn_error_options: Option<WarnErrorOptions>,
    /// Enables persisting compare packages in private builds
    pub enable_persist_compare_package: bool,
    /// This is for incremental.
    /// The [DbtState] of the previouis compile.
    /// Setting this will cause the 'load' phase to skip a lot of work
    /// and only check the file in the root package.
    pub prev_dbt_state: Option<Arc<DbtState>>,
    /// Skip installation of private dependencies
    pub skip_private_deps: bool,
    /// How to load internal (embedded) dbt packages
    pub internal_package_mode: InternalPackageMode,
}

impl LoadArgs {
    pub fn from_eval_args(arg: &EvalArgs) -> Self {
        Self {
            command: arg.command,
            io: arg.io.clone(),
            profile: arg.profile.clone(),
            profiles_dir: arg.profiles_dir.clone(),
            packages_install_path: arg.packages_install_path.clone(),
            internal_packages_install_path: arg.internal_packages_install_path.clone(),
            target: arg.target.clone(),
            update_deps: arg.update_deps,
            ai_provider: arg.ai_provider.clone(),
            add_package: arg.add_package.clone(),
            upgrade: arg.upgrade,
            lock: arg.lock,
            vars: arg.vars.clone(),
            root_vars_from_file: BTreeMap::new(),
            from_main: arg.from_main,
            threads: arg.num_threads,
            install_deps: arg.phase == Phases::Deps,
            debug_profile: arg.phase == Phases::Debug,
            version_check: arg.version_check,
            inline_sql: None,
            cli_warn_error: None,
            cli_warn_error_options: None,
            enable_persist_compare_package: arg.command == FsCommand::Extension("compare"),
            prev_dbt_state: None,
            skip_private_deps: arg.skip_private_deps,
            internal_package_mode: arg.internal_package_mode.clone(),
        }
    }

    /// Set the inline SQL for compilation
    pub fn with_inline_sql(mut self, inline_sql: Option<String>) -> Self {
        self.inline_sql = inline_sql;
        self
    }

    pub fn with_cli_warn_error(mut self, cli_warn_error: Option<bool>) -> Self {
        self.cli_warn_error = cli_warn_error;
        self
    }

    pub fn with_cli_warn_error_options(
        mut self,
        cli_warn_error_options: Option<WarnErrorOptions>,
    ) -> Self {
        self.cli_warn_error_options = cli_warn_error_options;
        self
    }
}
