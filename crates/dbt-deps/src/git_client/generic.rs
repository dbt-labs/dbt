//! Generic git client — clone + ls-remote fallback.
//!
//! Works with any git host. Used as fallback when host-specific
//! fast paths fail or aren't available.

use std::path::Path;

use async_trait::async_trait;
use dbt_common::{ErrorCode, FsResult, fs_err, tokiofs};
use tokio::process::Command;

use super::ParsedGitUrl;
use super::traits::GitHostClient;
use super::{DownloadMethod, DownloadOutcome, ResolveMethod};

/// Generic git client — uses `git init + fetch + checkout` for any URL.
pub struct GenericClient;

#[async_trait]
impl GitHostClient for GenericClient {
    fn download_supports_revision(&self) -> bool {
        true
    }

    async fn resolve_ref(
        &self,
        parsed: &ParsedGitUrl,
        revision: &str,
    ) -> FsResult<(String, ResolveMethod)> {
        ls_remote_resolve(&parsed.url, revision)
            .await
            .map(|sha| (sha, ResolveMethod::GitLsRemote))
    }

    async fn download(
        &self,
        parsed: &ParsedGitUrl,
        revision: &str,
        target_dir: &Path,
        subdirectory: Option<&str>,
    ) -> FsResult<DownloadOutcome> {
        checkout_revision(&parsed.url, target_dir, revision, subdirectory).await
    }
}

/// Error classes from running `git` as a subprocess.
///
/// We preserve the distinction between "couldn't spawn/wait for the process"
/// (likely a host config issue) and "git ran but returned non-zero" (which
/// each caller classifies from stderr in its own way).
enum GitErr {
    Io(std::io::Error),
    Failed { stderr: String },
}

