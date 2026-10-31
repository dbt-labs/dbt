use crate::AdapterEngine;
use crate::adapter::adapter_impl::AdapterImpl;
use crate::connection::{AdapterConnectionFactory, ConnectionGuard};
use crate::errors::{AdapterError, AdapterErrorKind};
use crate::sql_types::{TypeOps, make_arrow_field_v2};
use crate::{AdapterResult, errors::AsyncAdapterResult, metadata::*, record_batch::RecordBatchExt};
use arrow_schema::{Field, Schema};
use dbt_adbc::QueryCtx;
use dbt_common::cancellation::{Cancellable, CancellationToken};

use arrow_array::{Array, BooleanArray, Decimal128Array, RecordBatch, StringArray};

use dbt_adapter_core::ExecutionPhase;
use dbt_schemas::schemas::{
    legacy_catalog::{CatalogNodeStats, CatalogTable, ColumnMetadata, TableMetadata},
    relations::base::{BaseRelation, RelationPattern},
};
use indexmap::IndexMap;
use minijinja::State;

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub struct PostgresMetadataAdapter {
    adapter: AdapterImpl,
}

impl PostgresMetadataAdapter {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        let adapter = AdapterImpl::new(engine, None);
        Self { adapter }
    }
}

impl MetadataAdapter for PostgresMetadataAdapter {
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

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &mut ConnectionGuard<'_>,
                          relation: &Arc<dyn BaseRelation>|
              -> AdapterResult<Arc<Schema>> {
            // A Postgres connection sees only its own database: another one is refused, as
            // verify_database refuses it in a model, rather than answered from this one.
            let catalog = relation.database_as_str()?;
            if !catalog.is_empty() {
                adapter.verify_database(catalog.clone())?;
            }
            let schema = relation.schema_as_str()?;
            let identifier = relation.identifier_as_str()?;
            let sql = get_relation_columns_sql(relation.as_ref())?;

            let mut ctx = QueryCtx::new_metadata().with_desc("Get table schema");
            if let Some(node_id) = unique_id.clone() {
                ctx = ctx.with_node_id(&node_id);
            }
            if let Some(phase) = phase {
                ctx = ctx.with_phase(phase.as_str());
            }
            let (_, table) = adapter.query(&ctx, &mut *conn, &sql, None, token_clone.clone())?;
            let batch = table.original_record_batch();

            let fields = fields_from_columns(&batch, adapter.engine().type_ops().as_ref())?;
            if fields.is_empty() {
                return Err(AdapterError::new(
                    AdapterErrorKind::UnexpectedResult,
                    format!("No columns in pg_attribute for {catalog}.{schema}.{identifier}"),
                ));
            }
            Ok(Arc::new(Schema::new(fields)))
        };
        let reduce_f = |acc: &mut Acc,
                        relation: Arc<dyn BaseRelation>,
                        schema: AdapterResult<Arc<Schema>>|
         -> Result<(), Cancellable<AdapterError>> {
            acc.insert(relation.semantic_fqn(), schema);
            Ok(())
        };
        run_schema_cache_map_reduce(
            factory,
            relations.to_vec(),
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
        todo!("PostgresAdapter::list_relations_schemas_by_patterns")
    }

    fn freshness_inner(
        &self,
        _relations: &[Arc<dyn BaseRelation>],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>> {
        todo!("PostgresAdapter::freshness")
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
        _db_schemas: &[CatalogAndSchema],
        _token: CancellationToken,
        _report_progress: bool,
    ) -> AsyncAdapterResult<'_, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>> {
        // FIXME: Implement cache hydration
        let future = async move { Ok(BTreeMap::new()) };
        Box::pin(future)
    }
}

/// Arrow fields from the rows of [get_columns_sql], each keeping the type Postgres reported.
fn fields_from_columns(batch: &RecordBatch, type_ops: &dyn TypeOps) -> AdapterResult<Vec<Field>> {
    let column_names = batch.column_values::<StringArray>("column_name")?;
    let data_types = batch.column_values::<StringArray>("data_type")?;
    let is_nullables = batch.column_values::<BooleanArray>("is_nullable")?;
    let comments = batch.column_values::<StringArray>("remarks")?;

    (0..batch.num_rows())
        .map(|i| {
            let comment = match comments.value(i) {
                "" => None,
                c => Some(c.to_string()),
            };
            make_arrow_field_v2(
                type_ops,
                column_names.value(i).to_string(),
                data_types.value(i),
                Some(is_nullables.value(i)),
                comment,
            )
        })
        .collect()
}

/// [get_columns_sql] for a relation, by the names Postgres stores under its quote policy: an
/// unquoted `Orders` is `orders`.
fn get_relation_columns_sql(relation: &dyn BaseRelation) -> AdapterResult<String> {
    Ok(get_columns_sql(
        &relation.schema_as_resolved_str()?,
        &relation.identifier_as_resolved_str()?,
    ))
}

