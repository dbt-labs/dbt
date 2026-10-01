use crate::notices::{PackageNotice, PackageNoticeKind, VersionCompatKind};
use crate::semver::{Version, VersionSpecifier, versions_compatible};
use dbt_common::{ErrorCode, FsResult, err, fs_err};
use dbt_schemas::schemas::packages::DbtPackageEntry;
use dbt_schemas::schemas::serde::StringOrArrayOfStrings;
use reqwest_middleware::ClientWithMiddleware;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::OnceCell;

use crate::network_client::retrying_http_client;

pub const DBT_HUB_URL: &str = "https://hub.getdbt.com";
pub const DBT_CORE_FIXED_VERSION: &str = "1.8.7";

// tarball containing source code for version
#[derive(Deserialize, Clone, Debug, Default)]
pub struct HubPackageDownloads {
    pub tarball: String,
    pub sha1: Option<String>,
    // Hub-served mirror of `tarball`; unlike upstream, its `sha1` is trustworthy
    pub hub: Option<Box<HubPackageDownloads>>,
}

// tarball for fusion compatible version if it exists
#[derive(Deserialize, Clone, Debug)]
pub struct FusionHubPackageDownloads {
    pub tarball: Option<String>,
    pub sha1: Option<String>,
}

// Fusion compatibility metadata sourced from Package Hub
#[derive(Deserialize, Clone, Debug, Default)]
pub struct HubPackageFusionCompatibility {
    // true if required dbt version is defined
    pub require_dbt_version_defined: Option<bool>,
    // true or false is require_dbt_version_defined=true, else none
    pub require_dbt_version_compatible: Option<bool>,
    // true if dbt parse succeeded on the version, else false
    // Package Hub contains detailed errors from parse
    pub parse_compatible: Option<bool>,
    // true if we have tested the version and know it's compatible, else false
    pub manually_verified_compatible: Option<bool>,
    // true if we know the version is incompatible, else true
    pub manually_verified_incompatible: Option<bool>,
    // link to tarball for fusion compatible download if it exists
    pub fusion_compatible_download: Option<FusionHubPackageDownloads>,
}

// Final compatibility status based on metadata from Package Hub
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageVersionCompatibilityStatus {
    // Version is confirmed as Fusion compatible
    // Currently only applicable if manually verified as compatible
    Compatible,
    // Version has a required dbt version that excludes 2.0
    RequiredDbtVersionIncompatible,
    // Version has been verified as incompatible with Fusion
    ManuallyVerifiedIncompatible,
    // Fallback - does not indicate that version is incompatible,
    // just that we don't have adequate info to confirm it's compatible
    Unknown,
}

impl std::fmt::Display for PackageVersionCompatibilityStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageVersionCompatibilityStatus::Compatible => write!(f, "Compatible"),
            PackageVersionCompatibilityStatus::RequiredDbtVersionIncompatible => {
                write!(f, "Required dbt version does not match current version")
            }
            PackageVersionCompatibilityStatus::ManuallyVerifiedIncompatible => write!(
                f,
                "Manually verified by dbt as incompatible with this dbt version. See hub.getdbt.com for details"
            ),
            PackageVersionCompatibilityStatus::Unknown => {
                write!(f, "Could not determine compatibility")
            }
        }
    }
}

