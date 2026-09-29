use std::sync::Arc;

use dbt_loader::loader_hooks::{LoaderHooks, NoOpLoaderHooks};
use fs_deps::private_package::{LocalPrivatePackageResolver, PrivatePackageResolver};

pub struct LoaderFeature {
    pub hooks: Arc<dyn LoaderHooks>,
    pub private_package_resolver: Arc<dyn PrivatePackageResolver>,
}

impl Default for LoaderFeature {
    fn default() -> Self {
        Self {
            hooks: Arc::new(NoOpLoaderHooks),
            private_package_resolver: Arc::new(LocalPrivatePackageResolver),
        }
    }
}
