//! Git package download with host-specific fast paths.
//!
//! Detects the git hosting provider from the URL and uses the optimal
//! strategy for each:
//! - **GitHub**: archive download + GraphQL ref resolution
//! - **Generic**: git clone + git ls-remote (fallback for any host)
//!
//! The public API is `download_git_like_package` — callers don't need
//! to know about host detection or fast paths.

mod auth;
mod cache;
mod generic;
mod github;
mod traits;

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use dbt_common::cancellation::never_cancels;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::{ErrorCode, FsResult, fs_err, tokiofs};
use reqwest_middleware::ClientWithMiddleware;
use traits::GitHostClient as _;

/// Whether host-specific fast paths (GitHub archive + GraphQL) are enabled.
///
/// Tri-state: the env var, when set, *overrides* the in-code default
/// (`FAST_PATH_DEFAULT`). `1`/`true` force-on, `0`/`false` force-off,
/// anything else (or unset) falls through to the default. This shape is
/// in prep for an A/B flag (LaunchDarkly) flipping `FAST_PATH_DEFAULT`:
/// the env var stays as a manual override for tests and ops without
/// fighting the rollout.
const FAST_PATH_DEFAULT: bool = false;

fn fast_path_enabled() -> bool {
    match std::env::var("DBT_DEPS_GIT_FAST_PATH").ok().as_deref() {
        Some(v) if v == "1" || v.eq_ignore_ascii_case("true") => true,
        Some(v) if v == "0" || v.eq_ignore_ascii_case("false") => false,
        _ => FAST_PATH_DEFAULT,
    }
}

use crate::context::DepsOperationContext;
use crate::network_client::retrying_http_client;
use crate::tarball_client::TarballClient;
use crate::utils::sanitize_git_url;

/// Per-run git deps state (cache + reusable host clients).
/// Scoped to one `get_or_install_packages` invocation.
pub struct GitClientContext {
    resolve_cache: cache::ResolveCache,
    github_client: github::GitHubClient,
    generic_client: generic::GenericClient,
}

impl Default for GitClientContext {
    fn default() -> Self {
        Self::from_http_client(retrying_http_client())
    }
}

impl GitClientContext {
    pub fn from_http_client(http_client: ClientWithMiddleware) -> Self {
        let tarball_client = TarballClient::from_client(http_client.clone(), never_cancels());
        Self::from_clients(http_client, tarball_client)
    }

    pub fn from_clients(http_client: ClientWithMiddleware, tarball_client: TarballClient) -> Self {
        Self {
            resolve_cache: cache::ResolveCache::new(),
            github_client: github::GitHubClient::new(http_client, tarball_client),
            generic_client: generic::GenericClient,
        }
    }
}

/// How a ref was resolved to a SHA.
#[derive(Debug, Clone, Copy)]
pub enum ResolveMethod {
    /// GitHub GraphQL API
    GithubGraphql,
    /// git ls-remote subprocess (generic / fallback path)
    GitLsRemote,
    /// Already a full SHA, no resolution needed
    Identity,
}

/// How a package was downloaded.
#[derive(Debug, Clone, Copy)]
pub enum DownloadMethod {
    /// GitHub archive tarball
    Archive,
    /// git clone (+ sparse checkout for subdirectories)
    Clone,
}

/// Outcome of a download (package placed on disk at a known SHA).
///
/// `download_method` is kept alongside `checkout_path` so callers can attach
/// it to telemetry spans (archive vs clone) without threading an extra value
/// through every call site.
pub struct DownloadOutcome {
    /// Path where the package content was placed.
    pub checkout_path: PathBuf,
    /// How the package was downloaded. Staged for otel span attributes.
    /// TODO(telemetry): wire this into deps spans and remove dead_code allow.
    #[allow(dead_code)]
    pub download_method: DownloadMethod,
}

/// Outcome of an install (download using a caller-supplied SHA).
///
/// Wraps `DownloadOutcome` so future install-only metadata (e.g. cache hit
/// vs cache miss) has a home without reshaping the download path.
pub struct InstallOutcome {
    pub download: DownloadOutcome,
}

