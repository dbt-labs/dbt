use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_common::cancellation::{CancellationToken, CancelledError};
use dbt_common::io_args::IoArgs;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schemas::schemas::ResolvedCloudConfig;
use dbt_schemas::schemas::packages::DbtPackagesLock;

use crate::git_client::GitClientContext;
use crate::hub_client::{DBT_HUB_URL, HubClient};
use crate::network_client::retrying_http_client;
use crate::notices::{EmitPolicy, NoticeBuffer, prepare_for_emit};
use crate::private_package::PrivatePackageResolver;
use crate::tarball_client::TarballClient;

/// Shared runtime dependencies for deps resolve/install flows.
pub struct DepsOperationContext<'a> {
    pub io: &'a IoArgs,
    pub vars: &'a BTreeMap<String, dbt_yaml::Value>,
    pub jinja_env: &'a JinjaEnv,
    pub cancellation: &'a CancellationToken,
    pub hub_registry: HubClient,
    pub tarball_client: TarballClient,
    pub git_client: GitClientContext,
    pub skip_private_deps: bool,
    /// Captured into the `notices` buffer's [`EmitPolicy`] at construction.
    #[allow(dead_code)]
    pub version_check: bool,
    pub use_v2_compatible_package_downloads: bool,
    pub require_hub_verified_downloads: bool,
    pub private_package_resolver: Arc<dyn PrivatePackageResolver>,
    pub cloud_config: Option<ResolvedCloudConfig>,
    pub notices: NoticeBuffer,
}

impl<'a> DepsOperationContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn from_entry(
        io: &'a IoArgs,
        vars: &'a BTreeMap<String, dbt_yaml::Value>,
        jinja_env: &'a JinjaEnv,
        cancellation: &'a CancellationToken,
        skip_private_deps: bool,
        version_check: bool,
        use_v2_compatible_package_downloads: bool,
        require_hub_verified_downloads: bool,
        private_package_resolver: Arc<dyn PrivatePackageResolver>,
        cloud_config: Option<ResolvedCloudConfig>,
    ) -> Self {
        let hub_url_from_env = std::env::var("DBT_PACKAGE_HUB_URL");
        let hub_url = hub_url_from_env
            .as_deref()
            .map(|s| {
                if s.ends_with('/') {
                    // dbt-core required a trailing slash - here we support but do not require it.
                    &s[0..s.len() - 1]
                } else {
                    s
                }
            })
            .unwrap_or(DBT_HUB_URL);
        let http_client = retrying_http_client();
        let tarball_client = TarballClient::from_client(http_client.clone(), cancellation.clone());

        Self {
            io,
            vars,
            jinja_env,
            cancellation,
            hub_registry: HubClient::with_client(hub_url, http_client.clone()),
            tarball_client: tarball_client.clone(),
            git_client: GitClientContext::from_clients(http_client, tarball_client),
            skip_private_deps,
            version_check,
            use_v2_compatible_package_downloads,
            require_hub_verified_downloads,
            private_package_resolver,
            cloud_config,
            notices: NoticeBuffer::new(EmitPolicy::from_inputs(version_check)),
        }
    }

    pub(crate) fn flush_notices(&self, lock: &DbtPackagesLock) {
        for n in prepare_for_emit(self.notices.drain(), lock) {
            n.emit();
        }
    }

    #[inline]
    pub fn check_cancellation(&self) -> Result<(), CancelledError> {
        self.cancellation.check_cancellation()
    }
}
