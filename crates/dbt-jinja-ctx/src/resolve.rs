//! Per-render context structs for the resolve (parse) phase.
//!
//! Unlike [`crate::core::ResolveCore`] which describes the *env globals* set
//! once at parse-env construction, the structs in this module describe the
//! *per-render* context — what gets passed to `render_named_str<S: Serialize>`
//! at every template render call.
//!
//! Two flavours:
//! * [`ResolveBaseCtx`] — the parse-time base, used when rendering sources /
//!   docs / project YAML and as the base for per-model contexts.
//! * [`ResolveModelCtx`] — the per-model overlay layered on top of
//!   `ResolveBaseCtx` when rendering each `.sql` model file. Carries
//!   `this`/`ref`/`source`/`function`/`config`/`model`/etc.
//!
//! Object-typed slots are typed as [`MinijinjaValue`] for now: the underlying
//! `Object` impls (`DocMacro`, `DbtNamespace`, `ResolveRefFunction<T>`,
//! `ParseConfig<T>`, `MacroLookupContext`, …) live in `dbt-jinja-utils`,
//! which `dbt-jinja-ctx` cannot depend on. A later PR moves them into this
//! crate and tightens those slots. Until then, Object dispatch identity is
//! preserved through the [`crate::JinjaObject`] / `Value::from_serialize`
//! smuggle path because the ctx-internal `MinijinjaValue::from_object(...)`
//! wrapping happens at the caller site.

use std::collections::BTreeMap;

use minijinja::Value as MinijinjaValue;
use schemars::JsonSchema;
use serde::Serialize;

use crate::JinjaObject;
use crate::objects::{DbtNamespace, MacroLookupContext, ParseExecute};

