//! OS-appropriate resolution of a local, per-user cache directory.
//!
//! This is intentionally independent of the [`crate::lease`] module: it's a
//! general "where should dbt put a local cache file that isn't the flat
//! `~/.dbt` config directory" primitive, reused by anything that needs a
//! private per-user cache directory with tightened permissions (a lease
//! file, a credentials cache, ...). It is not lease-specific.
//!
//! Ported from gosnowflake's `secure_storage_manager.go` cache-directory
//! convention (`credCacheDirPath`), which resolves:
//! - Linux: an env-var override, then `$XDG_CACHE_HOME`, then `~/.cache`.
//! - macOS: `~/Library/Caches`.
//! - Windows: `%LocalAppData%`.
//!
//! Two deliberate deviations from the gosnowflake original:
//! - The vendor/product path segment is `dbt` instead of `Snowflake` (this
//!   crate has no Snowflake-specific knowledge -- see the crate-level docs).
//!   `dbt`'s existing flat config directory is `~/.dbt`
//!   (`dbt_common::constants::DBT_CONFIG_DIR`); this resolves a *different*,
//!   OS-appropriate cache directory rather than reusing that flat directory,
//!   matching the platform conventions above instead of hardcoding a `.dbt`
//!   style dotdir everywhere.
//! - Windows resolution goes through the `dirs` crate (already a workspace
//!   dependency) instead of gosnowflake's raw `FOLDERID_LocalAppData` FFI
//!   call, since `dirs::cache_dir()` already maps to the same location.
use std::io;
use std::path::{Path, PathBuf};

/// The vendor/product segment used in the resolved cache path (e.g.
/// `~/Library/Caches/dbt/Credentials` on macOS). Kept lowercase to match the
/// existing `.dbt` config-directory convention in this codebase, rather than
/// a capitalized display name -- this is a filesystem path segment, not
/// user-facing prose.
const VENDOR_DIR: &str = "dbt";

/// A subdirectory nested under the vendor directory on macOS and Windows so
/// that tighter permissions (`0700`) can be applied to just that
/// subdirectory, without restricting sibling cache data other tools might
/// place under the same vendor directory. Linux skips this: `~/.cache` is
/// already private to the user by convention, and `XDG_CACHE_HOME`-based
/// tooling doesn't generally nest a further permissions boundary here.
const CREDENTIALS_SUBDIR: &str = "credentials";

/// Overrides the resolved cache directory outright, when set to a path that
/// already exists as a directory. Primarily for tests and for environments
/// (e.g. containers) where the platform-default location isn't writable or
/// desired.
pub const CACHE_DIR_OVERRIDE_ENV: &str = "DBT_LEASE_CACHE_DIR";

/// Resolves (and creates, with restrictive permissions) the platform-
/// appropriate cache directory for dbt's local lease/cache files.
///
/// Restrictive permissions (`0700` on Unix) are applied to the leaf
/// directory, since it may end up holding sensitive cache data (e.g. a
/// credentials cache) even though this crate itself has no opinion on what's
/// stored there.
pub fn resolve_cache_dir() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(CACHE_DIR_OVERRIDE_ENV) {
        let dir = PathBuf::from(dir);
        if dir.is_dir() {
            return ensure_private_dir(&dir);
        }
    }

    let base = dirs::cache_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not determine a platform cache directory (no home directory?)",
        )
    })?;

    let dir = if cfg!(target_os = "linux") {
        base.join(VENDOR_DIR)
    } else {
        base.join(VENDOR_DIR).join(CREDENTIALS_SUBDIR)
    };

    ensure_private_dir(&dir)
}

/// Creates `dir` (and its parents) if missing, and restricts the leaf
/// directory to owner-only access on Unix (`0700`). Windows has no
/// equivalent bit-mode ACL concept here, so this is a no-op on Windows
/// beyond directory creation -- consistent with the gosnowflake original,
/// which also only applied Unix permission bits.
fn ensure_private_dir(dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }

    Ok(dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_cache_dir_honors_override_env() {
        let tmp = tempfile::tempdir().unwrap();
        let override_dir = tmp.path().join("override-cache");
        std::fs::create_dir_all(&override_dir).unwrap();

        // SAFETY: this test does not run concurrently with other tests that
        // read/write this specific env var; `cargo xtask test` runs each
        // test's process serially per-thread but env vars are process-wide,
        // so keep this test self-contained and restore the var afterward.
        let previous = std::env::var_os(CACHE_DIR_OVERRIDE_ENV);
        #[allow(clippy::disallowed_methods)]
        unsafe {
            std::env::set_var(CACHE_DIR_OVERRIDE_ENV, &override_dir);
        }

        let resolved = resolve_cache_dir().unwrap();

        #[allow(clippy::disallowed_methods)]
        unsafe {
            match &previous {
                Some(v) => std::env::set_var(CACHE_DIR_OVERRIDE_ENV, v),
                None => std::env::remove_var(CACHE_DIR_OVERRIDE_ENV),
            }
        }

        assert_eq!(resolved, override_dir);
    }

    #[test]
    fn ensure_private_dir_creates_and_restricts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nested").join("cache");

        let resolved = ensure_private_dir(&dir).unwrap();
        assert!(resolved.is_dir());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&resolved).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }
}
