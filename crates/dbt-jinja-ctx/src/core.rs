//! Phase-core ctx structs.
//!
//! [`GlobalCore`] is the typed shape of the three Jinja globals available on
//! every dbt jinja env from the load phase onwards: `run_started_at`,
//! `target`, `flags`. Every later phase's outer ctx flattens this core via
//! `#[serde(flatten)]` and adds phase-specific fields.
//!
//! `flags` is intentionally typed `MinijinjaValue` (not a concrete map or
//! Object) so the same field can carry the load-phase shape (a plain
//! `BTreeMap<String, Value>`, wrapped via `Value::from_serialize`) AND the
//! parse-phase shape (the richer `Flags` Object, wrapped via
//! `Value::from_object`). Both wrappings round-trip through serde with
//! Object identity intact via the [`crate::JinjaObject`] smuggle path.
//!
//! [`ResolveCore`] is the typed shape of the parse-phase env globals. It
//! flattens [`GlobalCore`] (so `run_started_at`/`target`/`flags` are shared,
//! not duplicated) and adds the parse-only fields (`project_name`, `env`,
//! `var`, `invocation_args_dict`, `invocation_id`, `database`, `schema`,
//! `write`). Compile and run reuse the parse env without re-registering
//! globals, so `ResolveCore` is the canonical globals shape for phases ≥
//! parse.

use std::collections::BTreeMap;

use dbt_jinja_vars::ConfiguredVar;
use minijinja::Value as MinijinjaValue;
use minijinja_contrib::modules::py_datetime::datetime::PyDateTime;
use schemars::JsonSchema;
use serde::Serialize;

use crate::TargetContextMap;
use crate::jinja_object::JinjaObject;

/// Globals available on every dbt jinja env from the load phase onwards.
/// Flattened into [`crate::LoadCtx`] and into every later phase's core.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GlobalCore {
    /// `{{ run_started_at }}` — UTC datetime of the dbt invocation.
    pub run_started_at: JinjaObject<PyDateTime>,

    /// `{{ target }}` — connection metadata (profile name, database, schema,
    /// adapter type, …).
    #[schemars(with = "serde_json::Value")]
    pub target: TargetContextMap,

    /// `{{ flags }}` — CLI + project flags. At load this wraps a plain
    /// `BTreeMap<String, Value>` (`Value::from_serialize`); at parse it
    /// wraps the `Flags` Object (`Value::from_object`). Either wrapping
    /// round-trips through serde with Object identity preserved via the
    /// minijinja `VALUE_HANDLES` smuggle path.
    #[schemars(with = "serde_json::Value")]
    pub flags: MinijinjaValue,
}

/// Globals registered on the **parse** `JinjaEnv`. Flattens [`GlobalCore`]
/// (so `run_started_at`/`target`/`flags` are shared with load) and adds
/// 8 parse-only fields. Compile and run reuse the parse env without
/// re-registering globals.
///
/// 11 keys total in the registered globals map, 1:1 with the `BTreeMap`
/// that `initialize_parse_jinja_environment` historically built. Field
/// order on serialization: `GlobalCore` keys first (run_started_at,
/// target, flags), then parse-specific fields in the order below.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ResolveCore {
    #[serde(flatten)]
    pub global_core: GlobalCore,

    /// `{{ project_name }}` — root project name.
    pub project_name: String,

    /// `{{ env }}` — alias of `target`. Both keys point to the same
    /// `Arc<BTreeMap<…>>` payload.
    #[schemars(with = "serde_json::Value")]
    pub env: TargetContextMap,

    /// `{{ invocation_args_dict }}` — flattened CLI / project-flag dict
    /// computed by `invocation_args_to_dict` in the parse init.
    #[schemars(with = "BTreeMap<String, serde_json::Value>")]
    pub invocation_args_dict: BTreeMap<String, MinijinjaValue>,

    /// `{{ invocation_id }}` — UUID identifying this invocation.
    pub invocation_id: String,

    /// `{{ var }}` — Jinja-side `var(…)` callable.
    pub var: JinjaObject<ConfiguredVar>,

    /// `{{ database }}` — default database from the resolved target.
    pub database: Option<String>,

    /// `{{ schema }}` — default schema from the resolved target.
    pub schema: Option<String>,

    /// `{{ write }}` — always `MinijinjaValue::NONE` at parse; populated at
    /// run by the run-phase ctx.
    #[schemars(with = "serde_json::Value")]
    pub write: MinijinjaValue,
}
