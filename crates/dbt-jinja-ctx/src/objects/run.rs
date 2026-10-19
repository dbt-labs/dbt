//! Run-phase `Object` impls — std-only types moved here from
//! `dbt-jinja-utils` so the typed run-phase ctx structs can hold them via
//! `JinjaObject<T>` rather than opaque `MinijinjaValue`.
//!
//! Currently moved: `HookConfig`.
//!
//! Pending later PRs (need handle traits or `dbt-common` decoupling to move):
//! * `RunConfig` → uses `dbt_schemas::schemas::project::ConfigKeys`.
//! * `WriteConfig` → uses `dbt_common::path::get_target_write_path` +
//!   `dbt_common::constants::DBT_RUN_DIR_NAME`.

use std::sync::Arc;

use minijinja::value::{Object, Value as MinijinjaValue};

/// `{{ pre_hooks[i] }}` / `{{ post_hooks[i] }}` — single hook entry from
/// the node's YAML config. Wraps the SQL string and the per-hook
/// `transaction` flag.
#[derive(Clone)]
pub struct HookConfig {
    /// SQL template the hook executes.
    pub sql: String,
    /// Whether this hook participates in the surrounding transaction.
    pub transaction: bool,
}

impl Object for HookConfig {
    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        match key.as_str() {
            Some("sql") => Some(MinijinjaValue::from(self.sql.clone())),
            Some("transaction") => Some(MinijinjaValue::from(self.transaction)),
            _ => None,
        }
    }
    fn render(self: &Arc<Self>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.sql)
    }
}

impl std::fmt::Debug for HookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HookConfig {{ sql: {} }}", self.sql)
    }
}
