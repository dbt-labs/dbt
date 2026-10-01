use std::path::PathBuf;
use std::{collections::HashMap, str::FromStr};

use dbt_common::{ErrorCode, FsError, FsResult, err, fs_err};
use dbt_schemas::schemas::packages::{
    GitPackage, HubPackage, PackageVersion, PrivatePackage, PrivatePackageProvider, TarballPackage,
};
use dbt_yaml::Value as YmlValue;

use super::semver::{
    Matchers, Version, VersionSpecifier, filter_installable, reduce_versions,
    resolve_to_specific_version,
};

use super::hub_client::{DBT_CORE_FIXED_VERSION, HubClient, HubPackageJson, HubPackageVersion};

/// Parse a version string that may be a single semver specifier or a
/// stringified list produced by Jinja rendering (e.g. `['>=0.8.0', '<0.9.0']`).
///
/// dbt-core renders the entire YAML through Jinja before parsing, so a list
/// value stays a YAML sequence.  In dbt-fusion the per-value rendering turns
/// the list into its string representation; this helper recovers the original
/// semantics by re-parsing the string as YAML to recover sequence structure.
fn parse_version_string(version: &str) -> Result<Vec<VersionSpecifier>, Box<FsError>> {
    if let Ok(dbt_yaml::Value::Sequence(items, _)) = dbt_yaml::from_str::<dbt_yaml::Value>(version)
    {
        let specs: Result<Vec<_>, _> = items
            .iter()
            .map(|v| {
                let s = v.as_str().ok_or_else(|| {
                    fs_err!(
                        ErrorCode::InvalidConfig,
                        "Expected string version specifier, got: {:?}",
                        v
                    )
                })?;
                VersionSpecifier::from_str(s)
            })
            .collect();
        return specs;
    }
    Ok(vec![VersionSpecifier::from_str(version)?])
}

#[derive(Debug, Clone)]
pub struct HubPinnedPackage {
    pub package: String,
    pub name: String,
    pub version: String,
    pub version_latest: String,
}

impl HubPinnedPackage {
    pub fn source_type(&self) -> String {
        "hub".to_string()
    }

    pub fn get_version(&self) -> String {
        self.version.clone()
    }

    pub fn get_version_latest(&self) -> String {
        self.version_latest.clone()
    }
}

#[derive(Debug, Clone)]
pub struct HubUnpinnedPackage {
    pub package: String,
    pub versions: Vec<Version>, // Semver Versions
    pub install_prerelease: Option<bool>,
    /// Set by [`crate::package_resolver::PackageResolver::resolve`]; reused for lock entries.
    pub(crate) resolved_hub: Option<ResolvedHubPackage>,
}

/// Pinned hub package paired with the hub metadata used to pin it. Lets
/// callers (notice recording, transitive resolution, v2-download substitution)
/// reuse what [`HubUnpinnedPackage::resolved`] already had to fetch.
#[derive(Clone, Debug)]
pub struct ResolvedHubPackage {
    pub pinned: HubPinnedPackage,
    pub hub_package: HubPackageJson,
    pub version: HubPackageVersion,
}

impl crate::notices::NoticeSource for ResolvedHubPackage {
    fn record_into(&self, buffer: &crate::notices::NoticeBuffer) {
        for n in self.hub_package.deprecation_notices() {
            buffer.record(n);
        }
        if let Some(n) = self.version.version_compat_notice(&self.hub_package.name) {
            buffer.record(n);
        }
    }
}

impl HubUnpinnedPackage {
    pub fn incorporate(&mut self, other: Self) {
        self.versions.extend(other.versions);
        self.resolved_hub = None;
    }

    pub async fn resolved(&self, hub_registry: &HubClient) -> FsResult<ResolvedHubPackage> {
        if !hub_registry.check_index(&self.package).await? {
            return err!(
                ErrorCode::InvalidConfig,
                "Package not found in hub registry: '{}'",
                self.package
            );
        }
        let version_range = reduce_versions(&self.versions)?;
        let hub_package = hub_registry.get_hub_package(&self.package).await?;
        let compatible_versions = hub_registry
            .get_compatible_versions(&hub_package, DBT_CORE_FIXED_VERSION, true)
            .await?;
        let prerelease_specified = self.versions.iter().any(|v| v.is_prerelease());
        let installable = filter_installable(
            &compatible_versions,
            self.install_prerelease.unwrap_or_default() || prerelease_specified,
        )?;
        if installable.is_empty() {
            return err!(
                ErrorCode::InvalidConfig,
                "No compatible versions found for package '{}'",
                self.package
            );
        }
        let Some(resolved_version) = resolve_to_specific_version(&version_range, &installable)?
        else {
            return err!(
                ErrorCode::InvalidConfig,
                "No compatible versions found for package '{}'",
                self.package
            );
        };
        let version = hub_package
            .versions
            .get(&resolved_version)
            .cloned()
            .expect("resolved version should exist in package metadata");
        let pinned = HubPinnedPackage {
            package: self.package.clone(),
            name: hub_package.name.clone(),
            version: resolved_version,
            version_latest: installable.last().unwrap().clone(),
        };
        Ok(ResolvedHubPackage {
            pinned,
            hub_package,
            version,
        })
    }
}

