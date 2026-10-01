use std::sync::Arc;

use async_trait::async_trait;
use dbt_adapter::engine::SidecarClient;
use dbt_common::FsResult;
use dbt_common::io_args::IoArgs;
use dbt_schemas::state::ResolverState;

/// Factory for creating a [SidecarClient] at runtime.
#[async_trait]
pub trait SidecarClientFactory: Send + Sync {
    async fn create_client(
        &self,
        io_args: &IoArgs,
        resolved_state: &ResolverState,
        long_living: bool,
        as_service: bool,
        debug: bool,
    ) -> FsResult<Arc<dyn SidecarClient>>;
}

/// Sidecar feature slot on [`FeatureStack`](super::feature_stack::FeatureStack).
pub struct SidecarFeature {
    pub factory: Option<Arc<dyn SidecarClientFactory>>,
}
