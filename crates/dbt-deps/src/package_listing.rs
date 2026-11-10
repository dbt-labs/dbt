use dbt_yaml::Verbatim;
use std::{
    collections::{BTreeMap, HashMap, hash_map::Entry},
    path::Path,
    sync::Arc,
};

use dbt_common::{ErrorCode, FsResult, err, io_args::IoArgs, unexpected_fs_err};
use dbt_common::{FsError, tracing::dbt_emit::emit_info_log_message};
use dbt_jinja_utils::{
    jinja_environment::JinjaEnv, phases::load::LoadContext, serde::into_typed_with_jinja,
};
use dbt_schemas::schemas::ResolvedCloudConfig;
use dbt_schemas::schemas::packages::{
    DbtPackageEntry, DbtPackages, DbtPackagesLock, GitPackage, HubPackage, LocalPackage,
    PrivatePackage, PrivatePackageProvider, TarballPackage,
};

use crate::{
    notices::{NoticeBuffer, PackageNotice, PackageNoticeKind},
    private_package::{LocalPrivatePackageResolver, PrivatePackageRef, PrivatePackageResolver},
    utils::{get_local_package_full_path, read_and_validate_dbt_project},
};

use super::types::{
    GitUnpinnedPackage, HubUnpinnedPackage, LocalPinnedPackage, LocalUnpinnedPackage,
    PrivateUnpinnedPackage, TarballUnpinnedPackage,
};

trait Incorporatable {
    #[allow(dead_code)]
    fn incorporate(&mut self, other: Self);
}

impl Incorporatable for GitUnpinnedPackage {
    fn incorporate(&mut self, other: Self) {
        self.incorporate(other);
    }
}

impl Incorporatable for PrivateUnpinnedPackage {
    fn incorporate(&mut self, other: Self) {
        self.incorporate(other);
    }
}

impl Incorporatable for TarballUnpinnedPackage {
    fn incorporate(&mut self, other: Self) {
        self.incorporate(other);
    }
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum UnpinnedPackage {
    Hub(HubUnpinnedPackage),
    Git(GitUnpinnedPackage),
    Local(LocalUnpinnedPackage),
    Private(PrivateUnpinnedPackage),
    Tarball(TarballUnpinnedPackage),
}

impl UnpinnedPackage {
    fn type_name(&self) -> &str {
        match self {
            UnpinnedPackage::Hub(_) => "hub",
            UnpinnedPackage::Git(_) => "git",
            UnpinnedPackage::Local(_) => "local",
            UnpinnedPackage::Private(_) => "private",
            UnpinnedPackage::Tarball(_) => "tarball",
        }
    }
}

pub struct PackageListing<'a> {
    pub io_args: IoArgs,
    pub vars: BTreeMap<String, dbt_yaml::Value>,
    pub packages: HashMap<String, UnpinnedPackage>,
    pub skip_private_deps: bool,
    pub private_package_resolver: Arc<dyn PrivatePackageResolver>,
    /// Populated by `resolve_private_packages_batch` before the per-entry
    /// loop runs; keyed by the rendered `private` string. A miss is resolved
    /// on demand by `get_or_resolve_private_package_url`.
    resolved_private_urls: HashMap<String, String>,
    /// Optional cloud-run context an implementation of [`PrivatePackageResolver`] may use.
    cloud_config: Option<ResolvedCloudConfig>,
    notices: &'a NoticeBuffer,
}

impl<'a> PackageListing<'a> {
    pub fn new(
        io_args: IoArgs,
        vars: BTreeMap<String, dbt_yaml::Value>,
        notices: &'a NoticeBuffer,
    ) -> Self {
        Self {
            io_args,
            vars,
            packages: HashMap::new(),
            skip_private_deps: false,
            private_package_resolver: Arc::new(LocalPrivatePackageResolver),
            resolved_private_urls: HashMap::new(),
            cloud_config: None,
            notices,
        }
    }

    pub fn with_skip_private_deps(mut self, skip: bool) -> Self {
        self.skip_private_deps = skip;
        self
    }

