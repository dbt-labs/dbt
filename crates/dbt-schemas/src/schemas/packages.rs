use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    path::{Path, PathBuf},
};

use dbt_yaml::{DbtSchema, UntaggedEnumDeserialize, Verbatim};
use serde::{Deserialize, Serialize};

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

#[derive(Debug, Serialize, Deserialize, Clone, DbtSchema)]
pub struct UpstreamProject {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, Default, DbtSchema)]
pub struct DbtPackages {
    #[serde(default)]
    pub projects: Vec<UpstreamProject>,
    #[serde(default)]
    pub packages: Vec<DbtPackageEntry>,
}

#[derive(Debug, Serialize, UntaggedEnumDeserialize, Clone, DbtSchema)]
#[serde(untagged)]
pub enum DbtPackageEntry {
    Hub(HubPackage),
    Git(GitPackage),
    Local(LocalPackage),
    Private(PrivatePackage),
    Tarball(TarballPackage),
}

impl DbtPackageEntry {
    /// Key identifying this entry, matching [`DbtPackageLock::entry_name`].
    ///
    /// Lets a lock entry be traced back to the `packages.yml` line that asked
    /// for it — the lock itself is sorted by package name, so declaration order
    /// is only recoverable this way.
    pub fn entry_name(&self) -> String {
        match self {
            DbtPackageEntry::Hub(package) => package.package.clone(),
            DbtPackageEntry::Git(package) => {
                let mut key = package.git.to_string();
                if let Some(subdirectory) = &package.subdirectory {
                    key.push_str(&format!("#{subdirectory}"));
                }
                key
            }
            DbtPackageEntry::Local(package) => package.local.to_string_lossy().to_string(),
            DbtPackageEntry::Private(package) => {
                let mut key = package.private.to_string();
                if let Some(subdirectory) = &package.subdirectory {
                    key.push_str(&format!("#{subdirectory}"));
                }
                key
            }
            DbtPackageEntry::Tarball(package) => package.tarball.to_string(),
        }
    }
}

