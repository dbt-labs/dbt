//! A table provider that knows its schema and nothing else.

use datafusion_catalog::{Session, TableProvider};
use datafusion_common::error::DataFusionError;
use datafusion_expr::Expr;
use dbt_schema_store::{CanonicalFqn, SchemaStoreTrait};
use std::any::Any;
use std::sync::Arc;

/// A [`TableProvider`] for binding SQL: it reports the schema in the schema
/// store and fails when scanned. Unlike `DelayedDataTableProvider` it does not
/// reference DataFusion's listing tables or file formats, so a host that only
/// binds does not link them.
#[derive(Debug)]
pub struct SchemaOnlyTableProvider {
    canonical_fqn: CanonicalFqn,
    schema_store: Arc<dyn SchemaStoreTrait>,
}

impl SchemaOnlyTableProvider {
    /// Creates a provider for `canonical_fqn`, whose schema is in `schema_store`.
    pub fn new(canonical_fqn: CanonicalFqn, schema_store: Arc<dyn SchemaStoreTrait>) -> Self {
        Self {
            canonical_fqn,
            schema_store,
        }
    }
}

#[async_trait::async_trait]
impl TableProvider for SchemaOnlyTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> Arc<arrow_schema::Schema> {
        let schema_entry = self
            .schema_store
            .get_schema(&self.canonical_fqn)
            .unwrap_or_else(|| {
                panic!("Schema not found for canonical FQN: {}", self.canonical_fqn);
            });
        schema_entry.inner().clone()
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        _projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        _limit: Option<usize>,
    ) -> Result<Arc<dyn datafusion::physical_plan::ExecutionPlan>, DataFusionError> {
        Err(DataFusionError::NotImplemented(format!(
            "{} is a schema-only table and cannot be scanned",
            self.canonical_fqn
        )))
    }

    fn table_type(&self) -> datafusion::datasource::TableType {
        datafusion::datasource::TableType::Base
    }
}
