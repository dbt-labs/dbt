//! SingleStore metadata adapter.
//!
//! Provides the schema-creation preflight (`create_schemas_if_not_exists` ->
//! `singlestore__create_schema`), per-relation schema fetch for unit tests and
//! contracts (`list_relations_schemas_inner`, via a zero-row probe), source
//! freshness (`freshness_inner`, via `information_schema.tables`), and
//! catalog parsing for `compile --write-catalog`
//! (`build_schemas_from_stats_sql` / `build_columns_from_get_columns` over the
//! RecordBatch produced by `singlestore__get_catalog`).
//!
//! Relation-cache hydration is implemented via `list_relations_in_parallel_inner`
//! using the shared MapReduce pattern with `list_relations`.

use crate::AdapterEngine;
use crate::adapter::adapter_impl::AdapterImpl;
use crate::connection::AdapterConnectionFactory;
use crate::errors::{
    AdapterError, AdapterErrorKind, AdapterResult, AsyncAdapterResult, Cancellable,
};
use crate::metadata::*;
use crate::record_batch::RecordBatchExt;
use crate::relation::do_create_relation;
use arrow_array::{
    Array, Decimal128Array, Int64Array, RecordBatch, StringArray, TimestampSecondArray,
};
use arrow_schema::Schema;
use dbt_adapter_core::{AdapterType, ExecutionPhase};
use dbt_adapter_engine::MapReduce;
use dbt_adbc::{Connection, QueryCtx};
use dbt_common::cancellation::CancellationToken;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::{
    legacy_catalog::{CatalogNodeStats, CatalogTable, ColumnMetadata, TableMetadata},
    relations::base::{BaseRelation, RelationPattern},
};
use indexmap::IndexMap;
use minijinja::State;

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::future;
use std::sync::Arc;

pub fn list_relations(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let schema = if engine.quoting().schema {
        db_schema.resolved_schema.clone()
    } else {
        db_schema.resolved_schema.to_lowercase()
    };

    let sql = format!(
        "SELECT table_schema, table_name, table_type \
         FROM information_schema.tables \
         WHERE table_schema = '{}'",
        dbt_adapter_sql::ident::escape_string_literal(&schema, AdapterType::SingleStore),
    );

    let batch = engine.execute(None, conn, ctx, &sql, token)?;

    if batch.num_rows() == 0 {
        return Ok(Vec::new());
    }

    let table_schemas = batch.column_values::<StringArray>("table_schema")?;
    let table_names = batch.column_values::<StringArray>("table_name")?;
    let table_types = batch.column_values::<StringArray>("table_type")?;

    let mut relations = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        let schema_name = table_schemas.value(i);
        let name = table_names.value(i);
        let relation_type = match table_types.value(i).to_ascii_uppercase().as_str() {
            "VIEW" => RelationType::View,
            _ => RelationType::Table,
        };

        let relation = do_create_relation(
            engine.adapter_type(),
            schema_name.to_string(),
            schema_name.to_string(),
            Some(name.to_string()),
            Some(relation_type),
            engine.quoting(),
        )
        .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, e.to_string()))?;

        relations.push(Arc::from(relation));
    }

    Ok(relations)
}

pub struct SingleStoreMetadataAdapter {
    adapter: AdapterImpl,
}

impl SingleStoreMetadataAdapter {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        let adapter = AdapterImpl::new(engine, None);
        Self { adapter }
    }
}

impl MetadataAdapter for SingleStoreMetadataAdapter {
    fn adapter_type(&self) -> AdapterType {
        self.adapter.adapter_type()
    }

