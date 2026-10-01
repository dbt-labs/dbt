//! Typed Jinja contexts for dbt-fusion phase environments.
//!
//! This crate defines the typed shape of every value that becomes available to
//! Jinja templates across the load, resolve (parse), render (compile), and run
//! phases. It replaces the historical pattern of constructing per-phase
//! `BTreeMap<String, MinijinjaValue>` maps with composable typed structs whose
//! field names — driven by plain `serde::Serialize` — are the Jinja keys.
//!
//! Two flavours of context exist:
//!
//! * **Env globals** — set once at `JinjaEnv` construction (load and parse only).
//!   These structs go through [`register::to_jinja_btreemap`] before
//!   `JinjaEnvBuilder.with_globals(...)` so the registered map is visible to
//!   downstream builder hooks (notably `try_with_macros`'s replay-mode
//!   detection). [`register::register_globals_from_serialize`] is the
//!   already-built-env equivalent for tests and callers that don't go through
//!   the builder.
//! * **Per-render contexts** — conceptually passed at every
//!   `render_named_str<S: Serialize>` call. The end-state is to flow them
//!   directly into that API; today's `dbt-parser` callers still consume the
//!   `BTreeMap<String, Value>` shape (they `.extend(...)` it onto a
//!   per-model overlay before rendering), so per-render ctx builders return
//!   that shape via [`register::to_jinja_btreemap`] until the caller-site
//!   migration lands.
//!
//! Object identity for callable Jinja values (`ref`, `source`, `config`, `var`,
//! `this`, …) is preserved through serde via the [`JinjaObject`] wrapper, which
//! relies on minijinja's built-in `VALUE_HANDLES` round-trip.
//!
//! Dyn-trait handles (e.g. [`AdapterHandle`]) live in the lightweight
//! `dbt-handles` crate so that downstream crates like `dbt-adapter` can
//! implement them without depending on this crate (or on the heavier
//! `dbt-common`). They are re-exported here for ergonomics.

pub mod compile;
pub mod core;
pub mod docs;
pub mod jinja_object;
pub mod load;
pub mod objects;
pub mod register;
pub mod resolve;
pub mod run;

pub use compile::{CompileBaseCtx, CompileNodeCtx, CustomSqlRenderCtx, OperationCtx};
pub use core::{GlobalCore, ResolveCore};
pub use dbt_handles::AdapterHandle;
pub use docs::DocsContext;
pub use jinja_object::JinjaObject;
pub use load::LoadCtx;
pub use objects::{
    DbtNamespace, DummyConfig, HookConfig, LazyModelWrapper, MacroLookupContext, ParseExecute,
};
pub use register::{register_globals_from_serialize, to_jinja_btreemap};
pub use resolve::{ResolveBaseCtx, ResolveModelCtx};
pub use run::RunNodeCtx;

/// Shape used by the `target` Jinja global. Cheap-cloned (`Arc`) across the
/// resolve / model loop.
pub type TargetContextMap = std::sync::Arc<std::collections::BTreeMap<String, minijinja::Value>>;

/// Shape backing the `{{ model }}` / `{{ node }}` Jinja slots. dbt Core hands
/// materialization macros a plain Python dict they are allowed to mutate
/// (`{% do model.update({...}) %}`), so the map is mutable and shared (`Arc`):
/// the `model` and `node` slots of one render observe each other's writes.
pub type ModelContextMap = std::sync::Arc<minijinja::value::mutable_map::MutableMap>;

/// Build a [`ModelContextMap`] from a serialized node map.
pub fn to_model_context_map(
    model_map: indexmap::IndexMap<String, minijinja::Value>,
) -> ModelContextMap {
    std::sync::Arc::new(minijinja::value::mutable_map::MutableMap::from(
        model_map
            .into_iter()
            .map(|(key, value)| (minijinja::Value::from(key), value))
            .collect::<minijinja::value::ValueMap>(),
    ))
}
