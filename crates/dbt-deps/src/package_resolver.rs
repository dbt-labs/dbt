//! Concurrent package resolution: one trait impl per unpinned package kind.
//!
//! [`PackageResolver::resolve`] checks cancellation then delegates to
//! [`PackageResolver::resolve_inner`]. The orchestrator in `compute_package_lock`
//! resolves a level concurrently (bounded), then merges transitive deps in sorted
//! key order (deterministic).

use dbt_common::{FsResult, stdfs};
use dbt_schemas::schemas::packages::{
    DbtPackageEntry, DbtPackageLock, GitPackageLock, HubPackageLock, LocalPackageLock,
    PackageVersion, PrivatePackageLock, TarballPackageLock,
};

use crate::context::DepsOperationContext;
use crate::git_client::download_git_like_package;
use crate::package_listing::UnpinnedPackage;
use crate::steps::load_dbt_packages;
use crate::types::{
    GitPinnedPackage, GitUnpinnedPackage, HubUnpinnedPackage, LocalPinnedPackage,
    LocalUnpinnedPackage, PrivatePinnedPackage, PrivateUnpinnedPackage, TarballPinnedPackage,
    TarballUnpinnedPackage,
};
use crate::utils::{ensure_dir, make_tempdir, read_and_validate_dbt_project};

/// Result of resolving one unpinned package: pinned metadata plus direct deps.
#[derive(Debug, Clone)]
#[allow(dead_code)] // orchestrator reads transitive_deps today; name/version for future checks
pub(crate) struct ResolvedPackage {
    pub project_name: String,
    pub pinned_version: Option<String>,
    pub transitive_deps: Vec<DbtPackageEntry>,
}

