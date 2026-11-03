//! Short-lived warehouse metadata cache for dbt State service request assembly.
//!
//! The cache is keyed by rendered relation name and stores only metadata that is
//! expensive or redundant to fetch while constructing service payloads. Failed
//! metadata lookups are deliberately not cached so callers can fail open and
//! retry later in the same invocation.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use dbt_schemas::schemas::relations::base::BaseRelation;
use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Debug, Default)]
pub struct RunCacheMetadataCache {
    ttl: Option<Duration>,
    relation_exists: DashMap<String, TimedEntry<bool>>,
    last_modified_epochs: DashMap<String, TimedEntry<Option<i64>>>,
    lookup_errors: DashMap<String, TimedEntry<String>>,
    in_flight: DashMap<String, Arc<FlightState>>,
    /// Per-key version counters, kept separate from `in_flight` so a version
    /// survives the refcounted `FlightState` entry being dropped and recreated
    /// while nobody is in flight for that key. See `bump_version`.
    versions: DashMap<String, Arc<AtomicU64>>,
    generation: AtomicU64,
    write_lock: StdMutex<()>,
    /// Coalesces BigQuery freshness lookups by database and schema.
    schema_fetch_coalescers: DashMap<(String, String), Arc<SchemaFetchCoalescer>>,
    /// Protects coalescer-map refcounts independently of cache writes.
    coalescer_lock: StdMutex<()>,
}

trait RefCounted {
    fn users(&self) -> &AtomicUsize;
}

#[derive(Debug, Default)]
struct FlightState {
    lock: Arc<Mutex<()>>,
    users: AtomicUsize,
}

impl RefCounted for FlightState {
    fn users(&self) -> &AtomicUsize {
        &self.users
    }
}

/// A relation and its cache version when added to a pending batch.
type PendingRelation = (Arc<dyn BaseRelation>, u64);

/// Merges concurrent BigQuery freshness misses for the same schema.
/// Refcounting removes idle entries from the process-wide cache.
#[derive(Debug, Default)]
struct SchemaFetchCoalescer {
    pending: Mutex<BTreeMap<String, PendingRelation>>,
    query_gate: Arc<Mutex<()>>,
    users: AtomicUsize,
}

impl RefCounted for SchemaFetchCoalescer {
    fn users(&self) -> &AtomicUsize {
        &self.users
    }
}

/// Holds a schema coalescer reservation while the query gate is held.
struct SchemaFetchReservation<'a> {
    cache: &'a RunCacheMetadataCache,
    key: (String, String),
    coalescer: Arc<SchemaFetchCoalescer>,
}

impl Drop for SchemaFetchReservation<'_> {
    fn drop(&mut self) {
        release_refcounted(
            &self.cache.schema_fetch_coalescers,
            &self.cache.coalescer_lock,
            &self.key,
            &self.coalescer,
        );
    }
}

pub struct SchemaFetchGuard<'a> {
    reservation: SchemaFetchReservation<'a>,
    generation_at_join: u64,
    // Drop unlocks the gate before the reservation releases its refcount.
    gate_guard: Option<OwnedMutexGuard<()>>,
}