    pub fn with_private_package_resolver(
        mut self,
        private_package_resolver: Arc<dyn PrivatePackageResolver>,
    ) -> Self {
        self.private_package_resolver = private_package_resolver;
        self
    }

    pub fn with_cloud_config(mut self, cloud_config: Option<ResolvedCloudConfig>) -> Self {
        self.cloud_config = cloud_config;
        self
    }

    pub fn in_dir(&self) -> &Path {
        &self.io_args.in_dir
    }

    pub async fn hydrate_dbt_packages(
        &mut self,
        packages: &DbtPackages,
        jinja_env: &JinjaEnv,
    ) -> FsResult<()> {
        self.resolve_private_packages_batch(&packages.packages, jinja_env)
            .await?;
        for package in packages.packages.iter() {
            self.incorporate(package.clone(), jinja_env).await?;
        }
        Ok(())
    }

    pub async fn hydrate_dbt_packages_lock(
        &mut self,
        dbt_packages_lock: &DbtPackagesLock,
        jinja_env: &JinjaEnv,
    ) -> FsResult<()> {
        let entries: Vec<DbtPackageEntry> = dbt_packages_lock
            .packages
            .iter()
            .cloned()
            .map(Into::into)
            .collect();
        self.resolve_private_packages_batch(&entries, jinja_env)
            .await?;
        for package in entries {
            self.incorporate(package, jinja_env).await?;
        }
        Ok(())
    }

    /// Batch-resolves all private packages in `entries` via
    /// `self.private_package_resolver` (one call, up front, before any
    /// individual git checkout), populating `self.resolved_private_urls`.
    /// Entries the resolver doesn't cover are resolved individually later,
    /// in `incorporate`, via `get_or_resolve_private_package_url`. A no-op
    /// when `skip_private_deps` is set, matching `incorporate`'s per-entry
    /// skip so private packages are never resolved (and can't hard-error)
    /// when the caller opted out.
    async fn resolve_private_packages_batch(
        &mut self,
        entries: &[DbtPackageEntry],
        jinja_env: &JinjaEnv,
    ) -> FsResult<()> {
        if self.skip_private_deps {
            return Ok(());
        }

        let deps_context = LoadContext::new(self.vars.clone());
        let mut refs = Vec::new();
        for entry in entries {
            let DbtPackageEntry::Private(p) = entry else {
                continue;
            };
            let rendered_private: String = {
                let value = dbt_yaml::to_value(&p.private).map_err(|e| {
                    unexpected_fs_err!("Failed to serialize private package URL: {e}")
                })?;
                into_typed_with_jinja(value, true, jinja_env, &deps_context, &[], None, true)
            }?;
            refs.push(PrivatePackageRef {
                private_def: rendered_private,
                provider: p
                    .provider
                    .map(PrivatePackageProvider::as_str)
                    .map(String::from),
            });
        }
        if refs.is_empty() {
            return Ok(());
        }

        self.resolved_private_urls = self
            .private_package_resolver
            .resolve_urls(&refs, &self.cloud_config)
            .await?;
        Ok(())
    }

