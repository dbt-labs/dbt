//! Feature flags for dbt Fusion.
//!
//! Every flag is declared once as a [`FlagDef`] and resolved through a single
//! precedence chain into a [`Resolved`] value that records where it came from.
//! Behavior change flags are flags that have graduated to [`Stage::Behavior`];
//! they live in the same registry as internal rollouts and preview features.
//!
//! Precedence, highest first:
//!
//! 1. Remote value for a flag declared [`Remote::Force`] (kill switches)
//! 2. CLI: `--feature NAME[=VALUE]`
//! 3. Env: `DBT_ENGINE_FEATURES=name,other=false`, then the flag's `legacy_env`
//! 4. The root project's `dbt_project.yml` `flags:` (preview and behavior flags only;
//!    a package's `flags:` never applies)
//! 5. `user_settings.yml` `flags:`
//! 6. Remote value for a flag declared [`Remote::Default`] or [`Remote::Force`]
//! 7. The flag's code default
//!
//! A [`Stage::Removed`] flag always resolves to its default.
//!
//! The binary resolves the invocation-scoped layers (1–3, 5, 6) once at startup
//! and [`install`]s them; call sites then read [`FlagDef::enabled`] or, for flags
//! users can set in `dbt_project.yml`, [`FlagDef::resolve_with_project`].

mod catalog;
mod def;
mod parse;
mod snapshot;

pub use catalog::{
    ALL, GIT_DEPS_FAST_PATH, LATEST_VERSION_POINTER_ENABLED_BY_DEFAULT,
    REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT,
};
pub use def::{FlagDef, Remote, Stage};
pub use parse::parse_bool;
pub use snapshot::{
    FEATURES_ENV, FlagSnapshot, FlagSnapshotBuilder, Resolved, Source, current, install,
};
