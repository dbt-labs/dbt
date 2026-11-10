//! Hashing a skill directory's contents.
//!
//! The hash over the source directory is recorded in the installed skill's
//! metadata for traceability. The variant that excludes `SKILL.md` lets the
//! installer tell an unchanged install (leave its bytes alone) from a drifted
//! one (reclaim it) without being fooled by the metadata dbt injects into the
//! installed `SKILL.md`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use dbt_common::{FsResult, stdfs};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

/// Both content digests of a skill directory, taken in a single walk.
///
/// The installer needs both at once: the full digest is recorded in metadata,
/// and the `SKILL.md`-excluding one tells an unchanged install from a drifted
/// one. Computing them together walks the source directory once instead of
/// twice per destination.
pub struct SkillDigests {
    /// Over every regular file, including the top-level `SKILL.md`.
    ///
    /// Recorded in the installed skill's metadata as `dbt.source_hash` for
    /// provenance — it says which source version an install came from. It is
    /// *not* consulted when deciding whether an install has drifted: the
    /// installed `SKILL.md` carries injected metadata, so its directory can
    /// never hash to the source's `full`. Drift is decided from
    /// [`Self::excluding_skill_md`] plus a byte comparison of the `SKILL.md`
    /// itself (see `install::install_one`). Treating `full` as authoritative
    /// drift state would be a bug.
    pub full: String,
    /// Over every regular file except the top-level `SKILL.md`.
    ///
    /// This is the drift signal for a skill's bundled files: it excludes the
    /// `SKILL.md` precisely so dbt's injected metadata cannot mask a real change.
    pub excluding_skill_md: String,
}

/// Compute both content digests of a skill directory in one walk.
///
/// Both relative paths and file bytes feed each hash, so a rename is a change.
/// Symlinks are not followed: only regular files contribute, matching what
/// `install::copy_skill` actually copies.
pub fn skill_digests(skill_dir: &Path) -> FsResult<SkillDigests> {
    let mut full = Sha256::new();
    let mut excluding = Sha256::new();
    for path in sorted_files(skill_dir) {
        let relative = relative_key(&path, skill_dir)?;
        let bytes = stdfs::read(&path)?;
        feed(&mut full, &relative, &bytes);
        if !is_top_level_skill_md(&path, skill_dir) {
            feed(&mut excluding, &relative, &bytes);
        }
    }
    Ok(SkillDigests {
        full: digest(full),
        excluding_skill_md: digest(excluding),
    })
}

/// Regular files under `skill_dir`, sorted so the hash is reproducible.
fn sorted_files(skill_dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = WalkDir::new(skill_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.path().to_path_buf())
        .collect();
    entries.sort();
    entries
}

/// Whether `path` is the skill's own top-level `SKILL.md`.
fn is_top_level_skill_md(path: &Path, skill_dir: &Path) -> bool {
    path.parent() == Some(skill_dir)
        && path.file_name() == Some(OsStr::new(crate::validate::SKILL_FILE))
}

/// `path` relative to `skill_dir`, with separators normalized so a hash taken
/// on Windows matches one on Unix.
fn relative_key(path: &Path, skill_dir: &Path) -> FsResult<String> {
    Ok(stdfs::diff_paths(path, skill_dir)?
        .to_string_lossy()
        .replace('\\', "/"))
}

/// Feed one file's relative path and bytes into a hasher, delimited so distinct
/// layouts cannot collide.
fn feed(hasher: &mut Sha256, relative: &str, bytes: &[u8]) {
    hasher.update(relative.as_bytes());
    hasher.update([0u8]);
    hasher.update(bytes);
    hasher.update([0u8]);
}

fn digest(hasher: Sha256) -> String {
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn full(dir: &Path) -> String {
        skill_digests(dir).unwrap().full
    }

    #[test]
    fn hashing_notices_content_and_renames() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("SKILL.md"), "hello").unwrap();
        let base = full(tmp.path());

        fs::write(tmp.path().join("SKILL.md"), "goodbye").unwrap();
        assert_ne!(full(tmp.path()), base);

        fs::write(tmp.path().join("SKILL.md"), "hello").unwrap();
        fs::write(tmp.path().join("extra.md"), "").unwrap();
        assert_ne!(full(tmp.path()), base);
    }

    #[test]
    fn hashing_covers_nested_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("SKILL.md"), "hello").unwrap();
        let before = full(tmp.path());

        fs::create_dir(tmp.path().join("scripts")).unwrap();
        fs::write(tmp.path().join("scripts/run.sh"), "echo hi").unwrap();
        assert_ne!(full(tmp.path()), before);
    }

    #[test]
    fn the_excluding_digest_ignores_only_the_top_level_skill_md() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("SKILL.md"), "hello").unwrap();
        fs::create_dir(tmp.path().join("scripts")).unwrap();
        fs::write(tmp.path().join("scripts/run.sh"), "echo hi").unwrap();

        let digests = skill_digests(tmp.path()).unwrap();
        assert_ne!(digests.full, digests.excluding_skill_md);

        // Rewriting the top-level SKILL.md moves `full` but not the excluding
        // digest; a bundled file moves both.
        let excluding_before = digests.excluding_skill_md.clone();
        fs::write(tmp.path().join("SKILL.md"), "changed").unwrap();
        let after = skill_digests(tmp.path()).unwrap();
        assert_ne!(after.full, digests.full);
        assert_eq!(after.excluding_skill_md, excluding_before);
    }
}
