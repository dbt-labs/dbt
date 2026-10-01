//! Shared Jinja variable and environment helpers for dbt.
//!
//! Provides `env_var`, `Var`, `VarFunction`, `ConfiguredVar`, and `DbtVars`
//! without depending on `dbt-common`, `dbt-jinja-utils`, or `dbt-schemas`.

mod configured_var;
mod dbt_vars;
mod env_var;
mod utils;
mod var;

pub use configured_var::ConfiguredVar;
pub use dbt_vars::DbtVars;
pub use env_var::{
    DBT_INTERNAL_ENV_VAR_PREFIX, DEFAULT_ENV_PLACEHOLDER, LookupFn, SECRET_ENV_VAR_PREFIX,
    SECRET_PLACEHOLDER, env_var, env_var_with_lookup,
};
pub use utils::{yml_timestamp_to_minijinja, yml_value_to_minijinja};
pub use var::{Var, VarFunction};
