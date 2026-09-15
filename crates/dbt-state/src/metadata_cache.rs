//! Short-lived warehouse metadata cache for dbt State service request assembly.
//!
//! The cache is keyed by rendered relation name and stores only metadata that is
//! expensive or redundant to fetch while constructing service payloads. Failed
//! metadata lookups are deliberately not cached so callers can fail open and
//! retry later in the same invocation.

use std::fmt::Display;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Debug, Default)]
pub struct RunCacheMetadataCache {
    ttl: Option<Duration>,
    relation_exists: DashMap<String, TimedEntry<bool>>,
    last_modified_epochs: DashMap<String, TimedEntry<Option<i64>>>,
    lookup_errors: DashMap<String, TimedEntry<String>>,
    in_flight: DashMap<String, Arc<FlightState>>,
    generation: AtomicU64,
    write_lock: StdMutex<()>,
}

#[derive(Debug, Default)]
struct FlightState {
    lock: Arc<Mutex<()>>,
    version: AtomicU64,
    users: AtomicUsize,
}

#[derive(Clone, Debug)]
struct TimedEntry<T> {
    value: T,
    fetched_at: Instant,
}

impl<T> TimedEntry<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            fetched_at: Instant::now(),
        }
    }

    fn is_expired(&self, ttl: Option<Duration>) -> bool {
        ttl.is_some_and(|ttl| self.fetched_at.elapsed() > ttl)
    }
}

