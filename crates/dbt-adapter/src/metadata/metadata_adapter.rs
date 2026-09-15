use crate::adapter::adapter_impl::AdapterImpl;
use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult, AsyncAdapterResult};
use crate::macro_exec::execute_macro;
use crate::relation::{RelationObject, create_relation, do_create_relation};
use crate::sql_types::{SdfSchema, arrow_schema_to_sdf_schema};
use crate::time_machine::{
    args_fetch_view_definitions, args_freshness, args_freshness_all_in_schema,
    args_list_relations_in_parallel, args_list_relations_schemas,
    args_list_relations_schemas_by_patterns, args_list_udfs, with_time_machine_metadata_wrapper,
};
use crate::{AdapterEngine, metadata::*};

use arrow::array::RecordBatch;
use dbt_adapter_core::ExecutionPhase;
use dbt_common::ErrorCode;
use dbt_common::cancellation::{Cancellable, CancellationToken};
use dbt_common::tracing::dbt_emit::emit_warn_log_message;

use dbt_schemas::schemas::{
    legacy_catalog::{CatalogTable, ColumnMetadata},
    relations::base::{BaseRelation, RelationPattern},
};
use dbt_schemas::state::ResolverState;
use dbt_schemas::stats::Stats;
use dbt_telemetry::NodeType;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

// XXX: we should unify relation representation as Arrow schemas across the codebase

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetadataQueryOptions {
    pub warehouse: Option<String>,
    /// Whether the adaptive broad-vs-sequential freshness prefetch is enabled.
    /// Only the Snowflake no-metadata-warehouse strategy consults it; other
    /// adapters ignore it. Defaults to `true` (adaptive on).
    pub adaptive_metadata_fetch: bool,
}

impl Default for MetadataQueryOptions {
    fn default() -> Self {
        Self {
            warehouse: None,
            adaptive_metadata_fetch: true,
        }
    }
}

/// `(parent, child)` pair for the relation cache dependency graph.
/// If a `parent` relation is `DROP ... CASCADE`, `child` would be dropped too.
pub type ParentChildPair = (Arc<dyn BaseRelation>, Arc<dyn BaseRelation>);

/// Adapter that supports metadata query.
///
/// Methods that perform I/O follow the `*_inner` pattern for transparent recording:
/// implementers override the `*_inner` methods with the real implementation, the trait's
/// public methods wrap those with recording, and call sites just use the public methods
/// without needing to know recording is happening.
pub trait MetadataAdapter: Send + Sync {
    /// The adapter type backing this metadata adapter (Snowflake, BigQuery, ...).
    /// Used by callers (e.g. `ViewDefinitionTraverser`) that need to construct
    /// dialect-shaped relations without an external mapping table.
    fn adapter_type(&self) -> AdapterType; // TODO: remove this and pass Arc
    // into ViewDefinitionTraverser instead

    fn build_schemas_from_stats_sql(
        &self,
        _: Arc<RecordBatch>,
    ) -> AdapterResult<BTreeMap<String, CatalogTable>>;

    fn build_columns_from_get_columns(
        &self,
        _: Arc<RecordBatch>,
    ) -> AdapterResult<BTreeMap<String, BTreeMap<String, ColumnMetadata>>>;

    #[allow(unused_variables)]
    fn is_permission_error(&self, e: &AdapterError) -> bool {
        #[cfg(debug_assertions)]
        {
            dbt_common::tracing::dbt_emit::println(format!(
                "is_permission_error: {:?}: {}",
                e,
                e.sqlstate()
            ));
        }
        false
    }

