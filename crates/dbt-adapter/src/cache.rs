use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dashmap::DashMap;
use dbt_common::tracing::span_info::SpanStatusRecorder as _;
use dbt_common::{FsResult, create_info_span};
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_telemetry::GenericOpExecuted;
use parking_lot::RwLock;
use tracing::Instrument as _;

use crate::Adapter;
use crate::{
    metadata::{CatalogAndSchema, RELATION_CACHE_OP_ID, RelationVec},
    relation::BaseRelationConfig,
};

type RelationCacheKey = String;

/// Fully qualified key for an entry in the cache: `(schema_key, relation_key)`.
pub(crate) type DepKey = (String, String);

#[derive(Debug, Clone)]
pub struct RelationCacheEntry {
    relation: Arc<dyn BaseRelation>,
    relation_config: Option<Arc<dyn BaseRelationConfig>>,
}

impl RelationCacheEntry {
    pub fn new(
        relation: Arc<dyn BaseRelation>,
        relation_config: Option<Arc<dyn BaseRelationConfig>>,
    ) -> Self {
        Self {
            relation,
            relation_config,
        }
    }

    pub fn relation(&self) -> Arc<dyn BaseRelation> {
        self.relation.clone()
    }

    pub fn relation_config(&self) -> Option<Arc<dyn BaseRelationConfig>> {
        self.relation_config.clone()
    }
}

#[derive(Debug, Clone, Default)]
struct SchemaEntry {
    relations: DashMap<RelationCacheKey, RelationCacheEntry>,
    // Tracks whether or not we have complete knowledge of this schema
    is_complete: bool,
}

/// A dialect agnostic cache of [RelationCacheEntry]
///
/// # Example
/// ```rust
/// use std::sync::Arc;
/// use dbt_schemas::schemas::relations::base::BaseRelation;
/// use dbt_adapter::cache::{RelationCache, RelationCacheEntry};
///
/// let cache = RelationCache::new();
/// let relation: Arc<dyn BaseRelation> = // ... some relation
///
/// // Insert relation into cache
/// cache.insert_relation(relation.clone());
///
/// // Retrieve cached relation
/// let cached: Option<RelationCacheEntry> = cache.get_relation(relation);
/// ```
#[derive(Debug, Clone, Default)]
pub struct RelationCache {
    // This structure loosely represents remote warehouse state
    // Outer key represents a database schema
    //
    // Schema Key -> SchemaEntry
    //               Relation Key -> Cache Entry (Relation + Relation Config)
    // The inner key is a unique key generated from a relation's fully qualified name
    // We also differentiate using [SchemaEntry] to see what information we actually know about that schema
    schemas_and_relations: DashMap<String, SchemaEntry>,
    /// Per-node set of dependents: `parent_to_children[k]` is the set of relations
    /// that would be cascade-dropped if `k` were dropped. Lifted out of
    /// `RelationCacheEntry` so the cache entry stays Clone-via-derive and
    /// graph state lives in one place. All mutation goes through `graph_lock`.
    graph: Arc<RwLock<RelationGraph>>,
    /// Whether the project defines any function (UDF) nodes. Set once, before
    /// the run starts.
    project_has_functions: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
struct RelationGraph {
    parent_to_children: HashMap<DepKey, HashSet<DepKey>>,
}

impl RelationCache {
    pub fn set_project_has_functions(&self, value: bool) {
        self.project_has_functions.store(value, Ordering::Relaxed);
    }

    pub fn project_has_functions(&self) -> bool {
        self.project_has_functions.load(Ordering::Relaxed)
    }

    /// Retrieves a cached entry by relation
    pub fn get_relation(&self, relation: &dyn BaseRelation) -> Option<RelationCacheEntry> {
        let (schema_key, relation_key) = Self::get_relation_cache_keys(relation);
        if let Some(schema) = self.schemas_and_relations.get(&schema_key) {
            schema
                .relations
                .get(&relation_key)
                .map(|r| r.value().clone())
        } else {
            None
        }
    }

    /// Inserts a relation of [Arc<dyn BaseRelation] along with an optional <Arc<dyn BaseRelationConfig>> if applicable
    pub fn insert_relation(
        &self,
        relation: Arc<dyn BaseRelation>,
        relation_config: Option<Arc<dyn BaseRelationConfig>>,
    ) -> Option<RelationCacheEntry> {
        let (schema_key, relation_key) = Self::get_relation_cache_keys(relation.as_ref());
        let entry = RelationCacheEntry::new(relation, relation_config);
        self.schemas_and_relations
            .entry(schema_key)
            .or_default()
            .relations
            .insert(relation_key, entry)
    }

