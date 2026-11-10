//! Port of [`lease.go`] from dbt Labs' gosnowflake fork.
//!
//! [`lease.go`]: https://github.com/dbt-labs/gosnowflake/blob/v2.1.0%2Bdbt/lease.go

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use rand::Rng;

const GRACE_PERIOD: Duration = Duration::from_millis(12);
const MIN_POLL_INTERVAL: Duration = Duration::from_millis(200);
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(5);

pub const MIN_REQUESTED_TTL: Duration = Duration::from_millis(400);
pub const MIN_LEASE_OPERATION_TIMEOUT: Duration = Duration::from_secs(3);
pub const DEFAULT_LEASE_OPERATION_TIMEOUT: Duration = Duration::from_secs(90);

/// Errors returned by lease operations.
#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    /// An I/O error occurred while reading, writing, or renaming the lease
    /// file.
    #[error("lease I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// [`Lease::renew`] was called after the lease had already expired (or
    /// was never successfully acquired, e.g. a [`LeaseHandler::broken_lease`]).
    #[error("lease '{id}' has expired: {path}")]
    Expired { id: String, path: PathBuf },

    /// [`Lease::release`] was called after the lease had already expired.
    /// This can happen when a process fails to renew a lease frequently
    /// enough, or uses a TTL that's too short for its workload.
    #[error("lease '{id}' was not held during release attempt: {path}")]
    NotHeldOnRelease { id: String, path: PathBuf },

    /// [`LeaseHandler::acquire`] gave up after its configured operation
    /// timeout without acquiring the lease.
    #[error("timed out trying to acquire lease after {timeout:?}: {path}")]
    AcquireTimedOut { path: PathBuf, timeout: Duration },

    /// [`Lease::renew`] gave up after the handler's configured operation
    /// timeout without renewing the lease.
    #[error("timed out trying to renew lease '{id}': {path}")]
    RenewTimedOut { id: String, path: PathBuf },

    /// We wrote our lease id to the file but read back someone else's,
    /// meaning a concurrent writer raced us within the grace period. This
    /// should be exceedingly rare and is treated as a retryable failure by
    /// [`LeaseHandler::acquire`] / [`Lease::renew`].
    #[error("racy lease write for '{id}': read back '{actual}' instead")]
    RacyWrite { id: String, actual: String },
}

/// [Leases] are acquired from a shared [LeaseHandler] instance.
///
/// Leases are not [Clone]: a given lease is held by exactly one owner.
///
/// [Leases]: https://en.wikipedia.org/wiki/Lease_(computer_science)
#[derive(Debug)]
pub struct Lease {
    id: String,
    /// Cached lease expiry, on the monotonic clock.
    ///
    /// [Instant] so it's immune to system clock jumps. The lease
    /// file holds the wall-clock expiry that other processes see.
    expiry: Option<Instant>,
    handler: Arc<LeaseHandler>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

impl Lease {
    /// The random lease identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Ensures the lease will remain valid for at least `ttl` longer.
    ///
    /// Callers should call this periodically to keep the lease alive
    /// (e.g. every `ttl / 2`).
    pub fn renew(&mut self, ttl: Duration) -> Result<(), LeaseError> {
        match self.handler.renew(&self.id, ttl, self.expiry) {
            Ok(new_expiry) => {
                self.expiry = Some(new_expiry);
                Ok(())
            }
            Err(e) => {
                self.expiry = None;
                Err(e)
            }
        }
    }

