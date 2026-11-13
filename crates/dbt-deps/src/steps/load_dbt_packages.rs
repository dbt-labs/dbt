use dbt_common::{
    ErrorCode, FsResult,
    constants::{DBT_DEPENDENCIES_YML, DBT_PACKAGES_YML},
    err, tokiofs,
};
use dbt_schemas::schemas::packages::DbtPackages;
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub enum DbtPackageType {
    PackageYml,
    DependenciesYml,
}

impl AsRef<str> for DbtPackageType {
    fn as_ref(&self) -> &str {
        match self {
            Self::PackageYml => DBT_PACKAGES_YML,
            Self::DependenciesYml => DBT_DEPENDENCIES_YML,
        }
    }
}

pub async fn load_dbt_packages(in_dir: &Path) -> FsResult<(Option<DbtPackages>, DbtPackageType)> {
    let package_yml_path = in_dir.join(DBT_PACKAGES_YML);
    let dbt_package_yml: Option<DbtPackages> = if tokiofs::path_exists(&package_yml_path).await {
        Some(read_dbt_package_yml(&package_yml_path).await?)
    } else {
        None
    };
    let dbt_dependencies_yml_path = in_dir.join(DBT_DEPENDENCIES_YML);
    let dbt_dependencies_yml: Option<DbtPackages> =
        if tokiofs::path_exists(&dbt_dependencies_yml_path).await {
            Some(read_dbt_package_yml(&dbt_dependencies_yml_path).await?)
        } else {
            None
        };
    // Determine which package definition to use
    // If both are present, we need to check if they are empty
    // If one is present and the other is not, we use the present one
    // If both are not present, we use the default package definition
    // If both are present and non-empty, we return an error
    match (dbt_package_yml, dbt_dependencies_yml) {
        (Some(dbt_package_yml), Some(dbt_dependencies_yml)) => {
            let has_packages_in_yml = !dbt_package_yml.packages.is_empty();
            let has_packages_in_deps = !dbt_dependencies_yml.packages.is_empty();
            let has_projects_in_deps = !dbt_dependencies_yml.projects.is_empty();

            match (
                has_packages_in_yml,
                has_packages_in_deps,
                has_projects_in_deps,
            ) {
                (true, false, true) => {
                    // Merge packages from packages.yml and projects from dependencies.yml
                    let mut merged = dbt_package_yml;
                    merged.projects = dbt_dependencies_yml.projects;
                    Ok((Some(merged), DbtPackageType::PackageYml))
                }
                (true, false, false) => Ok((Some(dbt_package_yml), DbtPackageType::PackageYml)),
                (false, true, _) => {
                    Ok((Some(dbt_dependencies_yml), DbtPackageType::DependenciesYml))
                }
                (false, false, _) => Ok((None, DbtPackageType::PackageYml)),
                (true, true, _) => err!(
                    ErrorCode::InvalidConfig,
                    "Both packages.yml and dependencies.yml dependency definitions exist. Only one is allowed."
                ),
            }
        }
        (Some(dbt_package_yml), None) => Ok((Some(dbt_package_yml), DbtPackageType::PackageYml)),
        (None, Some(dbt_dependencies_yml)) => {
            Ok((Some(dbt_dependencies_yml), DbtPackageType::DependenciesYml))
        }
        (None, None) => Ok((None, DbtPackageType::PackageYml)),
    }
}

async fn read_dbt_package_yml(package_yml_path: &Path) -> FsResult<DbtPackages> {
    let content = tokiofs::read_to_string(package_yml_path).await?;
    dbt_jinja_utils::serde::from_yaml_raw(&content, Some(package_yml_path), true, None)
}
