use std::path::Path;
use std::time::SystemTime;
use tokio;

use crate::error::LiftableResult;
use crate::{FsResult, ectx};

/// Wrapper around [`tokio::fs::create_dir_all`] that returns a useful error in case of failure.
pub async fn create_dir_all(path: impl AsRef<Path>) -> FsResult<()> {
    let path = path.as_ref();
    tokio::fs::create_dir_all(path)
        .await
        .lift(ectx!("Failed to create directory: {}", path.display()))
}

/// Wrapper around [`tokio::fs::remove_dir_all`] that returns a useful error in case of failure.
pub async fn remove_dir_all(path: impl AsRef<Path>) -> FsResult<()> {
    let path = path.as_ref();
    tokio::fs::remove_dir_all(path)
        .await
        .lift(ectx!("Failed to delete directory: {}", path.display()))
}

/// Wrapper around [`tokio::fs::read_to_string`] that returns a useful error in case of failure.
pub async fn read_to_string<P: AsRef<Path>>(path: P) -> FsResult<String> {
    let path = path.as_ref();
    tokio::fs::read_to_string(path)
        .await
        .lift(ectx!("Failed to read file: {}", path.display()))
}

/// Wrapper around [`tokio::fs::read`] that returns a useful error in case of failure.
pub async fn read(path: impl AsRef<Path>) -> FsResult<Vec<u8>> {
    let path = path.as_ref();
    tokio::fs::read(path)
        .await
        .lift(ectx!("Failed to read file: {}", path.display()))
}

/// Wrapper around [`tokio::fs::write`] that returns a useful error in case of failure.
pub async fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> FsResult<()> {
    let path = path.as_ref();
    tokio::fs::write(path, contents)
        .await
        .lift(ectx!("Failed to write file: {}", path.display()))
}

/// Wrapper around [`tokio::fs::copy`] that returns a useful error in case of failure.
pub async fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> FsResult<u64> {
    let from = from.as_ref();
    let to = to.as_ref();
    tokio::fs::copy(from, to).await.lift(ectx!(
        "Failed to copy file {} to {}",
        from.display(),
        to.display()
    ))
}

/// Wrapper around [`tokio::fs::metadata`] + [`Metadata::modified`] that returns a useful error in case of failure.
pub async fn last_modified<P: AsRef<Path>>(path: P) -> FsResult<SystemTime> {
    let path = path.as_ref();
    tokio::fs::metadata(path)
        .await
        .and_then(|metadata| metadata.modified())
        .lift(ectx!(
            "Failed to get last modified time of: {}",
            path.display()
        ))
}

/// Wrapper around [`tokio::fs::metadata`] that returns a useful error in case of failure.
pub async fn metadata(path: impl AsRef<Path>) -> FsResult<std::fs::Metadata> {
    let path = path.as_ref();
    tokio::fs::metadata(path)
        .await
        .lift(ectx!("Failed to get metadata for: {}", path.display()))
}

/// Check if a path exists (follows symlinks). Returns false on any error.
pub async fn path_exists(path: impl AsRef<Path>) -> bool {
    tokio::fs::metadata(path.as_ref()).await.is_ok()
}

/// Check if a path exists, returning false on permission errors rather than panicking.
pub async fn try_exists(path: impl AsRef<Path>) -> bool {
    tokio::fs::try_exists(path.as_ref()).await.unwrap_or(false)
}

/// Wrapper around [`tokio::fs::remove_file`] that returns a useful error in case of failure.
pub async fn remove_file(path: impl AsRef<Path>) -> FsResult<()> {
    let path = path.as_ref();
    tokio::fs::remove_file(path)
        .await
        .lift(ectx!("Failed to remove file: {}", path.display()))
}

/// Wrapper around [`tokio::fs::read_link`] that returns a useful error in case of failure.
pub async fn read_link(path: impl AsRef<Path>) -> FsResult<std::path::PathBuf> {
    let path = path.as_ref();
    tokio::fs::read_link(path)
        .await
        .lift(ectx!("Failed to read symlink: {}", path.display()))
}

/// Wrapper around [`tokio::fs::symlink`] (Unix) or [`tokio::fs::symlink_dir`] (Windows).
pub async fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) -> FsResult<()> {
    let target = target.as_ref();
    let link = link.as_ref();
    #[cfg(unix)]
    {
        tokio::fs::symlink(target, link).await.lift(ectx!(
            "Failed to create symlink from {} to {}",
            link.display(),
            target.display()
        ))
    }
    #[cfg(windows)]
    {
        tokio::fs::symlink_dir(target, link).await.lift(ectx!(
            "Failed to create symlink from {} to {}",
            link.display(),
            target.display()
        ))
    }
}

/// Wrapper around [`tokio::fs::rename`] that returns a useful error in case of failure.
pub async fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> FsResult<()> {
    let from = from.as_ref();
    let to = to.as_ref();
    tokio::fs::rename(from, to).await.lift(ectx!(
        "Failed to rename file {} to {}",
        from.display(),
        to.display()
    ))
}

/// Wrapper around [`tokio::fs::read_dir`] that returns a useful error in case of failure.
pub async fn read_dir(path: impl AsRef<Path>) -> FsResult<tokio::fs::ReadDir> {
    let path = path.as_ref();
    tokio::fs::read_dir(path)
        .await
        .lift(ectx!("Failed to read directory: {}", path.display()))
}

pub struct File {}
impl File {
    /// Wrapper around [`tokio::fs::File::create`] that returns a useful error in case of failure.
    pub async fn create<P: AsRef<Path>>(path: P) -> FsResult<tokio::fs::File> {
        let path = path.as_ref();
        tokio::fs::File::create(path)
            .await
            .lift(ectx!("Failed to create file: {}", path.display()))
    }
}
