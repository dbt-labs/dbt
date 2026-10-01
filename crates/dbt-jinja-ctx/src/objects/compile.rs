//! Compile-phase `Object` impls — std-only types moved here from
//! `dbt-jinja-utils` so the typed compile-phase ctx structs can hold them
//! via `JinjaObject<T>` rather than opaque `MinijinjaValue`.
//!
//! Currently moved: `DummyConfig`.
//!
//! Pending later PRs (need handle traits to move):
//! * `RefFunction`, `SourceFunction`, `FunctionFunction` → need
//!   `NodeResolverHandle` + `RuntimeConfigHandle`.
//! * `LazyFlatGraph` → needs `FlatGraphSource` (`Nodes` lives in
//!   `dbt-schemas`).
//! * `CompileConfig` → uses `dbt_common::DashMap`.
//! * `MicrobatchRefContext` → carried by `RefFunction`, moves with it.

use std::rc::Rc;
use std::sync::Arc;

use minijinja::listener::RenderingEventListener;
use minijinja::value::Object;
use minijinja::{Error as MinijinjaError, ErrorKind as MinijinjaErrorKind, State, Value};

/// Base-scope `{{ config(...) }}` placeholder used at compile/run-base scope.
/// Returns the empty string when called and `None` for any `.get(...)` call.
/// The real per-node `CompileConfig` / `RunConfig` overlays this when the
/// compile-node / run-node ctx is constructed.
#[derive(Debug)]
pub struct DummyConfig;

impl Object for DummyConfig {
    fn call(
        self: &Arc<Self>,
        _: &State,
        _: &[Value],
        _: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, MinijinjaError> {
        Ok(Value::from(""))
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        name: &str,
        _args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, MinijinjaError> {
        match name {
            "get" => Ok(Value::from(None::<Option<String>>)),
            _ => Err(MinijinjaError::new(
                MinijinjaErrorKind::UnknownMethod,
                format!("Unknown method on config: {name}"),
            )),
        }
    }
}