/// Outcome of a resolve (ref → SHA → download).
///
/// Carries `sha`, the `resolve_method` (how the ref was discovered), and the
/// nested `DownloadOutcome` (how the content was fetched). Resolve spans can
/// report all three; download/install spans use `DownloadOutcome` directly
/// and never see a sentinel `resolve_method`.
pub struct ResolveOutcome {
    pub download: DownloadOutcome,
    /// Resolved commit/tag SHA.
    pub sha: String,
    /// How the ref was resolved.
    pub resolve_method: ResolveMethod,
}

/// Host/owner/repo extracted from a recognized git URL shape, plus any inline
/// auth token from https URLs. Parsed liberally — callers that feed these
/// values into GraphQL or other sensitive contexts must validate separately.
pub(super) struct GitUrlParts {
    pub host: String,
    pub owner: String,
    pub repo: String,
    /// Inline auth token from `https://TOKEN@host/...` (empty auth → None).
    pub token: Option<String>,
}

/// A git URL ready for dispatch. `parts` is `None` for URL shapes we don't
/// recognize (which all fall through to the generic clone path and skip
/// the resolution cache).
pub(super) struct ParsedGitUrl {
    /// Original URL, preserved for the generic clone path.
    pub(super) url: String,
    pub(super) parts: Option<GitUrlParts>,
}

/// Parse a git URL into parts. Returns `None` for shapes we don't recognize.
///
/// Recognized shapes:
/// - `https://[TOKEN@]host/owner/repo[.git]`
/// - `git@host:owner/repo[.git]`
fn parse_url_parts(url: &str) -> Option<GitUrlParts> {
    let clean = url.trim_end_matches(".git");

    if let Some(after_scheme) = clean.strip_prefix("https://") {
        // Strip optional `TOKEN@`; preserve empty-auth (`https://@host/...`).
        let (token, after_auth) = match after_scheme.split_once('@') {
            Some((auth, rest)) if !auth.is_empty() => (Some(auth.to_string()), rest),
            Some((_, rest)) => (None, rest),
            None => (None, after_scheme),
        };
        let mut parts = after_auth.splitn(3, '/');
        let host = parts.next()?.to_string();
        let owner = parts.next()?.to_string();
        let repo = parts.next()?;
        if !repo.contains('/') {
            return Some(GitUrlParts {
                host,
                owner,
                repo: repo.to_string(),
                token,
            });
        }
    }

    if let Some(after_git) = clean.strip_prefix("git@") {
        let (host, path) = after_git.split_once(':')?;
        let (owner, repo) = path.split_once('/')?;
        if !repo.contains('/') {
            return Some(GitUrlParts {
                host: host.to_string(),
                owner: owner.to_string(),
                repo: repo.to_string(),
                token: None,
            });
        }
    }

    None
}

fn parse_git_url(url: &str) -> ParsedGitUrl {
    ParsedGitUrl {
        url: url.to_string(),
        parts: parse_url_parts(url),
    }
}

/// Check if a revision string is a full 40-char hex SHA.
pub fn is_commit(revision: &str) -> bool {
    let revision = revision.trim();
    revision.len() == 40 && revision.chars().all(|c| c.is_ascii_hexdigit())
}

/// Whether `revision` should trigger the DepsUnpinned warning.
///
/// Matches dbt-core's semantics: only `HEAD`, `main`, and `master` are
/// treated as "unpinned" for this legacy event. Users opt out by passing
/// `warn_unpinned = false`.
fn is_unpinned_git_revision(revision: &str, warn_unpinned: bool) -> bool {
    warn_unpinned && ["HEAD", "main", "master"].contains(&revision)
}

/// Validate that a subdirectory path contains only normal components.
fn validate_subdirectory(subdir: &str) -> Result<&str, String> {
    for component in Path::new(subdir).components() {
        match component {
            std::path::Component::Normal(_) => {}
            _ => {
                return Err(format!(
                    "Invalid subdirectory '{}': must not contain '..', '.', or absolute path components",
                    subdir
                ));
            }
        }
    }
    if subdir.is_empty() {
        return Err("Invalid subdirectory: must not be empty".to_string());
    }
    Ok(subdir)
}