/// Run `git <args>` (optionally in `cwd`) and return stdout on success.
/// Non-zero exit is always an error (`GitErr::Failed`); mirrors dbt-common's
/// [`run_cmd`](https://github.com/dbt-labs/dbt-common/blob/main/dbt_common/clients/system.py)
/// (`CommandResultError` on `returncode != 0`), unlike legacy paths that dropped
/// `subprocess.CompletedProcess` without checking the code.
///
/// Callers that intentionally ignore failures (e.g. best-effort
/// `sparse-checkout`) must not use this helper.
///
/// `LC_ALL=C` is set so error-message matching in callers is stable.
async fn run_git(args: &[&str], cwd: Option<&Path>) -> Result<Vec<u8>, GitErr> {
    let mut cmd = Command::new("git");
    cmd.args(args).env("LC_ALL", "C");
    cmd.kill_on_drop(true);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd.output().await.map_err(GitErr::Io)?;
    if !output.status.success() {
        return Err(GitErr::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output.stdout)
}

/// Analyze git stderr and return a user-friendly error message.
fn parse_git_clone_error(stderr: &str, repo: &str, revision: Option<&str>) -> (ErrorCode, String) {
    let sanitized_repo = crate::utils::sanitize_git_url(repo);
    let ref_name = revision.unwrap_or("HEAD");

    if stderr.contains("couldn't find remote ref") || stderr.contains("did not match any") {
        return (
            ErrorCode::PackageDownloadFailed,
            format!(
                "Git ref '{}' not found in repository '{}'. Please verify the tag/branch/commit exists.",
                ref_name, sanitized_repo
            ),
        );
    }

    if stderr.contains("Authentication failed")
        || stderr.contains("could not read Username")
        || stderr.contains("Invalid username or password")
        || stderr.contains("terminal prompts disabled")
    {
        return (
            ErrorCode::AuthFailed,
            format!(
                "Authentication failed for repository '{}'. Please check your credentials or access token.",
                sanitized_repo
            ),
        );
    }

    if stderr.contains("Repository not found")
        || stderr.contains("does not appear to be a git repository")
        || stderr.contains("not found")
    {
        return (
            ErrorCode::PackageDownloadFailed,
            format!(
                "Repository '{}' not found. Please verify the repository URL and your access permissions.",
                sanitized_repo
            ),
        );
    }

    if stderr.contains("403")
        || stderr.contains("Permission denied")
        || stderr.contains("Access denied")
    {
        return (
            ErrorCode::PermissionDenied,
            format!(
                "Permission denied for repository '{}'. Please verify you have access to this repository.",
                sanitized_repo
            ),
        );
    }

    if stderr.contains("returned error: 500")
        || stderr.contains("The requested URL returned error: 500")
    {
        return (
            ErrorCode::PackageDownloadFailed,
            format!(
                "Server error (HTTP 500) for repository '{}'. This could indicate an authentication issue or a temporary server problem. \
                 Please verify your credentials and try again.",
                sanitized_repo
            ),
        );
    }

    // Sanitize stderr to avoid leaking credentials from git error messages
    let sanitized_stderr = crate::utils::sanitize_git_url(stderr.trim());
    (
        ErrorCode::PackageDownloadFailed,
        format!(
            "Git clone failed for '{}': {}",
            sanitized_repo, sanitized_stderr
        ),
    )
}

/// Check out `revision` from `repo` into `clone_dir`, optionally restricted
/// to `subdirectory`. `revision` may be any git refspec — full SHA, branch,
/// or tag — since `git fetch <repo> <refspec>` accepts all of them. When a
/// subdirectory is requested, the final layout is flattened so the subdir's
/// contents live directly at `clone_dir`.
///
/// Implemented as `git init + fetch --depth=1 <revision> + checkout` so we
/// never pull the full default branch.
async fn checkout_revision(
    repo: &str,
    clone_dir: &Path,
    revision: &str,
    subdirectory: Option<&str>,
) -> FsResult<DownloadOutcome> {
    let clone_dir_str = clone_dir.to_string_lossy();

    run_git(&["init", &clone_dir_str], None)
        .await
        .map_err(|e| match e {
            GitErr::Io(e) => fs_err!(ErrorCode::GitError, "Error initializing repo: {e}"),
            GitErr::Failed { stderr } => {
                fs_err!(ErrorCode::GitError, "git init failed: {stderr}")
            }
        })?;

    // Sparse-checkout setup must happen before fetch/checkout so only the
    // requested subtree is materialized. This is purely an optimization:
    // failures are intentionally swallowed (e.g. very old git without
    // `sparse-checkout`, or builds without the `--filter`-required server
    // capability). The subsequent fetch+checkout still produces correct
    // contents — just with the whole tree present — and the later flatten
    // step picks out the right subdir. This is the "no smart stuff"
    // fallback mentioned in pre-refactor TODOs.
    // Intentionally ignore `run_git` errors here only (see `run_git` doc).
    if let Some(subdir) = subdirectory {
        let _ = run_git(
            &["sparse-checkout", "set", "--no-cone", subdir],
            Some(clone_dir),
        )
        .await;
    }

    run_git(&["fetch", "--depth=1", repo, revision], Some(clone_dir))
        .await
        .map_err(|e| match e {
            GitErr::Io(e) => fs_err!(ErrorCode::GitError, "Error fetching: {e}"),
            GitErr::Failed { stderr } => {
                let (code, msg) = parse_git_clone_error(&stderr, repo, Some(revision));
                fs_err!(code, "{}", msg)
            }
        })?;

    run_git(&["checkout", "FETCH_HEAD"], Some(clone_dir))
        .await
        .map_err(|e| match e {
            GitErr::Io(e) => fs_err!(ErrorCode::GitError, "Error checking out: {e}"),
            GitErr::Failed { stderr } => fs_err!(
                ErrorCode::PackageDownloadFailed,
                "Failed to checkout: {}",
                crate::utils::sanitize_git_url(&stderr)
            ),
        })?;

    // Flatten: move `clone_dir/subdir` contents up to `clone_dir`.
    if let Some(subdir) = subdirectory {
        let subdir_path = clone_dir.join(subdir);
        let tmp = clone_dir.with_extension("subdir_tmp");
        tokiofs::rename(&subdir_path, &tmp).await?;
        tokiofs::remove_dir_all(clone_dir).await?;
        tokiofs::rename(&tmp, clone_dir).await?;
    }

    Ok(DownloadOutcome {
        checkout_path: clone_dir.to_path_buf(),
        download_method: DownloadMethod::Clone,
    })
}

/// Resolve a ref to its 40-char SHA via `git ls-remote`. No rate limit.
async fn ls_remote_resolve(repo_url: &str, revision: &str) -> FsResult<String> {
    let sanitized = crate::utils::sanitize_git_url(repo_url);
    let stdout = run_git(&["ls-remote", repo_url, revision], None)
        .await
        .map_err(|e| match e {
            GitErr::Io(e) => fs_err!(ErrorCode::GitError, "git ls-remote failed: {e}"),
            GitErr::Failed { stderr } => {
                let (code, msg) = parse_git_clone_error(&stderr, repo_url, Some(revision));
                fs_err!(code, "git ls-remote failed for {sanitized}: {msg}")
            }
        })?;

    let stdout = String::from_utf8_lossy(&stdout);
    if let Some(sha) = stdout.lines().next().and_then(|l| l.split('\t').next()) {
        let sha = sha.trim();
        if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(sha.to_string());
        }
    }

    Err(fs_err!(
        ErrorCode::PackageDownloadFailed,
        "Could not resolve ref '{revision}' via ls-remote"
    ))
}
