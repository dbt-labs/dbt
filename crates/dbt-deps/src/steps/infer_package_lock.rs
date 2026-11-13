use dbt_common::constants::DBT_PROJECT_YML;
use dbt_common::io_args::IoArgs;
use dbt_common::stdfs;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schemas::schemas::packages::{
    DbtPackageEntry, DbtPackageLock, DbtPackages, DbtPackagesLock, HubPackage, HubPackageLock,
    LocalPackage, LocalPackageLock, PackageVersion,
};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::steps::load_dbt_packages;
use crate::utils::{get_local_package_full_path, read_and_validate_dbt_project};

/// Builds a package lock from the packages already installed in `dbt_packages_dir`,
/// without contacting the Package Hub.
///
/// Commands other than `dbt deps` only need the lock to check what's installed and to map
/// each `packages.yml` entry to its installed package name. When packages can't be resolved,
/// for example because the Package Hub is unreachable, they use this lock to load what's
/// installed.
///
/// Walks the root package entries and the `packages.yml`/`dependencies.yml` of every
/// installed package. Returns `None` when an entry can't be matched to an installed package
/// without the network: git, private and tarball entries, Jinja in a hub name or local path,
/// or a package that isn't installed. The caller then returns the resolution error.
pub async fn try_infer_dbt_packages_lock_from_installed(
    io: &IoArgs,
    dbt_packages_dir: &Path,
    dbt_packages: &DbtPackages,
    jinja_env: &JinjaEnv,
    vars: &BTreeMap<String, dbt_yaml::Value>,
) -> Option<DbtPackagesLock> {
    let mut pending: VecDeque<DbtPackageEntry> = dbt_packages.packages.iter().cloned().collect();
    let mut seen_entries = HashSet::new();
    let mut seen_names = HashSet::new();
    let mut packages = Vec::new();

    while let Some(entry) = pending.pop_front() {
        let InstalledEntry { lock, source_dir } = match entry {
            DbtPackageEntry::Hub(hub) => {
                installed_hub_entry(hub, dbt_packages_dir, jinja_env, vars).await?
            }
            DbtPackageEntry::Local(local) => {
                installed_local_entry(io, local, dbt_packages_dir, jinja_env, vars).await?
            }
            DbtPackageEntry::Git(_) | DbtPackageEntry::Private(_) | DbtPackageEntry::Tarball(_) => {
                return None;
            }
        };
        // Repeated entries are only walked once.
        if !seen_entries.insert(lock.lookup_key(&io.in_dir)) {
            continue;
        }
        // Same as `compute_package_lock`: keep the first package with a given name.
        if seen_names.insert(lock.package_name()) {
            packages.push(lock);
        }
        if let Some(transitive) = load_dbt_packages(&source_dir).await.ok()?.0 {
            pending.extend(transitive.packages);
        }
    }

    packages.sort_by_key(|package| package.package_name());
    Some(DbtPackagesLock {
        packages,
        // Never written to disk: an inferred lock doesn't claim to match packages.yml.
        sha1_hash: String::new(),
    })
}

struct InstalledEntry {
    lock: DbtPackageLock,
    /// Directory holding the package's own `packages.yml`/`dependencies.yml`.
    source_dir: PathBuf,
}

async fn installed_hub_entry(
    hub: HubPackage,
    dbt_packages_dir: &Path,
    jinja_env: &JinjaEnv,
    vars: &BTreeMap<String, dbt_yaml::Value>,
) -> Option<InstalledEntry> {
    if contains_jinja(&hub.package) {
        return None;
    }
    // Hub packages are named `<namespace>/<name>` and installed as `<name>`.
    let name = hub.package.rsplit('/').next()?.to_string();
    let install_dir = installed_package_dir(dbt_packages_dir, &name)?;
    let installed_version = read_and_validate_dbt_project(&install_dir, false, jinja_env, vars)
        .await
        .ok()?
        .version;
    let version = match (installed_version, hub.version) {
        (Some(installed), _) => PackageVersion::String(installed.to_string()),
        (None, Some(requested)) => requested,
        (None, None) => return None,
    };
    Some(InstalledEntry {
        lock: DbtPackageLock::Hub(HubPackageLock {
            package: hub.package,
            name,
            version,
        }),
        source_dir: install_dir,
    })
}

async fn installed_local_entry(
    io: &IoArgs,
    local: LocalPackage,
    dbt_packages_dir: &Path,
    jinja_env: &JinjaEnv,
    vars: &BTreeMap<String, dbt_yaml::Value>,
) -> Option<InstalledEntry> {
    if contains_jinja(&local.local.to_string_lossy()) {
        return None;
    }
    // `local:` paths, including those in an installed package's packages.yml, are relative
    // to the root project.
    let full_path = get_local_package_full_path(&io.in_dir, &local);
    let name = read_and_validate_dbt_project(&full_path, false, jinja_env, vars)
        .await
        .ok()?
        .name;
    installed_package_dir(dbt_packages_dir, &name)?;
    Some(InstalledEntry {
        lock: DbtPackageLock::Local(LocalPackageLock {
            local: stdfs::diff_paths(&full_path, &io.in_dir).ok()?,
            name,
        }),
        source_dir: full_path,
    })
}

fn installed_package_dir(dbt_packages_dir: &Path, name: &str) -> Option<PathBuf> {
    let install_dir = dbt_packages_dir.join(name);
    install_dir
        .join(DBT_PROJECT_YML)
        .exists()
        .then_some(install_dir)
}

fn contains_jinja(value: &str) -> bool {
    value.contains("{{") || value.contains("{%")
}
