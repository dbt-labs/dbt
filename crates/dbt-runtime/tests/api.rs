//! Names every public item from outside the crate, so the surface cannot rot
//! silently. Also exercises the parts that need no task to run: building a
//! runtime spawns no threads (they start on first use), so shutdown and the
//! `EnterGuard` behaviour are covered here.

use std::num::NonZeroUsize;

use dbt_runtime::builder::Builder;
use dbt_runtime::handle::{EnterGuard, Handle, TryCurrentError};
use dbt_runtime::task_hooks::TaskMeta;
use dbt_runtime::{Id, JoinError, JoinHandle, Runtime, spawn_blocking, spawn_mandatory_blocking};

#[expect(dead_code)]
fn build() -> Runtime {
    Builder::new()
        .thread_name("worker")
        .max_blocking_threads(4)
        .keep_alive(std::time::Duration::from_secs(1))
        .on_thread_start(|| {})
        .on_thread_stop(|| {})
        .on_task_terminate(|meta: &TaskMeta<'_>| {
            let _: Id = meta.id();
            let _ = meta.spawned_at();
        })
        .build()
}

#[expect(dead_code)]
fn use_runtime(rt: Runtime) {
    let handle: &Handle = rt.handle();

    let spawned: JoinHandle<u32> = rt.spawn_blocking(|| 1u32);
    let mandatory: Option<JoinHandle<u32>> = rt.spawn_mandatory_blocking(|| 1u32);
    let via_handle: JoinHandle<u32> = handle.spawn_blocking(|| 1u32);

    let _: Id = spawned.id();
    let _: &'static std::panic::Location<'static> = spawned.spawned_at();

    let _: usize = handle.max_parallelism();
    handle.set_max_parallelism(NonZeroUsize::new(8).unwrap());
    let _: usize = handle.num_blocking_threads();
    let _: usize = handle.num_idle_blocking_threads();
    let _: usize = handle.blocking_queue_depth();

    let guard: EnterGuard<'_> = handle.enter();
    let ambient: JoinHandle<u32> = spawn_blocking(|| 1u32);
    let ambient_mandatory: Option<JoinHandle<u32>> = spawn_mandatory_blocking(|| 1u32);
    drop(guard);

    // Dropping a `JoinHandle` detaches; that is the documented behaviour.
    drop((spawned, mandatory, via_handle, ambient, ambient_mandatory));

    rt.shutdown_timeout(std::time::Duration::from_secs(1));
}

#[expect(dead_code)]
fn errors(e: JoinError) -> std::io::Error {
    let _: Result<Handle, TryCurrentError> = Handle::try_current();
    let _: bool = e.is_panic();
    let _: bool = e.is_cancelled();
    let _: Id = e.id();
    e.into()
}

#[test]
fn public_api_compiles() {}

/// `block_on_worker` is the test-support entry point behind
/// `#[dbt_runtime::worker_test]`: it runs the closure on a worker thread, so
/// code that insists on being called from one is satisfied.
#[test]
fn block_on_worker_runs_the_closure_on_a_pool_worker() {
    let (value, on_worker) =
        dbt_runtime::testing::block_on_worker(|| (7u32, dbt_runtime::is_pool_worker()));
    assert_eq!(value, 7);
    assert!(on_worker, "the closure must run on a pool worker");
    assert!(
        !dbt_runtime::is_pool_worker(),
        "the calling thread is not a worker itself"
    );
}

/// A panic in the closure surfaces on the calling thread, so assertion messages
/// and `#[should_panic]` keep working through the macro.
#[test]
#[should_panic(expected = "from the worker")]
fn block_on_worker_resumes_a_panic_from_the_worker() {
    dbt_runtime::testing::block_on_worker(|| panic!("from the worker"));
}

/// Building a runtime spawns no threads: they start on first use. So this also
/// exercises shutdown -> `park::block_on` on the empty case.
#[test]
fn build_and_shut_down_an_idle_runtime() {
    let rt = Builder::new().max_blocking_threads(2).build();
    assert_eq!(rt.handle().num_blocking_threads(), 0);
    assert_eq!(rt.handle().blocking_queue_depth(), 0);
    rt.shutdown_timeout(std::time::Duration::from_secs(5));
}

#[test]
fn dropping_a_runtime_shuts_it_down() {
    drop(Builder::new().build());
}

#[test]
fn shutdown_background_does_not_wait() {
    Builder::new().build().shutdown_background();
}

/// `spawned_at()` reads the location out of the task allocation through the
/// vtable offsets in `Header`, so this checks both that `#[track_caller]` is
/// unbroken along the spawn path and that the offset arithmetic lands on the
/// right field.
#[test]
fn a_join_handle_reports_its_spawn_site() {
    let rt = Builder::new().build();

    let spawn_line = line!() + 1;
    let handle = rt.spawn_blocking(|| 7u32);

    let at = handle.spawned_at();
    assert_eq!(at.file(), file!(), "the call site, not the pool internals");
    assert_eq!(at.line(), spawn_line);
}

#[test]
fn there_is_no_ambient_runtime_by_default() {
    assert!(
        Handle::try_current().is_err(),
        "a handle must be entered explicitly"
    );
}

/// Worker threads deliberately do not enter their own runtime, so the free
/// `spawn_blocking` is unavailable inside a task. Entering is explicit and
/// scoped to the guard.
#[test]
fn entering_a_handle_is_explicit_and_scoped() {
    let rt = Builder::new().build();

    assert!(Handle::try_current().is_err());
    {
        let _guard = rt.handle().enter();
        assert!(Handle::try_current().is_ok());
    }
    assert!(
        Handle::try_current().is_err(),
        "the guard restores the previous handle on drop"
    );
}

#[test]
fn entered_handles_nest() {
    let outer = Builder::new().build();
    let inner = Builder::new().build();

    let g1 = outer.handle().enter();
    let g2 = inner.handle().enter();
    assert!(Handle::try_current().is_ok());
    drop(g2);
    assert!(
        Handle::try_current().is_ok(),
        "dropping the inner guard restores the outer handle"
    );
    drop(g1);
    assert!(Handle::try_current().is_err());
}
