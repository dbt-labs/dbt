//! Typed context used to render documentation fields.
//!
//! The field names are `REQUIRED_DOCS_KEYS` in dbt-core `tests/unit/context/test_context.py`.

use minijinja::Value as MinijinjaValue;
use serde::Serialize;

/// The context dbt Core renders documentation fields with.
#[derive(Debug, Clone, Serialize)]
pub struct DocsContext {
    /// The context itself.
    pub context: MinijinjaValue,
    /// The context minus `context` and `builtins`.
    pub builtins: MinijinjaValue,
    /// dbt version string.
    pub dbt_version: MinijinjaValue,
    /// `var()`, bound to the root project's package namespace.
    pub var: MinijinjaValue,
    /// `env_var()`.
    pub env_var: MinijinjaValue,
    /// Key-surface parity only; see `build_docs_resolve_context`.
    #[serde(rename = "return")]
    pub return_fn: MinijinjaValue,
    /// `fromjson()`.
    pub fromjson: MinijinjaValue,
    /// `tojson()`.
    pub tojson: MinijinjaValue,
    /// `fromyaml()`.
    pub fromyaml: MinijinjaValue,
    /// `toyaml()`.
    pub toyaml: MinijinjaValue,
    /// `set()`.
    pub set: MinijinjaValue,
    /// `set_strict()`.
    pub set_strict: MinijinjaValue,
    /// `zip()`.
    pub zip: MinijinjaValue,
    /// `zip_strict()`.
    pub zip_strict: MinijinjaValue,
    /// `log()`.
    pub log: MinijinjaValue,
    /// Invocation start timestamp.
    pub run_started_at: MinijinjaValue,
    /// Invocation id.
    pub invocation_id: MinijinjaValue,
    /// Rendering thread name.
    pub thread_id: MinijinjaValue,
    /// The `modules` namespace.
    pub modules: MinijinjaValue,
    /// Resolved project flags.
    pub flags: MinijinjaValue,
    /// `print()`.
    pub print: MinijinjaValue,
    /// `diff_of_two_dicts()`.
    pub diff_of_two_dicts: MinijinjaValue,
    /// `local_md5()`.
    pub local_md5: MinijinjaValue,
    /// The active target.
    pub target: MinijinjaValue,
    /// The root project name.
    pub project_name: MinijinjaValue,
    /// `doc()`.
    pub doc: MinijinjaValue,
}
