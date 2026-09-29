//! Minimal Jinja environment for profile rendering.
//!
//! Implements `env_var` and `var` functions plus base filters via `dbt-jinja-filters`.
//! No dependency on `dbt-jinja-utils` or `dbt-common`.

use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_jinja_vars::Var;
use minijinja::{Environment, Value};
use serde::Serialize;

/// Build a bare minijinja [`Environment`] with the filters profiles.yml needs.
pub fn profile_environment() -> Environment<'static> {
    let mut env = Environment::new();
    dbt_jinja_filters::register_filters(&mut env);
    env
}

// ── Context ──────────────────────────────────────────────────────────

/// The rendering context passed to `env.render_str(...)`.
#[derive(Serialize)]
pub struct ProfileContext {
    pub env_var: Value,
    pub var: Value,
    pub context: Value,
}

impl ProfileContext {
    pub fn new(
        vars: BTreeMap<String, dbt_yaml::Value>,
        overrides: Arc<BTreeMap<String, String>>,
    ) -> Self {
        Self {
            env_var: Value::from_func_func("env_var", move |state, args| {
                // Use placeholder_on_secret_access=true so that DBT_ENV_SECRET_*
                // variables are substituted with a sentinel placeholder during
                // Jinja rendering; render_secrets() resolves them afterwards.
                let values = Arc::clone(&overrides);
                let lookup = move |name: &str| {
                    crate::resolve::lookup_env_override(&values, name)
                        .map(|value| Value::from(value.as_str()))
                };
                dbt_jinja_vars::env_var_with_lookup(true, Some(&lookup), None, state, args)
            }),
            var: Value::from_object(Var::new(vars)),
            context: Value::from_serialize(BTreeMap::<String, Value>::new()),
        }
    }
}