    fn create_relations_from_executed_nodes(
        &self,
        resolved_state: &ResolverState,
        run_stats: &Stats,
    ) -> Vec<Arc<dyn BaseRelation>> {
        let catalog_resource_types = [
            NodeType::Source,
            NodeType::Model,
            NodeType::Snapshot,
            NodeType::Seed,
        ];
        let adapter_type = resolved_state.adapter_type;

        // Collect executed nodes and their direct source dependencies
        let mut relevant_ids = BTreeSet::new();
        for stat in &run_stats.stats {
            let unique_id = &stat.unique_id;
            let Some(node) = resolved_state.nodes.get_node(unique_id) else {
                continue;
            };
            if !catalog_resource_types.contains(&node.resource_type()) {
                continue;
            }

            relevant_ids.insert(unique_id.clone());
            // Include direct source parents from the parent map
            let parents = &node.base().depends_on.nodes;
            relevant_ids.extend(parents.iter().filter(|p| p.starts_with("source.")).cloned());
        }

        relevant_ids
            .iter()
            .filter_map(|uid| resolved_state.nodes.get_node(uid))
            .map(|node| {
                create_relation(
                    adapter_type,
                    node.database(),
                    node.schema(),
                    Some(node.alias()),
                    None,
                    node.quoting(),
                )
                .expect("Failed to create relations from nodes")
                .into()
            })
            .collect()
    }

    /// Create schemas if they don't exist
    #[allow(clippy::type_complexity)]
    fn create_schemas_if_not_exists(
        &self,
        state: &State<'_, '_>,
        catalog_schemas: Vec<(String, String, String)>,
    ) -> AdapterResult<Vec<(String, String, String, AdapterResult<()>)>>;

    // =========================================================================
    // Async I/O methods - use _inner pattern for recording
    // =========================================================================

    /// List UDFs under a given set of catalog and schemas (implementation).
    ///
    /// Override this method with your adapter's implementation.
    /// Call `list_user_defined_functions` for the recorded version.
    fn list_user_defined_functions_inner(
        &self,
        _catalog_schemas: &BTreeMap<String, BTreeSet<String>>,
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, Vec<UDF>> {
        Box::pin(async move { Ok(vec![]) })
    }

    /// List UDFs under a given set of catalog and schemas.
    ///
    /// This is a provided method that wraps `list_user_defined_functions_inner`
    /// with time machine recording.
    fn list_user_defined_functions<'a>(
        &'a self,
        catalog_schemas: &'a BTreeMap<String, BTreeSet<String>>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, Vec<UDF>> {
        with_time_machine_metadata_wrapper(
            "global",
            "list_user_defined_functions",
            args_list_udfs(catalog_schemas),
            self.list_user_defined_functions_inner(catalog_schemas, token),
        )
    }

    /// List relations and their schemas (implementation).
    ///
    /// Override this method with your adapter's implementation.
    /// Call `list_relations_schemas` for the recorded version.
    fn list_relations_schemas_inner(
        &self,
        unique_id: Option<String>,
        phase: Option<ExecutionPhase>,
        relations: &[Arc<dyn BaseRelation>],
        item_span_operation_id: Option<&str>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, HashMap<String, AdapterResult<Arc<Schema>>>>;

    /// List relations and their schemas.
    ///
    /// This is a provided method that wraps `list_relations_schemas_inner`
    /// with time machine recording.
    fn list_relations_schemas<'a>(
        &'a self,
        unique_id: Option<String>,
        phase: Option<ExecutionPhase>,
        relations: &'a [Arc<dyn BaseRelation>],
        item_span_operation_id: Option<&'a str>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, HashMap<String, AdapterResult<Arc<Schema>>>> {
        let caller_id = unique_id.clone().unwrap_or_else(|| "global".to_string());
        with_time_machine_metadata_wrapper(
            caller_id,
            "list_relations_schemas",
            args_list_relations_schemas(
                unique_id.clone(),
                phase.map(|p| p.as_str().to_string()),
                relations.iter().map(|r| r.semantic_fqn()),
            ),
            self.list_relations_schemas_inner(
                unique_id,
                phase,
                relations,
                item_span_operation_id,
                token,
            ),
        )
    }