// Translate the Fusion compatiblity metadata into a single state
fn get_fusion_compatibility_status(
    package_compatibility: &HubPackageFusionCompatibility,
) -> PackageVersionCompatibilityStatus {
    // Version has been manually verified as compatible, overrides all others
    if let Some(true) = package_compatibility.manually_verified_compatible {
        PackageVersionCompatibilityStatus::Compatible
    // Version has been manually verified as incompatible, overrides all others
    } else if let Some(true) = package_compatibility.manually_verified_incompatible {
        PackageVersionCompatibilityStatus::ManuallyVerifiedIncompatible
    // Legacy logic: if the version defines a required dbt version,
    // use that to determine compatibility
    } else if let Some(true) = package_compatibility.require_dbt_version_defined {
        // If the required dbt version excludes 2.0, consider it incompatible
        if let Some(false) = package_compatibility.require_dbt_version_compatible {
            PackageVersionCompatibilityStatus::RequiredDbtVersionIncompatible
        } else {
            // If required dbt version includes 2.0, consider it compatible
            PackageVersionCompatibilityStatus::Compatible
        }
    // Insufficient information to determine compatibility
    } else {
        PackageVersionCompatibilityStatus::Unknown
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct HubPackageVersion {
    pub name: String,
    pub packages: Vec<DbtPackageEntry>,
    pub downloads: HubPackageDownloads,
    #[serde(default)]
    pub require_dbt_version: Option<StringOrArrayOfStrings>,
    pub fusion_compatibility: Option<HubPackageFusionCompatibility>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct HubPackageJson {
    pub name: String,
    pub versions: HashMap<String, HubPackageVersion>,
    #[serde(default)]
    pub deprecated: bool,
    #[serde(default)]
    pub redirectnamespace: Option<String>,
    #[serde(default)]
    pub redirectname: Option<String>,
}

struct HubClientInner {
    client: ClientWithMiddleware,
    base_url: String,
    index: OnceCell<HashSet<String>>,
    cache: scc::HashMap<String, HubPackageJson>,
}

/// Client for interacting with the dbt Hub API.
///
/// Clone-safe and thread-safe via Arc interior. All methods take `&self`
/// allowing concurrent fetches without a mutable borrow.
#[derive(Clone)]
pub struct HubClient {
    inner: Arc<HubClientInner>,
}

impl HubClient {
    pub fn new(base_url: &str) -> Self {
        Self::with_client(base_url, retrying_http_client())
    }

    pub fn with_client(base_url: &str, client: ClientWithMiddleware) -> Self {
        Self {
            inner: Arc::new(HubClientInner {
                client,
                base_url: base_url.to_string(),
                index: OnceCell::new(),
                cache: scc::HashMap::new(),
            }),
        }
    }

    /// Hydrate the package index. Safe to call concurrently — OnceCell ensures
    /// only one fetch runs; all other callers wait for the same result.
    pub async fn hydrate_index(&self) -> FsResult<&HashSet<String>> {
        self.inner
            .index
            .get_or_try_init(|| async {
                let url = format!("{}/api/v1/index.json", self.inner.base_url);
                let res = self.inner.client.get(&url).send().await.map_err(|e| {
                    fs_err!(
                        ErrorCode::RuntimeError,
                        "Failed to get index from {url}: {e:#}"
                    )
                })?;
                if res.status().is_success() {
                    let index: Vec<String> = res.json().await.map_err(|e| {
                        fs_err!(
                            ErrorCode::RuntimeError,
                            "Failed to parse index from {url}; {}",
                            e.source()
                                .map_or_else(|| "unknown".to_string(), |source| source.to_string())
                        )
                    })?;
                    Ok(index.into_iter().collect())
                } else {
                    err!(
                        ErrorCode::RuntimeError,
                        "Failed to get index from {url}; status: {}",
                        res.status()
                    )
                }
            })
            .await
    }

    pub async fn get_hub_package(&self, package: &str) -> FsResult<HubPackageJson> {
        if let Some(entry) = self.inner.cache.get_async(package).await {
            return Ok(entry.get().clone());
        }
        let url = format!("{}/api/v1/{}.json", self.inner.base_url, package);
        let res = self.inner.client.get(&url).send().await.map_err(|e| {
            fs_err!(
                ErrorCode::RuntimeError,
                "Failed to get package from {url}: {e:#}"
            )
        })?;
        if res.status().is_success() {
            let hub_package: HubPackageJson = res.json().await.map_err(|e| {
                fs_err!(
                    ErrorCode::RuntimeError,
                    "Failed to get package from {url}; {}",
                    e.source()
                        .map_or_else(|| "unknown".to_string(), |source| source.to_string())
                )
            })?;
            // insert_async returns a bool (inserted/not), discard it.
            let _ = self
                .inner
                .cache
                .insert_async(package.to_string(), hub_package.clone())
                .await;
            Ok(hub_package)
        } else {
            err!(
                ErrorCode::RuntimeError,
                "Failed to get package from {url}; status: {}",
                res.status()
            )
        }
    }

    pub async fn check_index(&self, package: &str) -> FsResult<bool> {
        let index = self.hydrate_index().await?;
        Ok(index.contains(package))
    }

    pub async fn get_compatible_versions(
        &self,
        hub_package: &HubPackageJson,
        _dbt_version: &str,
        _should_version_check: bool,
    ) -> FsResult<Vec<String>> {
        // TODO: Implement version filtering. This should be done
        // once most of the regularly used hub packages have a
        // fusion compatible version in require_dbt_version.
        Ok(hub_package.versions.keys().cloned().collect())
    }
}

impl HubPackageJson {
    pub(crate) fn deprecation_notices(&self) -> Vec<PackageNotice> {
        let mut out = Vec::with_capacity(2);
        if self.redirectnamespace.is_some() || self.redirectname.is_some() {
            out.push(PackageNotice {
                key: self.name.clone(),
                kind: PackageNoticeKind::HubRedirect {
                    redirect_namespace: self.redirectnamespace.clone(),
                    redirect_name: self.redirectname.clone(),
                },
            });
        }
        if self.deprecated {
            out.push(PackageNotice {
                key: self.name.clone(),
                kind: PackageNoticeKind::HubDeprecated,
            });
        }
        out
    }
}

impl HubPackageVersion {
    /// Returns a notice if the current dbt version (`CARGO_PKG_VERSION`)
    /// doesn't satisfy this version's `require_dbt_version`, or if
    /// fusion-compat metadata flags it incompatible.
    pub(crate) fn version_compat_notice(&self, package_name: &str) -> Option<PackageNotice> {
        if let Some(compatibility_status) = self.fusion_compatibility.as_ref().map(
            |package_compatibility: &HubPackageFusionCompatibility| {
                get_fusion_compatibility_status(package_compatibility)
            },
        ) {
            if matches!(
                compatibility_status,
                PackageVersionCompatibilityStatus::RequiredDbtVersionIncompatible
            ) || matches!(
                compatibility_status,
                PackageVersionCompatibilityStatus::ManuallyVerifiedIncompatible
            ) {
                return Some(PackageNotice {
                    key: package_name.to_string(),
                    kind: PackageNoticeKind::HubVersionCompat(VersionCompatKind::Fusion(
                        compatibility_status.to_string(),
                    )),
                });
            }
            return None;
        }

        let current_version = env!("CARGO_PKG_VERSION");
        let required_versions = self.require_dbt_version.as_ref()?;

        let version_strings: Vec<String> = match required_versions {
            StringOrArrayOfStrings::String(s) => vec![s.clone()],
            StringOrArrayOfStrings::ArrayOfStrings(arr) => arr.clone(),
        };

        let mut all_versions = Vec::new();
        for version_str in &version_strings {
            match VersionSpecifier::from_str(version_str) {
                Ok(spec) => all_versions.push(Version::Spec(spec)),
                Err(_) => return None,
            }
        }
        match VersionSpecifier::from_str(&format!("={}", current_version)) {
            Ok(current_spec) => all_versions.push(Version::Spec(current_spec)),
            Err(_) => return None,
        }

        if versions_compatible(&all_versions) {
            return None;
        }

        let required = if version_strings.len() == 1 {
            version_strings[0].clone()
        } else {
            format!("[{}]", version_strings.join(", "))
        };
        Some(PackageNotice {
            key: package_name.to_string(),
            kind: PackageNoticeKind::HubVersionCompat(VersionCompatKind::RequireDbtVersion {
                required,
                current: current_version.to_string(),
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // Helper function to create a test HubPackageJson with deprecated flag
    fn create_deprecated_package() -> HubPackageJson {
        let mut versions = HashMap::new();
        versions.insert(
            "0.7.0".to_string(),
            HubPackageVersion {
                name: "dbt_utils".to_string(),
                packages: vec![],
                downloads: HubPackageDownloads {
                    tarball: "https://example.com/tarball.tar.gz".to_string(),
                    ..Default::default()
                },
                require_dbt_version: None,
                fusion_compatibility: None,
            },
        );

        HubPackageJson {
            name: "fishtown-analytics/dbt_utils".to_string(),
            versions,
            deprecated: true,
            redirectnamespace: None,
            redirectname: None,
        }
    }

    // Helper function to create a test HubPackageJson with redirect to new namespace and name
    fn create_redirected_package_full() -> HubPackageJson {
        let mut versions = HashMap::new();
        versions.insert(
            "0.7.0".to_string(),
            HubPackageVersion {
                name: "dbt_utils".to_string(),
                packages: vec![],
                downloads: HubPackageDownloads {
                    tarball: "https://example.com/tarball.tar.gz".to_string(),
                    ..Default::default()
                },
                require_dbt_version: None,
                fusion_compatibility: None,
            },
        );

        HubPackageJson {
            name: "fishtown-analytics/dbt_utils".to_string(),
            versions,
            deprecated: false,
            redirectnamespace: Some("dbt-labs".to_string()),
            redirectname: Some("dbt_utils".to_string()),
        }
    }

    // Helper function to create a test HubPackageJson with namespace redirect only
    fn create_redirected_package_namespace_only() -> HubPackageJson {
        let mut versions = HashMap::new();
        versions.insert(
            "1.0.0".to_string(),
            HubPackageVersion {
                name: "some_package".to_string(),
                packages: vec![],
                downloads: HubPackageDownloads {
                    tarball: "https://example.com/tarball.tar.gz".to_string(),
                    ..Default::default()
                },
                require_dbt_version: None,
                fusion_compatibility: None,
            },
        );

        HubPackageJson {
            name: "old-org/some_package".to_string(),
            versions,
            deprecated: false,
            redirectnamespace: Some("new-org".to_string()),
            redirectname: None,
        }
    }

    // Helper function to create a test HubPackageJson with name redirect only
    fn create_redirected_package_name_only() -> HubPackageJson {
        let mut versions = HashMap::new();
        versions.insert(
            "1.0.0".to_string(),
            HubPackageVersion {
                name: "old_name".to_string(),
                packages: vec![],
                downloads: HubPackageDownloads {
                    tarball: "https://example.com/tarball.tar.gz".to_string(),
                    ..Default::default()
                },
                require_dbt_version: None,
                fusion_compatibility: None,
            },
        );

        HubPackageJson {
            name: "org/old_name".to_string(),
            versions,
            deprecated: false,
            redirectnamespace: None,
            redirectname: Some("new_name".to_string()),
        }
    }

    #[test]
    fn test_deserialize_deprecated_package() {
        let json = r#"
        {
            "name": "fishtown-analytics/dbt_utils",
            "versions": {
                "0.7.0": {
                    "name": "dbt_utils",
                    "packages": [],
                    "downloads": {
                        "tarball": "https://example.com/tarball.tar.gz"
                    }
                }
            },
            "deprecated": true
        }
        "#;

        let package: HubPackageJson = serde_json::from_str(json).unwrap();
        assert_eq!(package.name, "fishtown-analytics/dbt_utils");
        assert!(package.deprecated);
        assert!(package.redirectnamespace.is_none());
        assert!(package.redirectname.is_none());
    }

    #[test]
    fn test_deserialize_redirected_package_full() {
        let json = r#"
        {
            "name": "fishtown-analytics/dbt_utils",
            "versions": {
                "0.7.0": {
                    "name": "dbt_utils",
                    "packages": [],
                    "downloads": {
                        "tarball": "https://example.com/tarball.tar.gz"
                    }
                }
            },
            "redirectnamespace": "dbt-labs",
            "redirectname": "dbt_utils"
        }
        "#;

        let package: HubPackageJson = serde_json::from_str(json).unwrap();
        assert_eq!(package.name, "fishtown-analytics/dbt_utils");
        assert!(!package.deprecated); // Should default to false
        assert_eq!(package.redirectnamespace.as_ref().unwrap(), "dbt-labs");
        assert_eq!(package.redirectname.as_ref().unwrap(), "dbt_utils");
    }

    #[test]
    fn test_deserialize_package_no_redirect_fields() {
        let json = r#"
        {
            "name": "some-org/some_package",
            "versions": {
                "1.0.0": {
                    "name": "some_package",
                    "packages": [],
                    "downloads": {
                        "tarball": "https://example.com/tarball.tar.gz"
                    }
                }
            }
        }
        "#;

        let package: HubPackageJson = serde_json::from_str(json).unwrap();
        assert_eq!(package.name, "some-org/some_package");
        assert!(!package.deprecated); // Should default to false
        assert!(package.redirectnamespace.is_none());
        assert!(package.redirectname.is_none());
    }

    #[test]
    fn test_deprecation_notices_deprecated_package() {
        let package = create_deprecated_package();
        let notices = package.deprecation_notices();
        assert_eq!(notices.len(), 1);
        assert!(matches!(
            &notices[0],
            PackageNotice {
                key,
                kind: PackageNoticeKind::HubDeprecated,
            }
            if key == "fishtown-analytics/dbt_utils"
        ));
    }

    #[test]
    fn test_deprecation_notices_full_redirect() {
        let package = create_redirected_package_full();
        let notices = package.deprecation_notices();
        assert_eq!(notices.len(), 1);
        assert!(matches!(
            &notices[0],
            PackageNotice {
                kind: PackageNoticeKind::HubRedirect {
                    redirect_namespace: Some(ns),
                    redirect_name: Some(name),
                },
                ..
            } if ns == "dbt-labs" && name == "dbt_utils"
        ));
    }

    #[test]
    fn test_deprecation_notices_namespace_redirect() {
        let package = create_redirected_package_namespace_only();
        let notices = package.deprecation_notices();
        assert_eq!(notices.len(), 1);
        assert!(matches!(
            &notices[0],
            PackageNotice {
                kind: PackageNoticeKind::HubRedirect {
                    redirect_namespace: Some(ns),
                    redirect_name: None,
                },
                ..
            } if ns == "new-org"
        ));
    }

    #[test]
    fn test_deprecation_notices_name_redirect() {
        let package = create_redirected_package_name_only();
        let notices = package.deprecation_notices();
        assert_eq!(notices.len(), 1);
        assert!(matches!(
            &notices[0],
            PackageNotice {
                kind: PackageNoticeKind::HubRedirect {
                    redirect_namespace: None,
                    redirect_name: Some(name),
                },
                ..
            } if name == "new_name"
        ));
    }

    #[test]
    fn test_fishtown_analytics_dbt_utils_case() {
        let package = create_deprecated_package();
        assert_eq!(package.name, "fishtown-analytics/dbt_utils");
        assert!(package.deprecated);
        assert!(package.versions.contains_key("0.7.0"));
        let notices = package.deprecation_notices();
        assert!(
            notices
                .iter()
                .any(|n| matches!(n.kind, PackageNoticeKind::HubDeprecated))
        );
    }

    #[test]
    fn test_deserialize_package_with_require_dbt_version_string() {
        let json = r#"
        {
            "name": "some-org/versioned_package",
            "versions": {
                "1.0.0": {
                    "name": "versioned_package",
                    "packages": [],
                    "downloads": {
                        "tarball": "https://example.com/tarball.tar.gz"
                    },
                    "require_dbt_version": ">=1.5.0"
                }
            }
        }
        "#;

        let package: HubPackageJson = serde_json::from_str(json).unwrap();
        assert_eq!(package.name, "some-org/versioned_package");

        let version = package.versions.get("1.0.0").unwrap();
        assert!(version.require_dbt_version.is_some());

        // Verify it's a string variant
        if let Some(StringOrArrayOfStrings::String(version_req)) = &version.require_dbt_version {
            assert_eq!(version_req, ">=1.5.0");
        } else {
            panic!("Expected StringOrArrayOfStrings::String variant");
        }
    }

    #[test]
    fn test_deserialize_package_with_require_dbt_version_array() {
        let json = r#"
        {
            "name": "some-org/versioned_package",
            "versions": {
                "1.0.0": {
                    "name": "versioned_package",
                    "packages": [],
                    "downloads": {
                        "tarball": "https://example.com/tarball.tar.gz"
                    },
                    "require_dbt_version": [">=1.5.0", "<2.0.0"]
                }
            }
        }
        "#;

        let package: HubPackageJson = serde_json::from_str(json).unwrap();
        assert_eq!(package.name, "some-org/versioned_package");

        let version = package.versions.get("1.0.0").unwrap();
        assert!(version.require_dbt_version.is_some());

        // Verify it's an array variant
        if let Some(StringOrArrayOfStrings::ArrayOfStrings(versions)) = &version.require_dbt_version
        {
            assert_eq!(versions.len(), 2);
            assert_eq!(versions[0], ">=1.5.0");
            assert_eq!(versions[1], "<2.0.0");
        } else {
            panic!("Expected StringOrArrayOfStrings::ArrayOfStrings variant");
        }
    }

    #[test]
    fn test_deserialize_package_without_require_dbt_version() {
        let json = r#"
        {
            "name": "some-org/unversioned_package",
            "versions": {
                "1.0.0": {
                    "name": "unversioned_package",
                    "packages": [],
                    "downloads": {
                        "tarball": "https://example.com/tarball.tar.gz"
                    }
                }
            }
        }
        "#;

        let package: HubPackageJson = serde_json::from_str(json).unwrap();
        assert_eq!(package.name, "some-org/unversioned_package");

        let version = package.versions.get("1.0.0").unwrap();
        assert!(version.require_dbt_version.is_none());
    }

    // Tests for compatibility using legacy require dbt version

    #[test]
    fn test_version_compat_notice_compatible() {
        let version = HubPackageVersion {
            name: "test_package".to_string(),
            packages: vec![],
            downloads: HubPackageDownloads {
                tarball: "https://example.com/tarball.tar.gz".to_string(),
                ..Default::default()
            },
            require_dbt_version: Some(StringOrArrayOfStrings::String(">=1.5.0".to_string())),
            fusion_compatibility: None,
        };
        assert!(
            version
                .version_compat_notice("test-org/test_package")
                .is_none()
        );
    }

    #[test]
    fn test_version_compat_notice_incompatible() {
        let version = HubPackageVersion {
            name: "test_package".to_string(),
            packages: vec![],
            downloads: HubPackageDownloads {
                tarball: "https://example.com/tarball.tar.gz".to_string(),
                ..Default::default()
            },
            require_dbt_version: Some(StringOrArrayOfStrings::String(">=100.0.0".to_string())),
            fusion_compatibility: None,
        };
        let notice = version
            .version_compat_notice("test-org/test_package")
            .expect("expected version-compat notice");
        assert_eq!(notice.key.as_str(), "test-org/test_package");
        assert!(matches!(
            notice.kind,
            PackageNoticeKind::HubVersionCompat(VersionCompatKind::RequireDbtVersion {
                required,
                ..
            })
            if required == ">=100.0.0"
        ));
    }

    #[test]
    fn test_version_compat_notice_range_compatible() {
        let version = HubPackageVersion {
            name: "test_package".to_string(),
            packages: vec![],
            downloads: HubPackageDownloads {
                tarball: "https://example.com/tarball.tar.gz".to_string(),
                ..Default::default()
            },
            require_dbt_version: Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                ">=1.0.0".to_string(),
                "<100.0.0".to_string(),
            ])),
            fusion_compatibility: None,
        };
        assert!(
            version
                .version_compat_notice("test-org/test_package")
                .is_none()
        );
    }

    #[test]
    fn test_version_compat_notice_range_incompatible() {
        let version = HubPackageVersion {
            name: "test_package".to_string(),
            packages: vec![],
            downloads: HubPackageDownloads {
                tarball: "https://example.com/tarball.tar.gz".to_string(),
                ..Default::default()
            },
            require_dbt_version: Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                ">=100.0.0".to_string(),
                "<200.0.0".to_string(),
            ])),
            fusion_compatibility: None,
        };
        let notice = version
            .version_compat_notice("test-org/test_package")
            .expect("expected version-compat notice");
        assert!(matches!(
            notice,
            PackageNotice {
                kind: PackageNoticeKind::HubVersionCompat(VersionCompatKind::RequireDbtVersion { required, .. }),
                ..
            } if required == "[>=100.0.0, <200.0.0]"
        ));
    }

    #[test]
    fn test_version_compat_notice_no_requirement() {
        let version = HubPackageVersion {
            name: "test_package".to_string(),
            packages: vec![],
            downloads: HubPackageDownloads {
                tarball: "https://example.com/tarball.tar.gz".to_string(),
                ..Default::default()
            },
            require_dbt_version: None,
            fusion_compatibility: None,
        };
        assert!(
            version
                .version_compat_notice("test-org/test_package")
                .is_none()
        );
    }

    // Tests for compatibility using Fusion compatibility metadata

    #[test]
    fn test_version_compat_notice_compatible_with_fusion_compatibility() {
        let version = HubPackageVersion {
            name: "test_package".to_string(),
            packages: vec![],
            downloads: HubPackageDownloads {
                tarball: "https://example.com/tarball.tar.gz".to_string(),
                ..Default::default()
            },
            require_dbt_version: Some(StringOrArrayOfStrings::String(">=1.5.0".to_string())),
            fusion_compatibility: Some(HubPackageFusionCompatibility {
                require_dbt_version_defined: Some(true),
                require_dbt_version_compatible: Some(true),
                parse_compatible: None,
                manually_verified_compatible: None,
                manually_verified_incompatible: None,
                fusion_compatible_download: None,
            }),
        };
        assert!(
            version
                .version_compat_notice("test-org/test_package")
                .is_none()
        );
    }

    #[test]
    fn test_version_compat_notice_incompatible_with_fusion_compatibility() {
        let version = HubPackageVersion {
            name: "test_package".to_string(),
            packages: vec![],
            downloads: HubPackageDownloads {
                tarball: "https://example.com/tarball.tar.gz".to_string(),
                ..Default::default()
            },
            require_dbt_version: Some(StringOrArrayOfStrings::String(">=100.0.0".to_string())),
            fusion_compatibility: Some(HubPackageFusionCompatibility {
                require_dbt_version_defined: Some(true),
                require_dbt_version_compatible: Some(false),
                parse_compatible: None,
                manually_verified_compatible: None,
                manually_verified_incompatible: None,
                fusion_compatible_download: None,
            }),
        };
        let notice = version
            .version_compat_notice("test-org/test_package")
            .expect("expected fusion-compat notice");
        assert!(matches!(
            notice,
            PackageNotice {
                key,
                kind: PackageNoticeKind::HubVersionCompat(VersionCompatKind::Fusion(_)),
            } if key == "test-org/test_package"
        ));
    }

    #[test]
    fn test_version_compat_notice_verified_incompatible_with_fusion_compatibility() {
        let version = HubPackageVersion {
            name: "test_package".to_string(),
            packages: vec![],
            downloads: HubPackageDownloads {
                tarball: "https://example.com/tarball.tar.gz".to_string(),
                ..Default::default()
            },
            require_dbt_version: Some(StringOrArrayOfStrings::String(">=100.0.0".to_string())),
            fusion_compatibility: Some(HubPackageFusionCompatibility {
                require_dbt_version_defined: Some(true),
                require_dbt_version_compatible: Some(true),
                parse_compatible: None,
                manually_verified_compatible: None,
                manually_verified_incompatible: Some(true),
                fusion_compatible_download: None,
            }),
        };
        let notice = version
            .version_compat_notice("test-org/test_package")
            .expect("expected fusion-compat notice");
        assert!(matches!(
            notice,
            PackageNotice {
                key,
                kind: PackageNoticeKind::HubVersionCompat(VersionCompatKind::Fusion(_)),
            } if key == "test-org/test_package"
        ));
    }

    #[test]
    fn test_version_compat_notice_no_requirement_with_fusion_compatibility() {
        let version = HubPackageVersion {
            name: "test_package".to_string(),
            packages: vec![],
            downloads: HubPackageDownloads {
                tarball: "https://example.com/tarball.tar.gz".to_string(),
                ..Default::default()
            },
            require_dbt_version: None,
            fusion_compatibility: Some(HubPackageFusionCompatibility {
                require_dbt_version_defined: Some(false),
                require_dbt_version_compatible: Some(false),
                parse_compatible: None,
                manually_verified_compatible: None,
                manually_verified_incompatible: None,
                fusion_compatible_download: None,
            }),
        };
        assert!(
            version
                .version_compat_notice("test-org/test_package")
                .is_none()
        );
    }

    #[test]
    fn test_deserialize_version_with_fusion_compatibility() {
        let json = r#"
        {
            "id": "get-select/dbt_snowflake_query_tags/2.6.0",
            "name": "dbt_snowflake_query_tags",
            "version": "2.6.0",
            "published_at": "1970-01-01T00:00:00.000000+00:00",
            "packages": [],
            "require_dbt_version": "<3.0.0",
            "works_with": [],
            "_source": {
                "type": "github",
                "url": "https://github.com/get-select/dbt-snowflake-query-tags/tree/2.6.0/",
                "readme": "https://raw.githubusercontent.com/get-select/dbt-snowflake-query-tags/2.6.0/README.md"
            },
            "downloads": {
                "tarball": "https://codeload.github.com/get-select/dbt-snowflake-query-tags/tar.gz/2.6.0",
                "format": "tgz",
                "sha1": "a37691d43a990655b703f7d847badce2a7ab87d1"
            },
            "fusion_compatibility": {
                "version": "2.6.0",
                "require_dbt_version_defined": true,
                "require_dbt_version_compatible": true,
                "parse_compatible": true,
                "parse_compatibility_result": {
                    "parse_exit_code": 0,
                    "total_errors": 0,
                    "total_warnings": 0,
                    "errors": [],
                    "warnings": [],
                    "fusion_version": "2.0.0-preview.101"
                },
                "manually_verified_compatible": true,
                "manually_verified_incompatible": false,
                "download_failed": false,
                "fusion_compatible_download": {}
            }
        }
        "#;

        let package_version: HubPackageVersion = serde_json::from_str(json).unwrap();
        assert_eq!(package_version.name, "dbt_snowflake_query_tags");
        assert_eq!(
            package_version.downloads.sha1,
            Some("a37691d43a990655b703f7d847badce2a7ab87d1".to_string())
        );
        assert!(package_version.downloads.hub.is_none());
        let fusion_compatibility: HubPackageFusionCompatibility =
            package_version.fusion_compatibility.unwrap();
        assert_eq!(
            fusion_compatibility.require_dbt_version_compatible,
            Some(true)
        );
        assert_eq!(fusion_compatibility.require_dbt_version_defined, Some(true));
    }

    #[test]
    fn test_deserialize_version_with_hub_mirror_download() {
        let json = r#"
        {
            "id": "fishtown-analytics/dbt-external-tables/0.4.0",
            "name": "dbt_external_tables",
            "version": "0.4.0",
            "published_at": "1970-01-01T00:00:00.000000+00:00",
            "packages": [],
            "works_with": [],
            "downloads": {
                "tarball": "https://codeload.github.com/fishtown-analytics/dbt-external-tables/tar.gz/0.4.0",
                "format": "tgz",
                "sha1": "34ea245587e15421b5482e94810f30315299bbbc",
                "hub": {
                    "tarball": "https://mirror.example-cdn.com/packages/fishtown-analytics/dbt-external-tables/tar.gz/0.4.0",
                    "format": "tgz",
                    "sha1": "34ea245587e15421b5482e94810f30315299bbbc"
                }
            }
        }
        "#;

        let package_version: HubPackageVersion = serde_json::from_str(json).unwrap();
        let hub_mirror = package_version
            .downloads
            .hub
            .expect("expected hub mirror download");
        assert_eq!(
            hub_mirror.tarball,
            "https://mirror.example-cdn.com/packages/fishtown-analytics/dbt-external-tables/tar.gz/0.4.0"
        );
        assert_eq!(
            hub_mirror.sha1,
            Some("34ea245587e15421b5482e94810f30315299bbbc".to_string())
        );
    }

    #[test]
    fn test_deserialize_version_with_fusion_compatible_download() {
        let json = r#"
        {
            "id": "get-select/dbt_snowflake_query_tags/2.6.0",
            "name": "dbt_snowflake_query_tags",
            "version": "2.6.0",
            "published_at": "1970-01-01T00:00:00.000000+00:00",
            "packages": [],
            "require_dbt_version": "<3.0.0",
            "works_with": [],
            "_source": {
                "type": "github",
                "url": "https://github.com/get-select/dbt-snowflake-query-tags/tree/2.6.0/",
                "readme": "https://raw.githubusercontent.com/get-select/dbt-snowflake-query-tags/2.6.0/README.md"
            },
            "downloads": {
                "tarball": "https://codeload.github.com/get-select/dbt-snowflake-query-tags/tar.gz/2.6.0",
                "format": "tgz",
                "sha1": "a37691d43a990655b703f7d847badce2a7ab87d1"
            },
            "fusion_compatibility": {
                "version": "2.6.0",
                "require_dbt_version_defined": true,
                "require_dbt_version_compatible": true,
                "parse_compatible": true,
                "parse_compatibility_result": {
                    "parse_exit_code": 0,
                    "total_errors": 0,
                    "total_warnings": 0,
                    "errors": [],
                    "warnings": [],
                    "fusion_version": "2.0.0-preview.101"
                },
                "manually_verified_compatible": true,
                "manually_verified_incompatible": false,
                "download_failed": false,
                "fusion_compatible_download": {
                    "tarball": "https://codeload.github.com/get-select/dbt-snowflake-query-tags/tar.gz/2.6.0",
                    "format": "tgz",
                    "sha1": "a37691d43a990655b703f7d847badce2a7ab87d1"
                }
            }
        }
        "#;

        let package_version: HubPackageVersion = serde_json::from_str(json).unwrap();
        assert_eq!(package_version.name, "dbt_snowflake_query_tags");
        let fusion_compatibility: HubPackageFusionCompatibility =
            package_version.fusion_compatibility.unwrap();
        assert_eq!(
            fusion_compatibility.require_dbt_version_compatible,
            Some(true)
        );
        assert_eq!(fusion_compatibility.require_dbt_version_defined, Some(true));
        let fusion_compatible_download: FusionHubPackageDownloads =
            fusion_compatibility.fusion_compatible_download.unwrap();
        assert_eq!(
            fusion_compatible_download.tarball,
            Some(
                "https://codeload.github.com/get-select/dbt-snowflake-query-tags/tar.gz/2.6.0"
                    .to_string()
            )
        );
        assert_eq!(
            fusion_compatible_download.sha1,
            Some("a37691d43a990655b703f7d847badce2a7ab87d1".to_string())
        );
    }

    #[test]
    fn test_deserialize_version_with_required_dbt_version_undefined() {
        let json = r#"
        {
            "id": "AltimateAI/altimate_snowflake_query_tags/v1.0.0",
            "name": "altimate_snowflake_query_tags",
            "version": "v1.0.0",
            "published_at": "1970-01-01T00:00:00.000000+00:00",
            "packages": [],
            "require_dbt_version": [],
            "works_with": [],
            "_source": {
                "type": "github",
                "url": "https://github.com/AltimateAI/altimate-dbt-snowflake-query-tags/tree/v1.0.0/",
                "readme": "https://raw.githubusercontent.com/AltimateAI/altimate-dbt-snowflake-query-tags/v1.0.0/README.md"
            },
            "downloads": {
                "tarball": "https://codeload.github.com/AltimateAI/altimate-dbt-snowflake-query-tags/tar.gz/v1.0.0",
                "format": "tgz",
                "sha1": "a81f990483608b4ac999fe774b592a8189607c20"
            },
            "fusion_compatibility": {
                "version": "1.0.0",
                "require_dbt_version_defined": false,
                "require_dbt_version_compatible": null,
                "parse_compatible": true,
                "parse_compatibility_result": {
                    "parse_exit_code": 0,
                    "total_errors": 0,
                    "total_warnings": 0,
                    "errors": [],
                    "warnings": [],
                    "fusion_version": "2.0.0-preview.153"
                },
                "manually_verified_compatible": false,
                "manually_verified_incompatible": false,
                "download_failed": false,
                "fusion_compatible_download": {}
            }
        }
        "#;

        let package_version: HubPackageVersion = serde_json::from_str(json).unwrap();
        assert!(package_version.downloads.hub.is_none());
        let fusion_compatibility: HubPackageFusionCompatibility =
            package_version.fusion_compatibility.unwrap();
        assert_eq!(
            fusion_compatibility.require_dbt_version_defined,
            Some(false)
        );
        assert_eq!(fusion_compatibility.require_dbt_version_compatible, None);
    }

    // Logic for various combinations of compatibility

    #[test]
    fn test_compatibility_with_require_dbt_version_incompatible() {
        let fusion_compatibility = HubPackageFusionCompatibility {
            require_dbt_version_defined: Some(true),
            require_dbt_version_compatible: Some(false),
            parse_compatible: Some(true),
            manually_verified_compatible: Some(false),
            manually_verified_incompatible: Some(false),
            fusion_compatible_download: None,
        };

        let compatibility_status = get_fusion_compatibility_status(&fusion_compatibility);

        assert_eq!(
            PackageVersionCompatibilityStatus::RequiredDbtVersionIncompatible,
            compatibility_status
        );
    }

    #[test]
    fn test_compatibility_status_with_require_dbt_version_compatible() {
        let fusion_compatibility = HubPackageFusionCompatibility {
            require_dbt_version_defined: Some(true),
            require_dbt_version_compatible: Some(true),
            parse_compatible: Some(true),
            manually_verified_compatible: Some(false),
            manually_verified_incompatible: Some(false),
            fusion_compatible_download: None,
        };

        let compatibility_status = get_fusion_compatibility_status(&fusion_compatibility);

        assert_eq!(
            PackageVersionCompatibilityStatus::Compatible,
            compatibility_status
        );
    }

    #[test]
    fn test_compatibility_status_with_manually_verified_incompatible_no_require_dbt_version() {
        let fusion_compatibility = HubPackageFusionCompatibility {
            require_dbt_version_defined: Some(false),
            require_dbt_version_compatible: None,
            parse_compatible: Some(true),
            manually_verified_compatible: Some(false),
            manually_verified_incompatible: Some(true),
            fusion_compatible_download: None,
        };

        let compatibility_status = get_fusion_compatibility_status(&fusion_compatibility);

        assert_eq!(
            PackageVersionCompatibilityStatus::ManuallyVerifiedIncompatible,
            compatibility_status
        );
    }

    #[test]
    fn test_compatibility_status_with_manually_verified_incompatible_require_dbt_version_compatible()
     {
        let fusion_compatibility = HubPackageFusionCompatibility {
            require_dbt_version_defined: Some(true),
            require_dbt_version_compatible: Some(true),
            parse_compatible: Some(true),
            manually_verified_compatible: Some(false),
            manually_verified_incompatible: Some(true),
            fusion_compatible_download: None,
        };

        let compatibility_status = get_fusion_compatibility_status(&fusion_compatibility);

        assert_eq!(
            PackageVersionCompatibilityStatus::ManuallyVerifiedIncompatible,
            compatibility_status
        );
    }

    #[test]
    fn test_compatibility_status_with_manually_verified_incompatible_require_dbt_version_incompatible()
     {
        let fusion_compatibility = HubPackageFusionCompatibility {
            require_dbt_version_defined: Some(true),
            require_dbt_version_compatible: Some(false),
            parse_compatible: Some(true),
            manually_verified_compatible: Some(false),
            manually_verified_incompatible: Some(true),
            fusion_compatible_download: None,
        };

        let compatibility_status = get_fusion_compatibility_status(&fusion_compatibility);

        assert_eq!(
            PackageVersionCompatibilityStatus::ManuallyVerifiedIncompatible,
            compatibility_status
        );
    }

    #[test]
    fn test_compatibility_status_with_manually_verified_compatible_require_dbt_version_incompatible()
     {
        let fusion_compatibility = HubPackageFusionCompatibility {
            require_dbt_version_defined: Some(true),
            require_dbt_version_compatible: Some(false),
            parse_compatible: Some(true),
            manually_verified_compatible: Some(true),
            manually_verified_incompatible: Some(false),
            fusion_compatible_download: None,
        };

        let compatibility_status = get_fusion_compatibility_status(&fusion_compatibility);

        assert_eq!(
            PackageVersionCompatibilityStatus::Compatible,
            compatibility_status
        );
    }

    #[test]
    fn test_compatibility_status_with_unknown_compatibility() {
        let fusion_compatibility = HubPackageFusionCompatibility {
            require_dbt_version_defined: Some(false),
            require_dbt_version_compatible: Some(true),
            parse_compatible: Some(true),
            manually_verified_compatible: Some(false),
            manually_verified_incompatible: Some(false),
            fusion_compatible_download: None,
        };

        let compatibility_status = get_fusion_compatibility_status(&fusion_compatibility);

        assert_eq!(
            PackageVersionCompatibilityStatus::Unknown,
            compatibility_status
        );
    }
}