    /// Makes an effort to release the lease, so other processes can acquire
    /// it sooner rather than waiting for it to expire naturally.
    ///
    /// Returns an error if the lease was not held at the time of the call,
    /// which can happen if the caller isn't renewing it often enough, or is
    /// using a TTL that's too short.
    pub fn release(&mut self) -> Result<(), LeaseError> {
        let expiry = self.expiry.take();
        self.handler.release(&self.id, expiry)
    }
}

/// File-based [lease]. Leases are acquired through a shared [LeaseHandler]
/// instance.
///
/// [lease]: https://en.wikipedia.org/wiki/Lease_(computer_science)
#[derive(Debug)]
pub struct LeaseHandler {
    /// Absolute path to the lease file.
    path: PathBuf,
    /// Directory where the lease file is stored.
    ///
    /// Temp files are staged in this directory before being atomically
    /// renamed into place.
    dir: PathBuf,
    /// How long to keep trying to acquire or renew a lease.
    ///
    /// Stored as milliseconds in an `AtomicI32` so it can be adjusted at
    /// runtime (e.g. via [`LeaseHandler::set_timeout`]) without requiring
    /// `&mut self`.
    timeout_millis: AtomicI32,
}

impl LeaseHandler {
    pub fn try_new(path: &Path, timeout: Duration) -> io::Result<Arc<Self>> {
        let abspath = clean_abspath(path)?;
        let dir = abspath.parent().map(Path::to_path_buf).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("lease path has no parent directory: {}", abspath.display()),
            )
        })?;
        let handler = Self {
            path: abspath,
            dir,
            timeout_millis: AtomicI32::new(0),
        };
        handler.set_timeout(timeout);
        Ok(Arc::new(handler))
    }

    /// Absolute path to the lease file this handler manages.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Sets how long `acquire`/`renew` retry before giving up, clamped to
    /// [`MIN_LEASE_OPERATION_TIMEOUT`] and truncated to whole milliseconds.
    pub fn set_timeout(&self, timeout: Duration) {
        let timeout = timeout.max(MIN_LEASE_OPERATION_TIMEOUT);
        let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
        self.timeout_millis.store(millis, Ordering::SeqCst);
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_millis.load(Ordering::SeqCst) as u64)
    }

    /// Returns a lease that was never actually acquired, and so can never be
    /// renewed or released successfully. Useful as a placeholder for callers
    /// that need a `Lease` value before deciding whether they actually need
    /// to acquire one.
    pub fn broken_lease(self: &Arc<Self>) -> Lease {
        let id = format!("broken-lease-{}", rand::rng().random_range(0..=u64::MAX));
        Lease {
            id,
            expiry: None,
            handler: Arc::clone(self),
        }
    }

    /// Blocks (with jittered polling, to avoid a thundering herd of
    /// competing processes) until the lease is acquired or the handler's
    /// operation timeout elapses.
    pub fn acquire(self: &Arc<Self>, ttl: Duration) -> Result<Lease, LeaseError> {
        let ttl = ttl.max(MIN_REQUESTED_TTL);
        let new_lease_id = random_lease_id();
        // Monotonic expiry to cache, wall-clock expiry to write to the file.
        let new_expiry = || {
            let ttl = ttl + GRACE_PERIOD;
            (Instant::now() + ttl, SystemTime::now() + ttl)
        };

        let mut base = Duration::ZERO;
        let mut m = GRACE_PERIOD;
        let deadline = Instant::now() + self.timeout();

        while Instant::now() < deadline {
            let remaining_half = deadline.saturating_duration_since(Instant::now()) / 2;
            let cap = remaining_half.min(MAX_POLL_INTERVAL);
            let read_result = self.read(base, m, cap);
            (base, m) = next_wait(base, m);

            let data = match read_result {
                Ok(data) => data,
                Err(_) => continue,
            };

            // 1. no one holds the lease
            if data.is_empty() {
                let (expiry, wall_expiry) = new_expiry();
                if self.write(&new_lease_id, wall_expiry).is_ok() {
                    return Ok(Lease {
                        id: new_lease_id,
                        expiry: Some(expiry),
                        handler: Arc::clone(self),
                    });
                }
                continue;
            }

            // The file's expiry is wall-clock, as other processes wrote it.
            let now = SystemTime::now();
            // 2. someone else's lease is still valid
            if now < data.expiry {
                m = m.min(data.expiry.duration_since(now).unwrap_or(Duration::ZERO));
                base = m / 2;
                continue;
            }

            // 3. the existing lease has expired -- take it over
            let (expiry, wall_expiry) = new_expiry();
            if self.write(&new_lease_id, wall_expiry).is_ok() {
                return Ok(Lease {
                    id: new_lease_id,
                    expiry: Some(expiry),
                    handler: Arc::clone(self),
                });
            }
        }

        Err(LeaseError::AcquireTimedOut {
            path: self.path.clone(),
            timeout: self.timeout(),
        })
    }

    fn renew(
        &self,
        lease_id: &str,
        ttl: Duration,
        current_expiry: Option<Instant>,
    ) -> Result<Instant, LeaseError> {
        let mut base = Duration::ZERO;
        let mut m = Duration::ZERO;
        let deadline = Instant::now() + self.timeout();

        while Instant::now() < deadline {
            let remaining_half = deadline.saturating_duration_since(Instant::now()) / 2;
            let cap = remaining_half.min(MAX_POLL_INTERVAL);
            wait(base, m, cap);
            (base, m) = next_wait(base, m);

            let now = Instant::now();
            // 1. is the lease still held at all?
            let Some(current_expiry) = current_expiry.filter(|&e| now + GRACE_PERIOD < e) else {
                return Err(LeaseError::Expired {
                    id: lease_id.to_string(),
                    path: self.path.clone(),
                });
            };
            // 2. does the held lease actually need renewing yet?
            let needs_renewal = now + ttl + GRACE_PERIOD >= current_expiry;
            if !needs_renewal {
                return Ok(current_expiry);
            }
            // Extend by 2x the requested TTL so the next renewal has slack
            // to arrive late without the lease expiring underneath it.
            let new_expiry = now + ttl * 2;
            let wall_expiry = SystemTime::now() + ttl * 2;
            if self.write(lease_id, wall_expiry).is_ok() {
                return Ok(new_expiry);
            }
        }

        Err(LeaseError::RenewTimedOut {
            id: lease_id.to_string(),
            path: self.path.clone(),
        })
    }

    fn release(&self, lease_id: &str, current_expiry: Option<Instant>) -> Result<(), LeaseError> {
        let now = Instant::now();
        let Some(current_expiry) = current_expiry.filter(|&e| now < e) else {
            return Err(LeaseError::NotHeldOnRelease {
                id: lease_id.to_string(),
                path: self.path.clone(),
            });
        };
        // Only delete the file if we're confident no one else raced us and
        // re-acquired it in the meantime.
        if now + GRACE_PERIOD < current_expiry {
            let _ = fs::remove_file(&self.path);
        }
        Ok(())
    }

    /// Atomically overwrites the lease file with `lease_id`/`expiry`, then
    /// reads it back (after a short grace period) to confirm no other
    /// process raced us. Written to a temp file first and renamed into
    /// place, which is atomic on both POSIX (`rename(2)`) and Windows
    /// (`ReplaceFile`/`MoveFileEx` under the hood in `tempfile::persist`).
    fn write(&self, lease_id: &str, expiry: SystemTime) -> Result<(), LeaseError> {
        fn to_unix_millis(t: SystemTime) -> i64 {
            match t.duration_since(UNIX_EPOCH) {
                Ok(d) => d.as_millis() as i64,
                Err(e) => -(e.duration().as_millis() as i64),
            }
        }

        let start = Instant::now();
        let ctime = SystemTime::now();
        let mut tmp = tempfile::Builder::new()
            .suffix(".lease")
            .tempfile_in(&self.dir)
            .map_err(|e| self.io_err(e))?;

        // Compensate for the wait before we read the file back below.
        let expiry_with_grace = expiry + GRACE_PERIOD;
        write!(
            tmp,
            "{}\r\n{}\r\n{}\r\n",
            lease_id,
            to_unix_millis(ctime),
            to_unix_millis(expiry_with_grace),
        )
        .map_err(|e| self.io_err(e))?;
        tmp.flush().map_err(|e| self.io_err(e))?;
        tmp.persist(&self.path).map_err(|e| self.io_err(e.error))?;

        let remaining_grace = GRACE_PERIOD.saturating_sub(start.elapsed());
        let data = self.read(remaining_grace, Duration::ZERO, GRACE_PERIOD)?;
        if data.lease_id != lease_id {
            return Err(LeaseError::RacyWrite {
                id: lease_id.to_string(),
                actual: data.lease_id,
            });
        }
        Ok(())
    }

    /// Reads the current lease id and expiry from the lease file, first
    /// calling [`wait`] so callers can conveniently fold in a polling delay.
    /// A missing or corrupt lease file is treated as "no one holds the
    /// lease" (a corrupt file is deleted so it self-heals).
    fn read(&self, base: Duration, m: Duration, cap: Duration) -> Result<LeaseData, LeaseError> {
        wait(base, m, cap);

        match File::open(&self.path) {
            Ok(mut f) => match parse_lease_file(&mut f) {
                Some(data) => Ok(data),
                None => {
                    let _ = fs::remove_file(&self.path);
                    Ok(LeaseData::empty())
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(LeaseData::empty()),
            Err(e) => Err(self.io_err(e)),
        }
    }

    fn io_err(&self, source: io::Error) -> LeaseError {
        LeaseError::Io {
            path: self.path.clone(),
            source,
        }
    }
}

struct LeaseData {
    lease_id: String,
    expiry: SystemTime,
}

impl LeaseData {
    fn empty() -> Self {
        Self {
            lease_id: String::new(),
            expiry: UNIX_EPOCH,
        }
    }

    fn is_empty(&self) -> bool {
        self.lease_id.is_empty()
    }
}

/// A lease file is `"{id}\r\n{ctime_millis}\r\n{expiry_millis}\r\n"`, written
/// as plain text so a corrupt/truncated file is trivially detectable.
/// `ctime` is written for diagnostic purposes but not currently read back.
fn parse_lease_file(f: &mut File) -> Option<LeaseData> {
    fn from_unix_millis(ms: i64) -> SystemTime {
        if ms >= 0 {
            UNIX_EPOCH + Duration::from_millis(ms as u64)
        } else {
            UNIX_EPOCH - Duration::from_millis((-ms) as u64)
        }
    }

    // A well-formed lease file is well under 128 bytes; treat anything
    // larger as corrupt/unexpected rather than reading an unbounded amount
    // of untrusted local file content.
    let mut buffer = Vec::with_capacity(128);
    let mut chunk = [0u8; 128];
    loop {
        match f.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                if buffer.len() >= 128 {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }

    let text = std::str::from_utf8(&buffer).ok()?;
    let mut parts = text.splitn(4, "\r\n");
    let lease_id = parts.next()?;
    let ctime_millis = parts.next()?;
    let expiry_millis = parts.next()?;
    let _ctime_millis: i64 = ctime_millis.parse().ok()?;
    let expiry_millis: i64 = expiry_millis.parse().ok()?;

    Some(LeaseData {
        lease_id: lease_id.to_string(),
        expiry: from_unix_millis(expiry_millis),
    })
}

/// Lexically normalizes a path like Go's `filepath.Abs` and `Clean`:
/// - drops `.` components
/// - resolves each `..` against the preceding component (`..` at the root stays
///   at the root).
/// - Symlinks aren't resolved.
fn clean_abspath(path: &Path) -> io::Result<PathBuf> {
    let mut cleaned = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                cleaned.pop();
            }
            other => cleaned.push(other),
        }
    }
    Ok(cleaned)
}

