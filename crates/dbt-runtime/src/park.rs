//! Mirrors tokio's `runtime/park.rs`.
//!
//! [`Inner`] is ported verbatim, including the three-state machine and its
//! comments.
//!
//! Blocking goes through [`CachedParkThread`], never through a freshly built
//! [`ParkThread`]: the parker lives in the [`CURRENT_PARKER`] thread-local, so
//! there is one per thread.
//!
//! Omitted from upstream, all for want of callers:
//!
//! - `ParkThread::shutdown` / `Inner::shutdown` (`condvar.notify_all()`), used by
//!   the schedulers this crate does not have.
//! - `ParkThread::park` / `ParkThread::park_timeout`, which upstream exposes for
//!   its drivers. `CachedParkThread` reaches `park_thread.inner` directly, as
//!   upstream's does, so nothing here goes through them.

use std::marker::PhantomData;
use std::pin::pin;
use std::rc::Rc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Wake, Waker};
use std::thread::AccessError;
use std::time::Duration;

/// Parks the current thread, and wakes it from a [`Waker`].
#[derive(Debug)]
pub(crate) struct ParkThread {
    inner: Arc<Inner>,
}

/// Unblocks a thread that was blocked by `ParkThread`.
#[derive(Clone, Debug)]
pub(crate) struct UnparkThread {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    state: AtomicUsize,
    mutex: Mutex<()>,
    condvar: Condvar,
}

const EMPTY: usize = 0;
const PARKED: usize = 1;
const NOTIFIED: usize = 2;

thread_local! {
    static CURRENT_PARKER: ParkThread = ParkThread::new();
}

// ==== impl ParkThread ====

impl ParkThread {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: AtomicUsize::new(EMPTY),
                mutex: Mutex::new(()),
                condvar: Condvar::new(),
            }),
        }
    }

    pub(crate) fn unpark(&self) -> UnparkThread {
        let inner = self.inner.clone();
        UnparkThread { inner }
    }
}

// ==== impl Inner ====

impl Inner {
    fn park(&self) {
        // If we were previously notified then we consume this notification and
        // return quickly.
        if self
            .state
            .compare_exchange(NOTIFIED, EMPTY, SeqCst, SeqCst)
            .is_ok()
        {
            return;
        }

        // Otherwise we need to coordinate going to sleep
        let mut m = self.mutex.lock().unwrap();

        match self.state.compare_exchange(EMPTY, PARKED, SeqCst, SeqCst) {
            Ok(_) => {}
            Err(NOTIFIED) => {
                // We must read here, even though we know it will be `NOTIFIED`.
                // This is because `unpark` may have been called again since we read
                // `NOTIFIED` in the `compare_exchange` above. We must perform an
                // acquire operation that synchronizes with that `unpark` to observe
                // any writes it made before the call to unpark. To do that we must
                // read from the write it made to `state`.
                let old = self.state.swap(EMPTY, SeqCst);
                debug_assert_eq!(old, NOTIFIED, "park state changed unexpectedly");

                return;
            }
            Err(actual) => panic!("inconsistent park state; actual = {actual}"),
        }

        loop {
            m = self.condvar.wait(m).unwrap();

            if self
                .state
                .compare_exchange(NOTIFIED, EMPTY, SeqCst, SeqCst)
                .is_ok()
            {
                // got a notification
                return;
            }

            // spurious wakeup, go back to sleep
        }
    }

