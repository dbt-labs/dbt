//! Package addition functionality for dbt-deps
//!
//! This module provides functionality for adding packages to `packages.yml` files.
//! It includes utilities for:
//! - Converting package strings to structured data
//! - Checking for duplicate packages
//! - Creating properly formatted package entries
//! - Adding packages to existing packages.yml files
//!

use dbt_common::tracing::span_info::SpanStatusRecorder as _;
use dbt_common::{ErrorCode, FsResult, create_info_span, err, stdfs};
use dbt_telemetry::{DepsAddPackage, PackageType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::utils::sanitize_git_url;

/// Package specification with name, optional version, and source type
#[derive(Debug, Clone, PartialEq)]
pub struct PackageSpec {
    pub name: String,
    pub version: Option<String>,
    pub source: PackageType,
}

/// Check if a string looks like a git SSH URL (e.g., `git@github.com:org/repo.git`)
fn is_git_ssh_url(s: &str) -> bool {
    s.starts_with("git@") && s.contains(':')
}

/// Parse a package specification string into a PackageSpec
///
/// Supported formats:
/// - Hub: `org/package@version` or `org/package@">=0.7.0,<0.8.0"`
/// - Git HTTPS: `https://example.com/org/repo.git` or `https://user:token@example.com/org/repo.git@rev`
/// - Git SSH: `git@example.com:org/repo.git` or `git@example.com:org/repo.git@rev`
/// - Tarball: `https://example.com/package.tar.gz`
/// - Local: `path/to/package`
fn parse_package(value: &str) -> PackageSpec {
    // Check for git SSH URL first (git@host:path) - these don't have "://"
    let is_ssh = is_git_ssh_url(value);

    // Split off @version from the end, but be careful with URLs containing @ in credentials
    let (base, version) = if let Some((base_part, version_part)) = value.rsplit_once('@') {
        if is_ssh {
            // For SSH URLs like git@host:path, only split if there are 2+ @ signs
            // git@host:path -> no split (1 @)
            // git@host:path@rev -> split (2 @s)
            if base_part.contains('@') {
                (base_part, Some(version_part.to_string()))
            } else {
                (value, None)
            }
        } else if let Ok(base_url) = url::Url::parse(base_part) {
            // base_part is a valid URL - only split if it has a path (not just credentials)
            // https://token@host/path -> has path, valid split
            // https://token -> no path, don't split (@ is part of URL)
            if !base_url.path().is_empty() && base_url.path() != "/" {
                (base_part, Some(version_part.to_string()))
            } else {
                (value, None)
            }
        } else if value.contains("://") {
            // Looks like a URL but base_part doesn't parse - don't split
            (value, None)
        } else {
            // Not a URL - hub or local package, split at @
            (base_part, Some(version_part.to_string()))
        }
    } else {
        (value, None)
    };

    // Determine package type
    // Check tarball first - can be URL or local path
    let source = if base.ends_with(".tar.gz") || base.ends_with(".tgz") {
        PackageType::Tarball
    } else if is_git_ssh_url(base) {
        PackageType::Git
    } else if url::Url::parse(base).is_ok() {
        // Any other URL is git
        PackageType::Git
    } else {
        // Not a URL - hub package (has version) or local path (no version)
        if version.is_some() {
            PackageType::Hub
        } else {
            PackageType::Local
        }
    };

    PackageSpec {
        name: base.to_string(),
        version,
        source,
    }
}

/// Packages YAML structure
#[derive(Debug, Serialize, Deserialize)]
pub struct PackagesYaml {
    pub packages: Vec<HashMap<String, dbt_yaml::Value>>,
}

/// Filter out duplicate packages in packages.yml so we don't have two entries for the same package
/// Loop through contents of `packages.yml` to ensure no duplicate package names + versions.
/// This will take into consideration exact match of a package name, as well as
/// a check to see if a package name exists within a name (i.e. a package name inside a git URL).
pub fn filter_out_duplicate_packages(
    mut packages_yml: PackagesYaml,
    spec: &PackageSpec,
) -> PackagesYaml {
    packages_yml.packages.retain(|pkg_entry| {
        for val in pkg_entry.values() {
            if let Some(val_str) = val.as_str()
                && val_str.contains(&spec.name)
            {
                return false; // Remove this package
            }
        }
        true // Keep this package
    });

    packages_yml
}

/// Create a formatted entry to add to `packages.yml` or `package-lock.yml` file
fn create_packages_yml_entry(
    package: &str,
    version: Option<&str>,
    source: &str,
) -> HashMap<String, dbt_yaml::Value> {
    let mut packages_yml_entry = HashMap::new();

    let package_key = if source == "hub" { "package" } else { source };
    let version_key = if source == "git" {
        "revision"
    } else {
        "version"
    };

    // Use serde to serialize the package name
    packages_yml_entry.insert(
        package_key.to_string(),
        dbt_yaml::to_value(package).unwrap(),
    );

    // For local packages, version is not applicable
    if source != "local"
        && source != "tarball"
        && let Some(ver) = version
    {
        if ver.contains(',') {
            let versions: Vec<String> = ver.split(',').map(|v| v.trim().to_string()).collect();
            packages_yml_entry.insert(
                version_key.to_string(),
                dbt_yaml::to_value(versions).unwrap(),
            );
        } else {
            packages_yml_entry.insert(version_key.to_string(), dbt_yaml::to_value(ver).unwrap());
        }
    }

    packages_yml_entry
}

/// Add a package to packages.yml
pub fn add_package_to_yml(
    spec: PackageSpec,
    project_root: &str,
    packages_path: &str,
) -> FsResult<()> {
    let packages_yml_filepath = format!("{project_root}/{packages_path}");
    let packages_path = Path::new(&packages_yml_filepath);

    // Create packages.yml if it doesn't exist
    if !packages_path.exists() {
        let initial_content = PackagesYaml {
            packages: Vec::new(),
        };
        let yaml_content = match dbt_yaml::to_string(&initial_content) {
            Ok(yaml) => yaml,
            Err(e) => {
                return err!(
                    ErrorCode::IoError,
                    "Failed to serialize packages.yml: {}",
                    e
                );
            }
        };
        stdfs::write(packages_path, yaml_content)?;
    }

    // Read existing packages.yml
    let yaml_content = stdfs::read_to_string(packages_path)?;
    let mut packages_yml: PackagesYaml = match dbt_yaml::from_str(&yaml_content) {
        Ok(yml) => yml,
        Err(e) => return err!(ErrorCode::IoError, "Failed to parse packages.yml: {}", e),
    };

    // Check for duplicates
    packages_yml = filter_out_duplicate_packages(packages_yml, &spec);

    // Create new package entry
    let source = package_type_to_source(spec.source);
    let new_package_entry = create_packages_yml_entry(&spec.name, spec.version.as_deref(), source);

    // Add the new package
    packages_yml.packages.push(new_package_entry);

    // Write back to file
    let yaml_content = match dbt_yaml::to_string(&packages_yml) {
        Ok(yaml) => yaml,
        Err(e) => {
            return err!(
                ErrorCode::IoError,
                "Failed to serialize packages.yml: {}",
                e
            );
        }
    };
    stdfs::write(packages_path, yaml_content)?;

    Ok(())
}

/// Helper to convert PackageType to source string for YAML entry
fn package_type_to_source(package_type: PackageType) -> &'static str {
    match package_type {
        PackageType::Hub => "hub",
        PackageType::Git => "git",
        PackageType::Tarball => "tarball",
        PackageType::Private => "private",
        PackageType::Local => "local",
        PackageType::Unspecified => unreachable!(),
    }
}

