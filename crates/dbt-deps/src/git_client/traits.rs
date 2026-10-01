//! Trait for git host clients.

use std::path::Path;

use async_trait::async_trait;
use dbt_common::FsResult;

use super::{
    DownloadOutcome, InstallOutcome, ParsedGitUrl, ResolveMethod, ResolveOutcome, is_commit,
};

/// Host-specific git package resolution and download.
///
/// Implementations should override the **atoms** — [`resolve_ref`](GitHostClient::resolve_ref) and
/// [`download`](GitHostClient::download) — and follow each method's documentation for inputs and
/// error semantics. Override [`can_handle`](GitHostClient::can_handle) and
/// [`download_supports_revision`](GitHostClient::download_supports_revision) when the defaults do not
/// match the transport. Default [`resolve`](GitHostClient::resolve) and [`install`](GitHostClient::install)
/// compose those hooks.
#[async_trait]
pub trait GitHostClient: Send + Sync {
    /// Primary fast-path gate for `parsed` (`get_git_client`).
    fn can_handle(&self, _parsed: &ParsedGitUrl) -> bool {
        false
    }

    /// When `true`, [`download`](GitHostClient::download) accepts the same
    /// non-SHA revision strings as [`resolve_ref`](GitHostClient::resolve_ref)
    /// (e.g. branch or tag names), so the default [`resolve`](GitHostClient::resolve)
    /// may fetch content in parallel with ref resolution. When `false`, only a
    /// full 40-character commit hash is valid for `download`; the default `resolve`
    /// resolves first, then downloads by SHA.
    ///
    /// [`install`](GitHostClient::install) always passes a SHA; this flag only
    /// affects the resolve-phase path for non-SHA `packages.yml` revisions.
    fn download_supports_revision(&self) -> bool {
        false
    }

    /// Resolve a ref name to its full SHA. Only called for non-SHA revisions.
    async fn resolve_ref(
        &self,
        parsed: &ParsedGitUrl,
        revision: &str,
    ) -> FsResult<(String, ResolveMethod)>;

    /// Download package content at the given `revision`.
    ///
    /// Must accept a full 40-character commit hash. When
    /// [`download_supports_revision`](GitHostClient::download_supports_revision) is
    /// `true`, may also accept ref names passed from the default `resolve` path.
    /// If `subdirectory` is `Some`, only that subtree is extracted into `target_dir`.
    async fn download(
        &self,
        parsed: &ParsedGitUrl,
        revision: &str,
        target_dir: &Path,
        subdirectory: Option<&str>,
    ) -> FsResult<DownloadOutcome>;

    /// Resolve phase: build a lock entry by resolving the user's ref and
    /// downloading the content. Full SHAs skip `resolve_ref`. For other
    /// revisions, uses parallel or sequential download depending on
    /// [`download_supports_revision`](GitHostClient::download_supports_revision).
    async fn resolve(
        &self,
        parsed: &ParsedGitUrl,
        revision: &str,
        target_dir: &Path,
        subdirectory: Option<&str>,
    ) -> FsResult<ResolveOutcome> {
        if is_commit(revision) {
            let download = self
                .download(parsed, revision, target_dir, subdirectory)
                .await?;
            return Ok(ResolveOutcome {
                download,
                sha: revision.trim().to_string(),
                resolve_method: ResolveMethod::Identity,
            });
        }
        if self.download_supports_revision() {
            let (download_result, resolve_result) = tokio::join!(
                self.download(parsed, revision, target_dir, subdirectory),
                self.resolve_ref(parsed, revision),
            );
            let download = download_result?;
            let (sha, resolve_method) = resolve_result?;
            Ok(ResolveOutcome {
                download,
                sha,
                resolve_method,
            })
        } else {
            let (sha, resolve_method) = self.resolve_ref(parsed, revision).await?;
            let download = self
                .download(parsed, &sha, target_dir, subdirectory)
                .await?;
            Ok(ResolveOutcome {
                download,
                sha,
                resolve_method,
            })
        }
    }

    /// Install phase: download at the SHA recorded in the lock file. No
    /// ref resolution since the lock pins to an exact commit.
    async fn install(
        &self,
        parsed: &ParsedGitUrl,
        sha: &str,
        target_dir: &Path,
        subdirectory: Option<&str>,
    ) -> FsResult<InstallOutcome> {
        let download = self.download(parsed, sha, target_dir, subdirectory).await?;
        Ok(InstallOutcome { download })
    }
}