fn random_lease_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("os entropy");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Sleeps for a duration uniformly sampled from `[base, m]`, capped at
/// `cap`, and returns how long it slept (`Duration::ZERO` if it didn't sleep
/// at all).
///
/// The jitter avoids the [thundering herd problem] when many processes are
/// polling the same lease file.
///
/// [thundering herd problem]: https://en.wikipedia.org/wiki/Thundering_herd_problem
fn wait(base: Duration, m: Duration, cap: Duration) -> Duration {
    let mut d = base;
    if m > base {
        let range_nanos = (m - base).as_nanos().min(u64::MAX as u128) as u64;
        d += Duration::from_nanos(rand::rng().random_range(0..=range_nanos));
    }
    if d.is_zero() || cap.is_zero() {
        return Duration::ZERO;
    }
    let d = d.min(cap);
    std::thread::sleep(d);
    d
}

/// Doubles the polling window for the next `wait` call, up to a floor of
/// [`MIN_POLL_INTERVAL`], and re-centers `base` at its midpoint.
fn next_wait(base: Duration, m: Duration) -> (Duration, Duration) {
    let _ = base;
    let m = if m < MIN_POLL_INTERVAL {
        MIN_POLL_INTERVAL
    } else {
        m.saturating_mul(2)
    };
    (m / 2, m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use tempfile::TempDir;

    /// A holder renewing every `ttl / 2` can be stalled for about a second before
    /// it loses the lease.
    const TINY_TTL: Duration = Duration::from_secs(2);
    /// A TTL no scheduling stall can outlast.
    const LONG_TTL: Duration = Duration::from_secs(60);

    /// Each test gets its own directory, so lease files never collide and are
    /// cleaned up even when the test fails.
    fn new_handler(timeout: Duration) -> (TempDir, Arc<LeaseHandler>) {
        let dir = tempfile::tempdir().unwrap();
        let handler = LeaseHandler::try_new(&dir.path().join("test.lease"), timeout).unwrap();
        (dir, handler)
    }

    fn file_lease_id(handler: &LeaseHandler) -> Option<String> {
        let data = handler
            .read(Duration::ZERO, Duration::ZERO, Duration::ZERO)
            .unwrap();
        (!data.is_empty()).then_some(data.lease_id)
    }

    #[cfg(unix)]
    #[test]
    fn clean_path_normalizes_like_go() {
        for (input, expected) in [
            ("/a/b/c.lease", "/a/b/c.lease"),
            ("/a/./b/../c.lease", "/a/c.lease"),
            ("/a//b/./c.lease", "/a/b/c.lease"),
            ("/a/b/../../c.lease", "/c.lease"),
            ("/../../c.lease", "/c.lease"),
        ] {
            assert_eq!(
                clean_abspath(Path::new(input)).unwrap(),
                Path::new(expected),
                "{input}"
            );
        }
    }

    #[test]
    fn try_new_cleans_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist").join("..").join("x.lease");
        let handler = LeaseHandler::try_new(&path, DEFAULT_LEASE_OPERATION_TIMEOUT).unwrap();
        assert_eq!(
            handler.path(),
            clean_abspath(dir.path()).unwrap().join("x.lease")
        );
    }

    #[test]
    fn clean_abspath_resolves_relative_paths_against_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let cleaned = clean_abspath(Path::new("a/./b/../c.lease")).unwrap();
        assert_eq!(
            cleaned,
            clean_abspath(&cwd).unwrap().join("a").join("c.lease")
        );
    }

    #[test]
    fn lease_acquire_renew_and_expire() {
        let (_dir, handler) = new_handler(DEFAULT_LEASE_OPERATION_TIMEOUT);
        let ttl = TINY_TTL;
        let mut lease = handler.acquire(ttl).unwrap();
        // successful renew()
        lease.renew(ttl).unwrap();
        std::thread::sleep(2 * ttl);
        // can't renew() after expiration
        let err = lease.renew(ttl).unwrap_err();
        assert!(matches!(err, LeaseError::Expired { .. }), "{err}");
    }

    #[test]
    fn lease_acquire_and_release() {
        let (_dir, handler) = new_handler(DEFAULT_LEASE_OPERATION_TIMEOUT);
        let ttl = TINY_TTL;
        let mut lease = handler.acquire(ttl).unwrap();
        lease.release().unwrap();
        assert_eq!(file_lease_id(&handler), None);
        // can't renew() after release()
        let err = lease.renew(ttl).unwrap_err();
        assert!(matches!(err, LeaseError::Expired { .. }), "{err}");
    }

    #[test]
    fn renew_broken_lease() {
        let (_dir, handler) = new_handler(DEFAULT_LEASE_OPERATION_TIMEOUT);
        let mut lease = handler.broken_lease();
        // can't renew() broken lease
        let err = lease.renew(MIN_REQUESTED_TTL).unwrap_err();
        assert!(matches!(err, LeaseError::Expired { .. }), "{err}");
    }

    /// Simulates `n` threads (standing in for processes) each acquiring the
    /// lease, working for a duration in `[d, m]` while renewing it every
    /// `ttl / 2`, and releasing it. Asserts that at most one thread ever
    /// holds the lease at a time.
    ///
    /// Durations accumulate: each thread's duration is drawn from
    /// `[previous duration, m]`.
    fn simulate_n_threads(
        handler: &Arc<LeaseHandler>,
        ttl: Duration,
        n: usize,
        mut d: Duration,
        m: Duration,
        explicit_release: bool,
    ) {
        let holding = AtomicBool::new(false);
        let mut useful_work = Duration::ZERO;
        let start = Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..n {
                if m > d {
                    d = rand::rng().random_range(d..=m);
                }
                useful_work += d;
                let work = d;
                let holding = &holding;
                scope.spawn(move || {
                    let mut lease = handler.acquire(ttl).unwrap();
                    let was_free = holding
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok();
                    assert!(was_free, "acquired lease should be exclusive");

                    let mut remaining = work;
                    while !remaining.is_zero() {
                        let step = (ttl / 2).min(remaining);
                        std::thread::sleep(step);
                        remaining -= step;
                        if !remaining.is_zero() {
                            lease.renew(ttl).expect("failed to renew lease");
                        }
                    }

                    let was_held = holding
                        .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok();
                    assert!(was_held, "more than one lease holder detected");

                    if explicit_release {
                        lease.release().unwrap();
                    }
                });
            }
        });
        let wall_clock = start.elapsed();
        println!(
            "Concurrent threads simulation: wall_clock_time={wall_clock:?}, \
             useful_work={useful_work:?} ({:.2}%)",
            useful_work.as_secs_f64() / wall_clock.as_secs_f64() * 100.0
        );
    }

    #[test]
    fn single_thread_lease_acquire() {
        let (_dir, handler) = new_handler(DEFAULT_LEASE_OPERATION_TIMEOUT);
        simulate_n_threads(&handler, TINY_TTL, 1, Duration::ZERO, Duration::ZERO, true);
    }

    #[test]
    fn concurrent_lease_acquire() {
        let (_dir, handler) = new_handler(DEFAULT_LEASE_OPERATION_TIMEOUT);
        let ttl = TINY_TTL;
        simulate_n_threads(&handler, ttl, 16, Duration::ZERO, Duration::ZERO, true);
    }

    #[test]
    fn concurrent_lease_acquire_with_random_duration_threads() {
        let (_dir, handler) = new_handler(DEFAULT_LEASE_OPERATION_TIMEOUT);
        let ttl = TINY_TTL;
        let d = Duration::ZERO;
        let m = 2 * TINY_TTL; // max duration of the threads
        simulate_n_threads(&handler, ttl, 4, d, m, true);
    }

    #[test]
    fn concurrent_lease_acquire_with_fitting_ttl() {
        // must be larger than n * d (#threads * duration)
        let (_dir, handler) = new_handler(Duration::from_secs(10 * 60));
        let ttl = Duration::from_secs(4); // well-sized TTL
        let d = Duration::from_secs(4); // TTL equals the duration of the threads
        simulate_n_threads(&handler, ttl, 4, d, d, true);
    }

    // The tests below have no counterpart in lease_test.go.

    /// Without the release on drop, each waiter would sit out the previous
    /// holder's whole [`LONG_TTL`] and the acquires would time out.
    #[test]
    fn concurrent_lease_acquire_released_on_drop() {
        let (_dir, handler) = new_handler(DEFAULT_LEASE_OPERATION_TIMEOUT);
        let ttl = LONG_TTL;
        simulate_n_threads(&handler, ttl, 16, Duration::ZERO, Duration::ZERO, false);
    }

    #[test]
    fn acquire_times_out_while_held() {
        let (_dir, holder) = new_handler(DEFAULT_LEASE_OPERATION_TIMEOUT);
        let mut lease = holder.acquire(LONG_TTL).unwrap();

        let contender = LeaseHandler::try_new(holder.path(), MIN_LEASE_OPERATION_TIMEOUT).unwrap();
        let err = contender.acquire(LONG_TTL).unwrap_err();
        assert!(matches!(err, LeaseError::AcquireTimedOut { .. }), "{err}");

        lease.release().unwrap();
    }

    #[test]
    fn expired_lease_is_taken_over() {
        let (_dir, handler) = new_handler(DEFAULT_LEASE_OPERATION_TIMEOUT);
        let mut stale = handler.acquire(MIN_REQUESTED_TTL).unwrap();
        std::thread::sleep(2 * MIN_REQUESTED_TTL);

        let mut fresh = handler.acquire(LONG_TTL).unwrap();
        assert_ne!(stale.id(), fresh.id());

        // The stale holder can neither renew nor release, and must not
        // delete the new holder's lease file trying to.
        assert!(stale.renew(MIN_REQUESTED_TTL).is_err());
        assert!(stale.release().is_err());
        assert_eq!(file_lease_id(&handler).as_deref(), Some(fresh.id()));

        fresh.release().unwrap();
    }

    #[test]
    fn corrupt_lease_file_self_heals() {
        let (_dir, handler) = new_handler(DEFAULT_LEASE_OPERATION_TIMEOUT);
        fs::write(handler.path(), b"not a valid lease file").unwrap();

        // treated as unheld and overwritten, rather than erroring forever
        let lease = handler.acquire(LONG_TTL).unwrap();
        assert_eq!(file_lease_id(&handler).as_deref(), Some(lease.id()));
    }
}
