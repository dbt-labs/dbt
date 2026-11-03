use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use dbt_schemas::schemas::{packages::LocalPackage, project::DbtProjectNameOnly};
use normalize_path::NormalizePath;

use dbt_common::{ErrorCode, FsResult, constants::DBT_PROJECT_YML, fs_err, tokiofs};

const DEFAULT_DEPS_MAX_CONCURRENCY: usize = 8;
const MAX_DEPS_CONCURRENCY: usize = 16;

/// Max concurrent package resolve/install operations.
///
/// Override with `DBT_DEPS_MAX_CONCURRENCY` (clamped to 1..=16).
pub fn max_resolve_concurrency() -> usize {
    match std::env::var("DBT_DEPS_MAX_CONCURRENCY") {
        Ok(raw) => raw
            .parse::<usize>()
            .map(|n| n.clamp(1, MAX_DEPS_CONCURRENCY))
            .unwrap_or(DEFAULT_DEPS_MAX_CONCURRENCY),
        Err(_) => DEFAULT_DEPS_MAX_CONCURRENCY,
    }
}
use dbt_jinja_utils::{
    jinja_environment::JinjaEnv,
    phases::load::LoadContext,
    serde::{into_typed_with_jinja, value_from_file_async},
    utils::SECRET_ENV_VAR_PREFIX,
};
use dbt_schemas::schemas::project::DbtProject;

/// Create `path` (and missing parents), mapping any I/O error into an
/// `FsResult` that includes the path for context.
pub async fn ensure_dir(path: &Path) -> FsResult<()> {
    tokiofs::create_dir_all(path).await.map_err(|e| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to create directory '{}': {}",
            path.display(),
            e,
        )
    })
}

/// Create a new `TempDir`, optionally inside `parent`, mapping any I/O error
/// into an `FsResult`. When `parent` is `None` the system temp dir is used.
pub fn make_tempdir(parent: Option<&Path>) -> FsResult<tempfile::TempDir> {
    let result = match parent {
        Some(dir) => tempfile::tempdir_in(dir),
        None => tempfile::tempdir(),
    };
    result.map_err(|e| fs_err!(ErrorCode::IoError, "Failed to create temp dir: {}", e))
}

/// Move a directory from `src` to `dst`.
///
/// Attempts an atomic `rename` first. If that fails due to a cross-device error
/// (src and dst on different filesystems), falls back to a recursive copy followed
/// by deletion of the source.
pub async fn move_dir(src: &Path, dst: &Path) -> FsResult<()> {
    if tokiofs::rename(src, dst).await.is_ok() {
        return Ok(());
    }
    // rename failed (e.g. cross-device) — fall back to recursive copy + delete
    copy_dir_recursive(src, dst).await?;
    tokiofs::remove_dir_all(src).await
}

async fn copy_dir_recursive(src: &Path, dst: &Path) -> FsResult<()> {
    tokiofs::create_dir_all(dst).await?;
    let mut entries = tokiofs::read_dir(src).await?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to read directory entry: {}", e))?
    {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry.file_type().await.map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to get file type for '{}': {}",
                src_path.display(),
                e
            )
        })?;
        if file_type.is_dir() {
            Box::pin(copy_dir_recursive(&src_path, &dst_path)).await?;
        } else {
            tokiofs::copy(&src_path, &dst_path).await?;
        }
    }
    Ok(())
}

pub fn get_local_package_full_path(in_dir: &Path, local_package: &LocalPackage) -> PathBuf {
    if local_package.local.is_absolute() {
        local_package.local.clone()
    } else {
        in_dir.join(&local_package.local)
    }
}

pub fn scrub_package_name_secret_env_vars(package_name: &str) -> Option<Cow<'_, str>> {
    let mut scrubbed = Cow::Borrowed(package_name);
    for (_, secret) in std::env::vars()
        .filter(|(key, value)| key.starts_with(SECRET_ENV_VAR_PREFIX) && !value.trim().is_empty())
    {
        if scrubbed.contains(secret.as_str()) {
            scrubbed = Cow::Owned(scrubbed.replace(secret.as_str(), "*****"));
        }
    }

    match scrubbed {
        Cow::Borrowed(_) => None,
        owned @ Cow::Owned(_) => Some(owned),
    }
}

/// Build the error raised when a package's `dbt_project.yml` cannot be found.
///
/// `checkout_path` is the resolved package directory (which may contain `..`
/// segments, e.g. `.../datahub/../common`). It is normalized lexically so the
/// reported path is readable; we normalize rather than canonicalize because a
/// genuinely-missing directory cannot be canonicalized. `dir_exists` selects
/// between "directory missing entirely" and "directory present but missing
/// `dbt_project.yml`".
fn missing_dbt_project_error(checkout_path: &Path, dir_exists: bool) -> Box<dbt_common::FsError> {
    let normalized = checkout_path.normalize();
    if dir_exists {
        fs_err!(
            ErrorCode::IoError,
            "Package does not contain a dbt_project.yml file: {}. Verify that each entry within packages.yml (and their transitive dependencies) contains a file named dbt_project.yml.",
            normalized.display()
        )
    } else {
        fs_err!(
            ErrorCode::IoError,
            "Package directory not found: {}. Verify that each entry within packages.yml (and their transitive dependencies) points to a directory containing a dbt_project.yml file.",
            normalized.display()
        )
    }
}

