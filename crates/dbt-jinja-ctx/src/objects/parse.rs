//! Parse-phase `Object` impls — std-only types moved here from
//! `dbt-jinja-utils` so the typed parse-phase ctx structs can hold them via
//! `JinjaObject<T>` rather than opaque `MinijinjaValue`.
//!
//! `ParseConfig<T>`, `ResolveRefFunction<T>` and friends are NOT here —
//! they're generic over the config-model type and need
//! `ConfigModelHandle` (dyn-erasure) before they can move. PR 6.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use minijinja::value::Object;

/// `{{ execute }}` value used during the parse phase.
///
/// `is_true()` flips the wrapped `AtomicBool` to `true` when truthy-tested
/// (e.g. by an `{% if execute %}` block in a model SQL file). The static
/// analyzer reads the bool after rendering to decide whether to emit
/// introspection warnings. `is_true()` itself returns `false` so the
/// `{% if execute %}` branch is never taken at parse — the side effect
/// (recording that the model *would* have entered an execute branch) is
/// the entire point of the type.
#[derive(Debug, Clone)]
pub struct ParseExecute(Arc<AtomicBool>);

impl ParseExecute {
    /// Construct a `ParseExecute` backed by `flag`. The flag is shared with
    /// the parse-phase orchestrator so it can read whether the model
    /// truthy-tested `execute` after the render completes.
    pub fn new(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }
}

impl Object for ParseExecute {
    fn is_true(self: &Arc<Self>) -> bool {
        self.0.store(true, Ordering::Relaxed);
        false
    }
}
