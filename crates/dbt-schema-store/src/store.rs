//! Filesystem-backed implementation of the schema store.
//!
//! The `dbt-schema-store` persists canonical schemas and, optionally, materialized
//! data for each dbt node.  This module contains the production implementation,
//! which understands the different entry classes (analyzed, frontier, deferred,
//! external) and maps them to their respective on-disk namespaces.

use crate::{
    CanonicalFqn, CanonicalIdentifier, DataStoreTrait, SchemaEntry, SchemaStoreResult,
    SchemaStoreTrait, parquet_cache::ParquetSchemaCache,
};
use arrow::array::RecordBatch;
use arrow_schema::{ArrowError, Schema, SchemaRef};
use bimap::BiMap;
use futures::StreamExt;
use parquet::arrow::ArrowWriter as ParquetArrowWriter;
use scc::{HashMap as SccHashMap, HashSet as SccHashSet};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, RwLock},
    time::{Duration, SystemTime},
};

type UniqueId = String;
type Timestamp = u128;

const DATA_DIR_NAME: &str = "data";
/// Epoch-append parquet dir for compile-time analyzed schemas (no TTL).
const SCHEMAS_ANALYZED_DIR: &str = "private/metadata/compile/schemas";
/// Epoch-append parquet dir for warehouse-fetched remote schemas (has TTL).
const SCHEMAS_REMOTE_DIR: &str = "private/metadata/warehouse/schemas";
const DBT_SCHEMA_ORIGIN_KEY: &str = "DBT:schema_origin";

/// Lookup key representing the origin of a schema entry.
///
/// The entry type encodes the guarantees required by the schema store:
/// * [`LookupEntry::Selected`] – models analyzed during the current invocation.
/// * [`LookupEntry::Frontier`] – sources, frontier nodes, and cross-project
///   references whose schemas come from the remote warehouse.
/// * [`LookupEntry::Deferred`] – nodes deferred to another manifest; they also
///   hydrate from remote storage.
/// * [`LookupEntry::External`] – tables outside of the project graph, discovered
///   lazily as DataFusion resolves them.
/// * [`LookupEntry::Local`] – sources with schema_origin=local, where schemas
///   are derived from YAML column definitions rather than the remote warehouse.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub enum LookupEntry {
    Selected(UniqueId),
    Frontier(CanonicalFqn),
    Deferred(CanonicalFqn),
    External(CanonicalFqn),
    Local(CanonicalFqn),
}

impl std::fmt::Display for LookupEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LookupEntry::Selected(unique_id) => write!(f, "Selected({})", unique_id),
            LookupEntry::Local(cfqn) => write!(f, "Local({})", cfqn),
            LookupEntry::Frontier(cfqn) => write!(f, "Frontier({})", cfqn),
            LookupEntry::Deferred(cfqn) => write!(f, "Deferred({})", cfqn),
            LookupEntry::External(cfqn) => write!(f, "External({})", cfqn),
        }
    }
}

/// Cached schema entry wrapper.
#[derive(Debug, Clone)]
struct SchemaEntryWrapper {
    schema_entry: OnceLock<SchemaEntry>,
    #[allow(dead_code)]
    timestamp: u128,
    from_prior_invocation: bool,
}

impl SchemaEntryWrapper {
    pub fn new(schema_entry: SchemaEntry, timestamp: u128, from_prior_invocation: bool) -> Self {
        let once_lock = OnceLock::new();
        once_lock
            .set(schema_entry)
            .expect("OnceLock should not be already set");
        Self {
            schema_entry: once_lock,
            timestamp,
            from_prior_invocation,
        }
    }

    pub fn timestamp(&self) -> u128 {
        self.timestamp
    }
}

/// Interior mutable state shared by [`SchemaStore`].
#[derive(Debug, Clone)]
#[allow(clippy::type_complexity)]
struct SchemaStoreState {
    cached_entries: SccHashMap<LookupEntry, Arc<SchemaEntryWrapper>>,
    /// Parquet-backed caches:
    /// - First cache covers compile-time analyzed schemas (Selected entries, no TTL).
    /// - Second covers warehouse-fetched remote schemas (all other entries, with TTL).
    parquet_caches: (
        Arc<RwLock<ParquetSchemaCache>>,
        Arc<RwLock<ParquetSchemaCache>>,
    ),
}

