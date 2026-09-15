use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::string::String;
use std::sync::Arc;

use arrow_schema::Schema;
use dbt_adapter_core::ExecutionPhase;
use dbt_common::cancellation::CancellationToken;
use dbt_common::{AdapterResult, AsyncAdapterResult};
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_schemas::schemas::relations::base::RelationPattern;

use crate::metadata::{MetadataFreshness, RelationSchemaPair, UDF};

pub trait ListRelationsSchemasStrategy: Send + Sync {
    fn run(
        &self,
        relations: Arc<Vec<Arc<dyn BaseRelation>>>,
        unique_id: Option<String>,
        phase: Option<ExecutionPhase>,
        item_span_operation_id: Option<&str>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'static, HashMap<String, AdapterResult<Arc<Schema>>>>;

    fn run_by_patterns(
        &self,
        patterns: Arc<Vec<RelationPattern>>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'static, Vec<(String, AdapterResult<RelationSchemaPair>)>>;
}

#[expect(dead_code)]
pub trait ListUDFsStrategy: Send + Sync {
    fn run(
        &self,
        catalog_schemas: &BTreeMap<String, BTreeSet<String>>,
        token: CancellationToken,
    ) -> AsyncAdapterResult<'static, Vec<UDF>>;
}

pub trait FreshnessStrategy: Send + Sync {
    fn run(
        &self,
        relations: &[Arc<dyn BaseRelation>],
        token: CancellationToken,
    ) -> AsyncAdapterResult<'static, BTreeMap<String, MetadataFreshness>>;
}
