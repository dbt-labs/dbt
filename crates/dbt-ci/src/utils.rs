use anyhow::{Result, bail};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use toml_edit::DocumentMut;

/// Lowercase hex sha256 of `data`; the one hashing form shared across the crate.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Rejects a download base url that isn't `https://`, so wheels are never
/// fetched over an insecure transport.
pub(crate) fn require_https(base_url: &str) -> Result<()> {
    if !base_url.starts_with("https://") {
        bail!("download base url must be an https URL, got {base_url:?}");
    }
    Ok(())
}

/// Retry only on errors that may heal: timeouts, connect failures, body blips.
/// `is_request()` (builder/config errors) is excluded — it won't change on retry.
pub(crate) fn is_transient(e: &reqwest::Error) -> bool {
    e.is_timeout() || e.is_connect() || e.is_body()
}

/// Exponential backoff between HTTP retries: 500ms, 1s, 2s, 4s, …
pub(crate) fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(500u64 * (1u64 << (attempt - 1)))
}

/// Nearest ancestor of `CARGO_MANIFEST_DIR` whose `Cargo.toml` has a
/// `[workspace]` table. Falls back to cwd (then `.`) if no ancestor matches.
pub(crate) fn cargo_workspace_root() -> PathBuf {
    let cwd_fallback = || env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let Some(start) = env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .ok()
        .or_else(|| env::current_dir().ok())
    else {
        return cwd_fallback();
    };
    let mut cur: &Path = start.as_path();
    loop {
        if has_workspace_table(cur) {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => return cwd_fallback(),
        }
    }
}

fn has_workspace_table(dir: &Path) -> bool {
    let path = dir.join("Cargo.toml");
    let Ok(text) = fs::read_to_string(&path) else {
        return false;
    };
    text.parse::<DocumentMut>()
        .map(|doc| doc.get("workspace").is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_table_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        assert!(has_workspace_table(dir.path()));
    }

    #[test]
    fn package_only_manifest_is_not_a_workspace() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        assert!(!has_workspace_table(dir.path()));
    }

    #[test]
    fn commented_workspace_line_is_not_detected() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "# [workspace] disabled while we split crates\n[package]\nname = \"x\"\n",
        )
        .unwrap();
        assert!(!has_workspace_table(dir.path()));
    }

    #[test]
    fn missing_manifest_is_not_a_workspace() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!has_workspace_table(dir.path()));
    }
}
