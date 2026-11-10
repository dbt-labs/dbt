use std::sync::Arc;

use dbt_tasks_core::lake_compute_catalog_attach::LakeComputeCatalogAttachChecker;
use dbt_tasks_core::lake_compute_mdls::LakeComputeMdlsChecker;
use dbt_tasks_core::lake_compute_propagation::LakeComputePropagationChecker;

pub struct LakeComputeFeature {
    /// A checker for verifying propagation from a lake compute target
    /// to the profile's native connection.
    ///
    /// Used during `dbt debug`. Returning `None` (the default) means this
    /// build has no such check available.
    pub propagation_checker: Option<Arc<dyn LakeComputePropagationChecker>>,
    /// A checker for verifying that the catalogs a project declares
    /// are reachable from a lake compute target.
    ///
    /// For use during `dbt debug`. Returning `None` (the default) means
    /// this build has no such check available.
    pub catalog_attach_checker: Option<Arc<dyn LakeComputeCatalogAttachChecker>>,
    /// A checker for the MDLS write + read-back round trip.
    ///
    /// For use during `dbt debug`. Returning `None` (the default) means
    /// this build has no such check available.
    pub mdls_checker: Option<Arc<dyn LakeComputeMdlsChecker>>,
}
