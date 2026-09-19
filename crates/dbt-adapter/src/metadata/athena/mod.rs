//! Athena metadata adapter.
//!
//! Athena is Trino over the Glue catalog, so every metadata read here is a
//! query against `information_schema` executed through the ADBC connection.
//! Provides the schema-creation preflight (`create_schemas_if_not_exists` ->
//! `athena__create_schema`), per-relation schema fetch for unit tests and
//! contracts (`list_relations_schemas_inner`, via a zero-row probe — Athena
//! returns `ResultSetMetadata` even for empty result sets, which the driver
//! maps to an Arrow schema), relation-cache hydration over
//! `information_schema.tables` (`list_relations`), and catalog parsing for
//! `compile --write-catalog` (`build_schemas_from_stats_sql` /
//! `build_columns_from_get_columns` over an `information_schema`-shaped
//! RecordBatch). Metadata-based source freshness is not available: Glue does
//! not expose a last-altered timestamp through `information_schema`, and
//! dbt-athena's own implementation reads it from S3 object metadata.

use crate::AdapterEngine;
use crate::adapter::adapter_impl::AdapterImpl;
use crate::connection::AdapterConnectionFactory;
use crate::errors::{AdapterError, AdapterErrorKind, AsyncAdapterResult, Cancellable};
use crate::relation::Relation;
use crate::{AdapterResult, metadata::*, record_batch::RecordBatchExt};
use arrow_schema::Schema;
use dbt_adapter_engine::MapReduce;
use dbt_adbc::{Connection, QueryCtx};
use dbt_common::cancellation::CancellationToken;

use arrow_array::{Array, Int64Array, RecordBatch, StringArray};

use dbt_adapter_core::ExecutionPhase;
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

/// Athena folds every unquoted identifier to lowercase and Glue stores names
/// lowercased, so `information_schema` literals are compared in lowercase.
/// Single quotes are doubled to keep the literal well-formed.
pub(crate) fn athena_string_literal(value: &str) -> String {
    value.to_lowercase().replace('\'', "''")
}

/// Map an `information_schema.tables.table_type` value to a dbt
/// [`RelationType`]. Trino reports `BASE TABLE` and `VIEW`; anything else is
/// treated as a table.
pub(crate) fn relation_type_from_table_type(table_type: &str) -> RelationType {
    match table_type.trim().to_ascii_uppercase().as_str() {
        "VIEW" => RelationType::View,
        _ => RelationType::Table,
    }
}

pub struct AthenaMetadataAdapter {
    adapter: AdapterImpl,
}

impl AthenaMetadataAdapter {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        let adapter = AdapterImpl::new(engine, None);
        Self { adapter }
    }
}

impl MetadataAdapter for AthenaMetadataAdapter {
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
        let table_owners = stats_sql_result.column_values::<StringArray>("table_owner")?;

        let mut result = BTreeMap::<String, CatalogTable>::new();

        for i in 0..table_catalogs.len() {
            let catalog = table_catalogs.value(i);
            let schema = table_schemas.value(i);
            let table = table_names.value(i);
            let data_type = data_types.value(i);
            let comment = comments.value(i);
            let owner = table_owners.value(i);

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
        // `information_schema.columns.ordinal_position` is a bigint in Trino.
        let column_indices = stats_sql_result.column_values::<Int64Array>("column_index")?;
        let column_types = stats_sql_result.column_values::<StringArray>("column_type")?;
        let column_comments = stats_sql_result.column_values::<StringArray>("column_comment")?;

        let mut columns_by_relation = BTreeMap::new();

        for i in 0..table_catalogs.len() {
            let catalog = table_catalogs.value(i);
            let schema = table_schemas.value(i);
            let table = table_names.value(i);

            let fully_qualified_name = format!("{catalog}.{schema}.{table}").to_lowercase();

            let column_name = column_names.value(i);
            let column_index = column_indices.value(i) as i128;
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

        // The Arrow schema comes from a zero-row probe: Athena returns the
        // result-set metadata even when no rows match, and the ADBC driver
        // maps it to an Arrow schema. The probe renders the name exactly as
        // materializations do (quote policy included). The HashMap key must
        // match `relation.semantic_fqn()`, so both forms are carried as a
        // tuple.
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
            let sql = format!("select * from {sql_name} where false limit 0");
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
            "list_relations_schemas_by_patterns is not yet implemented for the Athena metadata adapter",
        );
        Box::pin(future::ready(Err(Cancellable::Error(err))))
    }

    fn freshness_inner(
        &self,
        _relations: &[Arc<dyn BaseRelation>],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>> {
        let err = AdapterError::new(
            AdapterErrorKind::NotSupported,
            "metadata-based source freshness is not yet implemented for the Athena adapter",
        );
        Box::pin(future::ready(Err(Cancellable::Error(err))))
    }

    fn create_schemas_if_not_exists(
        &self,
        state: &State<'_, '_>,
        catalog_schemas: Vec<(String, String, String)>,
    ) -> AdapterResult<Vec<(String, String, String, AdapterResult<()>)>> {
        create_schemas_if_not_exists(&self.adapter, self, state, catalog_schemas)
    }

    fn supports_relation_progress(&self) -> bool {
        false
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
                || adapter.list_relations(&query_ctx, conn, db_schema, token_clone.clone()),
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
                Err(e) => Err(Cancellable::Error(e)),
            }
        };

        let map_reduce = MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None);
        map_reduce.run(Arc::new(db_schemas.to_vec()), token)
    }
}

/// List every table and view in a schema by querying `information_schema.tables`.
///
/// A schema that does not exist yields zero rows rather than an error, which
/// is what cache hydration wants for not-yet-created target schemas. The
/// catalog filter is skipped when dbt has no resolved database (Athena's
/// `information_schema` is scoped to the connection's catalog anyway).
pub fn list_relations(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let schema_literal = athena_string_literal(&db_schema.resolved_schema);
    let catalog_filter = if db_schema.resolved_catalog.is_empty() {
        String::new()
    } else {
        format!(
            " and lower(table_catalog) = '{}'",
            athena_string_literal(&db_schema.resolved_catalog)
        )
    };
    let sql = format!(
        "select table_schema, table_name, table_type \
         from information_schema.tables \
         where lower(table_schema) = '{schema_literal}'{catalog_filter}"
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
        let relation = Relation::new(
            engine.adapter_type(),
            Some(db_schema.resolved_catalog.clone()),
            Some(table_schemas.value(i).to_string()),
            Some(table_names.value(i).to_string()),
        )
        .with_relation_type(relation_type_from_table_type(table_types.value(i)))
        .with_quoting(engine.quoting());

        relations.push(Arc::new(relation) as Arc<dyn BaseRelation>);
    }

    Ok(relations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_are_lowercased_and_quote_escaped() {
        assert_eq!(athena_string_literal("Analytics_Bronze_QA"), "analytics_bronze_qa");
        assert_eq!(athena_string_literal("o'neil"), "o''neil");
    }

    #[test]
    fn table_type_mapping_follows_trino() {
        assert_eq!(relation_type_from_table_type("BASE TABLE"), RelationType::Table);
        assert_eq!(relation_type_from_table_type("VIEW"), RelationType::View);
        assert_eq!(relation_type_from_table_type("view"), RelationType::View);
        assert_eq!(relation_type_from_table_type("SOMETHING"), RelationType::Table);
    }
}
