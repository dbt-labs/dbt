//! File-based, inter-process [lease] concurrency primitive.
//!
//! [lease]: https://en.wikipedia.org/wiki/Lease_(computer_science)

mod lease;
pub use lease::*;
