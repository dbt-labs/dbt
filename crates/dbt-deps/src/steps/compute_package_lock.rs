use dbt_common::FsResult;
use dbt_schemas::schemas::packages::{DbtPackages, DbtPackagesLock};
use std::collections::HashSet;

use crate::notices::{PackageNotice, PackageNoticeKind};
use crate::package_listing::{PackageListing, UnpinnedPackage};
use crate::package_resolver::ResolvedPackage;
use crate::{package_checksum::fusion_sha1_hash_packages, utils::max_resolve_concurrency};

use crate::context::DepsOperationContext;

pub async fn compute_package_lock(
    ctx: &DepsOperationContext<'_>,
    dbt_packages: &DbtPackages,
) -> FsResult<DbtPackagesLock> {
    let sha1_hash = fusion_sha1_hash_packages(
        &dbt_packages.packages,
        ctx.use_v2_compatible_package_downloads,
    );
    let mut dbt_packages_lock = DbtPackagesLock::default();
    let mut package_listing = PackageListing::new(ctx.io.clone(), ctx.vars.clone(), &ctx.notices)
        .with_skip_private_deps(ctx.skip_private_deps)
        .with_private_package_resolver(ctx.private_package_resolver.clone())
        .with_cloud_config(ctx.cloud_config.clone());
    package_listing
        .hydrate_dbt_packages(dbt_packages, ctx.jinja_env)
        .await?;
    let mut final_listing = PackageListing::new(ctx.io.clone(), ctx.vars.clone(), &ctx.notices)
        .with_skip_private_deps(ctx.skip_private_deps)
        .with_private_package_resolver(ctx.private_package_resolver.clone())
        .with_cloud_config(ctx.cloud_config.clone());
    resolve_packages(ctx, &mut final_listing, &mut package_listing).await?;

    let mut final_keys: Vec<_> = final_listing.packages.keys().cloned().collect();
    final_keys.sort();
    for key in final_keys {
        let package = final_listing.packages.get(&key).expect("sorted key exists");
        dbt_packages_lock
            .packages
            .push(package.to_lock_entry(ctx).await?);
    }

    dbt_packages_lock.sha1_hash = sha1_hash;
    // Note: This is currently sorting by package name, but there's more to do here
    dbt_packages_lock.packages.sort_by_key(|a| a.package_name());
    // Deduplicate packages with the same project name, keeping only the first occurrence.
    // This handles cases where packages from different sources (e.g., calogica/dbt_date and
    // godatadriven/dbt_date) resolve to the same project name. dbt-core allows this and
    // deduplicates during resolution. We do the same here by removing duplicates.
    // Note: We intentionally do NOT error on duplicate package names here.
    // dbt-core handles duplicate package names by merging them during resolution
    // via the incorporate() method in the PackageListing. It only checks for
    // duplicate *project* names after fetching metadata, not duplicate package names.
    let mut seen = HashSet::new();
    dbt_packages_lock.packages.retain(|package| {
        let lookup_name = package.package_name();
        if seen.contains(&lookup_name) {
            ctx.notices.record(PackageNotice {
                key: lookup_name,
                kind: PackageNoticeKind::DuplicatePackageName,
            });
            false
        } else {
            seen.insert(lookup_name);
            true
        }
    });
    Ok(dbt_packages_lock)
}

async fn resolve_packages(
    ctx: &DepsOperationContext<'_>,
    final_listing: &mut PackageListing<'_>,
    package_listing: &mut PackageListing<'_>,
) -> FsResult<()> {
    let mut next_listing = PackageListing::new(ctx.io.clone(), ctx.vars.clone(), &ctx.notices)
        .with_skip_private_deps(package_listing.skip_private_deps)
        .with_private_package_resolver(ctx.private_package_resolver.clone())
        .with_cloud_config(ctx.cloud_config.clone());

    let mut keys: Vec<_> = package_listing.packages.keys().cloned().collect();
    keys.sort();
    ctx.check_cancellation()?;

    // Resolve concurrently within the level; apply results in sorted order
    // so next/final listings stay deterministic.
    let mut taken: Vec<UnpinnedPackage> = keys
        .iter()
        .map(|k| {
            package_listing
                .packages
                .remove(k)
                .expect("sorted key exists")
        })
        .collect();
    let resolved = resolve_level(ctx, &mut taken).await?;

    for (pkg, resolved) in taken.iter().zip(resolved) {
        ctx.check_cancellation()?;
        if !resolved.transitive_deps.is_empty() {
            next_listing
                .update_from(&resolved.transitive_deps, ctx.jinja_env)
                .await?;
        }
        final_listing.incorporate_unpinned_package(pkg)?;
    }

    if !next_listing.packages.is_empty() {
        Box::pin(resolve_packages(ctx, final_listing, &mut next_listing)).await?;
    }
    Ok(())
}

async fn resolve_level(
    ctx: &DepsOperationContext<'_>,
    packages: &mut [UnpinnedPackage],
) -> FsResult<Vec<ResolvedPackage>> {
    let max_concurrency = max_resolve_concurrency();
    let mut results = Vec::with_capacity(packages.len());

    for chunk in packages.chunks_mut(max_concurrency) {
        ctx.check_cancellation()?;
        let chunk_results =
            futures::future::try_join_all(chunk.iter_mut().map(|pkg| pkg.resolve(ctx))).await?;
        results.extend(chunk_results);
    }

    Ok(results)
}