/// Pin/fetch one package and discover its direct dependency entries.
pub(crate) trait PackageResolver {
    async fn resolve_inner(&mut self, ctx: &DepsOperationContext<'_>) -> FsResult<ResolvedPackage>;

    async fn resolve(&mut self, ctx: &DepsOperationContext<'_>) -> FsResult<ResolvedPackage> {
        ctx.check_cancellation()?;
        self.resolve_inner(ctx).await
    }

    async fn to_lock_entry(&self, ctx: &DepsOperationContext<'_>) -> FsResult<DbtPackageLock>;
}

impl PackageResolver for HubUnpinnedPackage {
    async fn resolve_inner(&mut self, ctx: &DepsOperationContext<'_>) -> FsResult<ResolvedPackage> {
        let resolved = self.resolved(&ctx.hub_registry).await?;
        ctx.notices.collect(&resolved);
        let out = ResolvedPackage {
            project_name: resolved.pinned.name.clone(),
            pinned_version: Some(resolved.pinned.version.clone()),
            transitive_deps: resolved.version.packages.clone(),
        };
        self.resolved_hub = Some(resolved);
        Ok(out)
    }

    async fn to_lock_entry(&self, ctx: &DepsOperationContext<'_>) -> FsResult<DbtPackageLock> {
        let pinned = match &self.resolved_hub {
            Some(resolved) => &resolved.pinned,
            None => &self.resolved(&ctx.hub_registry).await?.pinned,
        };
        Ok(DbtPackageLock::Hub(HubPackageLock {
            package: pinned.package.clone(),
            name: pinned.name.clone(),
            version: PackageVersion::String(pinned.version.clone()),
        }))
    }
}

impl PackageResolver for GitUnpinnedPackage {
    async fn resolve_inner(&mut self, ctx: &DepsOperationContext<'_>) -> FsResult<ResolvedPackage> {
        let tmp_dir = make_tempdir(None)?;
        let download_dir = tmp_dir.path().join("git_pkg");
        ensure_dir(&download_dir).await?;
        let (checkout_path, commit_sha) = download_git_like_package(
            ctx,
            &self.git,
            &self.revisions,
            &self.subdirectory,
            self.warn_unpinned.unwrap_or_default(),
            &download_dir,
        )
        .await?;
        self.revisions = vec![commit_sha.clone()];
        let dbt_project =
            read_and_validate_dbt_project(&checkout_path, true, ctx.jinja_env, ctx.vars).await?;
        self.name = Some(dbt_project.name.clone());
        let mut transitive_deps = Vec::new();
        if let Some(dbt_packages) = load_dbt_packages(&checkout_path).await?.0 {
            ctx.notices.collect(&dbt_packages);
            transitive_deps = dbt_packages.packages;
        }
        drop(tmp_dir);
        Ok(ResolvedPackage {
            project_name: dbt_project.name,
            pinned_version: Some(commit_sha),
            transitive_deps,
        })
    }

    async fn to_lock_entry(&self, _ctx: &DepsOperationContext<'_>) -> FsResult<DbtPackageLock> {
        let pinned_package: GitPinnedPackage = self.clone().try_into()?;
        Ok(DbtPackageLock::Git(GitPackageLock {
            git: self.original_entry.git.clone(),
            name: pinned_package.name,
            revision: pinned_package.revision,
            warn_unpinned: pinned_package.warn_unpinned,
            subdirectory: pinned_package.subdirectory,
            __unrendered__: pinned_package.unrendered,
        }))
    }
}

impl PackageResolver for LocalUnpinnedPackage {
    async fn resolve_inner(&mut self, ctx: &DepsOperationContext<'_>) -> FsResult<ResolvedPackage> {
        let pinned: LocalPinnedPackage = self.clone().try_into()?;
        let mut transitive_deps = Vec::new();
        if let Some(dbt_packages) = load_dbt_packages(&self.local).await?.0 {
            ctx.notices.collect(&dbt_packages);
            transitive_deps = dbt_packages.packages;
        }
        Ok(ResolvedPackage {
            project_name: pinned.name.clone(),
            pinned_version: None,
            transitive_deps,
        })
    }

    async fn to_lock_entry(&self, ctx: &DepsOperationContext<'_>) -> FsResult<DbtPackageLock> {
        let pinned_package: LocalPinnedPackage = self.clone().try_into()?;
        Ok(DbtPackageLock::Local(LocalPackageLock {
            name: pinned_package.name,
            local: stdfs::diff_paths(&self.local, &ctx.io.in_dir)?,
        }))
    }
}

impl PackageResolver for PrivateUnpinnedPackage {
    async fn resolve_inner(&mut self, ctx: &DepsOperationContext<'_>) -> FsResult<ResolvedPackage> {
        let tmp_dir = make_tempdir(None)?;
        let download_dir = tmp_dir.path().join("git_pkg");
        ensure_dir(&download_dir).await?;
        let (checkout_path, commit_sha) = download_git_like_package(
            ctx,
            &self.private,
            &self.revisions,
            &self.subdirectory,
            self.warn_unpinned.unwrap_or_default(),
            &download_dir,
        )
        .await?;
        self.revisions = vec![commit_sha.clone()];
        let dbt_project =
            read_and_validate_dbt_project(&checkout_path, true, ctx.jinja_env, ctx.vars).await?;
        self.name = Some(dbt_project.name.clone());
        let mut transitive_deps = Vec::new();
        if let Some(dbt_packages) = load_dbt_packages(&checkout_path).await?.0 {
            ctx.notices.collect(&dbt_packages);
            transitive_deps = dbt_packages.packages;
        }
        drop(tmp_dir);
        Ok(ResolvedPackage {
            project_name: dbt_project.name,
            pinned_version: Some(commit_sha),
            transitive_deps,
        })
    }

    async fn to_lock_entry(&self, _ctx: &DepsOperationContext<'_>) -> FsResult<DbtPackageLock> {
        let pinned_package: PrivatePinnedPackage = self.clone().try_into()?;
        Ok(DbtPackageLock::Private(PrivatePackageLock {
            private: self.original_entry.private.clone(),
            name: pinned_package.name,
            provider: pinned_package.provider,
            revision: pinned_package.revision,
            warn_unpinned: pinned_package.warn_unpinned,
            subdirectory: pinned_package.subdirectory,
            __unrendered__: pinned_package.unrendered,
        }))
    }
}

impl PackageResolver for TarballUnpinnedPackage {
    async fn resolve_inner(&mut self, ctx: &DepsOperationContext<'_>) -> FsResult<ResolvedPackage> {
        let tmp_dir = make_tempdir(None)?;
        let download_dir = tmp_dir.path().join("package");
        ensure_dir(&download_dir).await?;

        let checkout_path = ctx
            .tarball_client
            .download_and_extract_tarball(&self.tarball, &download_dir, true, None, &[], None)
            .await?;
        let dbt_project =
            read_and_validate_dbt_project(&checkout_path, true, ctx.jinja_env, ctx.vars).await?;
        self.name = Some(dbt_project.name.clone());
        let mut transitive_deps = Vec::new();
        if let Some(dbt_packages) = load_dbt_packages(&checkout_path).await?.0 {
            ctx.notices.collect(&dbt_packages);
            transitive_deps = dbt_packages.packages;
        }
        Ok(ResolvedPackage {
            project_name: dbt_project.name,
            pinned_version: None,
            transitive_deps,
        })
    }

    async fn to_lock_entry(&self, _ctx: &DepsOperationContext<'_>) -> FsResult<DbtPackageLock> {
        let pinned_package: TarballPinnedPackage = self.clone().try_into()?;
        let mut unrendered = pinned_package.unrendered;
        unrendered.remove("name");
        Ok(DbtPackageLock::Tarball(TarballPackageLock {
            tarball: self.original_entry.tarball.clone(),
            name: pinned_package.name,
            __unrendered__: unrendered,
        }))
    }
}

impl UnpinnedPackage {
    pub(crate) async fn resolve(
        &mut self,
        ctx: &DepsOperationContext<'_>,
    ) -> FsResult<ResolvedPackage> {
        match self {
            UnpinnedPackage::Hub(p) => p.resolve(ctx).await,
            UnpinnedPackage::Git(p) => p.resolve(ctx).await,
            UnpinnedPackage::Local(p) => p.resolve(ctx).await,
            UnpinnedPackage::Private(p) => p.resolve(ctx).await,
            UnpinnedPackage::Tarball(p) => p.resolve(ctx).await,
        }
    }

    pub(crate) async fn to_lock_entry(
        &self,
        ctx: &DepsOperationContext<'_>,
    ) -> FsResult<DbtPackageLock> {
        match self {
            UnpinnedPackage::Hub(p) => p.to_lock_entry(ctx).await,
            UnpinnedPackage::Git(p) => p.to_lock_entry(ctx).await,
            UnpinnedPackage::Local(p) => p.to_lock_entry(ctx).await,
            UnpinnedPackage::Private(p) => p.to_lock_entry(ctx).await,
            UnpinnedPackage::Tarball(p) => p.to_lock_entry(ctx).await,
        }
    }
}