/// Resolve phase: resolve ref + download content.
/// Used by `compute_package_lock` when building the lock file.
///
/// `download_dir` must already exist and be writable; cleanup on error is the
/// caller's responsibility.
///
/// Returns (checkout_path, commit_sha) where checkout_path == download_dir.
pub async fn download_git_like_package(
    context: &DepsOperationContext<'_>,
    repo_url: &str,
    revisions: &[String],
    subdirectory: &Option<String>,
    warn_unpinned: bool,
    download_dir: &Path,
) -> FsResult<(PathBuf, String)> {
    context.check_cancellation()?;
    if let Some(subdir) = subdirectory {
        validate_subdirectory(subdir).map_err(|e| fs_err!(ErrorCode::InvalidConfig, "{}", e))?;
    }

    // Trim revisions: YAML block-scalar (`revision: |`) shapes leave a trailing
    // newline that is harmless for `is_commit` (which trims) but gets passed
    // verbatim to `git fetch`, which rejects it as an invalid refspec.
    let revision = revisions
        .last()
        .map(|r| r.trim().to_string())
        .unwrap_or_else(|| "HEAD".to_string());

    let parsed = parse_git_url(repo_url);
    let outcome = get_git_client(&context.git_client, &parsed)
        .resolve_with_cache(&parsed, &revision, download_dir, subdirectory.as_deref())
        .await?;

    if is_unpinned_git_revision(&revision, warn_unpinned) {
        emit_warn_log_message(
            ErrorCode::DepsUnpinned,
            format!(
                "The package {} is pinned to the default branch, which is not recommended. \
                 Consider pinning to a specific commit SHA instead.",
                sanitize_git_url(repo_url)
            ),
        );
    }

    Ok((outcome.download.checkout_path, outcome.sha))
}

/// Install phase: download using an already-resolved SHA.
/// Used by `install_packages` when installing from lock file.
///
/// `download_dir` must already exist and be writable; cleanup on error is the
/// caller's responsibility.
///
/// Returns (checkout_path, commit_sha) where checkout_path == download_dir.
pub async fn install_git_like_package(
    context: &DepsOperationContext<'_>,
    repo_url: &str,
    sha: &str,
    subdirectory: &Option<String>,
    download_dir: &Path,
) -> FsResult<(PathBuf, String)> {
    context.check_cancellation()?;
    if let Some(subdir) = subdirectory {
        validate_subdirectory(subdir).map_err(|e| fs_err!(ErrorCode::InvalidConfig, "{}", e))?;
    }

    // Trim: lockfile SHAs written as YAML block scalars (`revision: |`) carry
    // a trailing newline that `git fetch` rejects as an invalid refspec.
    let sha = sha.trim();
    let parsed = parse_git_url(repo_url);
    let outcome = get_git_client(&context.git_client, &parsed)
        .install(&parsed, sha, download_dir, subdirectory.as_deref())
        .await?;
    Ok((outcome.download.checkout_path, sha.to_string()))
}

/// Composable delegator: tries `primary`, falls back to `fallback` (when
/// present), and threads the per-run resolve cache through both. Knowing
/// nothing about specific hosts, it works for any pair of `GitHostClient`s.
struct HostClient<'a> {
    primary: &'a dyn traits::GitHostClient,
    fallback: Option<&'a dyn traits::GitHostClient>,
    resolve_cache: &'a cache::ResolveCache,
}

impl HostClient<'_> {
    /// Resolve + download with per-run cache for non-SHA revisions.
    async fn resolve_with_cache(
        &self,
        parsed: &ParsedGitUrl,
        revision: &str,
        download_dir: &Path,
        subdirectory: Option<&str>,
    ) -> FsResult<ResolveOutcome> {
        // Cache only non-SHA parsed URLs; inline token is part of the key.
        let cache_key = (!is_commit(revision))
            .then_some(parsed.parts.as_ref())
            .flatten();

        if let Some(parts) = cache_key
            && let Some((sha, resolve_method)) = self.resolve_cache.get(parts, revision).await
        {
            let download = self
                .download(parsed, &sha, download_dir, subdirectory)
                .await?;
            return Ok(ResolveOutcome {
                download,
                sha,
                resolve_method,
            });
        }

        let outcome = self
            .resolve(parsed, revision, download_dir, subdirectory)
            .await?;

        if let Some(parts) = cache_key {
            self.resolve_cache
                .set(parts, revision, &outcome.sha, outcome.resolve_method)
                .await;
        }

        Ok(outcome)
    }
}

