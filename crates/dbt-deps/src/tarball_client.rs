//! Tarball download and extraction client.
//!
//! Downloads tarballs (`.tar.gz`) from URLs and extracts them directly to disk.
//! Uses async streaming extraction (no intermediate files or memory buffering)
//! and automatic retry logic for transient failures.
//! Supports selective extraction with root directory stripping and subdirectory filtering.
//! A supplied sha1 is verified against the buffered body before extraction.

use crate::utils::ensure_dir;
use async_compression::tokio::bufread::GzipDecoder;
use dbt_common::cancellation::CancellationToken;
use dbt_common::tracing::dbt_emit::emit_info_log_message;
use dbt_common::{ErrorCode, FsResult, err, fs_err};
use futures::StreamExt;
use reqwest_middleware::ClientWithMiddleware;
use sha1::Digest;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use tokio::io::AsyncBufRead;
use tokio_tar::{Archive, EntryType};
use tokio_util::io::StreamReader;

/// Client for downloading and extracting tarball archives.
#[derive(Clone)]
pub struct TarballClient {
    pub client: ClientWithMiddleware,
    cancellation: CancellationToken,
}

impl TarballClient {
    pub fn from_client(client: ClientWithMiddleware, cancellation: CancellationToken) -> Self {
        Self {
            client,
            cancellation,
        }
    }

    /// Download tarball from URL and extract to target directory with optional filtering.
    ///
    /// # Arguments
    /// * `download_url` - URL of the tarball to download
    /// * `target_path` - Directory to extract contents into. **Must already exist
    ///   and be writable; lifecycle (creation and cleanup on error) is the
    ///   caller's responsibility.**
    /// * `strip_root` - If true, strip the single root directory from archive
    /// * `subdirectory` - If provided, only extract entries from this subdirectory
    /// * `headers` - Additional HTTP request headers (e.g. `Authorization`);
    ///   pass `&[]` when none are needed
    /// * `expected_sha1` - If provided, the body is buffered and verified
    ///   before extraction; on mismatch nothing is extracted
    ///
    /// Streams download directly from network through gzip decoder to tar extractor,
    /// avoiding intermediate memory buffering or file I/O, unless `expected_sha1`
    /// is provided.
    pub async fn download_and_extract_tarball(
        &self,
        download_url: &str,
        target_path: &Path,
        strip_root: bool,
        subdirectory: Option<&str>,
        headers: &[(&str, &str)],
        expected_sha1: Option<&str>,
    ) -> FsResult<PathBuf> {
        self.cancellation.check_cancellation()?;

        let mut req = self.client.get(download_url);
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        let res = req.send().await.map_err(|e| {
            fs_err!(
                ErrorCode::RuntimeError,
                "Failed to get tarball from {download_url}: {e:#}"
            )
        })?;

        if !res.status().is_success() {
            return err!(
                ErrorCode::RuntimeError,
                "Failed to download tarball from {download_url}; status: {}",
                res.status()
            );
        }

        let reader: Pin<Box<dyn AsyncBufRead + Send>> = if let Some(expected_sha1) = expected_sha1 {
            let body = res.bytes().await.map_err(|e| {
                fs_err!(
                    ErrorCode::RuntimeError,
                    "Failed to read tarball body from {download_url}: {e}"
                )
            })?;
            let actual_sha1 = format!("{:x}", sha1::Sha1::digest(&body));
            if !actual_sha1.eq_ignore_ascii_case(expected_sha1) {
                return err!(
                    ErrorCode::RuntimeError,
                    "sha1 checksum mismatch for tarball downloaded from {download_url}: expected {expected_sha1}, got {actual_sha1}"
                );
            }
            Box::pin(io::Cursor::new(body))
        } else {
            // Convert reqwest stream to AsyncRead
            Box::pin(StreamReader::new(res.bytes_stream().map(|result| {
                result.map_err(|e| io::Error::other(format!("Failed to read stream: {}", e)))
            })))
        };

        let decoder = GzipDecoder::new(reader);
        let archive = Archive::new(decoder);

        self.extract_archive(archive, download_url, target_path, strip_root, subdirectory)
            .await
    }