    /// Convert schemas to SDF schemas.
    ///
    /// This wraps `list_relations_schemas` and converts the result.
    fn list_relations_sdf_schemas<'a>(
        &'a self,
        engine: &'a dyn AdapterEngine,
        unique_id: Option<String>,
        phase: Option<ExecutionPhase>,
        relations: &'a [Arc<dyn BaseRelation>],
        item_span_operation_id: Option<&'a str>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, HashMap<String, AdapterResult<SdfSchema>>> {
        let future = async move {
            self.list_relations_schemas(unique_id, phase, relations, item_span_operation_id, token)
                .await
                .map(|map| {
                    map.into_iter()
                        .map(|(k, v)| {
                            let v = v.and_then(|schema| {
                                arrow_schema_to_sdf_schema(schema, engine.type_ops().as_ref())
                            });
                            (k, v)
                        })
                        .collect()
                })
        };
        Box::pin(future)
    }

    /// List relations and their schemas by patterns (implementation).
    ///
    /// Override this method with your adapter's implementation.
    /// Call `list_relations_schemas_by_patterns` for the recorded version.
    #[allow(clippy::type_complexity)]
    fn list_relations_schemas_by_patterns_inner(
        &self,
        patterns: &[RelationPattern],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, Vec<(String, AdapterResult<RelationSchemaPair>)>>;

    /// List relations and their schemas by patterns.
    ///
    /// This is a provided method that wraps `list_relations_schemas_by_patterns_inner`
    /// with time machine recording.
    #[allow(clippy::type_complexity)]
    fn list_relations_schemas_by_patterns<'a>(
        &'a self,
        patterns: &'a [RelationPattern],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, Vec<(String, AdapterResult<RelationSchemaPair>)>> {
        with_time_machine_metadata_wrapper(
            "global",
            "list_relations_schemas_by_patterns",
            args_list_relations_schemas_by_patterns(
                patterns
                    .iter()
                    .map(|p| format!("{}.{}.{}", p.database, p.schema_pattern, p.table_pattern)),
            ),
            self.list_relations_schemas_by_patterns_inner(patterns, token),
        )
    }

    /// Get freshness of relations (implementation).
    ///
    /// Override this method with your adapter's implementation.
    /// Call `freshness` for the recorded version.
    fn freshness_inner(
        &self,
        relations: &[Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>>;

    /// Get freshness of relations.
    ///
    /// This is a provided method that wraps `freshness_inner`
    /// with time machine recording.
    fn freshness<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        with_time_machine_metadata_wrapper(
            "global",
            "freshness",
            args_freshness(relations.iter().map(|r| r.semantic_fqn()), None),
            self.freshness_inner(relations, token),
        )
    }

    fn freshness_with_options<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        _options: &'a MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        self.freshness(relations, token)
    }

    /// Get freshness of relations, honoring per-relation overrides
    /// (`loaded_at_field`, `loaded_at_query`).
    ///
    /// Default implementation falls back to the bulk `freshness` path and
    /// silently ignores overrides — adapters that haven't ported the
    /// override path yet retain today's behavior. Adapters that override this
    /// method are expected to partition: bulk INFORMATION_SCHEMA query for the
    /// non-override subset, one targeted query per override (mirroring dbt-core's
    /// run-cache plugin).
    ///
    /// `overrides` is keyed by `BaseRelation::semantic_fqn()` and is a subset of
    /// the relations passed in `relations`.
    fn freshness_with_overrides<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        _overrides: &'a BTreeMap<String, FreshnessOverride>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        self.freshness(relations, token)
    }

    fn freshness_with_overrides_and_options<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        overrides: &'a BTreeMap<String, FreshnessOverride>,
        _options: &'a MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        self.freshness_with_overrides(relations, overrides, token)
    }

    /// Fetch freshness for all tables in the given schema without per-table filtering.
    ///
    /// This mirrors the plugin's
    /// `_fetch_last_modified_epochs_from_schemas_in_catalog` which uses a
    /// `table_schema IN (...)` filter rather than per-table predicates.  For
    /// large projects the per-table OR-predicate on `INFORMATION_SCHEMA.TABLES`
    /// can be slower than a plain schema dump; adapters that have validated
    /// this approach override this method (and `supports_bulk_freshness_dump`).
    ///
    /// `relations` is the subset of input relations in this (database, schema)
    /// group; adapters use `find_matching_relation` on the dump results to key
    /// the returned map by the same semantic FQN as `relation.semantic_fqn()`.
    ///
    /// The default implementation returns an empty map; it is only reached by
    /// adapters that do not implement a bulk dump, which never route through this
    /// method (see `supports_bulk_freshness_dump`).
    fn freshness_all_in_schema_inner<'a>(
        &'a self,
        _database: &'a str,
        _schema: &'a str,
        _relations: &'a [Arc<dyn BaseRelation>],
        _options: &'a MetadataQueryOptions,
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        Box::pin(async move { Ok(BTreeMap::new()) })
    }

    fn freshness_all_in_schema<'a>(
        &'a self,
        database: &'a str,
        schema: &'a str,
        relations: &'a [Arc<dyn BaseRelation>],
        options: &'a MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        with_time_machine_metadata_wrapper(
            "global",
            "freshness_all_in_schema",
            args_freshness_all_in_schema(
                database,
                schema,
                relations.iter().map(|r| r.semantic_fqn()),
                options.warehouse.clone(),
            ),
            self.freshness_all_in_schema_inner(database, schema, relations, options, token),
        )
    }

    /// Whether this adapter implements a bulk per-schema freshness dump
    /// (`freshness_all_in_schema`).
    ///
    /// Governs which single path `freshness_all_in_schemas` takes: `true` →
    /// per-schema dumps; `false` → the per-table bulk query. An adapter uses
    /// exactly the one path it supports. Must be `true` for exactly the adapters
    /// that override `freshness_all_in_schema`.
    fn supports_bulk_freshness_dump(&self) -> bool {
        false
    }

    /// Fetch freshness for the given relations using the adapter's bulk strategy.
    ///
    /// This is *the* freshness-prefetch entry point: the run-cache orchestration
    /// hands over all non-override relations (which may span several databases
    /// and schemas) and lets the adapter bulk-load their freshness in as few
    /// queries as it can. The returned map is keyed by `relation.semantic_fqn()`;
    /// relations whose freshness the bulk query did not return are simply absent
    /// (the caller caches them as unknown). Prefetch is strictly a bulk load: any
    /// relation the bulk query does not cover is resolved by the per-node path at
    /// submit time, not re-queried per-relation here.
    ///
    /// The default implementation dispatches on `supports_bulk_freshness_dump`:
    /// adapters with a bulk dump group by resolved `(database, schema)` and issue
    /// one `freshness_all_in_schema` dump per group (fail-open per group); adapters
    /// without one use the batched per-table `freshness_with_overrides_and_options`.
    /// Adapters with a warehouse-specific strategy (Snowflake) override this method.
    ///
    /// Grouping uses each relation's *resolved* (normalized-if-unquoted)
    /// database/schema so that, e.g., Snowflake's uppercase folding lines up with
    /// the `WHERE table_schema = '...'` clause `freshness_all_in_schema` builds.
    fn freshness_all_in_schemas<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        options: &'a MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
        Box::pin(async move {
            if !self.supports_bulk_freshness_dump() {
                // No bulk per-schema dump — the per-table bulk query (one batched
                // statement) is this adapter's bulk path.
                let no_overrides = BTreeMap::new();
                return self
                    .freshness_with_overrides_and_options(relations, &no_overrides, options, token)
                    .await;
            }

            let mut groups: BTreeMap<(String, String), Vec<Arc<dyn BaseRelation>>> =
                BTreeMap::new();
            for relation in relations {
                let database = relation.database_as_resolved_str().unwrap_or_default();
                let schema = relation.schema_as_resolved_str().unwrap_or_default();
                groups
                    .entry((database, schema))
                    .or_default()
                    .push(Arc::clone(relation));
            }

            let mut result: BTreeMap<String, MetadataFreshness> = BTreeMap::new();
            for ((database, schema), group) in groups {
                result.extend(
                    freshness_group_dump(self, &database, &schema, &group, options, token.clone())
                        .await?,
                );
            }
            Ok(result)
        })
    }

    /// Check whether each relation exists, keyed by semantic FQN.
    ///
    /// The default implementation uses `list_relations_in_parallel`, which is
    /// already implemented by supported metadata adapters. Adapters that
    /// cannot list relations should return an adapter error from that method;
    /// callers may treat that as fail-open.
    fn relations_exist_inner<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, bool>> {
        let db_schemas = relations
            .iter()
            .map(CatalogAndSchema::from)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        let future = async move {
            let listed = self.list_relations_in_parallel(&db_schemas, token).await?;
            let mut result = BTreeMap::new();

            for relation in relations {
                let semantic_fqn = relation.semantic_fqn();
                let catalog_schema = CatalogAndSchema::from(relation);
                let Some(schema_relations) = listed.get(&catalog_schema) else {
                    result.insert(semantic_fqn, false);
                    continue;
                };

                let schema_relations = schema_relations.as_ref().map_err(|err| {
                    Cancellable::Error(AdapterError::new(err.kind(), err.message().to_string()))
                })?;
                let exists = schema_relations
                    .iter()
                    .any(|candidate| candidate.semantic_fqn() == semantic_fqn);
                result.insert(semantic_fqn, exists);
            }

            Ok(result)
        };
        Box::pin(future)
    }

    fn relations_exist<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, bool>> {
        self.relations_exist_inner(relations, token)
    }

    fn relations_exist_with_options<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        _options: &'a MetadataQueryOptions,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, bool>> {
        self.relations_exist(relations, token)
    }

    /// List relations in the specified [CatalogAndSchema] in parallel (implementation).
    ///
    /// Override this method with your adapter's implementation.
    /// Call `list_relations_in_parallel` for the recorded version.
    fn list_relations_in_parallel_inner(
        &self,
        db_schemas: &[CatalogAndSchema],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>>;

    /// List relations in the specified [CatalogAndSchema] in parallel.
    ///
    /// This is a provided method that wraps `list_relations_in_parallel_inner`
    /// with time machine recording.
    fn list_relations_in_parallel<'a>(
        &'a self,
        db_schemas: &'a [CatalogAndSchema],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>> {
        with_time_machine_metadata_wrapper(
            "global",
            "list_relations_in_parallel",
            args_list_relations_in_parallel(
                db_schemas
                    .iter()
                    .map(|s| (s.resolved_catalog.clone(), s.resolved_schema.clone())),
            ),
            self.list_relations_in_parallel_inner(db_schemas, token),
        )
    }

    /// Fetch view definitions for a batch of fully-qualified table references.
    ///
    /// Implementations are responsible for:
    /// - Issuing a single (or minimal-count) database query for the entire batch.
    /// - Returning a `ViewDefinition` for each input that *is* a view.
    /// - Reporting unresolvable views separately from ordinary tables, so
    ///   callers can treat their freshness metadata conservatively.
    /// - Omitting ordinary tables from the result. The orchestrator caches
    ///   those omissions so they are not re-fetched.
    ///
    /// This method must be safe to call concurrently from multiple async tasks;
    /// each call acquires its own connection via the engine's connection factory.
    ///
    /// The default implementation returns `NotSupported`; adapters that
    /// support view-definition fetching override it.
    fn fetch_view_definitions_inner<'a>(
        &'a self,
        _relations: &'a [Arc<dyn BaseRelation>],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'a, ViewDefinitionFetchResult> {
        Box::pin(async {
            Err(Cancellable::Error(AdapterError::new(
                AdapterErrorKind::NotSupported,
                "fetch_view_definitions is not supported by this adapter",
            )))
        })
    }

    /// Public, time-machine-recorded wrapper around `fetch_view_definitions_inner`.
    ///
    /// Mirrors the existing `*_inner`/public pattern used by `freshness`,
    /// `list_relations_schemas`, `list_user_defined_functions`, etc.
    fn fetch_view_definitions<'a>(
        &'a self,
        relations: &'a [Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'a, ViewDefinitionFetchResult> {
        with_time_machine_metadata_wrapper(
            "global",
            "fetch_view_definitions",
            args_fetch_view_definitions(relations.iter().map(|r| r.semantic_fqn())),
            self.fetch_view_definitions_inner(relations, token),
        )
    }

    /// Returns `(referenced, dependent)` edges for the relation-cache
    /// dependency graph. Default: empty (no native pg_depend-style query).
    fn fetch_relation_dependency_links_inner<'a>(
        &'a self,
        _db_schemas: &'a [CatalogAndSchema],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'a, Vec<ParentChildPair>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// Fetch freshness for one already-grouped `(database, schema)` set of relations