    async fn incorporate(
        &mut self,
        package: DbtPackageEntry,
        jinja_env: &JinjaEnv,
    ) -> FsResult<()> {
        let deps_context = LoadContext::new(self.vars.clone());
        match package {
            DbtPackageEntry::Hub(hub_package) => {
                let hub_package: HubPackage = {
                    let value = dbt_yaml::to_value(&hub_package).map_err(|e| {
                        unexpected_fs_err!("Failed to serialize hub package spec: {e}")
                    })?;
                    into_typed_with_jinja(value, true, jinja_env, &deps_context, &[], None, true)
                }?;
                if let Some(unpinned_package) = self.packages.get_mut(&hub_package.package) {
                    match unpinned_package {
                        UnpinnedPackage::Hub(hub_unpinned_package) => {
                            hub_unpinned_package.incorporate(hub_package.clone().try_into()?);
                        }
                        package_type => {
                            return err!(
                                ErrorCode::InvalidConfig,
                                "Found conflicting package types for package {}: 'hub' vs '{}'",
                                hub_package.package,
                                package_type.type_name(),
                            );
                        }
                    }
                } else {
                    self.packages.insert(
                        hub_package.package.clone(),
                        UnpinnedPackage::Hub(hub_package.try_into()?),
                    );
                }
            }
            DbtPackageEntry::Git(git_package) => {
                let git_package: GitPackage = {
                    let value = dbt_yaml::to_value(&git_package).map_err(|e| {
                        unexpected_fs_err!("Failed to serialize git package spec: {e}")
                    })?;
                    into_typed_with_jinja(value, true, jinja_env, &deps_context, &[], None, true)
                }?;
                let git_package_url: String = {
                    let value = dbt_yaml::to_value(&git_package.git).map_err(|e| {
                        unexpected_fs_err!("Failed to serialize git package URL: {e}")
                    })?;
                    into_typed_with_jinja(value, true, jinja_env, &deps_context, &[], None, true)
                }?;

                // Create key that includes subdirectory if present
                let mut package_key = git_package_url.clone();
                if let Some(subdirectory) = &git_package.subdirectory {
                    package_key.push_str(&format!("#{subdirectory}"));
                }

                self.handle_remote_package(
                    &package_key,
                    UnpinnedPackage::Git(GitUnpinnedPackage {
                        git: git_package_url,
                        name: None,
                        warn_unpinned: git_package.warn_unpinned,
                        revisions: git_package
                            .revision
                            .clone()
                            .map(|v| vec![v])
                            .unwrap_or_default(),
                        subdirectory: git_package.subdirectory.clone(),
                        unrendered: git_package.__unrendered__.clone(),
                        original_entry: git_package,
                    }),
                    "git",
                )?;
            }
            DbtPackageEntry::Local(local_package) => {
                let local_package: LocalPackage = {
                    let value = dbt_yaml::to_value(&local_package).map_err(|e| {
                        unexpected_fs_err!("Failed to serialize local package spec: {e}")
                    })?;
                    into_typed_with_jinja(value, true, jinja_env, &deps_context, &[], None, true)
                }?;
                // Get absolute path of local package
                let full_path = get_local_package_full_path(self.in_dir(), &local_package);

                let dbt_project =
                    read_and_validate_dbt_project(&full_path, true, jinja_env, &self.vars).await?;
                let package_key = full_path.to_string_lossy().to_string();
                match self.packages.entry(package_key) {
                    Entry::Occupied(_) => {
                        self.notices.record(PackageNotice {
                            key: dbt_project.name,
                            kind: PackageNoticeKind::DuplicatePackageName,
                        });
                    }
                    Entry::Vacant(entry) => {
                        entry.insert(UnpinnedPackage::Local(LocalUnpinnedPackage {
                            local: full_path,
                            name: Some(dbt_project.name),
                        }));
                    }
                }
            }
            DbtPackageEntry::Private(private_package) => {
                let mut private_package: PrivatePackage = {
                    let value = dbt_yaml::to_value(&private_package).map_err(|e| {
                        unexpected_fs_err!("Failed to serialize private package spec: {e}")
                    })?;
                    into_typed_with_jinja(value, true, jinja_env, &deps_context, &[], None, true)
                }?;
                let private_package_private: String = {
                    let value = dbt_yaml::to_value(&private_package.private).map_err(|e| {
                        unexpected_fs_err!("Failed to serialize private package URL: {e}")
                    })?;
                    into_typed_with_jinja(value, true, jinja_env, &deps_context, &[], None, true)
                }?;

                private_package.private = Verbatim::from(private_package_private);

                if self.skip_private_deps {
                    // Skip private packages when skip_private_deps is enabled
                    emit_info_log_message(format!(
                        "Skipping private package {} due to --skip-private-deps flag",
                        private_package.private.as_ref()
                    ));
                    return Ok(());
                }

                let private_package_url = self
                    .get_or_resolve_private_package_url(&private_package)
                    .await?;

                // Create key that includes subdirectory if present
                let mut package_key = private_package_url.clone();
                if let Some(subdirectory) = &private_package.subdirectory {
                    package_key.push_str(&format!("#{subdirectory}"));
                }

                self.handle_remote_package(
                    &package_key,
                    UnpinnedPackage::Private(PrivateUnpinnedPackage {
                        private: private_package_url,
                        name: None,
                        provider: private_package.provider,
                        warn_unpinned: private_package.warn_unpinned,
                        revisions: private_package
                            .revision
                            .clone()
                            .map(|v| vec![v])
                            .unwrap_or_default(),
                        subdirectory: private_package.subdirectory.clone(),
                        unrendered: private_package.__unrendered__.clone(),
                        original_entry: private_package,
                    }),
                    "private",
                )?;
            }
            DbtPackageEntry::Tarball(tarball_package) => {
                let tarball_package: TarballPackage = {
                    let value = dbt_yaml::to_value(&tarball_package).map_err(|e| {
                        unexpected_fs_err!("Failed to serialize tarball package spec: {e}")
                    })?;
                    into_typed_with_jinja(value, true, jinja_env, &deps_context, &[], None, true)
                }?;
                let tarball_url: String = {
                    let value = dbt_yaml::to_value(&tarball_package.tarball).map_err(|e| {
                        unexpected_fs_err!("Failed to serialize tarball package URL: {e}")
                    })?;
                    into_typed_with_jinja(value, true, jinja_env, &deps_context, &[], None, true)
                }?;

                self.handle_remote_package(
                    &tarball_url.clone(),
                    UnpinnedPackage::Tarball(TarballUnpinnedPackage {
                        tarball: tarball_url,
                        name: None,
                        unrendered: tarball_package.__unrendered__.clone(),
                        original_entry: tarball_package,
                    }),
                    "tarball",
                )?;
            }
        }
        Ok(())
    }

