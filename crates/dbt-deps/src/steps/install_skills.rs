//! Installing agent skills once packages are on disk.
//!
//! This runs as part of package installation rather than as a separate command,
//! so skills land at the same moment the packages that ship them do. It reads
//! only files on disk — no profile, no warehouse connection, no manifest.

use std::path::Path;

use dbt_common::{FsResult, stdfs};
use dbt_schemas::schemas::packages::DbtPackagesLock;
use dbt_schemas::schemas::project::DbtProject;
use dbt_skills::{InstalledPackage, install_package_skills};

/// What the skill install pass needs from the surrounding deps run.
pub struct SkillInstallInputs<'a> {
    /// Project root, which is both where `dbt_project.yml` lives and where the
    /// provider directories are written.
    pub in_dir: &'a Path,
    pub packages_install_path: &'a Path,
    pub dbt_packages_lock: &'a DbtPackagesLock,
    pub ai_provider: Option<&'a [String]>,
}

/// Install skills from the root project and every installed package.
///
/// `install_package_skills` owns the "what fails `dbt deps`" policy: it returns
/// an error only for an unresolved skill-name collision (which the user must
/// resolve, like duplicate model names) and warns-and-continues on everything
/// else, so the only failure to propagate here is that collision.
pub fn install_skills(inputs: SkillInstallInputs<'_>) -> FsResult<()> {
    let Some(root_project) = read_root_project(inputs.in_dir) else {
        return Ok(());
    };

    let packages = installed_packages(inputs.dbt_packages_lock, inputs.packages_install_path);

    install_package_skills(inputs.in_dir, &root_project, &packages, inputs.ai_provider)?;
    Ok(())
}

/// Read the root `dbt_project.yml` for `skill-paths`, `skills:` and `flags:`.
///
/// Returns `None` when the file is missing or unreadable; `dbt deps` has other,
/// better error paths for that, so this stays quiet.
fn read_root_project(in_dir: &Path) -> Option<DbtProject> {
    let contents = stdfs::read_to_string(in_dir.join("dbt_project.yml")).ok()?;
    dbt_skills::yaml::from_str::<DbtProject>(&contents).ok()
}

/// The installed packages, each paired with its on-disk root.
///
/// Order does not matter to `enabled` resolution: every skill resolves against
/// its own package's `skills:` config merged with the root project's override,
/// so no package can change another's outcome. Packages are taken in lock order.
fn installed_packages(
    dbt_packages_lock: &DbtPackagesLock,
    packages_install_path: &Path,
) -> Vec<InstalledPackage> {
    dbt_packages_lock
        .packages
        .iter()
        .map(|lock| {
            let name = lock.package_name();
            InstalledPackage {
                root: packages_install_path.join(&name),
                name,
                version: lock.version_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::packages::{DbtPackageLock, HubPackageLock, PackageVersion};

    fn hub_lock(package: &str, name: &str) -> DbtPackageLock {
        DbtPackageLock::Hub(HubPackageLock {
            package: package.to_string(),
            name: name.to_string(),
            version: PackageVersion::String("1.0.0".to_string()),
        })
    }

    #[test]
    fn packages_are_taken_in_lock_order() {
        let lock = DbtPackagesLock {
            packages: vec![
                hub_lock("dbt-labs/alpha", "alpha"),
                hub_lock("dbt-labs/beta", "beta"),
            ],
            sha1_hash: String::new(),
        };

        let packages = installed_packages(&lock, Path::new("dbt_packages"));
        let names: Vec<_> = packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn package_roots_are_under_the_install_path() {
        let lock = DbtPackagesLock {
            packages: vec![hub_lock("dbt-labs/alpha", "alpha")],
            sha1_hash: String::new(),
        };

        let packages = installed_packages(&lock, Path::new("dbt_packages"));
        assert_eq!(packages[0].root, Path::new("dbt_packages/alpha"));
    }
}