    /// Parks the current thread for at most `dur`.
    fn park_timeout(&self, dur: Duration) {
        // Like `park` above we have a fast path for an already-notified thread,
        // and afterwards we start coordinating for a sleep. Return quickly.
        if self
            .state
            .compare_exchange(NOTIFIED, EMPTY, SeqCst, SeqCst)
            .is_ok()
        {
            return;
        }

        if dur == Duration::from_millis(0) {
            return;
        }

        let m = self.mutex.lock().unwrap();

        match self.state.compare_exchange(EMPTY, PARKED, SeqCst, SeqCst) {
            Ok(_) => {}
            Err(NOTIFIED) => {
                // We must read again here, see `park`.
                let old = self.state.swap(EMPTY, SeqCst);
                debug_assert_eq!(old, NOTIFIED, "park state changed unexpectedly");

                return;
            }
            Err(actual) => panic!("inconsistent park_timeout state; actual = {actual}"),
        }

        // Wait with a timeout, and if we spuriously wake up or otherwise wake up
        // from a notification, we just want to unconditionally set the state back to
        // empty, either consuming a notification or un-flagging ourselves as
        // parked.
        let (_m, _result) = self.condvar.wait_timeout(m, dur).unwrap();

        match self.state.swap(EMPTY, SeqCst) {
            NOTIFIED => {} // got a notification, hurray!
            PARKED => {}   // no notification, alas
            n => panic!("inconsistent park_timeout state: {n}"),
        }
    }

    fn unpark(&self) {
        // To ensure the unparked thread will observe any writes we made before
        // this call, we must perform a release operation that `park` can
        // synchronize with. To do that we must write `NOTIFIED` even if `state`
        // is already `NOTIFIED`. That is why this must be a swap rather than a
        // compare-and-swap that returns if it reads `NOTIFIED` on failure.
        match self.state.swap(NOTIFIED, SeqCst) {
            EMPTY => return,    // no one was waiting
            NOTIFIED => return, // already unparked
            PARKED => {}        // gotta go wake someone up
            _ => panic!("inconsistent state in unpark"),
        }

        // There is a period between when the parked thread sets `state` to
        // `PARKED` (or last checked `state` in the case of a spurious wake
        // up) and when it actually waits on `cvar`. If we were to notify
        // during this period it would be ignored and then when the parked
        // thread went to sleep it would never wake up. Fortunately, it has
        // `lock` locked at this stage so we can acquire `lock` to wait until
        // it is ready to receive the notification.
        //
        // Releasing `lock` before the call to `notify_one` means that when the
        // parked thread wakes it doesn't get woken only to have to wait for us
        // to release `lock`.
        drop(self.mutex.lock().unwrap());

        self.condvar.notify_one();
    }
}

impl Default for ParkThread {
    fn default() -> Self {
        Self::new()
    }
}

// ===== impl UnparkThread =====

impl UnparkThread {
    // Upstream also has `unpark(&self)`, called by the I/O and time drivers to
    // wake the runtime directly. There are no drivers here, so the only way this
    // handle is ever used is as a `Waker`.

    /// Upstream hand-rolls a `RawWaker` here (`unparker_to_raw_waker`) over
    /// `Arc<Inner>`. `std::task::Wake` does the same thing, so the vtable is
    /// left to the standard library.
    pub(crate) fn into_waker(self) -> Waker {
        Waker::from(self.inner)
    }
}

impl Wake for Inner {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        Inner::unpark(self);
    }
}

/// Blocks the current thread using a condition variable.
#[derive(Debug)]
pub(crate) struct CachedParkThread {
    _anchor: PhantomData<Rc<()>>,
}

impl CachedParkThread {
    /// Creates a new `ParkThread` handle for the current thread.
    ///
    /// This type cannot be moved to other threads, so it should be created on
    /// the thread that the caller intends to park.
    pub(crate) fn new() -> CachedParkThread {
        CachedParkThread {
            _anchor: PhantomData,
        }
    }

    pub(crate) fn waker(&self) -> Result<Waker, AccessError> {
        self.unpark().map(UnparkThread::into_waker)
    }

    fn unpark(&self) -> Result<UnparkThread, AccessError> {
        self.with_current(ParkThread::unpark)
    }

    pub(crate) fn park(&mut self) {
        self.with_current(|park_thread| park_thread.inner.park())
            .unwrap();
    }

    pub(crate) fn park_timeout(&mut self, duration: Duration) {
        self.with_current(|park_thread| park_thread.inner.park_timeout(duration))
            .unwrap();
    }

