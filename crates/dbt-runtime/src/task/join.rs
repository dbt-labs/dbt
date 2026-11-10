//! Mirrors tokio's `runtime/task/join.rs`.

use std::fmt;
use std::marker::PhantomData;
use std::panic::{Location, RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::task::core::Header;
use crate::task::raw::RawTask;

/// An owned permission to await the completion of a blocking task, and to read
/// its output.
///
/// Dropping a `JoinHandle` **detaches**: the work stays submitted and runs to
/// completion, only the output is discarded. A caller that stops waiting has
/// not thereby un-submitted work that may already be half-done.
pub struct JoinHandle<T> {
    raw: RawTask,
    _p: PhantomData<T>,
}

unsafe impl<T: Send> Send for JoinHandle<T> {}
unsafe impl<T: Send> Sync for JoinHandle<T> {}

impl<T> UnwindSafe for JoinHandle<T> {}
impl<T> RefUnwindSafe for JoinHandle<T> {}

impl<T> JoinHandle<T> {
    pub(super) fn new(raw: RawTask) -> JoinHandle<T> {
        JoinHandle {
            raw,
            _p: PhantomData,
        }
    }

    /// Returns a [task ID] that uniquely identifies this task relative to other
    /// currently spawned tasks.
    ///
    /// [task ID]: crate::task::Id
    pub fn id(&self) -> super::Id {
        // Safety: The header pointer is valid.
        unsafe { Header::get_id(self.raw.header_ptr()) }
    }

    /// Returns the source code location where this task was spawned.
    ///
    /// Useful for attributing a task that is taking too long, or one whose
    /// `JoinHandle` a caller is stuck on: the location names the
    /// `spawn_blocking` call site rather than the pool.
    pub fn spawned_at(&self) -> &'static Location<'static> {
        // Safety: The header pointer is valid.
        unsafe { Header::get_spawn_location(self.raw.header_ptr()) }
    }
}

impl<T> Unpin for JoinHandle<T> {}

impl<T> Future for JoinHandle<T> {
    type Output = super::Result<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Upstream also runs `trace_leaf` (taskdump) and takes a coop budget
        // here. Neither applies: taskdump is out of scope, and `coop` is
        // tokio-private. See `blocking_task.rs` for the same note.
        let mut ret = Poll::Pending;

        // Try to read the task output. If the task is not yet complete, the
        // waker is stored and is notified once the task does complete.
        //
        // The function must go via the vtable, which requires erasing generic
        // types. To do this, the function "return" is placed on the stack
        // **before** calling the function and is passed into the function using
        // `*mut ()`.
        //
        // Safety:
        //
        // The type of `T` must match the task's output type.
        unsafe {
            self.raw.try_read_output(&mut ret, cx.waker());
        }

        ret
    }
}

impl<T> Drop for JoinHandle<T> {
    fn drop(&mut self) {
        if self.raw.state().drop_join_handle_fast().is_ok() {
            return;
        }

        self.raw.drop_join_handle_slow();
    }
}

impl<T> fmt::Debug for JoinHandle<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Safety: The header pointer is valid.
        let id_ptr = unsafe { Header::get_id_ptr(self.raw.header_ptr()) };
        let id = unsafe { id_ptr.as_ref() };
        fmt.debug_struct("JoinHandle").field("id", id).finish()
    }
}