    fn handle_remote_package(
        &mut self,
        package_key: &str,
        new_package: UnpinnedPackage,
        package_type: &str,
    ) -> FsResult<()> {
        if let Some(existing_package) = self.packages.get_mut(package_key) {
            match existing_package {
                UnpinnedPackage::Git(existing_git_package) if package_type == "git" => {
                    if let UnpinnedPackage::Git(new_git_package) = new_package {
                        existing_git_package.incorporate(new_git_package);
                    }
                }
                UnpinnedPackage::Private(existing_private_package) if package_type == "private" => {
                    if let UnpinnedPackage::Private(new_private_package) = new_package {
                        existing_private_package.incorporate(new_private_package);
                    }
                }
                UnpinnedPackage::Tarball(existing_tarball_package) if package_type == "tarball" => {
                    if let UnpinnedPackage::Tarball(new_tarball_package) = new_package {
                        existing_tarball_package.incorporate(new_tarball_package);
                    }
                }
                _ => {
                    return err!(
                        ErrorCode::InvalidConfig,
                        "Found conflicting package types for package {}: '{}' vs '{}'",
                        package_key,
                        package_type,
                        existing_package.type_name(),
                    );
                }
            }
        } else {
            self.packages.insert(package_key.to_string(), new_package);
        }
        Ok(())
    }

    fn handle_remote_unpinned_package<T: Incorporatable + Clone>(
        &mut self,
        package_key: &str,
        new_package: &UnpinnedPackage,
        package_type: &str,
    ) -> FsResult<()> {
        if let Some(existing_package) = self.packages.get_mut(package_key) {
            match existing_package {
                UnpinnedPackage::Git(existing_git_package) if package_type == "git" => {
                    if let UnpinnedPackage::Git(new_git_package) = new_package {
                        existing_git_package.incorporate(new_git_package.clone());
                    }
                }
                UnpinnedPackage::Private(existing_private_package) if package_type == "private" => {
                    if let UnpinnedPackage::Private(new_private_package) = new_package {
                        existing_private_package.incorporate(new_private_package.clone());
                    }
                }
                UnpinnedPackage::Tarball(existing_tarball_package) if package_type == "tarball" => {
                    if let UnpinnedPackage::Tarball(new_tarball_package) = new_package {
                        existing_tarball_package.incorporate(new_tarball_package.clone());
                    }
                }
                _ => {
                    return err!(
                        ErrorCode::InvalidConfig,
                        "Found conflicting package types for package {}: '{}' vs '{}'",
                        package_key,
                        package_type,
                        existing_package.type_name(),
                    );
                }
            }
        } else {
            self.packages
                .insert(package_key.to_string(), new_package.clone());
        }
        Ok(())
    }

