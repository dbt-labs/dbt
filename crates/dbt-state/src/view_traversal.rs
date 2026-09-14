//! Recursive, concurrent traversal of view definitions.
//!
//! `ViewDefinitionTraverser` wraps a `MetadataAdapter` and drives a BFS over
//! the dependency graph implied by view DDL. Each fetched view's SQL is
//! parsed via `SourcesExtractor::extract_upstreams` to extract upstream
//! references; new references become the next BFS frontier.
//!
//! Concurrency: in-flight fetches run as `tokio::task::JoinSet` tasks; the
//! orchestrator awaits one outcome at a time. Within a single call,
//! `seen_fqns` deduplicates queue entries; across calls, the long-lived
//! `cache` deduplicates already-resolved keys. Two concurrent calls that
//! both miss the cache for the same key may both fetch — accepted as a
//! simplification.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

use dashmap::DashMap;
use dbt_adapter::AdapterType;
use dbt_adapter::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use dbt_adapter::metadata::{MetadataAdapter, ViewDefinition};
use dbt_common::cancellation::{Cancellable, CancellationToken};
use dbt_frontend_common::FullyQualifiedName;
use dbt_frontend_common::sources_extractor::SourcesExtractor;
use dbt_schemas::schemas::relations::base::BaseRelation;
use tokio::task::JoinSet;
use tracing::Instrument as _;

/// The result of a recursive view-definition traversal.
#[derive(Debug, Clone)]
pub struct ViewTraversalResult {
    /// Mapping from a fully-qualified view name to the fetched definition.
    /// All keys are quoted, qualified strings produced by
    /// `BaseRelation::semantic_fqn()`.
    pub view_definitions: BTreeMap<String, ViewDefinition>,

    /// All fully-qualified names that were visited during traversal but are
    /// *not* views — e.g. base tables, system metadata objects, or objects
    /// for which the adapter could not retrieve a DDL (missing / permission
    /// denied). These are the leaves of the dependency graph.
    pub seen_tables: BTreeSet<String>,

    /// `BaseRelation` for each entry of `seen_tables`. Callers that need to
    /// fetch `last_modified_epoch` for these leaves (so out-of-band data
    /// changes get reflected in the run-cache request) use this to avoid
    /// re-parsing FQN strings into relations.
    ///
    /// Keyed by `BaseRelation::semantic_fqn()` — the canonical scheme used
    /// by the run-cache request builder's `parser_seen_relations` map.
    pub leaf_relations: BTreeMap<String, Arc<dyn BaseRelation>>,

    /// A subset of `seen_tables` whose view definitions could not be fetched.
    pub unresolvable_tables: BTreeSet<String>,
}

/// Drives recursive, concurrent traversal of view definitions across a
/// `MetadataAdapter`.
///
/// Owns the long-lived deduplication cache. Each call to `traverse()` reuses
/// the cache, so a view fetched on one call is not re-fetched on a later one.
///
/// A traverser is `MetadataAdapter`-agnostic — pass any adapter that
/// implements `fetch_view_definitions_inner`. Adapters that haven't been
/// ported yet will surface `NotSupported` from the first fetch call.
pub struct ViewDefinitionTraverser {
    /// The metadata adapter used to fetch view definitions.
    adapter: Arc<dyn MetadataAdapter>,

    sources_extractor: Arc<dyn SourcesExtractor>,

    /// Long-lived deduplication cache. Lives for the lifetime of the
    /// traverser; reused across every `traverse()` call.
    cache: Arc<DashMap<String, CachedViewLookup>>,
}

#[derive(Clone, Debug)]
enum CachedViewLookup {
    View(Arc<ViewDefinition>),
    NotView,
    Unresolvable,
}

type FetchTaskResult = (Vec<Arc<ViewDefinition>>, BTreeSet<String>);

impl ViewDefinitionTraverser {
    /// Create a traverser bound to the given adapter, with an empty cache.
    pub fn new(
        adapter: Arc<dyn MetadataAdapter>,
        sources_extractor: Arc<dyn SourcesExtractor>,
    ) -> Self {
        Self {
            adapter,
            sources_extractor,
            cache: Arc::new(DashMap::new()),
        }
    }

