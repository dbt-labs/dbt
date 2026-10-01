//! Pushes a rendered Homebrew formula to a tap repo.
//!
//! Strategy:
//!   1. Clone the tap into a temp dir. For HTTPS URLs, auth via
//!      `git -c http.extraHeader=Authorization: Basic <b64(x-access-token:TOKEN)>`
//!      — same pattern as actions/checkout. No token in the URL, so logs
//!      and `git remote -v` stay clean.
//!   2. Copy the rendered `.rb` into `Formula/`.
//!   3. If the working tree is clean (idempotent re-run with no changes),
//!      exit success without committing.
//!   4. Optionally set `user.{name,email}` via `--commit-author` /
//!      `--commit-email`; otherwise inherit whatever git config already
//!      has — so a local dev run uses the dev's identity.
//!   5. Commit + push (or just show the diff on `--dry-run`).
//!
//! `git` must be on PATH (it is, on every standard CI runner).

use crate::args::HomebrewPublishArgs;
use crate::release_version::validate_release_version;
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

pub fn execute(args: HomebrewPublishArgs) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(args: HomebrewPublishArgs) -> Result<()> {
    validate_release_version(&args.version)
        .with_context(|| format!("invalid --version {:?}", args.version))?;
    if !args.formula.is_file() {
        bail!("formula file not found: {}", args.formula.display());
    }
    let filename = args
        .formula
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("formula path has no filename: {}", args.formula.display()))?
        .to_string();
    if !filename.ends_with(".rb") {
        bail!("formula filename does not end in .rb: {filename}");
    }

    let token = read_token(&args.token_env, args.dry_run)?;
    // For HTTPS tap URLs, build an `http.extraHeader` config knob carrying
    // Basic auth credentials. Non-HTTPS URLs (file://, ssh) carry their
    // own auth.
    let auth_args = build_auth_args(&args.tap_repo, token.as_deref());

    let work = TempDir::new("dbt-ci-brew-publish")?;
    eprintln!("→ git clone {} -> {}", args.tap_repo, work.path().display());
    let mut clone_argv: Vec<OsString> = auth_args.clone();
    clone_argv.push("clone".into());
    if !is_local_path(&args.tap_repo) {
        clone_argv.push("--depth".into());
        clone_argv.push("1".into());
    }
    clone_argv.push("-b".into());
    clone_argv.push((&args.tap_branch).into());
    clone_argv.push((&args.tap_repo).into());
    clone_argv.push(work.path().into());
    run_git_os(None, &clone_argv)?;

    let formula_dir = work.path().join("Formula");
    fs::create_dir_all(&formula_dir)
        .with_context(|| format!("create {}", formula_dir.display()))?;
    let dest = formula_dir.join(&filename);
    fs::copy(&args.formula, &dest)
        .with_context(|| format!("copy {} -> {}", args.formula.display(), dest.display()))?;

    let status = run_git_capture(Some(work.path()), &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        eprintln!("✓ {} already up to date in tap", filename);
        return Ok(());
    }

    // Only override identity if BOTH flags are provided. Otherwise inherit
    // whatever git config the cloned tap has — which on a dev machine comes
    // from `~/.gitconfig`.
    match (&args.commit_author, &args.commit_email) {
        (Some(name), Some(email)) => {
            run_git(Some(work.path()), &["config", "user.name", name])?;
            run_git(Some(work.path()), &["config", "user.email", email])?;
        }
        (None, None) => {} // inherit
        _ => bail!("--commit-author and --commit-email must be set together"),
    }

    run_git(Some(work.path()), &["add", &format!("Formula/{filename}")])?;
    let message = format!(
        "{stem} {version}",
        stem = filename.trim_end_matches(".rb"),
        version = args.version,
    );
    run_git(Some(work.path()), &["commit", "-m", &message])?;

    if args.dry_run {
        eprintln!("→ dry-run: skipping push. Patch follows:\n");
        run_git(Some(work.path()), &["--no-pager", "show", "HEAD"])?;
        return Ok(());
    }

    // Push needs the same `-c http.extraHeader=…` knobs as clone.
    let mut push_argv: Vec<OsString> = auth_args;
    push_argv.push("push".into());
    push_argv.push("origin".into());
    push_argv.push((&args.tap_branch).into());
    run_git_os(Some(work.path()), &push_argv)?;
    eprintln!("✓ pushed {filename} to {}", args.tap_repo);
    Ok(())
}

fn read_token(env_name: &str, dry_run: bool) -> Result<Option<String>> {
    let v = env::var(env_name).ok().filter(|v| !v.is_empty());
    if v.is_none() && !dry_run {
        bail!("env var `{env_name}` is not set (required unless --dry-run)");
    }
    Ok(v)
}

