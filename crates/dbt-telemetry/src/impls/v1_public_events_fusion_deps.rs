use crate::proto::v1::public::events::fusion::deps::{
    DepsAddPackage, DepsAllPackagesInstalled, DepsPackageInstalled, PackageType,
};

impl PackageType {
    pub fn as_static_str(&self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Hub => "hub",
            Self::Git => "git",
            Self::Local => "local",
            Self::Private => "private",
            Self::Tarball => "tarball",
        }
    }
}

impl DepsAllPackagesInstalled {
    /// Creates a new `DepsAllPackagesInstalled` span event.
    ///
    /// # Arguments
    /// * `package_count` - The total number of packages to be installed.
    pub fn start(package_count: u64) -> Self {
        Self::new(package_count)
    }
}

impl DepsPackageInstalled {
    /// Creates a new `DepsPackageInstalled` span start message.
    ///
    /// # Arguments
    /// * package_name - The optional name of the package that was installed if known at span start.
    /// * package_type - The type of the package source
    /// * package_version - Optional version of the package (for hub packages, this is semantic version; for git/private, this is commit SHA)
    /// * package_url_or_path - Optional scrubbed URL or path (for git, local, tarball packages)
    pub fn start(
        package_name: Option<String>,
        package_type: PackageType,
        package_version: Option<String>,
        package_url_or_path: Option<String>,
    ) -> Self {
        Self::new(
            package_name,
            package_type,
            package_version,
            package_url_or_path,
            "M014".to_string(),
        )
    }
}

impl DepsAddPackage {
    /// Creates a new `DepsAddPackage` span start message.
    ///
    /// # Arguments
    /// * package_name - The name of the package being added.
    /// * package_type - The type of the package source
    /// * package_version - Optional version of the package (for hub packages, this is semantic version; for git/private, this is commit SHA)
    pub fn start(
        package_name: String,
        package_type: PackageType,
        package_version: Option<String>,
    ) -> Self {
        Self::new(
            package_name,
            package_type,
            package_version,
            "M032".to_string(),
        )
    }
}
