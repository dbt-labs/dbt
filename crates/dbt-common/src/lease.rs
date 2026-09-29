use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use uuid::Uuid;

use crate::path::DbtPath;
use crate::stdfs::{create_dir_all, read_to_string, remove_file};
use crate::time::{current_time_micros, time_micros};
use crate::tracing::dbt_emit::emit_info_log_message;
use crate::{ErrorCode, FsError, FsResult};

use std::path::Path;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

struct Lease {
    id: String,
    expiry: u128,
    handler: Arc<LeaseHandler>,
}

/// Holds the details of where the lease file is located and the timeout
/// to observe when attempting to acquire the lease. A value of Duration::ZERO
/// will result in no timeout.
struct LeaseHandler {
    path: PathBuf,
    dir_display: String,
    timeout: Duration,
}

struct LeaseData {
    lease_id: String,
    expiry: u128,
}

impl LeaseData {
    fn is_valid(&self) -> bool {
        self.expiry > current_time_micros()
    }
}

/// Acquires a lease on the given directory and spawns a background renewal task.
/// Returns a [`LeaseGuard`] that releases the lease when dropped.
/// Uses default values for ttl and the renewal interval.
pub async fn acquire_lease(project_dir: &Path, relative_dir: &Path) -> FsResult<LeaseGuard> {
    // default ttl and renewal
    let lease_ttl: Duration = Duration::from_millis(2000);
    let renewal_interval: Duration = Duration::from_millis(500);
    acquire_lease_with_renewal(project_dir, relative_dir, lease_ttl, renewal_interval).await
}

/// Returns the lease file path for a given directory name:
/// `~/.dbt/leases/.dbt_lease_{hash of <project_root>/<dir_name>/}`.
fn lease_file_path(project_root: &Path, dir_name: &Path) -> PathBuf {
    let dir_path = DbtPath::from(project_root.join(dir_name));

    let home_dir = DbtPath::from(dirs::home_dir().expect("Unable to get home directory"));
    debug_assert!(home_dir.is_absolute());

    let dbt_leases_dir = home_dir.join(".dbt/leases");
    create_dir_all(&dbt_leases_dir).ok();

    // Hash the directory that we are holding a lease on.
    let mut hasher = Sha256::new();
    hasher.update(dir_path.to_string_lossy().as_bytes());
    let hash = hasher.finalize();
    let hex = format!("{:x}", hash);
    let hash_string = hex[..10.min(hex.len())].to_string();

    dbt_leases_dir
        .join(format!(".dbt_lease_{hash_string}"))
        .to_path_buf()
}

impl LeaseHandler {
    fn new(path: PathBuf, dir_display: String, timeout: Duration) -> Self {
        LeaseHandler {
            path,
            dir_display,
            timeout,
        }
    }

    fn write(&self, lease_id: &str, expiry: u128) -> FsResult<()> {
        if let Some(parent) = self.path.parent() {
            create_dir_all(parent)?;
        }
        let contents = format!("{}\n{}\n", lease_id, expiry);
        crate::stdfs::write(&self.path, contents)
    }

    fn read(&self) -> FsResult<LeaseData> {
        let lease_contents = match read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(e) => {
                return Err(Box::new(FsError::new(
                    ErrorCode::FileIoError,
                    format!("Lease file does not exist. {e}"),
                )));
            }
        };
        let lines: Vec<&str> = lease_contents.lines().collect();

        if lines.len() < 2 {
            return Err(Box::new(FsError::new(
                ErrorCode::FileIoError,
                "Lease file is corrupted.".to_string(),
            )));
        }

        let lease_id = lines[0].to_string();
        let expiry_ms: u128 = lines[1].trim().parse().unwrap_or_default();

