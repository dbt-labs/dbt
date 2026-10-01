use std::sync::LazyLock;

use dbt_telemetry::{DepsAddPackage, DepsAllPackagesInstalled, DepsPackageInstalled, PackageType};
use dbt_tracing::StatusCode;

use super::{
    color::{GREEN, RED, maybe_apply_color},
    layout::right_align_static_action,
};

pub static INSTALLING_ACTION: LazyLock<String> = LazyLock::new(|| {
    // Use shared function for consistent column width even if this is a static string
    right_align_static_action("Installing")
});

pub static INSTALLED_ACTION: LazyLock<String> = LazyLock::new(|| {
    // Use shared function for consistent column width even if this is a static string
    right_align_static_action("Installed")
});

pub static ADDING_ACTION: LazyLock<String> = LazyLock::new(|| {
    // Use shared function for consistent column width even if this is a static string
    right_align_static_action("Adding")
});

pub static ADDED_ACTION: LazyLock<String> = LazyLock::new(|| {
    // Use shared function for consistent column width even if this is a static string
    right_align_static_action("Added")
});

/// Formats a package specification as "name@version" or just "name" if no version.
///
/// This is used for consistent package spec formatting across different event types
/// (DepsPackageInstalled, DepsAddPackage, etc.)
///
/// # Arguments
/// * `name` - The package name
/// * `version` - Optional package version
///
/// # Returns
/// A formatted string: "name@version" if version is present, otherwise just "name"
pub fn format_package_spec(name: &str, version: Option<&str>) -> String {
    match version {
        Some(v) => format!("{}@{}", name, v),
        None => name.to_string(),
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

fn format_version_suffix(package_type: PackageType, package_version: Option<&str>) -> String {
    match package_type {
        PackageType::Hub => {
            // For hub packages: "{name}: {version}"
            format!(
                ": {}",
                // version must be known at span end for hub packages
                package_version.unwrap_or("latest")
            )
        }
        // For git, local, private, tarball: "{name} from {version-hash}" or nothing if version unknown
        PackageType::Git | PackageType::Local | PackageType::Private | PackageType::Tarball => {
            if let Some(version) = package_version {
                format!(" from {}", version)
            } else {
                String::from("")
            }
        }
        PackageType::Unspecified => String::from(""),
    }
}

/// Formats a package installation phase message for span start (Installing).
///
/// # Arguments
/// * `event` - The DepsAllPackagesInstalled event
/// * `colorize` - Whether to apply green color to the "Installing" prefix
///
/// NOTE: as of today the package count is not included in the message,
/// because the true count is not resolved when span is started,
/// but it can be added in the future if dpes resolution flow is adjusted.
///
/// The total count is presented in the span end message.
pub fn format_package_install_start(_event: &DepsAllPackagesInstalled, colorize: bool) -> String {
    let prefix = maybe_apply_color(&GREEN, INSTALLING_ACTION.as_str(), colorize);

    format!("{} {}", prefix, "packages")
}

/// Formats a package installation phase message for span start (Installing).
///
/// # Arguments
/// * `event` - The DepsAllPackagesInstalled event
/// * `colorize` - Whether to apply green color to the "Installing" prefix
pub fn format_package_install_end(event: &DepsAllPackagesInstalled, colorize: bool) -> String {
    let prefix = maybe_apply_color(&GREEN, INSTALLED_ACTION.as_str(), colorize);

    format!(
        "{} {} {}",
        prefix,
        event.package_count,
        if event.package_count > 1 {
            "packages"
        } else {
            "package"
        }
    )
}

/// Formats a package installation message for span start (Installing).
///
/// # Arguments
/// * `pkg` - The DepsPackageInstalled event containing package details
/// * `colorize` - Whether to apply green color to the "Installing" prefix
///
/// # Returns
/// A formatted message string in the form: "Installing <details>"
/// where <details> varies by package type:
/// - Hub packages: "name"
/// - Other packages: "name" or "url/path" if name is unknown
pub fn format_package_installed_start(pkg: &DepsPackageInstalled, colorize: bool) -> String {
    let message_detail = get_package_display_name(pkg).unwrap_or("unknown");

    let prefix = maybe_apply_color(&GREEN, INSTALLING_ACTION.as_str(), colorize);
    format!("{} {}", prefix, message_detail)
}

/// Formats a package installation message for span end (Installed).
///
/// # Arguments
/// * `pkg` - The DepsPackageInstalled event containing package details
/// * 'status' - The installation status of the package
/// * `colorize` - Whether to apply green color to the "Installed" prefix
///
/// # Returns
/// A formatted message string in the form: "Installed <details>"
/// where <details> varies by package type:
/// - Hub packages: "name: version"
/// - Other packages: "name"
pub fn format_package_installed_end(
    pkg: &DepsPackageInstalled,
    status: StatusCode,
    colorize: bool,
) -> String {
    let pkg_display_name = get_package_display_name(pkg).unwrap_or("unknown");
    let version_suffix = format_version_suffix(pkg.package_type(), pkg.package_version.as_deref());

    let prefix = match status {
        StatusCode::Unset | StatusCode::Ok => {
            maybe_apply_color(&GREEN, INSTALLED_ACTION.as_str(), colorize)
        }
        StatusCode::Error => maybe_apply_color(&RED, "Failed", colorize),
    };

    format!("{} {}{}", prefix, pkg_display_name, version_suffix)
}

/// Formats a package add message for span start (Adding).
///
/// # Arguments
/// * `pkg` - The DepsAddPackage event containing package details
/// * `colorize` - Whether to apply green color to the "Adding" prefix
///
/// # Returns
/// A formatted message string in the form: "Adding <details>"
/// where <details> is the package name with optional version
pub fn format_package_add_start(pkg: &DepsAddPackage, colorize: bool) -> String {
    let prefix = maybe_apply_color(&GREEN, ADDING_ACTION.as_str(), colorize);
    format!("{} {}", prefix, pkg.package_name)
}

/// Formats a package add message for span end (Added).
///
/// # Arguments
/// * `pkg` - The DepsAddPackage event containing package details
/// * `status` - The add status of the package
/// * `colorize` - Whether to apply green color to the "Added" prefix
///
/// # Returns
/// A formatted message string in the form: "Added <details>"
/// where <details> is the package name with optional version
pub fn format_package_add_end(pkg: &DepsAddPackage, status: StatusCode, colorize: bool) -> String {
    let version_suffix = format_version_suffix(pkg.package_type(), pkg.package_version.as_deref());

    let prefix = match status {
        StatusCode::Unset | StatusCode::Ok => {
            maybe_apply_color(&GREEN, ADDED_ACTION.as_str(), colorize)
        }
        StatusCode::Error => maybe_apply_color(&RED, "Failed", colorize),
    };

    format!("{} {}{}", prefix, pkg.package_name, version_suffix)
}
