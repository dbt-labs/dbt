use dbt_common::io_args::IoArgs;
use dbt_common::tokiofs;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::{ErrorCode, FsResult, constants::DBT_PACKAGES_LOCK_FILE, err, fs_err, stdfs};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::serde::from_yaml_raw;
use dbt_schemas::schemas::packages::{
    DbtPackageEntry, DbtPackageLock, DbtPackages, DbtPackagesLock, DeprecatedDbtPackageLock,
    DeprecatedDbtPackagesLock, GitPackageLock, HubPackageLock, LocalPackageLock, PackageVersion,
    PrivatePackageLock, TarballPackageLock,
};
use std::{
    collections::BTreeMap, collections::HashMap, collections::HashSet, path::Path, str::FromStr,
};

use crate::package_checksum::{core_sha1_hash_package_file, fusion_sha1_hash_packages};
use crate::semver::{Version, VersionSpecifier, versions_compatible};
use crate::steps::DbtPackageType;
use crate::types::HubUnpinnedPackage;
use crate::utils::read_and_validate_dbt_project;

pub async fn try_load_valid_dbt_packages_lock(
    io: &IoArgs,
    dbt_packages_dir: &Path,
    package_definition_path: &Path,
    package_type: DbtPackageType,
    dbt_packages: &DbtPackages,
    jinja_env: &JinjaEnv,
    vars: &BTreeMap<String, dbt_yaml::Value>,
    use_v2_compatible_package_downloads: bool,
) -> FsResult<Option<DbtPackagesLock>> {
    let packages_lock_path = io.in_dir.join(DBT_PACKAGES_LOCK_FILE);
    let sha1_hash =
        fusion_sha1_hash_packages(&dbt_packages.packages, use_v2_compatible_package_downloads);
    if packages_lock_path.exists() {
        let yml_str = tokiofs::read_to_string(&packages_lock_path).await?;
        let rendered_yml: DbtPackagesLock =
            match from_yaml_raw(&yml_str, Some(&packages_lock_path), true, None) {
                Ok(rendered_yml) => rendered_yml,
                Err(e) => {
                    if e.to_string()
                        .contains("not match any variant of untagged enum DbtPackageLock")
                    {
                        return try_load_from_deprecated_dbt_packages_lock(
                            io,
                            dbt_packages_dir,
                            Some(dbt_packages),
                            &yml_str,
                            jinja_env,
                            vars,
                        )
                        .await;
                    }
                    return err!(
                        ErrorCode::IoError,
                        "Failed to parse package-lock.yml file: {}",
                        e
                    );
                }
            };
        if rendered_yml.sha1_hash == sha1_hash {
            return Ok(Some(rendered_yml));
        }
        // Core's checksum has no download-mode marker, so it cannot safely validate a package
        // installation created with v2-compatible Package Hub downloads. A compatibility-hash
        // error remains a conservative cache miss and falls through to normal resolution.
        if !use_v2_compatible_package_downloads
            && core_sha1_hash_package_file(package_definition_path, package_type, jinja_env, vars)
                .await
                .ok()
                .flatten()
                .is_some_and(|core_sha1_hash| rendered_yml.sha1_hash == core_sha1_hash)
        {
            return Ok(Some(rendered_yml));
        }
    }
    Ok(None)
}