impl From<DbtPackageLock> for DbtPackageEntry {
    fn from(dbt_package_lock: DbtPackageLock) -> Self {
        match dbt_package_lock {
            DbtPackageLock::Hub(hub_package_lock) => {
                DbtPackageEntry::Hub(HubPackage::from(hub_package_lock))
            }
            DbtPackageLock::Git(git_package_lock) => {
                DbtPackageEntry::Git(GitPackage::from(git_package_lock))
            }
            DbtPackageLock::Local(local_package_lock) => {
                DbtPackageEntry::Local(LocalPackage::from(local_package_lock))
            }
            DbtPackageLock::Private(private_package_lock) => {
                DbtPackageEntry::Private(PrivatePackage::from(private_package_lock))
            }
            DbtPackageLock::Tarball(tarball_package_lock) => {
                DbtPackageEntry::Tarball(TarballPackage::from(tarball_package_lock))
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, DbtSchema)]
pub struct HubPackage {
    /// Package identifier on the dbt package hub, in `org/name` form (e.g. `dbt-labs/dbt_utils`).
    pub package: String,
    /// Version pin. Accepts a single version string, a list of constraints (e.g. `[">=1.0.0", "<2.0.0"]`), or a number.
    #[serde(rename = "version", skip_serializing_if = "Option::is_none")]
    pub version: Option<PackageVersion>,
    /// Allow installation of pre-release versions when resolving `version`.
    #[serde(rename = "install_prerelease", skip_serializing_if = "Option::is_none")]
    pub install_prerelease: Option<bool>,
}

impl From<HubPackageLock> for HubPackage {
    fn from(hub_package_lock: HubPackageLock) -> Self {
        HubPackage {
            package: hub_package_lock.package,
            version: Some(hub_package_lock.version),
            install_prerelease: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, DbtSchema)]
pub struct GitPackage {
    /// Git clone URL of the package repository (e.g. `https://github.com/dbt-labs/dbt_utils.git`).
    pub git: Verbatim<String>,
    /// Revision to check out: a tag, branch, or commit SHA.
    #[serde(rename = "revision", skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Suppress the warning emitted when `revision` is unpinned (i.e. a branch name like `main`).
    #[serde(rename = "warn-unpinned", skip_serializing_if = "Option::is_none")]
    pub warn_unpinned: Option<bool>,
    /// Subdirectory of the repo where the dbt package is located.
    #[serde(rename = "subdirectory", skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
    #[schemars(skip)]
    #[serde(default, skip_serializing)]
    pub __unrendered__: HashMap<String, YmlValue>,
}

impl From<GitPackageLock> for GitPackage {
    fn from(git_package_lock: GitPackageLock) -> Self {
        GitPackage {
            git: git_package_lock.git,
            revision: Some(git_package_lock.revision),
            warn_unpinned: git_package_lock.warn_unpinned,
            subdirectory: git_package_lock.subdirectory,
            __unrendered__: git_package_lock.__unrendered__,
        }
    }
}

/// Git provider hosting a private package.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub enum PrivatePackageProvider {
    Github,
    Gitlab,
    /// Azure DevOps, also spelled `azure_devops`. Requires an `org/project/repo` path.
    #[serde(alias = "azure_devops")]
    Ado,
    /// Azure DevOps through Azure Active Directory. Hosted only, uses `org/repo`.
    AzureActiveDirectory,
}

impl PrivatePackageProvider {
    /// Every Azure DevOps spelling names the same provider.
    pub fn is_ado(self) -> bool {
        matches!(self, Self::Ado | Self::AzureActiveDirectory)
    }

    /// Azure DevOps spellings are interchangeable; other providers must match exactly.
    pub fn equivalent(self, other: Self) -> bool {
        (self.is_ado() && other.is_ado()) || self == other
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Ado => "ado",
            Self::AzureActiveDirectory => "azure_active_directory",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, DbtSchema)]
pub struct PrivatePackage {
    /// Private package identifier. Two-segment `org/repo` for GitHub, legacy Azure DevOps
    /// (`azure_devops`), or Azure Active Directory (`azure_active_directory`); three-or-more-segment
    /// `org/group/repo` for GitLab subgroups or Azure DevOps `org/project/repo` (`ado`).
    #[schemars(regex(pattern = r"^[\w\-\.]+(/[\w\-\.]+){1,}$"))]
    pub private: Verbatim<String>,
    /// Git provider. Defaults to `github` when unset.
    #[serde(rename = "provider", skip_serializing_if = "Option::is_none")]
    pub provider: Option<PrivatePackageProvider>,
    /// Revision to check out: a tag, branch, or commit SHA.
    #[serde(rename = "revision", skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// Suppress the warning emitted when `revision` is unpinned (i.e. a branch name like `main`).
    #[serde(rename = "warn-unpinned", skip_serializing_if = "Option::is_none")]
    pub warn_unpinned: Option<bool>,
    /// Subdirectory of the repo where the dbt package is located.
    #[serde(rename = "subdirectory", skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
    #[schemars(skip)]
    #[serde(default, skip_serializing)]
    pub __unrendered__: HashMap<String, YmlValue>,
}

impl From<PrivatePackageLock> for PrivatePackage {
    fn from(private_package_lock: PrivatePackageLock) -> Self {
        PrivatePackage {
            private: private_package_lock.private,
            provider: private_package_lock.provider,
            revision: Some(private_package_lock.revision),
            warn_unpinned: private_package_lock.warn_unpinned,
            subdirectory: private_package_lock.subdirectory,
            __unrendered__: private_package_lock.__unrendered__,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, DbtSchema)]
pub struct LocalPackage {
    /// Filesystem path to the local dbt package, relative to the project root.
    pub local: PathBuf,
}

impl From<LocalPackageLock> for LocalPackage {
    fn from(local_package_lock: LocalPackageLock) -> Self {
        LocalPackage {
            local: local_package_lock.local,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, DbtSchema)]
#[serde(untagged)]
pub enum PackageVersion {
    Number(f64),
    String(String),
    Array(Vec<String>),
}

impl Display for PackageVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageVersion::Number(number) => write!(f, "{}", number),
            PackageVersion::String(string) => write!(f, "{}", string),
            PackageVersion::Array(array) => write!(f, "[{}]", array.join(",")),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct DbtPackagesLock {
    #[serde(default)]
    pub packages: Vec<DbtPackageLock>,
    #[serde(default)]
    pub sha1_hash: String,
}

impl DbtPackagesLock {
    pub fn lookup_map(&self, root: &Path) -> BTreeMap<String, String> {
        self.packages
            .iter()
            .map(|p| (p.lookup_key(root), p.package_name()))
            .collect()
    }

    pub fn get_by_name(&self, name: &str) -> Option<&DbtPackageLock> {
        self.packages.iter().find(|p| p.package_name() == name)
    }

    pub fn has_duplicate_package_names(&self) -> bool {
        let mut seen = std::collections::HashSet::new();
        self.packages.iter().any(|p| !seen.insert(p.package_name()))
    }
}

#[derive(Debug, Serialize, UntaggedEnumDeserialize, Clone)]
#[serde(untagged)]
pub enum DbtPackageLock {
    Hub(HubPackageLock),
    Git(GitPackageLock),
    Local(LocalPackageLock),
    Private(PrivatePackageLock),
    Tarball(TarballPackageLock),
}

impl DbtPackageLock {
    pub fn package_name(&self) -> String {
        match self {
            DbtPackageLock::Hub(hub_package_lock) => hub_package_lock.name.to_string(),
            DbtPackageLock::Git(git_package_lock) => git_package_lock.name.to_string(),
            DbtPackageLock::Local(local_package_lock) => local_package_lock.name.to_string(),
            DbtPackageLock::Private(private_package_lock) => private_package_lock.name.to_string(),
            DbtPackageLock::Tarball(tarball_package_lock) => tarball_package_lock.name.to_string(),
        }
    }

    pub fn entry_name(&self) -> String {
        match self {
            DbtPackageLock::Hub(hub_package_lock) => hub_package_lock.package.to_string(),
            DbtPackageLock::Git(git_package_lock) => {
                let mut key = git_package_lock.git.to_string();
                if let Some(subdirectory) = &git_package_lock.subdirectory {
                    key.push_str(&format!("#{subdirectory}"));
                }
                key
            }
            DbtPackageLock::Local(local_package_lock) => {
                local_package_lock.local.to_string_lossy().to_string()
            }
            DbtPackageLock::Private(private_package_lock) => {
                let mut key = private_package_lock.private.to_string();
                if let Some(subdirectory) = &private_package_lock.subdirectory {
                    key.push_str(&format!("#{subdirectory}"));
                }
                key
            }
            DbtPackageLock::Tarball(tarball_package_lock) => {
                tarball_package_lock.tarball.to_string()
            }
        }
    }

    /// The pinned version or revision, when the package source has one.
    ///
    /// Local and tarball packages carry no version, so they return `None`.
    pub fn version_string(&self) -> Option<String> {
        match self {
            DbtPackageLock::Hub(package) => Some(package.version.to_string()),
            DbtPackageLock::Git(package) => Some(package.revision.clone()),
            DbtPackageLock::Private(package) => Some(package.revision.clone()),
            DbtPackageLock::Local(_) | DbtPackageLock::Tarball(_) => None,
        }
    }

    pub fn entry_type(&self) -> String {
        match self {
            DbtPackageLock::Hub(_) => "hub".to_string(),
            DbtPackageLock::Git(_) => "git".to_string(),
            DbtPackageLock::Local(_) => "local".to_string(),
            DbtPackageLock::Private(_) => "private".to_string(),
            DbtPackageLock::Tarball(_) => "tarball".to_string(),
        }
    }

    /// Key used to look up this lock entry against entries discovered in transitive
    /// `packages.yml` files. For `Local`, the lock stores a path relative to the root
    /// project; we resolve and canonicalize so the key matches no matter which working
    /// directory the comparison is computed from. For other variants, the entry name
    /// is already path-independent.
    pub fn lookup_key(&self, root: &Path) -> String {
        match self {
            DbtPackageLock::Local(local) => {
                let joined = root.join(&local.local);
                dbt_common::stdfs::canonicalize(&joined)
                    .unwrap_or(joined)
                    .to_string_lossy()
                    .to_string()
            }
            _ => self.entry_name(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HubPackageLock {
    pub package: String,
    pub name: String,
    #[serde(rename = "version")]
    pub version: PackageVersion,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GitPackageLock {
    pub git: Verbatim<String>,
    pub name: String,
    pub revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warn_unpinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
    #[serde(default, skip_serializing)]
    pub __unrendered__: HashMap<String, YmlValue>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LocalPackageLock {
    pub local: PathBuf,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PrivatePackageLock {
    pub private: Verbatim<String>,
    pub name: String,
    pub revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<PrivatePackageProvider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warn_unpinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
    #[serde(default, skip_serializing)]
    pub __unrendered__: HashMap<String, YmlValue>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TarballPackageLock {
    pub tarball: Verbatim<String>,
    pub name: String,
    #[serde(default, skip_serializing)]
    pub __unrendered__: HashMap<String, YmlValue>,
}

#[derive(Debug, Serialize, Deserialize, Clone, DbtSchema)]
pub struct TarballPackage {
    /// HTTPS URL of a `.tar.gz` archive containing the dbt package.
    pub tarball: Verbatim<String>,
    #[schemars(skip)]
    #[serde(default, skip_serializing)]
    pub __unrendered__: HashMap<String, YmlValue>,
}

impl From<TarballPackageLock> for TarballPackage {
    fn from(tarball_package_lock: TarballPackageLock) -> Self {
        TarballPackage {
            tarball: tarball_package_lock.tarball,
            __unrendered__: tarball_package_lock.__unrendered__,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct DeprecatedDbtPackagesLock {
    pub packages: Vec<DeprecatedDbtPackageLock>,
    #[serde(default)]
    pub sha1_hash: String,
}

#[derive(Debug, Serialize, UntaggedEnumDeserialize, Clone)]
#[serde(untagged)]
pub enum DeprecatedDbtPackageLock {
    Hub(DeprecatedHubPackageLock),
    Git(DeprecatedGitPackageLock),
    Local(DeprecatedLocalPackageLock),
    Private(DeprecatedPrivatePackageLock),
    Tarball(DeprecatedTarballPackageLock),
}

// NOTE: Every deprecated lock variant accepts and drops an optional `name` key.
// A partially-migrated `package-lock.yml` can mix entries that carry `name`
// with entries that do not; the missing `name` on one entry forces the whole
// file into this deprecated parser, which then re-infers names from the
// installed packages directory. Accepting (and ignoring) `name` here keeps such
// mixed files from failing with `UnusedConfigKey (dbt1060)`, matching dbt Core,
// which tolerates the key.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeprecatedHubPackageLock {
    pub package: String,
    #[serde(rename = "version")]
    pub version: PackageVersion,
    #[serde(default, skip_serializing)]
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeprecatedGitPackageLock {
    pub git: String,
    pub revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warn_unpinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
    #[serde(default, skip_serializing)]
    pub name: Option<String>,
    #[serde(default, skip_serializing)]
    pub __unrendered__: HashMap<String, YmlValue>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeprecatedLocalPackageLock {
    pub local: PathBuf,
    #[serde(default, skip_serializing)]
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeprecatedPrivatePackageLock {
    pub private: String,
    pub revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<PrivatePackageProvider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warn_unpinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<String>,
    #[serde(default, skip_serializing)]
    pub name: Option<String>,
    #[serde(default, skip_serializing)]
    pub __unrendered__: HashMap<String, YmlValue>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DeprecatedTarballPackageLock {
    pub tarball: String,
    #[serde(default, skip_serializing)]
    pub name: Option<String>,
    #[serde(default, skip_serializing)]
    pub __unrendered__: HashMap<String, YmlValue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_packages_lock_deserializes() {
        // Empty string (empty file)
        let result: DbtPackagesLock = dbt_yaml::from_str("").unwrap();
        assert!(result.packages.is_empty());
        assert!(result.sha1_hash.is_empty());

        // Fully commented-out content
        let commented =
            "# packages:\n#   - package: foo/bar\n#     version: 1.0.0\n# sha1_hash: abc123\n";
        let result: DbtPackagesLock = dbt_yaml::from_str(commented).unwrap();
        assert!(result.packages.is_empty());
        assert!(result.sha1_hash.is_empty());
    }

    /// A `package-lock.yml` mixing `name`/no-`name` entries fails the new schema
    /// (the entry without `name` matches no variant), forcing the deprecated
    /// parser — which must accept the `name` key on the other entries instead of
    /// rejecting it. https://github.com/dbt-labs/fs/issues/11678
    #[test]
    fn test_deprecated_lock_tolerates_mixed_name_forms() {
        let mixed = "\
packages:
  - package: fivetran/fivetran_utils
    version: [\">=0.4.3\", \"<1.0.0\"]
  - name: dbt_utils
    package: dbt-labs/dbt_utils
    version: \">=1.0.0\"
sha1_hash: 713df304d4720d43ae7280d2363c5e1b009e7c1b
";
        // The new schema requires `name` on hub entries, so the first (nameless)
        // entry makes the whole file fail to match the untagged enum.
        assert!(
            dbt_yaml::from_str::<DbtPackagesLock>(mixed).is_err(),
            "mixed lock should not match the new (name-required) schema"
        );

        // The deprecated schema must accept (and drop) the `name` key.
        let result: DeprecatedDbtPackagesLock = dbt_yaml::from_str(mixed).unwrap();
        assert_eq!(result.packages.len(), 2);
        match &result.packages[1] {
            DeprecatedDbtPackageLock::Hub(hub) => {
                assert_eq!(hub.package, "dbt-labs/dbt_utils");
                assert_eq!(hub.name.as_deref(), Some("dbt_utils"));
            }
            other => panic!("expected a hub lock entry, got {other:?}"),
        }
    }

    #[test]
    fn test_private_package_provider_spellings() {
        let package = |provider: &str| {
            dbt_yaml::from_str::<PrivatePackage>(&format!(
                "private: dbt-labs/some-repo\nprovider: {provider}\n"
            ))
            .map(|package| package.provider)
        };
        let lock = |provider: &str| {
            dbt_yaml::from_str::<PrivatePackageLock>(&format!(
                "private: dbt-labs/some-repo\nname: some-repo\nrevision: main\nprovider: {provider}\n"
            ))
            .map(|lock| lock.provider)
        };
        let deprecated_lock = |provider: &str| {
            dbt_yaml::from_str::<DeprecatedPrivatePackageLock>(&format!(
                "private: dbt-labs/some-repo\nrevision: main\nprovider: {provider}\n"
            ))
            .map(|lock| lock.provider)
        };

        for (spelling, provider) in [
            ("github", PrivatePackageProvider::Github),
            ("gitlab", PrivatePackageProvider::Gitlab),
            ("ado", PrivatePackageProvider::Ado),
            (
                "azure_active_directory",
                PrivatePackageProvider::AzureActiveDirectory,
            ),
        ] {
            assert_eq!(package(spelling).unwrap(), Some(provider));
            assert_eq!(lock(spelling).unwrap(), Some(provider));
            assert_eq!(deprecated_lock(spelling).unwrap(), Some(provider));
            assert_eq!(provider.as_str(), spelling);
            assert_eq!(dbt_yaml::to_string(&provider).unwrap().trim(), spelling);
        }

        // "azure_devops" is an alias for "ado", and normalizes to it on the way out
        for reader in [package, lock, deprecated_lock] {
            assert_eq!(
                reader("azure_devops").unwrap(),
                Some(PrivatePackageProvider::Ado)
            );
        }

        for invalid in ["azure-devops", "githbu", "GitHub"] {
            assert!(package(invalid).is_err(), "{invalid} should be rejected");
        }
    }

    #[test]
    fn test_private_package_provider_is_omitted_when_unset() {
        let package: PrivatePackage = dbt_yaml::from_str("private: dbt-labs/some-repo\n").unwrap();
        assert_eq!(package.provider, None);
        assert!(!dbt_yaml::to_string(&package).unwrap().contains("provider"));
    }
}