    /// Extract an already-opened tar archive into `target_path`.
    ///
    /// Split out so the extraction logic can be tested against in-memory
    /// archives. `download_url` is used for error messages only.
    async fn extract_archive<R>(
        &self,
        mut archive: Archive<R>,
        download_url: &str,
        target_path: &Path,
        strip_root: bool,
        subdirectory: Option<&str>,
    ) -> FsResult<PathBuf>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut entries = archive
            .entries()
            .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to read tar entries: {}", e))?;

        let mut root_dir: Option<String> = None;
        let mut prefix = PathBuf::new();
        let mut extracted_any = false;

        while let Some(entry_result) = entries.next().await {
            self.cancellation.check_cancellation()?;

            let mut entry = entry_result
                .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to read tar entry: {}", e))?;

            let entry_path: PathBuf = entry
                .path()
                .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to get entry path: {}", e))?
                .into_owned();

            // Determine/validate root directory
            if strip_root {
                // Skip special entries like pax_global_header and macOS resource forks
                let path_str = entry_path.to_string_lossy();
                if path_str == "pax_global_header" || path_str.starts_with("._") {
                    continue;
                }

                let first = entry_path
                    .components()
                    .next()
                    .and_then(|c| match c {
                        std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        fs_err!(
                            ErrorCode::InvalidConfig,
                            "Invalid tar entry path: {}",
                            entry_path.display()
                        )
                    })?;

                match &root_dir {
                    None => {
                        // Compute prefix once when root is discovered
                        prefix = match subdirectory {
                            Some(subdir) => PathBuf::from(&first).join(subdir),
                            None => PathBuf::from(&first),
                        };
                        root_dir = Some(first);
                    }
                    Some(existing_root) => {
                        if *existing_root != first {
                            return err!(
                                ErrorCode::InvalidConfig,
                                "Tarball has multiple root directories: '{}' and '{}'. Expected single root directory.",
                                existing_root,
                                first
                            );
                        }
                    }
                }
            } else if root_dir.is_none() && subdirectory.is_some() {
                // For non-strip-root with subdirectory, compute prefix once
                root_dir = Some(String::new()); // sentinel to avoid re-entering
                prefix = PathBuf::from(subdirectory.unwrap());
            }

            // Filter: skip entries outside the prefix
            if !prefix.as_os_str().is_empty() && !entry_path.starts_with(&prefix) {
                continue;
            }

            // Strip prefix to get relative path
            let relative_path: &Path = if !prefix.as_os_str().is_empty() {
                entry_path.strip_prefix(&prefix).unwrap_or(&entry_path)
            } else {
                &entry_path
            };

            // Skip empty paths (the prefix directory entry itself)
            if relative_path.as_os_str().is_empty() {
                continue;
            }

            let target_entry_path = target_path.join(relative_path);

            // Security: reject paths that escape the target directory (e.g. via ".." components)
            if target_entry_path
                .components()
                .any(|c| c == std::path::Component::ParentDir)
            {
                return err!(
                    ErrorCode::InvalidConfig,
                    "Refusing to extract tar entry with path traversal: {}",
                    entry_path.display()
                );
            }

            // Security: never create a link inside the extraction root. `unpack()`
            // applies no containment to a link *target*, so a link pointing outside
            // the root plus a later entry written through it is an arbitrary file
            // write. Devices and FIFOs are skipped too — a package needs neither.
            //
            // Skipped, not rejected: some published packages commit a self-referential
            // `integration_tests/dbt_packages/<pkg>` symlink, and erroring would make
            // them uninstallable for no security gain.
            let entry_type = entry.header().entry_type();
            if matches!(
                entry_type,
                EntryType::Symlink
                    | EntryType::Link
                    | EntryType::Char
                    | EntryType::Block
                    | EntryType::Fifo
            ) {
                emit_info_log_message(format!(
                    "Skipping unsupported tar entry ({entry_type:?}) in tarball from {download_url}: {}",
                    entry_path.display()
                ));
                continue;
            }

            // `unpack()` relies on the archive's own directory entries, which a
            // producer may omit — and won't be materialized via a skipped link.
            if let Some(parent) = target_entry_path.parent() {
                ensure_dir(parent).await?;
            }

            entry.unpack(&target_entry_path).await.map_err(|e| {
                fs_err!(
                    ErrorCode::IoError,
                    "Failed to unpack entry {}: {}",
                    entry_path.display(),
                    e
                )
            })?;

            extracted_any = true;
        }

