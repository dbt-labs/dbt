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
pub mod sql_types;

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
    Array, Decimal128Array, Int32Array, Int64Array, RecordBatch, StringArray,
    TimestampSecondArray, UInt32Array, UInt64Array,
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

use std::collections::{BTreeMap, HashMap};
use std::future;
use std::sync::Arc;

/// Bundles the engine, query context, and cancellation token for `list_relations`
/// so the function signature stays within CodeScene's argument-count threshold.
pub struct ListRelationsCtx<'a> {
    pub engine: &'a dyn AdapterEngine,
    pub query_ctx: &'a QueryCtx,
    pub token: CancellationToken,
}

pub fn list_relations(
    ctx: &mut ListRelationsCtx<'_>,
    conn: &mut dyn Connection,
    db_schema: &CatalogAndSchema,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let raw_schema = if !db_schema.resolved_schema.is_empty() {
        &db_schema.resolved_schema
    } else {
        &db_schema.resolved_catalog
    };

    let schema = if ctx.engine.quoting().schema {
        raw_schema.clone()
    } else {
        raw_schema.to_lowercase()
    };

    let sql = format!(
        "SELECT table_schema, table_name, table_type \
         FROM information_schema.tables \
         WHERE table_schema = '{}'",
        dbt_adapter_sql::ident::escape_string_literal(&schema, AdapterType::SingleStore),
    );

    let batch = ctx
        .engine
        .execute(None, conn, ctx.query_ctx, &sql, ctx.token.clone())?;

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
            ctx.engine.adapter_type(),
            schema_name.to_string(),
            schema_name.to_string(),
            Some(name.to_string()),
            Some(relation_type),
            ctx.engine.quoting(),
        )
        .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, e.to_string()))?;

        relations.push(Arc::from(relation));
    }

    Ok(relations)
}

struct CatalogTableMeta<'a> {
    catalog: &'a str,
    schema: &'a str,
    table: &'a str,
    data_type: &'a str,
    comment: &'a str,
    owner: &'a str,
}

fn create_catalog_table(meta: CatalogTableMeta<'_>) -> CatalogTable {
    let metadata = TableMetadata {
        materialization_type: meta.data_type.to_string(),
        schema: meta.schema.to_string(),
        name: meta.table.to_string(),
        database: Some(meta.catalog.to_string()),
        comment: match meta.comment {
            "" => None,
            _ => Some(meta.comment.to_string()),
        },
        owner: Some(meta.owner.to_string()),
    };

    let no_stats = CatalogNodeStats {
        id: "has_stats".to_string(),
        label: "Has Stats?".to_string(),
        value: serde_json::Value::Bool(false),
        description: Some("Indicates whether there are statistics for this table".to_string()),
        include: false,
    };

    CatalogTable {
        metadata,
        columns: IndexMap::new(),
        stats: BTreeMap::from([("has_stats".to_string(), no_stats)]),
        unique_id: None,
    }
}

fn create_column_metadata(
    name: &str,
    index: i128,
    data_type: &str,
    comment: &str,
) -> ColumnMetadata {
    ColumnMetadata {
        name: name.to_string(),
        index,
        data_type: data_type.to_string(),
        comment: match comment {
            "" => None,
            _ => Some(comment.to_string()),
        },
    }
}

fn extract_timestamp_secs(col: &dyn Array, row: usize) -> Option<i64> {
    if col.is_null(row) {
        return None;
    }
    if let Some(i64_col) = col.as_any().downcast_ref::<Int64Array>() {
        return Some(i64_col.value(row));
    }
    if let Some(ts_col) = col.as_any().downcast_ref::<TimestampSecondArray>() {
        return Some(ts_col.value(row));
    }
    None
}

