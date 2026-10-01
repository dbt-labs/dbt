//! A file-based [lease](https://en.wikipedia.org/wiki/Lease_(computer_science))
//! for gating concurrent access to a shared local file across independent OS
//! processes.
//!
//! Unlike a plain lock file, a lease has a TTL: if the holder dies without
//! releasing it, the lease simply expires and another process can take over.
//! That liveness property is the whole point of this primitive over a
//! hold-forever advisory lock -- a crashed process can never wedge every
//! other process out of the shared resource forever.
//!
//! Ported from `lease.go` in dbt Labs' Snowflake Go driver fork (merged via
//! "Double-checked lease so the happy path doesn't require a lease", in
//! production use gating a shared local credentials-cache file).

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use rand::Rng;

/// A lease can never be requested for less than this: a shorter TTL just
/// turns the polling loop into a busy spin with no benefit.
pub const MIN_REQUESTED_TTL: Duration = Duration::from_millis(400);

/// A [`LeaseHandler`] can never be configured with a shorter operation
/// timeout than this, for the same reason.
pub const MIN_LEASE_OPERATION_TIMEOUT: Duration = Duration::from_secs(3);

/// Default lease TTL, carried over from gosnowflake's
/// `secure_storage_manager.go` (`_defaultLeaseTTL`), which has used this
/// value in production to gate its shared credentials-cache file.
pub const DEFAULT_LEASE_TTL: Duration = Duration::from_secs(30);

/// Default operation timeout (how long [`LeaseHandler::acquire`] /
/// [`Lease::renew`] retry before giving up), carried over from gosnowflake's
/// `secure_storage_manager.go` (`_defaultLeaseOperationTimeout`).
pub const DEFAULT_LEASE_OPERATION_TIMEOUT: Duration = Duration::from_secs(90);

/// Extra buffer added to every expiry we write and every liveness check we
/// perform, to absorb clock skew and the time it takes other processes to
/// observe our write. Also used as the delay after writing a lease before we
/// read it back, to catch racy concurrent writers.
const GRACE_PERIOD: Duration = Duration::from_millis(12);

const MIN_POLL_INTERVAL: Duration = Duration::from_millis(200);
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(5);

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

/// A lease acquired from a shared [`LeaseHandler`].
///
/// Leases are not `Clone`: a given lease is held by exactly one owner, who is
/// responsible for renewing it (via [`Lease::renew`]) before it expires and
/// releasing it (via [`Lease::release`]) when done.
#[derive(Debug)]
pub struct Lease {
    id: String,
    // `None` means "not currently held" (never acquired, expired, or
    // released) -- treated as a lease that expired at the Unix epoch for
    // comparison purposes, i.e. always "not held".
    expiry: Option<SystemTime>,
    handler: Arc<LeaseHandler>,
    /// Whether relaxed reads are allowed for this lease.
    ///
    /// This is a hint for consumers of the lease, not enforced by the lease
    /// itself. Callers can inspect this field to decide whether to perform
    /// relaxed (non-renewing) reads while holding this lease. Destructive
    /// operations (e.g. writes) should never be performed under a relaxed
    /// read, even when this field is `true`.
    pub relaxed_read_allowed: bool,
}