impl RunCacheMetadataCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            ttl: Some(ttl),
            ..Self::default()
        }
    }

    pub fn with_ttl_seconds(ttl_seconds: i64) -> Self {
        if ttl_seconds <= 0 {
            Self::new()
        } else {
            Self::with_ttl(Duration::from_secs(ttl_seconds as u64))
        }
    }

    pub fn relation_exists(&self, relation: &str) -> Option<bool> {
        get_cached(&self.relation_exists, relation, self.ttl)
    }

    pub fn last_modified_epoch(&self, relation: &str) -> Option<Option<i64>> {
        get_cached(&self.last_modified_epochs, relation, self.ttl)
    }

    pub fn lookup_error(&self, lookup: &str) -> Option<String> {
        get_cached(&self.lookup_errors, lookup, self.ttl)
    }

    pub fn insert_relation_exists(&self, relation: impl Into<String>, exists: bool) {
        self.insert_value(
            &self.relation_exists,
            "relation_exists",
            relation.into(),
            exists,
        );
    }

    pub fn insert_last_modified_epoch(&self, relation: impl Into<String>, epoch: Option<i64>) {
        self.insert_value(
            &self.last_modified_epochs,
            "last_modified_epoch",
            relation.into(),
            epoch,
        );
    }

    pub fn remove_last_modified_epoch(&self, relation: &str) {
        let _guard = lock_write(&self.write_lock);
        self.bump_version(&lookup_error_key("last_modified_epoch", relation));
        self.last_modified_epochs.remove(relation);
    }

    pub fn insert_lookup_error(&self, lookup: impl Into<String>, error: impl Into<String>) {
        let lookup = lookup.into();
        let _guard = lock_write(&self.write_lock);
        self.bump_version(&lookup);
        self.lookup_errors
            .insert(lookup, TimedEntry::new(error.into()));
    }

    pub fn remove_lookup_error(&self, lookup: &str) {
        let _guard = lock_write(&self.write_lock);
        self.bump_version(lookup);
        self.lookup_errors.remove(lookup);
    }

    /// Drop a cached `relation_exists` lookup failure. Callers use this after a
    /// cancelled lookup, so cancellation isn't mistaken for a genuine warehouse
    /// failure and cached for other in-flight lookups of the same relation.
    pub fn remove_relation_exists_error(&self, relation: &str) {
        self.remove_lookup_error(&lookup_error_key("relation_exists", relation));
    }

    /// Drop a cached `last_modified_epoch` lookup failure. See
    /// `remove_relation_exists_error`.
    pub fn remove_last_modified_epoch_error(&self, relation: &str) {
        self.remove_lookup_error(&lookup_error_key("last_modified_epoch", relation));
    }

    pub fn invalidate_relation_metadata(&self, relation: &str) {
        let _guard = lock_write(&self.write_lock);
        for kind in ["relation_exists", "last_modified_epoch"] {
            self.bump_version(&lookup_error_key(kind, relation));
        }
        self.relation_exists.remove(relation);
        self.last_modified_epochs.remove(relation);
        self.lookup_errors
            .remove(&lookup_error_key("relation_exists", relation));
        self.lookup_errors
            .remove(&lookup_error_key("last_modified_epoch", relation));
    }

    pub async fn get_or_try_insert_relation_exists<E, F, Fut>(
        &self,
        relation: &str,
        fetch: F,
    ) -> Result<bool, E>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<bool, E>>,
        E: Display,
    {
        get_or_try_insert(
            self,
            &self.relation_exists,
            &self.lookup_errors,
            self.ttl,
            "relation_exists",
            relation,
            fetch,
        )
        .await
    }

    pub fn begin_last_modified_prefetch(&self, relation: &str) -> MetadataPrefetchGuard<'_> {
        MetadataPrefetchGuard::new(self, lookup_error_key("last_modified_epoch", relation))
    }

    pub async fn get_or_try_insert_last_modified_epoch<E, F, Fut>(
        &self,
        relation: &str,
        fetch: F,
    ) -> Result<Option<i64>, E>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<Option<i64>, E>>,
        E: Display,
    {
        get_or_try_insert(
            self,
            &self.last_modified_epochs,
            &self.lookup_errors,
            self.ttl,
            "last_modified_epoch",
            relation,
            fetch,
        )
        .await
    }

    fn bump_version(&self, key: &str) {
        if let Some(state) = self.in_flight.get(key) {
            state.version.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn insert_value<T>(
        &self,
        map: &DashMap<String, TimedEntry<T>>,
        kind: &str,
        key: String,
        value: T,
    ) {
        let _guard = lock_write(&self.write_lock);
        let lookup = lookup_error_key(kind, &key);
        self.bump_version(&lookup);
        commit_value(map, &self.lookup_errors, &lookup, key, value);
    }

    pub fn clear(&self) {
        let _guard = lock_write(&self.write_lock);
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.relation_exists.clear();
        self.last_modified_epochs.clear();
        self.lookup_errors.clear();
    }
}

async fn get_or_try_insert<T, E, F, Fut>(
    cache: &RunCacheMetadataCache,
    map: &DashMap<String, TimedEntry<T>>,
    errors: &DashMap<String, TimedEntry<String>>,
    ttl: Option<Duration>,
    lookup_kind: &str,
    key: &str,
    fetch: F,
) -> Result<T, E>
where
    T: Clone,
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: Display,
{
    if let Some(value) = get_cached(map, key, ttl) {
        return Ok(value);
    }

    let lookup_key = lookup_error_key(lookup_kind, key);
    let flight_guard = MetadataPrefetchGuard::new(cache, lookup_key);
    let state = &flight_guard.state;

    loop {
        let _lock_guard = state.lock.lock().await;

        if let Some(value) = get_cached(map, key, ttl) {
            return Ok(value);
        }

        let version_at_fetch = state.version.load(Ordering::Relaxed);
        let generation_at_fetch = cache.generation.load(Ordering::Relaxed);
        let result = fetch().await;

        // Keep the check and insertion under the same lock as invalidation and
        // clear. Otherwise an invalidation can land between the check and the
        // insert, allowing a stale result to repopulate the cache.
        let _write_guard = lock_write(&cache.write_lock);
        let unchanged = cache.generation.load(Ordering::Relaxed) == generation_at_fetch
            && state.version.load(Ordering::Relaxed) == version_at_fetch;
        if !unchanged {
            // Invalidation or clear raced this fetch. Do not expose its stale
            // result; the reusable fetch is retried under the new generation.
            continue;
        }

        if let Ok(value) = &result {
            commit_value(
                map,
                errors,
                &flight_guard.key,
                key.to_string(),
                value.clone(),
            );
        } else if let Err(error) = &result {
            errors.insert(flight_guard.key.clone(), TimedEntry::new(error.to_string()));
        }
        return result;
    }
}

pub struct MetadataPrefetchGuard<'a> {
    cache: &'a RunCacheMetadataCache,
    key: String,
    state: Arc<FlightState>,
    generation: u64,
    version: u64,
    lock_guard: Option<OwnedMutexGuard<()>>,
}

