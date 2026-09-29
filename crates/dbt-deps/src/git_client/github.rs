//! GitHub-specific fast path: archive download + GraphQL ref resolution.
//!
//! Ref resolution tries GraphQL first and falls back to `git ls-remote` on any
//! failure. Downloads use the GitHub archive endpoint; on failure the caller
//! falls back to `git clone`.

use std::path::Path;

use async_trait::async_trait;
use dbt_common::tracing::dbt_emit::emit_debug_log_message;
use dbt_common::{ErrorCode, FsResult, fs_err};
use reqwest_middleware::ClientWithMiddleware;

use super::auth::github_token_for_request;
use super::traits::GitHostClient;
use super::{DownloadMethod, DownloadOutcome, ParsedGitUrl, ResolveMethod};
use crate::tarball_client::TarballClient;

/// Whether a string is a syntactically valid GitHub owner or repo name.
///
/// GitHub allows `[a-zA-Z0-9_.-]`. Required before interpolating owner/repo
/// values into GraphQL queries — anything else could break the query or open
/// injection vectors.
pub(super) fn is_gh_repo_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// GitHub fast-path client (archive + GraphQL); repo/token come from `parsed`.
pub struct GitHubClient {
    http_client: ClientWithMiddleware,
    tarball_client: TarballClient,
}

impl GitHubClient {
    pub fn new(http_client: ClientWithMiddleware, tarball_client: TarballClient) -> Self {
        Self {
            http_client,
            tarball_client,
        }
    }
}

#[async_trait]
impl GitHostClient for GitHubClient {
    /// Eligible when the URL parses as a github.com URL with owner+repo
    /// names that pass `is_gh_repo_valid_name` (interpolation safety for
    /// the GraphQL query and archive endpoint).
    fn can_handle(&self, parsed: &ParsedGitUrl) -> bool {
        parsed.parts.as_ref().is_some_and(|p| {
            p.host == "github.com"
                && is_gh_repo_valid_name(&p.owner)
                && is_gh_repo_valid_name(&p.repo)
        })
    }

    fn download_supports_revision(&self) -> bool {
        true
    }

    async fn resolve_ref(
        &self,
        parsed: &ParsedGitUrl,
        revision: &str,
    ) -> FsResult<(String, ResolveMethod)> {
        let parts = require_parts(parsed)?;
        let owner = &parts.owner;
        let repo = &parts.repo;
        let auth_token = parts.token.as_deref();
        graphql_resolve(&self.http_client, owner, repo, revision, auth_token)
            .await
            .map(|sha| (sha, ResolveMethod::GithubGraphql))
    }

    async fn download(
        &self,
        parsed: &ParsedGitUrl,
        revision: &str,
        target_dir: &Path,
        subdirectory: Option<&str>,
    ) -> FsResult<DownloadOutcome> {
        emit_debug_log_message(
            "Using experimental GitHub GraphQL package resolution & install path",
        );
        let parts = require_parts(parsed)?;
        let owner = &parts.owner;
        let repo = &parts.repo;
        let auth_token = parts.token.as_deref();
        download_archive(
            &self.tarball_client,
            owner,
            repo,
            revision,
            target_dir,
            subdirectory,
            auth_token,
        )
        .await?;
        Ok(DownloadOutcome {
            checkout_path: target_dir.to_path_buf(),
            download_method: DownloadMethod::Archive,
        })
    }
}

/// Internal: extract `parts` from a `ParsedGitUrl`. The fast-path gate in
/// `mod.rs::get_git_client` only routes to GitHub when parts are present
/// and valid, so reaching this with `None` is a programming error.
fn require_parts(parsed: &ParsedGitUrl) -> FsResult<&super::GitUrlParts> {
    parsed.parts.as_ref().ok_or_else(|| {
        fs_err!(
            ErrorCode::PackageDownloadFailed,
            "Missing GitHub URL parts (internal: GitHub fast path called with unrecognized URL)"
        )
    })
}

// ============================================================================
// Archive download
// ============================================================================

async fn download_archive(
    tarball_client: &TarballClient,
    owner: &str,
    repo: &str,
    revision: &str,
    target_dir: &Path,
    subdirectory: Option<&str>,
    auth_token: Option<&str>,
) -> FsResult<()> {
    let archive_url = format!(
        "https://github.com/{}/{}/archive/{}.tar.gz",
        owner, repo, revision
    );

    let auth_header = github_token_for_request(auth_token).map(|t| format!("Bearer {}", t));
    let headers: Vec<(&str, &str)> = auth_header
        .as_deref()
        .map(|v| vec![("Authorization", v)])
        .unwrap_or_default();

    tarball_client
        .download_and_extract_tarball(&archive_url, target_dir, true, subdirectory, &headers, None)
        .await
        .map_err(|e| {
            fs_err!(
                ErrorCode::PackageDownloadFailed,
                "Failed to download/extract archive: {}",
                e
            )
        })?;

    Ok(())
}