    fn build_schemas_from_stats_sql(
        &self,
        stats_sql_result: Arc<RecordBatch>,
    ) -> AdapterResult<BTreeMap<String, CatalogTable>> {
        if stats_sql_result.num_rows() == 0 {
            return Ok(BTreeMap::new());
        }

        let table_catalogs = stats_sql_result.column_values::<StringArray>("table_database")?;
        let table_schemas = stats_sql_result.column_values::<StringArray>("table_schema")?;
        let table_names = stats_sql_result.column_values::<StringArray>("table_name")?;
        let data_types = stats_sql_result.column_values::<StringArray>("table_type")?;
        let comments = stats_sql_result.column_values::<StringArray>("table_comment")?;
        // SingleStore has no table_owner concept; catalog.sql returns `null as table_owner`.
        // We handle this gracefully — use empty string when the column is missing or null.
        let table_owners: Option<&StringArray> = stats_sql_result
            .column_by_name("table_owner")
            .and_then(|c| c.as_any().downcast_ref::<StringArray>());

        let mut result = BTreeMap::<String, CatalogTable>::new();

        for i in 0..table_catalogs.len() {
            let catalog = table_catalogs.value(i);
            let schema = table_schemas.value(i);
            let table = table_names.value(i);
            let data_type = data_types.value(i);
            let comment = comments.value(i);
            let owner = table_owners
                .map(|col| if col.is_null(i) { "" } else { col.value(i) })
                .unwrap_or("");

            let fully_qualified_name = format!("{catalog}.{schema}.{table}").to_lowercase();

            let entry = result.entry(fully_qualified_name.clone());

            if matches!(entry, Entry::Vacant(_)) {
                let node_metadata = TableMetadata {
                    materialization_type: data_type.to_string(),
                    schema: schema.to_string(),
                    name: table.to_string(),
                    database: Some(catalog.to_string()),
                    comment: match comment {
                        "" => None,
                        _ => Some(comment.to_string()),
                    },
                    owner: Some(owner.to_string()),
                };

                let no_stats = CatalogNodeStats {
                    id: "has_stats".to_string(),
                    label: "Has Stats?".to_string(),
                    value: serde_json::Value::Bool(false),
                    description: Some(
                        "Indicates whether there are statistics for this table".to_string(),
                    ),
                    include: false,
                };

                let node = CatalogTable {
                    metadata: node_metadata,
                    columns: IndexMap::new(),
                    stats: BTreeMap::from([("has_stats".to_string(), no_stats)]),
                    unique_id: None,
                };
                result.insert(fully_qualified_name.clone(), node);
            }
        }
        Ok(result)
    }

    fn build_columns_from_get_columns(
        &self,
        stats_sql_result: Arc<RecordBatch>,
    ) -> AdapterResult<BTreeMap<String, BTreeMap<String, ColumnMetadata>>> {
        if stats_sql_result.num_rows() == 0 {
            return Ok(BTreeMap::new());
        }

        let table_catalogs = stats_sql_result.column_values::<StringArray>("table_database")?;
        let table_schemas = stats_sql_result.column_values::<StringArray>("table_schema")?;
        let table_names = stats_sql_result.column_values::<StringArray>("table_name")?;

        let column_names = stats_sql_result.column_values::<StringArray>("column_name")?;
        let column_indices = stats_sql_result.column_values::<Decimal128Array>("column_index")?;
        let column_types = stats_sql_result.column_values::<StringArray>("column_type")?;
        let column_comments = stats_sql_result.column_values::<StringArray>("column_comment")?;

        let mut columns_by_relation = BTreeMap::new();

        for i in 0..table_catalogs.len() {
            let catalog = table_catalogs.value(i);
            let schema = table_schemas.value(i);
            let table = table_names.value(i);

            let fully_qualified_name = format!("{catalog}.{schema}.{table}").to_lowercase();

            let column_name = column_names.value(i);
            let column_index = column_indices.value(i);
            let column_type = column_types.value(i);
            let column_comment = column_comments.value(i);

            let column = ColumnMetadata {
                name: column_name.to_string(),
                index: column_index,
                data_type: column_type.to_string(),
                comment: match column_comment {
                    "" => None,
                    _ => Some(column_comment.to_string()),
                },
            };

            columns_by_relation
                .entry(fully_qualified_name.clone())
                .or_insert(BTreeMap::new())
                .insert(column_name.to_string(), column);
        }
        Ok(columns_by_relation)
    }