    /// Removes and returns a cached entry by key
    fn evict(&self, schema_key: &str, key: &str) -> Option<RelationCacheEntry> {
        // Schema Read Guard -> Inner Delete
        // We do not need to lock reads to the schema as the read guard is sufficient
        if let Some(relations) = self.schemas_and_relations.get(schema_key) {
            relations
                .value()
                .relations
                .remove(key)
                .map(|(_key, value)| value)
        } else {
            None
        }
    }

    /// Removes and returns a cached entry by relation
    pub fn evict_relation(&self, relation: &dyn BaseRelation) -> Option<RelationCacheEntry> {
        let (schema_key, relation_key) = Self::get_relation_cache_keys(relation);
        self.evict(&schema_key, &relation_key)
    }

    /// Inserts a schema and its relations into the cache
    pub fn insert_schema(&self, schema: CatalogAndSchema, relations: RelationVec) {
        let schema_key = schema.to_string();
        let cached_relations: DashMap<_, _> = relations
            .iter()
            .map(|r| {
                let key = Self::normalize_relation_key(&schema_key, r.as_ref());
                (key, RelationCacheEntry::new(r.clone(), None))
            })
            .collect();

        self.schemas_and_relations.insert(
            schema_key,
            SchemaEntry {
                relations: cached_relations,
                is_complete: true,
            },
        );
    }

    pub fn insert_many(
        &self,
        to_insert: impl IntoIterator<Item = (CatalogAndSchema, Vec<Arc<dyn BaseRelation>>)>,
    ) {
        to_insert
            .into_iter()
            .for_each(|(schema, relations)| self.insert_schema(schema, relations));
    }

    /// Apply a batch of `(referenced, dependent)` dependency edges. Pairs whose
    /// referenced schema is uncached or whose referenced relation isn't in the
    /// cache are silently skipped — see [`add_link`].
    pub fn add_links<I>(&self, links: I)
    where
        I: IntoIterator<Item = (Arc<dyn BaseRelation>, Arc<dyn BaseRelation>)>,
    {
        for (referenced, dependent) in links {
            self.add_link(referenced.as_ref(), dependent.as_ref());
        }
    }

    pub fn evict_schema_for_relation(&self, relation: &dyn BaseRelation) {
        let schema_key = Self::get_schema_cache_key_from_relation(relation);
        self.schemas_and_relations.remove(&schema_key);
    }

    /// Checks if the entire schema was cached
    ///
    /// If relation provided does not contain catalog/database and schema information
    /// this function will always return false
    pub fn contains_full_schema_for_relation(&self, relation: &dyn BaseRelation) -> bool {
        self.schemas_and_relations
            .get(&Self::get_schema_cache_key_from_relation(relation))
            .map(|entry| entry.is_complete)
            .unwrap_or(false)
    }

    pub fn contains_full_schema(&self, schema: &CatalogAndSchema) -> bool {
        self.schemas_and_relations
            .get(&schema.to_string())
            .map(|entry| entry.is_complete)
            .unwrap_or(false)
    }

    pub fn contains_relation(&self, relation: &dyn BaseRelation) -> bool {
        let (schema_key, relation_key) = Self::get_relation_cache_keys(relation);
        if let Some(relation_cache) = self.schemas_and_relations.get(&schema_key) {
            relation_cache.value().relations.contains_key(&relation_key)
        } else {
            false
        }
    }

    /// Rename, preserving the renamed entry's incoming-dependency set and
    /// rewriting `old_key → new_key` in every other entry's set so cascade
    /// traversal still sees the renamed node. Mirrors Python
    /// `RelationsCache.rename`.
    pub fn rename_relation(
        &self,
        old: &dyn BaseRelation,
        new: Arc<dyn BaseRelation>,
    ) -> Option<RelationCacheEntry> {
        let mut graph = self.graph.write();

        let (old_schema_key, old_relation_key) = Self::get_relation_cache_keys(old);
        let (new_schema_key, new_relation_key) = Self::get_relation_cache_keys(new.as_ref());

        let old_dep_key: DepKey = (old_schema_key.clone(), old_relation_key.clone());
        let new_dep_key: DepKey = (new_schema_key, new_relation_key);

        if old_dep_key == new_dep_key {
            return self.get_entry_by_dep_key(&old_dep_key);
        }

        let original_entry = self.evict(&old_schema_key, &old_relation_key)?;
        let new_entry = RelationCacheEntry::new(new, original_entry.relation_config.clone());

        // Move the renamed node's incoming-edge set to its new key.
        if let Some(refs) = graph.parent_to_children.remove(&old_dep_key) {
            graph.parent_to_children.insert(new_dep_key.clone(), refs);
        }
        // Rewrite old → new in every other node's dependents.
        for refs in graph.parent_to_children.values_mut() {
            if refs.remove(&old_dep_key) {
                refs.insert(new_dep_key.clone());
            }
        }

        self.schemas_and_relations
            .entry(new_dep_key.0)
            .or_default()
            .relations
            .insert(new_dep_key.1, new_entry.clone());

        Some(new_entry)
    }

