use dbt_adapter_core::AdapterType;
use dbt_common::{FsResult, io_args::IoArgs};
use dbt_schemas::schemas::{Nodes, ResolvedCloudConfig, common::DbtQuoting};

/// Hooks called within the resolve phase
pub trait ResolverHooks: Send + Sync {
    /// Hook called before the resolve phase begins.
    fn pre_resolve(
        &self,
        _io: &IoArgs,
        _adapter_type: AdapterType,
        _nodes: &mut Nodes,
        _quoting: DbtQuoting,
    ) -> FsResult<()> {
        Ok(())
    }

    /// Hook called after the resolve phase completes.
    ///
    /// `adapter_type` is the target's default adapter, passed for the same reason
    /// `pre_resolve` takes it: a hook may synthesize nodes, and every node carries
    /// the adapter it resolves against rather than inferring one downstream.
    fn post_resolve(
        &self,
        _io: &IoArgs,
        _adapter_type: AdapterType,
        _nodes: &mut Nodes,
        _root_project_name: &str,
        _quoting: DbtQuoting,
        _cloud_config: &Option<ResolvedCloudConfig>,
    ) -> FsResult<()> {
        Ok(())
    }
}

/// Default no-op implementation of [ResolverHooks].
pub struct NoOpResolverHooks;
impl ResolverHooks for NoOpResolverHooks {}