impl SchemaFetchGuard<'_> {
    /// Take pending relations and their cache versions for this query.
    pub async fn drain(&self) -> BTreeMap<String, PendingRelation> {
        let mut pending = self.reservation.coalescer.pending.lock().await;
        std::mem::take(&mut *pending)
    }

    /// Requeue a cancelled batch for another waiter to retry.
    pub async fn restore(&self, drained: BTreeMap<String, PendingRelation>) {
        let mut pending = self.reservation.coalescer.pending.lock().await;
        for (name, relation) in drained {
            pending.entry(name).or_insert(relation);
        }
    }

    /// Whether the cache was cleared after this guard joined.
    pub fn is_stale(&self, cache: &RunCacheMetadataCache) -> bool {
        cache.generation.load(Ordering::Relaxed) != self.generation_at_join
    }

    /// Whether the relation's cache version still matches its join snapshot.
    pub fn is_relation_fresh(
        &self,
        cache: &RunCacheMetadataCache,
        relation: &str,
        version_at_join: u64,
    ) -> bool {
        cache.last_modified_flight_version(relation) == version_at_join
    }

    /// Atomically cache a result if the cache and relation versions are unchanged.
    pub fn insert_last_modified_epoch_if_fresh(
        &self,
        cache: &RunCacheMetadataCache,
        relation: impl Into<String>,
        epoch: Option<i64>,
        version_at_join: u64,
    ) -> bool {
        let relation = relation.into();
        let _write_guard = lock_write(&cache.write_lock);
        if cache.generation.load(Ordering::Relaxed) != self.generation_at_join
            || cache.last_modified_flight_version(&relation) != version_at_join
        {
            return false;
        }
        commit_value(
            &cache.last_modified_epochs,
            &cache.lookup_errors,
            &lookup_error_key("last_modified_epoch", &relation),
            relation,
            epoch,
        );
        true
    }
}

