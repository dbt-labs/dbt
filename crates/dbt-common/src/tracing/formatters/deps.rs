use std::fmt;

use dbt_telemetry::{DepsAddPackage, DepsAllPackagesInstalled, DepsPackageInstalled, PackageType};
use dbt_tracing::StatusCode;

use super::{
    color::{GREEN, MaybeColored, RED},
    layout::RightAligned,
};

pub const INSTALLING_ACTION: RightAligned<&str> = RightAligned("Installing");
pub const INSTALLED_ACTION: RightAligned<&str> = RightAligned("Installed");
pub const ADDING_ACTION: RightAligned<&str> = RightAligned("Adding");
pub const ADDED_ACTION: RightAligned<&str> = RightAligned("Added");

/// Displays a package specification as "name@version" or just "name" if no version.
///
/// This is used for consistent package spec formatting across different event types
/// (DepsPackageInstalled, DepsAddPackage, etc.)
pub struct PackageSpec<'a> {
    pub name: &'a str,
    pub version: Option<&'a str>,
}

impl fmt::Display for PackageSpec<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.version {
            Some(v) => write!(f, "{}@{}", self.name, v),
            None => f.write_str(self.name),
        }
    }
}

/// Gets the display name for a package if available.
///
/// For hub packages: returns the package name (should always be known)
/// For other packages: returns name if known, otherwise url/path
pub fn get_package_display_name(pkg: &DepsPackageInstalled) -> Option<&str> {
    match pkg.package_type() {
        // hub package names are always known
        PackageType::Hub => pkg.package_name.as_deref(),
        // For git, local, private, tarball: use name if known, otherwise url/path
        PackageType::Git | PackageType::Local | PackageType::Private | PackageType::Tarball => pkg
            .package_name
            .as_deref()
            .or(pkg.package_url_or_path.as_deref()),
        PackageType::Unspecified => None,
    }
}

/// Version shown after a package name.
struct VersionSuffix<'a> {
    package_type: PackageType,
    package_version: Option<&'a str>,
}

impl fmt::Display for VersionSuffix<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.package_type {
            PackageType::Hub => {
                // For hub packages: "{name}: {version}"
                write!(
                    f,
                    ": {}",
                    // version must be known at span end for hub packages
                    self.package_version.unwrap_or("latest")
                )
            }
            // For git, local, private, tarball: "{name} from {version-hash}" or nothing if version unknown
            PackageType::Git | PackageType::Local | PackageType::Private | PackageType::Tarball => {
                if let Some(version) = self.package_version {
                    write!(f, " from {}", version)
                } else {
                    Ok(())
                }
            }
            PackageType::Unspecified => Ok(()),
        }
    }
}

/// `Installed`/`Added` in green, or `Failed` in red for an error status.
fn outcome_prefix(
    success: RightAligned<&'static str>,
    status: StatusCode,
    colorize: bool,
) -> MaybeColored<'static, OutcomePrefix> {
    match status {
        StatusCode::Unset | StatusCode::Ok => {
            MaybeColored::new(&GREEN, OutcomePrefix::Succeeded(success), colorize)
        }
        StatusCode::Error => MaybeColored::new(&RED, OutcomePrefix::Failed, colorize),
    }
}

/// Action shown for a finished package operation.
#[derive(Clone, Copy)]
enum OutcomePrefix {
    Succeeded(RightAligned<&'static str>),
    /// Unlike the success action, not aligned to the action column
    Failed,
}

impl fmt::Display for OutcomePrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Succeeded(action) => write!(f, "{action}"),
            Self::Failed => f.write_str("Failed"),
        }
    }
}

/// Displays the start of the package installation phase (Installing).
///
/// NOTE: as of today the package count is not included in the message,
/// because the true count is not resolved when span is started,
/// but it can be added in the future if dpes resolution flow is adjusted.
///
/// The total count is presented in the span end message.
pub struct PackageInstallStart<'a> {
    pub event: &'a DepsAllPackagesInstalled,
    pub colorize: bool,
}

impl fmt::Display for PackageInstallStart<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} packages",
            MaybeColored::new(&GREEN, INSTALLING_ACTION, self.colorize)
        )
    }
}

/// Displays the end of the package installation phase (Installed) with the package count.
pub struct PackageInstallEnd<'a> {
    pub event: &'a DepsAllPackagesInstalled,
    pub colorize: bool,
}

impl fmt::Display for PackageInstallEnd<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.event.package_count;
        write!(
            f,
            "{} {} {}",
            MaybeColored::new(&GREEN, INSTALLED_ACTION, self.colorize),
            count,
            if count > 1 { "packages" } else { "package" }
        )
    }
}

/// Displays a package installation start (Installing).
///
/// Writes the pattern: "Installing <details>" where <details> varies by package type:
/// - Hub packages: "name"
/// - Other packages: "name" or "url/path" if name is unknown
pub struct PackageInstalledStart<'a> {
    pub pkg: &'a DepsPackageInstalled,
    pub colorize: bool,
}

impl fmt::Display for PackageInstalledStart<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}",
            MaybeColored::new(&GREEN, INSTALLING_ACTION, self.colorize),
            get_package_display_name(self.pkg).unwrap_or("unknown")
        )
    }
}

/// Displays a package installation end (Installed).
///
/// Writes the pattern: "Installed <details>" where <details> varies by package type:
/// - Hub packages: "name: version"
/// - Other packages: "name"
pub struct PackageInstalledEnd<'a> {
    pub pkg: &'a DepsPackageInstalled,
    pub status: StatusCode,
    pub colorize: bool,
}

impl fmt::Display for PackageInstalledEnd<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}{}",
            outcome_prefix(INSTALLED_ACTION, self.status, self.colorize),
            get_package_display_name(self.pkg).unwrap_or("unknown"),
            VersionSuffix {
                package_type: self.pkg.package_type(),
                package_version: self.pkg.package_version.as_deref(),
            }
        )
    }
}

/// Displays a package add start (Adding).
///
/// Writes the pattern: "Adding <package name>"
pub struct PackageAddStart<'a> {
    pub pkg: &'a DepsAddPackage,
    pub colorize: bool,
}

impl fmt::Display for PackageAddStart<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}",
            MaybeColored::new(&GREEN, ADDING_ACTION, self.colorize),
            self.pkg.package_name
        )
    }
}

/// Displays a package add end (Added).
///
/// Writes the pattern: "Added <package name><version suffix>"
pub struct PackageAddEnd<'a> {
    pub pkg: &'a DepsAddPackage,
    pub status: StatusCode,
    pub colorize: bool,
}

impl fmt::Display for PackageAddEnd<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}{}",
            outcome_prefix(ADDED_ACTION, self.status, self.colorize),
            self.pkg.package_name,
            VersionSuffix {
                package_type: self.pkg.package_type(),
                package_version: self.pkg.package_version.as_deref(),
            }
        )
    }
}
