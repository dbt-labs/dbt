//! Test support. Not meant for production code paths.

use crate::builder::Builder;
use crate::context::blocking::try_enter_blocking_region;

/// Runs `f` on a worker thread of a private pool and returns its value.
///
/// Adapter code asserts that database connections are only ever created by pool
/// worker threads. A test that reaches such code from the thread the test
/// harness gave it aborts the process, so it has to hand the work to a worker
/// the way production does. `f` therefore sees both the worker context and an
/// entered [`Handle`](crate::Handle), like any other blocking task.
///
/// A panic in `f` is resumed on the calling thread, so assertion messages and
/// `#[should_panic]` behave as if the body had run here.
///
/// Prefer [`worker_test`](crate::worker_test), which wraps a synchronous test
/// body in this.
///
/// # Panics
///
/// Panics if called from a pool worker or from inside a tokio runtime, where
/// blocking the caller risks deadlocking the pool. An async test should use
/// [`spawn_blocking`](crate::spawn_blocking) and await it instead.
pub fn block_on_worker<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let runtime = Builder::new().max_blocking_threads(4).build();
    let handle = runtime.spawn_blocking(f);

    let mut region = try_enter_blocking_region().expect(
        "block_on_worker() cannot block a pool worker or a thread driving a tokio runtime; \
         in an async test, spawn_blocking(..).await instead",
    );
    match region.block_on(handle) {
        Ok(Ok(value)) => value,
        Ok(Err(join_error)) if join_error.is_panic() => {
            std::panic::resume_unwind(join_error.into_panic())
        }
        Ok(Err(join_error)) => panic!("the worker thread never ran the body: {join_error}"),
        Err(access_error) => panic!("the calling thread cannot be parked: {access_error}"),
    }
}
