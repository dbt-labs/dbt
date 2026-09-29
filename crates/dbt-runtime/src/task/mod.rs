//! Mirrors tokio's `runtime/task/mod.rs`, trimmed to what a blocking pool
//! needs.
//!
//! Blocking tasks are *unowned*: they are handed straight to a worker thread
//! and never live in an `OwnedTasks` list, never sit in a scheduler run queue,
//! and are never rescheduled (a `BlockingTask` is `Ready` on its first poll).
//! Everything upstream keeps for those three purposes is left out here —
//! `LocalNotified`, `OwnedTasks`/`list.rs`, `Notified`'s raw conversions, the
//! `taskdump` hooks, and `Header::owner_id`. `Notified` itself is kept only
//! because [`Schedule`] mentions it.

use std::marker::PhantomData;
use std::panic::Location;
use std::ptr::NonNull;
use std::{fmt, mem};

use crate::future::Future;
use crate::task::core::Header;
use crate::task::error::JoinError;
use crate::task::join::JoinHandle;
use crate::task::raw::RawTask;
use crate::task_hooks::TaskCallback;

pub mod core;
pub mod error;
pub mod harness;
pub mod id;
pub mod join;
pub mod raw;
pub mod state;
pub mod waker;

pub use crate::task::id::Id;

/// An owned handle to the task, tracked by ref count.
#[repr(transparent)]
pub(crate) struct Task<S: 'static> {
    raw: RawTask,
    _p: PhantomData<S>,
}

unsafe impl<S> Send for Task<S> {}
unsafe impl<S> Sync for Task<S> {}

/// A task was notified.
#[repr(transparent)]
pub(crate) struct Notified<S: 'static>(Task<S>);

// safety: This type cannot be used to touch the task without first verifying
// that the value is on a thread where it is safe to poll the task.
unsafe impl<S: Schedule> Send for Notified<S> {}
unsafe impl<S: Schedule> Sync for Notified<S> {}

/// A task that is not owned by any `OwnedTasks`. Used for blocking tasks.
/// This type holds two ref-counts.
pub(crate) struct UnownedTask<S: 'static> {
    raw: RawTask,
    _p: PhantomData<S>,
}

// safety: This type can only be created given a Send task.
unsafe impl<S> Send for UnownedTask<S> {}
unsafe impl<S> Sync for UnownedTask<S> {}

/// Task result sent back.
pub(crate) type Result<T> = std::result::Result<T, JoinError>;

/// Hooks for scheduling tasks which are needed in the task harness.
#[derive(Clone)]
pub(crate) struct TaskHarnessScheduleHooks {
    pub(crate) task_terminate_callback: Option<TaskCallback>,
}

pub(crate) trait Schedule: Sync + Sized + 'static {
    /// The task has completed work and is ready to be released. The scheduler
    /// should release it immediately and return it. The task module will batch
    /// the ref-dec with setting other options.
    ///
    /// If the scheduler has already released the task, then None is returned.
    fn release(&self, task: &Task<Self>) -> Option<Task<Self>>;

    /// Schedule the task
    fn schedule(&self, task: Notified<Self>);

    fn hooks(&self) -> TaskHarnessScheduleHooks;

    /// Schedule the task to run in the near future, yielding the thread to
    /// other tasks.
    fn yield_now(&self, task: Notified<Self>) {
        self.schedule(task);
    }

    /// Polling the task resulted in a panic. Should the runtime shutdown?
    fn unhandled_panic(&self) {
        // By default, do nothing. This maintains the 1.0 behavior.
    }
}