/// Inner implementation that does the actual package addition
fn add_package_inner(spec: PackageSpec, project_dir: &Path) -> FsResult<()> {
    // For local packages, validate that the path exists
    if spec.source == PackageType::Local {
        let local_path = Path::new(&spec.name);
        if local_path.is_absolute() {
            if !local_path.exists() {
                return err!(
                    ErrorCode::InvalidConfig,
                    "Local package path does not exist: {}",
                    spec.name
                );
            }
        } else {
            // For relative paths, check if they exist relative to the project directory
            let full_path = project_dir.join(local_path);
            if !full_path.exists() {
                return err!(
                    ErrorCode::InvalidConfig,
                    "Local package path does not exist: {} (resolved to: {})",
                    spec.name,
                    full_path.display()
                );
            }
        }
    }

    // Convert project_dir to string for the function call
    let project_root = match project_dir.to_str() {
        Some(root) => root,
        None => {
            return err!(ErrorCode::InvalidConfig, "Invalid project directory path");
        }
    };

    // Add the package to packages.yml
    match add_package_to_yml(spec, project_root, "packages.yml") {
        Ok(()) => Ok(()),
        Err(e) => err!(
            ErrorCode::IoError,
            "Failed to add package to packages.yml: {}",
            e
        ),
    }
}

