use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use dbt_runtime::builder::Builder;

#[dbt_runtime::test]
async fn test_macro_provides_dbt_runtime_handle() {
    let handle = dbt_runtime::Handle::try_current().expect("handle should be set");
    let result = handle.spawn_blocking(|| 6 * 7).await.unwrap();
    assert_eq!(result, 42);
}

#[dbt_runtime::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_macro_multi_thread() {
    let handle = dbt_runtime::Handle::try_current().expect("handle should be set");
    let result = handle.spawn_blocking(|| 6 * 7).await.unwrap();
    assert_eq!(result, 42);
}

#[test]
fn runs_a_blocking_task_end_to_end() {
    let rt = Builder::new().max_blocking_threads(2).build();
    let handle = rt.handle();

    let h = handle.spawn_blocking(|| 6 * 7);
    let out = futures::executor::block_on(h).expect("task should complete");
    assert_eq!(out, 42);

    drop(rt);
}

#[test]
fn runs_many_tasks() {
    let rt = Builder::new().max_blocking_threads(4).build();
    let handle = rt.handle();
    let counter = Arc::new(AtomicUsize::new(0));

    let handles: Vec<_> = (0..64)
        .map(|i| {
            let counter = Arc::clone(&counter);
            handle.spawn_blocking(move || {
                counter.fetch_add(1, Ordering::Relaxed);
                i * 2
            })
        })
        .collect();

    let mut sum = 0;
    for h in handles {
        sum += futures::executor::block_on(h).unwrap();
    }

    assert_eq!(counter.load(Ordering::Relaxed), 64);
    assert_eq!(sum, (0..64).map(|i| i * 2).sum::<i32>());
    drop(rt);
}

#[test]
fn a_panicking_task_reports_a_join_error() {
    let rt = Builder::new().build();
    let handle = rt.handle();

    let h = handle.spawn_blocking(|| panic!("boom"));
    let err = futures::executor::block_on(h).expect_err("should be a JoinError");
    assert!(err.is_panic());
    drop(rt);
}

#[test]
fn worker_threads_are_marked_as_pool_workers() {
    let rt = Builder::new().max_blocking_threads(2).build();
    let seen = Arc::new(AtomicBool::new(false));
    let seen2 = Arc::clone(&seen);

    let h = rt.handle().spawn_blocking(move || {
        seen2.store(dbt_runtime::is_pool_worker(), Ordering::Relaxed);
    });
    futures::executor::block_on(h).unwrap();
    assert!(
        seen.load(Ordering::Relaxed),
        "worker thread should report is_pool_worker() == true"
    );
    drop(rt);
}

/// Lets tasks occupy a worker thread until the test releases them.
struct Gate {
    open: Mutex<bool>,
    condvar: Condvar,
}

impl Gate {
    fn new() -> Self {
        Self {
            open: Mutex::new(false),
            condvar: Condvar::new(),
        }
    }

    fn wait(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.condvar.wait(open).unwrap();
        }
    }

    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.condvar.notify_all();
    }
}

/// Blocks until `predicate` holds, panicking if that takes too long.
fn wait_for(what: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !predicate() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn the_thread_cap_can_be_changed_while_the_pool_runs() {
    let rt = Builder::new().max_blocking_threads(4).build();
    assert_eq!(rt.handle().max_parallelism(), 4);

    rt.handle()
        .set_max_parallelism(NonZeroUsize::new(8).unwrap());
    assert_eq!(rt.handle().max_parallelism(), 8);
    drop(rt);
}

#[test]
fn raising_the_thread_cap_admits_more_threads() {
    let rt = Builder::new().max_blocking_threads(1).build();
    let handle = rt.handle();
    let gate = Arc::new(Gate::new());
    let running = Arc::new(AtomicUsize::new(0));

    let occupied = {
        let gate = Arc::clone(&gate);
        let running = Arc::clone(&running);
        handle.spawn_blocking(move || {
            running.fetch_add(1, Ordering::SeqCst);
            gate.wait();
        })
    };
    wait_for("the first task to occupy the only thread", || {
        running.load(Ordering::SeqCst) == 1
    });

    // The cap is consulted when a task finds no idle thread, so this one gets a
    // thread of its own even though the first task never yields.
    handle.set_max_parallelism(NonZeroUsize::new(2).unwrap());
    let admitted = handle.spawn_blocking(|| 42);
    assert_eq!(futures::executor::block_on(admitted).unwrap(), 42);

    gate.open();
    futures::executor::block_on(occupied).unwrap();
    drop(rt);
}

#[test]
fn lowering_the_thread_cap_below_the_running_threads_stops_spawning() {
    let rt = Builder::new().max_blocking_threads(4).build();
    let handle = rt.handle();
    let gate = Arc::new(Gate::new());
    let running = Arc::new(AtomicUsize::new(0));

    let occupied: Vec<_> = (0..3)
        .map(|_| {
            let gate = Arc::clone(&gate);
            let running = Arc::clone(&running);
            handle.spawn_blocking(move || {
                running.fetch_add(1, Ordering::SeqCst);
                gate.wait();
            })
        })
        .collect();
    wait_for("three tasks to occupy three threads", || {
        running.load(Ordering::SeqCst) == 3
    });
    assert_eq!(handle.num_blocking_threads(), 3);

    // Three threads are already running, which is over the new cap, so this
    // task waits in the queue instead of getting a fourth thread.
    handle.set_max_parallelism(NonZeroUsize::new(2).unwrap());
    let queued = handle.spawn_blocking(|| 42);
    assert_eq!(handle.blocking_queue_depth(), 1);
    assert_eq!(handle.num_blocking_threads(), 3);

    gate.open();
    assert_eq!(futures::executor::block_on(queued).unwrap(), 42);
    for handle in occupied {
        futures::executor::block_on(handle).unwrap();
    }
    drop(rt);
}
