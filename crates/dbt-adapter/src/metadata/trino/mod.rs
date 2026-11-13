//! Trino metadata adapter.
//!
//! Provides the schema-creation preflight (`create_schemas_if_not_exists` ->
//! `trino__create_schema`), the per-relation schema fetch for unit tests and
//! contracts (`list_relations_schemas_inner`, via a zero-row probe) and `get_relation`.
//! Relation-cache hydration is intentionally empty, so dbt falls back to the per-relation
//! `list_relations_without_caching` / `get_relation` macros, and the batch existence check
//! reports `NotSupported` rather than reading that empty listing as absence. Catalog parsing
//! (`compile --write-catalog`) and metadata-based source freshness are not implemented yet.

use crate::AdapterEngine;
use crate::adapter::adapter_impl::AdapterImpl;
use crate::connection::{AdapterConnectionFactory, ConnectionGuard};
use crate::errors::{AdapterError, AdapterErrorKind, AsyncAdapterResult, Cancellable};
use crate::relation::do_create_relation;
use crate::{AdapterResult, metadata::*};
use arrow_array::{RecordBatch, StringArray};
use arrow_schema::Schema;
use dbt_adapter_core::ExecutionPhase;
use dbt_adapter_engine::record_batch::RecordBatchExt;
use dbt_adapter_sql::ident::{quote_identifier, quote_string_literal};
use dbt_adbc::QueryCtx;
use dbt_common::cancellation::CancellationToken;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::{
    legacy_catalog::{CatalogTable, ColumnMetadata},
    relations::base::{BaseRelation, RelationPattern},
};
use minijinja::State;

use std::collections::{BTreeMap, HashMap};
use std::future;
use std::sync::Arc;

// Arrow field metadata keys the Trino ADBC driver sets (driverbase-go sqlwrapper) next to
// `sql.database_type_name`, which carries the bare name of scalar types (`VARCHAR`, `DECIMAL`,
// `TIMESTAMP WITH TIME ZONE`).
/// The length of a `varchar(n)` or `char(n)`.
pub(crate) const TRINO_LENGTH_METADATA_KEY: &str = "sql.length";
/// The precision of a `decimal(p,s)`.
pub(crate) const TRINO_PRECISION_METADATA_KEY: &str = "sql.precision";
/// The scale of a `decimal(p,s)`.
pub(crate) const TRINO_SCALE_METADATA_KEY: &str = "sql.scale";
/// The precision `p` of a `timestamp(p)`, `timestamp(p) with time zone` or `time(p)`.
pub(crate) const TRINO_FRACTIONAL_SECONDS_PRECISION_METADATA_KEY: &str =
    "sql.fractional_seconds_precision";

pub struct TrinoMetadataAdapter {
    adapter: AdapterImpl,
}

impl TrinoMetadataAdapter {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        let adapter = AdapterImpl::new(engine, None);
        Self { adapter }
    }
}

impl MetadataAdapter for TrinoMetadataAdapter {
    fn adapter_type(&self) -> AdapterType {
        self.adapter.adapter_type()
    }

    fn build_schemas_from_stats_sql(
        &self,
        _stats_sql_result: Arc<RecordBatch>,
    ) -> AdapterResult<BTreeMap<String, CatalogTable>> {
        Err(AdapterError::new(
            AdapterErrorKind::NotSupported,
            "catalog generation is not yet implemented for the Trino adapter",
        ))
    }

    fn build_columns_from_get_columns(
        &self,
        _stats_sql_result: Arc<RecordBatch>,
    ) -> AdapterResult<BTreeMap<String, BTreeMap<String, ColumnMetadata>>> {
        Err(AdapterError::new(
            AdapterErrorKind::NotSupported,
            "catalog generation is not yet implemented for the Trino adapter",
        ))
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

        // The Arrow schema comes straight from a zero-row probe: the Trino ADBC driver
        // returns the result-set schema even for empty results, with the type of each column
        // in the field metadata, so no string-based type parsing is needed. The probe renders
        // the name exactly as materializations do (quote policy included), while the HashMap
        // key must match `relation.semantic_fqn()`, so both forms are carried as a tuple.
        let keys: Vec<(String, String)> = relations
            .iter()
            .map(|relation| (relation.semantic_fqn(), relation.render_self_as_str()))
            .collect();

        let factory = Box::new(AdapterConnectionFactory::new(self.adapter.engine().clone()));

        let adapter = self.adapter.clone();
        let token_clone = token.clone();
        let map_f = move |conn: &mut ConnectionGuard<'_>,
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
            "list_relations_schemas_by_patterns is not yet implemented for the Trino metadata adapter",
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
            "metadata-based source freshness is not yet implemented for the Trino adapter",
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
        _db_schemas: &[CatalogAndSchema],
        _token: CancellationToken,
        _report_progress: bool,
    ) -> AsyncAdapterResult<'_, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>> {
        // Cache hydration not implemented: dbt falls back to the per-relation
        // `list_relations_without_caching` / `get_relation` macros.
        let future = async move { Ok(BTreeMap::new()) };
        Box::pin(future)
    }

    /// The default reads the listing above, which is empty, so every relation would be
    /// reported absent. Returning `NotSupported` lets callers fail open instead.
    fn relations_exist_inner<'a>(
        &'a self,
        _relations: &'a [Arc<dyn BaseRelation>],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'a, BTreeMap<String, bool>> {
        let err = AdapterError::new(
            AdapterErrorKind::NotSupported,
            "relation existence checks are not yet implemented for the Trino adapter",
        );
        Box::pin(future::ready(Err(Cancellable::Error(err))))
    }
}