        Ok(LeaseData {
            lease_id,
            expiry: expiry_ms,
        })
    }

    /// Attempt to acquire the lease on the file.
    /// The timeout on the LeaseHandler will be observed.
    /// If timeout == Duration::ZERO, the process will wait until the lease can be acquired
    /// Returns the expiration of the lease
    async fn acquire(self: &Arc<Self>, lease_id: String, ttl: Duration) -> FsResult<u128> {
        let start = Instant::now();

        let mut logged_wait = false;
        loop {
            if self.timeout > Duration::ZERO && start.elapsed() >= self.timeout {
                break;
            }

            match &self.read() {
                // check if lease is valid
                // if valid wait to acquire
                Ok(lease_data) if lease_data.is_valid() => {
                    if !logged_wait {
                        emit_info_log_message(format!(
                            "Waiting for lease on '{}' directory",
                            self.dir_display
                        ));
                        logged_wait = true;
                    }

                    sleep(Duration::from_millis(100)).await;
                    continue;
                }
                _ => (),
            }
            // acquire the lease on all cases where current lease is invalid
            let expiry_ms = time_micros(SystemTime::now() + ttl);
            self.write(&lease_id, expiry_ms)?;
            return Ok(expiry_ms);
        }

        Err(Box::new(FsError::new(
            ErrorCode::FileIoError,
            "Unable to acquire lease".to_string(),
        )))
    }

    // If the lease is still valid for the lease_id, renew the lease with a new expiration value
    // If lease invalid or does not exist return an error
    fn renew(&self, lease_id: &str, ttl: Duration) -> FsResult<u128> {
        let expiry = time_micros(SystemTime::now() + ttl);
        match &self.read() {
            // Another process holds a valid lease — cannot renew or re-acquire
            Ok(lease_data) if lease_data.is_valid() && lease_data.lease_id != lease_id => {
                Err(Box::new(FsError::new(
                    ErrorCode::IoError,
                    "Lease unable to be renewed.".to_string(),
                )))
            }
            // Lease is ours (valid + matching ID), or expired/missing — (re-)acquire
            _ => {
                self.write(lease_id, expiry)?;
                Ok(expiry)
            }
        }
    }
}

impl Lease {
    pub fn new(path: PathBuf, dir_display: String, timeout: Duration) -> Lease {
        let handler = LeaseHandler::new(path, dir_display, timeout);

        Lease {
            id: Uuid::new_v4().to_string(),
            expiry: 0,
            handler: handler.into(),
        }
    }

    pub async fn acquire(&mut self, ttl: Duration) -> FsResult<()> {
        self.expiry = self.handler.acquire(self.id.clone(), ttl).await?;
        Ok(())
    }

    pub fn renew(&mut self, ttl: Duration) -> FsResult<()> {
        self.expiry = self.handler.renew(&self.id, ttl)?;
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(lease_data) = self.handler.read() {
            if lease_data.is_valid() && self.id == lease_data.lease_id {
                let _ = remove_file(&self.handler.path);
            }
        }
    }
}

pub struct LeaseGuard {
    handle: Option<JoinHandle<()>>,
}

impl LeaseGuard {
    /// Not required to call.
    /// While [LeaseGuard] implements [Drop],
    /// this releases the lease explicitly and has the ability to await until the lease has been dropped.
    pub async fn release(mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

fn spawn_lease_renewal(mut lease: Lease, ttl: Duration, interval: Duration) -> LeaseGuard {
    let handle = tokio::spawn(async move {
        loop {
            sleep(interval).await;
            lease.renew(ttl).ok();
        }
    });
    LeaseGuard {
        handle: Some(handle),
    }
}

/// Acquires a lease on the given directory and spawns a background renewal task.
/// Returns a [`LeaseGuard`] that releases the lease when dropped.
async fn acquire_lease_with_renewal(
    project_dir: &Path,
    relative_dir: &Path,
    ttl: Duration,
    renewal_interval: Duration,
) -> FsResult<LeaseGuard> {
    let lease_path = lease_file_path(project_dir, relative_dir);
    let mut lease = Lease::new(
        lease_path,
        relative_dir.display().to_string(),
        Duration::ZERO,
    );
    lease.acquire(ttl).await?;
    Ok(spawn_lease_renewal(lease, ttl, renewal_interval))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Returns a unique lease file path in the system temp dir for test isolation.
    fn test_lease_path() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("lease_test_{pid}_{id}.lock"))
    }

