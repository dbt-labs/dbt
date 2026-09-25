use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use arrow_schema::SchemaRef;
use blake3::Hasher;
use dbt_adapter_core::AdapterType;
use dbt_common::FsResult;
use dbt_common::sccmap::HashMap as SccHashMap;
use dbt_schema_store::{CanonicalFqn, SchemaStoreTrait};
use scc::hash_map::Entry;

type UnitTestExpectedSchemaFingerprint = [u8; 32];

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FixtureSchemaCachePolicy {
    /// Accept a schema already available in the shared cache.
    Reuse,
    /// Make the first request in this invocation fetch a fresh schema.
    Refresh,
}

/// All inputs that must match before reusing an inferred unit-test schema.
///
/// Each field is hashed into the cache key; the strings themselves are not stored.
#[derive(Debug, Clone, Copy)]
pub struct UnitTestExpectedSchemaKeyInput<'a> {
    /// Adapter used to interpret SQL and data types.
    pub adapter_type: AdapterType,
    /// Model whose output schema is being inferred.
    pub model_unique_id: &'a str,
    /// SQL that identifies the structural inference request.
    pub local_probe_shape_sql: &'a str,
    /// SQL that identifies the adapter inference request.
    pub fallback_probe_shape_sql: &'a str,
    /// Whether adapter inference inspects a query instead of a temporary relation.
    pub fallback_with_query_schema: bool,
    /// Serialized vars, environment variables, and macro overrides.
    pub serialized_overrides: &'a str,
}

/// Invocation-scoped fingerprint of an expected-schema inference request.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub struct UnitTestExpectedSchemaKey {
    fingerprint: UnitTestExpectedSchemaFingerprint,
}

impl UnitTestExpectedSchemaKey {
    pub fn new(input: UnitTestExpectedSchemaKeyInput<'_>) -> Self {
        let adapter_name: &'static str = input.adapter_type.into();
        let fallback_mode = if input.fallback_with_query_schema {
            b"query-schema".as_slice()
        } else {
            b"temporary-relation".as_slice()
        };
        let mut hasher = Hasher::new();
        update_fingerprint_component(&mut hasher, b"adapter", adapter_name.as_bytes());
        update_fingerprint_component(
            &mut hasher,
            b"model_unique_id",
            input.model_unique_id.as_bytes(),
        );
        update_fingerprint_component(
            &mut hasher,
            b"local_probe_shape_sql",
            input.local_probe_shape_sql.as_bytes(),
        );
        update_fingerprint_component(
            &mut hasher,
            b"fallback_probe_shape_sql",
            input.fallback_probe_shape_sql.as_bytes(),
        );
        update_fingerprint_component(&mut hasher, b"fallback_mode", fallback_mode);
        update_fingerprint_component(
            &mut hasher,
            b"serialized_overrides",
            input.serialized_overrides.as_bytes(),
        );

        Self {
            fingerprint: *hasher.finalize().as_bytes(),
        }
    }
}