impl<'a> MetadataPrefetchGuard<'a> {
    fn new(cache: &'a RunCacheMetadataCache, key: String) -> Self {
        let state = acquire_flight(&cache.in_flight, &cache.write_lock, &key);
        Self {
            cache,
            key,
            generation: cache.generation.load(Ordering::Relaxed),
            version: state.version.load(Ordering::Relaxed),
            state,
            lock_guard: None,
        }
    }

    /// Serialize a bulk metadata fetch with per-relation lookups for this key.
    pub async fn acquire(&mut self) {
        self.lock_guard = Some(Arc::clone(&self.state.lock).lock_owned().await);
    }

    pub fn insert_last_modified_epoch(&self, relation: impl Into<String>, epoch: Option<i64>) {
        let _write_guard = lock_write(&self.cache.write_lock);
        if self.cache.generation.load(Ordering::Relaxed) != self.generation
            || self.state.version.load(Ordering::Relaxed) != self.version
        {
            // The cache was invalidated while this fetch was in flight; the
            // result is stale and must not be committed. The caller's miss
            // will be retried under the new generation/version.
            tracing::trace!(
                key = %self.key,
                "dropping stale last-modified epoch write: cache generation/version changed \
                 since prefetch began"
            );
            return;
        }
        commit_value(
            &self.cache.last_modified_epochs,
            &self.cache.lookup_errors,
            &self.key,
            relation.into(),
            epoch,
        );
    }
}

impl Drop for MetadataPrefetchGuard<'_> {
    fn drop(&mut self) {
        release_flight(
            &self.cache.in_flight,
            &self.cache.write_lock,
            &self.key,
            &self.state,
        );
    }
}

fn commit_value<T>(
    map: &DashMap<String, TimedEntry<T>>,
    errors: &DashMap<String, TimedEntry<String>>,
    lookup: &str,
    key: String,
    value: T,
) {
    errors.remove(lookup);
    map.insert(key, TimedEntry::new(value));
}

fn lock_write(lock: &StdMutex<()>) -> std::sync::MutexGuard<'_, ()> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn acquire_flight(
    in_flight: &DashMap<String, Arc<FlightState>>,
    write_lock: &StdMutex<()>,
    key: &str,
) -> Arc<FlightState> {
    let _write_guard = write_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = in_flight
        .entry(key.to_owned())
        .or_insert_with(|| Arc::new(FlightState::default()))
        .clone();
    state.users.fetch_add(1, Ordering::Relaxed);
    state
}

fn release_flight(
    in_flight: &DashMap<String, Arc<FlightState>>,
    write_lock: &StdMutex<()>,
    key: &str,
    state: &Arc<FlightState>,
) {
    let _write_guard = write_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.users.fetch_sub(1, Ordering::Relaxed) == 1 {
        in_flight.remove_if(key, |_, current| Arc::ptr_eq(current, state));
    }
}

fn get_cached<T: Clone>(
    map: &DashMap<String, TimedEntry<T>>,
    key: &str,
    ttl: Option<Duration>,
) -> Option<T> {
    if let Some(value) = map.get(key) {
        if value.is_expired(ttl) {
            let fetched_at = value.fetched_at;
            drop(value);
            map.remove_if(key, |_, value| value.fetched_at == fetched_at);
            None
        } else {
            Some(value.value.clone())
        }
    } else {
        None
    }
}