/// The relation type of `database.schema.identifier`, from the catalog's
/// `information_schema.tables` and `system.metadata.materialized_views`. Trino stores
/// identifiers in lowercase, quoted or not.
///
/// https://github.com/starburstdata/dbt-trino/blob/d7b73f6/dbt/include/trino/macros/adapters.sql#L34
fn get_relation_sql(database: &str, schema: &str, identifier: &str) -> String {
    let literal = |s: &str| quote_string_literal(&s.to_lowercase(), AdapterType::Trino);
    let catalog = quote_identifier(&database.to_lowercase(), AdapterType::Trino);
    let (q_catalog, q_schema, q_ident) = (literal(database), literal(schema), literal(identifier));
    format!(
        "select \
           case when mv.name is not null then 'materialized_view' \
                when t.table_type = 'BASE TABLE' then 'table' \
                when t.table_type = 'VIEW' then 'view' \
                else t.table_type \
           end as table_type \
         from {catalog}.information_schema.tables t \
         left join ( \
           select * from system.metadata.materialized_views \
           where catalog_name = {q_catalog} and schema_name = {q_schema}) mv \
           on mv.catalog_name = t.table_catalog \
          and mv.schema_name = t.table_schema \
          and mv.name = t.table_name \
         where t.table_schema = {q_schema} and t.table_name = {q_ident}"
    )
}

/// `get_relation` for Trino.
#[allow(clippy::too_many_arguments)]
pub(crate) fn get_relation(
    adapter: &AdapterImpl,
    state: &State,
    ctx: &QueryCtx,
    conn: &mut ConnectionGuard<'_>,
    database: &str,
    schema: &str,
    identifier: &str,
    token: CancellationToken,
) -> AdapterResult<Option<Box<dyn BaseRelation>>> {
    let sql = get_relation_sql(database, schema, identifier);
    let batch = adapter
        .engine()
        .execute(Some(state), conn.as_mut(), ctx, &sql, token)?;
    if batch.num_rows() == 0 {
        return Ok(None);
    }

    let arr = batch.column_values::<StringArray>("table_type")?;
    let relation_type = match arr.value(0) {
        "table" => Some(RelationType::Table),
        "view" => Some(RelationType::View),
        "materialized_view" => Some(RelationType::MaterializedView),
        other => {
            return Err(AdapterError::new(
                AdapterErrorKind::Internal,
                format!("Unexpected relation type: {other}"),
            ));
        }
    };
    let relation = do_create_relation(
        AdapterType::Trino,
        database.to_string(),
        schema.to_string(),
        Some(identifier.to_string()),
        relation_type,
        adapter.quoting(),
    )?;
    Ok(Some(relation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_types::DefaultTypeOps;
    use crate::stmt_splitter::DefaultStmtSplitter;
    use dbt_common::cancellation::never_cancels;
    use dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING;

    /// The relation listing is empty, so the default existence check would report every
    /// relation absent; existing tables must not be treated as missing.
    #[dbt_runtime::test]
    async fn relations_exist_fails_open() {
        let adapter = AdapterImpl::new_mock(
            AdapterType::Trino,
            BTreeMap::new(),
            DEFAULT_RESOLVED_QUOTING,
            Arc::new(DefaultTypeOps::new(AdapterType::Trino)),
            Arc::new(DefaultStmtSplitter),
        );
        let metadata = TrinoMetadataAdapter::new(adapter.engine().clone());
        let relation: Arc<dyn BaseRelation> = do_create_relation(
            AdapterType::Trino,
            "iceberg".to_string(),
            "analytics".to_string(),
            Some("events".to_string()),
            Some(RelationType::Table),
            DEFAULT_RESOLVED_QUOTING,
        )
        .unwrap()
        .into();

        let result = metadata.relations_exist(&[relation], never_cancels()).await;
        assert!(
            matches!(
                &result,
                Err(Cancellable::Error(error)) if error.kind() == AdapterErrorKind::NotSupported
            ),
            "{result:?}"
        );
    }

    #[test]
    fn get_relation_sql_reads_the_catalog_information_schema() {
        let sql = get_relation_sql("My\"Cat", "Sch'ema", "Events");
        assert!(
            sql.contains(r#"from "my""cat".information_schema.tables t"#),
            "{sql}"
        );
        assert!(
            sql.contains("where catalog_name = 'my\"cat' and schema_name = 'sch''ema'"),
            "{sql}"
        );
        assert!(
            sql.contains("where t.table_schema = 'sch''ema' and t.table_name = 'events'"),
            "{sql}"
        );
    }
}