    /// Gets a reference to the `ParkThread` handle for this thread.
    ///
    /// Fails once the thread is shutting down and its thread-locals have been
    /// destroyed — which is why [`Self::waker`] returns a `Result` rather than
    /// panicking. Pool shutdown can run from exactly there.
    fn with_current<F, R>(&self, f: F) -> Result<R, AccessError>
    where
        F: FnOnce(&ParkThread) -> R,
    {
        CURRENT_PARKER.try_with(|inner| f(inner))
    }

    /// Runs `f` to completion on the current thread, parking between polls.
    ///
    /// Upstream takes a coop budget around the poll; `coop` is tokio-private.
    pub(crate) fn block_on<F: Future>(&mut self, f: F) -> Result<F::Output, AccessError> {
        use std::task::Context;
        use std::task::Poll::Ready;

        let waker = self.waker()?;
        let mut cx = Context::from_waker(&waker);

        let mut f = pin!(f);

        loop {
            if let Ready(v) = f.as_mut().poll(&mut cx) {
                return Ok(v);
            }

            self.park();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::thread;
    use std::time::Instant;

    use tokio::sync::oneshot;

    #[test]
    fn a_notification_before_the_park_is_not_lost() {
        let park = ParkThread::new();

        park.unpark().into_waker().wake();
        park.inner.park();
    }

    #[test]
    fn park_wakes_on_a_notification_from_another_thread() {
        let park = ParkThread::new();
        let waker = park.unpark().into_waker();

        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            waker.wake();
        });

        park.inner.park();
        sender.join().unwrap();
    }

    /// Exercises the `PARKED` arm of the trailing `swap`: waiting out the full
    /// timeout must leave `state` back at `EMPTY`, so a second park still works.
    /// If it did not, the next `park_timeout` would hit
    /// `panic!("inconsistent park_timeout state")`.
    #[test]
    fn park_timeout_gives_up_and_leaves_the_state_clean() {
        let park = ParkThread::new();

        let start = Instant::now();
        park.inner.park_timeout(Duration::from_millis(30));
        assert!(
            start.elapsed() >= Duration::from_millis(15),
            "returned before the timeout could have elapsed"
        );

        park.unpark().into_waker().wake();
        park.inner.park_timeout(Duration::from_millis(30));
    }

    /// The `NOTIFIED` arm of the same `swap`.
    #[test]
    fn park_timeout_returns_early_on_a_notification() {
        let park = ParkThread::new();
        let waker = park.unpark().into_waker();

        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            waker.wake();
        });

        let start = Instant::now();
        park.inner.park_timeout(Duration::from_secs(30));
        assert!(
            start.elapsed() < Duration::from_secs(25),
            "waited out the timeout instead of taking the notification"
        );

        sender.join().unwrap();
    }

    #[test]
    fn block_on_drives_a_future_woken_from_another_thread() {
        let (tx, rx) = oneshot::channel();

        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            tx.send(7u32).unwrap();
        });

        let out = CachedParkThread::new().block_on(rx).unwrap();
        assert_eq!(out.unwrap(), 7);
        sender.join().unwrap();
    }

    /// The reason for the thread-local: every waker on a thread comes from the
    /// same `Inner`, so a future re-polled by a later `block_on` sees a waker it
    /// already holds and need not re-register.
    #[test]
    fn wakers_from_one_thread_are_all_the_same_waker() {
        let first = CachedParkThread::new().waker().unwrap();
        let second = CachedParkThread::new().waker().unwrap();

        assert!(
            first.will_wake(&second),
            "two `block_on` calls on a thread must share its parker"
        );

        // Whereas a parker built by hand is a different one, which is why
        // nothing outside this module builds one.
        let standalone = ParkThread::new().unpark().into_waker();
        assert!(!first.will_wake(&standalone));
    }

    #[test]
    fn each_thread_gets_its_own_parker() {
        let ours = CachedParkThread::new().waker().unwrap();

        let theirs = thread::spawn(|| CachedParkThread::new().waker().unwrap())
            .join()
            .unwrap();

        assert!(!ours.will_wake(&theirs));
    }
}