impl TryFrom<HubPackage> for HubUnpinnedPackage {
    type Error = Box<FsError>;

    fn try_from(hub_package: HubPackage) -> Result<Self, Self::Error> {
        let versions = match hub_package.version {
            Some(PackageVersion::Array(versions)) => versions
                .iter()
                .map(|v| VersionSpecifier::from_str(v))
                .collect::<Result<Vec<_>, _>>()?,
            Some(PackageVersion::String(version)) => parse_version_string(&version)?,
            Some(PackageVersion::Number(version)) => {
                vec![VersionSpecifier::from_str(&version.to_string())?]
            }
            None => vec![VersionSpecifier {
                major: None,
                minor: None,
                patch: None,
                prerelease: None,
                build: None,
                matcher: Matchers::Exact,
            }],
        };
        Ok(Self {
            package: hub_package.package,
            versions: versions.into_iter().map(Version::Spec).collect(),
            install_prerelease: hub_package.install_prerelease,
            resolved_hub: None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct GitPinnedPackage {
    pub git: String,
    pub name: String,
    pub revision: String,
    pub warn_unpinned: Option<bool>,
    pub subdirectory: Option<String>,
    pub unrendered: HashMap<String, YmlValue>,
}

#[derive(Debug, Clone)]
pub struct GitUnpinnedPackage {
    pub git: String,
    pub name: Option<String>,
    pub warn_unpinned: Option<bool>,
    pub revisions: Vec<String>,
    pub subdirectory: Option<String>,
    pub unrendered: HashMap<String, YmlValue>,
    pub original_entry: GitPackage,
}

impl GitUnpinnedPackage {
    pub fn incorporate(&mut self, other: Self) {
        self.warn_unpinned = self.warn_unpinned.or(other.warn_unpinned);
        self.revisions.extend(other.revisions);
    }
}

impl TryFrom<GitUnpinnedPackage> for GitPinnedPackage {
    type Error = Box<FsError>;

    fn try_from(mut git_unpinned_package: GitUnpinnedPackage) -> Result<Self, Self::Error> {
        let revision = git_unpinned_package
            .revisions
            .pop()
            .unwrap_or_else(|| "HEAD".to_string());
        Ok(Self {
            git: git_unpinned_package.git,
            // Unwrap or error
            name: git_unpinned_package.name.ok_or_else(|| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Git package name is required for pinned packages"
                )
            })?,
            revision,
            warn_unpinned: git_unpinned_package.warn_unpinned,
            subdirectory: git_unpinned_package.subdirectory,
            unrendered: git_unpinned_package.unrendered,
        })
    }
}

impl TryFrom<GitPackage> for GitUnpinnedPackage {
    type Error = Box<FsError>;

    fn try_from(git_package: GitPackage) -> Result<Self, Self::Error> {
        Ok(Self {
            git: (*git_package.git).clone(),
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
        })
    }
}

#[derive(Debug, Clone)]
pub struct LocalPinnedPackage {
    pub local: PathBuf,
    pub name: String,
}

impl TryFrom<LocalUnpinnedPackage> for LocalPinnedPackage {
    type Error = Box<FsError>;

    fn try_from(local_unpinned_package: LocalUnpinnedPackage) -> Result<Self, Self::Error> {
        Ok(Self {
            local: local_unpinned_package.local,
            name: local_unpinned_package.name.ok_or_else(|| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Local package name is required for pinned packages"
                )
            })?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct LocalUnpinnedPackage {
    pub local: PathBuf,
    pub name: Option<String>,
}

// Private packages
#[derive(Debug, Clone)]
pub struct PrivatePinnedPackage {
    pub private: String,
    pub name: String,
    pub provider: Option<PrivatePackageProvider>,
    pub revision: String,
    pub warn_unpinned: Option<bool>,
    pub subdirectory: Option<String>,
    pub unrendered: HashMap<String, YmlValue>,
}

#[derive(Debug, Clone)]
pub struct PrivateUnpinnedPackage {
    pub private: String,
    pub name: Option<String>,
    pub provider: Option<PrivatePackageProvider>,
    pub warn_unpinned: Option<bool>,
    pub revisions: Vec<String>,
    pub subdirectory: Option<String>,
    pub unrendered: HashMap<String, YmlValue>,
    pub original_entry: PrivatePackage,
}

impl PrivateUnpinnedPackage {
    pub fn incorporate(&mut self, other: Self) {
        self.warn_unpinned = self.warn_unpinned.or(other.warn_unpinned);
        self.revisions.extend(other.revisions);
    }
}

impl TryFrom<PrivateUnpinnedPackage> for PrivatePinnedPackage {
    type Error = Box<FsError>;

    fn try_from(mut private_unpinned_package: PrivateUnpinnedPackage) -> Result<Self, Self::Error> {
        let revision = private_unpinned_package
            .revisions
            .pop()
            .unwrap_or_else(|| "HEAD".to_string());
        Ok(Self {
            private: private_unpinned_package.private,
            // Unwrap or error
            name: private_unpinned_package.name.ok_or_else(|| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Private package name is required for pinned packages"
                )
            })?,
            provider: private_unpinned_package.provider,
            revision,
            warn_unpinned: private_unpinned_package.warn_unpinned,
            subdirectory: private_unpinned_package.subdirectory,
            unrendered: private_unpinned_package.unrendered,
        })
    }
}

impl TryFrom<PrivatePackage> for PrivateUnpinnedPackage {
    type Error = Box<FsError>;

    fn try_from(private_package: PrivatePackage) -> Result<Self, Self::Error> {
        Ok(Self {
            private: (*private_package.private).clone(),
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
        })
    }
}

// Tarball packages
#[derive(Debug, Clone)]
pub struct TarballPinnedPackage {
    pub tarball: String,
    pub name: String,
    pub unrendered: HashMap<String, YmlValue>,
}

impl TarballPinnedPackage {
    pub fn source_type(&self) -> String {
        "tarball".to_string()
    }

    pub fn get_version(&self) -> String {
        "tarball".to_string()
    }

    pub fn nice_version_name(&self) -> String {
        format!("tarball (url: {})", self.tarball)
    }
}

impl TryFrom<TarballUnpinnedPackage> for TarballPinnedPackage {
    type Error = Box<FsError>;

    fn try_from(tarball_unpinned_package: TarballUnpinnedPackage) -> Result<Self, Self::Error> {
        Ok(Self {
            tarball: tarball_unpinned_package.tarball,
            name: tarball_unpinned_package.name.ok_or_else(|| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Tarball package name is required for pinned packages"
                )
            })?,
            unrendered: tarball_unpinned_package.unrendered,
        })
    }
}

