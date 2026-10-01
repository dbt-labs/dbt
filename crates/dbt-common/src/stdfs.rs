use crate::{ErrorCode, FsResult, ectx, fs_err};

use crate::error::LiftableResult;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Wrapper around [`std::fs::canonicalize`] that returns a useful error in case of failure.
pub fn canonicalize<P: AsRef<Path>>(path: P) -> FsResult<PathBuf> {
    let path = path.as_ref();
    {
        #[cfg(not(target_os = "windows"))]
        {
            // Only place in our codebase where std::fs::canonicalize is allowed:
            #[allow(clippy::disallowed_methods)]
            std::fs::canonicalize(path)
        }
        #[cfg(target_os = "windows")]
        {
            dunce::canonicalize(path)
        }
    }
    .lift(ectx!("Failed to canonicalize path: {}", path.display()))
}

/// Wrapper around [`std::fs::create_dir_all`] that returns a useful error in case of failure.
pub fn create_dir_all<P: AsRef<Path>>(path: P) -> FsResult<()> {
    let path = path.as_ref();
    std::fs::create_dir_all(path).lift(ectx!("Failed to create directory: {}", path.display()))
}

/// Wrapper around [`std::fs::remove_dir_all`] that returns a useful error in case of failure.
pub fn remove_dir_all<P: AsRef<Path>>(path: P) -> FsResult<()> {
    let path = path.as_ref();
    std::fs::remove_dir_all(path).lift(ectx!("Failed to delete directory: {}", path.display()))
}

/// Wrapper around [`std::fs::read_to_string`] that returns a useful error in case of failure.
pub fn read_to_string<P: AsRef<Path>>(path: P) -> FsResult<String> {
    let path = path.as_ref();
    std::fs::read_to_string(path).lift(ectx!("Failed to read file: {}", path.display()))
}

/// Wrapper around [`std::fs::exists`] that returns a useful error in case of failure.
pub fn exists<P: AsRef<Path>>(path: P) -> FsResult<bool> {
    let path = path.as_ref();
    std::fs::exists(path).lift(ectx!(
        "Failed to check if file/dir exists: {}",
        path.display()
    ))
}

/// Wrapper around [`std::fs::write`] that returns a useful error in case of failure.
pub fn write<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, contents: C) -> FsResult<()> {
    let path = path.as_ref();
    std::fs::write(path, contents).lift(ectx!("Failed to write file: {}", path.display()))
}

/// Wrapper around [`std::fs::metadata`] that returns a useful error in case of failure.
pub fn metadata<P: AsRef<Path>>(path: P) -> FsResult<Metadata> {
    let path = path.as_ref();
    std::fs::metadata(path).lift(ectx!("Failed to get metadata for: {}", path.display()))
}

/// Wrapper around [`std::fs::remove_file`] that returns a useful error in case of failure.
pub fn remove_file<P: AsRef<Path>>(path: P) -> FsResult<()> {
    let path = path.as_ref();
    std::fs::remove_file(path).lift(ectx!("Failed to remove file: {}", path.display()))
}

/// Wrapper around [`std::fs::copy`] that returns a useful error in case of failure.
pub fn copy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> FsResult<u64> {
    let from = from.as_ref();
    let to = to.as_ref();
    std::fs::copy(from, to).lift(ectx!(
        "Failed to copy file {} to {}",
        from.display(),
        to.display()
    ))
}

/// Wrapper around [`std::fs::rename`] that returns a useful error in case of failure.
pub fn move_file<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> FsResult<()> {
    let from = from.as_ref();
    let to = to.as_ref();
    std::fs::rename(from, to).lift(ectx!(
        "Failed to move file {} to {}",
        from.display(),
        to.display()
    ))
}

/// Wrapper around [`std::fs::metadata`] + [`Metadata::modified`] that returns a useful error in case of failure.
pub fn last_modified<P: AsRef<Path>>(path: P) -> FsResult<SystemTime> {
    let path = path.as_ref();
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .lift(ectx!(
            "Failed to get last modified time of: {}",
            path.display()
        ))
}

/// Wrapper around [`std::fs::read`] that returns a useful error in case of failure.
pub fn read<P: AsRef<Path>>(path: P) -> FsResult<Vec<u8>> {
    let path = path.as_ref();
    std::fs::read(path).lift(ectx!("Failed to read file: {}", path.display()))
}

/// Wrapper around [`std::fs::read_dir`] that returns a useful error in case of failure.
pub fn read_dir<P: AsRef<Path>>(path: P) -> FsResult<std::fs::ReadDir> {
    let path = path.as_ref();
    std::fs::read_dir(path).lift(ectx!("Failed to read directory: {}", path.display()))
}

/// Wrapper around [`std::fs::rename`] that returns a useful error in case of failure.
pub fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> FsResult<()> {
    let from = from.as_ref();
    let to = to.as_ref();
    std::fs::rename(from, to).lift(ectx!(
        "Failed to move file {} to {}",
        from.display(),
        to.display()
    ))
}

/// Wrapper around [`pathdiff::diff_paths`] that returns a useful error in case of failure.
pub fn diff_paths<P: AsRef<Path>, Q: AsRef<Path>>(to: P, from: Q) -> FsResult<PathBuf> {
    let to = to.as_ref();
    let from = from.as_ref();
    // First try with the paths as provided
    pathdiff::diff_paths(to, from)
        // If that fails, try to canonicalize both paths and try again
        .or_else(|| {
            let to_canon = canonicalize(to).ok()?;
            let from_canon = canonicalize(from).ok()?;
            pathdiff::diff_paths(&to_canon, &from_canon)
        })
        .ok_or_else(|| {
            fs_err!(
                ErrorCode::InvalidArgument,
                "Failed to diff paths {} and {}",
                from.display(),
                to.display()
            )
        })
}

/// Wrapper around [`std::fs::symlink`] that returns a useful error in case of failure.
pub fn symlink<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> FsResult<()> {
    let from = from.as_ref();
    let to = to.as_ref();
    #[cfg(not(target_os = "windows"))]
    {
        std::os::unix::fs::symlink(from, to).lift(ectx!(
            "Failed to create symlink from {} to {}",
            from.display(),
            to.display()
        ))
    }
    #[cfg(target_os = "windows")]
    {
        std::os::windows::fs::symlink_dir(from, to).lift(ectx!(
            "Failed to create symlink from {} to {}",
            from.display(),
            to.display()
        ))
    }
}

pub struct File {}
impl File {
    /// Wrapper around [`std::fs::File::open`] that returns a useful error in case of failure.
    pub fn open<P: AsRef<Path>>(path: P) -> FsResult<std::fs::File> {
        let path = path.as_ref();
        std::fs::File::open(path).lift(ectx!("Failed to open file: {}", path.display()))
    }

    /// Wrapper around [`std::fs::File::create`] that returns a useful error in case of failure.
    pub fn create<P: AsRef<Path>>(path: P) -> FsResult<std::fs::File> {
        let path = path.as_ref();
        std::fs::File::create(path).lift(ectx!("Failed to create file: {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::create_dir_all;
    #[test]
    fn test_diff_paths_error_case() {
        let target_path = PathBuf::from("./stdfs_test_dir");
        create_dir_all(&target_path).unwrap();
        let curr_dir = std::env::current_dir().unwrap();
        // The correct order is: to, from (destination, base)
        let result = diff_paths(&target_path, &curr_dir);
        remove_dir_all(&target_path).unwrap();
        assert!(result.is_ok());
    }
}
