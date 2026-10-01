use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};

/// General-case semaphore implementation.
///
/// Typical Dijkstra Semaphore algorithm over atomics, wait and notify functions.
///
/// The `atomic-wait` crate by Mara Bos is used as it provides the atomic wait and wake
/// functionality that exists in C++20's `std::atomic<T>::wait` and `std::atomic<T>::notify_all`
/// but is not yet available in stable Rust.
struct AtomicSemaphoreBase {
    /// The atomic counter representing the number of available permits.
    ///
    /// `u32` was chosen because that is the atomic that Linux uses for futexes,
    /// and as such, the type chosen by the `atomic-wait` crate.
    a: AtomicU32,
}

impl AtomicSemaphoreBase {
    pub const fn new(count: u32) -> Self {
        let a = AtomicU32::new(count);
        Self { a }
    }

    /// Releases `update` semaphore permits.
    ///
    /// If acquiring all the permits is allowed by the wrapping Semaphore,
    /// `force_wake` must be true because it's possible that the thread trying
    /// to acquire all permits is waiting for the number of permits to go from
    /// `max - 1` to `max`. But when the semaphore only allows acquiring one
    /// permit at a time, wake-ups are only needed when the previous number of
    /// permits was `0`. Because that's the only value that would block a
    /// thread from acquiring a single permit.
    #[inline]
    pub fn release(&self, update: u32, force_wake: bool) {
        let old = self.a.fetch_add(update, Ordering::Release);
        debug_assert!(
            update <= u32::MAX - old,
            "update is greater than the expected value"
        );
        if force_wake || old == 0u32 {
            atomic_wait::wake_all(&self.a);
        }
    }

    // Try to acquire a permit without blocking.
    #[inline]
    fn try_acquire_impl(&self, old: u32, ask: u32) -> bool {
        old >= ask
            && self
                .a
                .compare_exchange_weak(old, old - ask, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
    }

    pub fn acquire(&self) {
        loop {
            // wait until the value is not 0 anymore
            atomic_wait::wait(&self.a, 0);
            let old = self.a.load(Ordering::Relaxed);
            if self.try_acquire_impl(old, 1) {
                break;
            }
        }
    }

    pub fn try_acquire(&self) -> bool {
        let old = self.a.load(Ordering::Acquire);
        self.try_acquire_impl(old, 1)
    }

    pub fn acquire_many(&self, ask: u32) {
        debug_assert!(ask > 0, "cannot acquire zero permits");
        let mut insufficient = (ask - 1).min(self.a.load(Ordering::Relaxed));
        loop {
            // wait until the value is not `insufficient` anymore
            atomic_wait::wait(&self.a, insufficient);
            let old = self.a.load(Ordering::Relaxed);
            if self.try_acquire_impl(old, ask) {
                break;
            }
            insufficient = old;
        }
    }
}

/// Counting semaphore implementation.
pub struct Semaphore {
    /// The maximum number of permits the semaphore can hold.
    ///
    /// NOTE: If release() gets called more than this number, it will not
    /// panic, but will simply increase the count of available permits.
    max: u32,
    base: AtomicSemaphoreBase,
}

impl Semaphore {
    pub const fn new(count: u32) -> Self {
        debug_assert!(count > 0, "Semaphore must allow for at least one permit");
        Self {
            max: count,
            base: AtomicSemaphoreBase::new(count),
        }
    }

    /// Get the number of available permits the semaphore started with.
    pub fn max(&self) -> u32 {
        self.max
    }

    /// Acquire a permit, blocking until one is available.
    #[must_use]
    pub fn acquire(&self) -> PermitGuard<'_, false> {
        self.base.acquire();
        PermitGuard { base: &self.base }
    }