#[derive(Debug, Clone)]
pub struct TarballUnpinnedPackage {
    pub tarball: String,
    pub name: Option<String>,
    pub unrendered: HashMap<String, YmlValue>,
    pub original_entry: TarballPackage,
}

impl TarballUnpinnedPackage {
    #[allow(unused_variables)]
    pub fn incorporate(&mut self, other: Self) {
        // For tarball packages, we don't need to merge anything since they're always pinned
        // Just keep the current values
    }
}

impl TryFrom<TarballPackage> for TarballUnpinnedPackage {
    type Error = Box<FsError>;

    fn try_from(tarball_package: TarballPackage) -> Result<Self, Self::Error> {
        Ok(Self {
            tarball: (*tarball_package.tarball).clone(),
            name: None,
            unrendered: tarball_package.__unrendered__.clone(),
            original_entry: tarball_package,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_string_single() {
        let versions = parse_version_string(">=0.8.0").unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].matcher, Matchers::GreaterThanOrEqualTo);
        assert_eq!(versions[0].major, Some(0));
        assert_eq!(versions[0].minor, Some(8));
        assert_eq!(versions[0].patch, Some(0));
    }

    #[test]
    fn test_parse_version_string_jinja_list_single_quotes() {
        // MiniJinja renders lists with single quotes: ['>=0.8.0', '<0.9.0']
        let versions = parse_version_string("['>=0.8.0', '<0.9.0']").unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].matcher, Matchers::GreaterThanOrEqualTo);
        assert_eq!(versions[0].major, Some(0));
        assert_eq!(versions[0].minor, Some(8));
        assert_eq!(versions[1].matcher, Matchers::LessThan);
        assert_eq!(versions[1].major, Some(0));
        assert_eq!(versions[1].minor, Some(9));
    }

    #[test]
    fn test_parse_version_string_jinja_list_double_quotes() {
        // Python Jinja2 may also produce double quotes
        let versions = parse_version_string(r#"[">=0.8.0", "<0.9.0"]"#).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].matcher, Matchers::GreaterThanOrEqualTo);
        assert_eq!(versions[1].matcher, Matchers::LessThan);
    }

    #[test]
    fn test_hub_package_with_stringified_list_version() {
        let hub_package = HubPackage {
            package: "dbt-labs/dbt_utils".to_string(),
            version: Some(PackageVersion::String("['>=0.8.0', '<0.9.0']".to_string())),
            install_prerelease: None,
        };
        let unpinned: HubUnpinnedPackage = hub_package.try_into().unwrap();
        assert_eq!(unpinned.versions.len(), 2);
    }
}
