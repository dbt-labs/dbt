mod resolve;

use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub use dbt_schemas::schemas::{DbtCloudConfig, DbtCloudContext, DbtCloudProject};
pub use resolve::{
    CloudCredentials, ResolvedCloudConfig, detect_unlinked_project, resolve_cloud_config,
};

/// Test-only env var honored by [`get_cloud_project_path`] in debug builds.
/// Set by the test harness to point the loader at a non-existent directory,
/// isolating tests from the developer's real `~/.dbt/dbt_cloud.yml`.
pub const TEST_CLOUD_CONFIG_DIR_ENV: &str = "_TEST_CLOUD_CONFIG_DIR";

/// Returns the expected path to `~/.dbt/dbt_cloud.yml`.
///
/// In debug builds, honors [`TEST_CLOUD_CONFIG_DIR_ENV`] as a test-only
/// override so the test harness can isolate a process from the developer's
/// real `~/.dbt/dbt_cloud.yml` without moving files or overriding `HOME`.
/// Stripped from release binaries.
pub fn get_cloud_project_path() -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os(TEST_CLOUD_CONFIG_DIR_ENV) {
        return Ok(PathBuf::from(dir).join("dbt_cloud.yml"));
    }

    dirs::home_dir()
        .map(|home| home.join(".dbt").join("dbt_cloud.yml"))
        .ok_or_else(|| "Could not determine home directory".to_string())
}

/// Reads and parses `dbt_cloud.yml` at `path`, returning the full config
/// or `None` if the file does not exist.
pub fn parse_cloud_config(path: &Path) -> Result<Option<DbtCloudConfig>, String> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!("Failed to read {}: {}", path.display(), e));
        }
    };

    let config: DbtCloudConfig = dbt_yaml::from_str(&content)
        .map_err(|e| format!("Failed to parse dbt_cloud.yml: {}", e))?;

    Ok(Some(config))
}

/// Reads and parses `dbt_cloud.yml` at `path`, returning the active
/// [`DbtCloudProject`] if one is configured.
pub fn parse_active_cloud_project(path: &Path) -> Result<Option<DbtCloudProject>, String> {
    let config = match parse_cloud_config(path)? {
        Some(config) => config,
        None => return Ok(None),
    };

    let active_project_id = config.context.active_project;
    Ok(config
        .projects
        .into_iter()
        .find(|p| p.project_id == active_project_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cloud_config_dir_override_redirects_path() {
        // `cargo test` always runs with debug_assertions on, so the
        // override branch is compiled in.
        let original = std::env::var_os(TEST_CLOUD_CONFIG_DIR_ENV);
        unsafe {
            #[allow(clippy::disallowed_methods)]
            std::env::set_var(TEST_CLOUD_CONFIG_DIR_ENV, "/nonexistent/dbt-test");
        }

        let path = get_cloud_project_path().unwrap();
        assert_eq!(path, PathBuf::from("/nonexistent/dbt-test/dbt_cloud.yml"));
        assert!(!path.exists());
        assert!(parse_cloud_config(&path).unwrap().is_none());

        unsafe {
            #[allow(clippy::disallowed_methods)]
            match original {
                Some(v) => std::env::set_var(TEST_CLOUD_CONFIG_DIR_ENV, v),
                None => std::env::remove_var(TEST_CLOUD_CONFIG_DIR_ENV),
            }
        }
    }
}