    /// Try to acquire a permit without blocking.
    #[must_use]
    pub fn try_acquire(&self) -> Option<PermitGuard<'_, false>> {
        if self.base.try_acquire() {
            Some(PermitGuard { base: &self.base })
        } else {
            None
        }
    }

    /// Like [Semaphore::acquire], but caller must ensure that
    /// [Semaphore::unguarded_release] is called.
    ///
    /// Failing to do so may lead to deadlocks as acquired permits don't get released.
    pub fn unguarded_acquire(&self) {
        self.base.acquire();
    }

    /// Undo the effect of [Semaphore::unguarded_acquire].
    pub fn unguarded_release(&self) {
        self.base.release(1, false);
    }
}

impl fmt::Debug for Semaphore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Semaphore")
            .field("max", &self.max)
            .field("available", &self.base.a.load(Ordering::Relaxed))
            .finish()
    }
}

/// A counting semaphore that allows a thread to try to acquire all permits at once.
///
/// This has a bit more overhead than [Semaphore], because it has to notify all waiting
/// on every release.
pub struct AcquireAllSemaphore {
    inner: Semaphore,
}

impl AcquireAllSemaphore {
    pub const fn new(count: u32) -> Self {
        Self {
            inner: Semaphore::new(count),
        }
    }

    /// Get the number of available permits the semaphore started with.
    pub fn max(&self) -> u32 {
        self.inner.max()
    }

    /// Acquire a permit, blocking until one is available.
    #[must_use]
    pub fn acquire(&self) -> PermitGuard<'_, true> {
        self.inner.base.acquire();
        PermitGuard {
            base: &self.inner.base,
        }
    }

    /// Try to acquire a permit without blocking.
    #[must_use]
    pub fn try_acquire(&self) -> Option<PermitGuard<'_, true>> {
        if self.inner.base.try_acquire() {
            Some(PermitGuard {
                base: &self.inner.base,
            })
        } else {
            None
        }
    }

    /// Like [Semaphore::acquire], but caller must ensure that
    /// [Semaphore::unguarded_release] is called.
    ///
    /// Failing to do so may lead to deadlocks as acquired permits don't get released.
    pub fn unguarded_acquire(&self) {
        self.inner.base.acquire();
    }

    /// Undo the effect of [Semaphore::unguarded_acquire].
    pub fn unguarded_release(&self) {
        self.inner.base.release(1, true);
    }
    /// Wait for all permits to be available and acquire them all at once.
    ///
    /// ```rust
    /// let semaphore = Semaphore::new(8);
    /// semaphore.acquire_all(); // will block until all 8 permits are available
    /// ```
    #[must_use]
    pub fn acquire_all(&self) -> PermitGuardAll<'_> {
        self.inner.base.acquire_many(self.inner.max);
        PermitGuardAll { semaphore: self }
    }

    /// Like [Semaphore::acquire_all], but caller must ensure that
    /// [Semaphore::unguarded_release_all] is called.
    ///
    /// Failing to do so may lead to deadlocks as acquired permits don't get released.
    pub fn unguarded_acquire_all(&self) {
        self.inner.base.acquire_many(self.inner.max);
    }

    /// Undo the effect of [Semaphore::unguarded_acquire_all].
    pub fn unguarded_release_all(&self) {
        self.inner.base.release(self.inner.max, true);
    }
}

/// A guard that releases the semaphore permit when dropped.
pub struct PermitGuard<'a, const FORCE_WAKE: bool> {
    base: &'a AtomicSemaphoreBase,
}

impl<const FORCE_WAKE: bool> Drop for PermitGuard<'_, FORCE_WAKE> {
    fn drop(&mut self) {
        self.base.release(1, FORCE_WAKE)
    }
}

/// A guard that releases all permits when dropped.
pub struct PermitGuardAll<'a> {
    semaphore: &'a AcquireAllSemaphore,
}