/// The columns of one table or view, in order, with their types as Postgres writes them
/// (`format_type`), from the catalog rather than `information_schema`, which shows only the
/// relations the user holds privileges on and spells user-defined types as `USER-DEFINED`.
fn get_columns_sql(schema: &str, identifier: &str) -> String {
    let literal = |s: &str| format!("'{}'", s.replace('\'', "''"));
    format!(
        "select
    a.attname::text as column_name,
    pg_catalog.format_type(a.atttypid, a.atttypmod) as data_type,
    not a.attnotnull as is_nullable,
    coalesce(pg_catalog.col_description(a.attrelid, a.attnum), '') as remarks
from pg_catalog.pg_attribute as a
join pg_catalog.pg_class as c on c.oid = a.attrelid
join pg_catalog.pg_namespace as n on n.oid = c.relnamespace
where n.nspname = {schema}
  and c.relname = {identifier}
  and a.attnum > 0
  and not a.attisdropped
order by a.attnum",
        schema = literal(schema),
        identifier = literal(identifier),
    )
}

#[cfg(test)]
mod tests {
    use super::{fields_from_columns, get_columns_sql, get_relation_columns_sql};
    use crate::metadata::ARROW_FIELD_COMMENT_METADATA_KEY;
    use crate::relation::create_relation;
    use crate::sql_types::DefaultTypeOps;
    use arrow_array::{BooleanArray, RecordBatch, StringArray};
    use dbt_adapter_core::AdapterType;
    use dbt_adapter_sql::types::metadata_sql_type_key;
    use dbt_schemas::schemas::common::ResolvedQuoting;
    use dbt_schemas::schemas::relations::base::BaseRelation;
    use std::sync::Arc;

    fn columns(rows: &[(&str, &str, bool, &str)]) -> RecordBatch {
        RecordBatch::try_from_iter([
            (
                "column_name",
                Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.0))) as _,
            ),
            (
                "data_type",
                Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.1))) as _,
            ),
            (
                "is_nullable",
                Arc::new(BooleanArray::from_iter(rows.iter().map(|r| Some(r.2)))) as _,
            ),
            (
                "remarks",
                Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r.3))) as _,
            ),
        ])
        .unwrap()
    }

    #[test]
    fn fields_keep_name_order_nullability_and_comment() {
        let batch = columns(&[
            ("id", "bigint", false, "The key"),
            ("name", "text", true, ""),
        ]);
        let fields =
            fields_from_columns(&batch, &DefaultTypeOps::new(AdapterType::Postgres)).unwrap();
        let summary: Vec<_> = fields
            .iter()
            .map(|f| {
                let comment = f.metadata().get(ARROW_FIELD_COMMENT_METADATA_KEY);
                (
                    f.name().as_str(),
                    f.is_nullable(),
                    comment.map(String::as_str),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [("id", false, Some("The key")), ("name", true, None)]
        );
    }

    #[test]
    fn fields_keep_the_type_postgres_reported() {
        let batch = columns(&[
            ("lat", "double precision", true, ""),
            ("at", "timestamp with time zone", true, ""),
        ]);
        let fields =
            fields_from_columns(&batch, &DefaultTypeOps::new(AdapterType::Postgres)).unwrap();
        let key = metadata_sql_type_key(AdapterType::Postgres);
        let types: Vec<_> = fields
            .iter()
            .map(|f| f.metadata().get(key).unwrap().as_str())
            .collect();
        assert_eq!(types, ["double precision", "timestamp with time zone"]);
    }

    fn relation(schema: &str, identifier: &str, quoted: bool) -> Box<dyn BaseRelation> {
        let quoting = ResolvedQuoting {
            database: quoted,
            identifier: quoted,
            schema: quoted,
        };
        create_relation(
            AdapterType::Postgres,
            "db".to_string(),
            schema.to_string(),
            Some(identifier.to_string()),
            None,
            quoting,
        )
        .unwrap()
    }

    #[test]
    fn get_relation_columns_sql_looks_up_an_unquoted_name_as_postgres_folds_it() {
        let sql =
            get_relation_columns_sql(relation("Analytics", "Orders", false).as_ref()).unwrap();
        assert!(sql.contains("n.nspname = 'analytics'"));
        assert!(sql.contains("c.relname = 'orders'"));
    }

    #[test]
    fn get_relation_columns_sql_keeps_a_quoted_name() {
        let sql = get_relation_columns_sql(relation("Analytics", "Orders", true).as_ref()).unwrap();
        assert!(sql.contains("n.nspname = 'Analytics'"));
        assert!(sql.contains("c.relname = 'Orders'"));
    }

    #[test]
    fn get_columns_sql_filters_on_the_relation() {
        let sql = get_columns_sql("analytics", "orders");
        assert!(sql.contains("n.nspname = 'analytics'"));
        assert!(sql.contains("c.relname = 'orders'"));
    }

    #[test]
    fn get_columns_sql_escapes_quotes_in_names() {
        let sql = get_columns_sql("o'brien", "it's");
        assert!(sql.contains("n.nspname = 'o''brien'"));
        assert!(sql.contains("c.relname = 'it''s'"));
    }
}