/// Per-render parse-base context. Today's `build_resolve_context` populates
/// this 1:1 — same field names, same key constants
/// (`MACRO_DISPATCH_ORDER`, `TARGET_PACKAGE_NAME`).
///
/// Used at every template render that doesn't have a per-model overlay
/// (e.g. project-yml resolution, source docs, selectors) and as the base for
/// per-model contexts.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ResolveBaseCtx {
    /// `{{ doc(package, name) }}` — `DocMacro` Object lookup.
    ///
    /// Typed `MinijinjaValue` while `DocMacro` lives in `dbt-jinja-utils`;
    /// a future PR moves it here and tightens to `JinjaObject<DocMacro>`.
    #[schemars(with = "serde_json::Value")]
    pub doc: MinijinjaValue,

    /// `{{ MACRO_DISPATCH_ORDER }}` — per-package dispatch order map.
    /// Key string matches `minijinja::constants::MACRO_DISPATCH_ORDER`.
    ///
    /// Each value is `MinijinjaValue::from(Vec<String>)` constructed at the
    /// call site, NOT a serde-serialized `Vec<String>`. The dispatch lookup
    /// in `minijinja/src/dispatch_object.rs` does
    /// `order.downcast_object::<Vec<String>>()` to read the search order, so
    /// the underlying `Object` type must be `Vec<String>` specifically.
    /// Going through serde would produce a `MutableVec<Value>` instead and
    /// silently break dispatch.
    #[serde(rename = "MACRO_DISPATCH_ORDER")]
    #[schemars(with = "BTreeMap<String, Vec<String>>")]
    pub macro_dispatch_order: BTreeMap<String, MinijinjaValue>,

    /// `{{ TARGET_PACKAGE_NAME }}` — local project name.
    /// Key string matches `minijinja::constants::TARGET_PACKAGE_NAME`.
    #[serde(rename = "TARGET_PACKAGE_NAME")]
    pub target_package_name: String,

    /// `{{ execute }}` — `false` at parse, flipped to `true` at compile/run
    /// (PR 5+).
    pub execute: bool,

    /// `{{ context }}` — `MacroLookupContext` used by naming macros for
    /// root-project metadata and macro dispatch.
    pub context: JinjaObject<MacroLookupContext>,

    /// `{{ node }}` — `Value::NONE` at base scope; populated per-model.
    #[schemars(with = "serde_json::Value")]
    pub node: MinijinjaValue,

    /// `{{ connection_name }}` — empty string at base scope.
    pub connection_name: String,

    /// `{{ store_result(...) }}` — `ResultStore`-backed closure.
    ///
    /// Present at *base* scope, not just on [`ResolveModelCtx`], because
    /// `generate_{database,schema,alias}_name` macros are invoked with the
    /// bare base ctx (see `dbt_jinja_utils::utils::generate_component_name`).
    /// dbt-core runs those through `generate_generate_name_macro_context`,
    /// which builds a `MacroContext(ProviderContext)` and therefore always
    /// exposes the three result-store functions. Omitting them here made a
    /// customer macro that calls `run_query(...)` inside
    /// `generate_schema_name` blow up on `load_result`, and the caller's
    /// `unwrap_or_else` silently fell back to the profile schema.
    ///
    /// [`crate::CompileBaseCtx`] already carries these at base scope for the
    /// same reason.
    #[schemars(with = "serde_json::Value")]
    pub store_result: MinijinjaValue,

    /// `{{ load_result(...) }}` — `ResultStore`-backed closure.
    /// See [`ResolveBaseCtx::store_result`] for why this is at base scope.
    #[schemars(with = "serde_json::Value")]
    pub load_result: MinijinjaValue,

    /// `{{ store_raw_result(...) }}` — `ResultStore`-backed closure.
    /// See [`ResolveBaseCtx::store_result`] for why this is at base scope.
    #[schemars(with = "serde_json::Value")]
    pub store_raw_result: MinijinjaValue,

    /// `{{ builtins.* }}` — `ref`/`source`/`metric`/`function` stand-ins exposing `.config`, present at base scope for naming macros (dbt-core does the same).
    /// Shadowed by [`ResolveModelCtx::builtins`] on per-model renders.
    #[schemars(with = "serde_json::Value")]
    pub builtins: MinijinjaValue,

    /// `{{ ref.config }}` — same stand-in [`ResolveBaseCtx::builtins`] holds under `ref`; `UNDEFINED` when no runtime config is in scope.
    /// Shadowed by the callable [`ResolveModelCtx::ref_fn`].
    #[serde(rename = "ref")]
    #[schemars(with = "serde_json::Value")]
    pub ref_fn: MinijinjaValue,

    /// `{{ source.config }}` — see [`ResolveBaseCtx::ref_fn`].
    /// Shadowed by the callable [`ResolveModelCtx::source`].
    #[schemars(with = "serde_json::Value")]
    pub source: MinijinjaValue,

    /// `{{ metric.config }}` — see [`ResolveBaseCtx::ref_fn`].
    /// Shadowed by the callable [`ResolveModelCtx::metric`].
    #[schemars(with = "serde_json::Value")]
    pub metric: MinijinjaValue,

    /// `{{ function.config }}` — see [`ResolveBaseCtx::ref_fn`].
    /// Shadowed by the callable [`ResolveModelCtx::function`].
    #[schemars(with = "serde_json::Value")]
    pub function: MinijinjaValue,

    /// Per-package namespace objects. Each entry becomes its own top-level
    /// Jinja global via `#[serde(flatten)]` — e.g. `{ "dbt": <DbtNamespace>,
    /// "snowflake": <DbtNamespace>, … }` flattens into individual `{{ dbt }}`,
    /// `{{ snowflake }}` keys.
    ///
    /// Naming note: this is the *dispatch-side* `DbtNamespace` from
    /// [`crate::objects::lookup`], distinct from
    /// `dbt_parser::dbt_namespace::DbtNamespace` (parse-phase
    /// adapter-call tracking — different concern, different crate).
    #[serde(flatten)]
    #[schemars(with = "BTreeMap<String, serde_json::Value>")]
    pub dbt_namespaces: BTreeMap<String, JinjaObject<DbtNamespace>>,
}