pub async fn read_and_validate_dbt_project(
    checkout_path: &Path,
    show_errors_or_warnings: bool,
    jinja_env: &JinjaEnv,
    vars: &BTreeMap<String, dbt_yaml::Value>,
) -> FsResult<DbtProject> {
    let path_to_dbt_project = checkout_path.join(DBT_PROJECT_YML);
    if !tokiofs::path_exists(&path_to_dbt_project).await {
        // Distinguish "the package directory is missing entirely" from "the
        // directory exists but has no dbt_project.yml", so a co-location
        // problem (e.g. a `local:` package source dir not present) is not
        // misreported as a missing project file.
        let dir_exists = tokiofs::path_exists(checkout_path).await;
        return Err(missing_dbt_project_error(checkout_path, dir_exists));
    }

    // Try to deserialize only the package name for error reporting,
    // falling back to the path if deserialization fails
    let dependency_package_name = value_from_file_async(&path_to_dbt_project, false, None)
        .await
        .ok()
        .and_then(|value| {
            let deps_context = LoadContext::new(vars.clone());
            into_typed_with_jinja::<DbtProjectNameOnly, _>(
                value,
                false,
                jinja_env,
                &deps_context,
                &[],
                None,
                false,
            )
            .ok()
        })
        .map(|p| p.name)
        .unwrap_or_else(|| path_to_dbt_project.to_string_lossy().to_string());

    let deps_context = LoadContext::new(vars.clone());
    into_typed_with_jinja(
        value_from_file_async(
            &path_to_dbt_project,
            show_errors_or_warnings,
            Some(&dependency_package_name),
        )
        .await?,
        false,
        jinja_env,
        &deps_context,
        &[],
        Some(&dependency_package_name),
        show_errors_or_warnings,
    )
}