fn lookup_error_key(kind: &str, relation: &str) -> String {
    format!("{kind}:{relation}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;
    use tokio::time::{Duration as TokioDuration, sleep, timeout};

    #[dbt_runtime::test]
    async fn relation_exists_lookup_caches_success() {
        let cache = RunCacheMetadataCache::new();
        let calls = Arc::new(AtomicUsize::new(0));

        let first = cache
            .get_or_try_insert_relation_exists("analytics.orders", {
                let calls = Arc::clone(&calls);
                move || {
                    let calls = Arc::clone(&calls);
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, &'static str>(true)
                    }
                }
            })
            .await
            .unwrap();
        let second = cache
            .get_or_try_insert_relation_exists("analytics.orders", || async {
                Ok::<_, &'static str>(false)
            })
            .await
            .unwrap();

        assert!(first);
        assert!(second);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(cache.in_flight.is_empty());
    }

    #[tokio::test]
    async fn concurrent_last_modified_lookup_fetches_once() {
        let cache = Arc::new(RunCacheMetadataCache::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let first_started = Arc::new(Notify::new());
        let second_started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());

        let first = tokio::spawn({
            let cache = Arc::clone(&cache);
            let calls = Arc::clone(&calls);
            let first_started = Arc::clone(&first_started);
            let release = Arc::clone(&release);
            async move {
                cache
                    .get_or_try_insert_last_modified_epoch("analytics.orders", move || {
                        let calls = Arc::clone(&calls);
                        let first_started = Arc::clone(&first_started);
                        let release = Arc::clone(&release);
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            first_started.notify_one();
                            release.notified().await;
                            Ok::<_, &'static str>(Some(123))
                        }
                    })
                    .await
            }
        });

        first_started.notified().await;

        let second = tokio::spawn({
            let cache = Arc::clone(&cache);
            let calls = Arc::clone(&calls);
            let second_started = Arc::clone(&second_started);
            let release = Arc::clone(&release);
            async move {
                cache
                    .get_or_try_insert_last_modified_epoch("analytics.orders", move || {
                        let calls = Arc::clone(&calls);
                        let second_started = Arc::clone(&second_started);
                        let release = Arc::clone(&release);
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            second_started.notify_one();
                            release.notified().await;
                            Ok::<_, &'static str>(Some(123))
                        }
                    })
                    .await
            }
        });

        // Give the second caller time to enter its fetch path. A single-flight
        // implementation will time out here because it waits on the first fetch.
        let _ = timeout(TokioDuration::from_millis(100), second_started.notified()).await;
        release.notify_waiters();

        assert_eq!(first.await.unwrap().unwrap(), Some(123));
        assert_eq!(second.await.unwrap().unwrap(), Some(123));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn blocked_prefetch_does_not_repopulate_removed_epoch() {
        let cache = Arc::new(RunCacheMetadataCache::new());
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let task = tokio::spawn({
            let cache = Arc::clone(&cache);
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            async move {
                let mut guard = cache.begin_last_modified_prefetch("analytics.orders");
                guard.acquire().await;
                started.notify_one();
                release.notified().await;
                guard.insert_last_modified_epoch("analytics.orders", Some(123));
            }
        });

        started.notified().await;
        cache.remove_last_modified_epoch("analytics.orders");
        release.notify_one();
        task.await.unwrap();

        assert_eq!(cache.last_modified_epoch("analytics.orders"), None);
        assert!(cache.in_flight.is_empty());
    }

    #[tokio::test]
    async fn prefetch_serializes_with_per_relation_lookup() {
        let cache = Arc::new(RunCacheMetadataCache::new());
        let lookup_started = Arc::new(Notify::new());
        let mut guard = cache.begin_last_modified_prefetch("analytics.orders");
        guard.acquire().await;

        let lookup = tokio::spawn({
            let cache = Arc::clone(&cache);
            let lookup_started = Arc::clone(&lookup_started);
            async move {
                cache
                    .get_or_try_insert_last_modified_epoch("analytics.orders", move || {
                        let lookup_started = Arc::clone(&lookup_started);
                        async move {
                            lookup_started.notify_one();
                            Ok::<_, &'static str>(Some(200))
                        }
                    })
                    .await
            }
        });

        assert!(
            timeout(TokioDuration::from_millis(100), lookup_started.notified())
                .await
                .is_err()
        );
        drop(guard);

        assert_eq!(lookup.await.unwrap().unwrap(), Some(200));
        assert_eq!(
            cache.last_modified_epoch("analytics.orders"),
            Some(Some(200))
        );
    }

    #[tokio::test]
    async fn invalidation_retries_in_flight_lookup_before_caching_result() {
        let cache = Arc::new(RunCacheMetadataCache::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let task = tokio::spawn({
            let cache = Arc::clone(&cache);
            let calls = Arc::clone(&calls);
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            async move {
                cache
                    .get_or_try_insert_last_modified_epoch("analytics.orders", move || {
                        let calls = Arc::clone(&calls);
                        let started = Arc::clone(&started);
                        let release = Arc::clone(&release);
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            started.notify_one();
                            release.notified().await;
                            Ok::<_, &'static str>(Some(123))
                        }
                    })
                    .await
            }
        });

        started.notified().await;
        cache.invalidate_relation_metadata("analytics.orders");
        release.notify_one();
        started.notified().await;
        release.notify_one();

        assert_eq!(task.await.unwrap().unwrap(), Some(123));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            cache.last_modified_epoch("analytics.orders"),
            Some(Some(123))
        );
    }

    #[tokio::test]
    async fn concurrent_relation_exists_lookup_fetches_once_and_clear_retries() {
        let cache = Arc::new(RunCacheMetadataCache::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let task = tokio::spawn({
            let cache = Arc::clone(&cache);
            let calls = Arc::clone(&calls);
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            async move {
                cache
                    .get_or_try_insert_relation_exists("analytics.orders", move || {
                        let calls = Arc::clone(&calls);
                        let started = Arc::clone(&started);
                        let release = Arc::clone(&release);
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            started.notify_one();
                            release.notified().await;
                            Ok::<_, &'static str>(true)
                        }
                    })
                    .await
            }
        });

        started.notified().await;
        cache.clear();
        release.notify_one();
        started.notified().await;
        release.notify_one();

        assert!(task.await.unwrap().unwrap());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(cache.relation_exists("analytics.orders"), Some(true));
        assert!(cache.in_flight.is_empty());
    }

    #[tokio::test]
    async fn cancelled_lookup_releases_coordination_entry() {
        let cache = Arc::new(RunCacheMetadataCache::new());
        let started = Arc::new(Notify::new());
        let task = tokio::spawn({
            let cache = Arc::clone(&cache);
            let started = Arc::clone(&started);
            async move {
                cache
                    .get_or_try_insert_relation_exists("analytics.orders", move || {
                        let started = Arc::clone(&started);
                        async move {
                            started.notify_one();
                            std::future::pending::<Result<bool, &'static str>>().await
                        }
                    })
                    .await
            }
        });

        started.notified().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(cache.in_flight.is_empty());
    }

    #[dbt_runtime::test]
    async fn failed_lookup_is_not_cached_for_fail_open_callers() {
        let cache = RunCacheMetadataCache::new();

        let err = cache
            .get_or_try_insert_last_modified_epoch("analytics.orders", || async {
                Err::<Option<i64>, _>("warehouse metadata unavailable")
            })
            .await
            .unwrap_err();

        assert_eq!(err, "warehouse metadata unavailable");
        assert_eq!(cache.last_modified_epoch("analytics.orders"), None);
        assert_eq!(
            cache.lookup_error("last_modified_epoch:analytics.orders"),
            Some("warehouse metadata unavailable".to_string())
        );

        let epoch = cache
            .get_or_try_insert_last_modified_epoch("analytics.orders", || async {
                Ok::<_, &'static str>(Some(123))
            })
            .await
            .unwrap();

        assert_eq!(epoch, Some(123));
        assert_eq!(
            cache.last_modified_epoch("analytics.orders"),
            Some(Some(123))
        );
        assert_eq!(
            cache.lookup_error("last_modified_epoch:analytics.orders"),
            None
        );
    }

    #[dbt_runtime::test]
    async fn ttl_expiry_refreshes_cached_values() {
        let cache = RunCacheMetadataCache::with_ttl(Duration::from_millis(5));
        let calls = Arc::new(AtomicUsize::new(0));

        let first = cache
            .get_or_try_insert_relation_exists("analytics.orders", {
                let calls = Arc::clone(&calls);
                move || {
                    let calls = Arc::clone(&calls);
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, &'static str>(true)
                    }
                }
            })
            .await
            .unwrap();
        assert!(first);

        sleep(TokioDuration::from_millis(10)).await;

        let second = cache
            .get_or_try_insert_relation_exists("analytics.orders", {
                let calls = Arc::clone(&calls);
                move || {
                    let calls = Arc::clone(&calls);
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok::<_, &'static str>(false)
                    }
                }
            })
            .await
            .unwrap();

        assert!(!second);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[dbt_runtime::test]
    async fn ttl_expiry_refreshes_lookup_errors() {
        let cache = RunCacheMetadataCache::with_ttl(Duration::from_millis(5));

        let err = cache
            .get_or_try_insert_relation_exists("analytics.orders", || async {
                Err::<bool, _>("metadata unavailable")
            })
            .await
            .unwrap_err();
        assert_eq!(err, "metadata unavailable");
        assert_eq!(
            cache.lookup_error("relation_exists:analytics.orders"),
            Some("metadata unavailable".to_string())
        );

        sleep(TokioDuration::from_millis(10)).await;

        assert_eq!(cache.lookup_error("relation_exists:analytics.orders"), None);
    }

    #[test]
    fn lookup_errors_can_be_cleared_after_later_success() {
        let cache = RunCacheMetadataCache::new();

        cache.insert_lookup_error("custom_lookup:analytics.raw.orders", "metadata unavailable");
        assert_eq!(
            cache.lookup_error("custom_lookup:analytics.raw.orders"),
            Some("metadata unavailable".to_string())
        );

        cache.remove_lookup_error("custom_lookup:analytics.raw.orders");
        assert_eq!(
            cache.lookup_error("custom_lookup:analytics.raw.orders"),
            None
        );
    }

    #[test]
    fn direct_success_inserts_clear_lookup_errors() {
        let cache = RunCacheMetadataCache::new();

        cache.insert_lookup_error("relation_exists:analytics.orders", "metadata unavailable");
        cache.insert_relation_exists("analytics.orders", true);
        assert_eq!(cache.lookup_error("relation_exists:analytics.orders"), None);

        cache.insert_lookup_error(
            "last_modified_epoch:analytics.orders",
            "metadata unavailable",
        );
        cache.insert_last_modified_epoch("analytics.orders", Some(123));
        assert_eq!(
            cache.lookup_error("last_modified_epoch:analytics.orders"),
            None
        );
    }

    #[test]
    fn relation_metadata_can_be_invalidated_after_relation_changes() {
        let cache = RunCacheMetadataCache::new();

        cache.insert_relation_exists("analytics.orders", false);
        cache.insert_last_modified_epoch("analytics.orders", None);
        cache.insert_lookup_error("relation_exists:analytics.orders", "missing");
        cache.insert_lookup_error("last_modified_epoch:analytics.orders", "missing");

        cache.invalidate_relation_metadata("analytics.orders");

        assert_eq!(cache.relation_exists("analytics.orders"), None);
        assert_eq!(cache.last_modified_epoch("analytics.orders"), None);
        assert_eq!(cache.lookup_error("relation_exists:analytics.orders"), None);
        assert_eq!(
            cache.lookup_error("last_modified_epoch:analytics.orders"),
            None
        );
    }
}