/// Builds `git -c http.extraHeader=Authorization: Basic <b64>` argv prefix
/// for HTTPS URLs. Returns an empty Vec for non-HTTPS URLs (file://, ssh) or
/// when no token is provided. The header is set via `-c` so it never enters
/// the URL — `git remote -v` and clone logs stay clean.
///
/// GitHub's git HTTP backend accepts Basic auth, not Bearer. The
/// `x-access-token` username is the convention `actions/checkout` uses
/// internally and works for both user PATs and GitHub App tokens.
fn build_auth_args(tap_url: &str, token: Option<&str>) -> Vec<OsString> {
    let Some(token) = token else {
        return Vec::new();
    };
    if !tap_url.starts_with("https://") {
        return Vec::new();
    }
    let creds = BASE64.encode(format!("x-access-token:{token}"));
    vec![
        "-c".into(),
        format!("http.extraHeader=Authorization: Basic {creds}").into(),
    ]
}

/// True for `file://` URLs and absolute or relative filesystem paths. Used to
/// skip `git clone --depth` (which git ignores for local clones, with a
/// warning that just clutters logs).
fn is_local_path(url: &str) -> bool {
    url.starts_with("file://")
        || url.starts_with('/')
        || url.starts_with("./")
        || url.starts_with("../")
}

fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<()> {
    let argv: Vec<OsString> = args.iter().map(|s| (*s).into()).collect();
    run_git_os(cwd, &argv)
}

fn run_git_os(cwd: Option<&Path>, args: &[OsString]) -> Result<()> {
    let mut cmd = Command::new("git");
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    cmd.args(args);
    let status = cmd
        .status()
        .with_context(|| format!("spawn `git {}`", display_argv(args)))?;
    if !status.success() {
        bail!(
            "`git {}` exited with status {}",
            display_argv(args),
            status.code().unwrap_or(-1),
        );
    }
    Ok(())
}

fn run_git_capture(cwd: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    cmd.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let out = cmd
        .output()
        .with_context(|| format!("spawn `git {}`", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "`git {}` exited with status {}",
            args.join(" "),
            out.status.code().unwrap_or(-1),
        );
    }
    String::from_utf8(out.stdout).context("`git status` produced non-UTF8 output")
}

/// Lossy display for logging — skips any `extraHeader` args so tokens never
/// land in error messages.
fn display_argv(args: &[OsString]) -> String {
    let mut out = String::new();
    let mut skip_next = false;
    for a in args {
        if skip_next {
            out.push_str("***");
            skip_next = false;
            out.push(' ');
            continue;
        }
        let s = a.to_string_lossy();
        if s == "-c" {
            out.push_str("-c ");
            skip_next = true;
            continue;
        }
        out.push_str(&s);
        out.push(' ');
    }
    out.trim_end().to_string()
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(stem: &str) -> Result<Self> {
        let base = env::temp_dir();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = base.join(format!("{stem}-{pid}-{nanos}"));
        fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
        Ok(Self { path })
    }
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_auth_args_emits_basic_auth_header_for_https() {
        let got = build_auth_args("https://github.com/dbt-labs/homebrew-dbt.git", Some("abc"));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], OsString::from("-c"));
        // base64("x-access-token:abc") = "eC1hY2Nlc3MtdG9rZW46YWJj"
        assert_eq!(
            got[1],
            OsString::from("http.extraHeader=Authorization: Basic eC1hY2Nlc3MtdG9rZW46YWJj")
        );
    }

    #[test]
    fn build_auth_args_empty_for_local_path() {
        assert!(build_auth_args("file:///tmp/tap.git", Some("abc")).is_empty());
        assert!(build_auth_args("/tmp/tap.git", Some("abc")).is_empty());
        assert!(build_auth_args("git@github.com:x/y.git", Some("abc")).is_empty());
    }

    #[test]
    fn build_auth_args_empty_when_no_token() {
        assert!(build_auth_args("https://github.com/x/y.git", None).is_empty());
    }

    #[test]
    fn is_local_path_recognizes_filesystem_and_file_urls() {
        assert!(is_local_path("file:///tmp/tap.git"));
        assert!(is_local_path("/tmp/tap.git"));
        assert!(is_local_path("./tap.git"));
        assert!(is_local_path("../tap.git"));
        assert!(!is_local_path("https://github.com/x/y.git"));
        assert!(!is_local_path("git@github.com:x/y.git"));
    }

    #[test]
    fn display_argv_redacts_value_after_dash_c() {
        let argv = vec![
            OsString::from("-c"),
            OsString::from("http.extraHeader=Authorization: Bearer secret"),
            OsString::from("clone"),
            OsString::from("https://x/y.git"),
        ];
        let got = display_argv(&argv);
        assert!(!got.contains("secret"), "got: {got}");
        assert!(got.contains("***"));
        assert!(got.contains("clone"));
        assert!(got.contains("https://x/y.git"));
    }

    #[test]
    fn read_token_returns_none_when_missing_in_dry_run() {
        // Unique env var so we don't collide with other tests
        let key = "DBT_CI_TEST_TOKEN_NOT_SET_XYZ_777";
        unsafe { env::remove_var(key) };
        assert_eq!(read_token(key, true).unwrap(), None);
    }

    #[test]
    fn read_token_errors_when_missing_and_not_dry_run() {
        let key = "DBT_CI_TEST_TOKEN_NOT_SET_XYZ_888";
        unsafe { env::remove_var(key) };
        let err = read_token(key, false).unwrap_err().to_string();
        assert!(err.contains("is not set"), "got: {err}");
    }
}
