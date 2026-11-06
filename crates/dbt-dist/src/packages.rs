//! Optional libraries shipped with dbt.

use dbt_common::{ErrorCode, FsResult, fs_err};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Library directory, relative to the install prefix.
pub const LIBRARY_DIRECTORY: &str = "lib/dbt";

/// Where the install scripts put dbt, as named by their options.
#[derive(Debug, PartialEq, Eq)]
pub enum InstallLocation {
    /// `--prefix`: dbt in `<prefix>/bin`, libraries in `<prefix>/lib/dbt`.
    Prefix(PathBuf),
    /// The deprecated `--to`: dbt in this directory, without libraries.
    Directory(PathBuf),
}

impl InstallLocation {
    /// For a dbt binary in `bin_dir`: its parent is the prefix when `bin_dir` is named
    /// `bin`; any other directory is updated in place.
    pub fn for_bin_dir(bin_dir: &Path) -> Self {
        match bin_dir.parent() {
            Some(prefix) if bin_dir.file_name().is_some_and(|name| name == "bin") => {
                Self::Prefix(prefix.to_path_buf())
            }
            _ => Self::Directory(bin_dir.to_path_buf()),
        }
    }

    /// The directory that holds the dbt binary.
    pub fn bin_dir(&self) -> PathBuf {
        match self {
            Self::Prefix(prefix) => prefix.join("bin"),
            Self::Directory(dir) => dir.clone(),
        }
    }

    /// The directory that receives the libraries, if this location installs them.
    pub fn lib_dir(&self) -> Option<PathBuf> {
        match self {
            Self::Prefix(prefix) => Some(prefix.join("lib").join("dbt")),
            Self::Directory(_) => None,
        }
    }

    /// The install script option for this location (`install.sh --prefix`,
    /// `install.ps1 -prefix`, or `to`), and its path.
    pub fn script_option(&self) -> (&'static str, &Path) {
        match self {
            Self::Prefix(prefix) => ("prefix", prefix),
            Self::Directory(dir) => ("to", dir),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackageManifest {
    pub package: Vec<SidecarPackage>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SidecarPackage {
    pub name: String,
    pub upstream_repo: String,
    pub upstream_version: String,
    pub install_as: String,
    pub channels: Vec<String>,
    pub source: String,
    pub cdn_path_template: String,
    pub checksums: BTreeMap<String, String>,
}

impl PackageManifest {
    pub fn bundled() -> FsResult<Self> {
        Self::parse(include_str!("../optional-packages.toml"))
    }

    pub fn parse(text: &str) -> FsResult<Self> {
        let manifest: Self = toml::from_str(text).map_err(|error| {
            fs_err!(
                ErrorCode::InvalidArgument,
                "Invalid optional package manifest: {error}"
            )
        })?;
        let mut names = std::collections::BTreeSet::new();
        for package in &manifest.package {
            if !valid_component(&package.name)
                || !valid_component(&package.install_as)
                || !names.insert(&package.name)
                || package.source != "cdn"
                || package.checksums.values().any(|checksum| {
                    checksum.len() != 64 || !checksum.bytes().all(|c| c.is_ascii_hexdigit())
                })
            {
                return Err(fs_err!(
                    ErrorCode::InvalidArgument,
                    "Invalid optional package {}",
                    package.name
                ));
            }
        }
        Ok(manifest)
    }
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

impl SidecarPackage {
    pub fn applies(&self, channel: &str, target: &str) -> bool {
        self.channels.iter().any(|value| value == channel) && self.checksums.contains_key(target)
    }

    pub fn filename(&self, target: &str) -> String {
        let (prefix, suffix) = if target.contains("windows") {
            ("", "dll")
        } else if target.contains("apple") {
            ("lib", "dylib")
        } else {
            ("lib", "so")
        };
        format!("{prefix}{}.{suffix}", self.install_as)
    }

    pub fn cdn_path(&self, target: &str) -> String {
        let cdn_target = target.replace("unknown-linux-gnu", "manylinux_2_17-linux-gnu");
        let extension = if target.contains("windows") {
            "dll"
        } else if target.contains("apple") {
            "dylib"
        } else {
            "so"
        };
        self.cdn_path_template
            .replace("{target}", &cdn_target)
            .replace("{version}", &self.upstream_version)
            .replace("{extension}", extension)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_pins_real_targets_and_filenames() {
        let manifest = PackageManifest::bundled().unwrap();
        let bridge = &manifest.package[0];
        assert!(bridge.applies("pypi", "aarch64-apple-darwin"));
        assert!(!bridge.applies("pypi", "x86_64-apple-darwin"));
        assert_eq!(
            bridge.filename("aarch64-apple-darwin"),
            "libadbc_driver_bridge.dylib"
        );
        assert!(
            bridge
                .cdn_path("x86_64-unknown-linux-gnu")
                .contains("x86_64-manylinux_2_17-linux-gnu.so.zst")
        );
    }

    #[test]
    fn install_location_is_a_prefix_only_for_a_bin_directory() {
        let prefix = InstallLocation::for_bin_dir(Path::new("/opt/dbt/bin"));
        assert_eq!(prefix, InstallLocation::Prefix(PathBuf::from("/opt/dbt")));
        assert_eq!(prefix.bin_dir(), Path::new("/opt/dbt/bin"));
        assert_eq!(prefix.lib_dir().unwrap(), Path::new("/opt/dbt/lib/dbt"));
        assert_eq!(prefix.script_option(), ("prefix", Path::new("/opt/dbt")));

        let dir = InstallLocation::for_bin_dir(Path::new("/opt/dbt"));
        assert_eq!(dir, InstallLocation::Directory(PathBuf::from("/opt/dbt")));
        assert_eq!(dir.bin_dir(), Path::new("/opt/dbt"));
        assert_eq!(dir.lib_dir(), None);
        assert_eq!(dir.script_option(), ("to", Path::new("/opt/dbt")));

        assert!(matches!(
            InstallLocation::for_bin_dir(Path::new("/")),
            InstallLocation::Directory(_)
        ));
    }
}
