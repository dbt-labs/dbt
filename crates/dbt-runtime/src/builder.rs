use std::sync::Arc;
use std::time::Duration;

use crate::runtime::{Callback, Runtime};
use crate::task_hooks::{TaskCallback, TaskMeta};

/// Name fn used for threads spawned by the pool.
pub type ThreadNameFn = Arc<dyn Fn() -> String + Send + Sync + 'static>;

/// Default upper bound on pool threads.
///
/// This is how many threads can be running jinja and database queries
/// at the same time. Configuration is dynamic, this is just the default.
pub const DEFAULT_MAX_BLOCKING_THREADS: usize = 48;

/// Configures and builds a [`Runtime`].
#[derive(Clone)]
pub struct Builder {
    /// Name fn used for threads spawned by the pool.
    pub(crate) thread_name: ThreadNameFn,

    /// Stack size used for threads spawned by the pool.
    pub(crate) thread_stack_size: Option<usize>,

    /// Callback to run after each thread starts.
    pub(crate) after_start: Option<Callback>,

    /// To run before each thread stops.
    pub(crate) before_stop: Option<Callback>,

    /// To run after each task terminates.
    pub(crate) task_terminate_callback: Option<TaskCallback>,

    /// Cap on thread usage.
    pub(crate) max_blocking_threads: usize,

    /// How long an idle worker waits before exiting.
    pub(crate) keep_alive: Option<Duration>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Builder {
        Builder {
            thread_name: Arc::new(|| "dbt-blocking-worker".into()),
            thread_stack_size: None,
            after_start: None,
            before_stop: None,
            task_terminate_callback: None,
            max_blocking_threads: DEFAULT_MAX_BLOCKING_THREADS,
            keep_alive: None,
        }
    }

    /// Sets a fixed name for every thread the pool spawns.
    pub fn thread_name(&mut self, val: impl Into<String>) -> &mut Self {
        let val = val.into();
        self.thread_name = Arc::new(move || val.clone());
        self
    }

    /// Sets a closure invoked to name each thread the pool spawns.
    pub fn thread_name_fn<F>(&mut self, f: F) -> &mut Self
    where
        F: Fn() -> String + Send + Sync + 'static,
    {
        self.thread_name = Arc::new(f);
        self
    }

    /// Sets the stack size, in bytes, for threads the pool spawns.
    pub fn thread_stack_size(&mut self, val: usize) -> &mut Self {
        self.thread_stack_size = Some(val);
        self
    }

    /// Runs `f` on each worker thread after it starts.
    pub fn on_thread_start<F>(&mut self, f: F) -> &mut Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.after_start = Some(Arc::new(f));
        self
    }

    /// Runs `f` on each worker thread before it stops.
    pub fn on_thread_stop<F>(&mut self, f: F) -> &mut Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.before_stop = Some(Arc::new(f));
        self
    }

    /// Runs `f` after each task terminates.
    pub fn on_task_terminate<F>(&mut self, f: F) -> &mut Self
    where
        F: Fn(&TaskMeta<'_>) + Send + Sync + 'static,
    {
        self.task_terminate_callback = Some(Arc::new(f));
        self
    }

    /// Caps the number of threads the pool will spawn.
    ///
    /// The pool grows on demand: a task submitted while every thread is busy
    /// starts a new one, up to this cap. Past the cap, work accumulates in
    /// a queue being consumed by the thread pool.
    ///
    /// The threads are not always active and will exit if left idle for too
    /// long. You can change this timeout duration with [`thread_keep_alive`].
    ///
    /// # Queue Behavior
    ///
    /// When a blocking task is submitted, it will be inserted into a queue. If available, one of
    /// the idle threads will be notified to run the task. Otherwise, if the threshold set by this
    /// method has not been reached, a new thread will be spawned. If no idle thread is available
    /// and no more threads are allowed to be spawned, the task will remain in the queue until one
    /// of the busy threads pick it up. Note that since the queue does not apply any backpressure,
    /// it could potentially grow unbounded.
    ///
    /// # Panics
    ///
    /// This will panic if `val` is not larger than `0`.
    ///
    /// # Deadlock risk
    ///
    /// A task that *waits* on the [`JoinHandle`] of another task on the same
    /// pool can deadlock. Once `max_blocking_threads` tasks are each waiting
    /// on work that is still queued, no thread is ever freed to run it and the
    /// queue never drains. The smaller the cap, the easier this is to hit.
    ///
    /// [`Handle`]: crate::handle::Handle
    /// [`JoinHandle`]: crate::task::join::JoinHandle
    /// [`thread_keep_alive`]: Self::thread_keep_alive
    pub fn max_blocking_threads(&mut self, val: usize) -> &mut Self {
        assert!(val > 0, "max_blocking_threads must be greater than 0");
        self.max_blocking_threads = val;
        self
    }

    /// Sets how long an idle worker waits before exiting. Defaults to 10s.
    pub fn keep_alive(&mut self, duration: Duration) -> &mut Self {
        self.keep_alive = Some(duration);
        self
    }

    /// Builds the runtime that owns the pool.
    pub fn build(&self) -> Runtime {
        Runtime::new(self)
    }
}