    /// Insert a known `ViewDefinition` directly into the cache, bypassing
    /// any remote fetch. The cache is keyed by `view_def.fqn`. Idempotent
    /// overwrite: replaces any prior entry for the same fqn.
    ///
    /// Intended for selected view models in the current run, whose
    /// compiled SQL is locally available and authoritative.
    pub fn insert_view_definition(&self, view_def: ViewDefinition) {
        self.cache.insert(
            view_def.fqn.clone(),
            CachedViewLookup::View(Arc::new(view_def)),
        );
    }

    /// Recursively traverse view definitions starting from the given relations.
    ///
    /// For each fetched view definition, parses its SQL via the
    /// `SourcesExtractor` to extract upstream references and continues the
    /// traversal with those as the next BFS frontier.
    ///
    /// Sequential `traverse()` calls reuse the cache, so a view fetched on one
    /// call is not re-fetched on a later one. Two concurrent calls that race
    /// on the same uncached key may both fetch.
    pub async fn traverse(
        &self,
        relations: &[Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AdapterResult<ViewTraversalResult> {
        let adapter_type = self.adapter.adapter_type();
        let mut seen_fqns: BTreeSet<String> = BTreeSet::new();
        let mut seen_relations: BTreeMap<String, Arc<dyn BaseRelation>> = BTreeMap::new();
        let mut view_definitions: BTreeMap<String, Arc<ViewDefinition>> = BTreeMap::new();
        let mut unresolvable_tables: BTreeSet<String> = BTreeSet::new();
        let mut queue: VecDeque<Arc<dyn BaseRelation>> = relations.iter().cloned().collect();
        let mut inflight: JoinSet<AdapterResult<FetchTaskResult>> = JoinSet::new();

        loop {
            // 1. Drain the queue, classifying each relation against the cache.
            let mut to_fetch: Vec<Arc<dyn BaseRelation>> = Vec::new();
            while let Some(rel) = queue.pop_front() {
                let fqn = rel.semantic_fqn();
                if !seen_fqns.insert(fqn.clone()) {
                    continue;
                }
                seen_relations
                    .entry(fqn.clone())
                    .or_insert_with(|| Arc::clone(&rel));
                let cached = self.cache.get(&fqn).map(|e| e.value().clone());
                match cached {
                    Some(CachedViewLookup::View(view_def)) => record_and_enqueue(
                        adapter_type,
                        &mut view_definitions,
                        &mut queue,
                        &seen_fqns,
                        &view_def,
                        self.sources_extractor.as_ref(),
                    )?,
                    Some(CachedViewLookup::NotView) => { /* leaf: not a view, skip. */ }
                    Some(CachedViewLookup::Unresolvable) => {
                        unresolvable_tables.insert(fqn);
                    }
                    None => to_fetch.push(rel),
                }
            }

            // 2. Submit one batched fetch task for everything not in cache.
            if !to_fetch.is_empty() {
                let adapter = self.adapter.clone();
                let cache = self.cache.clone();
                let token_clone = token.clone();
                inflight.spawn(
                    async move {
                        let fqns: Vec<String> = to_fetch.iter().map(|r| r.semantic_fqn()).collect();
                        let res = adapter.fetch_view_definitions(&to_fetch, token_clone).await;
                        match res {
                            Ok(view_defs) => {
                                let mut by_fqn: HashMap<String, ViewDefinition> = view_defs
                                    .definitions
                                    .into_iter()
                                    .map(|v| (v.fqn.clone(), v))
                                    .collect();
                                let mut fetched_views: Vec<Arc<ViewDefinition>> = Vec::new();
                                let mut unresolvable = BTreeSet::new();
                                for fqn in fqns {
                                    if let Some(view_def) = by_fqn.remove(&fqn) {
                                        let arc = Arc::new(view_def);
                                        cache.insert(fqn, CachedViewLookup::View(Arc::clone(&arc)));
                                        fetched_views.push(arc);
                                    } else if view_defs.unresolvable.contains(&fqn) {
                                        cache.insert(fqn.clone(), CachedViewLookup::Unresolvable);
                                        unresolvable.insert(fqn);
                                    } else {
                                        cache.insert(fqn, CachedViewLookup::NotView);
                                    }
                                }
                                Ok((fetched_views, unresolvable))
                            }
                            Err(cancellable) => Err(match cancellable {
                                Cancellable::Error(e) => e,
                                Cancellable::Cancelled => AdapterError::new(
                                    AdapterErrorKind::Cancelled,
                                    "fetch cancelled",
                                ),
                            }),
                        }
                    }
                    .in_current_span(),
                );
            }

            // 3. Wait for at least one in-flight task; loop until done.
            if inflight.is_empty() && queue.is_empty() {
                break;
            }
            let (views, unresolvable) = inflight
                .join_next()
                .await
                .expect("inflight non-empty due to check above")
                .map_err(|e| {
                    AdapterError::new(
                        AdapterErrorKind::Internal,
                        format!("BFS worker panicked: {e}"),
                    )
                })??;

            unresolvable_tables.extend(unresolvable);
            for v in views {
                record_and_enqueue(
                    adapter_type,
                    &mut view_definitions,
                    &mut queue,
                    &seen_fqns,
                    &v,
                    self.sources_extractor.as_ref(),
                )?;
            }
        }

        let view_keys: BTreeSet<String> = view_definitions.keys().cloned().collect();
        let seen_tables: BTreeSet<String> = seen_fqns
            .into_iter()
            .filter(|fqn| !view_keys.contains(fqn))
            .collect();
        // `seen_relations` is already keyed by `semantic_fqn()` (the BFS dedup
        // key), which matches `seen_tables` and the request builder's
        // `parser_seen_relations` convention. Filter to non-view leaves.
        let leaf_relations: BTreeMap<String, Arc<dyn BaseRelation>> = seen_relations
            .into_iter()
            .filter(|(fqn, _)| seen_tables.contains(fqn))
            .collect();
        Ok(ViewTraversalResult {
            view_definitions: view_definitions
                .into_iter()
                .map(|(k, v)| (k, (*v).clone()))
                .collect(),
            seen_tables,
            leaf_relations,
            unresolvable_tables,
        })
    }
}

/// Parse `view_def.definition` and return the qualified upstream references.
///
/// Routes through `SourcesExtractor::extract_upstreams`, using `adapter_type`
/// (which carries the dialect) and `view_def.default_catalog` /
/// `view_def.default_schema`. Returns each upstream reference as a
/// `FullyQualifiedName` (parsed catalog/schema/table parts), not a stringified
/// form.
///
/// Returning the parsed parts (rather than the stringified form) avoids
/// a round-trip mismatch: `FullyQualifiedName::Display` is a naive,
/// non-round-trippable dot-separated format like `DB.S.T`, while
/// `BaseRelation::semantic_fqn()` produces the quoted form `"DB"."S"."T"`.
/// The BFS orchestrator constructs synthetic relations directly from
/// these parts and uses `semantic_fqn()` as the cache key, ensuring
/// byte-identical keys with seed relations.
fn extract_referenced_tables(
    adapter_type: AdapterType,
    view_def: &ViewDefinition,
    sources_extractor: &dyn SourcesExtractor,
) -> AdapterResult<BTreeSet<FullyQualifiedName>> {
    sources_extractor
        .extract_upstreams(
            adapter_type,
            &view_def.definition,
            &view_def.default_catalog,
            &view_def.default_schema,
            false,
        )
        .map_err(|e| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                format!("failed to extract source tables from view DDL: {e}"),
            )
        })
        .map(|upstreams| upstreams.into_iter().map(|nr| nr.into_inner()).collect())
}