    fn list_relations_schemas_inner(
        &self,
        unique_id: Option<String>,
        phase: Option<ExecutionPhase>,
        relations: &[Arc<dyn BaseRelation>],
        item_span_operation_id: Option<&str>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, HashMap<String, AdapterResult<Arc<Schema>>>> {
        type Acc = HashMap<String, AdapterResult<Arc<Schema>>>;

        // SingleStore is a 2-part name system: `schema` maps to a SingleStore
        // database. We use a zero-row probe (`SELECT * FROM schema.table WHERE 0`)
        // to get the Arrow schema. The HashMap key must match `relation.semantic_fqn()`.
        let keys: Vec<(String, String)> = relations
            .iter()
            .map(|relation| (relation.semantic_fqn(), relation.render_self_as_str()))
            .collect();

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &'_ mut dyn Connection,
                          key: &(String, String)|
              -> AdapterResult<Arc<Schema>> {
            let (_semantic_fqn, sql_name) = key;
            let sql = format!("SELECT * FROM {sql_name} WHERE 0 LIMIT 0");
            let mut ctx = QueryCtx::default().with_desc("Get table schema");
            if let Some(node_id) = unique_id.clone() {
                ctx = ctx.with_node_id(&node_id);
            }
            if let Some(phase) = phase {
                ctx = ctx.with_phase(phase.as_str());
            }
            let (_, table) = adapter.query(&ctx, conn, &sql, None, token_clone.clone())?;
            Ok(table.original_record_batch().schema())
        };

        let reduce_f = |acc: &mut Acc,
                        key: (String, String),
                        schema: AdapterResult<Arc<Schema>>|
         -> Result<(), Cancellable<AdapterError>> {
            let (semantic_fqn, _sql_name) = key;
            acc.insert(semantic_fqn, schema);
            Ok(())
        };

        run_schema_cache_map_reduce(
            factory,
            keys,
            item_span_operation_id,
            map_f,
            reduce_f,
            None,
            token,
        )
    }

    fn list_relations_schemas_by_patterns_inner(
        &self,
        _patterns: &[RelationPattern],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, Vec<(String, AdapterResult<RelationSchemaPair>)>> {
        let err = AdapterError::new(
            AdapterErrorKind::NotSupported,
            "list_relations_schemas_by_patterns is not yet implemented for the SingleStore metadata adapter",
        );
        Box::pin(future::ready(Err(Cancellable::Error(err))))
    }

    fn freshness_inner(
        &self,
        relations: &[Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>> {
        if relations.is_empty() {
            return Box::pin(future::ready(Ok(BTreeMap::new())));
        }

        type Acc = BTreeMap<String, MetadataFreshness>;

        // Group relations by schema so we can batch the information_schema query per schema.
        // SingleStore: database == schema. We issue one query per schema with a WHERE IN clause.
        let mut by_schema: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for relation in relations {
            let schema = relation.schema_as_str().unwrap_or_default().to_string();
            let identifier = relation.identifier_as_str().unwrap_or_default().to_string();
            let fqn = relation.semantic_fqn();
            by_schema.entry(schema).or_default().push((identifier, fqn));
        }

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();

        // Flatten into tasks: (schema, Vec<(identifier, fqn)>)
        let tasks: Vec<(String, Vec<(String, String)>)> = by_schema.into_iter().collect();

        let map_f = move |conn: &'_ mut dyn Connection,
                          task: &(String, Vec<(String, String)>)|
              -> AdapterResult<Vec<(String, MetadataFreshness)>> {
            let (schema, table_entries) = task;
            let escaped_schema =
                dbt_adapter_sql::ident::escape_string_literal(schema, AdapterType::SingleStore);

            // Build IN clause for table names
            let table_names_in = table_entries
                .iter()
                .map(|(name, _)| {
                    format!(
                        "'{}'",
                        dbt_adapter_sql::ident::escape_string_literal(
                            name,
                            AdapterType::SingleStore
                        )
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");

            // information_schema.tables has UPDATE_TIME (NULL for tables never updated).
            // Fall back to CREATE_TIME when UPDATE_TIME is NULL.
            // TABLE_TYPE tells us if it's a view.
            let sql = format!(
                "SELECT table_name, table_type, \
                 UNIX_TIMESTAMP(COALESCE(update_time, create_time)) AS last_modified \
                 FROM information_schema.tables \
                 WHERE table_schema = '{escaped_schema}' \
                   AND table_name IN ({table_names_in})"
            );

            let ctx = QueryCtx::default().with_desc("Extracting freshness from information schema");
            let batch = adapter
                .engine()
                .execute(None, conn, &ctx, &sql, token_clone.clone())?;

            if batch.num_rows() == 0 {
                return Ok(vec![]);
            }

            let names = batch.column_values::<StringArray>("table_name")?;
            let types = batch.column_values::<StringArray>("table_type")?;
            // UNIX_TIMESTAMP returns a numeric — try as Int64 first, then fall back
            let timestamps_raw = batch.column_by_name("last_modified");

            let mut results = Vec::with_capacity(batch.num_rows());
            for i in 0..batch.num_rows() {
                let name = names.value(i);
                let is_view = types.value(i).eq_ignore_ascii_case("view");

                // Map back to semantic FQN
                let fqn = table_entries
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(name))
                    .map(|(_, fqn)| fqn.clone());

                let Some(fqn) = fqn else { continue };

                if let Some(ts_col) = timestamps_raw {
                    // Try i64 (BIGINT / INT) first
                    let ts_secs =
                        if let Some(i64_col) = ts_col.as_any().downcast_ref::<Int64Array>() {
                            if i64_col.is_null(i) {
                                continue;
                            }
                            i64_col.value(i)
                        } else if let Some(ts_col) =
                            ts_col.as_any().downcast_ref::<TimestampSecondArray>()
                        {
                            if ts_col.is_null(i) {
                                continue;
                            }
                            ts_col.value(i)
                        } else {
                            // Unknown type for timestamp column; skip this row
                            continue;
                        };

                    let freshness = MetadataFreshness::from_secs(ts_secs, is_view)?;
                    results.push((fqn, freshness));
                }
            }
            Ok(results)
        };

        let reduce_f = move |acc: &mut Acc,
                             _task: (String, Vec<(String, String)>),
                             result: AdapterResult<Vec<(String, MetadataFreshness)>>|
              -> Result<(), Cancellable<AdapterError>> {
            let rows = result.map_err(Cancellable::Error)?;
            for (fqn, freshness) in rows {
                acc.insert(fqn, freshness);
            }
            Ok(())
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(tasks), token)
    }

    fn create_schemas_if_not_exists(
        &self,
        state: &State<'_, '_>,
        catalog_schemas: Vec<(String, String, String)>,
    ) -> AdapterResult<Vec<(String, String, String, AdapterResult<()>)>> {
        create_schemas_if_not_exists(&self.adapter, self, state, catalog_schemas)
    }

    fn list_relations_in_parallel_inner(
        &self,
        db_schemas: &[CatalogAndSchema],
        token: CancellationToken,
        report_progress: bool,
    ) -> AsyncAdapterResult<'_, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>> {
        type Acc = BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>;
        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &'_ mut dyn Connection,
                          db_schema: &CatalogAndSchema|
              -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
            let query_ctx = QueryCtx::default().with_desc("list_relations_in_parallel");
            with_relation_list_item_span(
                report_progress.then_some(RELATION_CACHE_OP_ID),
                &db_schema.to_string(),
                || adapter.list_relations(None, &query_ctx, conn, db_schema, token_clone.clone()),
            )
        };

        let reduce_f = move |acc: &mut Acc,
                             db_schema: CatalogAndSchema,
                             relations: AdapterResult<Vec<Arc<dyn BaseRelation>>>|
              -> Result<(), Cancellable<AdapterError>> {
            match relations {
                Ok(relations) => {
                    acc.insert(db_schema, Ok(relations));
                    Ok(())
                }
                Err(e) => {
                    // If the schema (database) doesn't exist, treat as empty —
                    // matches the behaviour of other adapters and prevents hard
                    // failures when schemas are created lazily.
                    if e.message().contains("doesn't exist")
                        || e.message().contains("does not exist")
                        || e.message().contains("Unknown database")
                    {
                        acc.insert(db_schema, Ok(Vec::new()));
                        Ok(())
                    } else {
                        Err(Cancellable::Error(e))
                    }
                }
            }
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(db_schemas.to_vec()), token)
    }
}