        // Validate that we extracted something
        if !extracted_any {
            if let Some(subdir) = subdirectory {
                return err!(
                    ErrorCode::InvalidConfig,
                    "No entries found matching subdirectory '{}' in tarball from {}",
                    subdir,
                    download_url
                );
            } else if strip_root {
                return err!(
                    ErrorCode::InvalidConfig,
                    "No root directory found in tarball from {}",
                    download_url
                );
            } else {
                return err!(
                    ErrorCode::InvalidConfig,
                    "No entries found in tarball from {}",
                    download_url
                );
            }
        }

        Ok(target_path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_common::cancellation::never_cancels;
    use tempfile::TempDir;
    use tokio_tar::{Builder, EntryType, Header};

    const TEST_URL: &str = "http://test/package.tar.gz";
    const PROJECT_YML: &str = "bad_package/dbt_project.yml";

    /// Append one entry. `link_target` applies to hard/symbolic links only.
    async fn append(
        builder: &mut Builder<Vec<u8>>,
        entry_type: EntryType,
        path: &str,
        contents: &[u8],
        link_target: Option<&Path>,
    ) {
        let mut header = Header::new_gnu();
        header.set_entry_type(entry_type);
        // Directories need the execute bit, or unpacking their contents fails.
        header.set_mode(if entry_type.is_dir() { 0o755 } else { 0o644 });
        header.set_size(contents.len() as u64);
        header.set_path(path).unwrap();
        if let Some(link_target) = link_target {
            header.set_link_name(link_target).unwrap();
        }
        header.set_cksum();
        builder.append(&header, contents).await.unwrap();
    }

    async fn append_file(builder: &mut Builder<Vec<u8>>, path: &str, contents: &[u8]) {
        append(builder, EntryType::Regular, path, contents, None).await;
    }

    /// Write the name into the header verbatim, bypassing the `set_path`
    /// validation that refuses `..`, as a hostile archive would.
    async fn append_file_raw_path(builder: &mut Builder<Vec<u8>>, raw_path: &str, contents: &[u8]) {
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::Regular);
        header.set_mode(0o644);
        header.set_size(contents.len() as u64);
        header.as_old_mut().name[..raw_path.len()].copy_from_slice(raw_path.as_bytes());
        header.set_cksum();
        builder.append(&header, contents).await.unwrap();
    }

    fn extraction_root(tmp: &TempDir) -> PathBuf {
        let target = tmp.path().join("dbt_packages");
        std::fs::create_dir_all(&target).unwrap();
        target
    }

    /// Extract with root stripping, as package installs do.
    async fn extract(archive_bytes: &[u8], target: &Path) -> FsResult<PathBuf> {
        let http_client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
        TarballClient::from_client(http_client, never_cancels())
            .extract_archive(Archive::new(archive_bytes), TEST_URL, target, true, None)
            .await
    }