    /// Record that `dependent` depends on `referenced`. No-op if
    /// `referenced`'s schema isn't cached (external relation) or if
    /// `referenced` itself isn't in the cache.
    pub fn add_link(&self, referenced: &dyn BaseRelation, dependent: &dyn BaseRelation) {
        let (ref_schema_key, ref_relation_key) = Self::get_relation_cache_keys(referenced);

        if !self.schemas_and_relations.contains_key(&ref_schema_key) {
            return;
        }

        let ref_key = (ref_schema_key, ref_relation_key);
        if !self.contains_dep_key(&ref_key) {
            return;
        }

        let (dep_schema_key, dep_relation_key) = Self::get_relation_cache_keys(dependent);

        self.graph
            .write()
            .parent_to_children
            .entry(ref_key)
            .or_default()
            .insert((dep_schema_key, dep_relation_key));
    }

    /// Cascade-drop `relation` and every transitive dependent. Returns the
    /// set of entries removed (including `relation` itself).
    pub fn drop_relation_cascade(&self, relation: &dyn BaseRelation) -> HashSet<DepKey> {
        let mut graph = self.graph.write();

        let (schema_key, relation_key) = Self::get_relation_cache_keys(relation);
        let start: DepKey = (schema_key, relation_key);

        if self.get_entry_by_dep_key(&start).is_none() {
            return HashSet::new();
        }

        let consequences = collect_consequences(&graph.parent_to_children, start);
        for key in &consequences {
            self.evict(&key.0, &key.1);
            graph.parent_to_children.remove(key);
        }
        for refs in graph.parent_to_children.values_mut() {
            for key in &consequences {
                refs.remove(key);
            }
        }

        consequences
    }

    fn get_entry_by_dep_key(&self, key: &DepKey) -> Option<RelationCacheEntry> {
        let schema_entry = self.schemas_and_relations.get(&key.0)?;
        Some(schema_entry.relations.get(&key.1)?.value().clone())
    }

    fn contains_dep_key(&self, key: &DepKey) -> bool {
        self.schemas_and_relations
            .get(&key.0)
            .is_some_and(|s| s.relations.contains_key(&key.1))
    }