impl SchemaStoreState {
    /// Pre-populates the state with any schemas already persisted on disk.
    ///
    /// Entries older than their configured interval are considered stale and
    /// will not be registered, forcing a re-fetch.
    ///
    /// Each entry is paired with its optional refresh interval. `None` means
    /// no expiration (cached indefinitely).
    pub fn init(
        target_dir: &Path,
        entries_with_intervals: &[(LookupEntry, Option<Duration>)],
    ) -> Self {
        let cached_schemas = SccHashMap::new();

        // Build interval maps for each dir (analyzed has no TTL; remote has TTL).
        // Only snapshot Selected entries are pre-loaded from the analyzed cache at init.
        // Non-snapshot Selected entries get their schemas computed fresh during compilation;
        // pre-loading them would cause schema_store.exists() to return true prematurely,
        // preventing incremental models from fetching their warehouse schema.
        let analyzed_intervals: Vec<(String, Option<Duration>)> = entries_with_intervals
            .iter()
            .filter_map(|(entry, _)| {
                if let LookupEntry::Selected(uid) = entry {
                    if uid.starts_with("snapshot") {
                        Some((entry.to_string(), None))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        let remote_intervals: Vec<(String, Option<Duration>)> = entries_with_intervals
            .iter()
            .filter_map(|(entry, interval)| {
                if matches!(entry, LookupEntry::Selected(_)) {
                    None // selected entries live in analyzed, not remote
                } else {
                    Some((entry.to_string(), *interval))
                }
            })
            .collect();

        let analyzed_dir = target_dir.join(SCHEMAS_ANALYZED_DIR);
        let remote_dir = target_dir.join(SCHEMAS_REMOTE_DIR);

        // analyzed: key filter ON — full set of Selected uids is known at startup.
        // remote: key filter OFF — External entries are not in entries_with_intervals.
        let analyzed = ParquetSchemaCache::load(&analyzed_dir, &analyzed_intervals, false);
        let remote = ParquetSchemaCache::load(&remote_dir, &remote_intervals, true);

        // Pre-populate the SCC map so exists()/get_schema() work normally.
        // Only snapshot Selected entries and all non-Selected entries are pre-loaded.
        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis();
        for (entry, interval) in entries_with_intervals {
            if matches!(entry, LookupEntry::Frontier(_)) {
                dbt_common::tracing::dbt_emit::emit_debug_log_message(format!(
                    "Initializing schema store with entry: {:?} and interval: {:?}",
                    entry, interval
                ));
            }

            let key = entry.to_string();
            let maybe = if let LookupEntry::Selected(uid) = entry {
                if uid.starts_with("snapshot") {
                    analyzed.get(&key)
                } else {
                    continue;
                }
            } else {
                remote.get(&key)
            };
            if let Some(schema_entry) = maybe {
                let wrapper = Arc::new(SchemaEntryWrapper::new(schema_entry.clone(), now_ms, true));
                let _ = cached_schemas.upsert_sync(entry.clone(), wrapper);
            }
        }

        let parquet_caches = (
            Arc::new(RwLock::new(analyzed)),
            Arc::new(RwLock::new(remote)),
        );

        Self {
            cached_entries: cached_schemas,
            parquet_caches,
        }
    }

    /// Returns `true` if the requested lookup entry already exists on disk.
    pub fn exists(&self, entry: &LookupEntry) -> bool {
        self.cached_entries.contains_sync(entry)
    }

    /// Async equivalent of [`SchemaStoreState::exists`].
    async fn exists_async(&self, entry: &LookupEntry) -> bool {
        self.cached_entries.contains_async(entry).await
    }

    /// Ensures the given lookup entry is tracked by the cache without eagerly
    /// hydrating the underlying schema.
    pub fn try_register_entry(&self, entry: &LookupEntry) -> Option<Arc<SchemaEntryWrapper>> {
        let (_analyzed, remote) = &self.parquet_caches;
        let key = entry.to_string();
        let guard = remote.read().expect("parquet_cache lock poisoned");
        if guard.contains(&key) {
            let now_ms = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_millis();
            let schema_entry = guard.get(&key)?.clone();
            drop(guard);
            let wrapper = Arc::new(SchemaEntryWrapper::new(schema_entry, now_ms, true));
            let _ = self
                .cached_entries
                .upsert_sync(entry.clone(), wrapper.clone());
            return Some(wrapper);
        }
        None
    }

    /// Retrieves the schema from the cache.
    pub fn get_schema(&self, entry: &LookupEntry) -> Option<SchemaEntry> {
        self.cached_entries
            .read_sync(entry, |_, v| Arc::clone(v))
            .and_then(|wrapper| wrapper.schema_entry.get().cloned())
    }

    /// Async variant of [`SchemaStoreState::get_schema`].
    pub async fn get_schema_async(&self, entry: &LookupEntry) -> Option<SchemaEntry> {
        self.cached_entries
            .read_async(entry, |_, v| Arc::clone(v))
            .await
            .and_then(|wrapper| wrapper.schema_entry.get().cloned())
    }

    /// Writes the canonical schema to the parquet cache and updates the in-memory map.
    pub fn register_schema(
        &self,
        entry: &LookupEntry,
        original_schema: Option<SchemaRef>,
        schema: SchemaRef,
        overwrite: bool,
    ) -> SchemaStoreResult<SchemaEntry> {
        if !overwrite && self.exists(entry) {
            return Ok(self.get_schema(entry).expect("Entry should exist"));
        }

        let (analyzed, remote) = &self.parquet_caches;

        // For External entries, `exists()` only checks `cached_entries`, which
        // is empty for entries not in entries_with_intervals at startup (Externals).
        // Check the parquet cache directly here so `overwrite=false` honours a
        // schema loaded from a previous run's epoch file.
        if !overwrite && !matches!(entry, LookupEntry::Local(_)) {
            let cache = if matches!(entry, LookupEntry::Selected(_)) {
                analyzed
            } else {
                remote
            };
            let guard = cache.read().expect("parquet_cache lock poisoned");
            if let Some(existing) = guard.get(&entry.to_string()).cloned() {
                drop(guard);
                let now_ms = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or(Duration::ZERO)
                    .as_millis();
                let wrapper = Arc::new(SchemaEntryWrapper::new(existing.clone(), now_ms, true));
                let _ = self.cached_entries.upsert_sync(entry.clone(), wrapper);
                return Ok(existing);
            }
        }

        let schema_entry = SchemaEntry::from_sdf_arrow_schema(original_schema, schema);

        // Local entries are always re-derived from YAML column definitions at
        // startup — never persist them to the epoch files. We still insert into
        // cached_entries so the rest of the store machinery works normally.
        if !matches!(entry, LookupEntry::Local(_)) {
            let cache = if matches!(entry, LookupEntry::Selected(_)) {
                analyzed
            } else {
                remote
            };
            let mut guard = cache.write().expect("parquet_cache lock poisoned");
            guard.upsert(entry.to_string(), schema_entry.clone())?;
        }

        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis();
        let wrapper = Arc::new(SchemaEntryWrapper::new(schema_entry.clone(), now_ms, false));
        let _ = self.cached_entries.upsert_sync(entry.clone(), wrapper);
        Ok(schema_entry)
    }

    /// Evicts stale entries from the cache based on their refresh intervals.
    ///
    /// Returns the number of entries evicted.
    pub fn evict_stale_entries(
        &self,
        entries_with_intervals: &[(LookupEntry, Option<Duration>)],
    ) -> usize {
        let mut evicted_count = 0;

        for (entry, interval) in entries_with_intervals {
            // Skip entries without a refresh interval (cached indefinitely)
            if interval.is_none() {
                continue;
            }

            // Skip selected entries (they're compiled, not cached remotely)
            if matches!(entry, LookupEntry::Selected(_)) {
                continue;
            }

            // Check if entry exists in cache and if it's stale (logs if stale)
            if let Some(wrapper) = self.cached_entries.read_sync(entry, |_, v| Arc::clone(v))
                && Self::is_entry_stale(entry, wrapper.timestamp(), *interval)
            {
                self.cached_entries.remove_sync(entry);
                let (_analyzed, remote) = &self.parquet_caches;
                remote
                    .write()
                    .expect("parquet_cache lock poisoned")
                    .remove(&entry.to_string());
                evicted_count += 1;
            }
        }

        evicted_count
    }

    /// Checks if a cached entry is stale based on its timestamp and refresh interval.
    fn is_entry_stale(
        entry: &LookupEntry,
        timestamp: u128,
        refresh_interval: Option<Duration>,
    ) -> bool {
        if let Some(interval) = refresh_interval {
            let now_millis = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_millis();
            let age = Duration::from_millis(now_millis.saturating_sub(timestamp) as u64);
            if age > interval {
                dbt_common::tracing::dbt_emit::emit_debug_log_message(format!(
                    "Schema cache entry {:?} is stale (age: {:?}, refresh_interval: {:?})",
                    entry, age, interval
                ));
                return true;
            }
        }
        false
    }
}

/// Primary filesystem-backed implementation of [`SchemaStoreTrait`].
#[derive(Debug)]
pub struct SchemaStore {
    selected: BiMap<CanonicalFqn, UniqueId>,
    frontier: BiMap<CanonicalFqn, UniqueId>,
    deferred: RwLock<BiMap<CanonicalFqn, UniqueId>>,
    external: SccHashSet<CanonicalFqn>,
    local: BiMap<CanonicalFqn, UniqueId>,
    state: SchemaStoreState,
}

impl SchemaStore {
    /// Creates a new filesystem-backed schema store rooted at `cache_dir`.
    ///
    /// `refresh_intervals` maps unique_id -> refresh interval for per-source TTL.
    /// Sources not in the map or with `None` value use no expiration (cached indefinitely).
    ///
    /// `local` maps cfqn -> unique_id for sources with `schema_origin=local`.
    /// `local_schemas` contains the Arrow schemas derived from YAML column definitions.
    /// Schemas are registered during construction.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cache_dir: PathBuf,
        selected: HashMap<CanonicalFqn, UniqueId>,
        frontier: HashMap<CanonicalFqn, UniqueId>,
        local: HashMap<CanonicalFqn, UniqueId>,
        local_schemas: Vec<crate::LocalSchemaEntry>,
        refresh_intervals: HashMap<String, Option<Duration>>,
    ) -> Self {
        // Helper to get refresh interval for a unique_id
        let get_interval = |uid: &String| refresh_intervals.get(uid).copied().flatten();

        // Build entries with their refresh intervals
        let entries_with_intervals: Vec<(LookupEntry, Option<Duration>)> = selected
            .values()
            .map(|uid| (LookupEntry::Selected(uid.clone()), uid))
            .chain(
                frontier
                    .iter()
                    .map(|(cfqn, uid)| (LookupEntry::Frontier(cfqn.clone()), uid)),
            )
            .chain(
                local
                    .iter()
                    .map(|(cfqn, uid)| (LookupEntry::Local(cfqn.clone()), uid)),
            )
            .map(|(entry, uid)| (entry, get_interval(uid)))
            .collect();

        let state = SchemaStoreState::init(&cache_dir, &entries_with_intervals);

        let store = Self {
            selected: selected.into_iter().collect(),
            frontier: frontier.into_iter().collect(),
            deferred: RwLock::new(BiMap::new()),
            external: SccHashSet::new(),
            local: local.into_iter().collect(),
            state,
        };

        // Register local schemas during construction
        for ls in local_schemas {
            let entry = LookupEntry::Local(ls.cfqn.clone());
            let schema_with_origin = add_schema_origin_metadata(ls.schema.clone(), "local");
            let _ = store
                .state
                .register_schema(&entry, None, schema_with_origin, true);
        }

        store
    }

    /// Finds the [`LookupEntry`] corresponding to a canonical FQN.
    pub fn resolve_lookup_entry_by_cfqn(&self, cfqn: &CanonicalFqn) -> Option<LookupEntry> {
        if let Some(unique_id) = self.selected.get_by_left(cfqn) {
            Some(LookupEntry::Selected(unique_id.clone()))
        } else if self.local.contains_left(cfqn) {
            Some(LookupEntry::Local(cfqn.clone()))
        } else if self.frontier.contains_left(cfqn) {
            Some(LookupEntry::Frontier(cfqn.clone()))
        } else if self
            .deferred
            .read()
            .expect("deferred lock poisoned")
            .get_by_left(cfqn)
            .is_some()
        {
            Some(LookupEntry::Deferred(cfqn.clone()))
        } else if self.external.contains_sync(cfqn) {
            Some(LookupEntry::External(cfqn.clone()))
        } else {
            None
        }
    }

    /// Finds the [`LookupEntry`] corresponding to a dbt `unique_id`.
    pub fn resolve_lookup_entry_by_unique_id(&self, unique_id: &str) -> Option<LookupEntry> {
        if self.selected.contains_right(unique_id) {
            Some(LookupEntry::Selected(unique_id.to_string()))
        } else if let Some(cfqn) = self.local.get_by_right(unique_id) {
            Some(LookupEntry::Local(cfqn.clone()))
        } else if let Some(cfqn) = self.frontier.get_by_right(unique_id) {
            Some(LookupEntry::Frontier(cfqn.clone()))
        } else if self
            .deferred
            .read()
            .expect("deferred lock poisoned")
            .get_by_right(unique_id)
            .is_some()
        {
            debug_assert!(
                false,
                "Deferred entry should be found in either selected or frontier"
            );
            None
        } else {
            None
        }
    }

    /// Registers deferred nodes whose schemas must be sourced from remote storage.
    pub fn set_deferred(&self, deferred: HashMap<CanonicalFqn, UniqueId>) -> bool {
        let mut guard = self.deferred.write().expect("deferred lock poisoned");
        let mut changed = false;
        for (cfqn, uid) in deferred {
            if !guard.contains_left(&cfqn) {
                guard.insert(cfqn.clone(), uid);
                let entry = LookupEntry::Deferred(cfqn);
                self.state.try_register_entry(&entry);
                changed = true;
            }
        }
        changed
    }

    /// Evicts stale entries from the schema store cache.
    ///
    /// Returns the number of entries evicted.
    pub fn evict_stale_entries(
        &self,
        refresh_intervals: &HashMap<String, Option<Duration>>,
    ) -> usize {
        use std::time::Duration;

        let get_interval = |uid: &String| refresh_intervals.get(uid).copied().flatten();

        let entries_with_intervals: Vec<(LookupEntry, Option<Duration>)> = self
            .selected
            .iter()
            .map(|(_, uid)| (LookupEntry::Selected(uid.clone()), uid))
            .chain(
                self.frontier
                    .iter()
                    .map(|(cfqn, uid)| (LookupEntry::Frontier(cfqn.clone()), uid)),
            )
            .chain(
                self.local
                    .iter()
                    .map(|(cfqn, uid)| (LookupEntry::Local(cfqn.clone()), uid)),
            )
            .map(|(entry, uid)| (entry, get_interval(uid)))
            .collect();

        self.state.evict_stale_entries(&entries_with_intervals)
    }

    fn visit_cfqn<F>(&self, mut f: F)
    where
        F: FnMut(&CanonicalFqn),
    {
        for (cfqn, _) in self.selected.iter() {
            f(cfqn);
        }
        for (cfqn, _) in self.frontier.iter() {
            f(cfqn);
        }
        for (cfqn, _) in self.deferred.read().expect("deferred lock poisoned").iter() {
            f(cfqn);
        }
        self.external.iter_sync(|cfqn| {
            f(cfqn);
            true
        });
        for (cfqn, _) in self.local.iter() {
            f(cfqn);
        }
    }

    /// Checks if a schema exists for a specific lookup entry type.
    pub fn exists_by_lookup(&self, entry: &LookupEntry) -> bool {
        self.state.exists(entry)
    }

    /// Re-registers a `Selected` schema as a `Frontier` entry in the remote cache.
    ///
    /// Called after a local model executes so downstream models in the same run
    /// can find the upstream schema as a Frontier cache hit.
    pub fn promote_to_frontier(&self, cfqn: &CanonicalFqn) -> SchemaStoreResult<()> {
        let (_analyzed, remote) = &self.state.parquet_caches;

        let selected_entry = match self.selected.get_by_left(cfqn) {
            Some(uid) => LookupEntry::Selected(uid.clone()),
            None => return Ok(()), // not a selected node; nothing to promote
        };

        let schema_entry = match self.state.get_schema(&selected_entry) {
            Some(s) => s,
            None => return Ok(()), // schema not yet registered; nothing to promote
        };

        let frontier_entry = LookupEntry::Frontier(cfqn.clone());
        let frontier_key = frontier_entry.to_string();
        remote
            .write()
            .expect("remote cache lock poisoned")
            .upsert(frontier_key, schema_entry.clone())?;

        // Also insert into cached_entries so exists()/get_schema() see the entry
        // immediately within the same run.
        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis();
        let wrapper = Arc::new(SchemaEntryWrapper::new(schema_entry, now_ms, false));
        let _ = self
            .state
            .cached_entries
            .upsert_sync(frontier_entry, wrapper);

        Ok(())
    }

    /// Flushes all in-memory parquet-cache entries to disk as new epoch files.
    pub fn save(&self, target_dir: &Path) -> SchemaStoreResult<()> {
        let (analyzed, remote) = &self.state.parquet_caches;

        let mut analyzed_guard = analyzed.write().expect("analyzed cache lock poisoned");
        if !analyzed_guard.is_empty() {
            let analyzed_dir = target_dir.join(SCHEMAS_ANALYZED_DIR);
            analyzed_guard.save_to(&analyzed_dir)?;
        }
        drop(analyzed_guard);

        let mut remote_guard = remote.write().expect("remote cache lock poisoned");
        if !remote_guard.is_empty() {
            let remote_dir = target_dir.join(SCHEMAS_REMOTE_DIR);
            remote_guard.save_to(&remote_dir)?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SchemaStoreTrait for SchemaStore {
    fn exists(&self, cfqn: &CanonicalFqn) -> bool {
        let entry = self.resolve_lookup_entry_by_cfqn(cfqn);
        entry.as_ref().is_some_and(|entry| self.state.exists(entry))
    }

    async fn exists_async(&self, cfqn: &CanonicalFqn) -> bool {
        if let Some(entry) = self.resolve_lookup_entry_by_cfqn(cfqn) {
            self.state.exists_async(&entry).await
        } else {
            false
        }
    }

    fn exists_by_unique_id(&self, unique_id: &str) -> bool {
        self.resolve_lookup_entry_by_unique_id(unique_id)
            .is_some_and(|entry| self.state.exists(&entry))
    }

    fn get_schema(&self, cfqn: &CanonicalFqn) -> Option<SchemaEntry> {
        let entry = self.resolve_lookup_entry_by_cfqn(cfqn)?;
        self.state.get_schema(&entry)
    }

    async fn get_schema_async(&self, cfqn: &CanonicalFqn) -> Option<SchemaEntry> {
        let entry = self.resolve_lookup_entry_by_cfqn(cfqn)?;
        self.state.get_schema_async(&entry).await
    }

    fn schema_is_from_prior_invocation(&self, cfqn: &CanonicalFqn) -> bool {
        let Some(entry) = self.resolve_lookup_entry_by_cfqn(cfqn) else {
            return false;
        };
        self.state
            .cached_entries
            .read_sync(&entry, |_, wrapper| wrapper.from_prior_invocation)
            .unwrap_or(false)
    }

    fn get_schema_by_unique_id(&self, unique_id: &str) -> Option<SchemaEntry> {
        let entry = self.resolve_lookup_entry_by_unique_id(unique_id)?;
        self.state.get_schema(&entry)
    }

    async fn get_schema_by_unique_id_async(&self, unique_id: &str) -> Option<SchemaEntry> {
        let entry = self.resolve_lookup_entry_by_unique_id(unique_id)?;
        self.state.get_schema_async(&entry).await
    }

    fn register_schema(
        &self,
        cfqn: &CanonicalFqn,
        original_schema: Option<SchemaRef>,
        schema: SchemaRef,
        overwrite: bool,
    ) -> SchemaStoreResult<SchemaEntry> {
        let entry = if let Some(entry) = self.resolve_lookup_entry_by_cfqn(cfqn) {
            entry
        } else {
            LookupEntry::External(cfqn.clone())
        };
        let result = self
            .state
            .register_schema(&entry, original_schema, schema, overwrite)?;
        if let LookupEntry::External(cfqn) = entry {
            let _ = self.external.insert_sync(cfqn);
        }
        Ok(result)
    }

    fn promote_to_frontier(&self, cfqn: &CanonicalFqn) -> SchemaStoreResult<()> {
        self.promote_to_frontier(cfqn)
    }

    fn catalog_names(&self) -> Vec<CanonicalIdentifier> {
        let mut catalogs = BTreeSet::new();
        self.visit_cfqn(|cfqn| {
            catalogs.insert(cfqn.catalog().clone());
        });
        catalogs.into_iter().collect()
    }

    fn schema_names(&self, catalog: &CanonicalIdentifier) -> Vec<CanonicalIdentifier> {
        let mut schemas = BTreeSet::new();
        self.visit_cfqn(|cfqn| {
            if cfqn.catalog() == catalog {
                schemas.insert(cfqn.schema().clone());
            }
        });
        schemas.into_iter().collect()
    }

    fn table_names(
        &self,
        catalog: &CanonicalIdentifier,
        schema: &CanonicalIdentifier,
    ) -> Vec<CanonicalIdentifier> {
        let mut tables = BTreeSet::new();
        self.visit_cfqn(|cfqn| {
            if cfqn.catalog() == catalog && cfqn.schema() == schema {
                tables.insert(cfqn.table().clone());
            }
        });
        tables.into_iter().collect()
    }
}

#[derive(Debug, Clone)]
pub struct DataStore {
    store_dir: PathBuf,
}

impl DataStore {
    pub fn new(target_dir: PathBuf) -> Self {
        let store_dir = target_dir.join(DATA_DIR_NAME);
        Self { store_dir }
    }
}

#[async_trait::async_trait]
impl DataStoreTrait for DataStore {
    fn persist_data(
        &self,
        cfqn: &CanonicalFqn,
        schema: SchemaRef,
        batches: Vec<RecordBatch>,
    ) -> SchemaStoreResult<usize> {
        let path = self.get_path_to_data(cfqn);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| {
            ArrowError::IoError(format!("Failed to create directory: {}", path.display()), e)
        })?;
        persist_data_as_parquet_file(schema, true, batches, &path)
    }

    async fn persist_data_async(
        &self,
        cfqn: &CanonicalFqn,
        schema: SchemaRef,
        stream: std::pin::Pin<
            Box<dyn futures::Stream<Item = SchemaStoreResult<RecordBatch>> + Send + 'static>,
        >,
    ) -> SchemaStoreResult<usize> {
        let path = self.get_path_to_data(cfqn);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| {
            ArrowError::IoError(format!("Failed to create directory: {}", path.display()), e)
        })?;
        persist_data_as_parquet_file_async(schema, true, stream, &path).await
    }

    fn get_path_to_data(&self, cfqn: &CanonicalFqn) -> PathBuf {
        // XXX: Normalize to lowercase to ensure case-insensitive lookups work on
        // case-sensitive filesystems. Using file paths to encode case sensitivity is volatile
        self.store_dir
            .join(cfqn.catalog().to_ascii_lowercase())
            .join(cfqn.schema().to_ascii_lowercase())
            .join(cfqn.table().to_ascii_lowercase())
            .join("output.parquet")
    }
}

fn make_parquet_writer(
    schema: SchemaRef,
    delete_on_error: bool,
    output_path: &Path,
) -> SchemaStoreResult<parquet::arrow::ArrowWriter<std::fs::File>> {
    let parquet_file = std::fs::File::create(output_path).map_err(|e| {
        ArrowError::IoError(
            format!("Failed to create file: {}", output_path.display()),
            e,
        )
    })?;
    match ParquetArrowWriter::try_new(parquet_file, schema, None) {
        Ok(writer) => Ok(writer),
        Err(e) => {
            if delete_on_error {
                std::fs::remove_file(output_path).map_err(|e| {
                    ArrowError::IoError(
                        format!("Failed to remove file: {}", output_path.display()),
                        e,
                    )
                })?;
            }
            Err(ArrowError::ParquetError(format!(
                "Failed to create ParquetArrowWriter: {}",
                e
            )))
        }
    }
}

/// Adds schema origin metadata to an Arrow schema.
fn add_schema_origin_metadata(schema: SchemaRef, origin: &str) -> SchemaRef {
    let mut metadata: HashMap<String, String> = schema.metadata().clone();
    metadata.insert(DBT_SCHEMA_ORIGIN_KEY.to_string(), origin.to_string());
    Arc::new(Schema::new_with_metadata(schema.fields().clone(), metadata))
}

/// Writes the provided record batches to disk using the canonical schema.
fn persist_data_as_parquet_file(
    schema: SchemaRef,
    delete_on_error: bool,
    batches: Vec<RecordBatch>,
    output_path: &Path,
) -> SchemaStoreResult<usize> {
    let mut parquet_writer = make_parquet_writer(schema, delete_on_error, output_path)?;
    let mut num_rows = 0;
    for batch in batches {
        num_rows += batch.num_rows();
        parquet_writer.write(&batch)?;
    }
    parquet_writer.close().map_err(|e| {
        ArrowError::ParquetError(format!(
            "Failed to close ParquetArrowWriter at {}: {}",
            output_path.display(),
            e,
        ))
    })?;
    Ok(num_rows)
}

/// Async variant of [`persist_data_as_parquet_file`].
async fn persist_data_as_parquet_file_async(
    schema: SchemaRef,
    delete_on_error: bool,
    mut stream: std::pin::Pin<
        Box<dyn futures::Stream<Item = SchemaStoreResult<RecordBatch>> + Send + 'static>,
    >,
    output_path: &Path,
) -> SchemaStoreResult<usize> {
    let mut parquet_writer = make_parquet_writer(schema, delete_on_error, output_path)?;
    let mut num_rows = 0;
    while let Some(res) = stream.next().await {
        let batch = match res {
            Ok(batch) => batch,
            Err(e) => {
                parquet_writer.close().map_err(|e| {
                    ArrowError::ParquetError(format!(
                        "Failed to close ParquetArrowWriter at {}: {}",
                        output_path.display(),
                        e,
                    ))
                })?;
                return Err(ArrowError::ParquetError(format!(
                    "Failed to read record batch: {}",
                    e,
                )));
            }
        };
        num_rows += batch.num_rows();
        parquet_writer.write(&batch)?;
    }
    parquet_writer.close().map_err(|e| {
        ArrowError::ParquetError(format!(
            "Failed to close ParquetArrowWriter at {}: {}",
            output_path.display(),
            e,
        ))
    })?;
    Ok(num_rows)
}

/// Reads the Arrow schema embedded in a parquet file and returns it as a
/// [`SchemaEntry`] along with the file's modification timestamp (millis since
/// epoch).
pub fn read_cached_schema_from_parquet(
    table_path: &Path,
) -> SchemaStoreResult<(SchemaEntry, Timestamp)> {
    use parquet::arrow::arrow_reader::{ArrowReaderOptions, ParquetRecordBatchReaderBuilder};

    let file = std::fs::File::open(table_path).map_err(|e| {
        ArrowError::IoError(format!("Failed to open file: {}", table_path.display()), e)
    })?;
    let options = ArrowReaderOptions::new().with_skip_arrow_metadata(false);
    let reader_builder = ParquetRecordBatchReaderBuilder::try_new_with_options(file, options)?;
    let arrow_schema = reader_builder.schema().clone();

    let timestamp = std::fs::metadata(table_path)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| SystemTime::now())
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_millis();

    Ok((
        SchemaEntry::from_sdf_arrow_schema(None, arrow_schema),
        timestamp,
    ))
}
