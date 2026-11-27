use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};

use dbt_common::tracing::dbt_emit::emit_info_log_message;
use dbt_common::tracing::span_info::find_and_update_span_attrs;
use dbt_common::{ErrorCode, FsError, FsResult, constants::DBT_PACKAGES_LOCK_FILE, fs_err, stdfs};
use dbt_schemas::schemas::packages::{DbtPackageLock, DbtPackagesLock};
use dbt_telemetry::DepsAllPackagesInstalled;
use dbt_yaml::Verbatim;
use tempfile::{NamedTempFile, TempDir};

use crate::context::DepsOperationContext;
use crate::package_installer::StagedPackage;
use crate::package_listing::{PackageListing, UnpinnedPackage};
use crate::utils::{make_tempdir, max_resolve_concurrency, scrub_secret_env_vars};

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
/// Keep the previous packages until their replacements and the new lockfile are published.
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

    let packages_lock_path = ctx.io.in_dir.join(DBT_PACKAGES_LOCK_FILE);
    publish_packages(
        &staged,
        packages_install_path,
        &packages_lock_path,
        &package_lock_str,
    )?;

    let installed: HashSet<_> = staged.into_iter().map(|package| package.name).collect();

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

/// Moves the staged packages into place and publishes the lockfile that describes them.
///
/// The lockfile is written to a temporary file before any package moves, so a lockfile
/// that can't be written leaves `packages_install_path` untouched. If a package move or
/// the final rename of the lockfile fails, the previous packages are restored.
fn publish_packages(
    staged: &[StagedPackage],
    packages_install_path: &Path,
    lock_path: &Path,
    lock_contents: &str,
) -> FsResult<()> {
    let lockfile = StagedLockfile::new(lock_path, lock_contents).map_err(lock_write_error)?;
    let backups = make_tempdir(Some(packages_install_path))?;
    let mut replacements = Vec::with_capacity(staged.len());
    swap_in(
        staged,
        packages_install_path,
        backups.path(),
        &mut replacements,
    )
    .and_then(|()| lockfile.commit().map_err(lock_write_error))
    .map_err(|error| roll_back(replacements, backups, error))
}

fn lock_write_error(error: io::Error) -> Box<FsError> {
    fs_err!(
        ErrorCode::IoError,
        "Failed to write package-lock.yml file: {error}"
    )
}

/// A package moved to `target`, and where the package it replaced was backed up.
struct Replacement {
    target: PathBuf,
    previous: Option<PathBuf>,
}

/// Moves each staged package to `<packages_install_path>/<name>`, first moving any
/// package already there into `backups`. Records every move in `replacements`.
fn swap_in(
    staged: &[StagedPackage],
    packages_install_path: &Path,
    backups: &Path,
    replacements: &mut Vec<Replacement>,
) -> FsResult<()> {
    for (index, package) in staged.iter().enumerate() {
        let target = packages_install_path.join(&package.name);
        // The index keeps the backup names unique if two packages share a name.
        let previous = back_up(&target, backups.join(format!("{index}-{}", package.name)))?;
        // Record the move before checking it, so a failed move still restores the backup.
        let moved = stdfs::rename(&package.path, &target);
        replacements.push(Replacement { target, previous });
        moved?;
    }
    Ok(())
}

/// Moves `target` to `backup`, and returns `None` if there's nothing at `target`.
fn back_up(target: &Path, backup: PathBuf) -> FsResult<Option<PathBuf>> {
    match fs::rename(target, &backup) {
        Ok(()) => Ok(Some(backup)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(fs_err!(
            ErrorCode::IoError,
            "Failed to back up package '{}': {error}",
            target.display()
        )),
    }
}

/// Undoes `replacements` in reverse order and returns `error`. If a package can't be
/// restored, keeps `backups` and adds its location to the error.
fn roll_back(
    replacements: Vec<Replacement>,
    backups: TempDir,
    error: Box<FsError>,
) -> Box<FsError> {
    let mut restore_error = None;
    for Replacement { target, previous } in replacements.into_iter().rev() {
        let restored = remove_entry(&target).and_then(|()| match previous {
            Some(backup) => stdfs::rename(backup, target),
            None => Ok(()),
        });
        if let Err(e) = restored {
            restore_error.get_or_insert(e);
        }
    }
    let Some(restore_error) = restore_error else {
        return error;
    };
    // The backups are inside the install path, which the next `dbt deps` cleans up.
    let backup_path = backups.keep();
    Box::new(error.with_chained_errors(fs_err!(
        ErrorCode::IoError,
        "Failed to restore the previous packages. Copy them from '{}' before running dbt deps again, which deletes that directory: {restore_error}",
        backup_path.display()
    )))
}

/// The new lockfile, written beside the old one and published by rename, so a failed
/// write never truncates the old lockfile.
struct StagedLockfile<'a> {
    file: NamedTempFile,
    destination: PathBuf,
    contents: &'a str,
}