    #[cfg(test)]
    pub(crate) fn get_children(&self, relation: &dyn BaseRelation) -> Vec<DepKey> {
        let (s, r) = Self::get_relation_cache_keys(relation);
        self.graph
            .read()
            .parent_to_children
            .get(&(s, r))
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Removes all entries from the cache
    pub fn clear(&self) {
        self.schemas_and_relations.clear();
    }

    /// Number of total relations cached
    pub fn num_relations(&self) -> usize {
        self.schemas_and_relations
            .iter()
            .map(|entry| entry.value().relations.len())
            .sum()
    }

    /// Number of total schemas cached
    pub fn num_schemas(&self) -> usize {
        self.schemas_and_relations
            .iter()
            .filter(|entry| entry.value().is_complete)
            .count()
    }

    /// Helper: Generates cache key pairs from a [BaseRelation]
    fn get_relation_cache_keys(relation: &dyn BaseRelation) -> (String, String) {
        (
            Self::get_schema_cache_key_from_relation(relation),
            Self::get_relation_cache_key_from_relation(relation),
        )
    }

    /// Helper: Generates a relation cache key from a [BaseRelation]
    fn get_relation_cache_key_from_relation(relation: &dyn BaseRelation) -> String {
        relation.semantic_fqn()
    }

    /// Helper: Generates a schema cache key from a [BaseRelation]
    fn get_schema_cache_key_from_relation(relation: &dyn BaseRelation) -> String {
        CatalogAndSchema::from(relation).to_string()
    }

    /// Helper: Generates a normalized relation key by substituting the relation's own schema
    /// prefix with the provided `schema_key`.
    ///
    /// This is needed when the warehouse normalizes identifier casing (e.g. Databricks stores
    /// all schema names in lowercase in `information_schema`). The caller holds the
    /// user-specified `schema_key` (which may be uppercase), while the relation returned by
    /// the warehouse carries the warehouse-normalized (lowercase) schema. Replacing the prefix
    /// ensures that later lookups via the caller's `CatalogAndSchema` key find the relation.
    fn normalize_relation_key(schema_key: &str, relation: &dyn BaseRelation) -> String {
        let relation_schema_key = Self::get_schema_cache_key_from_relation(relation);
        let relation_fqn = Self::get_relation_cache_key_from_relation(relation);
        if relation_fqn.starts_with(&relation_schema_key) {
            format!(
                "{}{}",
                schema_key,
                &relation_fqn[relation_schema_key.len()..]
            )
        } else {
            relation_fqn
        }
    }
}

/// Hydrates the relation cache of the given [Adapter]
/// The [Adapter] must support [MetadataAdapter] as well
///
/// Takes in an input of placeholder relations from which to hydrate schemas for
/// if they are not already cached.
///
/// `display_action` names this call's progress indicator; each caller uses a
/// distinct label.
pub async fn hydrate_relation_cache_if_not_already_cached(
    dummy_relations: &[Arc<dyn BaseRelation>],
    adapter: &Arc<Adapter>,
    display_action: &str,
) -> FsResult<()> {
    if adapter.metadata_adapter().is_none() {
        return Ok(());
    }
    // Calculate the set of schemas to hydrate
    let cache_misses: Vec<CatalogAndSchema> = {
        let relation_cache = adapter.engine().relation_cache();
        let mut seen: BTreeSet<_> = BTreeSet::new();
        dummy_relations
            .iter()
            .filter_map(|r| {
                let schema = CatalogAndSchema::from(r);
                if seen.contains(&schema) || relation_cache.contains_full_schema(&schema) {
                    None
                } else {
                    seen.insert(schema.clone());
                    Some(schema)
                }
            })
            .collect()
    };
    if cache_misses.is_empty() {
        return Ok(());
    }

    // Adapters whose `supports_relation_progress()` is `false` (unimplemented stubs)
    // never emit per-item spans; advertising a count for them would leave the bar
    // stuck at 0/N.
    let item_count_total = adapter
        .metadata_adapter()
        .is_some_and(|m| m.supports_relation_progress())
        .then_some(cache_misses.len() as u64);

    let span = create_info_span(GenericOpExecuted::new(
        RELATION_CACHE_OP_ID.to_string(),
        display_action.to_string(),
        item_count_total,
    ));

    adapter
        .hydrate_relation_cache(&cache_misses)
        .instrument(span.clone())
        .await
        .record_status(&span)
}

#[inline]
fn collect_consequences(
    parent_to_children: &HashMap<DepKey, HashSet<DepKey>>,
    start: DepKey,
) -> HashSet<DepKey> {
    let mut visited = HashSet::from([start.clone()]);
    let mut queue = VecDeque::from([start]);
    while let Some(key) = queue.pop_front() {
        if let Some(children) = parent_to_children.get(&key) {
            for child in children {
                if visited.insert(child.clone()) {
                    queue.push_back(child.clone());
                }
            }
        }
    }
    visited
}

#[cfg(test)]
mod tests {
    use crate::AdapterType;

    use super::*;
    use dbt_schemas::schemas::{common::ResolvedQuoting, relations::DEFAULT_RESOLVED_QUOTING};

