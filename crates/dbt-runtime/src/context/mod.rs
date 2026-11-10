//! Analogous to tokio's runtime CONTEXT thread-local but carries
//! the handle that gives access to the spawner of tasks in the
//! blocking pool for jinja and dataabse work.

use std::cell::Cell;
use std::thread::AccessError;

use crate::runtime::ThreadId;
use crate::task::id::Id;

pub mod blocking;

pub mod current;
pub use current::SetCurrentGuard;

use current::HandleCell;

struct Context {
    /// Uniquely identifies the current thread
    thread_id: Cell<Option<ThreadId>>,

    /// Handle to the context that allows spawning tasks on the dbt blocking pool.
    current: HandleCell,

    /// The id of the task currently being polled on this thread, if any.
    current_task_id: Cell<Option<Id>>,

    /// Whether this thread is one of the pool's own worker threads.
    ///
    /// Distinct from `current` being set: a caller may enter a handle on any
    /// thread, but only a worker may not block on the pool's shutdown.
    is_pool_worker: Cell<bool>,
}

thread_local! {
    static CONTEXT: Context = const {
        Context {
            thread_id: Cell::new(None),
            current: HandleCell::new(),
            current_task_id: Cell::new(None),
            is_pool_worker: Cell::new(false),
        }
    };
}

#[expect(dead_code)]
pub(crate) fn thread_id() -> Result<ThreadId, AccessError> {
    CONTEXT.try_with(|ctx| match ctx.thread_id.get() {
        Some(id) => id,
        None => {
            let id = ThreadId::next();
            ctx.thread_id.set(Some(id));
            id
        }
    })
}

pub(crate) fn set_current_task_id(id: Option<Id>) -> Option<Id> {
    CONTEXT
        .try_with(|ctx| ctx.current_task_id.replace(id))
        .unwrap_or(None)
}

pub(crate) fn current_task_id() -> Option<Id> {
    CONTEXT
        .try_with(|ctx| ctx.current_task_id.get())
        .unwrap_or(None)
}

/// Whether the calling thread is one of the pool's worker threads.
pub fn is_pool_worker() -> bool {
    CONTEXT
        .try_with(|ctx| ctx.is_pool_worker.get())
        .unwrap_or(false)
}

pub(crate) fn set_pool_worker(val: bool) {
    CONTEXT.with(|ctx| ctx.is_pool_worker.set(val));
}
