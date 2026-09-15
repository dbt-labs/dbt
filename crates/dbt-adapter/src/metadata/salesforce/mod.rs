use crate::AdapterEngine;
use crate::adapter::adapter_impl::*;
use crate::errors::{AdapterResult, AsyncAdapterResult};
use crate::metadata::*;
use arrow_array::RecordBatch;
use arrow_schema::Schema;
use dbt_adapter_core::ExecutionPhase;
use dbt_common::cancellation::CancellationToken;
use dbt_schemas::schemas::relations::base::{BaseRelation, RelationPattern};
use minijinja::State;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

pub struct SalesforceMetadataAdapter {
    // adapter: AdapterImpl,
}

impl SalesforceMetadataAdapter {
    pub fn new(engine: Arc<dyn AdapterEngine>) -> Self {
        let _adapter = AdapterImpl::new(engine, None);
        Self { /* adapter */ }
    }
}

impl MetadataAdapter for SalesforceMetadataAdapter {
    fn adapter_type(&self) -> AdapterType {
        AdapterType::Salesforce
    }

    fn build_schemas_from_stats_sql(
        &self,
        _: Arc<RecordBatch>,
    ) -> AdapterResult<BTreeMap<String, dbt_schemas::schemas::legacy_catalog::CatalogTable>> {
        unimplemented!("SalesforceAdapter does not implement build_schemas_from_stats_sql");
    }

    fn build_columns_from_get_columns(
        &self,
        _: Arc<RecordBatch>,
    ) -> AdapterResult<
        BTreeMap<String, BTreeMap<String, dbt_schemas::schemas::legacy_catalog::ColumnMetadata>>,
    > {
        unimplemented!("SalesforceAdapter does not implement build_columns_from_get_columns");
    }

    fn list_user_defined_functions_inner(
        &self,
        _catalog_schemas: &BTreeMap<String, BTreeSet<String>>,
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, Vec<UDF>> {
        let future = std::future::ready(Ok(Default::default()));
        Box::pin(future)
    }

    fn list_relations_schemas_inner(
        &self,
        _unique_id: Option<String>,
        _phase: Option<ExecutionPhase>,
        _relations: &[Arc<dyn BaseRelation>],
        _item_span_operation_id: Option<&str>,
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, HashMap<String, AdapterResult<Arc<Schema>>>> {
        let future = std::future::ready(Ok(Default::default()));
        Box::pin(future)
    }

    /// List relations schemas by patterns (use information schema query)
    fn list_relations_schemas_by_patterns_inner(
        &self,
        _relations_pattern: &[RelationPattern],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, Vec<(String, AdapterResult<RelationSchemaPair>)>> {
        let future = std::future::ready(Ok(Default::default()));
        Box::pin(future)
    }

    fn create_schemas_if_not_exists(
        &self,
        _state: &State<'_, '_>,
        _catalog_schemas: Vec<(String, String, String)>,
    ) -> AdapterResult<Vec<(String, String, String, AdapterResult<()>)>> {
        Ok(Default::default())
    }

    fn freshness_inner(
        &self,
        _relations: &[Arc<dyn BaseRelation>],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<String, MetadataFreshness>> {
        let future = std::future::ready(Ok(Default::default()));
        Box::pin(future)
    }

    /// Reference: https://github.com/dbt-labs/dbt-adapters/blob/f492c919d3bd415bf5065b3cd8cd1af23562feb0/dbt-snowflake/src/dbt/include/snowflake/macros/metadata/list_relations_without_caching.sql
    fn list_relations_in_parallel_inner(
        &self,
        _db_schemas: &[CatalogAndSchema],
        _token: CancellationToken,
    ) -> AsyncAdapterResult<'_, BTreeMap<CatalogAndSchema, AdapterResult<RelationVec>>> {
        let future = std::future::ready(Ok(Default::default()));
        Box::pin(future)
    }
}