    #[test]
    fn test_different_key_creation() {
        use crate::relation::do_create_relation;

        let cache = RelationCache::default();

        // Create relations with different combinations of database, schema, identifier
        let relation1: Arc<dyn BaseRelation> = do_create_relation(
            AdapterType::Postgres,
            "db1".to_string(),
            "schema1".to_string(),
            Some("table1".to_string()),
            None,
            DEFAULT_RESOLVED_QUOTING,
        )
        .unwrap()
        .into();

        let relation2: Arc<dyn BaseRelation> = do_create_relation(
            AdapterType::Postgres,
            "db2".to_string(),
            "schema1".to_string(),
            Some("table1".to_string()),
            None,
            DEFAULT_RESOLVED_QUOTING,
        )
        .unwrap()
        .into();

        let relation3: Arc<dyn BaseRelation> = do_create_relation(
            AdapterType::Postgres,
            "db1".to_string(),
            "schema2".to_string(),
            Some("table1".to_string()),
            None,
            DEFAULT_RESOLVED_QUOTING,
        )
        .unwrap()
        .into();

        let relation4: Arc<dyn BaseRelation> = do_create_relation(
            AdapterType::Postgres,
            "db1".to_string(),
            "schema1".to_string(),
            Some("table2".to_string()),
            None,
            DEFAULT_RESOLVED_QUOTING,
        )
        .unwrap()
        .into();

        let relation1_dup: Arc<dyn BaseRelation> = do_create_relation(
            AdapterType::Postgres,
            "db1".to_string(),
            "schema1".to_string(),
            Some("table1".to_string()),
            None,
            DEFAULT_RESOLVED_QUOTING,
        )
        .unwrap()
        .into();

        // Insert relations into cache
        cache.insert_relation(relation1.clone(), None);
        cache.insert_relation(relation2.clone(), None);
        cache.insert_relation(relation3.clone(), None);
        cache.insert_relation(relation4.clone(), None);
        cache.insert_relation(relation1_dup.clone(), None);

        // Verify all different relations are cached separately
        assert!(cache.contains_relation(relation1.as_ref()));
        assert!(cache.contains_relation(relation2.as_ref()));
        assert!(cache.contains_relation(relation3.as_ref()));
        assert!(cache.contains_relation(relation4.as_ref()));

        // Verify cache keys are different
        let key1 = RelationCache::get_relation_cache_key_from_relation(relation1.as_ref());
        let key2 = RelationCache::get_relation_cache_key_from_relation(relation2.as_ref());
        let key3 = RelationCache::get_relation_cache_key_from_relation(relation3.as_ref());
        let key4 = RelationCache::get_relation_cache_key_from_relation(relation4.as_ref());
        let key5 = RelationCache::get_relation_cache_key_from_relation(relation1_dup.as_ref());

        // Different relations should have different keys
        assert_ne!(key1, key2);
        assert_ne!(key1, key3);
        assert_ne!(key1, key4);
        assert_ne!(key2, key3);
        assert_ne!(key2, key4);
        assert_ne!(key3, key4);

        // Same relation should have same key
        assert_eq!(key1, key5);
    }

    #[test]
    fn test_quoting_policy_affects_cache_keys() {
        use crate::relation::do_create_relation;

        let cache = RelationCache::default();

        let relation_quoted: Arc<dyn BaseRelation> = do_create_relation(
            AdapterType::Postgres,
            "MyDB".to_string(),
            "MySchema".to_string(),
            Some("MyTable".to_string()),
            None,
            DEFAULT_RESOLVED_QUOTING,
        )
        .unwrap()
        .into();

        // With no quoting
        let relation_unquoted: Arc<dyn BaseRelation> = do_create_relation(
            AdapterType::Postgres,
            "MyDB".to_string(),
            "MySchema".to_string(),
            Some("MyTable".to_string()),
            None,
            ResolvedQuoting {
                database: false,
                schema: false,
                identifier: false,
            },
        )
        .unwrap()
        .into();

        let key_quoted =
            RelationCache::get_relation_cache_key_from_relation(relation_quoted.as_ref());
        let key_unquoted =
            RelationCache::get_relation_cache_key_from_relation(relation_unquoted.as_ref());

        // Cache keys should be different due to quoting policy affecting normalization
        // This is intentional! Quoting enforces different semantics within dialects
        // Relations created with identical quoting configs should result in cache hits
        assert_ne!(key_quoted, key_unquoted);

        cache.insert_relation(relation_quoted.clone(), None);
        cache.insert_relation(relation_unquoted.clone(), None);

        // Both should exist as separate entries
        assert!(cache.contains_relation(relation_quoted.as_ref()));
        assert!(cache.contains_relation(relation_unquoted.as_ref()));

        // Test that we find the unquoted relation when searching with unquoted policy
        let search_relation_unquoted: Arc<dyn BaseRelation> = do_create_relation(
            AdapterType::Postgres,
            "MyDB".to_string(),
            "MySchema".to_string(),
            Some("MyTable".to_string()),
            None,
            ResolvedQuoting {
                database: false,
                schema: false,
                identifier: false,
            },
        )
        .unwrap()
        .into();

        let found_unquoted_entry = cache.get_relation(search_relation_unquoted.as_ref());
        assert!(found_unquoted_entry.is_some());
    }

