use std::collections::HashSet;
use std::path::{Component, Path};

use dbt_common::tracing::dbt_emit::emit_info_log_message;
use dbt_common::tracing::span_info::find_and_update_span_attrs;
use dbt_common::{ErrorCode, FsResult, constants::DBT_PACKAGES_LOCK_FILE, fs_err, stdfs};
use dbt_schemas::schemas::packages::{DbtPackageLock, DbtPackagesLock};
use dbt_telemetry::DepsAllPackagesInstalled;
use dbt_yaml::Verbatim;

use crate::context::DepsOperationContext;
use crate::package_installer::StagedPackage;
use crate::package_listing::{PackageListing, UnpinnedPackage};
use crate::utils::{max_resolve_concurrency, scrub_secret_env_vars};

fn package_lock_needs_scrub(package: &DbtPackageLock) -> bool {
    match package {
        DbtPackageLock::Git(git_package_lock) => {
            scrub_secret_env_vars(git_package_lock.git.as_str()).is_some()
        }
        DbtPackageLock::Tarball(tarball_package_lock) => {
            scrub_secret_env_vars(tarball_package_lock.tarball.as_str()).is_some()
        }
        _ => false,
    }
}

fn scrub_package_lock_for_file(dbt_packages_lock: &mut DbtPackagesLock) {
    for package in dbt_packages_lock.packages.iter_mut() {
        match package {
            DbtPackageLock::Git(git_package_lock) => {
                if let Some(scrubbed) = scrub_secret_env_vars(git_package_lock.git.as_str()) {
                    git_package_lock.git = Verbatim::from(scrubbed.into_owned());
                }
            }
            DbtPackageLock::Tarball(tarball_package_lock) => {
                if let Some(scrubbed) = scrub_secret_env_vars(tarball_package_lock.tarball.as_str())
                {
                    tarball_package_lock.tarball = Verbatim::from(scrubbed.into_owned());
                }
            }
            _ => {}
        }
    }
}

/// Installs each locked package into its own scratch dir under `packages_install_path`,
/// then moves each package out of its scratch dir to `<packages_install_path>/<name>`.
/// A failed download leaves the previous packages and `package-lock.yml` unchanged.
pub async fn install_packages(
    ctx: &DepsOperationContext<'_>,
    dbt_packages_lock: &DbtPackagesLock,
    packages_install_path: &Path,
) -> FsResult<()> {
    let package_lock_str = if dbt_packages_lock
        .packages
        .iter()
        .any(package_lock_needs_scrub)
    {
        let mut scrubbed = DbtPackagesLock {
            packages: dbt_packages_lock.packages.clone(),
            sha1_hash: dbt_packages_lock.sha1_hash.clone(),
        };
        scrub_package_lock_for_file(&mut scrubbed);
        dbt_yaml::to_string(&scrubbed).unwrap()
    } else {
        dbt_yaml::to_string(dbt_packages_lock).unwrap()
    };
    stdfs::create_dir_all(packages_install_path)?;

    let mut package_listing = PackageListing::new(ctx.io.clone(), ctx.vars.clone(), &ctx.notices)
        .with_skip_private_deps(ctx.skip_private_deps)
        .with_private_package_resolver(ctx.private_package_resolver.clone())
        .with_cloud_config(ctx.cloud_config.clone());
    package_listing
        .hydrate_dbt_packages_lock(dbt_packages_lock, ctx.jinja_env)
        .await?;

    find_and_update_span_attrs(|ev: &mut DepsAllPackagesInstalled| {
        ev.package_count = package_listing.packages.len() as u64
    });

    ctx.check_cancellation()?;

    let to_install: Vec<&UnpinnedPackage> = package_listing
        .packages
        .values()
        .filter(|pkg| {
            if ctx.skip_private_deps
                && let UnpinnedPackage::Private(p) = pkg
            {
                emit_info_log_message(format!(
                    "Skipping private package {} due to --skip-private-deps flag",
                    p.name.as_ref().unwrap_or(&p.private)
                ));
                false
            } else {
                true
            }
        })
        .collect();

    let staged = install_packages_concurrent(ctx, packages_install_path, &to_install).await?;

    // Check every name before any package moves, so a bad one changes nothing.
    for package in &staged {
        check_package_dir_name(&package.name)?;
    }

    let mut installed = HashSet::new();
    for package in staged {
        let target = packages_install_path.join(&package.name);
        remove_entry(&target)?;
        stdfs::rename(&package.path, &target)?;
        installed.insert(package.name);
    }

    // The new packages are in place, so the lock now describes them.
    let packages_lock_path = ctx.io.in_dir.join(DBT_PACKAGES_LOCK_FILE);
    std::fs::write(&packages_lock_path, &package_lock_str).map_err(|e| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to write package-lock.yml file: {}",
            e,
        )
    })?;

    // Remove packages the lock no longer lists, and scratch dirs left by a killed install.
    for entry in stdfs::read_dir(packages_install_path)? {
        let entry = entry.map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to read packages install dir: {}",
                e,
            )
        })?;
        if !installed.contains(entry.file_name().to_string_lossy().as_ref()) {
            remove_entry(&entry.path())?;
        }
    }
    Ok(())
}

/// A package is installed to `<packages_install_path>/<name>`, and `name` comes from the
/// package's own `dbt_project.yml`. It must be a single dir name: an empty name or `..`
/// would make `install_packages` delete the install path or the dir above it.
fn check_package_dir_name(name: &str) -> FsResult<()> {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(dir)), None) if dir == name => Ok(()),
        _ => Err(fs_err!(
            ErrorCode::InvalidConfig,
            "Package name '{}' is not a valid directory name",
            name,
        )),
    }
}

/// Removes a file, dir or symlink. `remove_dir_all` removes a symlink without following it.
fn remove_entry(path: &Path) -> FsResult<()> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.is_file() => stdfs::remove_file(path),
        Ok(_) => stdfs::remove_dir_all(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(fs_err!(
            ErrorCode::IoError,
            "Failed to read {}: {}",
            path.display(),
            e,
        )),
    }
}

async fn install_packages_concurrent(
    ctx: &DepsOperationContext<'_>,
    dest: &Path,
    packages: &[&UnpinnedPackage],
) -> FsResult<Vec<StagedPackage>> {
    let max_concurrency = max_resolve_concurrency();
    let mut staged = Vec::with_capacity(packages.len());
    for chunk in packages.chunks(max_concurrency) {
        ctx.check_cancellation()?;
        staged.extend(
            futures::future::try_join_all(chunk.iter().map(|pkg| pkg.install(ctx, dest))).await?,
        );
    }
    Ok(staged)
}
