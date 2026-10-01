//! Lightweight `std`-only home for dyn-trait handles shared between
//! `dbt-jinja-ctx` and the crates that implement them (`dbt-adapter`,
//! `dbt-schemas`, …).
//!
//! The crate exists so the typed Jinja context layer doesn't have to drag
//! `dbt-common` (with its `arrow` / `parquet` / `dashmap` / `schemars` /
//! full `minijinja` dep tree) through every consumer just to share a 5-line
//! interface declaration. Each handle here is a tight `Send + Sync + Debug`
//! trait with the minimum method surface its consumers actually invoke
//! through the trait object.
//!
//! This crate may also serve as a seed for the broader effort to extract
//! lightweight pieces out of `dbt-common`.

use std::any::Any;
use std::fmt::Debug;

/// Handle to the adapter object carried alongside Jinja phase contexts.
///
/// The trait body is intentionally minimal: ctx structs only need to *carry*
/// an adapter reference forward. Methods (e.g. `adapter_type`, `dispatch`)
/// will be added as later phase migrations move ctx-builder code that
/// dispatches through the adapter onto this trait.
pub trait AdapterHandle: Debug + Send + Sync + Any {
    /// Upcast to `&dyn Any` so consumers can `downcast_ref` to the concrete
    /// adapter type when needed (transitional escape hatch — remove once all
    /// phase code talks through this trait).
    fn as_any(&self) -> &dyn Any;
}