/// via [`MetadataAdapter::freshness_all_in_schema`], with per-group fail-open.
///
/// Ordinary dump failures return an empty map so a single bad schema never aborts
/// the prefetch. ReplayDataMissing is propagated so callers can fall back to a
/// legacy replay event. An empty result means "unknown freshness" for the group;
/// uncovered relations are resolved by the per-node path at submit time.
pub(crate) async fn freshness_group_dump<A: MetadataAdapter + ?Sized>(
    adapter: &A,
    database: &str,
    schema: &str,
    relations: &[Arc<dyn BaseRelation>],
    options: &MetadataQueryOptions,
    token: CancellationToken,
) -> Result<BTreeMap<String, MetadataFreshness>, Cancellable<AdapterError>> {
    match adapter
        .freshness_all_in_schema(database, schema, relations, options, token)
        .await
    {
        Ok(dump) => Ok(dump),
        Err(Cancellable::Error(err)) if err.kind() == AdapterErrorKind::ReplayDataMissing => {
            Err(Cancellable::Error(err))
        }
        Err(Cancellable::Cancelled) => Err(Cancellable::Cancelled),
        // A cancelled join/query can surface as `Cancellable::Error` with kind
        // `Cancelled` instead of `Cancellable::Cancelled` — treat it the same
        // way so cancellation always stops the run instead of fail-opening.
        Err(Cancellable::Error(err)) if err.kind() == AdapterErrorKind::Cancelled => {
            Err(Cancellable::Error(err))
        }
        Err(Cancellable::Error(err)) => {
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State schema-level freshness dump failed for {database}.{schema}: {err}; \
                     omitting freshness for {} relations",
                    relations.len()
                ),
            );
            Ok(BTreeMap::new())
        }
    }
}

