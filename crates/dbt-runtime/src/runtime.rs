//! Mirrors tokio's `runtime` but we only need it to carry the spawner
//! of tasks in the blocking dbt pool.

use std::num::NonZeroU64;
use std::time::Duration;

use crate::builder::Builder;
use crate::handle::{Handle, HandleInner};
use crate::pool::BlockingPool;
use crate::task::join::JoinHandle;
use crate::task_hooks::TaskHooks;

/// Boundary value to prevent stack overflow caused by a large-sized
/// Future being placed in the stack.
pub(crate) const BOX_FUTURE_THRESHOLD: usize = if cfg!(debug_assertions) { 2048 } else { 16384 };

/// After thread starts / before thread stops
pub(crate) type Callback = std::sync::Arc<dyn Fn() + Send + Sync>;

/// Owns a blocking pool and hands out [`Handle`]s to it.
///
/// This is the analogue of tokio's `Runtime`, minus the schedulers and drivers:
/// there is nothing to drive, only blocking work to hand off. What it exists
/// for is ownership. Upstream the pool is a field of `Runtime`, which is why
/// every path into the pool has a `&Handle` in scope to pass along, and why the
/// pool never has to store the runtime it belongs to. Reproducing that
/// ownership here is what lets `pool.rs` thread `rt: &Handle` exactly as
/// upstream does.
///
/// Dropping the `Runtime` shuts the pool down and joins its threads. Build one
/// with [`Builder`].
///
/// ```no_run
/// use dbt_runtime::builder::Builder;
///
/// let rt = Builder::new().max_blocking_threads(4).build();
/// let out = rt.spawn_blocking(|| 6 * 7);
/// // `out` is a `JoinHandle`; await it, or poll it on any executor.
/// drop(rt); // shuts the pool down and joins the workers
/// ```
#[derive(Debug)]
pub struct Runtime {
    /// Handle to the runtime. Holds a clone of the pool's spawner, so it stays
    /// usable for as long as any handle lives.
    handle: Handle,

    /// The pool itself, held here so that dropping the runtime shuts it down.
    ///
    /// Declared last: fields drop in declaration order, so `handle` is released
    /// before the pool waits on its workers.
    blocking_pool: BlockingPool,
}

impl Runtime {
    pub(crate) fn new(builder: &Builder) -> Runtime {
        let blocking_pool = BlockingPool::new(builder, builder.max_blocking_threads);

        let handle = Handle::new(HandleInner {
            blocking_spawner: blocking_pool.spawner().clone(),
            task_hooks: TaskHooks {
                task_terminate_callback: builder.task_terminate_callback.clone(),
            },
        });

        Runtime {
            handle,
            blocking_pool,
        }
    }

    /// A handle to this runtime, for [`Handle::enter`] and the free
    /// [`spawn_blocking`](crate::pool::spawn_blocking).
    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Runs `func` on the blocking pool.
    #[track_caller]
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.handle.spawn_blocking(func)
    }

    /// Runs `func` on the blocking pool as mandatory work.
    ///
    /// Mandatory tasks are guaranteed to run unless a shutdown is already
    /// taking place, in which case `None` is returned.
    #[track_caller]
    pub fn spawn_mandatory_blocking<F, R>(&self, func: F) -> Option<JoinHandle<R>>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.handle.spawn_mandatory_blocking(func)
    }

    /// Shuts the pool down, waiting at most `duration` for its threads to
    /// finish.
    ///
    /// Threads still running a task past the timeout become detached: they run
    /// to completion but are no longer joined.
    pub fn shutdown_timeout(mut self, duration: Duration) {
        self.blocking_pool.shutdown(Some(duration));
    }

    /// Shuts the pool down without waiting for its threads at all.
    pub fn shutdown_background(self) {
        self.shutdown_timeout(Duration::from_nanos(0));
    }
}

#[derive(Eq, PartialEq, Clone, Copy, Hash, Debug)]
pub(crate) struct ThreadId(NonZeroU64);

impl ThreadId {
    pub(crate) fn next() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

        static NEXT_ID: AtomicU64 = AtomicU64::new(0);

        let mut last = NEXT_ID.load(Relaxed);
        loop {
            let id = match last.checked_add(1) {
                Some(id) => id,
                None => exhausted(),
            };

            match NEXT_ID.compare_exchange_weak(last, id, Relaxed, Relaxed) {
                Ok(_) => return ThreadId(NonZeroU64::new(id).unwrap()),
                Err(id) => last = id,
            }
        }
    }
}

#[cold]
fn exhausted() -> ! {
    panic!("failed to generate unique thread ID: bitspace exhausted")
}