/// Record a fetched view in the result map and enqueue any newly-seen
/// upstream references onto the BFS queue.
///
/// `seen_tables` is consulted (read-only) to avoid pushing duplicates; the
/// queue-drain step is responsible for actually inserting into `seen_tables`.
fn record_and_enqueue(
    adapter_type: AdapterType,
    view_definitions: &mut BTreeMap<String, Arc<ViewDefinition>>,
    queue: &mut VecDeque<Arc<dyn BaseRelation>>,
    seen_tables: &BTreeSet<String>,
    view_def: &Arc<ViewDefinition>,
    sources_extractor: &dyn SourcesExtractor,
) -> AdapterResult<()> {
    view_definitions.insert(view_def.fqn.clone(), Arc::clone(view_def));
    let refs = extract_referenced_tables(adapter_type, view_def, sources_extractor)?;
    for ref_fqn in refs {
        // `ref_fqn` is a `FullyQualifiedName` with separate catalog/schema/table
        // identifiers. Build a synthetic `BaseRelation` from those parts so
        // the cache key derives from `semantic_fqn()`, matching seed relations.
        match synthetic_relation_from_parts(&ref_fqn, adapter_type) {
            Ok(rel) => {
                if !seen_tables.contains(&rel.semantic_fqn()) {
                    queue.push_back(rel);
                }
            }
            Err(e) => unreachable!("synthetic relation construction should not fail: {e}"),
        };
    }
    Ok(())
}

