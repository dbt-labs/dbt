//! The dbt Adapter Engine.

/// Cow module for managing copy-on-write data structures (mostly strings).
pub mod cow;

pub mod map_reduce;
pub use map_reduce::ConnectionFactory;
pub use map_reduce::MapReduce;
