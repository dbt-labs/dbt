pub mod attributes;
pub mod impls;
pub mod macros;
#[path = "gen/mod.rs"]
pub mod proto;
pub mod schemas;
pub mod serialize;

pub use attributes::*;
pub use schemas::*;

// Test-only utilities for enumerating proto message types.
// Available in this crate's tests or when dependents opt-in via feature.
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
