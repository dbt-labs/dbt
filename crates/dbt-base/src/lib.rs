//! Utility types and functions used across the fusion workspace.
//!
//! **NOTE**: The purpose of this crate is to sit at the very root of the fusion
//! dependency graph, so it can be pulled in by any other fusion crate. To this
//! end, it
//!  - MUST NOT depend on any other fusion crates!
//!  - MUST NOT have any external dependencies on third-party crates, as any
//!    dependencies here would bloat up everything.
//!
//! As such, type aliases for the [MaybeStableHasher] variants of `DashMap` and
//! `scc::HashMap` are in the `dbt-common` crate instead.

pub mod cancel;

pub mod hasher;

pub mod hashmap {
    /// A HashMap variant that uses a stable hasher in debug builds and a
    /// Dos-resistant hasher in release builds.
    pub type HashMap<K, V> =
        std::collections::HashMap<K, V, crate::hasher::MaybeStableHasherBuilder>;

    /// Creates a new HashMap with the stable/Dos-resistant hasher. Saves some
    /// typing compared with `HashMap::with_hasher(...)`.
    #[inline]
    pub fn new<K, V>() -> HashMap<K, V> {
        HashMap::with_hasher(crate::hasher::MaybeStableHasherBuilder::default())
    }
}

pub mod hashset {
    /// A HashSet variant that uses a stable hasher in debug builds and a
    /// Dos-resistant hasher in release builds.
    pub type HashSet<T> = std::collections::HashSet<T, crate::hasher::MaybeStableHasherBuilder>;

    /// Creates a new HashSet with the stable/Dos-resistant hasher. Saves some
    /// typing compared with `HashSet::with_hasher(...)`.
    #[inline]
    pub fn new<T>() -> HashSet<T> {
        HashSet::with_hasher(crate::hasher::MaybeStableHasherBuilder::default())
    }
}

#[doc(inline)]
pub use cancel::Cancellable;
#[doc(inline)]
pub use cancel::{CancellationToken, CancellationTokenSource, CancelledError};

#[doc(inline)]
pub use hasher::{MaybeStableHasher, MaybeStableHasherBuilder};

#[doc(inline)]
pub use crate::hashmap::HashMap;
#[doc(inline)]
pub use crate::hashset::HashSet;
