use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use dbt_cloud_config::ResolvedCloudConfig;
use dbt_common::FsResult;
use dbt_common::io_args::EvalArgs;
use dbt_schemas::schemas::semantic_layer::semantic_manifest::SemanticManifest;

/// Trait interface for a Metricflow client.
#[async_trait]
pub trait MetricflowClient: Send + Sync {
    fn override_semantic_manifest(&self, manifest: SemanticManifest) -> FsResult<()>;

    async fn export(&self, saved_query: &str, export_name: &str, write_cache: bool)
    -> FsResult<()>;

    fn into_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;
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