/// This is the constructor for a new task. Three references to the task are
/// created. The first task reference is usually put into an `OwnedTasks`
/// immediately. The Notified is sent to the scheduler as an ordinary
/// notification.
fn new_task<T, S>(
    task: T,
    scheduler: S,
    id: Id,
    spawned_at: SpawnLocation,
) -> (Task<S>, Notified<S>, JoinHandle<T::Output>)
where
    S: Schedule,
    T: Future + 'static,
    T::Output: 'static,
{
    let raw = RawTask::new::<T, S>(task, scheduler, id, spawned_at);
    let task = Task {
        raw,
        _p: PhantomData,
    };
    let notified = Notified(Task {
        raw,
        _p: PhantomData,
    });
    let join = JoinHandle::new(raw);

    (task, notified, join)
}

/// Creates a new task with an associated join handle. This method is used
/// only when the task is not going to be stored in an `OwnedTasks` list.
///
/// Currently only blocking tasks use this method.
pub(crate) fn unowned<T, S>(
    task: T,
    scheduler: S,
    id: Id,
    spawned_at: SpawnLocation,
) -> (UnownedTask<S>, JoinHandle<T::Output>)
where
    S: Schedule,
    T: Send + Future + 'static,
    T::Output: Send + 'static,
{
    let (task, notified, join) = new_task(task, scheduler, id, spawned_at);

    // This transfers the ref-count of task and notified into an UnownedTask.
    // This is valid because an UnownedTask holds two ref-counts.
    let unowned = UnownedTask {
        raw: task.raw,
        _p: PhantomData,
    };
    mem::forget(task);
    mem::forget(notified);

    (unowned, join)
}

impl<S: 'static> Task<S> {
    unsafe fn new(raw: RawTask) -> Task<S> {
        Task {
            raw,
            _p: PhantomData,
        }
    }

    /// # Safety
    ///
    /// `ptr` must be a valid pointer to a [`Header`].
    unsafe fn from_raw(ptr: NonNull<Header>) -> Task<S> {
        unsafe { Task::new(RawTask::from_raw(ptr)) }
    }

    fn header(&self) -> &Header {
        self.raw.header()
    }
}

impl<S: Schedule> Task<S> {
    /// Preemptively cancels the task as part of the shutdown process.
    pub(crate) fn shutdown(self) {
        let raw = self.raw;
        mem::forget(self);
        raw.shutdown();
    }
}

impl<S: Schedule> UnownedTask<S> {
    fn into_task(self) -> Task<S> {
        // Convert into a task.
        let task = Task {
            raw: self.raw,
            _p: PhantomData,
        };
        mem::forget(self);

        // Drop a ref-count since an UnownedTask holds two.
        task.header().state.ref_dec();

        task
    }

    pub(crate) fn run(self) {
        let raw = self.raw;
        mem::forget(self);

        // Transfer one ref-count to a Task object.
        let task = Task::<S> {
            raw,
            _p: PhantomData,
        };

        // Use the other ref-count to poll the task.
        raw.poll();
        // Decrement our extra ref-count
        drop(task);
    }

    pub(crate) fn shutdown(self) {
        self.into_task().shutdown();
    }
}

impl<S: 'static> Drop for Task<S> {
    fn drop(&mut self) {
        // Decrement the ref count
        if self.header().state.ref_dec() {
            // Deallocate if this is the final ref count
            self.raw.dealloc();
        }
    }
}

impl<S: 'static> Drop for UnownedTask<S> {
    fn drop(&mut self) {
        // Decrement the ref count
        if self.raw.header().state.ref_dec_twice() {
            // Deallocate if this is the final ref count
            self.raw.dealloc();
        }
    }
}

impl<S> fmt::Debug for Task<S> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "Task({:p})", self.header())
    }
}

impl<S> fmt::Debug for Notified<S> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "task::Notified({:p})", self.0.header())
    }
}

#[derive(Copy, Clone)]
pub(crate) struct SpawnLocation(pub &'static Location<'static>);

impl From<&'static Location<'static>> for SpawnLocation {
    fn from(location: &'static Location<'static>) -> Self {
        Self(location)
    }
}

impl SpawnLocation {
    #[track_caller]
    #[inline]
    pub(crate) fn capture() -> Self {
        Self::from(Location::caller())
    }
}