impl<'a> StagedLockfile<'a> {
    fn new(path: &Path, contents: &'a str) -> io::Result<Self> {
        // Write through a symlinked lockfile rather than replacing the link.
        let destination = match path.symlink_metadata() {
            Ok(metadata) if metadata.file_type().is_symlink() => path.canonicalize()?,
            Ok(_) => path.to_owned(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => path.to_owned(),
            Err(error) => return Err(error),
        };
        // A rename can replace a read-only lockfile, so refuse one explicitly, even
        // for root. Opening for write checks access without truncating the file.
        let permissions = match OpenOptions::new().write(true).open(&destination) {
            Ok(file) => {
                let permissions = file.metadata()?.permissions();
                if permissions.readonly() {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                Some(permissions)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        let mut builder = tempfile::Builder::new();
        // Like `fs::write`, create a new lockfile with 0o666 less the umask.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            builder.permissions(fs::Permissions::from_mode(0o666));
        }
        let mut file = builder.tempfile_in(parent)?;
        file.write_all(contents.as_bytes())?;
        if let Some(permissions) = permissions {
            file.as_file().set_permissions(permissions)?;
        }
        file.as_file().sync_all()?;
        Ok(Self {
            file,
            destination,
            contents,
        })
    }

    fn commit(self) -> io::Result<()> {
        match self.file.persist(&self.destination) {
            Ok(_) => Ok(()),
            // A lockfile bind-mounted on its own, as in a container, can't be replaced by
            // rename. Write it in place instead, as earlier versions did.
            Err(error)
                if matches!(
                    error.error.kind(),
                    io::ErrorKind::ResourceBusy | io::ErrorKind::CrossesDevices
                ) =>
            {
                fs::write(&self.destination, self.contents)
            }
            Err(error) => Err(error.error),
        }
    }
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
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An install dir holding `old_pkg` at "v1", and a lockfile beside it.
    struct Fixture {
        _root: TempDir,
        install_path: PathBuf,
        lock_path: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let install_path = root.path().join("dbt_packages");
            fs::create_dir_all(install_path.join("old_pkg")).unwrap();
            fs::write(install_path.join("old_pkg/version"), "v1").unwrap();
            let lock_path = root.path().join("package-lock.yml");
            fs::write(&lock_path, "old lock").unwrap();
            Self {
                _root: root,
                install_path,
                lock_path,
            }
        }

        /// Stages `name` at `version`, or a package whose files are missing for `None`.
        fn stage(&self, name: &str, version: Option<&str>) -> StagedPackage {
            let scratch = make_tempdir(Some(&self.install_path)).unwrap();
            let path = scratch.path().join(name);
            if let Some(version) = version {
                fs::create_dir(&path).unwrap();
                fs::write(path.join("version"), version).unwrap();
            }
            StagedPackage::for_test(name, path, scratch)
        }

        fn version(&self, name: &str) -> Option<String> {
            fs::read_to_string(self.install_path.join(name).join("version")).ok()
        }

        fn entries(&self) -> Vec<String> {
            let mut names: Vec<_> = fs::read_dir(&self.install_path)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }

        fn publish(&self, staged: &[StagedPackage]) -> FsResult<()> {
            publish_packages(staged, &self.install_path, &self.lock_path, "new lock")
        }
    }

    #[test]
    fn publishing_replaces_the_packages_and_the_lockfile() {
        let fixture = Fixture::new();
        let staged = vec![
            fixture.stage("new_pkg", Some("v1")),
            fixture.stage("old_pkg", Some("v2")),
        ];

        fixture.publish(&staged).unwrap();
        drop(staged);

        assert_eq!(fixture.version("old_pkg").as_deref(), Some("v2"));
        assert_eq!(fixture.version("new_pkg").as_deref(), Some("v1"));
        assert_eq!(fs::read_to_string(&fixture.lock_path).unwrap(), "new lock");
        assert_eq!(fixture.entries(), ["new_pkg", "old_pkg"]);
    }

    #[test]
    fn a_failed_move_restores_every_package_already_moved() {
        let fixture = Fixture::new();
        // `new_pkg` and `old_pkg` move before `broken_pkg` fails, so both must be undone.
        let staged = vec![
            fixture.stage("new_pkg", Some("v1")),
            fixture.stage("old_pkg", Some("v2")),
            fixture.stage("broken_pkg", None),
        ];

        fixture
            .publish(&staged)
            .expect_err("moving broken_pkg should fail");
        drop(staged);

        assert_eq!(fixture.version("old_pkg").as_deref(), Some("v1"));
        assert_eq!(fs::read_to_string(&fixture.lock_path).unwrap(), "old lock");
        assert_eq!(fixture.entries(), ["old_pkg"]);
    }

    #[test]
    fn a_read_only_lockfile_fails_before_any_package_moves() {
        let fixture = Fixture::new();
        let writable = fs::metadata(&fixture.lock_path).unwrap().permissions();
        let mut read_only = writable.clone();
        read_only.set_readonly(true);
        fs::set_permissions(&fixture.lock_path, read_only).unwrap();
        let staged = vec![fixture.stage("old_pkg", Some("v2"))];

        let result = fixture.publish(&staged);
        // Windows can't delete a read-only file, so restore access before cleanup.
        fs::set_permissions(&fixture.lock_path, writable).unwrap();

        let error = result.expect_err("the lockfile write should fail");

        assert!(
            error
                .to_string()
                .contains("Failed to write package-lock.yml"),
            "unexpected error: {error}"
        );
        assert!(
            staged[0].path.is_dir(),
            "the staged package should not move"
        );
        drop(staged);
        assert_eq!(fixture.version("old_pkg").as_deref(), Some("v1"));
        assert_eq!(fs::read_to_string(&fixture.lock_path).unwrap(), "old lock");
        assert_eq!(fixture.entries(), ["old_pkg"]);
    }

    #[cfg(unix)]
    #[test]
    fn publishing_keeps_the_lockfile_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let fixture = Fixture::new();
        fs::set_permissions(&fixture.lock_path, fs::Permissions::from_mode(0o640)).unwrap();

        fixture.publish(&[]).unwrap();

        let mode = fs::metadata(&fixture.lock_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o640);
        assert_eq!(fs::read_to_string(&fixture.lock_path).unwrap(), "new lock");
    }
}
