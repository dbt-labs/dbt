//! Invocation-scoped storage for the Jinja `graph` mapping.
//!
//! In dbt-core, `graph` is `manifest.flat_graph` — a single plain `dict` that
//! hangs off the one `Manifest` instance and lives for the whole invocation.
//! It is empty during parsing and filled in with the flat graph afterwards,
//! but it is the *same* object throughout, so a macro can stash scratch state
//! on it in one render and read it back in a later one. Packages depend on
//! that: Elementary's `set_cache`/`get_cache` are built on
//! `graph.setdefault("elementary", {})`.
//!
//! Fusion has no single long-lived manifest object to hang this off, and
//! `build_resolve_model_context` (the parse-phase context builder) is reached
//! through several layers of resolver functions with no per-invocation owner
//! in scope — `DbtRuntimeConfig` is per *package*, not per invocation. So the
//! mapping lives here instead, behind an explicit
//! [`reset_invocation_graph`] that a long-lived process (LSP, service) calls
//! once per invocation so scratch state cannot leak from one project into the
//! next.
//!
//! See dbt-labs/fs#13454.

use std::sync::{Arc, OnceLock, RwLock};

use minijinja::value::mutable_map::MutableMap;

static INVOCATION_GRAPH: OnceLock<RwLock<Arc<MutableMap>>> = OnceLock::new();

fn slot() -> &'static RwLock<Arc<MutableMap>> {
    INVOCATION_GRAPH.get_or_init(|| RwLock::new(Arc::new(MutableMap::new())))
}

/// The `graph` mapping for the current invocation.
///
/// Every Jinja context that exposes `graph` — parse, compile and run alike —
/// hands out this same object, so mutations made by one macro or hook are
/// visible to every later one.
pub fn invocation_graph() -> Arc<MutableMap> {
    slot().read().expect("poisoned lock").clone()
}

/// Install a fresh `graph` mapping, dropping the previous invocation's.
///
/// Called once per invocation before any Jinja rendering.
pub fn reset_invocation_graph() {
    *slot().write().expect("poisoned lock") = Arc::new(MutableMap::new());
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::Value;

    /// Both properties the holder exists for, asserted in one test body on
    /// purpose. They were two tests, which raced: `cargo test` runs a crate's
    /// tests in one process on a thread pool, so the second test's
    /// `reset_invocation_graph()` could land between the first's write and its
    /// read and swap the map out from under it. (`cargo nextest`, which
    /// `cargo xtask test` uses, is process-per-test and hid the race.) Any
    /// future test that resets the graph must live here too, in sequence.
    #[test]
    fn invocation_graph_is_shared_and_reset_between_invocations() {
        let key = Value::from("scratch_key");

        // A write through one handle is visible through the next.
        reset_invocation_graph();
        invocation_graph().insert(key.clone(), Value::from("v"));
        assert_eq!(
            invocation_graph().get(&key),
            Some(Value::from("v")),
            "a write through one handle must be visible through the next"
        );

        // A reset drops the previous invocation's scratch state.
        reset_invocation_graph();
        assert_eq!(
            invocation_graph().get(&key),
            None,
            "scratch state must not leak across invocations"
        );
    }
}
