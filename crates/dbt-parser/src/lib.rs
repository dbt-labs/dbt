//! Resolver is a crate for resolving a dbt project. It is responsible
//! for resolving all project source files (i.e. models, seeds, tests, macros etc.)
//! and propagating all configuration properties.
//!
//! The result of resolution is a "parse" phase `manifest.json`, whereby
//! "parse" is used to describe the pre-compiled manifest output, which
//! features the results of having rendered the dbt sql to extract the
//! full project configuration.

#![deny(missing_docs)]

pub mod args;
/// Compile-time constants for the dbt-parser crate
pub mod constants;
/// DbtNamespace for intercepting dbt macro calls during parse phase
pub mod dbt_namespace;
#[cfg(test)]
mod dbt_project_config_tests;

/// FQN-based `dbt_project.yml` config resolution.
///
/// The implementation lives in `dbt-schemas` so that it can be shared with code
/// that runs before a manifest exists (e.g. the deps-time skill install pass),
/// which cannot depend on this crate. Re-exported here for existing call sites.
pub mod dbt_project_config {
    pub use dbt_schemas::schemas::project::{
        DbtProjectConfig, ProjectConfigResolver, RootProjectConfigs, build_root_project_configs,
        disallow_plus_prefix_from_flags, init_project_config, recur_build_dbt_project_config,
        strip_resource_paths_from_ref_path,
    };
}
/// Parallel dispatch utilities
pub mod parallel;
/// Python AST parsing utilities
pub mod python_ast;
/// Python file information collection
pub mod python_file_info;
/// Python model validation
pub mod python_validation;
/// Python AST visitor for extracting dbt function calls
pub mod python_visitor;
pub mod renderer;
#[cfg(test)]
mod renderer_test;
/// All of the individual resolve functions broken out into their own files
pub mod resolve;
pub mod resolver;
/// Hooks for extending resolver behavior
pub mod resolver_hooks;
pub mod sql_file_info;
pub mod tests;
mod unused_config_paths;
pub mod utils;
/// Validator functions for node configs
pub mod validation;