fn extract_column_index(col: &dyn Array, row: usize) -> i128 {
    if let Some(arr) = col.as_any().downcast_ref::<UInt64Array>() {
        arr.value(row) as i128
    } else if let Some(arr) = col.as_any().downcast_ref::<Int64Array>() {
        arr.value(row) as i128
    } else if let Some(arr) = col.as_any().downcast_ref::<UInt32Array>() {
        arr.value(row) as i128
    } else if let Some(arr) = col.as_any().downcast_ref::<Int32Array>() {
        arr.value(row) as i128
    } else if let Some(arr) = col.as_any().downcast_ref::<Decimal128Array>() {
        arr.value(row)
    } else {
        row as i128
    }
}

fn parse_freshness_batch(
    batch: &RecordBatch,
    table_entries: &[(String, String)],
) -> AdapterResult<Vec<(String, MetadataFreshness)>> {
    if batch.num_rows() == 0 {
        return Ok(Vec::new());
    }

    let names = batch.column_values::<StringArray>("table_name")?;
    let types = batch.column_values::<StringArray>("table_type")?;
    let timestamps_col = batch.column_by_name("last_modified");

    let mut results = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        let name = names.value(i);
        let is_view = types.value(i).eq_ignore_ascii_case("view");

        let Some((_, fqn)) = table_entries
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        else {
            continue;
        };

        let Some(ts_col) = timestamps_col else {
            continue;
        };

        let Some(ts_secs) = extract_timestamp_secs(ts_col.as_ref(), i) else {
            continue;
        };

        results.push((fqn.clone(), MetadataFreshness::from_secs(ts_secs, is_view)?));
    }

    Ok(results)
}

