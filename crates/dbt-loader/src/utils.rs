use dbt_common::io_args::IoArgs;
use dbt_common::path::DbtPath;
use dbt_common::{
    ErrorCode, FsResult,
    constants::{DBT_DEPENDENCIES_YML, DBT_PACKAGES_YML},
    fs_err, stdfs,
};
use dbt_jinja_utils::serde::from_yaml_raw;
use pathdiff::diff_paths;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
};

use dbt_schemas::schemas::packages::{DbtPackageEntry, DbtPackages};
use fs_deps::utils::get_local_package_full_path;
use serde::de::DeserializeOwned;
use std::{fs::metadata, io, time::SystemTime};

use ignore::gitignore::Gitignore;
use walkdir::WalkDir;

// ------------------------------------------------------------------------------------------------
// path, directory, and file stuff

pub fn collect_file_info<P: AsRef<Path>, T: Fn(&Path) -> bool>(
    base_path: P,
    relative_paths: &[String],
    info_paths: &mut Vec<(DbtPath, SystemTime)>,
    dbtignore: Option<&Gitignore>,
    filter: T,
) -> io::Result<()> {
    if !base_path.as_ref().exists() {
        return Ok(());
    }
    for relative_path in relative_paths {
        let full_path = base_path.as_ref().join(relative_path);
        if !full_path.exists() {
            continue;
        }
        // Configure WalkDir to respect gitignore patterns at the directory level
        let walker = WalkDir::new(full_path.clone());

        // Process files as normal, but use a filter function to skip directories that match gitignore
        for entry_result in walker.into_iter().filter_entry(|e| {
            let diff_path = diff_paths(e.path(), &full_path).unwrap();
            if !filter(diff_path.as_path()) {
                return false;
            }

            // If there's no gitignore or if this is not a directory, always process it
            if dbtignore.is_none() || !e.file_type().is_dir() {
                return true;
            }

            // For directories, check if they should be included
            let rel_path = e
                .path()
                .strip_prefix(base_path.as_ref())
                .unwrap_or_else(|_| e.path());
            !dbtignore.unwrap().matched(rel_path, true).is_ignore()
        }) {
            let entry = entry_result?;
            if entry.file_type().is_file()
                || (entry.file_type().is_symlink() && entry.path().is_file())
            {
                // Match dbt-core's [!.#~]* filename discovery pattern. Check only the
                // basename so files inside dot-prefixed directories remain discoverable.
                let file_name = entry.file_name().to_string_lossy();
                if file_name
                    .as_bytes()
                    .first()
                    .is_some_and(|byte| matches!(byte, b'.' | b'#' | b'~'))
                {
                    continue;
                }
                // Skip macOS AppleDouble resource fork files (._*) — they are never dbt assets
                // and contain binary metadata that causes UTF-8 read failures on Linux.
                if file_name.starts_with("._") {
                    continue;
                }
                // Check if this file should be ignored by .dbtignore
                if let Some(gitignore) = dbtignore {
                    let path = entry.path();
                    let relative_to_base = path.strip_prefix(base_path.as_ref()).unwrap_or(path);
                    let is_dir = entry.file_type().is_dir();
                    if gitignore.matched(relative_to_base, is_dir).is_ignore() {
                        continue; // Skip this file as it's ignored
                    }
                }
                let metadata = metadata(entry.path())?;
                let modified_time = metadata.modified()?;
                info_paths.push((DbtPath::from(entry.path()), modified_time));
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------
// string stuff
pub fn indent(data: &str, spaces: usize) -> String {
    let indent = " ".repeat(spaces);
    data.lines()
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<String>>()
        .join("\n")
}

// ------------------------------------------------------------------------------------------------
// stupid other helpers:

// TODO: this function should read to a yaml::Value so as to avoid double-io
///
/// `dependency_package_name` is used to determine if the file is part of a dependency package,
/// which affects how errors are reported.
pub fn load_raw_yml<T: DeserializeOwned>(
    path: &Path,
    dependency_package_name: Option<&str>,
) -> FsResult<T> {
    let mut file = std::fs::File::open(path).map_err(|e| {
        fs_err!(
            code => ErrorCode::IoError,
            loc => path.to_path_buf(),
            "Cannot open file dbt_project.yml: {}",
            e,
        )
    })?;
    let mut data = String::new();
    file.read_to_string(&mut data).map_err(|e| {
        fs_err!(
            code => ErrorCode::IoError,
            loc => path.to_path_buf(),
            "Cannot read file dbt_project.yml: {}",
            e,
        )
    })?;

    from_yaml_raw(&data, Some(path), true, dependency_package_name)
}

fn process_package_file(
    io_args: &IoArgs,
    package_file_path: &Path,
    package_lookup_map: &BTreeMap<String, String>,
    dependency_package_name: Option<&str>,
) -> FsResult<BTreeSet<String>> {
    // If the lookup map is empty, it means no packages were defined in the main project.
    // We can't resolve any dependencies, so return an empty set.
    if package_lookup_map.is_empty() {
        return Ok(BTreeSet::new());
    }

    let mut dependencies = BTreeSet::new();
    let dbt_packages: DbtPackages = load_raw_yml(package_file_path, dependency_package_name)?;
    for package in dbt_packages.packages {
        let entry_name = match package {
            DbtPackageEntry::Hub(hub_package) => hub_package.package,
            DbtPackageEntry::Git(git_package) => {
                let mut key = (*git_package.git).clone();
                if let Some(subdirectory) = &git_package.subdirectory {
                    key.push_str(&format!("#{subdirectory}"));
                }
                key
            }
            DbtPackageEntry::Local(local_package) => {
                // Resolve `local:` paths against the root project's in_dir — matching the
                // convention dbt-deps uses when writing package-lock.yml. Even for a
                // transitive packages.yml inside dbt_packages/<pkg>/, a relative `local:`
                // entry refers to a directory relative to the root, not to the package's
                // own dir. Canonicalize to also handle absolute paths (fusion #1337) and
                // any `./` segments uniformly with DbtPackageLock::lookup_key.
                let full_path = get_local_package_full_path(&io_args.in_dir, &local_package);
                stdfs::canonicalize(&full_path)
                    .unwrap_or(full_path)
                    .to_string_lossy()
                    .to_string()
            }
            DbtPackageEntry::Private(private_package) => {
                let mut key = (*private_package.private).clone();
                if let Some(subdirectory) = &private_package.subdirectory {
                    key.push_str(&format!("#{subdirectory}"));
                }
                key
            }
            DbtPackageEntry::Tarball(tarball_package) => (*tarball_package.tarball).clone(),
        };
        if let Some(entry_name) = package_lookup_map.get(&entry_name) {
            dependencies.insert(entry_name.to_string());
        } else {
            // Package not found in lookup map - this can happen when loading from package-lock.yml
            // without packages.yml, and an installed package has dependencies not in the lock file.
            // We skip this dependency rather than error out.
            use dbt_common::tracing::dbt_emit::emit_warn_log_message;
            emit_warn_log_message(
                ErrorCode::InvalidConfig,
                format!(
                    "Package dependency '{}' not found in package-lock.yml. Skipping. \
                     Run 'fs deps --upgrade' with a packages.yml to resolve all dependencies.",
                    entry_name
                ),
            );
        }
    }
    Ok(dependencies)
}

pub fn identify_package_dependencies(
    io_args: &IoArgs,
    in_dir: &Path,
    package_lookup_map: &BTreeMap<String, String>,
    dependency_package_name: Option<&str>,
) -> FsResult<BTreeSet<String>> {
    let mut dependencies = BTreeSet::new();

    // Process dependencies.yml if it exists
    let dependencies_yml_path = in_dir.join(DBT_DEPENDENCIES_YML);
    if dependencies_yml_path.exists() {
        dependencies.extend(process_package_file(
            io_args,
            &dependencies_yml_path,
            package_lookup_map,
            dependency_package_name,
        )?);
    }

    // Process packages.yml if it exists
    let packages_yml_path = in_dir.join(DBT_PACKAGES_YML);
    if packages_yml_path.exists() {
        dependencies.extend(process_package_file(
            io_args,
            &packages_yml_path,
            package_lookup_map,
            dependency_package_name,
        )?);
    }

    Ok(dependencies)
}

#[cfg(all(test, unix))]
mod tests {
    use super::collect_file_info;
    use std::os::unix::fs::symlink;

    #[test]
    fn collect_file_info_includes_symlinked_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let models_dir = temp_dir.path().join("models");
        std::fs::create_dir(&models_dir).unwrap();
        std::fs::write(models_dir.join("shared.sql"), "select 1").unwrap();
        symlink("shared.sql", models_dir.join("linked.sql")).unwrap();

        let mut paths = Vec::new();
        collect_file_info(
            temp_dir.path(),
            &["models".to_string()],
            &mut paths,
            None,
            |_| true,
        )
        .unwrap();

        assert!(
            paths
                .iter()
                .any(|(path, _)| path.as_path() == models_dir.join("linked.sql"))
        );
    }

    #[test]
    fn collect_file_info_matches_core_filename_exclusions() {
        let dir = tempfile::tempdir().unwrap();
        let models = dir.path().join("models");
        std::fs::create_dir_all(models.join(".archive")).unwrap();
        for name in [
            "selected_model.sql",
            ".hidden_source.sql",
            "#temporary.sql",
            "~backup.sql",
        ] {
            std::fs::write(models.join(name), "select 1").unwrap();
        }
        std::fs::write(models.join(".archive/visible.sql"), "select 1").unwrap();
        std::fs::write(models.join(".archive/.hidden.sql"), "select 1").unwrap();

        let mut files = Vec::new();
        collect_file_info(
            dir.path(),
            &["models".to_string()],
            &mut files,
            None,
            |_| true,
        )
        .unwrap();

        let mut paths: Vec<_> = files
            .into_iter()
            .map(|(path, _)| path.to_string_lossy().to_string())
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                models
                    .join(".archive/visible.sql")
                    .to_string_lossy()
                    .to_string(),
                models
                    .join("selected_model.sql")
                    .to_string_lossy()
                    .to_string(),
            ]
        );
    }
}