impl Drop for SchemaFetchGuard<'_> {
    fn drop(&mut self) {
        // Unlock before releasing the reservation so new joins reuse this gate.
        self.gate_guard.take();
    }
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

    /// Merge relations into the schema batch and wait for its query gate.
    pub async fn join_schema_fetch(
        &self,
        database: &str,
        schema: &str,
        relations: &BTreeMap<String, Arc<dyn BaseRelation>>,
    ) -> SchemaFetchGuard<'_> {
        let key = (database.to_owned(), schema.to_owned());
        let coalescer = acquire_refcounted(
            &self.schema_fetch_coalescers,
            &self.coalescer_lock,
            key.clone(),
        );
        let reservation = SchemaFetchReservation {
            cache: self,
            key,
            coalescer,
        };
        {
            let mut pending = reservation.coalescer.pending.lock().await;
            for (name, relation) in relations {
                let version = self.last_modified_flight_version(name);
                pending.insert(name.clone(), (Arc::clone(relation), version));
            }
        }
        let generation_at_join = self.generation.load(Ordering::Relaxed);
        let gate_guard = Arc::clone(&reservation.coalescer.query_gate)
            .lock_owned()
            .await;
        SchemaFetchGuard {
            reservation,
            generation_at_join,
            gate_guard: Some(gate_guard),
        }
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

    /// Bump a key's version even when nothing is in flight. Counters survive
    /// `clear()`; the generation check rejects writes started before a clear.
    fn bump_version(&self, key: &str) {
        self.versions
            .entry(key.to_string())
            .or_default()
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Current version for an arbitrary (already-namespaced) key.
    fn version_for_key(&self, key: &str) -> u64 {
        self.versions
            .get(key)
            .map(|version| version.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Cache version captured when a relation joins the schema batch.
    fn last_modified_flight_version(&self, relation: &str) -> u64 {
        self.version_for_key(&lookup_error_key("last_modified_epoch", relation))
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

        let version_at_fetch = cache.version_for_key(&flight_guard.key);
        let generation_at_fetch = cache.generation.load(Ordering::Relaxed);
        let result = fetch().await;

        // Keep the check and insertion under the same lock as invalidation and
        // clear. Otherwise an invalidation can land between the check and the
        // insert, allowing a stale result to repopulate the cache.
        let _write_guard = lock_write(&cache.write_lock);
        let unchanged = cache.generation.load(Ordering::Relaxed) == generation_at_fetch
            && cache.version_for_key(&flight_guard.key) == version_at_fetch;
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
        let state = acquire_refcounted(&cache.in_flight, &cache.write_lock, key.clone());
        Self {
            generation: cache.generation.load(Ordering::Relaxed),
            version: cache.version_for_key(&key),
            cache,
            key,
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
            || self.cache.version_for_key(&self.key) != self.version
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
        release_refcounted(
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

/// Get or create an entry and increment its refcount under `lock`.
fn acquire_refcounted<K, V>(map: &DashMap<K, Arc<V>>, lock: &StdMutex<()>, key: K) -> Arc<V>
where
    K: std::hash::Hash + Eq,
    V: Default + RefCounted,
{
    let _guard = lock_write(lock);
    let value = map
        .entry(key)
        .or_insert_with(|| Arc::new(V::default()))
        .clone();
    value.users().fetch_add(1, Ordering::Relaxed);
    value
}

fn release_refcounted<K, V>(map: &DashMap<K, Arc<V>>, lock: &StdMutex<()>, key: &K, value: &Arc<V>)
where
    K: std::hash::Hash + Eq,
    V: RefCounted,
{
    let _guard = lock_write(lock);
    if value.users().fetch_sub(1, Ordering::Relaxed) == 1 {
        map.remove_if(key, |_, current| Arc::ptr_eq(current, value));
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

    use dbt_adapter::relation::create_relation;
    use dbt_adapter_core::AdapterType;
    use dbt_schemas::schemas::common::ResolvedQuoting;

    fn test_relation(schema: &str, table: &str) -> (String, Arc<dyn BaseRelation>) {
        let relation: Arc<dyn BaseRelation> = create_relation(
            AdapterType::Bigquery,
            "db".to_string(),
            schema.to_string(),
            Some(table.to_string()),
            None,
            ResolvedQuoting::default(),
        )
        .unwrap()
        .into();
        let name = relation.semantic_fqn();
        (name, relation)
    }

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

    #[tokio::test]
    async fn concurrent_schema_fetch_joins_merge_into_holders_drain() {
        let cache = Arc::new(RunCacheMetadataCache::new());
        let (first_name, first_relation) = test_relation("analytics", "orders");
        let (second_name, second_relation) = test_relation("analytics", "customers");

        let holder = cache
            .join_schema_fetch("db", "analytics", &BTreeMap::new())
            .await;
        let relation_sets = vec![
            BTreeMap::from([(first_name.clone(), first_relation)]),
            BTreeMap::from([(second_name.clone(), second_relation)]),
        ];
        let mut joins = Vec::new();
        for relations in &relation_sets {
            let mut join = Box::pin(async {
                let guard = cache.join_schema_fetch("db", "analytics", relations).await;
                guard.drain().await.is_empty()
            });
            assert!(
                std::future::poll_fn(|cx| {
                    std::task::Poll::Ready(join.as_mut().poll(cx).is_pending())
                })
                .await
            );
            joins.push(join);
        }

        let drained = holder.drain().await;
        assert_eq!(drained.len(), 2);
        assert!(drained.contains_key(&first_name));
        assert!(drained.contains_key(&second_name));
        drop(holder);

        for join in joins {
            assert!(join.await);
        }
    }

    #[tokio::test]
    async fn cancelled_schema_fetch_join_releases_its_reservation() {
        use std::future::Future;

        let cache = RunCacheMetadataCache::new();
        let key = ("db".to_string(), "analytics".to_string());
        let holder = cache
            .join_schema_fetch("db", "analytics", &BTreeMap::new())
            .await;

        // Cancel once while waiting for the pending-set lock.
        let pending_lock = holder.reservation.coalescer.pending.lock().await;
        let empty = BTreeMap::new();
        let mut pending_waiter = Box::pin(cache.join_schema_fetch("db", "analytics", &empty));
        assert!(
            std::future::poll_fn(|cx| {
                std::task::Poll::Ready(pending_waiter.as_mut().poll(cx).is_pending())
            })
            .await
        );
        drop(pending_waiter);
        assert_eq!(
            cache
                .schema_fetch_coalescers
                .get(&key)
                .unwrap()
                .users
                .load(Ordering::Relaxed),
            1
        );
        drop(pending_lock);

        // Cancel again after merging, while waiting for the query gate.
        let mut gate_waiter = Box::pin(cache.join_schema_fetch("db", "analytics", &empty));
        assert!(
            std::future::poll_fn(|cx| {
                std::task::Poll::Ready(gate_waiter.as_mut().poll(cx).is_pending())
            })
            .await
        );
        drop(gate_waiter);
        assert_eq!(
            cache
                .schema_fetch_coalescers
                .get(&key)
                .unwrap()
                .users
                .load(Ordering::Relaxed),
            1
        );

        drop(holder);
        assert!(!cache.schema_fetch_coalescers.contains_key(&key));
    }

    #[tokio::test]
    async fn schema_fetch_guard_is_stale_after_cache_clear() {
        let cache = RunCacheMetadataCache::new();
        let guard = cache
            .join_schema_fetch("db", "analytics", &BTreeMap::new())
            .await;
        assert!(!guard.is_stale(&cache));

        cache.clear();
        assert!(guard.is_stale(&cache));
    }

    #[tokio::test]
    async fn schema_fetch_guard_detects_per_relation_invalidation() {
        // Per-relation invalidation must stale this relation without clearing the cache.
        let cache = RunCacheMetadataCache::new();
        let (name, relation) = test_relation("analytics", "orders");

        // Match production order: acquire the relation guard before joining.
        let _prefetch_guard = cache.begin_last_modified_prefetch(&name);

        let guard = cache
            .join_schema_fetch(
                "db",
                "analytics",
                &BTreeMap::from([(name.clone(), relation)]),
            )
            .await;
        let drained = guard.drain().await;
        let (_, version_at_join) = *drained.get(&name).unwrap();
        assert!(guard.is_relation_fresh(&cache, &name, version_at_join));

        // Simulate invalidation while the query is in flight.
        cache.invalidate_relation_metadata(&name);

        assert!(!guard.is_relation_fresh(&cache, &name, version_at_join));
        assert!(!guard.is_stale(&cache));
    }

    #[tokio::test]
    async fn dropped_prefetch_guard_does_not_reset_relation_version() {
        let cache = RunCacheMetadataCache::new();
        let (name, relation) = test_relation("analytics", "orders");
        let prefetch = cache.begin_last_modified_prefetch(&name);
        let guard = cache
            .join_schema_fetch(
                "db",
                "analytics",
                &BTreeMap::from([(name.clone(), relation)]),
            )
            .await;
        let drained = guard.drain().await;
        let (_, version_at_join) = *drained.get(&name).unwrap();

        drop(prefetch);
        cache.invalidate_relation_metadata(&name);
        let _recreated = cache.begin_last_modified_prefetch(&name);

        assert!(!guard.insert_last_modified_epoch_if_fresh(
            &cache,
            name.clone(),
            Some(1),
            version_at_join,
        ));
        assert_eq!(cache.last_modified_epoch(&name), None);
    }

    #[tokio::test]
    async fn insert_last_modified_epoch_if_fresh_skips_invalidated_relation() {
        // Check and commit atomically to avoid stale writes.
        let cache = RunCacheMetadataCache::new();
        let (fresh_name, fresh_relation) = test_relation("analytics", "orders");
        let (stale_name, stale_relation) = test_relation("analytics", "customers");
        let _fresh_guard = cache.begin_last_modified_prefetch(&fresh_name);
        let _stale_guard = cache.begin_last_modified_prefetch(&stale_name);

        let guard = cache
            .join_schema_fetch(
                "db",
                "analytics",
                &BTreeMap::from([
                    (fresh_name.clone(), fresh_relation),
                    (stale_name.clone(), stale_relation),
                ]),
            )
            .await;
        let drained = guard.drain().await;
        let (_, fresh_version) = *drained.get(&fresh_name).unwrap();
        let (_, stale_version) = *drained.get(&stale_name).unwrap();

        // Simulate one relation rebuilding during the query.
        cache.invalidate_relation_metadata(&stale_name);

        assert!(guard.insert_last_modified_epoch_if_fresh(
            &cache,
            fresh_name.clone(),
            Some(100),
            fresh_version
        ));
        assert!(!guard.insert_last_modified_epoch_if_fresh(
            &cache,
            stale_name.clone(),
            Some(200),
            stale_version
        ));

        assert_eq!(cache.last_modified_epoch(&fresh_name), Some(Some(100)));
        assert_eq!(cache.last_modified_epoch(&stale_name), None);
    }
}