fn build_freshness_sql(schema: &str, table_names: &[String]) -> String {
    let escaped_schema =
        dbt_adapter_sql::ident::escape_string_literal(schema, AdapterType::SingleStore);
    let table_names_in = table_names
        .iter()
        .map(|name| {
            format!(
                "'{}'",
                dbt_adapter_sql::ident::escape_string_literal(name, AdapterType::SingleStore)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "SELECT table_name, table_type, \
         UNIX_TIMESTAMP(COALESCE(update_time, create_time)) AS last_modified \
         FROM information_schema.tables \
         WHERE table_schema = '{escaped_schema}' \
           AND table_name IN ({table_names_in})"
    )
}

fn query_schema_freshness(
    engine: &dyn AdapterEngine,
    conn: &mut dyn Connection,
    schema: &str,
    table_entries: &[(String, String)],
    token: CancellationToken,
) -> AdapterResult<Vec<(String, MetadataFreshness)>> {
    let names: Vec<String> = table_entries.iter().map(|(n, _)| n.clone()).collect();
    let sql = build_freshness_sql(schema, &names);
    let ctx = QueryCtx::default().with_desc("Extracting freshness from information schema");
    let batch = engine.execute(None, conn, &ctx, &sql, token)?;
    parse_freshness_batch(&batch, table_entries)
}

fn is_missing_database_error(e: &AdapterError) -> bool {
    const PATTERNS: &[&str] = &["doesn't exist", "does not exist", "Unknown database"];
    let msg = e.message();
    PATTERNS.iter().any(|pattern| msg.contains(pattern))
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

    fn is_permission_error(&self, e: &AdapterError) -> bool {
        e.vendor_code() == Some(1044) || e.vendor_code() == Some(1045) || e.vendor_code() == Some(1227)
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
            let fully_qualified_name = format!(
                "{}.{}.{}",
                table_catalogs.value(i),
                table_schemas.value(i),
                table_names.value(i)
            )
            .to_lowercase();

            if result.contains_key(&fully_qualified_name) {
                continue;
            }

            let owner = table_owners
                .map(|col| if col.is_null(i) { "" } else { col.value(i) })
                .unwrap_or("");

            result.insert(
                fully_qualified_name,
                create_catalog_table(CatalogTableMeta {
                    catalog: table_catalogs.value(i),
                    schema: table_schemas.value(i),
                    table: table_names.value(i),
                    data_type: data_types.value(i),
                    comment: comments.value(i),
                    owner,
                }),
            );
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
        let column_indices_col = stats_sql_result.column_by_name("column_index");
        let column_types = stats_sql_result.column_values::<StringArray>("column_type")?;
        let column_comments = stats_sql_result.column_values::<StringArray>("column_comment")?;

        let mut columns_by_relation = BTreeMap::new();

        for i in 0..table_catalogs.len() {
            let fully_qualified_name = format!(
                "{}.{}.{}",
                table_catalogs.value(i),
                table_schemas.value(i),
                table_names.value(i)
            )
            .to_lowercase();

            let column_name = column_names.value(i);
            let column_index = match column_indices_col {
                Some(col) => extract_column_index(col.as_ref(), i),
                None => i as i128,
            };
            let column = create_column_metadata(
                column_name,
                column_index,
                column_types.value(i),
                column_comments.value(i),
            );

            columns_by_relation
                .entry(fully_qualified_name)
                .or_insert_with(BTreeMap::new)
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

        let mut by_schema: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for relation in relations {
            let schema = relation.schema_as_str().unwrap_or_default().to_string();
            let identifier = relation.identifier_as_str().unwrap_or_default().to_string();
            let fqn = relation.semantic_fqn();
            by_schema.entry(schema).or_default().push((identifier, fqn));
        }

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));
        let engine = self.adapter.engine().clone();
        let token_clone = token.clone();
        let tasks: Vec<(String, Vec<(String, String)>)> = by_schema.into_iter().collect();

        let map_f = move |conn: &'_ mut dyn Connection,
                          task: &(String, Vec<(String, String)>)|
              -> AdapterResult<Vec<(String, MetadataFreshness)>> {
            query_schema_freshness(engine.as_ref(), conn, &task.0, &task.1, token_clone.clone())
        };

        let reduce_f = move |acc: &mut Acc,
                             _task: (String, Vec<(String, String)>),
                             result: AdapterResult<Vec<(String, MetadataFreshness)>>|
              -> Result<(), Cancellable<AdapterError>> {
            for (fqn, freshness) in result.map_err(Cancellable::Error)? {
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
            let mut lr_ctx = ListRelationsCtx {
                engine: adapter.engine().as_ref(),
                query_ctx: &query_ctx,
                token: token_clone.clone(),
            };
            with_relation_list_item_span(
                report_progress.then_some(RELATION_CACHE_OP_ID),
                &db_schema.to_string(),
                || list_relations(&mut lr_ctx, conn, db_schema),
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
                    if is_missing_database_error(&e) {
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

#[allow(clippy::too_many_arguments)]
pub fn get_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    let raw_schema = if !schema.is_empty() {
        schema
    } else {
        database
    };
    let query_schema = if adapter.quoting().schema {
        raw_schema.to_string()
    } else {
        raw_schema.to_lowercase()
    };

    let query_identifier = if adapter.quoting().identifier {
        identifier.to_string()
    } else {
        identifier.to_lowercase()
    };

    let sql = format!(
        r#"
            select
                case table_type
                    when 'VIEW' then 'view'
                    else 'table'
                end as `type`
            from information_schema.tables
            where table_schema = '{query_schema}'
              and table_name = '{query_identifier}'
        "#
    );

    let batch = adapter
        .engine()
        .execute(Some(state), conn, ctx, &sql, token)?;
    if batch.num_rows() == 0 {
        return Ok(None);
    }

    let column = batch.column_by_name("type").unwrap();
    let string_array = column.as_any().downcast_ref::<StringArray>().unwrap();

    if string_array.len() != 1 {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            "Did not find 'type' for a relation",
        ));
    }

    let relation_type = match string_array.value(0) {
        "table" => Some(RelationType::Table),
        "view" => Some(RelationType::View),
        _ => {
            return Err(AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                format!("Unsupported relation type {}", string_array.value(0)),
            ));
        }
    };

    let relation = do_create_relation(
        adapter.adapter_type(),
        database.to_string(),
        schema.to_string(),
        Some(identifier.to_string()),
        relation_type,
        adapter.quoting(),
    )?;
    Ok(Some(relation))
}
