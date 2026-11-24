use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use dbt_cloud_config::ResolvedCloudConfig;
use dbt_common::FsResult;
use dbt_common::io_args::EvalArgs;
use dbt_schemas::schemas::semantic_layer::semantic_manifest::SemanticManifest;
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};

/// Trait interface for a Metricflow client.
#[async_trait]
pub trait MetricflowClient: Send + Sync {
    fn override_semantic_manifest(&self, manifest: SemanticManifest) -> FsResult<()>;

    async fn export(&self, saved_query: &str, export_name: &str, write_cache: bool)
    -> FsResult<()>;

    fn into_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;
}

/// Whether `node` is a model written in MetricFlow SQL: its `metricflow` config is true. No
/// model has the config while the `metricflow_v2` flag is off.
///
/// Such a model does not render to the SQL of its target. The build's
/// [`SqlRewriter`](crate::sql_rewriter::SqlRewriter) expands what it renders to.
pub fn is_metricflow_sql(node: &dyn InternalDbtNodeAttributes) -> bool {
    node.as_any()
        .downcast_ref::<DbtModel>()
        .is_some_and(|model| model.deprecated_config.metricflow == Some(true))
}

/// Factory for creating a [MetricflowClient] at runtime.
#[async_trait]
pub trait MetricflowClientFactory: Send + Sync {
    /// Create a Metricflow client.
    async fn create_client(
        &self,
        arg: &EvalArgs,
        semantic_manifest: &SemanticManifest,
        dbt_cloud_config: Option<&ResolvedCloudConfig>,
        semantic_layer_spec_is_legacy: bool,
    ) -> FsResult<Option<Arc<dyn MetricflowClient>>>;
}