impl Drop for PermitGuardAll<'_> {
    fn drop(&mut self) {
        self.semaphore.unguarded_release_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc::channel;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_semaphore_basic_acquire_release() {
        let semaphore = Semaphore::new(2);
        assert_eq!(semaphore.max(), 2);

        let permit0 = semaphore.acquire();
        let _permit1 = semaphore.acquire();

        drop(permit0);
        let _permit2 = semaphore.acquire();
        assert!(semaphore.try_acquire().is_none());
    }

    #[test]
    fn test_semaphore_release_more_than_initial() {
        let semaphore = Semaphore::new(1);
        // releasing without acquiring first
        semaphore.unguarded_release();

        let permit0 = semaphore.try_acquire();
        assert!(permit0.is_some());
        // The semaphore handles the case where more permits are released than
        // initially available by expanding the count of available permits.
        // Any other strategy would be too complicated and error-prone.
        let permit1 = semaphore.try_acquire();
        assert!(permit1.is_some());
        let permit2 = semaphore.try_acquire();
        assert!(permit2.is_none());
    }

    #[test]
    fn test_semaphore_basic_concurrent_access() {
        let semaphore = Arc::new(Semaphore::new(2));
        let mut handles = vec![];

        for i in 0..3 {
            let sem = semaphore.clone();
            handles.push(thread::spawn(move || {
                let permit = sem.acquire();
                thread::sleep(Duration::from_millis(100));
                drop(permit);
                i
            }));
        }
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results, vec![0, 1, 2]);
    }

    #[test]
    fn test_semaphore_acquire_all() {
        const SCHED_PERIOD: Duration = Duration::from_millis(100);
        let semaphore = Arc::new(AcquireAllSemaphore::new(4));
        let mut handles = vec![];

        let permit = semaphore.acquire_all(); // acquire all permits at once

        let counter = Arc::new(AtomicU32::new(0)); // shared counter
        {
            let counter = Arc::clone(&counter);
            let sem = semaphore.clone();
            // Add one thread that has to wait for all the permits.
            handles.push(thread::spawn(move || {
                // Let the 2 other threads run first and then block on acquiring all permits.
                thread::sleep(SCHED_PERIOD);
                let permits = sem.acquire_all();
                let x = counter.load(Ordering::Acquire);
                drop(permits);
                counter.fetch_add(1, Ordering::Release);
                x
            }));
        }
        // Get only half of the permits, so release makes permits go from 4 to 3, then 3 to 2.
        // Without `force_wake` on release(), the thread waiting for all permits would never
        // be notified.
        for _ in 0..2 {
            let counter = Arc::clone(&counter);
            let sem = semaphore.clone();
            handles.push(thread::spawn(move || {
                let permit = sem.acquire();
                let x = counter.load(Ordering::Acquire);
                thread::sleep(SCHED_PERIOD);
                drop(permit);
                counter.fetch_add(1, Ordering::Release);
                x
            }));
        }
        // all threads remain blocked until all permits are released
        thread::sleep(SCHED_PERIOD);
        drop(permit); // release all permits at once and wake-up all threads
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        // the 3 threads can observe any value from 0 to 2, so the maximum must be 2
        let max_result = results.iter().max().cloned().unwrap_or(0);
        assert!(max_result <= 2);
    }

    #[test]
    fn test_semaphore_wait_signals() {
        let main: Arc<Semaphore> = Arc::new(Semaphore::new(1));
        let child_1 = main.clone();
        let child_2 = main.clone();
        let (tx, rx) = channel();

        let tx = Arc::new(tx);

        let tx_1 = tx.clone();
        let _ = thread::spawn(move || {
            child_1.unguarded_acquire();
            tx_1.send(()).unwrap();
        });

        let _ = thread::spawn(move || {
            child_2.unguarded_acquire();
            tx.send(()).unwrap();
        });

        // if main doesn't release one of the children will get stuck
        main.unguarded_release();
        let _ = rx.recv();
    }

    #[test]
    #[should_panic]
    fn test_semaphore_zero_permits() {
        let _ = Semaphore::new(0);
    }
}