/// Sanitizes username/password segments from git urls
/// e.g. https://username:password@github.com/dbt-labs/secret-project
///   becomes https://github.com/dbt-labs/secret-project
pub fn sanitize_git_url(url: &str) -> String {
    if let Ok(mut parsed) = url::Url::parse(url) {
        let _ = parsed.set_username("");
        let _ = parsed.set_password(None);
        parsed.to_string()
    } else {
        // Fallback if Url can't parse - use regex
        let patterns_to_remove = [
            // https://token@host
            r"https://[^@/]+@([^/]+)",
            // https://user:pass@host
            r"https://[^:]+:[^@/]+@([^/]+)",
            // git@host:path
            r"git@([^:]+):",
        ];

        let mut sanitized = url.to_string();
        for pattern in &patterns_to_remove {
            if let Ok(re) = regex::Regex::new(pattern) {
                if *pattern == r"git@([^:]+):" {
                    // Special handling for SSH format: git@host:path -> https://host/path
                    sanitized = re.replace(&sanitized, "https://$1/").to_string();
                } else {
                    sanitized = re.replace(&sanitized, "https://$1").to_string();
                }
            }
        }
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_dbt_project_error_normalizes_dotdot_in_path() {
        // `.../datahub/../common` should be reported as the resolved
        // `.../common`, not verbatim with the `..` segment.
        let checkout_path = Path::new("/repo/datahub/../common");
        let err = missing_dbt_project_error(checkout_path, true);
        let msg = err.to_string();
        // Build the expected path via `normalize()` so the comparison uses the
        // platform's path separator (`/repo/common` on Unix, `\repo\common` on
        // Windows) rather than hard-coded forward slashes.
        let expected = checkout_path.normalize().display().to_string();
        assert!(
            msg.contains(&expected),
            "expected normalized path in message, got: {msg}"
        );
        assert!(!msg.contains(".."), "path should be normalized, got: {msg}");
    }

    #[test]
    fn missing_dbt_project_error_reports_missing_project_file_when_dir_exists() {
        let err = missing_dbt_project_error(Path::new("/repo/common"), true);
        let msg = err.to_string();
        assert!(
            msg.contains("does not contain a dbt_project.yml file"),
            "got: {msg}"
        );
        assert!(!msg.contains("directory not found"), "got: {msg}");
    }

    #[test]
    fn missing_dbt_project_error_reports_missing_directory_when_dir_absent() {
        let err = missing_dbt_project_error(Path::new("/repo/does_not_exist"), false);
        let msg = err.to_string();
        assert!(msg.contains("Package directory not found"), "got: {msg}");
        assert!(
            !msg.contains("does not contain a dbt_project.yml file"),
            "got: {msg}"
        );
    }

    #[test]
    fn test_sanitize_git_url_basic_credentials() {
        let url = "https://username:password@github.com/dbt-labs/secret-project";
        let sanitized = sanitize_git_url(url);
        assert_eq!(sanitized, "https://github.com/dbt-labs/secret-project");
    }

    #[test]
    fn test_sanitize_git_url_token_only() {
        let url = "https://ghp_1234567890abcdef@github.com/dbt-labs/secret-project";
        let sanitized = sanitize_git_url(url);
        assert_eq!(sanitized, "https://github.com/dbt-labs/secret-project");
    }

    #[test]
    fn test_sanitize_git_url_github_token() {
        let url = "https://ghp_abcdef1234567890@github.com/dbt-labs/dbt-core.git";
        let sanitized = sanitize_git_url(url);
        assert_eq!(sanitized, "https://github.com/dbt-labs/dbt-core.git");
    }

    #[test]
    fn test_sanitize_git_url_ssh_format() {
        let url = "git@github.com:dbt-labs/secret-project.git";
        let sanitized = sanitize_git_url(url);
        assert_eq!(sanitized, "https://github.com/dbt-labs/secret-project.git");
    }

    #[test]
    fn test_sanitize_git_url_with_path_and_query() {
        let url = "https://user:pass@github.com/dbt-labs/secret-project.git?ref=main";
        let sanitized = sanitize_git_url(url);
        assert_eq!(
            sanitized,
            "https://github.com/dbt-labs/secret-project.git?ref=main"
        );
    }

    #[test]
    fn test_sanitize_git_url_already_clean() {
        let url = "https://github.com/dbt-labs/dbt-core.git";
        let sanitized = sanitize_git_url(url);
        assert_eq!(sanitized, "https://github.com/dbt-labs/dbt-core.git");
    }

    #[test]
    fn test_sanitize_git_url_http_instead_of_https() {
        let url = "http://user:pass@github.com/dbt-labs/secret-project.git";
        let sanitized = sanitize_git_url(url);
        assert_eq!(sanitized, "http://github.com/dbt-labs/secret-project.git");
    }

    #[test]
    fn test_sanitize_git_url_username_only() {
        let url = "https://username@github.com/dbt-labs/secret-project.git";
        let sanitized = sanitize_git_url(url);
        assert_eq!(sanitized, "https://github.com/dbt-labs/secret-project.git");
    }

    #[test]
    fn test_sanitize_git_url_special_characters_in_credentials() {
        let url = "https://user%40domain.com:pass%21word@github.com/dbt-labs/secret-project.git";
        let sanitized = sanitize_git_url(url);
        assert_eq!(sanitized, "https://github.com/dbt-labs/secret-project.git");
    }

    #[test]
    fn test_sanitize_git_url_gitlab() {
        let url = "https://oauth2:glpat-1234567890@gitlab.com/dbt-labs/secret-project.git";
        let sanitized = sanitize_git_url(url);
        assert_eq!(sanitized, "https://gitlab.com/dbt-labs/secret-project.git");
    }

    #[test]
    fn test_sanitize_git_url_azure_devops() {
        let url = "https://user:pat@dev.azure.com/organization/project/_git/repo";
        let sanitized = sanitize_git_url(url);
        assert_eq!(
            sanitized,
            "https://dev.azure.com/organization/project/_git/repo"
        );
    }

    #[test]
    fn test_sanitize_git_url_bitbucket() {
        let url = "https://user:app_password@bitbucket.org/dbt-labs/secret-project.git";
        let sanitized = sanitize_git_url(url);
        assert_eq!(
            sanitized,
            "https://bitbucket.org/dbt-labs/secret-project.git"
        );
    }

    #[test]
    fn test_sanitize_git_url_invalid_url() {
        let url = "not-a-valid-url";
        let sanitized = sanitize_git_url(url);
        // Should return the original string since it's not a valid URL
        assert_eq!(sanitized, "not-a-valid-url");
    }

    #[test]
    fn test_sanitize_git_url_empty_string() {
        let url = "";
        let sanitized = sanitize_git_url(url);
        assert_eq!(sanitized, "");
    }

    #[test]
    fn test_sanitize_git_url_multiple_credentials() {
        let url = "https://user1:pass1@github.com/dbt-labs/secret-project.git";
        let sanitized = sanitize_git_url(url);
        assert_eq!(sanitized, "https://github.com/dbt-labs/secret-project.git");
    }

    #[test]
    fn test_sanitize_git_url_with_fragment() {
        let url = "https://user:pass@github.com/dbt-labs/secret-project.git#v1.0.0";
        let sanitized = sanitize_git_url(url);
        assert_eq!(
            sanitized,
            "https://github.com/dbt-labs/secret-project.git#v1.0.0"
        );
    }

    #[test]
    fn test_sanitize_git_url_long_token() {
        let url = "https://ghp_1234567890abcdef1234567890abcdef12345678@github.com/dbt-labs/secret-project.git";
        let sanitized = sanitize_git_url(url);
        assert_eq!(sanitized, "https://github.com/dbt-labs/secret-project.git");
    }

    #[test]
    fn test_sanitize_git_url_x_access_token() {
        let url =
            "https://x-access-token:ghp_1234567890abcdef@github.com/dbt-labs/secret-project.git";
        let sanitized = sanitize_git_url(url);
        assert_eq!(sanitized, "https://github.com/dbt-labs/secret-project.git");
    }
}