// This is a hack to support the old dbt_packages_lock.yml file format
// In the future, we should not support just checking for directory names
async fn try_load_from_deprecated_dbt_packages_lock(
    io: &IoArgs,
    dbt_packages_dir: &Path,
    dbt_packages: Option<&DbtPackages>,
    yml_str: &str,
    jinja_env: &JinjaEnv,
    vars: &BTreeMap<String, dbt_yaml::Value>,
) -> FsResult<Option<DbtPackagesLock>> {
    match from_yaml_raw::<DeprecatedDbtPackagesLock>(yml_str, None, true, None) {
        // Here, we need to do a fuzzy lookup on the old dbt_packages_lock.yml file
        Ok(DeprecatedDbtPackagesLock {
            packages: deprecated_packages,
            sha1_hash,
        }) => {
            emit_warn_log_message(
                ErrorCode::FmtError,
                "Old format package-lock.yml file found. Please provide package definitions.",
            );

            if !dbt_packages_dir.exists() {
                emit_warn_log_message(
                    ErrorCode::FmtError,
                    "Attempted to infer package name from package-lock.yml, but no packages directory found, skipping...",
                );

                return Ok(None);
            }

            // List directories in dbt_packages_dir
            let all_packages = match dbt_packages_dir.read_dir() {
                Ok(dir_entries) => dir_entries.collect::<Result<Vec<_>, _>>().map_err(|e| {
                    fs_err!(
                        ErrorCode::IoError,
                        "Failed to read package directory entries: {}",
                        e
                    )
                })?,
                Err(e) => {
                    emit_warn_log_message(
                        ErrorCode::IoError,
                        format!(
                            "Failed to read packages directory at {}: {}",
                            dbt_packages_dir.display(),
                            e
                        ),
                    );

                    return Ok(None);
                }
            };

            let mut avail_packages = HashSet::new();
            for package in all_packages {
                let package_path = package.path();
                let package_name = package_path.file_name().unwrap().to_str().unwrap();
                avail_packages.insert(package_name.to_lowercase());
            }
            let mut packages = Vec::new();
            for package in deprecated_packages {
                match package {
                    DeprecatedDbtPackageLock::Hub(hub) => {
                        let package = hub.package;
                        let version = hub.version;
                        // Split package name and version (if "/" exists)
                        let parts: Vec<&str> = package.split('/').collect();
                        let package_name = parts.last().expect("Package name should exist");
                        if avail_packages.contains(package_name.to_lowercase().as_str()) {
                            packages.push(DbtPackageLock::Hub(HubPackageLock {
                                package: package.to_string(),
                                name: (*package_name).to_string(),
                                version,
                            }));
                        } else {
                            emit_warn_log_message(
                                ErrorCode::FmtError,
                                format!(
                                    "Attempted to infer package name from package-lock.yml, but package {} not found in '{}', skipping...",
                                    package,
                                    dbt_packages_dir.display()
                                ),
                            );

                            return Ok(None);
                        }
                    }
                    DeprecatedDbtPackageLock::Git(package) => {
                        let git = package.git;
                        let revision = package.revision;
                        let warn_unpinned = package.warn_unpinned;
                        let subdirectory = package.subdirectory;
                        let unrendered = package.__unrendered__;

                        let parts: Vec<&str> = git.split('/').collect();
                        let package_name = parts.last().expect("Package name should exist");
                        if avail_packages.contains(package_name.to_lowercase().as_str()) {
                            packages.push(DbtPackageLock::Git(GitPackageLock {
                                git: git.to_owned().into(),
                                name: (*package_name).to_string(),
                                revision,
                                warn_unpinned,
                                subdirectory,
                                __unrendered__: unrendered,
                            }));
                        } else {
                            emit_warn_log_message(
                                ErrorCode::FmtError,
                                format!(
                                    "Attempted to infer package name from package-lock.yml, but package {} not found in '{}', skipping...",
                                    git,
                                    dbt_packages_dir.display()
                                ),
                            );

                            return Ok(None);
                        }
                    }
                    DeprecatedDbtPackageLock::Local(local) => {
                        let local_path = local.local;
                        // Find the package name from the `dbt_project.yml` file located in the local package
                        let dbt_project_path = if let Ok(dbt_project_path) =
                            stdfs::diff_paths(&local_path, &io.in_dir)
                        {
                            dbt_project_path
                        } else {
                            io.in_dir.join(&local_path)
                        };

                        let dbt_project =
                            read_and_validate_dbt_project(&dbt_project_path, true, jinja_env, vars)
                                .await?;
                        let package_name = dbt_project.name;
                        packages.push(DbtPackageLock::Local(LocalPackageLock {
                            name: package_name,
                            local: local_path,
                        }));
                    }
                    DeprecatedDbtPackageLock::Private(package) => {
                        let private = package.private;
                        let revision = package.revision;
                        let provider = package.provider;
                        let warn_unpinned = package.warn_unpinned;
                        let subdirectory = package.subdirectory.clone();
                        let unrendered = package.__unrendered__;

                        // Parse package name from the private path (e.g., "org/repo" -> "repo")
                        let parts: Vec<&str> = private.split('/').collect();
                        let mut package_name =
                            (*parts.last().expect("Package name should exist")).to_string();

                        // If there's a subdirectory, append it to the package name
                        // This is necessary because the same repo can have multiple packages in different subdirectories
                        if let Some(ref subdir) = subdirectory {
                            let subdir_name = subdir
                                .split('/')
                                .next_back()
                                .unwrap_or(subdir.as_str())
                                .to_string();
                            package_name = subdir_name;
                        }

                        if avail_packages.contains(package_name.to_lowercase().as_str()) {
                            packages.push(DbtPackageLock::Private(PrivatePackageLock {
                                private: private.to_owned().into(),
                                name: package_name,
                                revision,
                                provider,
                                warn_unpinned,
                                subdirectory,
                                __unrendered__: unrendered,
                            }));
                        } else {
                            emit_warn_log_message(
                                ErrorCode::FmtError,
                                format!(
                                    "Attempted to infer package name from package-lock.yml, but package {} not found in '{}', skipping...",
                                    private,
                                    dbt_packages_dir.display()
                                ),
                            );

                            return Ok(None);
                        }
                    }
                    DeprecatedDbtPackageLock::Tarball(package) => {
                        let tarball = package.tarball;
                        let unrendered = package.__unrendered__;

                        // Parse package name from the tarball URL
                        // Try to extract from filename (e.g., "https://example.com/package-1.0.0.tar.gz" -> "package")
                        let url_parts: Vec<&str> = tarball.split('/').collect();
                        let tarball_str = tarball.as_str();
                        let filename = url_parts.last().unwrap_or(&tarball_str);
                        // Remove common extensions and version suffix
                        let package_name = filename
                            .trim_end_matches(".tar.gz")
                            .trim_end_matches(".tgz")
                            .split('-')
                            .next()
                            .unwrap_or(filename)
                            .to_string();

                        if avail_packages.contains(package_name.to_lowercase().as_str()) {
                            packages.push(DbtPackageLock::Tarball(TarballPackageLock {
                                tarball: tarball.to_owned().into(),
                                name: package_name,
                                __unrendered__: unrendered,
                            }));
                        } else {
                            emit_warn_log_message(
                                ErrorCode::FmtError,
                                format!(
                                    "Attempted to infer package name from package-lock.yml, but package {} not found in '{}', skipping...",
                                    tarball,
                                    dbt_packages_dir.display()
                                ),
                            );

                            return Ok(None);
                        }
                    }
                }
            }
            let dbt_packages_lock = DbtPackagesLock {
                packages,
                sha1_hash,
            };
            // HACK: This is a temporary hack to ensure the old package lock format is invalidated (i.e. not loaded)
            // when it conflicts with the packages.yml spec (ex. a version constraint conflicts)
            // We only validate hub packages for now to aid the most common autofix upgrade scenario
            if let Some(dbt_packages) = dbt_packages
                && let Err(e) = validate_deprecated_hub_lock_hack(dbt_packages, &dbt_packages_lock)
            {
                emit_warn_log_message(ErrorCode::InvalidConfig, e.to_string());
                return Ok(None);
            }

            Ok(Some(dbt_packages_lock))
        }
        Err(e) => {
            err!(
                ErrorCode::IoError,
                "Failed to parse package-lock.yml file: {}",
                e
            )
        }
    }
}