/// Create schemas if they don't exist
///
/// Caveat: you'll want to first use this helper to create catalogs for the schemas you're going to create
/// before using it to create schemas
#[allow(clippy::type_complexity)]
pub fn create_schemas_if_not_exists(
    adapter: &AdapterImpl,
    metadata_adapter: &dyn MetadataAdapter,
    state: &State,
    catalog_schemas: Vec<(String, String, String)>,
) -> AdapterResult<Vec<(String, String, String, AdapterResult<()>)>> {
    let map_f = |(catalog, schema, unique_id): (String, String, String)| -> AdapterResult<(String, String, String, AdapterResult<()>)> {
        let mock_relation = do_create_relation(
            adapter.adapter_type(),
            catalog.clone(),
            schema.clone(),
            None,
            None,
            adapter.quoting()
        )?;
        let res =
        match execute_macro(state, &[RelationObject::new(Arc::from(mock_relation)).into_value()], "create_schema") {
            Ok(_) => Ok(()),
            Err(e) => {
                if metadata_adapter.is_permission_error(&e) {
                    Ok(())
                } else if adapter.adapter_type() == AdapterType::Bigquery {
                    Err(e)
                } else {
                    let chars = e.sqlstate().as_bytes();
                    let sqlstate: [u8; 5] = chars[..5].try_into().map_err(|_| e.clone())?;
                    let err_string = format!(
                        "Failed to create schema '{schema}' in database '{catalog}' in remote for {unique_id}: {}", e.message()
                    );
                    return Err(AdapterError::new_with_sqlstate_and_vendor_code(e.kind(), err_string, sqlstate, e.vendor_code()));
                }
            }
        };
        Ok((catalog, schema, unique_id, res))
    };

    catalog_schemas.into_iter().map(map_f).collect()
}