// ============================================================================
// GraphQL resolution
// ============================================================================

async fn graphql_resolve(
    http_client: &ClientWithMiddleware,
    owner: &str,
    repo: &str,
    revision: &str,
    auth_token: Option<&str>,
) -> FsResult<String> {
    // Escape all interpolated values for GraphQL safety.
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let owner_escaped = escape(owner);
    let repo_escaped = escape(repo);
    let rev_escaped = escape(revision);
    let query = format!(
        r#"{{ repository(owner: "{owner_escaped}", name: "{repo_escaped}") {{ tag_ref: ref(qualifiedName: "refs/tags/{rev_escaped}") {{ target {{ oid }} }} object_ref: object(expression: "{rev_escaped}") {{ oid }} }} }}"#
    );

    let effective_token = github_token_for_request(auth_token);
    let json = fire_graphql(http_client, &query, effective_token.as_deref()).await?;
    let repo_data = &json["data"]["repository"];
    let sha = repo_data["tag_ref"]["target"]["oid"]
        .as_str()
        .or_else(|| repo_data["object_ref"]["oid"].as_str());

    match sha {
        Some(s) if s.len() == 40 => Ok(s.to_string()),
        _ => Err(fs_err!(
            ErrorCode::PackageDownloadFailed,
            "Could not resolve {owner}/{repo} ref '{revision}'"
        )),
    }
}

async fn fire_graphql(
    http_client: &ClientWithMiddleware,
    query: &str,
    auth_token: Option<&str>,
) -> FsResult<serde_json::Value> {
    let mut req = http_client
        .post("https://api.github.com/graphql")
        .header("Content-Type", "application/json")
        .header(
            "User-Agent",
            concat!("dbt-fusion/", env!("CARGO_PKG_VERSION")),
        );

    if let Some(token) = auth_token.filter(|token| !token.is_empty()) {
        req = req.header("Authorization", format!("Bearer {}", token));
    }

    let resp = req
        .json(&serde_json::json!({ "query": query }))
        .send()
        .await
        .map_err(|e| {
            fs_err!(
                ErrorCode::PackageDownloadFailed,
                "GraphQL request failed: {}",
                e
            )
        })?;

    let status = resp.status();
    if !status.is_success() {
        // Map auth-relevant HTTP statuses to typed error codes so the user
        // sees `AuthFailed` / `PermissionDenied` instead of a generic
        // download error. Other statuses fall through as
        // `PackageDownloadFailed`.
        let code = match status.as_u16() {
            401 => ErrorCode::AuthFailed,
            403 => ErrorCode::PermissionDenied,
            _ => ErrorCode::PackageDownloadFailed,
        };
        return Err(fs_err!(code, "GraphQL HTTP {}", status));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| {
        fs_err!(
            ErrorCode::PackageDownloadFailed,
            "GraphQL parse failed: {}",
            e
        )
    })?;

    if let Some(errors) = json.get("errors") {
        return Err(fs_err!(
            ErrorCode::PackageDownloadFailed,
            "GraphQL errors: {}",
            errors
        ));
    }

    Ok(json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_gh_repo_valid_name_accepts_github_char_classes() {
        // Alphanumeric, dash, underscore, dot — all legal.
        assert!(is_gh_repo_valid_name("dbt-labs"));
        assert!(is_gh_repo_valid_name("dbt_utils"));
        assert!(is_gh_repo_valid_name("v1.0.0"));
        assert!(is_gh_repo_valid_name("A1"));
        assert!(is_gh_repo_valid_name("a.b-c_d"));
    }

    #[test]
    fn is_gh_repo_valid_name_rejects_empty_and_injection_chars() {
        assert!(!is_gh_repo_valid_name(""));
        // Characters that would break GraphQL interpolation.
        assert!(!is_gh_repo_valid_name(r#"evil"org"#));
        assert!(!is_gh_repo_valid_name(r"evil\org"));
        assert!(!is_gh_repo_valid_name("evil$org"));
        assert!(!is_gh_repo_valid_name("evil{org}"));
        assert!(!is_gh_repo_valid_name("a/b"));
        assert!(!is_gh_repo_valid_name("a b"));
        // Non-ASCII.
        assert!(!is_gh_repo_valid_name("café"));
    }
}