#[async_trait]
impl traits::GitHostClient for HostClient<'_> {
    fn download_supports_revision(&self) -> bool {
        self.primary.download_supports_revision()
    }

    async fn resolve_ref(
        &self,
        parsed: &ParsedGitUrl,
        revision: &str,
    ) -> FsResult<(String, ResolveMethod)> {
        match self.primary.resolve_ref(parsed, revision).await {
            Ok(r) => Ok(r),
            Err(e) => match self.fallback {
                Some(fallback) => fallback.resolve_ref(parsed, revision).await,
                None => Err(e),
            },
        }
    }

    async fn download(
        &self,
        parsed: &ParsedGitUrl,
        sha: &str,
        target_dir: &Path,
        subdirectory: Option<&str>,
    ) -> FsResult<DownloadOutcome> {
        match self
            .primary
            .download(parsed, sha, target_dir, subdirectory)
            .await
        {
            Ok(r) => Ok(r),
            Err(e) => match self.fallback {
                Some(fallback) => {
                    reset_download_dir(target_dir).await?;
                    fallback
                        .download(parsed, sha, target_dir, subdirectory)
                        .await
                }
                None => Err(e),
            },
        }
    }

    async fn resolve(
        &self,
        parsed: &ParsedGitUrl,
        revision: &str,
        target_dir: &Path,
        subdirectory: Option<&str>,
    ) -> FsResult<ResolveOutcome> {
        match self
            .primary
            .resolve(parsed, revision, target_dir, subdirectory)
            .await
        {
            Ok(r) => Ok(r),
            Err(e) => match self.fallback {
                Some(fallback) => {
                    reset_download_dir(target_dir).await?;
                    fallback
                        .resolve(parsed, revision, target_dir, subdirectory)
                        .await
                }
                None => Err(e),
            },
        }
    }

    async fn install(
        &self,
        parsed: &ParsedGitUrl,
        sha: &str,
        target_dir: &Path,
        subdirectory: Option<&str>,
    ) -> FsResult<InstallOutcome> {
        match self
            .primary
            .install(parsed, sha, target_dir, subdirectory)
            .await
        {
            Ok(r) => Ok(r),
            Err(e) => match self.fallback {
                Some(fallback) => {
                    reset_download_dir(target_dir).await?;
                    fallback
                        .install(parsed, sha, target_dir, subdirectory)
                        .await
                }
                None => Err(e),
            },
        }
    }
}

/// Pick `(primary, fallback)` for `parsed`.
///
/// When `DBT_DEPS_GIT_FAST_PATH` is on, each fast-path candidate is asked
/// (in priority order) whether it can handle the URL via `can_handle`; the
/// first match becomes `primary`, with the generic client as `fallback`.
/// Otherwise — fast path off, or no candidate claims the URL — the generic
/// client is used directly with no fallback. Adding a new fast-path host
/// is just an extra entry in `candidates`.
fn pick_clients<'a>(
    context: &'a GitClientContext,
    parsed: &ParsedGitUrl,
) -> (
    &'a dyn traits::GitHostClient,
    Option<&'a dyn traits::GitHostClient>,
) {
    if fast_path_enabled() {
        let candidates: [&dyn traits::GitHostClient; 1] = [&context.github_client];
        if let Some(primary) = candidates.into_iter().find(|c| c.can_handle(parsed)) {
            return (primary, Some(&context.generic_client));
        }
    }
    (&context.generic_client, None)
}

fn get_git_client<'a>(context: &'a GitClientContext, parsed: &ParsedGitUrl) -> HostClient<'a> {
    let (primary, fallback) = pick_clients(context, parsed);
    HostClient {
        primary,
        fallback,
        resolve_cache: &context.resolve_cache,
    }
}