/// Main function to add a package to packages.yml
/// This function handles conversion, telemetry span creation, and package addition
///
/// TODO: Currently doesn't support user-specified source type like dbt-core does.
/// The source is auto-detected from the package string format.
pub fn add_package(package_str: &str, project_dir: &Path) -> FsResult<()> {
    // Parse the package string (includes type detection)
    let spec = parse_package(package_str);

    // Create telemetry span for the add operation
    let add_span = create_info_span(DepsAddPackage::start(
        sanitize_git_url(&spec.name),
        spec.source,
        spec.version.clone(),
    ));

    let _guard = add_span.enter();

    // Execute the add operation within the span
    add_package_inner(spec, project_dir).record_status(&add_span)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_package() {
        // Define test cases: (input, expected_name, expected_version, expected_source)
        let cases: Vec<(&str, &str, Option<&str>, PackageType)> = vec![
            // Hub packages
            (
                "dbt-labs/dbt-utils",
                "dbt-labs/dbt-utils",
                None,
                PackageType::Local,
            ), // no version = local
            (
                "dbt-labs/dbt-utils@1.0.0",
                "dbt-labs/dbt-utils",
                Some("1.0.0"),
                PackageType::Hub,
            ),
            (
                "dbt-labs/dbt-utils@>=1.0.0,<2.0.0",
                "dbt-labs/dbt-utils",
                Some(">=1.0.0,<2.0.0"),
                PackageType::Hub,
            ),
            // Git HTTPS without credentials
            (
                "https://example.com/fake-org/fake-repo.git",
                "https://example.com/fake-org/fake-repo.git",
                None,
                PackageType::Git,
            ),
            (
                "https://example.com/fake-org/fake-repo.git@main",
                "https://example.com/fake-org/fake-repo.git",
                Some("main"),
                PackageType::Git,
            ),
            (
                "https://example.com/fake-org/fake-repo.git@v1.2.3",
                "https://example.com/fake-org/fake-repo.git",
                Some("v1.2.3"),
                PackageType::Git,
            ),
            // Git HTTPS with credentials (token in URL)
            (
                "https://ghp_token123@example.com/fake-org/fake-repo.git",
                "https://ghp_token123@example.com/fake-org/fake-repo.git",
                None,
                PackageType::Git,
            ),
            (
                "https://ghp_token123@example.com/fake-org/fake-repo.git@main",
                "https://ghp_token123@example.com/fake-org/fake-repo.git",
                Some("main"),
                PackageType::Git,
            ),
            (
                "https://user:secret@example.com/fake-org/fake-repo.git@v1.0.0",
                "https://user:secret@example.com/fake-org/fake-repo.git",
                Some("v1.0.0"),
                PackageType::Git,
            ),
            // Git SSH URLs
            (
                "git@example.com:fake-org/fake-repo.git",
                "git@example.com:fake-org/fake-repo.git",
                None,
                PackageType::Git,
            ),
            (
                "git@example.com:fake-org/fake-repo.git@main",
                "git@example.com:fake-org/fake-repo.git",
                Some("main"),
                PackageType::Git,
            ),
            (
                "git@example.com:fake-org/fake-repo.git@v2.0.0",
                "git@example.com:fake-org/fake-repo.git",
                Some("v2.0.0"),
                PackageType::Git,
            ),
            // Tarball URLs
            (
                "https://example.com/packages/fake-package.tar.gz",
                "https://example.com/packages/fake-package.tar.gz",
                None,
                PackageType::Tarball,
            ),
            (
                "https://example.com/releases/v1.0.0.tgz",
                "https://example.com/releases/v1.0.0.tgz",
                None,
                PackageType::Tarball,
            ),
            // Local tarball paths
            (
                "./packages/my-package.tar.gz",
                "./packages/my-package.tar.gz",
                None,
                PackageType::Tarball,
            ),
            (
                "/absolute/path/to/package.tgz",
                "/absolute/path/to/package.tgz",
                None,
                PackageType::Tarball,
            ),
            // Local paths (no version = local)
            (
                "packages/my-local-package",
                "packages/my-local-package",
                None,
                PackageType::Local,
            ),
            (
                "./relative/path/to/pkg",
                "./relative/path/to/pkg",
                None,
                PackageType::Local,
            ),
            (
                "/absolute/path/to/pkg",
                "/absolute/path/to/pkg",
                None,
                PackageType::Local,
            ),
        ];

        // Run all cases and collect failures
        let mut failures = Vec::new();
        for (input, expected_name, expected_version, expected_source) in cases {
            let result = parse_package(input);
            let expected_version = expected_version.map(|s| s.to_string());

            if result.name != expected_name
                || result.version != expected_version
                || result.source != expected_source
            {
                failures.push(format!(
                    "Input: {input:?}\n  Expected: name={expected_name:?}, version={expected_version:?}, source={expected_source:?}\n  Got:      name={:?}, version={:?}, source={:?}",
                    result.name, result.version, result.source
                ));
            }
        }

        // Report all failures at once
        if !failures.is_empty() {
            panic!("parse_package tests failed:\n\n{}", failures.join("\n\n"));
        }
    }

    #[test]
    fn test_create_packages_yml_entry_hub() {
        let entry = create_packages_yml_entry("test-package", Some("1.0.0"), "hub");
        assert_eq!(
            entry.get("package").unwrap().as_str().unwrap(),
            "test-package"
        );
        assert_eq!(entry.get("version").unwrap().as_str().unwrap(), "1.0.0");
    }

    #[test]
    fn test_create_packages_yml_entry_git() {
        let entry = create_packages_yml_entry("test-package", Some("main"), "git");
        assert_eq!(entry.get("git").unwrap().as_str().unwrap(), "test-package");
        assert_eq!(entry.get("revision").unwrap().as_str().unwrap(), "main");
    }

    #[test]
    fn test_create_packages_yml_entry_with_multiple_versions() {
        let entry = create_packages_yml_entry("test-package", Some("1.0.0,1.1.0"), "hub");
        assert_eq!(
            entry.get("package").unwrap().as_str().unwrap(),
            "test-package"
        );

        if let Some(versions) = entry.get("version").unwrap().as_sequence() {
            assert_eq!(versions.len(), 2);
            assert_eq!(versions[0].as_str().unwrap(), "1.0.0");
            assert_eq!(versions[1].as_str().unwrap(), "1.1.0");
        } else {
            panic!("Expected sequence for multiple versions");
        }
    }

    #[test]
    fn test_create_packages_yml_entry_local() {
        let entry = create_packages_yml_entry("packages/my-local-package", None, "local");
        assert_eq!(
            entry.get("local").unwrap().as_str().unwrap(),
            "packages/my-local-package"
        );
        // Local packages should not have a version field
        assert!(!entry.contains_key("version"));
    }

    #[test]
    fn test_add_local_package_integration() {
        use std::fs;
        use tempfile::TempDir;

        // Create a temporary directory structure
        let temp_dir = TempDir::new().unwrap();
        let project_dir = temp_dir.path();

        // Create a local package directory
        let local_package_dir = project_dir.join("packages").join("my-local-package");
        fs::create_dir_all(&local_package_dir).unwrap();

        // Create a dbt_project.yml in the local package to make it valid
        let dbt_project_content = r#"
name: "my-local-package"
version: "1.0.0"
config-version: 2
"#;
        stdfs::write(
            local_package_dir.join("dbt_project.yml"),
            dbt_project_content,
        )
        .unwrap();

        // Test adding the local package
        let result = add_package("packages/my-local-package", project_dir);
        assert!(result.is_ok(), "Failed to add local package: {result:?}");

        // Verify the packages.yml was created with correct content
        let packages_yml_path = project_dir.join("packages.yml");
        assert!(packages_yml_path.exists(), "packages.yml should be created");

        let content = fs::read_to_string(&packages_yml_path).unwrap();
        assert!(
            content.contains("local: packages/my-local-package"),
            "packages.yml should contain local package entry"
        );
        assert!(
            !content.contains("version:"),
            "local packages should not have version field"
        );
    }
}