/// Build a synthetic `Arc<dyn BaseRelation>` from the parsed parts of a
/// `FullyQualifiedName` so its `semantic_fqn()` round-trips to the same
/// cache key seed relations use.
///
/// Returns the underlying `do_create_relation` error if construction fails,
/// so callers can include it in diagnostics.
fn synthetic_relation_from_parts(
    fqn: &FullyQualifiedName,
    adapter_type: AdapterType,
) -> Result<Arc<dyn BaseRelation>, minijinja::Error> {
    let q = dbt_schemas::schemas::common::ResolvedQuoting {
        database: true,
        schema: true,
        identifier: true,
    };
    let b = dbt_adapter::relation::do_create_relation(
        adapter_type,
        fqn.catalog().name().to_string(),
        fqn.schema().name().to_string(),
        Some(fqn.table().name().to_string()),
        None,
        q,
    )?;
    Ok(Arc::from(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_adapter::AdapterType;
    use dbt_adapter::errors::AdapterResult;
    use dbt_adapter::metadata::{
        CatalogAndSchema, MetadataAdapter, MetadataFreshness, RelationSchemaPair, RelationVec,
    };
    use dbt_adapter_core::ExecutionPhase;
    use dbt_common::AsyncAdapterResult;
    use dbt_common::cancellation::CancellationToken;
    use dbt_schemas::schemas::common::ResolvedQuoting;
    use dbt_schemas::schemas::legacy_catalog::{CatalogTable, ColumnMetadata};
    use dbt_schemas::schemas::relations::base::{BaseRelation, RelationPattern};
    use dbt_tasks_sa::sources_extractor::DefaultSourcesExtractor;
    use parking_lot::Mutex as PlMutex;
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    struct MockAdapter {
        graph: HashMap<String, Option<String>>,
        unresolvable: BTreeSet<String>,
        call_counts: Arc<PlMutex<HashMap<String, usize>>>,
        latency_ms: u64,
        errors: HashMap<String, String>,
    }

    impl MockAdapter {
        fn new(graph: HashMap<String, Option<String>>) -> Self {
            Self {
                graph,
                unresolvable: BTreeSet::new(),
                call_counts: Arc::new(PlMutex::new(HashMap::new())),
                latency_ms: 0,
                errors: HashMap::new(),
            }
        }

        #[allow(dead_code)]
        fn with_latency(mut self, ms: u64) -> Self {
            self.latency_ms = ms;
            self
        }

        #[allow(dead_code)]
        fn count_for(&self, fqn: &str) -> usize {
            self.call_counts.lock().get(fqn).copied().unwrap_or(0)
        }

        #[allow(dead_code)]
        fn with_error(mut self, fqn: String, msg: &str) -> Self {
            self.errors.insert(fqn, msg.to_string());
            self
        }

        fn with_unresolvable(mut self, fqn: String) -> Self {
            self.unresolvable.insert(fqn);
            self
        }
    }

    impl MetadataAdapter for MockAdapter {
        fn adapter_type(&self) -> AdapterType {
            AdapterType::Snowflake
        }

        fn build_schemas_from_stats_sql(
            &self,
            _: Arc<arrow_array::RecordBatch>,
        ) -> AdapterResult<BTreeMap<String, CatalogTable>> {
            Ok(Default::default())
        }
        fn build_columns_from_get_columns(
            &self,
            _: Arc<arrow_array::RecordBatch>,
        ) -> AdapterResult<BTreeMap<String, BTreeMap<String, ColumnMetadata>>> {
            Ok(Default::default())
        }
        fn create_schemas_if_not_exists(
            &self,
            _: &minijinja::State<'_, '_>,
            _: Vec<(String, String, String)>,
        ) -> AdapterResult<Vec<(String, String, String, AdapterResult<()>)>> {
            Ok(vec![])
        }
        fn list_relations_schemas_inner(
            &self,
            _: Option<String>,
            _: Option<ExecutionPhase>,
            _: &[Arc<dyn BaseRelation>],
            _: Option<&str>,
            _: CancellationToken,
        ) -> AsyncAdapterResult<'_, HashMap<String, AdapterResult<Arc<arrow_schema::Schema>>>>
        {
            Box::pin(async { Ok(Default::default()) })
        }
        fn list_relations_schemas_by_patterns_inner(
            &self,
            _: &[RelationPattern],
            _: CancellationToken,
        ) -> AsyncAdapterResult<'_, Vec<(String, AdapterResult<RelationSchemaPair>)>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn freshness_inner(
            &self,
            _: &[Arc<dyn BaseRelation>],
            _: CancellationToken,
        ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>> {
            Box::pin(async { Ok(Default::default()) })
        }
        fn list_relations_in_parallel_inner(
            &self,
            _: &[CatalogAndSchema],
            _: CancellationToken,
        ) -> AsyncAdapterResult<'_, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>>
        {
            Box::pin(async { Ok(Default::default()) })
        }

        fn fetch_view_definitions_inner<'a>(
            &'a self,
            relations: &'a [Arc<dyn BaseRelation>],
            _token: CancellationToken,
        ) -> AsyncAdapterResult<'a, dbt_adapter::metadata::ViewDefinitionFetchResult> {
            let latency = self.latency_ms;
            let counts = Arc::clone(&self.call_counts);
            let graph = self.graph.clone();
            let errors = self.errors.clone();
            let unresolvable = self.unresolvable.clone();
            let fqns: Vec<String> = relations.iter().map(|r| r.semantic_fqn()).collect();

            Box::pin(async move {
                if latency > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(latency)).await;
                }
                {
                    let mut counts = counts.lock();
                    for fqn in &fqns {
                        *counts.entry(fqn.clone()).or_default() += 1;
                    }
                }
                for fqn in &fqns {
                    if let Some(msg) = errors.get(fqn) {
                        return Err(Cancellable::Error(AdapterError::new(
                            AdapterErrorKind::Driver,
                            msg.clone(),
                        )));
                    }
                }
                let mut out = Vec::new();
                for fqn in &fqns {
                    if let Some(Some(sql)) = graph.get(fqn) {
                        out.push(ViewDefinition {
                            fqn: fqn.clone(),
                            definition: sql.clone(),
                            dialect: AdapterType::Snowflake,
                            default_catalog: "DB".to_string(),
                            default_schema: "S".to_string(),
                        });
                    }
                }
                let unresolvable = fqns
                    .into_iter()
                    .filter(|fqn| unresolvable.contains(fqn))
                    .collect();
                Ok(dbt_adapter::metadata::ViewDefinitionFetchResult {
                    definitions: out,
                    unresolvable,
                })
            })
        }
    }

    fn rel(name: &str) -> Arc<dyn BaseRelation> {
        let q = ResolvedQuoting {
            database: true,
            schema: true,
            identifier: true,
        };
        let b = dbt_adapter::relation::do_create_relation(
            AdapterType::Snowflake,
            "DB".to_string(),
            "S".to_string(),
            Some(name.to_string()),
            None,
            q,
        )
        .expect("create_relation");
        Arc::from(b)
    }

    #[dbt_runtime::test]
    async fn extract_referenced_tables_returns_empty_for_constant_select() {
        let v = ViewDefinition {
            fqn: r#""DB"."S"."V""#.to_string(),
            definition: "CREATE VIEW v AS SELECT 1 AS x".to_string(),
            dialect: AdapterType::Snowflake,
            default_catalog: "DB".to_string(),
            default_schema: "S".to_string(),
        };
        let refs = extract_referenced_tables(AdapterType::Snowflake, &v, &DefaultSourcesExtractor)
            .expect("parse ok");
        assert!(refs.is_empty(), "expected no upstream refs, got {refs:?}");
    }

    #[dbt_runtime::test]
    async fn extract_referenced_tables_finds_qualified_ref() {
        let v = ViewDefinition {
            fqn: r#""DB"."S"."V""#.to_string(),
            definition: "CREATE VIEW v AS SELECT * FROM upstream_t".to_string(),
            dialect: AdapterType::Snowflake,
            default_catalog: "DB".to_string(),
            default_schema: "S".to_string(),
        };
        let refs = extract_referenced_tables(AdapterType::Snowflake, &v, &DefaultSourcesExtractor)
            .expect("parse ok");
        // The unqualified `upstream_t` must come back qualified against the
        // view's default catalog/schema. Snowflake normalizes unquoted
        // identifiers to uppercase.
        assert_eq!(refs.len(), 1);
        let only = refs.into_iter().next().unwrap();
        assert_eq!(only.catalog().name(), "DB");
        assert_eq!(only.schema().name(), "S");
        assert_eq!(only.table().name(), "UPSTREAM_T");
    }

    #[dbt_runtime::test]
    async fn single_view_no_deps_yields_one_view_zero_tables() {
        let mut graph = HashMap::new();
        let v_fqn = rel("V").semantic_fqn();
        graph.insert(v_fqn.clone(), Some("SELECT 1 AS x".to_string()));

        let adapter = Arc::new(MockAdapter::new(graph)) as Arc<dyn MetadataAdapter>;
        let traverser =
            ViewDefinitionTraverser::new(Arc::clone(&adapter), Arc::new(DefaultSourcesExtractor));

        let result = traverser
            .traverse(&[rel("V")], CancellationToken::never_cancels())
            .await
            .expect("ok");

        assert_eq!(result.view_definitions.len(), 1);
        assert!(result.view_definitions.contains_key(&v_fqn));
        assert!(
            result.seen_tables.is_empty(),
            "got: {:?}",
            result.seen_tables
        );
    }

    #[dbt_runtime::test]
    async fn linear_chain_v1_to_v2_to_t1() {
        let v1 = rel("V1").semantic_fqn();
        let v2 = rel("V2").semantic_fqn();
        let t1 = rel("T1").semantic_fqn();

        let mut graph = HashMap::new();
        graph.insert(v1.clone(), Some("SELECT * FROM V2".to_string()));
        graph.insert(v2.clone(), Some("SELECT * FROM T1".to_string()));
        graph.insert(t1.clone(), None); // table, not a view

        let adapter = Arc::new(MockAdapter::new(graph)) as Arc<dyn MetadataAdapter>;
        let traverser =
            ViewDefinitionTraverser::new(Arc::clone(&adapter), Arc::new(DefaultSourcesExtractor));

        let result = traverser
            .traverse(&[rel("V1")], CancellationToken::never_cancels())
            .await
            .expect("ok");

        assert_eq!(result.view_definitions.len(), 2);
        assert!(result.view_definitions.contains_key(&v1));
        assert!(result.view_definitions.contains_key(&v2));
        assert_eq!(result.seen_tables.len(), 1);
        assert!(result.seen_tables.contains(&t1));
    }

    #[dbt_runtime::test]
    async fn synthetic_relation_round_trips_through_fqn_parts() {
        // Build a relation, parse a SQL "SELECT * FROM <its-name>", and
        // assert that the synthetic relation built from the parsed FQN parts
        // produces a `semantic_fqn()` byte-equal to the original.
        let r1 = rel("X");
        let original_fqn = r1.semantic_fqn();
        let v = ViewDefinition {
            fqn: r#""DB"."S"."V""#.to_string(),
            definition: format!("SELECT * FROM {}", original_fqn),
            dialect: AdapterType::Snowflake,
            default_catalog: "DB".to_string(),
            default_schema: "S".to_string(),
        };
        let refs = extract_referenced_tables(AdapterType::Snowflake, &v, &DefaultSourcesExtractor)
            .expect("parse ok");
        assert_eq!(refs.len(), 1);
        let parsed = refs.into_iter().next().unwrap();
        let r2 =
            synthetic_relation_from_parts(&parsed, AdapterType::Snowflake).expect("synthetic ok");
        assert_eq!(original_fqn, r2.semantic_fqn());
    }

    #[dbt_runtime::test]
    async fn diamond_t1_fetched_once() {
        let v1 = rel("V1").semantic_fqn();
        let v2 = rel("V2").semantic_fqn();
        let v3 = rel("V3").semantic_fqn();
        let t1 = rel("T1").semantic_fqn();

        let mut graph = HashMap::new();
        graph.insert(
            v1.clone(),
            Some("SELECT * FROM V2 UNION ALL SELECT * FROM V3".to_string()),
        );
        graph.insert(v2.clone(), Some("SELECT * FROM T1".to_string()));
        graph.insert(v3.clone(), Some("SELECT * FROM T1".to_string()));
        graph.insert(t1.clone(), None);

        let mock = Arc::new(MockAdapter::new(graph));
        let adapter: Arc<dyn MetadataAdapter> = mock.clone();
        let traverser = ViewDefinitionTraverser::new(adapter, Arc::new(DefaultSourcesExtractor));

        let result = traverser
            .traverse(&[rel("V1")], CancellationToken::never_cancels())
            .await
            .expect("ok");

        assert_eq!(result.view_definitions.len(), 3);
        assert!(result.seen_tables.contains(&t1));

        // T1 fetched at most once.
        assert_eq!(mock.count_for(&t1), 1, "t1 fetched more than once");
    }

    #[dbt_runtime::test]
    async fn cycle_v1_to_v2_to_v1_terminates() {
        let v1 = rel("V1").semantic_fqn();
        let v2 = rel("V2").semantic_fqn();

        let mut graph = HashMap::new();
        graph.insert(v1.clone(), Some("SELECT * FROM V2".to_string()));
        graph.insert(v2.clone(), Some("SELECT * FROM V1".to_string()));

        let mock = Arc::new(MockAdapter::new(graph));
        let adapter: Arc<dyn MetadataAdapter> = mock.clone();
        let traverser = ViewDefinitionTraverser::new(adapter, Arc::new(DefaultSourcesExtractor));

        // If the cycle isn't terminated, this test hangs forever.
        // Wrap in a timeout to fail fast.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            traverser.traverse(&[rel("V1")], CancellationToken::never_cancels()),
        )
        .await
        .expect("traverse timed out — cycle not terminated")
        .expect("traverse error");

        assert_eq!(result.view_definitions.len(), 2);
        assert_eq!(mock.count_for(&v1), 1);
        assert_eq!(mock.count_for(&v2), 1);
    }

    #[dbt_runtime::test]
    async fn cross_call_dedup() {
        let v1 = rel("V1").semantic_fqn();

        let mut graph = HashMap::new();
        graph.insert(v1.clone(), Some("SELECT 1".to_string()));

        let mock = Arc::new(MockAdapter::new(graph));
        let adapter: Arc<dyn MetadataAdapter> = mock.clone();
        let traverser = ViewDefinitionTraverser::new(adapter, Arc::new(DefaultSourcesExtractor));

        traverser
            .traverse(&[rel("V1")], CancellationToken::never_cancels())
            .await
            .expect("ok");
        traverser
            .traverse(&[rel("V1")], CancellationToken::never_cancels())
            .await
            .expect("ok");

        // V1 fetched exactly once across both calls.
        assert_eq!(mock.count_for(&v1), 1);
    }

    #[dbt_runtime::test]
    async fn unresolvable_relation_is_preserved_on_cache_hit() {
        let secure_view = rel("SECURE_VIEW").semantic_fqn();
        let graph = HashMap::new();

        let mock = Arc::new(MockAdapter::new(graph).with_unresolvable(secure_view.clone()));
        let adapter: Arc<dyn MetadataAdapter> = mock.clone();
        let traverser = ViewDefinitionTraverser::new(adapter, Arc::new(DefaultSourcesExtractor));

        let first = traverser
            .traverse(&[rel("SECURE_VIEW")], CancellationToken::never_cancels())
            .await
            .expect("first traversal");
        assert!(first.seen_tables.contains(&secure_view));
        assert!(first.unresolvable_tables.contains(&secure_view));

        let second = traverser
            .traverse(&[rel("SECURE_VIEW")], CancellationToken::never_cancels())
            .await
            .expect("second traversal");
        assert!(second.seen_tables.contains(&secure_view));
        assert!(second.unresolvable_tables.contains(&secure_view));
        assert_eq!(mock.count_for(&secure_view), 1);
    }

    #[dbt_runtime::test]
    async fn fetch_error_propagates() {
        let v1 = rel("V1").semantic_fqn();
        let mut graph = HashMap::new();
        graph.insert(v1.clone(), Some("SELECT 1".to_string()));

        let mock = Arc::new(MockAdapter::new(graph).with_error(v1.clone(), "boom"));
        let adapter: Arc<dyn MetadataAdapter> = mock.clone();
        let traverser = ViewDefinitionTraverser::new(adapter, Arc::new(DefaultSourcesExtractor));

        let err = traverser
            .traverse(&[rel("V1")], CancellationToken::never_cancels())
            .await
            .expect_err("expected error");

        assert!(err.to_string().contains("boom"), "got: {err}");
    }

    #[dbt_runtime::test]
    async fn insert_view_definition_skips_remote_fetch() {
        let v1 = rel("V1").semantic_fqn();
        let mock =
            Arc::new(MockAdapter::new(HashMap::new()).with_error(v1.clone(), "must not fetch"));
        let adapter: Arc<dyn MetadataAdapter> = mock.clone();
        let traverser = ViewDefinitionTraverser::new(adapter, Arc::new(DefaultSourcesExtractor));

        traverser.insert_view_definition(ViewDefinition {
            fqn: v1.clone(),
            definition: "SELECT 1".to_string(),
            dialect: AdapterType::Snowflake,
            default_catalog: "DB".to_string(),
            default_schema: "S".to_string(),
        });

        let result = traverser
            .traverse(&[rel("V1")], CancellationToken::never_cancels())
            .await
            .expect("ok");

        assert_eq!(result.view_definitions.len(), 1);
        assert!(result.view_definitions.contains_key(&v1));
        assert_eq!(mock.count_for(&v1), 0, "must not fetch primed view");
    }

    #[dbt_runtime::test]
    async fn insert_view_definition_overrides_existing_entry() {
        let v1 = rel("V1").semantic_fqn();
        let mut graph = HashMap::new();
        graph.insert(v1.clone(), Some("SELECT 'remote'".to_string()));

        let mock = Arc::new(MockAdapter::new(graph));
        let adapter: Arc<dyn MetadataAdapter> = mock.clone();
        let traverser = ViewDefinitionTraverser::new(adapter, Arc::new(DefaultSourcesExtractor));

        traverser
            .traverse(&[rel("V1")], CancellationToken::never_cancels())
            .await
            .expect("ok");
        assert_eq!(mock.count_for(&v1), 1);

        traverser.insert_view_definition(ViewDefinition {
            fqn: v1.clone(),
            definition: "SELECT 'compiled'".to_string(),
            dialect: AdapterType::Snowflake,
            default_catalog: "DB".to_string(),
            default_schema: "S".to_string(),
        });

        let result = traverser
            .traverse(&[rel("V1")], CancellationToken::never_cancels())
            .await
            .expect("ok");
        assert_eq!(result.view_definitions[&v1].definition, "SELECT 'compiled'");
        assert_eq!(mock.count_for(&v1), 1, "must not refetch after insert");
    }

    #[dbt_runtime::test]
    async fn insert_view_definition_supplies_upstreams_to_downstream_traverse() {
        let v = rel("V").semantic_fqn();
        let t = rel("T").semantic_fqn();
        let downstream = rel("D").semantic_fqn();

        let mut graph = HashMap::new();
        graph.insert(downstream.clone(), Some(format!("SELECT * FROM {}", v)));
        graph.insert(t.clone(), None);

        let mock = Arc::new(MockAdapter::new(graph));
        let adapter: Arc<dyn MetadataAdapter> = mock.clone();
        let traverser = ViewDefinitionTraverser::new(adapter, Arc::new(DefaultSourcesExtractor));

        traverser.insert_view_definition(ViewDefinition {
            fqn: v.clone(),
            definition: format!("SELECT * FROM {}", t),
            dialect: AdapterType::Snowflake,
            default_catalog: "DB".to_string(),
            default_schema: "S".to_string(),
        });

        let result = traverser
            .traverse(&[rel("D")], CancellationToken::never_cancels())
            .await
            .expect("ok");

        assert_eq!(result.view_definitions.len(), 2);
        assert!(result.view_definitions.contains_key(&downstream));
        assert!(result.view_definitions.contains_key(&v));
        assert!(result.seen_tables.contains(&t));
        assert_eq!(mock.count_for(&v), 0, "V must not be fetched");
        assert_eq!(mock.count_for(&t), 1, "T fetched once as leaf");
    }

    #[dbt_runtime::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_returns_promptly() {
        let v1 = rel("V1").semantic_fqn();
        let mut graph = HashMap::new();
        graph.insert(v1.clone(), Some("SELECT 1".to_string()));

        // Big latency so we have time to cancel.
        let mock = Arc::new(MockAdapter::new(graph).with_latency(2000));
        let adapter: Arc<dyn MetadataAdapter> = mock.clone();
        let traverser = ViewDefinitionTraverser::new(adapter, Arc::new(DefaultSourcesExtractor));

        // CancellationTokenSource lets us actually cancel.
        let source = dbt_common::cancellation::CancellationTokenSource::new();
        let token = source.token();
        let handle = tokio::spawn(async move { traverser.traverse(&[rel("V1")], token).await });

        // Give the task a moment to spawn the fetch, then cancel.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        source.cancel();

        let start = std::time::Instant::now();
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), handle)
            .await
            .expect("did not return after cancellation");
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "elapsed: {elapsed:?}"
        );
        let _ = result; // either Err(cancelled) or Ok depending on timing — both acceptable
    }
}