    fn assert_rejected(result: &FsResult<PathBuf>, needle: &str, context: impl std::fmt::Debug) {
        match result {
            Ok(p) => panic!("expected rejection of {context:?}, but extraction succeeded at {p:?}"),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains(needle),
                    "expected error containing {needle:?} for {context:?}, got: {msg}"
                );
            }
        }
    }

    /// A symlink out of the root, followed by a write *through* it. The link is
    /// skipped, so the write lands on a real directory inside the root.
    #[dbt_runtime::test]
    async fn contains_symlink_traversal_escape() {
        let tmp = tempfile::tempdir().unwrap();
        let target = extraction_root(&tmp);

        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let victim = outside.join("example.txt");
        std::fs::write(&victim, b"ORIGINAL").unwrap();

        let mut builder = Builder::new(Vec::new());
        append_file(&mut builder, PROJECT_YML, b"name: bad_package\n").await;
        let link = EntryType::Symlink;
        append(&mut builder, link, "bad_package/evil", &[], Some(&outside)).await;
        append_file(&mut builder, "bad_package/evil/example.txt", b"OWNED\n").await;
        let bytes = builder.into_inner().await.unwrap();

        let result = extract(&bytes, &target).await;

        assert!(result.is_ok(), "expected extraction to succeed: {result:?}");
        assert_eq!(std::fs::read(&victim).unwrap(), b"ORIGINAL");
        assert!(
            std::fs::symlink_metadata(target.join("evil"))
                .unwrap()
                .is_dir()
        );
        assert_eq!(
            std::fs::read(target.join("evil/example.txt")).unwrap(),
            b"OWNED\n"
        );
    }

    /// The entry is not created and the rest of the package still installs.
    #[dbt_runtime::test]
    async fn skips_link_and_device_entries() {
        for entry_type in [
            EntryType::Symlink,
            EntryType::Link,
            EntryType::Char,
            EntryType::Block,
            EntryType::Fifo,
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let target = extraction_root(&tmp);
            let outside = tmp.path().join("outside.txt");
            std::fs::write(&outside, b"ORIGINAL").unwrap();

            let link_target = (entry_type == EntryType::Symlink || entry_type == EntryType::Link)
                .then_some(outside.as_path());

            let mut builder = Builder::new(Vec::new());
            append_file(&mut builder, PROJECT_YML, b"name: bad_package\n").await;
            append(
                &mut builder,
                entry_type,
                "bad_package/entry",
                &[],
                link_target,
            )
            .await;
            let bytes = builder.into_inner().await.unwrap();

            let result = extract(&bytes, &target).await;

            assert!(
                result.is_ok(),
                "expected {entry_type:?} to be skipped, got: {result:?}"
            );
            assert!(
                std::fs::symlink_metadata(target.join("entry")).is_err(),
                "{entry_type:?} was created inside the extraction root"
            );
            assert_eq!(
                std::fs::read(&outside).unwrap(),
                b"ORIGINAL",
                "{entry_type:?} modified a file outside the extraction root"
            );
            assert_eq!(
                std::fs::read(target.join("dbt_project.yml")).unwrap(),
                b"name: bad_package\n"
            );
        }
    }

    /// Packages committing a self-referential `dbt_packages/<pkg> -> ../..`
    /// symlink must keep installing; only the symlink is dropped.
    #[dbt_runtime::test]
    async fn installs_package_with_self_referential_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let target = extraction_root(&tmp);

        let mut builder = Builder::new(Vec::new());
        append_file(
            &mut builder,
            "good_package/dbt_project.yml",
            b"name: good_package\n",
        )
        .await;
        append(
            &mut builder,
            EntryType::Symlink,
            "good_package/integration_tests/dbt_packages/good_package",
            &[],
            Some(Path::new("../..")),
        )
        .await;
        append_file(
            &mut builder,
            "good_package/macros/my_macro.sql",
            b"{% macro %}",
        )
        .await;
        let bytes = builder.into_inner().await.unwrap();

        let result = extract(&bytes, &target).await;

        assert!(result.is_ok(), "expected extraction to succeed: {result:?}");
        assert_eq!(
            std::fs::read(target.join("macros/my_macro.sql")).unwrap(),
            b"{% macro %}"
        );
        assert!(
            std::fs::symlink_metadata(target.join("integration_tests/dbt_packages/good_package"))
                .is_err()
        );
    }

    #[dbt_runtime::test]
    async fn rejects_parent_dir_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let target = extraction_root(&tmp);

        let mut builder = Builder::new(Vec::new());
        append_file(&mut builder, PROJECT_YML, b"name: bad_package\n").await;
        append_file_raw_path(&mut builder, "bad_package/../escaped.txt", b"OWNED\n").await;
        let bytes = builder.into_inner().await.unwrap();

        let result = extract(&bytes, &target).await;

        assert_rejected(&result, "path traversal", "..");
        assert!(!tmp.path().join("escaped.txt").exists());
    }

    #[dbt_runtime::test]
    async fn extracts_regular_package() {
        let tmp = tempfile::tempdir().unwrap();
        let target = extraction_root(&tmp);

        let mut builder = Builder::new(Vec::new());
        append_file(
            &mut builder,
            "good_package/dbt_project.yml",
            b"name: good_package\n",
        )
        .await;
        append(
            &mut builder,
            EntryType::Directory,
            "good_package/models/",
            &[],
            None,
        )
        .await;
        append_file(
            &mut builder,
            "good_package/models/my_model.sql",
            b"select 1\n",
        )
        .await;
        let bytes = builder.into_inner().await.unwrap();

        let result = extract(&bytes, &target).await;

        assert!(result.is_ok(), "expected extraction to succeed: {result:?}");
        assert_eq!(
            std::fs::read(target.join("dbt_project.yml")).unwrap(),
            b"name: good_package\n"
        );
        assert_eq!(
            std::fs::read(target.join("models/my_model.sql")).unwrap(),
            b"select 1\n"
        );
    }

    /// Unlike `extract`, the download path decodes gzip, so these tests need a
    /// real `.tar.gz`. Deterministic: the tar headers carry no mtime.
    async fn good_package_tarball_gz() -> Vec<u8> {
        use async_compression::tokio::write::GzipEncoder;
        use tokio::io::AsyncWriteExt;

        let mut builder = Builder::new(Vec::new());
        append_file(
            &mut builder,
            "good_package/dbt_project.yml",
            b"name: good_package\n",
        )
        .await;
        let tar_bytes = builder.into_inner().await.unwrap();

        let mut encoder = GzipEncoder::new(Vec::new());
        encoder.write_all(&tar_bytes).await.unwrap();
        encoder.shutdown().await.unwrap();
        encoder.into_inner()
    }

    /// Serve `good_package_tarball_gz` over HTTP and download it into `target`.
    async fn download_good_package(
        target: &Path,
        expected_sha1: Option<&str>,
    ) -> FsResult<PathBuf> {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(good_package_tarball_gz().await),
            )
            .mount(&server)
            .await;

        let http_client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
        TarballClient::from_client(http_client, never_cancels())
            .download_and_extract_tarball(&server.uri(), target, true, None, &[], expected_sha1)
            .await
    }

    fn assert_extracted(target: &Path) {
        assert_eq!(
            std::fs::read(target.join("dbt_project.yml")).unwrap(),
            b"name: good_package\n"
        );
    }

    #[dbt_runtime::test]
    async fn download_and_extract_tarball_matching_sha1_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let target = extraction_root(&tmp);
        let sha1 = format!("{:x}", sha1::Sha1::digest(good_package_tarball_gz().await));

        let result = download_good_package(&target, Some(&sha1)).await;

        assert!(result.is_ok(), "expected download to succeed: {result:?}");
        assert_extracted(&target);
    }

    #[dbt_runtime::test]
    async fn download_and_extract_tarball_mismatched_sha1_fails_and_extracts_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let target = extraction_root(&tmp);
        let wrong_sha1 = "0000000000000000000000000000000000000000";

        let result = download_good_package(&target, Some(wrong_sha1)).await;

        assert_rejected(&result, "sha1 checksum mismatch", "mismatched sha1");
        assert!(
            !target.join("dbt_project.yml").exists(),
            "nothing should be extracted when the sha1 check fails"
        );
    }

    /// Guards the untouched streaming path, which the `expected_sha1` branch moved.
    #[dbt_runtime::test]
    async fn download_and_extract_tarball_no_expected_sha1_is_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let target = extraction_root(&tmp);

        let result = download_good_package(&target, None).await;

        assert!(result.is_ok(), "expected download to succeed: {result:?}");
        assert_extracted(&target);
    }
}
