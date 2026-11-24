//! Feature flags for dbt Fusion.
//!
//! Every flag is declared once as a [`FlagDef`] and resolved through a single
//! precedence chain into a [`Resolved`] value that records where it came from.
//! A flag's [`Stage`] controls where it can be set. Behavior change flags are flags
//! that have reached [`Stage::Behavior`]; they live in the same registry as
//! experimental and beta features.
//!
//! Precedence, highest first:
//!
//! 1. Remote value for a flag declared [`Remote::Force`] (kill switches)
//! 2. The flag's environment variable, `DBT_ENGINE_EXPERIMENTAL_<NAME>=1|0`
//!    ([`FlagDef::env_var`]), then the flag's `legacy_env`
//! 3. Beta flags only: `--beta-features a,b`, then `DBT_ENGINE_BETA_FEATURES=a,b`, then
//!    `beta_features: {a: true}` in the root project's `dbt_project.yml` `flags:`. The two
//!    lists only turn a feature on; the environment variable turns it off
//! 4. Behavior flags only: `<name>: <bool>` in the root project's `dbt_project.yml`
//!    `flags:`, then the settings of step 3, with a warning. A flag that was a beta
//!    feature keeps the setting a project gave it then
//! 5. Beta and behavior flags only: `user_settings.yml` `flags:`
//! 6. Remote value for a flag declared [`Remote::Default`] or [`Remote::Force`]
//! 7. The flag's code default
//!
//! So an experimental flag is set only by its environment variable (and by a remote value,
//! when it is declared to take one). A package's `flags:` never applies.
//!
//! A [`Stage::Graduated`] flag is always on and a [`Stage::Abandoned`] flag is always off.
//!
//! The binary resolves the invocation-scoped layers (1–3, 5, 6) once at startup
//! and [`install`]s them; call sites then read [`FlagDef::enabled`] or, for flags
//! users can set in `dbt_project.yml`, [`FlagDef::resolve_with_project`].

mod catalog;
mod def;
mod parse;
mod snapshot;

pub use catalog::{
    ALL, FRONTEND_PROTOCOL, GIT_DEPS_FAST_PATH, LAKECOMPUTE_PROFILE_SETUP,
    LATEST_VERSION_POINTER_ENABLED_BY_DEFAULT, LOCK_WITH_STATE, METRICFLOW_V2,
    REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT, USE_V2_EMPTY_SQL_REWRITE,
};
pub use def::{FlagDef, NoProjectFlags, ProjectFlags, Remote, Stage};
pub use parse::parse_bool;
pub use snapshot::{
    BETA_FEATURES_ENV, FlagSnapshot, FlagSnapshotBuilder, Resolved, Source, current, install,
};
