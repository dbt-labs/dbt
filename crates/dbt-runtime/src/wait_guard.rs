//! Optional queue-wait observation composed over the ordinary blocking pool API.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{JoinError, spawn_blocking};

/// Runs `func` on the blocking pool, optionally holding a guard while it waits.
///
/// If the task has not started after `threshold`, calls `wait_guard` on the
/// awaiting task and stores its result until a worker starts `func`. The guard
/// is dropped before `func` runs, or if the queued task is discarded. Work that
/// starts before the timeout is observed never creates a guard, even if it runs
/// longer than `threshold`. The guard covers only the wait after the callback,
/// not the initial threshold or time before the awaiting task is polled again.
///
/// The callback runs under a private slot mutex, never the pool mutex. Keep it
/// short and do not wait for this task or other work in the same pool: a worker
/// may briefly wait for the callback to finish before starting `func`.
///
/// Like dropping a [`crate::JoinHandle`], dropping this future after its first
/// poll detaches the work. An existing guard stays with the queued work, but no
/// new guard is created after detachment. A callback panic also detaches the
/// work; it does not cancel it.
///
/// # Panics
///
/// Panics if polled without an entered [`crate::Handle`] or a Tokio runtime
/// with time enabled, or if `wait_guard` panics.
pub async fn spawn_blocking_with_wait_guard<F, R, G>(
    threshold: Duration,
    wait_guard: impl FnOnce() -> G,
    func: F,
) -> Result<R, JoinError>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
    G: Send + 'static,
{
    let slot = Arc::new(Mutex::new(Slot::Empty));
    let consumer = Consumer(Arc::clone(&slot));
    let mut handle = spawn_blocking(move || {
        // Atomically prevent late initialization, then drop any guard before work.
        drop(consumer);
        func()
    });

    match tokio::time::timeout(threshold, &mut handle).await {
        Ok(result) => result,
        Err(_) => {
            {
                let mut slot = slot
                    .lock()
                    .expect("[internal exception] wait guard slot mutex was unexpectedly poisoned");
                if matches!(*slot, Slot::Empty) {
                    *slot = Slot::Filled(wait_guard());
                }
            }
            handle.await
        }
    }
}

enum Slot<G> {
    Empty,
    Filled(G),
    Consumed,
}

/// Consumes the slot both on admission and when the pool discards queued work.
struct Consumer<G>(Arc<Mutex<Slot<G>>>);

impl<G> Drop for Consumer<G> {
    fn drop(&mut self) {
        let previous = {
            // A panicking callback leaves the slot empty. The detached worker
            // must still be able to consume it and run, as with ordinary spawn.
            // This critical section only locks and replaces the slot, so it cannot panic.
            let mut slot = self.0.lock().unwrap_or_else(|error| error.into_inner());
            std::mem::replace(&mut *slot, Slot::Consumed)
        };
        // Guard destruction is caller code too; run it outside the slot mutex.
        drop(previous);
    }
}