    #[test]
    fn test_concurrent_mixed_operations_no_race_condition() {
        use crate::metadata::CatalogAndSchema;
        use crate::relation::do_create_relation;
        use std::sync::Arc;
        use std::thread;

        let cache = Arc::new(RelationCache::default());
        let num_threads = 8;
        let operations_per_thread = 50;

        let relations: Vec<Arc<dyn BaseRelation>> = (0..operations_per_thread)
            .flat_map(|i| {
                // Create relations in 3 different schemas
                (0..3).map(move |schema_id| {
                    do_create_relation(
                        AdapterType::Postgres,
                        "test_db".to_string(),
                        format!("schema_{schema_id}"),
                        Some(format!("table_{schema_id}_{i}")),
                        None,
                        DEFAULT_RESOLVED_QUOTING,
                    )
                    .unwrap()
                    .into()
                })
            })
            .collect();

        let relations = Arc::new(relations);

        let handles: Vec<_> = (0..num_threads)
            .map(|thread_id| {
                let cache = cache.clone();
                let relations = relations.clone();

                thread::spawn(move || {
                    for i in 0..operations_per_thread {
                        let relation_idx =
                            (thread_id * operations_per_thread + i) % relations.len();
                        let relation = &relations[relation_idx];

                        match i % 7 {
                            0 => {
                                // Individual relation insert
                                cache.insert_relation(Arc::clone(relation), None);
                            }
                            1 => {
                                // Individual relation evict
                                cache.evict_relation(relation.as_ref());
                            }
                            2 => {
                                // Schema hydration
                                let schema = CatalogAndSchema::from(relation);
                                let schema_relations: Vec<_> = relations
                                    .iter()
                                    .filter(|r| CatalogAndSchema::from(*r) == schema)
                                    .cloned()
                                    .collect();
                                cache.insert_schema(schema, schema_relations);
                            }
                            3 => {
                                // Schema eviction
                                cache.evict_schema_for_relation(relation.as_ref());
                            }
                            4 => {
                                // Read operations (most common in real usage)
                                cache.contains_relation(relation.as_ref());
                                cache.get_relation(relation.as_ref());
                            }
                            5 => {
                                // Schema checks
                                cache.contains_full_schema_for_relation(relation.as_ref());
                            }
                            6 => {
                                // Rename operations (less common but important)
                                let new_relation = do_create_relation(
                                    AdapterType::Postgres,
                                    "test_db".to_string(),
                                    format!("schema_{}", thread_id % 3),
                                    Some(format!("renamed_table_{thread_id}_{i}")),
                                    None,
                                    DEFAULT_RESOLVED_QUOTING,
                                )
                                .map(Arc::from)
                                .unwrap();
                                cache.rename_relation(relation.as_ref(), new_relation);
                            }
                            _ => unreachable!(),
                        }

                        // Occasionally clear everything (stress test)
                        if i % 25 == 0 && thread_id == 0 {
                            cache.clear();
                        }
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        // Verify consistency after all operations
        for relation in relations.iter() {
            let contains = cache.contains_relation(relation.as_ref());
            let get_result = cache.get_relation(relation.as_ref());

            // consistency check: if contains says it exists, it must actually exist!
            if contains {
                assert!(
                    get_result.is_some(),
                    "Cache inconsistency: contains_relation=true but get_relation=None for relation: {:?}",
                    relation.semantic_fqn()
                );
            }

            // Schema-level consistency
            let schema_exists = cache.contains_full_schema_for_relation(relation.as_ref());
            if schema_exists && contains {
                assert!(
                    get_result.is_some(),
                    "Schema exists and relation exists, but get_relation failed"
                );
            }
        }
        // Test survived all concurrent operations without panicking or corrupting
    }

    /// Regression test for https://github.com/dbt-labs/dbt-fusion/issues/943
    ///
    /// When schema name contains uppercase letters on Databricks, the second `dbtf seed`
    /// run fails with TABLE_OR_VIEW_ALREADY_EXISTS.
    ///
    /// Root cause: `insert_schema` stores relations under the outer schema key derived
    /// from the caller's (user-specified, uppercase) `CatalogAndSchema`, but the inner
    /// relation keys come from `semantic_fqn()` of the warehouse-returned relations
    /// (which have lowercase schema, as Databricks normalises names to lowercase).
    /// The outer key matches on the next lookup (so `is_complete = true`), but the
    /// inner key doesn't — `get_relation_value_from_cache` therefore returns
    /// `none_value()` ("table doesn't exist"), causing the seed materializer to attempt
    /// CREATE TABLE on an already-existing table.
    #[test]
    fn test_databricks_uppercase_schema_seed_second_run_repro() {
        use crate::metadata::CatalogAndSchema;
        use crate::relation::do_create_relation;

        let cache = RelationCache::default();

        // The user specifies schema "NOTLIKETHIS" (uppercase) in their dbt project.
        let user_relation: Arc<dyn BaseRelation> = do_create_relation(
            AdapterType::Databricks,
            "my_catalog".to_string(),
            "NOTLIKETHIS".to_string(),
            Some("my_seed".to_string()),
            None,
            DEFAULT_RESOLVED_QUOTING,
        )
        .unwrap()
        .into();

        // Databricks returns schema names in lowercase from information_schema.tables.
        let db_relation: Arc<dyn BaseRelation> = do_create_relation(
            AdapterType::Databricks,
            "my_catalog".to_string(),
            "notlikethis".to_string(),
            Some("my_seed".to_string()),
            None,
            DEFAULT_RESOLVED_QUOTING,
        )
        .unwrap()
        .into();

        // Simulates `update_relation_cache`: insert_schema is called with the
        // CatalogAndSchema derived from the user relation (uppercase schema key)
        // but with the warehouse-returned relation (lowercase schema in semantic_fqn).
        let schema = CatalogAndSchema::from(user_relation.as_ref());
        cache.insert_schema(schema, vec![db_relation]);

        // The schema is marked as complete (is_complete = true), so the cache believes
        // it has full knowledge of this schema.
        assert!(cache.contains_full_schema_for_relation(user_relation.as_ref()));

        // The relation must be findable via the user-specified (uppercase) relation.
        // Without the fix this assertion fails: the inner relation key ("NOTLIKETHIS")
        // doesn't match the DB-returned key ("notlikethis"), so get_relation returns
        // None while contains_full_schema says the schema is complete — causing
        // get_relation_value_from_cache to return none_value() instead of the relation.
        assert!(
            cache.contains_relation(user_relation.as_ref()),
            "issue #943: uppercase schema 'NOTLIKETHIS' should find the relation that \
             Databricks returned with lowercase schema 'notlikethis'"
        );
    }

    #[test]
    fn test_project_has_functions_defaults_false_and_is_settable() {
        let cache = RelationCache::default();
        assert!(!cache.project_has_functions());
        cache.set_project_has_functions(true);
        assert!(cache.project_has_functions());
    }

    #[test]
    fn test_insert_relation_into_complete_schema() {
        use crate::metadata::CatalogAndSchema;
        use crate::relation::do_create_relation;

        let bq_relation = |identifier: &str| -> Arc<dyn BaseRelation> {
            do_create_relation(
                AdapterType::Bigquery,
                "my_project".to_string(),
                "my_dataset".to_string(),
                Some(identifier.to_string()),
                None,
                DEFAULT_RESOLVED_QUOTING,
            )
            .unwrap()
            .into()
        };

        let cache = RelationCache::default();

        let table = bq_relation("my_table");
        cache.insert_schema(CatalogAndSchema::from(table.as_ref()), vec![table.clone()]);

        let udf = bq_relation("add_one");
        assert!(cache.contains_full_schema_for_relation(udf.as_ref()));
        assert!(!cache.contains_relation(udf.as_ref()));

        // Listing routines afterwards makes the UDF findable...
        cache.insert_relation(udf.clone(), None);
        assert!(cache.contains_relation(udf.as_ref()));
        assert!(cache.get_relation(udf.as_ref()).is_some());

        // ...without evicting the listed relations or clearing completeness.
        assert!(cache.contains_relation(table.as_ref()));
        assert!(cache.contains_full_schema_for_relation(udf.as_ref()));
    }

    /// Helpers + tests ported from
    /// `dbt-adapters/dbt-adapters/tests/unit/test_cache.py`.
    ///
    /// Graph layout: b→a, c→b, d→b, e→a, f→d. Used to validate cascade-drop
    /// and reference-aware rename semantics in the Rust port.
    mod v1_test_cache {
        use super::*;
        use crate::relation::do_create_relation;
        use dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING;

        fn make_rel(ident: &str) -> Arc<dyn BaseRelation> {
            do_create_relation(
                AdapterType::Postgres,
                "dbt".to_string(),
                "schema".to_string(),
                Some(ident.to_string()),
                None,
                DEFAULT_RESOLVED_QUOTING,
            )
            .unwrap()
            .into()
        }

        /// Build the standard test graph: relations a..f with links
        /// b→a, c→b, d→b, e→a, f→d.
        fn build_cache() -> RelationCache {
            let cache = RelationCache::default();
            for ident in ["a", "b", "c", "d", "e", "f"] {
                cache.insert_relation(make_rel(ident), None);
            }
            cache.add_link(make_rel("a").as_ref(), make_rel("b").as_ref());
            cache.add_link(make_rel("b").as_ref(), make_rel("c").as_ref());
            cache.add_link(make_rel("b").as_ref(), make_rel("d").as_ref());
            cache.add_link(make_rel("a").as_ref(), make_rel("e").as_ref());
            cache.add_link(make_rel("d").as_ref(), make_rel("f").as_ref());
            cache
        }

        fn present_idents(cache: &RelationCache) -> BTreeSet<String> {
            ["a", "b", "c", "d", "e", "f", "b__tmp", "b__backup"]
                .into_iter()
                .filter(|ident| cache.contains_relation(make_rel(ident).as_ref()))
                .map(|s| s.to_string())
                .collect()
        }

        /// Mirrors Python `test_drop_inner`: dropping `b` cascades through
        /// b→{c,d}→f, leaving only `{a, e}`.
        #[test]
        fn test_drop_inner() {
            let cache = build_cache();
            assert_eq!(
                present_idents(&cache),
                ["a", "b", "c", "d", "e", "f"]
                    .into_iter()
                    .map(String::from)
                    .collect()
            );

            cache.drop_relation_cascade(make_rel("b").as_ref());

            assert_eq!(
                present_idents(&cache),
                ["a", "e"].into_iter().map(String::from).collect()
            );
        }

        /// Mirrors Python `test_rename_and_drop`: simulates the view
        /// materialization rename-swap sequence.
        ///
        /// 1. drop nonexistent b__backup / b__tmp (no-ops)
        /// 2. add b__tmp
        /// 3. rename b -> b__backup (preserves its referenced_by set)
        /// 4. rename b__tmp -> b (fresh, no dependents)
        /// 5. drop b__backup cascade → c, d, f gone; new b survives
        ///
        /// Final cache contains {a, b, e} and `a` is now referenced only by `e`.
        #[test]
        fn test_rename_and_drop() {
            let cache = build_cache();
            cache.drop_relation_cascade(make_rel("b__backup").as_ref());
            cache.drop_relation_cascade(make_rel("b__tmp").as_ref());

            cache.insert_relation(make_rel("b__tmp"), None);
            assert!(cache.contains_relation(make_rel("b__tmp").as_ref()));

            // Rename b -> b__backup (its referenced_by set {c, d} moves with it).
            cache.rename_relation(make_rel("b").as_ref(), make_rel("b__backup"));
            assert!(!cache.contains_relation(make_rel("b").as_ref()));
            assert!(cache.contains_relation(make_rel("b__backup").as_ref()));

            // Rename b__tmp -> b (fresh entry, no incoming refs).
            cache.rename_relation(make_rel("b__tmp").as_ref(), make_rel("b"));
            assert!(cache.contains_relation(make_rel("b").as_ref()));
            assert!(!cache.contains_relation(make_rel("b__tmp").as_ref()));

            // Drop b__backup cascade — c, d, f should follow it out.
            cache.drop_relation_cascade(make_rel("b__backup").as_ref());

            assert_eq!(
                present_idents(&cache),
                ["a", "b", "e"].into_iter().map(String::from).collect()
            );

            // Verify a's referenced_by now only contains e (b's link was severed).
            let refs = cache.get_children(make_rel("a").as_ref());
            assert_eq!(refs.len(), 1);
            let e_schema_key =
                RelationCache::get_schema_cache_key_from_relation(make_rel("e").as_ref());
            let e_relation_key =
                RelationCache::get_relation_cache_key_from_relation(make_rel("e").as_ref());
            assert_eq!(refs[0], (e_schema_key, e_relation_key));
        }

        /// Regression for the Redshift view materialization bug fixed by this
        /// patch: after model `a` runs (rename-swap + drop a__backup cascade),
        /// the cached entry for downstream view `b` must be gone — otherwise
        /// the materialization for model `b` thinks `b` exists and emits a
        /// bogus `alter table b rename to b__dbt_backup`.
        #[test]
        fn test_redshift_view_repro_a_cascade_evicts_b() {
            // Initial state: hydration cached both a and b; b refs a.
            let cache = RelationCache::default();
            cache.insert_relation(make_rel("a"), None);
            cache.insert_relation(make_rel("b"), None);
            cache.add_link(make_rel("a").as_ref(), make_rel("b").as_ref());

            // Materializer creates a__dbt_tmp.
            cache.insert_relation(make_rel("a__dbt_tmp"), None);
            // Renames a -> a__dbt_backup (carries b in its referenced_by).
            cache.rename_relation(make_rel("a").as_ref(), make_rel("a__dbt_backup"));
            // Renames a__dbt_tmp -> a (fresh entry).
            cache.rename_relation(make_rel("a__dbt_tmp").as_ref(), make_rel("a"));

            // Drop a__dbt_backup cascade — should evict the dependent view b
            // since the warehouse will also drop it via CASCADE.
            cache.drop_relation_cascade(make_rel("a__dbt_backup").as_ref());

            assert!(
                !cache.contains_relation(make_rel("b").as_ref()),
                "after drop a__dbt_backup cascade, view b must be evicted from cache"
            );
            assert!(
                cache.contains_relation(make_rel("a").as_ref()),
                "the freshly renamed a must remain in cache"
            );
        }
    }
}
