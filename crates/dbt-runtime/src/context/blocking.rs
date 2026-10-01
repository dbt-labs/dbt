//! Mirrors tokio's `runtime/context/blocking.rs`.

use std::marker::PhantomData;
use std::thread::AccessError;
use std::time::Duration;

use crate::util::markers::NotSendOrSync;

/// Guard tracking that a caller has entered a blocking region.
#[must_use]
pub(crate) struct BlockingRegionGuard {
    _p: PhantomData<NotSendOrSync>,
}

/// Returns a guard if it is safe to block the current thread, `None` if the
/// caller is inside a tokio async context or in a dbt blocking context.
pub(crate) fn try_enter_blocking_region() -> Option<BlockingRegionGuard> {
    use tokio::runtime::Handle;
    // A pool worker must never block on its own pool's shutdown: shutdown
    // waits for every worker to exit, including this one. Upstream has no
    // equivalent guard, because there a blocking worker is not "entered".
    //
    // And shutdown is not the only situation where this would cause a
    // deadlock. Depending on what the blocking threads are waiting on,
    // trying to spawn a new task on the blocking pool can cause a deadlock.
    if super::is_pool_worker() || Handle::try_current().is_ok() {
        None
    } else {
        Some(BlockingRegionGuard::new())
    }
}

impl BlockingRegionGuard {
    pub(super) fn new() -> BlockingRegionGuard {
        BlockingRegionGuard { _p: PhantomData }
    }

    /// Blocks the thread on the specified future, returning the value with
    /// which that future completes.
    pub(crate) fn block_on<F>(&mut self, f: F) -> Result<F::Output, AccessError>
    where
        F: Future,
    {
        use crate::park::CachedParkThread;

        let mut park = CachedParkThread::new();
        park.block_on(f)
    }

    /// Blocks the thread on the specified future for **at most** `timeout`.
    ///
    /// If the future completes before `timeout`, the result is returned. If
    /// `timeout` elapses, then `Err` is returned.
    pub(crate) fn block_on_timeout<F>(&mut self, f: F, timeout: Duration) -> Result<F::Output, ()>
    where
        F: Future,
    {
        use crate::park::CachedParkThread;
        use std::task::Context;
        use std::task::Poll::Ready;
        use std::time::Instant;

        let mut park = CachedParkThread::new();
        let waker = park.waker().map_err(|_| ())?;
        let mut cx = Context::from_waker(&waker);

        let mut f = std::pin::pin!(f);
        let when = Instant::now() + timeout;

        loop {
            if let Ready(v) = f.as_mut().poll(&mut cx) {
                return Ok(v);
            }

            let now = Instant::now();

            if now >= when {
                return Err(());
            }

            park.park_timeout(when - now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::thread;

    use tokio::sync::oneshot;

    /// Outside a tokio runtime and off a pool worker, blocking is allowed.
    fn guard() -> BlockingRegionGuard {
        try_enter_blocking_region().expect("blocking is allowed on a test thread")
    }

    #[test]
    fn block_on_waits_for_the_future() {
        let (tx, rx) = oneshot::channel();

        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            tx.send(7u32).unwrap();
        });

        let out = guard().block_on(rx).unwrap();
        assert_eq!(out.unwrap(), 7);
        sender.join().unwrap();
    }

    #[test]
    fn block_on_timeout_returns_the_output_when_it_arrives_in_time() {
        let (tx, rx) = oneshot::channel();

        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            tx.send(7u32).unwrap();
        });

        let out = guard()
            .block_on_timeout(rx, Duration::from_secs(30))
            .expect("should not time out");
        assert_eq!(out.unwrap(), 7);
        sender.join().unwrap();
    }

    #[test]
    fn block_on_timeout_gives_up_on_a_future_that_never_completes() {
        // Holding the sender keeps the receiver pending.
        let (_tx, rx) = oneshot::channel::<u32>();

        assert!(
            guard()
                .block_on_timeout(rx, Duration::from_millis(20))
                .is_err()
        );
    }

    /// A zero timeout still polls once, so an already-ready future is returned
    /// rather than reported as a timeout.
    #[test]
    fn block_on_timeout_polls_once_even_with_no_time_left() {
        let (tx, rx) = oneshot::channel();
        tx.send(7u32).unwrap();

        let out = guard()
            .block_on_timeout(rx, Duration::from_nanos(0))
            .expect("a ready future needs no time");
        assert_eq!(out.unwrap(), 7);
    }
}