/// Per-model overlay layered onto [`ResolveBaseCtx`] for each model SQL
/// render. Today's `build_resolve_model_context<T>` populates this 1:1 — 17
/// keys total, same field-key strings (including the underscore-decorated
/// `__dbt_target_unique_id__` / `__dbt_current_path__` /
/// `__dbt_current_span__` constants).
///
/// Every `Object`-typed field is typed [`MinijinjaValue`] (wrapped at
/// construction site via `MinijinjaValue::from_object(...)` /
/// `MinijinjaValue::from_function(closure)`) rather than a concrete type
/// going through serde. Two reasons:
///
/// 1. The Object impls (`ResolveRefFunction<T>`, `ParseConfig<T>`,
///    `ResolveSourceFunction<T>`, `ResolveFunctionFunction<T>`,
///    `ResolveMetricFunction<T>`, `ParseExecute`, `ParseMetricReference`,
///    `MacroLookupContext`) live in
///    `dbt-jinja-utils`; this crate can't depend on them. A later PR moves
///    them here and tightens to `JinjaObject<…>`.
/// 2. `builtins` is constructed via
///    `Value::from_object(BTreeMap<String, MinijinjaValue>)` and downstream
///    code downcasts via `.downcast_ref::<BTreeMap<String, MinijinjaValue>>()`.
///    Going through serde would produce a `MutableMap<Value, Value>` instead
///    and silently break the downcast — same trap PR 3 hit with
///    `MACRO_DISPATCH_ORDER`'s `Vec<String>` values.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ResolveModelCtx {
    /// `{{ this }}` — `ResolveThisFunction<T>` Object delegating to a
    /// constructed `RelationObject`.
    #[schemars(with = "serde_json::Value")]
    pub this: MinijinjaValue,

    /// `{{ ref(...) }}` — `ResolveRefFunction<T>` Object that records each
    /// resolved ref into the shared `sql_resources` vec.
    #[serde(rename = "ref")]
    #[schemars(with = "serde_json::Value")]
    pub ref_fn: MinijinjaValue,

    /// `{{ source(...) }}` — `ResolveSourceFunction<T>` Object.
    #[schemars(with = "serde_json::Value")]
    pub source: MinijinjaValue,

    /// `{{ function(...) }}` — `ResolveFunctionFunction<T>` Object.
    #[schemars(with = "serde_json::Value")]
    pub function: MinijinjaValue,

    /// `{{ metric(...) }}` — `ResolveMetricFunction<T>` Object that records
    /// `SqlResource::Metric`.
    #[schemars(with = "serde_json::Value")]
    pub metric: MinijinjaValue,

    /// `{{ config(...) }}` — `ParseConfig<T>` Object.
    #[schemars(with = "serde_json::Value")]
    pub config: MinijinjaValue,

    /// `{{ model.* }}` — mutable dict-compatible Object backing the
    /// parse-time stub model dict (built via `convert_yml_to_value_map` plus
    /// a serialized merged config). Downstream adapter code reads via
    /// `state.lookup("model").get_attr(...)`.
    #[schemars(with = "serde_json::Value")]
    pub model: MinijinjaValue,

    /// `{{ builtins.* }}` — `BTreeMap<String, MinijinjaValue>` Object holding
    /// `ref` / `source` / `function` / `metric` / `config`. Compile/run-phase code
    /// downcasts this to `BTreeMap<String, MinijinjaValue>` to overlay
    /// per-node validations.
    #[schemars(with = "serde_json::Value")]
    pub builtins: MinijinjaValue,

    /// `{{ graph }}` — the invocation-wide mutable mapping, empty at
    /// parse-model scope; the flat graph is merged into that same mapping at
    /// compile time. Mirrors dbt-core, where `graph` is `manifest.flat_graph`:
    /// one dict for the whole invocation, `{}` until `build_flat_graph()` runs,
    /// so macros can use it as scratch state throughout. dbt-labs/fs#13454.
    #[schemars(with = "serde_json::Value")]
    pub graph: MinijinjaValue,

    /// `{{ store_result(...) }}` — `ResultStore`-backed closure.
    #[schemars(with = "serde_json::Value")]
    pub store_result: MinijinjaValue,

    /// `{{ load_result(...) }}` — `ResultStore`-backed closure.
    #[schemars(with = "serde_json::Value")]
    pub load_result: MinijinjaValue,

    /// `{{ store_raw_result(...) }}` — `ResultStore`-backed closure.
    #[schemars(with = "serde_json::Value")]
    pub store_raw_result: MinijinjaValue,

    /// `{{ execute }}` — `ParseExecute(AtomicBool)` Object that records
    /// when the model truthy-tests it during parse; used to gate static
    /// analysis.
    pub execute: JinjaObject<ParseExecute>,

    /// `{{ context }}` — `MacroLookupContext` Object for `context['name']`
    /// macro dispatch.
    pub context: JinjaObject<MacroLookupContext>,

    /// `{{ TARGET_UNIQUE_ID }}` — `<package>.<model_name>` identifier.
    /// Key string matches `minijinja::constants::TARGET_UNIQUE_ID`.
    #[serde(rename = "TARGET_UNIQUE_ID")]
    pub target_unique_id: String,

    /// `{{ __minijinja_current_path }}` — display path of the rendering
    /// `.sql` file. Key string matches
    /// `minijinja::constants::CURRENT_PATH`.
    #[serde(rename = "__minijinja_current_path")]
    pub current_path: String,

    /// `{{ __minijinja_current_span }}` — span at the start of rendering
    /// (typically `minijinja::machinery::Span::default()`). Key string
    /// matches `minijinja::constants::CURRENT_SPAN`.
    ///
    /// Wrapped as `MinijinjaValue::from_serialize(Span::default())` at the
    /// call site rather than typed as the concrete `Span` here, because the
    /// `Span` type lives in `minijinja::machinery` (private to the runtime)
    /// and we want to keep this crate's view of it opaque.
    #[serde(rename = "__minijinja_current_span")]
    #[schemars(with = "serde_json::Value")]
    pub current_span: MinijinjaValue,
}
