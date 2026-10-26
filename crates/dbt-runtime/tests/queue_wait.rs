use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use dbt_runtime::builder::Builder;
use dbt_runtime::{JoinHandle, Runtime, spawn_blocking_with_wait_guard};
use futures::FutureExt;

const THRESHOLD: Duration = Duration::from_millis(1);
const WATCHDOG: Duration = Duration::from_secs(5);

/// Counts guard destruction, so the worker can verify that waiting ended first.
struct WaitGuard(Arc<AtomicUsize>);

impl Drop for WaitGuard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Occupies the only worker until the sender releases it. The acknowledgement
/// ensures subsequent test steps see running work, not a race with worker startup.
fn occupy_worker(runtime: &Runtime) -> (mpsc::Sender<()>, JoinHandle<()>) {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let task = runtime.spawn_blocking(move || {
        started_tx.send(()).unwrap();
        release_rx.recv_timeout(WATCHDOG).unwrap();
    });
    started_rx.recv_timeout(WATCHDOG).unwrap();
    (release_tx, task)
}

/// Already-running work must not open a wait when its execution exceeds the
/// threshold. The current-thread timer driver cannot progress before the startup
/// acknowledgement, so timer expiry is observed only after work is running.
#[tokio::test(flavor = "current_thread")]
async fn running_work_never_opens_a_wait() {
    let runtime = Builder::new().max_blocking_threads(1).build();
    let enter = runtime.handle().enter();
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut task = Box::pin(spawn_blocking_with_wait_guard(
        THRESHOLD,
        || panic!("execution time is not queue wait"),
        move || {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(WATCHDOG).unwrap();
            42
        },
    ));

    // Submit and acknowledge startup before yielding to the timer driver.
    assert!(futures::poll!(&mut task).is_pending());
    started_rx.recv_timeout(WATCHDOG).unwrap();
    // Expire the timer while work remains running: the consumed slot suppresses
    // the callback, even though the join handle is still pending.
    tokio::time::sleep(THRESHOLD).await;
    assert!(futures::poll!(&mut task).is_pending());
    release_tx.send(()).unwrap();
    assert_eq!(task.await.unwrap(), 42);
    drop(enter);
    runtime.shutdown_background();
}

/// A timeout creates exactly one guard; detaching the future must leave that
/// guard with queued work, which closes it before invoking the user closure.
#[tokio::test(flavor = "current_thread")]
async fn queued_guard_survives_detachment_and_closes_before_work() {
    let runtime = Builder::new().max_blocking_threads(1).build();
    let enter = runtime.handle().enter();
    let (release_worker, worker) = occupy_worker(&runtime);
    let drops = Arc::new(AtomicUsize::new(0));
    let in_work = drops.clone();
    let mut callbacks = 0;
    let (finished_tx, finished_rx) = mpsc::channel();
    let mut task = Box::pin(spawn_blocking_with_wait_guard(
        THRESHOLD,
        || {
            callbacks += 1;
            WaitGuard(drops.clone())
        },
        move || {
            assert_eq!(in_work.load(Ordering::SeqCst), 1);
            finished_tx.send(()).unwrap();
        },
    ));

    // The sole worker stays occupied throughout submission and timer expiry.
    assert!(futures::poll!(&mut task).is_pending());
    tokio::time::sleep(THRESHOLD).await;
    assert!(futures::poll!(&mut task).is_pending());
    drop(task);
    assert_eq!(callbacks, 1);
    assert_eq!(drops.load(Ordering::SeqCst), 0);

    release_worker.send(()).unwrap();
    worker.await.unwrap();
    // There is no join handle after detachment; acknowledge completion explicitly.
    finished_rx.recv_timeout(WATCHDOG).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    drop(enter);
    runtime.shutdown_background();
}

/// A shut-down pool rejects work before the timeout. Discarding the closure
/// must close its slot without invoking either the guard callback or user work.
#[tokio::test(flavor = "current_thread")]
async fn shutdown_rejects_work_without_opening_a_wait() {
    let runtime = Builder::new().max_blocking_threads(1).build();
    let handle = runtime.handle().clone();
    let _enter = handle.enter();
    runtime.shutdown_background();

    let error = spawn_blocking_with_wait_guard(
        Duration::ZERO,
        || panic!("rejected work must not open a wait"),
        || panic!("rejected work must not run"),
    )
    .await
    .unwrap_err();
    assert!(error.is_cancelled());
}

/// A callback panic unwinds the awaiter while holding the slot mutex. Recovering
/// that mutex must let detached work run rather than causing a second panic.
#[tokio::test(flavor = "current_thread")]
async fn callback_panic_does_not_poison_detached_work() {
    let runtime = Builder::new().max_blocking_threads(1).build();
    let enter = runtime.handle().enter();
    let (release_worker, worker) = occupy_worker(&runtime);
    let (finished_tx, finished_rx) = mpsc::channel();
    let task = std::panic::AssertUnwindSafe(spawn_blocking_with_wait_guard(
        THRESHOLD,
        || panic!("guard initialization failed"),
        move || finished_tx.send(()).unwrap(),
    ))
    .catch_unwind();

    assert!(task.await.is_err());
    release_worker.send(()).unwrap();
    worker.await.unwrap();
    finished_rx.recv_timeout(WATCHDOG).unwrap();
    drop(enter);
    runtime.shutdown_background();
}
