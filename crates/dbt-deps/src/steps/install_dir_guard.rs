//! Ownership check for the packages install dir.
//!
//! `dbt deps` empties the install dir before reinstalling. To avoid emptying a directory dbt
//! did not create, the install dir is marked with a `CACHEDIR.TAG` (https://bford.info/cachedir/)
//! and a non-empty dir without a valid tag is not considered owned by dbt.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

const CACHEDIR_TAG_FILE_NAME: &str = "CACHEDIR.TAG";
const CACHEDIR_TAG_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";
const CACHEDIR_TAG_CONTENTS: &str = "Signature: 8a477f597d28d172789f06886806bc55\n\
# This file is a cache directory tag created by dbt.\n\
# For information about cache directory tags see https://bford.info/cachedir/\n";

/// Returns the reason why `dir` is not owned by dbt, or `None` if it is safe to empty.
///
/// A missing or empty dir is safe; any other dir needs a valid `CACHEDIR.TAG`.
pub(super) fn unowned_dir_reason(dir: &Path) -> Option<String> {
    let is_empty = match fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_none(),
        // Missing or not a directory: nothing for us to guard.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => return Some(format!("failed to read `{}`: {e}", dir.display())),
    };
    if is_empty {
        return None;
    }

    let tag_path = dir.join(CACHEDIR_TAG_FILE_NAME);
    // The tag must not be a symlink, per https://bford.info/cachedir/
    if tag_path.is_symlink() {
        return Some(format!(
            "expected `{CACHEDIR_TAG_FILE_NAME}` to be a regular file, got a symlink"
        ));
    }
    if !tag_path.is_file() {
        return Some(format!("missing `{CACHEDIR_TAG_FILE_NAME}` file"));
    }

    let mut buf = [0u8; CACHEDIR_TAG_SIGNATURE.len()];
    match fs::File::open(&tag_path).and_then(|mut f| f.read_exact(&mut buf)) {
        Ok(()) if buf == CACHEDIR_TAG_SIGNATURE => None,
        Ok(()) => Some(format!(
            "invalid signature in `{CACHEDIR_TAG_FILE_NAME}` file"
        )),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Some(format!(
            "invalid signature in `{CACHEDIR_TAG_FILE_NAME}` file"
        )),
        Err(e) => Some(format!("failed to read `{}`: {e}", tag_path.display())),
    }
}

/// Marks `dir` as created by dbt.
pub(super) fn write_cachedir_tag(dir: &Path) -> io::Result<()> {
    fs::write(dir.join(CACHEDIR_TAG_FILE_NAME), CACHEDIR_TAG_CONTENTS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_dir_is_safe() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(unowned_dir_reason(&tmp.path().join("nope")).is_none());
    }

    #[test]
    fn empty_dir_is_safe() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(unowned_dir_reason(tmp.path()).is_none());
    }

    #[test]
    fn tagged_dir_is_safe() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("pkg"), "x").unwrap();
        write_cachedir_tag(tmp.path()).unwrap();
        assert!(unowned_dir_reason(tmp.path()).is_none());
    }

    #[test]
    fn untagged_non_empty_dir_is_unowned() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("pkg"), "x").unwrap();
        let reason = unowned_dir_reason(tmp.path()).unwrap();
        assert!(reason.contains("missing"), "{reason}");
    }

    #[test]
    fn wrong_or_short_signature_is_unowned() {
        for contents in [
            "Signature: 1234",
            "Signature: 8a477f597d28d172789f06886806bc5",
        ] {
            let tmp = tempfile::tempdir().unwrap();
            fs::write(tmp.path().join(CACHEDIR_TAG_FILE_NAME), contents).unwrap();
            let reason = unowned_dir_reason(tmp.path()).unwrap();
            assert!(reason.contains("invalid signature"), "{reason}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_tag_is_unowned() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real.tag");
        fs::write(&real, CACHEDIR_TAG_CONTENTS).unwrap();
        let dir = tmp.path().join("dir");
        fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink(&real, dir.join(CACHEDIR_TAG_FILE_NAME)).unwrap();
        let reason = unowned_dir_reason(&dir).unwrap();
        assert!(reason.contains("symlink"), "{reason}");
    }
}