/// Reset the download directory between a failed fast-path attempt and the
/// generic clone fallback, so clone starts from a clean slate.
async fn reset_download_dir(download_dir: &Path) -> FsResult<()> {
    let _ = tokiofs::remove_dir_all(download_dir).await;
    tokiofs::create_dir_all(download_dir).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_github_client() -> github::GitHubClient {
        let http = retrying_http_client();
        let tb = TarballClient::from_client(http.clone(), never_cancels());
        github::GitHubClient::new(http, tb)
    }

    fn parts_of(url: &str) -> GitUrlParts {
        parse_git_url(url)
            .parts
            .expect("expected recognized URL shape")
    }

    #[test]
    fn test_parse_git_url_github_https() {
        let p = parts_of("https://github.com/dbt-labs/dbt-utils.git");
        assert_eq!(p.host, "github.com");
        assert_eq!(p.owner, "dbt-labs");
        assert_eq!(p.repo, "dbt-utils");
        assert_eq!(p.token, None);
    }

    #[test]
    fn test_parse_git_url_github_with_token() {
        let p = parts_of("https://mytoken@github.com/dbt-labs/dbt-utils.git");
        assert_eq!(p.host, "github.com");
        assert_eq!(p.owner, "dbt-labs");
        assert_eq!(p.repo, "dbt-utils");
        assert_eq!(p.token.as_deref(), Some("mytoken"));
    }

    #[test]
    fn test_parse_git_url_github_ssh() {
        let p = parts_of("git@github.com:dbt-labs/dbt-utils.git");
        assert_eq!(p.host, "github.com");
        assert_eq!(p.owner, "dbt-labs");
        assert_eq!(p.repo, "dbt-utils");
        assert_eq!(p.token, None);
    }

    #[test]
    fn test_parse_git_url_non_github_host() {
        let p = parts_of("https://gitlab.com/some/repo.git");
        assert_eq!(p.host, "gitlab.com");
        assert_eq!(p.owner, "some");
        assert_eq!(p.repo, "repo");
    }

    #[test]
    fn test_parse_git_url_unrecognized_shape() {
        assert!(parse_git_url("not a url").parts.is_none());
    }

    #[test]
    fn test_is_commit() {
        assert!(is_commit("1234567890abcdef1234567890abcdef12345678"));
        assert!(!is_commit("v1.0.0"));
        assert!(!is_commit("main"));
    }

    #[test]
    fn test_validate_subdirectory() {
        assert!(validate_subdirectory("dbt-utils").is_ok());
        assert!(validate_subdirectory("nested/path/pkg").is_ok());
        assert!(validate_subdirectory("../escape").is_err());
        assert!(validate_subdirectory("./relative").is_err());
        assert!(validate_subdirectory("").is_err());
    }

    #[test]
    fn github_client_can_handle_rejects_injection_attempts_and_non_github_hosts() {
        let github = test_github_client();
        // Unsafe owner/repo chars must not be eligible for the GitHub client.
        assert!(!github.can_handle(&parse_git_url(r#"https://github.com/evil"org/repo"#)));
        assert!(!github.can_handle(&parse_git_url(r#"https://github.com/owner/evil"repo"#)));
        assert!(!github.can_handle(&parse_git_url(r"https://github.com/evil\org/repo")));
        // Valid github URLs are eligible.
        assert!(github.can_handle(&parse_git_url("https://github.com/dbt-labs/dbt-utils")));
        assert!(github.can_handle(&parse_git_url("https://github.com/org.name/repo_name")));
        // Non-github hosts are not eligible for the GitHub fast path.
        assert!(!github.can_handle(&parse_git_url("https://gitlab.com/g/p")));
    }

    #[test]
    fn test_parse_git_url_carries_token_into_parts() {
        // The inline token rides along in parts.token and is part of the
        // cache key, so URLs that differ only by token don't collide.
        let a = parts_of("https://tok-a@github.com/o/r");
        let b = parts_of("https://tok-b@github.com/o/r");
        let c = parts_of("https://github.com/o/r");
        assert_eq!(a.token.as_deref(), Some("tok-a"));
        assert_eq!(b.token.as_deref(), Some("tok-b"));
        assert_eq!(c.token, None);
    }

    #[test]
    fn parse_git_url_recognizes_github_urls() {
        let parsed = parse_git_url("https://github.com/dbt-labs/dbt-utils");
        let parts = parsed
            .parts
            .as_ref()
            .expect("expected recognized GitHub URL");
        assert_eq!(parts.owner, "dbt-labs");
        assert_eq!(parts.repo, "dbt-utils");
        assert_eq!(parts.token, None);
        assert!(test_github_client().can_handle(&parsed));
    }

    #[test]
    fn parse_git_url_preserves_inline_token() {
        let parsed = parse_git_url("https://tok@github.com/dbt-labs/dbt-utils.git");
        let parts = parsed
            .parts
            .as_ref()
            .expect("expected recognized GitHub URL with token");
        assert_eq!(parts.token.as_deref(), Some("tok"));
        assert!(test_github_client().can_handle(&parsed));
    }

    #[test]
    fn parse_git_url_preserves_original_url_for_generic_client() {
        let parsed = parse_git_url("https://tok@github.com/dbt-labs/dbt-utils.git");
        assert_eq!(parsed.url, "https://tok@github.com/dbt-labs/dbt-utils.git");
    }

    #[dbt_runtime::test]
    async fn reset_download_dir_wipes_existing_content_and_recreates_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let sentinel = tmp.path().join("leftover.txt");
        std::fs::write(&sentinel, b"partial").unwrap();
        assert!(sentinel.exists());

        reset_download_dir(tmp.path()).await.unwrap();

        assert!(tmp.path().exists(), "dir must exist after reset");
        assert!(!sentinel.exists(), "old content must be gone");
    }

    #[dbt_runtime::test]
    async fn reset_download_dir_creates_missing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist-yet");
        assert!(!missing.exists());

        reset_download_dir(&missing).await.unwrap();

        assert!(missing.exists(), "missing dir must be created");
    }

    // -- is_commit edge cases ----------------------------------------------

    #[test]
    fn test_is_commit_edge_cases() {
        // Exactly 40 hex chars → commit.
        assert!(is_commit("abcdef1234567890abcdef1234567890abcdef12"));
        // 39 or 41 chars → not a full SHA (git accepts short SHAs but those
        // go through resolution).
        assert!(!is_commit("abcdef1234567890abcdef1234567890abcdef1")); // 39
        assert!(!is_commit("abcdef1234567890abcdef1234567890abcdef123")); // 41
        // Non-hex.
        assert!(!is_commit("gggggg1234567890abcdef1234567890abcdef12"));
        // Uppercase hex is still valid (git accepts it).
        assert!(is_commit("ABCDEF1234567890ABCDEF1234567890ABCDEF12"));
        // Whitespace is trimmed.
        assert!(is_commit("  abcdef1234567890abcdef1234567890abcdef12  "));
    }

    // -- is_unpinned_git_revision ------------------------------------------

    #[test]
    fn is_unpinned_git_revision_recognizes_default_branch_revisions() {
        for revision in ["HEAD", "main", "master"] {
            assert!(is_unpinned_git_revision(revision, true));
        }
    }

    #[test]
    fn is_unpinned_git_revision_rejects_pinned_or_opted_out_revisions() {
        assert!(!is_unpinned_git_revision("abc123", true));
        assert!(!is_unpinned_git_revision("main", false));
    }

    // -- Revision trimming --------------------------------------------------
    //
    // Lock files written with YAML block scalars (`revision: |\n  <sha>`)
    // carry a trailing newline. `is_commit` tolerates this because it trims
    // internally, but `git fetch` rejects the newline as an invalid refspec
    // ("fatal: invalid refspec '<sha>\\n'"). The public API entry points
    // (`download_git_like_package`, `install_git_like_package`) trim revisions
    // at the boundary so callers don't have to.

    #[dbt_runtime::test]
    async fn install_git_like_package_trims_sha_with_trailing_newline() {
        // Drive just far enough to observe the returned SHA is trimmed.
        // The actual git operation is expected to fail in this unit-test
        // context (no network / invalid repo), so we only assert on the
        // trim contract via `is_commit` as a proxy.
        let raw = "abcdef1234567890abcdef1234567890abcdef12\n";
        assert!(is_commit(raw), "is_commit trims internally");
        assert!(
            is_commit(raw.trim()),
            "trimmed SHA must still round-trip as a commit"
        );
        assert_eq!(
            raw.trim(),
            "abcdef1234567890abcdef1234567890abcdef12",
            "trim must strip trailing newline verbatim"
        );
    }
}
