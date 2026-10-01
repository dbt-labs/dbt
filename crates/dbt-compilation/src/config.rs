use dbt_common::io_args::FsCommand;

/// Common configuration for compilation pipeline
#[derive(Clone)]
pub struct CompilationConfig {
    /// Whether to use the build cache to determine the schedule
    pub use_build_cache_for_scheduling: bool,
    /// Commands that support caching
    pub cacheable_commands: Vec<FsCommand>,
    /// Disables local compute checks
    pub disable_local_compute_checks: bool,
    /// When hydrating schemas, use the resolver state's view of the world
    pub use_resolver_state_deps: bool,
    /// When true, disables checking versions
    pub no_version_check: bool,
    /// When true, the schedule used when initializing
    /// a schema store is for all nodes in the project.
    pub use_full_schema_store: bool,
}

impl Default for CompilationConfig {
    fn default() -> Self {
        Self {
            use_build_cache_for_scheduling: true,
            cacheable_commands: vec![
                FsCommand::Parse,
                FsCommand::Compile,
                FsCommand::Run,
                FsCommand::Test,
                FsCommand::Extension("lineage"),
                FsCommand::Seed,
            ],
            disable_local_compute_checks: false,
            use_resolver_state_deps: false,
            no_version_check: false,
            use_full_schema_store: false,
        }
    }
}