    /// Cleanup helper — ignores errors if the file doesn't exist.
    fn cleanup(path: &PathBuf) {
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_lease_data_is_valid_not_expired() {
        let future_ms = current_time_micros() + 60_000; // 60s from now
        let data = LeaseData {
            lease_id: "test".to_string(),
            expiry: future_ms,
        };
        assert!(data.is_valid());
    }

    #[test]
    fn test_lease_data_is_valid_expired() {
        let past_ms = current_time_micros().saturating_sub(1_000); // 1s ago
        let data = LeaseData {
            lease_id: "test".to_string(),
            expiry: past_ms,
        };
        assert!(!data.is_valid());
    }

    #[test]
    fn test_write_and_read_round_trip() {
        let path = test_lease_path();
        let handler = LeaseHandler::new(path.clone(), "test".to_string(), Duration::ZERO);

        let lease_id = "round-trip-id".to_string();
        let expiry: u128 = current_time_micros() + 30_000;

        handler.write(&lease_id, expiry).unwrap();
        let data = handler.read().expect("read should succeed after write");

        assert_eq!(data.lease_id, lease_id);
        assert_eq!(data.expiry, expiry);

        cleanup(&path);
    }

    #[dbt_runtime::test]
    async fn test_acquire_no_existing_lease() {
        let lease_id = "acquire-no-existing-lease".to_string();
        let path = test_lease_path();
        let handler = Arc::new(LeaseHandler::new(
            path.clone(),
            "test".to_string(),
            Duration::from_secs(5),
        ));

        let lease = handler.acquire(lease_id, Duration::from_secs(10));
        assert!(
            lease.await.is_ok(),
            "acquire should succeed when no lease file exists"
        );

        cleanup(&path);
    }

    #[dbt_runtime::test]
    async fn test_acquire_expired_lease() {
        let lease_id = "acquire-expired-lease".to_string();
        let path = test_lease_path();
        let handler = Arc::new(LeaseHandler::new(
            path.clone(),
            "test".to_string(),
            Duration::from_secs(5),
        ));

        // Write an already-expired lease
        let past_expiry = current_time_micros().saturating_sub(1_000);
        handler.write("old-owner", past_expiry).unwrap();

        let lease = handler.acquire(lease_id, Duration::from_secs(10));
        assert!(
            lease.await.is_ok(),
            "acquire should succeed when existing lease is expired"
        );

        cleanup(&path);
    }

    #[dbt_runtime::test]
    async fn test_acquire_timeout() {
        let lease_id = "acquire-with-timeout".to_string();
        let path = test_lease_path();
        let handler = Arc::new(LeaseHandler::new(
            path.clone(),
            "test".to_string(),
            Duration::from_millis(300),
        ));

        // Write a lease that won't expire for a long time
        let future_expiry = current_time_micros() + 600_000;
        handler.write("blocker", future_expiry).unwrap();

        let result = handler.acquire(lease_id, Duration::from_secs(10));
        assert!(
            result.await.is_err(),
            "acquire should fail when active lease exists and timeout elapses"
        );

        cleanup(&path);
    }

    #[dbt_runtime::test]
    async fn test_renew_valid_lease() {
        let lease_id = "renew-valid-lease".to_string();
        let path = test_lease_path();
        let handler = Arc::new(LeaseHandler::new(
            path.clone(),
            "test".to_string(),
            Duration::from_secs(5),
        ));

        let lease_expiry = handler
            .acquire(lease_id.clone(), Duration::from_secs(2))
            .await
            .expect("acquire should succeed");
        let original_expiry = lease_expiry;

        // Small sleep so renewed expiry is clearly different
        sleep(Duration::from_millis(10)).await;

        let result = handler.renew(&lease_id, Duration::from_secs(5));
        assert!(result.is_ok(), "renew should succeed on a valid lease");
        assert!(
            result.ok().unwrap() > original_expiry,
            "expiry should be extended after renew"
        );

        cleanup(&path);
    }

    #[dbt_runtime::test]
    async fn test_renew_expired_lease() {
        let lease_id = "renew-expired-lease".to_string();
        let path = test_lease_path();
        let handler = Arc::new(LeaseHandler::new(
            path.clone(),
            "test".to_string(),
            Duration::from_secs(5),
        ));

        // Acquire with a very short TTL, then wait for it to expire
        let _lease = handler
            .acquire(lease_id.clone(), Duration::from_millis(1))
            .await
            .expect("acquire should succeed");
        sleep(Duration::from_millis(50)).await;

        let result = handler.renew(&lease_id, Duration::from_secs(5));
        assert!(
            result.is_ok(),
            "renew should re-acquire when the lease has expired"
        );

        cleanup(&path);
    }
}
