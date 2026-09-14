//! A pool of threads for blocking runtime work.
//!
//! The module layout mirrors tokio's, so that files can be compared
//! against upstream.
//!
//! | this crate | tokio                                       |
//! | ---------- | --------------------------------------------|
//! | crate root | `runtime/blocking/` (+ parts of `runtime/`) |
//! | `task/`    | `runtime/task/`                             |
//! | `context/` | `runtime/context/`                          |
//! | `util/`    | `util/`                                     |

#![allow(unused_qualifications)]

mod pool;
pub use pool::{spawn_blocking, spawn_mandatory_blocking};

mod schedule;
mod shutdown;

mod task;
pub use task::error::JoinError;
pub use task::id::{Id, id, try_id};
pub use task::join::JoinHandle;

mod blocking_task;
pub(crate) use blocking_task::BlockingTask;

mod runtime;
pub use runtime::Runtime;

pub mod builder;
pub mod task_hooks;

pub mod handle;
pub use handle::Handle;

mod context;
pub mod testing;
pub use context::current::SetCurrentGuard;
pub use context::is_pool_worker;

mod future;
mod park;
mod util;

pub use dbt_runtime_macros::main;
pub use dbt_runtime_macros::test;
pub use dbt_runtime_macros::worker_test;
