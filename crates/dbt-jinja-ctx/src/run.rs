//! Per-render context structs for the run (materialize) phase.
//!
//! Run shares the parse `JinjaEnv` (mutated in place by
//! `configure_compile_and_run_jinja_environment`) so the structs in this
//! module describe *per-render* contexts only.
//!
//! The single struct here, [`RunNodeCtx`], is the per-node overlay layered
//! on top of [`crate::CompileBaseCtx`] when materializing each node. It
//! shadows several base entries (`config`, `model`, `node`, `builtins`,
//! `context`, `connection_name`, `TARGET_PACKAGE_NAME`,
//! `store_result` / `load_result` / `store_raw_result`) and adds run-only
//! keys (`pre_hooks` / `post_hooks` conditional, `write`,
//! `load_agate_table` optional, `submit_python_job`,
//! `__minijinja_current_path`, `__minijinja_current_span`).
//!
//! Object-typed slots stay `MinijinjaValue` for the same reasons as
//! [`crate::CompileNodeCtx`]: their concrete impls (`RunConfig`,
//! `LazyModelWrapper`, `WriteConfig`, `HookConfig`, `RelationObject`) live
//! in `dbt-jinja-utils` / `dbt-adapter` and need a follow-up move to tighten
//! to `JinjaObject<T>`.

use minijinja::Value as MinijinjaValue;
use schemars::JsonSchema;
use serde::Serialize;

use crate::JinjaObject;
use crate::compile::CompileBaseCtx;
use crate::objects::{LazyModelWrapper, MacroLookupContext};

/// Per-node run-time overlay layered onto [`crate::CompileBaseCtx`] for each
/// node materialization. Today's `build_run_node_context<S>` populates this
/// 1:1 — same field-key strings (including the underscore-decorated
/// `__minijinja_current_path` / `__minijinja_current_span` constants).
///
/// At construction, the calling materializer first builds `CompileBaseCtx`,
/// then overlays a `RunNodeCtx` on top. Several keys are intentionally
/// re-emitted on the overlay — they shadow the base entries with per-node
/// variants. The `pre_hooks` / `post_hooks` / `load_agate_table` fields are
/// `Option<MinijinjaValue>` and `#[serde(skip_serializing_if = "Option::is_none")]`
/// so they only land in the overlay map when actually present (today's
/// `BTreeMap::insert` only fires when the corresponding YAML key is set or
/// the agate table is supplied).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RunNodeCtx {
    /// Typed base context. `None` on today's BTreeMap-overlay path; `Some`
    /// on the future typed-throughout path where this struct is passed
    /// directly to `render_named_str`. Flattened so base keys appear at the
    /// top level; per-node fields declared below shadow any shared keys.
    #[serde(flatten)]
    pub base: Option<CompileBaseCtx>,

    /// `{{ this }}` — `RelationObject` for the node's adapter relation.
    #[schemars(with = "serde_json::Value")]
    pub this: MinijinjaValue,

    /// `{{ database }}` — node's adapter database.
    pub database: String,

    /// `{{ schema }}` — node's adapter schema.
    pub schema: String,

    /// `{{ identifier }}` — node's adapter alias (note: run uses `name`,
    /// not `alias`; alias is the truncated form for path-length safety on
    /// Linux. See the comment in `build_run_node_context`.)
    pub identifier: String,

    /// `{{ pre_hooks }}` — sequence of `HookConfig` Objects, only emitted
    /// when the node's config contains a `pre_hook` entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Value>")]
    pub pre_hooks: Option<MinijinjaValue>,

    /// `{{ post_hooks }}` — sequence of `HookConfig` Objects, only emitted
    /// when the node's config contains a `post_hook` entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Value>")]
    pub post_hooks: Option<MinijinjaValue>,

    /// `{{ config(...) }}` — `RunConfig` Object backed by the node's
    /// merged config map. Shadows [`crate::CompileBaseCtx::config`].
    #[schemars(with = "serde_json::Value")]
    pub config: MinijinjaValue,

    /// `{{ model.* }}` — `LazyModelWrapper` Object that lazy-loads
    /// `compiled_code` from the on-disk compiled SQL on first attribute
    /// access. Shadows the base.
    pub model: JinjaObject<LazyModelWrapper>,

    /// `{{ node.* }}` — same `LazyModelWrapper` content as `model` (run
    /// phase aliases them; compile-node does not). Shadows
    /// [`crate::CompileBaseCtx::node`] (`Value::NONE`).
    pub node: JinjaObject<LazyModelWrapper>,

    /// `{{ connection_name }}` — empty string at run scope (overlays).
    pub connection_name: String,

    /// `{{ store_result(...) }}` — `ResultStore`-backed closure.
    #[schemars(with = "serde_json::Value")]
    pub store_result: MinijinjaValue,

    /// `{{ load_result(...) }}` — `ResultStore`-backed closure.
    #[schemars(with = "serde_json::Value")]
    pub load_result: MinijinjaValue,

    /// `{{ store_raw_result(...) }}` — `ResultStore`-backed closure.
    #[schemars(with = "serde_json::Value")]
    pub store_raw_result: MinijinjaValue,

    /// `{{ submit_python_job(...) }}` — closure invoked for Python models;
    /// always present on the run ctx (no-op for non-Python nodes).
    #[schemars(with = "serde_json::Value")]
    pub submit_python_job: MinijinjaValue,

    /// `{{ context }}` — `MacroLookupContext` keyed by the node's
    /// package_name (NOT root project's; this differs from
    /// [`crate::CompileBaseCtx::context`]).
    pub context: JinjaObject<MacroLookupContext>,

    /// `{{ write(...) }}` — `WriteConfig` Object closure that writes a
    /// payload to the target run directory.
    #[schemars(with = "serde_json::Value")]
    pub write: MinijinjaValue,

    /// `{{ load_agate_table(...) }}` — closure that returns the seed's
    /// loaded agate table; only emitted when an `agate_table` was supplied
    /// to `build_run_node_context`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Value>")]
    pub load_agate_table: Option<MinijinjaValue>,

    /// `{{ builtins.* }}` — overlay map carrying the per-node `RunConfig`
    /// alongside the base's ref/source/function. Same downcast contract as
    /// [`crate::CompileNodeCtx::builtins`].
    #[schemars(with = "serde_json::Value")]
    pub builtins: MinijinjaValue,

    /// `{{ TARGET_PACKAGE_NAME }}` — node's package_name (overlays the
    /// base, which is the local project's name at base-build time).
    #[serde(rename = "TARGET_PACKAGE_NAME")]
    pub target_package_name: String,

    /// `{{ __minijinja_current_path }}` — node path selected from the
    /// execution phase and expressed relative to the out_dir when applicable.
    /// Key string matches `minijinja::constants::CURRENT_PATH`.
    #[serde(rename = "__minijinja_current_path")]
    pub current_path: String,

    /// `{{ __minijinja_current_span }}` — span at the start of rendering.
    /// Key string matches `minijinja::constants::CURRENT_SPAN`.
    #[serde(rename = "__minijinja_current_span")]
    #[schemars(with = "serde_json::Value")]
    pub current_span: MinijinjaValue,
}