/// Load package-lock.yml without validating against packages.yml
/// Used when packages.yml doesn't exist but we want to install from the lock file
/// This matches dbt-core behavior where the lock file can be used independently
pub async fn load_dbt_packages_lock_without_validation(
    io: &IoArgs,
    dbt_packages_dir: &Path,
    jinja_env: &JinjaEnv,
    vars: &BTreeMap<String, dbt_yaml::Value>,
) -> FsResult<Option<DbtPackagesLock>> {
    let packages_lock_path = io.in_dir.join(DBT_PACKAGES_LOCK_FILE);
    if !packages_lock_path.exists() {
        return Ok(None);
    }

    let yml_str = stdfs::read_to_string(&packages_lock_path)?;
    let rendered_yml: DbtPackagesLock =
        match from_yaml_raw(&yml_str, Some(&packages_lock_path), true, None) {
            Ok(rendered_yml) => rendered_yml,
            Err(e) => {
                if e.to_string()
                    .contains("not match any variant of untagged enum DbtPackageLock")
                {
                    // Try loading deprecated format
                    // For deprecated format without packages.yml, we try to infer package names
                    // from the installed packages directory
                    return try_load_from_deprecated_dbt_packages_lock(
                        io,
                        dbt_packages_dir,
                        None,
                        &yml_str,
                        jinja_env,
                        vars,
                    )
                    .await;
                }
                return err!(
                    ErrorCode::IoError,
                    "Failed to parse package-lock.yml file: {}",
                    e
                );
            }
        };

    Ok(Some(rendered_yml))
}

/// Iterates over the packages in the packages.yml and validates the version constraints against the version in the package-lock.yml
fn validate_deprecated_hub_lock_hack(
    dbt_packages: &DbtPackages,
    dbt_packages_lock: &DbtPackagesLock,
) -> FsResult<()> {
    // Build a HashMap for O(1) lookup instead of O(n) linear search per package
    let lock_hub_packages: HashMap<&str, &HubPackageLock> = dbt_packages_lock
        .packages
        .iter()
        .filter_map(|lock| match lock {
            DbtPackageLock::Hub(hub_lock) => Some((hub_lock.package.as_str(), hub_lock)),
            _ => None,
        })
        .collect();

    for hub_package in dbt_packages
        .packages
        .iter()
        .filter_map(|entry| match entry {
            DbtPackageEntry::Hub(hub) => Some(hub),
            _ => None,
        })
    {
        let Some(lock_entry) = lock_hub_packages.get(hub_package.package.as_str()) else {
            return err!(
                ErrorCode::InvalidConfig,
                "Old format package-lock.yml missing hub package '{}'",
                hub_package.package
            );
        };

        let unpinned_package: HubUnpinnedPackage = hub_package.clone().try_into()?;
        let mut versions = unpinned_package.versions;
        versions.push(Version::Spec(lock_version_spec(&lock_entry.version)?));
        if !versions_compatible(&versions) {
            return err!(
                ErrorCode::InvalidConfig,
                "Version '{}' in old format package-lock.yml for package '{}' does not satisfy packages.yml constraint",
                lock_entry.version,
                hub_package.package
            );
        }
    }

    Ok(())
}

fn lock_version_spec(package_version: &PackageVersion) -> FsResult<VersionSpecifier> {
    match package_version {
        PackageVersion::String(version) => VersionSpecifier::from_str(version),
        _ => err!(
            ErrorCode::InvalidConfig,
            "Expected a single resolved version in package-lock.yml, found '{}'",
            package_version
        ),
    }
}
