use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use dbt_path::normalize;
use dbt_schemas::schemas::{packages::LocalPackage, project::DbtProjectNameOnly};

use dbt_common::{
    ErrorCode, FsError, FsResult,
    constants::DBT_PROJECT_YML,
    env_secrets::{env_secret_values, scrub_secrets, scrub_secrets_from_error},
    fs_err, tokiofs,
};

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

pub fn get_local_package_full_path(in_dir: &Path, local_package: &LocalPackage) -> PathBuf {
    if local_package.local.is_absolute() {
        local_package.local.clone()
    } else {
        in_dir.join(&local_package.local)
    }
}

/// Replaces the value of every `DBT_ENV_SECRET*` environment variable found in `text` with
/// `*****`. Returns `None` when `text` holds no secret.
pub fn scrub_secret_env_vars(text: &str) -> Option<Cow<'_, str>> {
    scrub_secrets(text, &env_secret_values())
}

/// Scrubs the values of the `DBT_ENV_SECRET*` environment variables from the message of
/// `err` and of every error chained to it.
///
/// A package URL in `packages.yml` may carry a secret, for example
/// `https://{{ env_var('DBT_ENV_SECRET_GIT_TOKEN') }}@github.com/org/repo.git`, and an error
/// about that package may repeat the rendered URL. Call this before the error is logged,
/// recorded on a span or returned.
pub(crate) fn scrub_secret_env_vars_from_error(err: Box<FsError>) -> Box<FsError> {
    scrub_secrets_from_error(err, &env_secret_values())
}

/// Build the error raised when a package's `dbt_project.yml` cannot be found.
///
/// `checkout_path` is the resolved package directory (which may contain `..`
/// segments, e.g. `.../datahub/../common`). It is normalized lexically so the
/// reported path is readable; we normalize rather than canonicalize because a
/// genuinely-missing directory cannot be canonicalized. `dir_exists` selects
/// between "directory missing entirely" and "directory present but missing
/// `dbt_project.yml`".
fn missing_dbt_project_error(checkout_path: &Path, dir_exists: bool) -> Box<FsError> {
    let normalized = normalize(checkout_path);
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

/// Matches the credentials of a URL anywhere in a text: `scheme://user[:password]@`.
static URL_CREDENTIALS_REGEX: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(?i)\b([a-z][a-z0-9+.-]*://)[^/?#\s'"]+@"#).unwrap());

/// Removes the credentials from every URL in `text`, e.g. in git's stderr:
/// `fatal: could not read Password for 'https://token@github.com': ...`
///   becomes `fatal: could not read Password for 'https://github.com': ...`
///
/// Use this for text that contains URLs. [`sanitize_git_url`] is for a string that is a
/// URL as a whole: it returns git's `fatal: ...` and `remote: ...` lines unchanged,
/// because they parse as URLs with the scheme `fatal` or `remote`.
pub(crate) fn strip_url_credentials(text: &str) -> Cow<'_, str> {
    URL_CREDENTIALS_REGEX.replace_all(text, "$1")
}

/// The URL of a git or tarball package as logs and telemetry show it: without credentials,
/// and with the values of the `DBT_ENV_SECRET*` environment variables as `*****`. A secret
/// can be anywhere in the URL, for example in the query string.
pub(crate) fn package_url_for_display(url: &str) -> String {
    package_url_for_display_with_secrets(url, &env_secret_values())
}

fn package_url_for_display_with_secrets(url: &str, secrets: &[String]) -> String {
    // Scrub first. `sanitize_git_url` percent-encodes some characters, and an encoded
    // secret would no longer match.
    match scrub_secrets(url, secrets) {
        Some(scrubbed) => sanitize_git_url(&scrubbed),
        None => sanitize_git_url(url),
    }
}

/// Sanitizes username/password segments from git urls
/// e.g. https://username:password@github.com/dbt-labs/secret-project
///   becomes https://github.com/dbt-labs/secret-project
///
/// `url` must be a URL as a whole. For text that contains URLs, such as git's stderr, use
/// [`strip_url_credentials`].
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
        let expected = normalize(checkout_path).display().to_string();
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

    fn secrets(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn package_url_for_display_scrubs_a_secret_anywhere_in_the_url() {
        let secrets = secrets(&["tok-1", "p@ss word"]);
        for (url, expected) in [
            (
                "https://example.com/pkg.tar.gz?private_token=tok-1",
                "https://example.com/pkg.tar.gz?private_token=*****",
            ),
            (
                "https://example.com/tok-1/pkg.tar.gz",
                "https://example.com/*****/pkg.tar.gz",
            ),
            (
                "https://tok-1@github.com/org/repo.git",
                "https://github.com/org/repo.git",
            ),
            // The URL parser would encode this secret as `p@ss%20word`.
            (
                "https://example.com/pkg.tar.gz?key=p@ss word",
                "https://example.com/pkg.tar.gz?key=*****",
            ),
            (
                "https://example.com/pkg.tar.gz",
                "https://example.com/pkg.tar.gz",
            ),
        ] {
            assert_eq!(
                package_url_for_display_with_secrets(url, &secrets),
                expected
            );
        }
    }

    #[test]
    fn strip_url_credentials_strips_the_token_git_repeats_in_stderr() {
        // git's stderr when the token in the package URL is rejected and git can't prompt
        // for a password (no terminal, as in a container).
        for reason in ["No such device or address", "Device not configured"] {
            let stderr = format!(
                "fatal: could not read Password for 'https://ghp_token123@github.com': {reason}"
            );
            assert_eq!(
                strip_url_credentials(&stderr),
                format!("fatal: could not read Password for 'https://github.com': {reason}")
            );
        }
    }

    #[test]
    fn strip_url_credentials_strips_every_url_in_the_text() {
        assert_eq!(
            strip_url_credentials(
                "remote: denied for https://user:p@ss@github.com/org/repo.git\n\
                 fatal: unable to access 'ssh://git:tok@example.com:22/org/repo.git/'"
            ),
            "remote: denied for https://github.com/org/repo.git\n\
             fatal: unable to access 'ssh://example.com:22/org/repo.git/'"
        );
    }

    #[test]
    fn strip_url_credentials_leaves_text_without_credentials_unchanged() {
        for text in [
            "fatal: repository 'https://github.com/org/repo.git/' not found",
            "error: pathspec 'user@example.com' did not match any file(s) known to git",
            "fatal: couldn't find remote ref https://github.com/org/repo/tree/a@b",
            "fatal: unable to access 'https://example.com:8443?next=a@b'",
            "",
        ] {
            assert_eq!(strip_url_credentials(text), text);
        }
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