impl Lease {
    /// The random id identifying this lease in the lease file.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Ensures the lease will remain valid for at least `ttl` longer.
    /// Callers should call this periodically to keep the lease alive (e.g.
    /// every `ttl / 2`).
    pub fn renew(&mut self, ttl: Duration) -> Result<(), LeaseError> {
        let current_expiry = self.expiry.unwrap_or(UNIX_EPOCH);
        match self.handler.renew(&self.id, ttl, current_expiry) {
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
    /// using a TTL that's too short for its workload.
    pub fn release(&mut self) -> Result<(), LeaseError> {
        let current_expiry = self.expiry.take().unwrap_or(UNIX_EPOCH);
        self.handler.release(&self.id, current_expiry)
    }
}

/// Handler for a single file-based lease, shared by every party that wants
/// to acquire it. Cheap to clone (it's just a path and an atomic timeout),
/// and safe to share across threads.
#[derive(Debug)]
pub struct LeaseHandler {
    /// Absolute path to the lease file.
    path: PathBuf,
    /// Directory containing the lease file, where temp files are staged
    /// before being atomically renamed into place.
    dir: PathBuf,
    /// How long `acquire`/`renew` retry before giving up. Stored as
    /// milliseconds in an `AtomicI32` so it can be adjusted at runtime (e.g.
    /// via [`LeaseHandler::set_timeout`]) without requiring `&mut self`.
    timeout_millis: AtomicI32,
}

impl LeaseHandler {
    /// Creates a new handler for the lease file at `path`. `path` need not
    /// exist yet.
    pub fn new(path: impl AsRef<Path>, timeout: Duration) -> io::Result<Arc<Self>> {
        let abspath = std::path::absolute(path.as_ref())?;
        let dir = abspath
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
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
        let id = format!("broken-lease-{}", rand::rng().random::<i64>());
        Lease {
            id,
            expiry: None,
            handler: Arc::clone(self),
            relaxed_read_allowed: false,
        }
    }

    /// Blocks (with jittered polling, to avoid a thundering herd of
    /// competing processes) until the lease is acquired or the handler's
    /// operation timeout elapses.
    pub fn acquire(self: &Arc<Self>, ttl: Duration) -> Result<Lease, LeaseError> {
        let ttl = ttl.max(MIN_REQUESTED_TTL);
        let new_lease_id = random_lease_id();
        let new_expiry = || SystemTime::now() + ttl + GRACE_PERIOD;

        let mut base = Duration::ZERO;
        let mut m = GRACE_PERIOD;
        let deadline = SystemTime::now() + self.timeout();

        while SystemTime::now() < deadline {
            let remaining_half = deadline
                .duration_since(SystemTime::now())
                .unwrap_or(Duration::ZERO)
                / 2;
            let cap = remaining_half.min(MAX_POLL_INTERVAL);
            let read_result = self.read(base, m, cap);
            (base, m) = next_wait(base, m);

            let data = match read_result {
                Ok(data) => data,
                Err(_) => continue,
            };

            // 1. no one holds the lease
            if data.is_empty() {
                let expiry = new_expiry();
                if self.write(&new_lease_id, expiry).is_ok() {
                    return Ok(Lease {
                        id: new_lease_id,
                        expiry: Some(expiry),
                        handler: Arc::clone(self),
                        relaxed_read_allowed: false,
                    });
                }
                continue;
            }

            let now = SystemTime::now();
            // 2. someone else's lease is still valid
            if now < data.expiry {
                m = m.min(data.expiry.duration_since(now).unwrap_or(Duration::ZERO));
                base = m / 2;
                continue;
            }

            // 3. the existing lease has expired -- take it over
            let expiry = new_expiry();
            if self.write(&new_lease_id, expiry).is_ok() {
                return Ok(Lease {
                    id: new_lease_id,
                    expiry: Some(expiry),
                    handler: Arc::clone(self),
                    relaxed_read_allowed: false,
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
        current_expiry: SystemTime,
    ) -> Result<SystemTime, LeaseError> {
        let mut base = Duration::ZERO;
        let mut m = Duration::ZERO;
        let deadline = SystemTime::now() + self.timeout();

        while SystemTime::now() < deadline {
            let remaining_half = deadline
                .duration_since(SystemTime::now())
                .unwrap_or(Duration::ZERO)
                / 2;
            let cap = remaining_half.min(MAX_POLL_INTERVAL);
            wait(base, m, cap);
            (base, m) = next_wait(base, m);

            let now = SystemTime::now();
            // 1. is the lease still held at all?
            let held = now + GRACE_PERIOD < current_expiry;
            if !held {
                return Err(LeaseError::Expired {
                    id: lease_id.to_string(),
                    path: self.path.clone(),
                });
            }
            // 2. does the held lease actually need renewing yet?
            let needs_renewal = now + ttl + GRACE_PERIOD >= current_expiry;
            if !needs_renewal {
                return Ok(current_expiry);
            }
            // Extend by 2x the requested TTL so the next renewal has slack
            // to arrive late without the lease expiring underneath it.
            let new_expiry = now + ttl * 2;
            if self.write(lease_id, new_expiry).is_ok() {
                return Ok(new_expiry);
            }
        }

        Err(LeaseError::RenewTimedOut {
            id: lease_id.to_string(),
            path: self.path.clone(),
        })
    }

    fn release(&self, lease_id: &str, current_expiry: SystemTime) -> Result<(), LeaseError> {
        let now = SystemTime::now();
        if now >= current_expiry {
            return Err(LeaseError::NotHeldOnRelease {
                id: lease_id.to_string(),
                path: self.path.clone(),
            });
        }
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

        let elapsed = SystemTime::now()
            .duration_since(ctime)
            .unwrap_or(Duration::ZERO);
        let remaining_grace = GRACE_PERIOD.saturating_sub(elapsed);
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

fn random_lease_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Sleeps for a duration uniformly sampled from `[base, m]`, capped at
/// `cap`, and returns how long it slept (`Duration::ZERO` if it didn't sleep
/// at all).
///
/// The jitter avoids the [thundering herd
/// problem](https://en.wikipedia.org/wiki/Thundering_herd_problem) when
/// many processes are polling the same lease file.
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

fn to_unix_millis(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    }
}

fn from_unix_millis(ms: i64) -> SystemTime {
    if ms >= 0 {
        UNIX_EPOCH + Duration::from_millis(ms as u64)
    } else {
        UNIX_EPOCH - Duration::from_millis((-ms) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicI32 as StdAtomicI32;
    use std::sync::atomic::Ordering as StdOrdering;

    fn temp_lease_path(name: &str) -> PathBuf {
        static COUNTER: StdAtomicI32 = StdAtomicI32::new(0);
        let n = COUNTER.fetch_add(1, StdOrdering::SeqCst);
        std::env::temp_dir().join(format!(
            "dbt-lease-test-{name}-{}-{n}.lease",
            std::process::id()
        ))
    }

    #[test]
    fn acquire_renew_and_expire() {
        let path = temp_lease_path("acquire_renew_expire");
        let ttl = MIN_REQUESTED_TTL;

        let handler = LeaseHandler::new(&path, DEFAULT_LEASE_OPERATION_TIMEOUT).unwrap();
        let mut lease = handler.acquire(ttl).unwrap();
        lease.renew(ttl).unwrap();
        std::thread::sleep(2 * ttl);
        assert!(lease.renew(ttl).is_err());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn acquire_and_release() {
        let path = temp_lease_path("acquire_release");
        let ttl = MIN_REQUESTED_TTL;

        let handler = LeaseHandler::new(&path, DEFAULT_LEASE_OPERATION_TIMEOUT).unwrap();
        let mut lease = handler.acquire(ttl).unwrap();
        assert!(!lease.relaxed_read_allowed);
        lease.release().unwrap();
        assert!(lease.renew(ttl).is_err());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn renew_broken_lease() {
        let path = temp_lease_path("renew_broken");
        let handler = LeaseHandler::new(&path, DEFAULT_LEASE_OPERATION_TIMEOUT).unwrap();
        let mut lease = handler.broken_lease();

        assert!(
            !lease.relaxed_read_allowed,
            "relaxed reads must be explicitly opted into, not implied by a broken lease"
        );

        let err = lease.renew(MIN_REQUESTED_TTL).unwrap_err();
        assert!(matches!(err, LeaseError::Expired { .. }));
    }

    #[test]
    fn corrupt_lease_file_self_heals() {
        let path = temp_lease_path("corrupt");
        fs::write(&path, b"not a valid lease file").unwrap();

        let handler = LeaseHandler::new(&path, DEFAULT_LEASE_OPERATION_TIMEOUT).unwrap();
        // Acquire should succeed by treating the corrupt file as unheld and
        // overwriting it, rather than erroring out forever.
        let lease = handler.acquire(MIN_REQUESTED_TTL).unwrap();
        assert!(!lease.id().is_empty());

        let _ = fs::remove_file(&path);
    }

    /// Simulates `n` concurrent threads (standing in for separate OS
    /// processes) each acquiring the lease, doing bounded "work" while
    /// periodically renewing, and releasing it -- asserting that at most one
    /// thread ever holds the lease at a time.
    fn simulate_n_threads(handler: &Arc<LeaseHandler>, ttl: Duration, n: usize, work: Duration) {
        use std::sync::atomic::AtomicBool;

        let holding = Arc::new(AtomicBool::new(false));
        std::thread::scope(|scope| {
            for _ in 0..n {
                let handler = Arc::clone(handler);
                let holding = Arc::clone(&holding);
                scope.spawn(move || {
                    let mut lease = handler.acquire(ttl).unwrap();

                    let was_free = holding
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok();
                    assert!(was_free, "acquired lease should be held exclusively");

                    let mut remaining = work;
                    while remaining > Duration::ZERO {
                        let step = (ttl / 2).min(remaining);
                        std::thread::sleep(step);
                        remaining = remaining.saturating_sub(step);
                        if remaining > Duration::ZERO {
                            lease.renew(ttl).expect("failed to renew lease");
                        }
                    }

                    let was_held = holding
                        .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok();
                    assert!(was_held, "more than one lease holder detected");

                    lease.release().unwrap();
                });
            }
        });
    }

    #[test]
    fn single_thread_lease_acquire() {
        let path = temp_lease_path("single");
        let handler = LeaseHandler::new(&path, DEFAULT_LEASE_OPERATION_TIMEOUT).unwrap();
        simulate_n_threads(&handler, MIN_REQUESTED_TTL, 1, Duration::ZERO);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn concurrent_lease_acquire() {
        let path = temp_lease_path("concurrent");
        let handler = LeaseHandler::new(&path, DEFAULT_LEASE_OPERATION_TIMEOUT).unwrap();
        simulate_n_threads(&handler, MIN_REQUESTED_TTL, 8, Duration::ZERO);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn concurrent_lease_acquire_with_fitting_ttl() {
        let path = temp_lease_path("concurrent_fitting");
        // Timeout must comfortably exceed n * work.
        let handler = LeaseHandler::new(&path, Duration::from_secs(30)).unwrap();
        let ttl = Duration::from_millis(500);
        simulate_n_threads(&handler, ttl, 4, ttl);
        let _ = fs::remove_file(&path);
    }
}
