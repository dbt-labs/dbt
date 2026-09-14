#[macro_use]
pub mod macros;

pub mod adapter;
pub mod artifact_io;
pub mod atomic;
pub mod cancellation;
pub mod constants;
pub mod hashing;
pub mod io_utils;
pub mod node_selector;
pub mod pretty_string;
pub mod static_analysis;
pub mod stats;
pub mod stdfs;
pub mod string_utils;
pub mod tokiofs;
#[macro_use]
pub extern crate dbt_error as error;
pub use dbt_error::{
    AdapterError, AdapterErrorKind, AdapterResult, AsyncAdapterResult, Cancellable,
    CodeLocationWithFile, CompiledSpans, ErrContext, ErrorCode, FsError, FsResult, LiftableResult,
    MacroSpan, MacroSpansOnly, Span, ectx, err, fs_err, into_fs_error, not_implemented_err,
    unexpected_err, unexpected_fs_err,
};
pub mod behavior_flags;
pub mod embedded_install_scripts;
pub mod fail_fast;
pub mod infer_schema_registry;
pub mod io_args;
pub mod lease;
pub mod once_cell_vars;
pub mod path;
pub mod row_limit;
pub mod serde_utils;
pub mod status_reporter;
pub mod time;
pub mod tracing;
pub mod url;
pub mod user_defined_schema_registry;
pub mod warn_error_options;

// Re-export span creation functions that were previously exported as macros
pub use tracing::{
    create_debug_span, create_debug_span_with_parent, create_info_span,
    create_info_span_with_parent, create_root_info_span,
};

mod discrete_event_emitter;
pub use discrete_event_emitter::DiscreteEventEmitter;

pub mod dashmap {
    /// A DashMap variant that uses a stable hasher in debug builds and a
    /// DoS-resistant hasher in release builds.
    #[allow(clippy::disallowed_types)]
    pub type DashMap<K, V> = dashmap::DashMap<K, V, dbt_base::MaybeStableHasherBuilder>;

    /// Creates a new DashMap with the stable/DoS-resistant hasher. Saves some
    /// typing compared with `DashMap::with_hasher(...)`.
    #[inline]
    pub fn new<K, V>() -> DashMap<K, V>
    where
        K: std::hash::Hash + Eq,
    {
        DashMap::with_hasher(dbt_base::MaybeStableHasherBuilder::default())
    }
}

pub mod sccmap {
    /// A scc::HashMap variant that uses a stable hasher in debug builds and a
    /// DoS-resistant hasher in release builds.
    #[allow(clippy::disallowed_types)]
    pub type HashMap<K, V> = scc::HashMap<K, V, dbt_base::MaybeStableHasherBuilder>;

    /// Creates a new scc::HashMap with the stable/DoS-resistant hasher. Saves some
    /// typing compared with `scc::HashMap::with_hasher(...)`.
    #[inline]
    pub fn new<K, V>() -> HashMap<K, V>
    where
        K: std::hash::Hash + Eq,
    {
        HashMap::with_hasher(dbt_base::MaybeStableHasherBuilder::default())
    }
}

/// A module for re-exporting commonly used collection types in a centralized
/// place
pub mod collections {

    #[doc(inline)]
    pub use dbt_base::HashMap;
    #[doc(inline)]
    pub use dbt_base::HashSet;

    #[doc(inline)]
    pub use crate::dashmap::DashMap;
    #[doc(inline)]
    pub use crate::sccmap::HashMap as SccHashMap;
}
