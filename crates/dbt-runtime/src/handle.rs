//! Mirrors tokio's `runtime/handle.rs`, scoped to the blocking pool.
//!
//! A `Handle` is a cheap, cloneable reference to a running [`Runtime`]. Setting
//! one as current (via [`Handle::enter`]) is what lets the free
//! [`spawn_blocking`] find a pool without being handed one.
//!
//! # Shape
//!
//! Upstream, `Handle` wraps a `scheduler::Handle`, an `Arc` holding the
//! scheduler's shared state — including `blocking_spawner`, its clone of the
//! pool's [`Spawner`], and the runtime's `task_hooks`. [`HandleInner`] here
//! plays that role. It is why upstream can pass `rt: &Handle` down into the
//! pool and have the pool read configuration back off it, and why this crate
//! now does the same.
//!
//! This is the crate's *own* handle, and it is the only ambient context the
//! pool has. It is deliberately not a `tokio::runtime::Handle`: entering this
//! one says "blocking work can be handed to this pool", which is a fact about
//! this crate, not about any tokio runtime. The one place the crate does consult
//! tokio's handle is `context::blocking`, to answer a different question —
//! whether the *caller* is on a thread driving a tokio runtime, and so must not
//! be blocked.
//!
//! # Who gets a handle
//!
//! Any thread that wants to hand work off, by way of [`Handle::enter`]. Worker
//! threads enter it too, as upstream's do, which is the line that makes the free
//! [`spawn_blocking`] usable from inside a blocking task — see decision D1 in
//! `tokio-blocking-map.md` for why that must ultimately be rejected at the spawn
//! site instead.
//!
//! [`Runtime`]: crate::runtime::Runtime
//! [`spawn_blocking`]: crate::pool::spawn_blocking

use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::sync::Arc;

use crate::context::current;
use crate::pool::Spawner;
use crate::task::join::JoinHandle;
use crate::task_hooks::TaskHooks;

/// A handle to a [`Runtime`].
///
/// Cloning is cheap: every clone refers to the same runtime.
///
/// [`Runtime`]: crate::runtime::Runtime
#[derive(Clone)]
pub struct Handle {
    pub(crate) inner: Arc<HandleInner>,
}

/// State the runtime shares with every [`Handle`] to it.
///
/// Stands in for tokio's `scheduler::Handle`.
pub(crate) struct HandleInner {
    /// The runtime's blocking pool.
    pub(crate) blocking_spawner: Spawner,

    /// Hooks invoked on task events.
    pub(crate) task_hooks: TaskHooks,
}

impl HandleInner {
    pub(crate) fn hooks(&self) -> &TaskHooks {
        &self.task_hooks
    }
}

/// Resets the current handle when dropped.
///
/// Guards must be dropped in the reverse order they were acquired.
#[must_use]
pub struct EnterGuard<'a> {
    _guard: current::SetCurrentGuard,
    _handle_lifetime: PhantomData<&'a Handle>,
}

impl Handle {
    pub(crate) fn new(inner: HandleInner) -> Handle {
        Handle {
            inner: Arc::new(inner),
        }
    }

    /// Returns the handle set for the current thread.
    ///
    /// # Panics
    ///
    /// Panics if called outside a [`Handle::enter`] scope. In particular this
    /// panics inside a blocking task, by design — see the module docs. For a
    /// non-panicking version see [`Handle::try_current`].
    #[track_caller]
    pub fn current() -> Handle {
        match Handle::try_current() {
            Ok(handle) => handle,
            Err(e) => {
                if cfg!(debug_assertions) {
                    panic!("{e}\n{:?}", std::backtrace::Backtrace::force_capture());
                } else {
                    panic!("{e}");
                }
            }
        }
    }

    /// Returns the handle set for the current thread, if any.
    pub fn try_current() -> Result<Handle, TryCurrentError> {
        current::with_current(Handle::clone)
    }

    /// Sets this handle as the current one until the returned guard is dropped.
    pub fn enter(&self) -> EnterGuard<'_> {
        EnterGuard {
            _guard: current::try_set_current(self)
                .expect("cannot enter a runtime handle while the thread is shutting down"),
            _handle_lifetime: PhantomData,
        }
    }

    /// Like [`Handle::enter`] but returns a guard that the caller can store
    /// outside of the call stack (this requires more care).
    ///
    /// # Safety
    ///
    /// The caller must ensure the returned [`SetCurrentGuard`] is dropped on
    /// the same thread that called this method, and in reverse order relative
    /// to any other entered guards on that thread.
    pub unsafe fn enter_owned(&self) -> crate::context::SetCurrentGuard {
        current::try_set_current(self)
            .expect("cannot enter a runtime handle while the thread is shutting down")
    }

    /// The runtime's blocking pool.
    pub(crate) fn blocking_spawner(&self) -> &Spawner {
        &self.inner.blocking_spawner
    }

    pub fn max_parallelism(&self) -> usize {
        self.blocking_spawner().max_parallelism()
    }

    /// Change the maximum number of threads of the running pool.
    ///
    /// Threads that are already running are not stopped: lowering the cap only
    /// prevents new threads from being spawned, so the pool converges to the
    /// new cap as idle threads exit after the keep-alive timeout.
    pub fn set_max_parallelism(&self, val: NonZeroUsize) {
        self.blocking_spawner().set_max_parallelism(val);
    }

    /// Runs `func` on the blocking pool.
    #[track_caller]
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.blocking_spawner().spawn_blocking(self, func)
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
        self.blocking_spawner().spawn_mandatory_blocking(self, func)
    }

    /// Number of threads the pool currently has alive.
    pub fn num_blocking_threads(&self) -> usize {
        self.blocking_spawner().num_threads()
    }

    /// Number of pool threads currently idle.
    pub fn num_idle_blocking_threads(&self) -> usize {
        self.blocking_spawner().num_idle_threads()
    }

    /// Number of tasks queued and not yet picked up by a thread.
    pub fn blocking_queue_depth(&self) -> usize {
        self.blocking_spawner().queue_depth()
    }
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("Handle").finish_non_exhaustive()
    }
}

/// Errors returned when there is no current [`Handle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryCurrentError {
    /// No runtime handle has been set for this thread.
    NoContext,
    /// The thread is terminating and its thread-locals are being destroyed.
    ThreadLocalDestroyed,
}

impl std::fmt::Display for TryCurrentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TryCurrentError::NoContext => f.write_str(
                "there is no blocking-pool runtime set for the current thread; \
                 call `Handle::enter()` first",
            ),
            TryCurrentError::ThreadLocalDestroyed => {
                f.write_str("the runtime handle is unavailable because the thread is shutting down")
            }
        }
    }
}

impl std::error::Error for TryCurrentError {}