fn update_fingerprint_component(hasher: &mut Hasher, name: &[u8], value: &[u8]) {
    hasher.update(&(name.len() as u64).to_le_bytes());
    hasher.update(name);
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct UnitTestSchemaStats {
    pub fixture_requests: u64,
    pub fixture_owners: u64,
    pub fixture_coalesced: u64,
    pub fixture_wait_ns: u64,
    pub fixture_fetch_ns: u64,
    pub expected_requests: u64,
    pub expected_hits: u64,
    pub expected_misses: u64,
    pub expected_inference_ns: u64,
}

#[derive(Debug, Default)]
struct UnitTestSchemaCounters {
    fixture_requests: AtomicU64,
    fixture_owners: AtomicU64,
    fixture_coalesced: AtomicU64,
    fixture_wait_ns: AtomicU64,
    fixture_fetch_ns: AtomicU64,
    expected_requests: AtomicU64,
    expected_hits: AtomicU64,
    expected_misses: AtomicU64,
    expected_inference_ns: AtomicU64,
}

impl UnitTestSchemaCounters {
    fn add_duration(counter: &AtomicU64, duration: Duration) {
        counter.fetch_add(
            duration.as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }
}

#[derive(Debug, Default)]
pub struct UnitTestSchemaState {
    /// Entry guards serialize shared-cache rechecks and fetches by relation.
    /// `Some` means this invocation has accepted or refreshed the schema.
    fixture_schema_fetches: SccHashMap<CanonicalFqn, Option<SchemaRef>>,
    expected_schema_entries: SccHashMap<UnitTestExpectedSchemaKey, SchemaRef>,
    counters: UnitTestSchemaCounters,
}

impl UnitTestSchemaState {
    pub async fn get_or_try_fetch_fixture_schema<F, Fut>(
        &self,
        canonical_fqn: CanonicalFqn,
        schema_cache: &dyn SchemaStoreTrait,
        cache_policy: FixtureSchemaCachePolicy,
        fetch: F,
    ) -> FsResult<SchemaRef>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = FsResult<SchemaRef>>,
    {
        self.counters
            .fixture_requests
            .fetch_add(1, Ordering::Relaxed);
        let wait_started = Instant::now();
        let mut entry_guard = self
            .fixture_schema_fetches
            .entry_async(canonical_fqn)
            .await
            .or_insert(None);
        UnitTestSchemaCounters::add_duration(
            &self.counters.fixture_wait_ns,
            wait_started.elapsed(),
        );

        if let Some(invocation_schema) = entry_guard.get() {
            self.counters
                .fixture_coalesced
                .fetch_add(1, Ordering::Relaxed);
            return Ok(schema_cache
                .get_schema_async(entry_guard.key())
                .await
                .map_or_else(
                    || Arc::clone(invocation_schema),
                    |entry| Arc::clone(entry.inner()),
                ));
        }

        if cache_policy == FixtureSchemaCachePolicy::Reuse
            && let Some(entry) = schema_cache.get_schema_async(entry_guard.key()).await
        {
            let schema = Arc::clone(entry.inner());
            *entry_guard.get_mut() = Some(Arc::clone(&schema));
            self.counters
                .fixture_coalesced
                .fetch_add(1, Ordering::Relaxed);
            return Ok(schema);
        }

        self.counters.fixture_owners.fetch_add(1, Ordering::Relaxed);
        let fetch_started = Instant::now();
        let schema = fetch().await;
        UnitTestSchemaCounters::add_duration(
            &self.counters.fixture_fetch_ns,
            fetch_started.elapsed(),
        );
        if let Ok(schema) = &schema {
            *entry_guard.get_mut() = Some(Arc::clone(schema));
        }
        schema
    }

    pub fn get_or_try_infer_expected_schema(
        &self,
        key: UnitTestExpectedSchemaKey,
        infer: impl FnOnce() -> FsResult<SchemaRef>,
    ) -> FsResult<SchemaRef> {
        self.counters
            .expected_requests
            .fetch_add(1, Ordering::Relaxed);
        match self.expected_schema_entries.entry_sync(key) {
            Entry::Occupied(entry) => {
                self.counters.expected_hits.fetch_add(1, Ordering::Relaxed);
                Ok(Arc::clone(entry.get()))
            }
            Entry::Vacant(entry) => {
                self.counters
                    .expected_misses
                    .fetch_add(1, Ordering::Relaxed);
                let started = Instant::now();
                let schema = infer();
                UnitTestSchemaCounters::add_duration(
                    &self.counters.expected_inference_ns,
                    started.elapsed(),
                );
                let schema = schema?;
                entry.insert_entry(Arc::clone(&schema));
                Ok(schema)
            }
        }
    }

    pub fn stats_snapshot(&self) -> UnitTestSchemaStats {
        UnitTestSchemaStats {
            fixture_requests: self.counters.fixture_requests.load(Ordering::Relaxed),
            fixture_owners: self.counters.fixture_owners.load(Ordering::Relaxed),
            fixture_coalesced: self.counters.fixture_coalesced.load(Ordering::Relaxed),
            fixture_wait_ns: self.counters.fixture_wait_ns.load(Ordering::Relaxed),
            fixture_fetch_ns: self.counters.fixture_fetch_ns.load(Ordering::Relaxed),
            expected_requests: self.counters.expected_requests.load(Ordering::Relaxed),
            expected_hits: self.counters.expected_hits.load(Ordering::Relaxed),
            expected_misses: self.counters.expected_misses.load(Ordering::Relaxed),
            expected_inference_ns: self.counters.expected_inference_ns.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    use arrow_schema::{DataType, Field, Schema};
    use dbt_adapter_core::AdapterType;
    use dbt_schema_store::mock_store::MockSchemaStore;
    use dbt_schema_store::{CanonicalFqn, SchemaStoreTrait};

    use super::{
        FixtureSchemaCachePolicy, UnitTestExpectedSchemaKey, UnitTestExpectedSchemaKeyInput,
        UnitTestSchemaState,
    };

    #[dbt_runtime::test]
    async fn fixture_schema_state_serializes_fetches_by_relation() {
        let state = UnitTestSchemaState::default();
        let canonical_fqn = CanonicalFqn::default();
        let schema_cache = MockSchemaStore::new();
        let fetch_count = AtomicUsize::new(0);

        let first = state.get_or_try_fetch_fixture_schema(
            canonical_fqn.clone(),
            &schema_cache,
            FixtureSchemaCachePolicy::Reuse,
            || async {
                fetch_count.fetch_add(1, Ordering::Relaxed);
                tokio::task::yield_now().await;
                let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
                schema_cache
                    .register_schema(&canonical_fqn, None, Arc::clone(&schema), true)
                    .unwrap();
                Ok(schema)
            },
        );
        let second = state.get_or_try_fetch_fixture_schema(
            canonical_fqn.clone(),
            &schema_cache,
            FixtureSchemaCachePolicy::Reuse,
            || async {
                fetch_count.fetch_add(1, Ordering::Relaxed);
                Ok(Arc::new(Schema::empty()))
            },
        );
        let (first, second) = tokio::join!(biased;
            first,
            second,
        );

        assert_eq!(first.unwrap().field(0).name(), "id");
        assert_eq!(second.unwrap().field(0).name(), "id");

        let stats = state.stats_snapshot();
        assert_eq!(fetch_count.load(Ordering::Relaxed), 1);
        assert_eq!(stats.fixture_requests, 2);
        assert_eq!(stats.fixture_owners, 1);
        assert_eq!(stats.fixture_coalesced, 1);
    }

    #[dbt_runtime::test]
    async fn fixture_schema_refresh_retries_failed_fetch() {
        use dbt_common::{ErrorCode, fs_err};

        let state = UnitTestSchemaState::default();
        let canonical_fqn = CanonicalFqn::default();
        let schema_cache = MockSchemaStore::new();
        schema_cache
            .register_schema(
                &canonical_fqn,
                None,
                Arc::new(Schema::new(vec![Field::new(
                    "stale_id",
                    DataType::Int64,
                    false,
                )])),
                true,
            )
            .unwrap();

        let first = state
            .get_or_try_fetch_fixture_schema(
                canonical_fqn.clone(),
                &schema_cache,
                FixtureSchemaCachePolicy::Refresh,
                || async { Err(fs_err!(ErrorCode::Generic, "fetch failed")) },
            )
            .await;
        assert!(first.is_err());

        let schema = state
            .get_or_try_fetch_fixture_schema(
                canonical_fqn,
                &schema_cache,
                FixtureSchemaCachePolicy::Refresh,
                || async {
                    Ok(Arc::new(Schema::new(vec![Field::new(
                        "id",
                        DataType::Int64,
                        false,
                    )])))
                },
            )
            .await
            .unwrap();

        assert_eq!(schema.field(0).name(), "id");
        let stats = state.stats_snapshot();
        assert_eq!(stats.fixture_requests, 2);
        assert_eq!(stats.fixture_owners, 2);
        assert_eq!(stats.fixture_coalesced, 0);
    }

    #[dbt_runtime::test]
    async fn fixture_schema_coordination_reads_latest_shared_schema() {
        let state = UnitTestSchemaState::default();
        let canonical_fqn = CanonicalFqn::default();
        let schema_cache = MockSchemaStore::new();
        let first_schema = Arc::new(Schema::new(vec![Field::new(
            "first_id",
            DataType::Int64,
            false,
        )]));
        schema_cache
            .register_schema(&canonical_fqn, None, Arc::clone(&first_schema), true)
            .unwrap();

        let first = state
            .get_or_try_fetch_fixture_schema(
                canonical_fqn.clone(),
                &schema_cache,
                FixtureSchemaCachePolicy::Reuse,
                || async { panic!("cached schema should skip fetch") },
            )
            .await
            .unwrap();
        assert_eq!(first.field(0).name(), "first_id");

        let second_schema = Arc::new(Schema::new(vec![Field::new(
            "second_id",
            DataType::Int64,
            false,
        )]));
        schema_cache
            .register_schema(&canonical_fqn, None, second_schema, true)
            .unwrap();

        let second = state
            .get_or_try_fetch_fixture_schema(
                canonical_fqn,
                &schema_cache,
                FixtureSchemaCachePolicy::Reuse,
                || async { panic!("cached schema should skip fetch") },
            )
            .await
            .unwrap();
        assert_eq!(second.field(0).name(), "second_id");
    }

    #[dbt_runtime::test]
    async fn fixture_schema_refresh_replaces_prior_cache_once() {
        let state = UnitTestSchemaState::default();
        let canonical_fqn = CanonicalFqn::default();
        let schema_cache = MockSchemaStore::new();
        schema_cache
            .register_schema(
                &canonical_fqn,
                None,
                Arc::new(Schema::new(vec![Field::new(
                    "stale_id",
                    DataType::Int64,
                    false,
                )])),
                true,
            )
            .unwrap();
        let fetch_count = AtomicUsize::new(0);

        let first = state.get_or_try_fetch_fixture_schema(
            canonical_fqn.clone(),
            &schema_cache,
            FixtureSchemaCachePolicy::Refresh,
            || async {
                fetch_count.fetch_add(1, Ordering::Relaxed);
                tokio::task::yield_now().await;
                let schema = Arc::new(Schema::new(vec![Field::new(
                    "fresh_id",
                    DataType::Int64,
                    false,
                )]));
                schema_cache
                    .register_schema(&canonical_fqn, None, Arc::clone(&schema), true)
                    .unwrap();
                Ok(schema)
            },
        );
        let second = state.get_or_try_fetch_fixture_schema(
            canonical_fqn.clone(),
            &schema_cache,
            FixtureSchemaCachePolicy::Refresh,
            || async {
                fetch_count.fetch_add(1, Ordering::Relaxed);
                Ok(Arc::new(Schema::empty()))
            },
        );
        let (first, second) = tokio::join!(biased; first, second);

        assert_eq!(first.unwrap().field(0).name(), "fresh_id");
        assert_eq!(second.unwrap().field(0).name(), "fresh_id");
        assert_eq!(fetch_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn expected_schema_key_is_inline_and_covers_each_input() {
        let input = UnitTestExpectedSchemaKeyInput {
            adapter_type: AdapterType::Snowflake,
            model_unique_id: "model.pkg.orders",
            local_probe_shape_sql: "select 1 as local_id",
            fallback_probe_shape_sql: "select 1 as fallback_id",
            fallback_with_query_schema: true,
            serialized_overrides: "null",
        };
        let baseline = UnitTestExpectedSchemaKey::new(input);

        assert_eq!(size_of::<UnitTestExpectedSchemaKey>(), 32);
        assert_eq!(baseline, UnitTestExpectedSchemaKey::new(input));

        let changed_inputs = [
            UnitTestExpectedSchemaKeyInput {
                adapter_type: AdapterType::Bigquery,
                ..input
            },
            UnitTestExpectedSchemaKeyInput {
                model_unique_id: "model.pkg.customers",
                ..input
            },
            UnitTestExpectedSchemaKeyInput {
                local_probe_shape_sql: "select 2 as local_id",
                ..input
            },
            UnitTestExpectedSchemaKeyInput {
                fallback_probe_shape_sql: "select 2 as fallback_id",
                ..input
            },
            UnitTestExpectedSchemaKeyInput {
                fallback_with_query_schema: false,
                ..input
            },
            UnitTestExpectedSchemaKeyInput {
                serialized_overrides: r#"{"vars":{"region":"west"}}"#,
                ..input
            },
        ];

        for changed_input in changed_inputs {
            assert_ne!(baseline, UnitTestExpectedSchemaKey::new(changed_input));
        }
    }

    #[test]
    fn expected_schema_cache_infers_each_key_once() {
        let state = Arc::new(UnitTestSchemaState::default());
        let inference_count = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(8));
        let key = UnitTestExpectedSchemaKey::new(UnitTestExpectedSchemaKeyInput {
            adapter_type: AdapterType::Snowflake,
            model_unique_id: "model.pkg.orders",
            local_probe_shape_sql: "select 1 as id",
            fallback_probe_shape_sql: "select 1 as id",
            fallback_with_query_schema: true,
            serialized_overrides: "null",
        });

        let handles = (0..8)
            .map(|_| {
                let state = Arc::clone(&state);
                let inference_count = Arc::clone(&inference_count);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    state
                        .get_or_try_infer_expected_schema(key, || {
                            inference_count.fetch_add(1, Ordering::Relaxed);
                            thread::sleep(Duration::from_millis(10));
                            Ok(Arc::new(Schema::new(vec![Field::new(
                                "id",
                                DataType::Int64,
                                false,
                            )])))
                        })
                        .unwrap()
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            assert_eq!(handle.join().unwrap().field(0).name(), "id");
        }
        let stats = state.stats_snapshot();
        assert_eq!(inference_count.load(Ordering::Relaxed), 1);
        assert_eq!(stats.expected_misses, 1);
        assert_eq!(stats.expected_hits, 7);
    }

    #[test]
    fn expected_schema_cache_retries_failed_inference() {
        use dbt_common::{ErrorCode, fs_err};

        let state = UnitTestSchemaState::default();
        let key = UnitTestExpectedSchemaKey::new(UnitTestExpectedSchemaKeyInput {
            adapter_type: AdapterType::Snowflake,
            model_unique_id: "model.pkg.orders",
            local_probe_shape_sql: "select 1 as id",
            fallback_probe_shape_sql: "select 1 as id",
            fallback_with_query_schema: true,
            serialized_overrides: "null",
        });

        let first = state.get_or_try_infer_expected_schema(key, || {
            Err(fs_err!(ErrorCode::Generic, "inference failed"))
        });
        assert!(first.is_err());

        let schema = state
            .get_or_try_infer_expected_schema(key, || {
                Ok(Arc::new(Schema::new(vec![Field::new(
                    "id",
                    DataType::Int64,
                    false,
                )])))
            })
            .unwrap();

        assert_eq!(schema.field(0).name(), "id");
        let stats = state.stats_snapshot();
        assert_eq!(stats.expected_misses, 2);
        assert_eq!(stats.expected_hits, 0);
    }
}
