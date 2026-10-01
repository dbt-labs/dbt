//! `Object` impls used as values in typed-ctx structs.
//!
//! These move here progressively from `dbt-jinja-utils` so the typed-ctx
//! structs can hold concrete `JinjaObject<T>` slots instead of opaque
//! `MinijinjaValue`. Each move is gated on the type being expressible
//! without depending on `dbt-common` / `dbt-schemas` / `dbt-adapter`
//! (`dbt-jinja-ctx` deliberately depends on none of those).
//!
//! Currently moved:
//! * Parse phase (PR 5): `ParseExecute`, `MacroLookupContext`,
//!   `DbtNamespace` (dispatch-side).
//! * Compile phase: `DummyConfig`.
//! * Run phase: `HookConfig`, `LazyModelWrapper`.
//!
//! Pending later PRs:
//! * `ParseMetricReference`, `ParseConfigValue` — bundle with `ParseConfig`
//!   when `ConfigModelHandle` lands and dyn-erases `<T>`.
//! * `DocMacro` — defer until its `dbt-common` dependency chain
//!   (`CodeLocationWithFile`, `StatusReporter`,
//!   `emit_warn_log_from_fs_error`) gets sorted.
//! * `ResolveRefFunction<T>`, `ResolveSourceFunction<T>`,
//!   `ResolveFunctionFunction<T>`, `ParseConfig<T>` — `ConfigModelHandle`
//!   dyn-erasure.
//! * `RefFunction`, `SourceFunction`, `FunctionFunction`,
//!   `MicrobatchRefContext`, `LazyFlatGraph` — need
//!   `NodeResolverHandle` / `RuntimeConfigHandle` / `FlatGraphSource`.
//! * `CompileConfig`, `RunConfig` — depend on `dbt_common::DashMap` /
//!   `dbt_schemas::project::ConfigKeys`.
//! * `WriteConfig` — uses `dbt_common::path::get_target_write_path`.

pub mod compile;
pub mod lookup;
pub mod parse;
pub mod run;

pub use compile::DummyConfig;
pub use lookup::{DbtNamespace, MacroLookupContext};
pub use parse::ParseExecute;
pub use run::{HookConfig, LazyModelWrapper};