    pub fn incorporate_unpinned_package(&mut self, package: &UnpinnedPackage) -> FsResult<()> {
        match package {
            UnpinnedPackage::Hub(hub_unpinned_package) => {
                if let Some(existing_hub_unpinned_package) =
                    self.packages.get_mut(&hub_unpinned_package.package)
                {
                    match existing_hub_unpinned_package {
                        UnpinnedPackage::Hub(existing_hub_unpinned_package) => {
                            existing_hub_unpinned_package.incorporate(hub_unpinned_package.clone());
                        }
                        package_type => {
                            return err!(
                                ErrorCode::InvalidConfig,
                                "Found conflicting package types for package {}: 'hub' vs '{}'",
                                hub_unpinned_package.package,
                                package_type.type_name(),
                            );
                        }
                    }
                } else {
                    self.packages
                        .insert(hub_unpinned_package.package.clone(), package.clone());
                }
            }
            UnpinnedPackage::Git(git_unpinned_package) => {
                // Create key that includes subdirectory if present
                let mut package_key = git_unpinned_package.git.clone();
                if let Some(subdirectory) = &git_unpinned_package.subdirectory {
                    package_key.push_str(&format!("#{subdirectory}"));
                }
                self.handle_remote_unpinned_package::<GitUnpinnedPackage>(
                    &package_key,
                    package,
                    "git",
                )?;
            }
            UnpinnedPackage::Local(local_package) => {
                let pinned_package = LocalPinnedPackage::try_from(local_package.clone())?;
                if let Some(existing_local_unpinned_package) =
                    self.packages.get_mut(&pinned_package.name)
                {
                    match existing_local_unpinned_package {
                        UnpinnedPackage::Local(existing_local_unpinned_package) => {
                            if existing_local_unpinned_package.local != pinned_package.local {
                                return err!(
                                    ErrorCode::InvalidConfig,
                                    "Found conflicting package paths for package {}: '{}' vs '{}'",
                                    pinned_package.name,
                                    existing_local_unpinned_package.local.to_string_lossy(),
                                    pinned_package.local.to_string_lossy(),
                                );
                            }
                        }
                        _ => {
                            return err!(
                                ErrorCode::InvalidConfig,
                                "Found conflicting package types for package {}: 'local' vs '{}'",
                                pinned_package.name,
                                existing_local_unpinned_package.type_name(),
                            );
                        }
                    }
                } else {
                    self.packages.insert(
                        pinned_package.name.to_string(),
                        UnpinnedPackage::Local(LocalUnpinnedPackage {
                            local: pinned_package.local,
                            name: Some(pinned_package.name.clone()),
                        }),
                    );
                }
            }
            UnpinnedPackage::Private(private_unpinned_package) => {
                // Create key that includes subdirectory if present
                let mut package_key = private_unpinned_package.private.clone();
                if let Some(subdirectory) = &private_unpinned_package.subdirectory {
                    package_key.push_str(&format!("#{subdirectory}"));
                }
                self.handle_remote_unpinned_package::<PrivateUnpinnedPackage>(
                    &package_key,
                    package,
                    "private",
                )?;
            }
            UnpinnedPackage::Tarball(tarball_unpinned_package) => {
                self.handle_remote_unpinned_package::<TarballUnpinnedPackage>(
                    &tarball_unpinned_package.tarball,
                    package,
                    "tarball",
                )?;
            }
        }
        Ok(())
    }