pub fn flatten_catalog_schemas(
    catalog_schemas: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<(String, String)> {
    catalog_schemas
        .iter()
        .flat_map(|(catalog, schemas)| {
            schemas
                .iter()
                .map(|schema| (catalog.clone(), schema.clone()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time_machine::{
        EventReplayer, MetadataCallArgs, RecordedEvent, get_or_init_recording,
        get_or_init_replayer, reset_time_machine_globals,
    };
    use chrono::{TimeZone, Utc};
    use dbt_schemas::schemas::common::ResolvedQuoting;
    use flate2::read::GzDecoder;
    use std::io::{BufRead, BufReader};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TIME_MACHINE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct MockMetadataAdapter {
        freshness_all_in_schema_calls: AtomicUsize,
        replay_missing: bool,
    }

    impl MockMetadataAdapter {
        fn new() -> Self {
            Self {
                freshness_all_in_schema_calls: AtomicUsize::new(0),
                replay_missing: false,
            }
        }
    }

    impl MetadataAdapter for MockMetadataAdapter {
        fn adapter_type(&self) -> AdapterType {
            AdapterType::Snowflake
        }

        fn build_schemas_from_stats_sql(
            &self,
            _: Arc<RecordBatch>,
        ) -> AdapterResult<BTreeMap<String, CatalogTable>> {
            Ok(BTreeMap::new())
        }

        fn build_columns_from_get_columns(
            &self,
            _: Arc<RecordBatch>,
        ) -> AdapterResult<BTreeMap<String, BTreeMap<String, ColumnMetadata>>> {
            Ok(BTreeMap::new())
        }

        fn create_schemas_if_not_exists(
            &self,
            _: &State<'_, '_>,
            _: Vec<(String, String, String)>,
        ) -> AdapterResult<Vec<(String, String, String, AdapterResult<()>)>> {
            Ok(Vec::new())
        }

        fn list_relations_schemas_inner(
            &self,
            _: Option<String>,
            _: Option<ExecutionPhase>,
            _: &[Arc<dyn BaseRelation>],
            _: Option<&str>,
            _: CancellationToken,
        ) -> AsyncAdapterResult<'_, HashMap<String, AdapterResult<Arc<Schema>>>> {
            Box::pin(async { Ok(HashMap::new()) })
        }

        fn list_relations_schemas_by_patterns_inner(
            &self,
            _: &[RelationPattern],
            _: CancellationToken,
        ) -> AsyncAdapterResult<'_, Vec<(String, AdapterResult<RelationSchemaPair>)>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn freshness_inner(
            &self,
            _: &[Arc<dyn BaseRelation>],
            _: CancellationToken,
        ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>> {
            Box::pin(async { Ok(BTreeMap::new()) })
        }

        fn freshness_all_in_schema_inner<'a>(
            &'a self,
            _database: &'a str,
            _schema: &'a str,
            relations: &'a [Arc<dyn BaseRelation>],
            _options: &'a MetadataQueryOptions,
            _token: CancellationToken,
        ) -> AsyncAdapterResult<'a, BTreeMap<String, MetadataFreshness>> {
            Box::pin(async move {
                self.freshness_all_in_schema_calls
                    .fetch_add(1, Ordering::SeqCst);
                if self.replay_missing {
                    return Err(Cancellable::Error(AdapterError::new(
                        AdapterErrorKind::ReplayDataMissing,
                        "missing schema freshness recording",
                    )));
                }
                Ok(BTreeMap::from([(
                    relations[0].semantic_fqn(),
                    MetadataFreshness {
                        last_altered: Utc.timestamp_millis_opt(1_234_000).unwrap(),
                        is_view: false,
                    },
                )]))
            })
        }

        fn list_relations_in_parallel_inner(
            &self,
            _: &[CatalogAndSchema],
            _: CancellationToken,
        ) -> AsyncAdapterResult<'_, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>>
        {
            Box::pin(async { Ok(BTreeMap::new()) })
        }
    }

    #[dbt_runtime::test]
    async fn schema_freshness_replay_missing_is_propagated_for_legacy_fallback() {
        let _guard = TIME_MACHINE_TEST_LOCK.lock().await;
        let adapter = MockMetadataAdapter {
            replay_missing: true,
            ..MockMetadataAdapter::new()
        };
        let relation: Arc<dyn BaseRelation> = create_relation(
            AdapterType::Snowflake,
            "db".to_string(),
            "schema".to_string(),
            Some("table".to_string()),
            None,
            ResolvedQuoting::default(),
        )
        .unwrap()
        .into();

        let result = freshness_group_dump(
            &adapter,
            "db",
            "schema",
            &[relation],
            &MetadataQueryOptions::default(),
            CancellationToken::never_cancels(),
        )
        .await;
        assert!(matches!(
            result,
            Err(Cancellable::Error(error))
                if error.kind() == AdapterErrorKind::ReplayDataMissing
        ));
    }

    #[dbt_runtime::test]
    async fn schema_wide_freshness_records_and_replays_without_calling_inner() {
        let _guard = TIME_MACHINE_TEST_LOCK.lock().await;
        reset_time_machine_globals().await.unwrap();

        let dir = tempfile::tempdir().unwrap();
        let handle = get_or_init_recording(
            dir.path(),
            "snowflake",
            "test-invocation",
            None,
            CancellationToken::never_cancels(),
        );

        let relation: Arc<dyn BaseRelation> = create_relation(
            AdapterType::Snowflake,
            "db".to_string(),
            "schema".to_string(),
            Some("table".to_string()),
            None,
            ResolvedQuoting::default(),
        )
        .unwrap()
        .into();
        let relations = vec![relation];
        let adapter = MockMetadataAdapter::new();

        let recorded = adapter
            .freshness_all_in_schema(
                "db",
                "schema",
                &relations,
                &MetadataQueryOptions::default(),
                CancellationToken::never_cancels(),
            )
            .await
            .unwrap();
        assert_eq!(
            adapter.freshness_all_in_schema_calls.load(Ordering::SeqCst),
            1
        );

        handle.shutdown().await.unwrap();

        let events = read_recorded_events(dir.path());
        let RecordedEvent::MetadataCall(event) = &events[0] else {
            panic!("expected metadata event");
        };
        assert_eq!(event.caller_id, "global");
        assert_eq!(event.method, "freshness_all_in_schema");
        assert!(matches!(
            &event.args,
            MetadataCallArgs::FreshnessAllInSchema {
                database,
                schema,
                relations: args,
                warehouse,
            } if database == "db"
                && schema == "schema"
                && args == &vec![relations[0].semantic_fqn()]
                && warehouse.is_none()
        ));

        reset_time_machine_globals().await.unwrap();
        get_or_init_replayer(|| Ok(Arc::new(EventReplayer::load(dir.path())?))).unwrap();

        let replay_adapter = MockMetadataAdapter::new();
        let replayed = replay_adapter
            .freshness_all_in_schema(
                "db",
                "schema",
                &relations,
                &MetadataQueryOptions::default(),
                CancellationToken::never_cancels(),
            )
            .await
            .unwrap();

        assert_eq!(
            replay_adapter
                .freshness_all_in_schema_calls
                .load(Ordering::SeqCst),
            0
        );
        assert_eq!(recorded.len(), replayed.len());
        let key = relations[0].semantic_fqn();
        assert_eq!(recorded[&key].last_altered, replayed[&key].last_altered);
        assert_eq!(recorded[&key].is_view, replayed[&key].is_view);

        reset_time_machine_globals().await.unwrap();
    }

    fn read_recorded_events(path: &std::path::Path) -> Vec<RecordedEvent> {
        let file = std::fs::File::open(path.join("events.ndjson.gz")).unwrap();
        BufReader::new(GzDecoder::new(file))
            .lines()
            .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
            .collect()
    }
}