    pub async fn update_from(
        &mut self,
        packages: &Vec<DbtPackageEntry>,
        jinja_env: &JinjaEnv,
    ) -> FsResult<()> {
        for package in packages {
            self.incorporate(package.clone(), jinja_env).await?;
        }
        Ok(())
    }

    async fn get_or_resolve_private_package_url(
        &mut self,
        private_package: &PrivatePackage,
    ) -> FsResult<String> {
        let private_def = private_package.private.as_ref();

        // Common case: private package has already been resolved.
        if let Some(resolved) = self.resolved_private_urls.get(private_def) {
            return Ok(resolved.clone());
        }

        // Package not yet seen - must resolve. This should only happen with transitive
        // package dependencies, since we resolve everything from packages.yml up-front.
        let new_package_urls = self
            .private_package_resolver
            .resolve_urls(
                &[PrivatePackageRef {
                    private_def: private_def.to_string(),
                    provider: private_package
                        .provider
                        .map(PrivatePackageProvider::as_str)
                        .map(String::from),
                }],
                &self.cloud_config,
            )
            .await?;
        let url = new_package_urls
            .get(private_def)
            .ok_or_else(|| {
                FsError::new(
                    ErrorCode::Generic,
                    format!("packages resolved, but requested package, {private_def} not found"),
                )
            })?
            .clone();

        // Save the resolved URL in the local map
        self.resolved_private_urls
            .insert(private_def.to_string(), url.clone());

        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_common::io_args::IoArgs;
    use std::collections::HashMap;

    #[test]
    fn test_handle_remote_package_with_subdirectory() {
        let io_args = IoArgs::default();
        let notices = NoticeBuffer::default();
        let mut package_listing = PackageListing::new(io_args, BTreeMap::new(), &notices);

        // Create two git packages with the same URL but different subdirectories
        let git_package_1 = UnpinnedPackage::Git(GitUnpinnedPackage {
            git: "https://github.com/dbt-labs/dbt-core.git".to_string(),
            name: None,
            warn_unpinned: None,
            revisions: vec!["main".to_string()],
            subdirectory: Some("core".to_string()),
            unrendered: HashMap::new(),
            original_entry: GitPackage {
                git: Verbatim::from("https://github.com/dbt-labs/dbt-core.git".to_string()),
                revision: Some("main".to_string()),
                warn_unpinned: None,
                subdirectory: Some("core".to_string()),
                __unrendered__: HashMap::new(),
            },
        });

        let git_package_2 = UnpinnedPackage::Git(GitUnpinnedPackage {
            git: "https://github.com/dbt-labs/dbt-core.git".to_string(),
            name: None,
            warn_unpinned: None,
            revisions: vec!["main".to_string()],
            subdirectory: Some("adapters".to_string()),
            unrendered: HashMap::new(),
            original_entry: GitPackage {
                git: Verbatim::from("https://github.com/dbt-labs/dbt-core.git".to_string()),
                revision: Some("main".to_string()),
                warn_unpinned: None,
                subdirectory: Some("adapters".to_string()),
                __unrendered__: HashMap::new(),
            },
        });

        // Add the first package
        package_listing
            .handle_remote_package(
                "https://github.com/dbt-labs/dbt-core.git#core",
                git_package_1,
                "git",
            )
            .unwrap();

        // Add the second package - should be treated as a separate package
        package_listing
            .handle_remote_package(
                "https://github.com/dbt-labs/dbt-core.git#adapters",
                git_package_2,
                "git",
            )
            .unwrap();

        // Verify that both packages are stored with different keys
        assert_eq!(package_listing.packages.len(), 2);
        assert!(
            package_listing
                .packages
                .contains_key("https://github.com/dbt-labs/dbt-core.git#core")
        );
        assert!(
            package_listing
                .packages
                .contains_key("https://github.com/dbt-labs/dbt-core.git#adapters")
        );
    }

    #[test]
    fn test_handle_remote_package_same_url_no_subdirectory() {
        let io_args = IoArgs::default();
        let notices = NoticeBuffer::default();
        let mut package_listing = PackageListing::new(io_args, BTreeMap::new(), &notices);

        // Create two git packages with the same URL and no subdirectory
        let git_package_1 = UnpinnedPackage::Git(GitUnpinnedPackage {
            git: "https://github.com/dbt-labs/dbt-core.git".to_string(),
            name: None,
            warn_unpinned: None,
            revisions: vec!["main".to_string()],
            subdirectory: None,
            unrendered: HashMap::new(),
            original_entry: GitPackage {
                git: Verbatim::from("https://github.com/dbt-labs/dbt-core.git".to_string()),
                revision: Some("main".to_string()),
                warn_unpinned: None,
                subdirectory: None,
                __unrendered__: HashMap::new(),
            },
        });

        let git_package_2 = UnpinnedPackage::Git(GitUnpinnedPackage {
            git: "https://github.com/dbt-labs/dbt-core.git".to_string(),
            name: None,
            warn_unpinned: None,
            revisions: vec!["develop".to_string()],
            subdirectory: None,
            unrendered: HashMap::new(),
            original_entry: GitPackage {
                git: Verbatim::from("https://github.com/dbt-labs/dbt-core.git".to_string()),
                revision: Some("develop".to_string()),
                warn_unpinned: None,
                subdirectory: None,
                __unrendered__: HashMap::new(),
            },
        });

        // Add the first package
        package_listing
            .handle_remote_package(
                "https://github.com/dbt-labs/dbt-core.git",
                git_package_1,
                "git",
            )
            .unwrap();

        // Add the second package - should be incorporated into the first one
        package_listing
            .handle_remote_package(
                "https://github.com/dbt-labs/dbt-core.git",
                git_package_2,
                "git",
            )
            .unwrap();

        // Verify that only one package is stored (they should be incorporated)
        assert_eq!(package_listing.packages.len(), 1);
        assert!(
            package_listing
                .packages
                .contains_key("https://github.com/dbt-labs/dbt-core.git")
        );
    }

    mod private_packages_batch_resolution {
        use super::*;
        use async_trait::async_trait;
        use dbt_jinja_utils::phases::load::init::initialize_load_profile_jinja_environment;
        use dbt_schemas::schemas::packages::{DbtPackageLock, PrivatePackageLock};
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// Fake [`PrivatePackageResolver`] used to test batching behavior
        /// without a real resolver implementation: resolves `org/repo1` and
        /// `org/repo2` and leaves everything else for local git URL
        /// building, while counting how many times it was called (to assert
        /// batching).
        struct FakeResolver {
            calls: AtomicUsize,
        }

        impl FakeResolver {
            fn new() -> Self {
                Self {
                    calls: AtomicUsize::new(0),
                }
            }
        }

        #[async_trait]
        impl PrivatePackageResolver for FakeResolver {
            async fn resolve_urls(
                &self,
                entries: &[PrivatePackageRef],
                _cloud_config: &Option<ResolvedCloudConfig>,
            ) -> FsResult<HashMap<String, String>> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(entries
                    .iter()
                    .filter(|e| e.private_def == "org/repo1" || e.private_def == "org/repo2")
                    .map(|e| {
                        (
                            e.private_def.clone(),
                            format!("https://tok@github.com/{}.git", e.private_def),
                        )
                    })
                    .collect())
            }
        }

        fn private_package(private: &str) -> PrivatePackage {
            PrivatePackage {
                private: private.to_string().into(),
                provider: None,
                revision: None,
                warn_unpinned: None,
                subdirectory: None,
                __unrendered__: HashMap::new(),
            }
        }

        fn assert_resolved_via_resolver(package_listing: &PackageListing) {
            assert_eq!(package_listing.packages.len(), 2);
            for pkg in package_listing.packages.values() {
                match pkg {
                    UnpinnedPackage::Private(p) => {
                        assert!(p.private.starts_with("https://tok@github.com/org/repo"));
                    }
                    other => panic!("expected a private package, got {other:?}"),
                }
            }
        }

        #[dbt_runtime::test]
        async fn hydrate_dbt_packages_batches_private_resolution_into_one_call() {
            let resolver = Arc::new(FakeResolver::new());
            let io_args = IoArgs::default();
            let notices = NoticeBuffer::default();
            let mut package_listing = PackageListing::new(io_args, BTreeMap::new(), &notices)
                .with_private_package_resolver(resolver.clone());

            let dbt_packages = DbtPackages {
                projects: vec![],
                packages: vec![
                    DbtPackageEntry::Private(private_package("org/repo1")),
                    DbtPackageEntry::Private(private_package("org/repo2")),
                ],
            };

            let jinja_env = initialize_load_profile_jinja_environment();
            package_listing
                .hydrate_dbt_packages(&dbt_packages, &jinja_env)
                .await
                .unwrap();

            assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
            assert_resolved_via_resolver(&package_listing);
        }

        #[dbt_runtime::test]
        async fn hydrate_dbt_packages_lock_batches_private_resolution_into_one_call() {
            // Regression test: private packages must be resolved on both the
            // lock-regen path (`hydrate_dbt_packages`) and the
            // install-from-existing-lockfile path
            // (`hydrate_dbt_packages_lock`) — resolving only on one would
            // silently break steady-state runs.
            let resolver = Arc::new(FakeResolver::new());
            let io_args = IoArgs::default();
            let notices = NoticeBuffer::default();
            let mut package_listing = PackageListing::new(io_args, BTreeMap::new(), &notices)
                .with_private_package_resolver(resolver.clone());

            let dbt_packages_lock = DbtPackagesLock {
                packages: vec![
                    DbtPackageLock::Private(PrivatePackageLock {
                        private: "org/repo1".to_string().into(),
                        name: "repo1".to_string(),
                        revision: "main".to_string(),
                        provider: None,
                        warn_unpinned: None,
                        subdirectory: None,
                        __unrendered__: HashMap::new(),
                    }),
                    DbtPackageLock::Private(PrivatePackageLock {
                        private: "org/repo2".to_string().into(),
                        name: "repo2".to_string(),
                        revision: "main".to_string(),
                        provider: None,
                        warn_unpinned: None,
                        subdirectory: None,
                        __unrendered__: HashMap::new(),
                    }),
                ],
                sha1_hash: String::new(),
            };

            let jinja_env = initialize_load_profile_jinja_environment();
            package_listing
                .hydrate_dbt_packages_lock(&dbt_packages_lock, &jinja_env)
                .await
                .unwrap();

            assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
            assert_resolved_via_resolver(&package_listing);
        }

        #[dbt_runtime::test]
        async fn hydrate_dbt_packages_skips_resolution_when_skip_private_deps() {
            // Regression test: batch resolution must respect `skip_private_deps`
            // the same way the per-entry path in `incorporate` does. Before this
            // fix, the batch call ran unconditionally and could hard-error (or
            // call out to a resolver) for a private package the caller asked to
            // skip entirely.
            let resolver = Arc::new(FakeResolver::new());
            let io_args = IoArgs::default();
            let notices = NoticeBuffer::default();
            let mut package_listing = PackageListing::new(io_args, BTreeMap::new(), &notices)
                .with_skip_private_deps(true)
                .with_private_package_resolver(resolver.clone());

            // Not "org/repo1" or "org/repo2" - the fake resolver would fail to
            // resolve it if it were ever called.
            let dbt_packages = DbtPackages {
                projects: vec![],
                packages: vec![DbtPackageEntry::Private(private_package(
                    "unresolvable-org/unresolvable-repo",
                ))],
            };

            let jinja_env = initialize_load_profile_jinja_environment();
            package_listing
                .hydrate_dbt_packages(&dbt_packages, &jinja_env)
                .await
                .unwrap();

            assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
            assert!(package_listing.packages.is_empty());
        }
    }
}
