use std::ffi::OsString;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::result::Result;
use std::sync::OnceLock;
use std::time::Duration;
use std::{env, fmt, io};

use crate::driver::DriverFilenameDisplay;
use crate::driver_channel::foundry_driver;
use crate::*;

use adbc_core::error::{Error, Status};
use percent_encoding::AsciiSet;
use sha2::{Digest, Sha256};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

static INSTALLABLE_DRIVERS: &[Backend; 12] = &[
    Backend::Snowflake,
    Backend::BigQuery,
    Backend::ClickHouse,
    Backend::Postgres,
    Backend::Databricks,
    Backend::Redshift,
    Backend::DuckDB,
    Backend::DuckDBExtended,
    Backend::LakeCompute,
    Backend::Salesforce,
    Backend::Spark,
    Backend::SQLServer,
];

/// Linux drivers are labelled with the manylinux image they were built in,
/// so the label is not the same for every driver. Only `OS_ALTERNATES` and the
/// tests read these, so off Linux they are dead code outside `cfg(test)`.
#[cfg(any(target_os = "linux", test))]
const MANYLINUX_2_17_TARGET_OS: &str = "manylinux_2_17-linux-gnu";
#[cfg(any(target_os = "linux", test))]
const MANYLINUX_2_28_TARGET_OS: &str = "manylinux_2_28-linux-gnu";
const MACOS_TARGET_OS: &str = "apple-darwin";
const WINDOWS_TARGET_OS: &str = "pc-windows-msvc";

/// Field order matters: the derived `Ord` compares fields top-to-bottom,
/// matching the (os, arch, version) sort key used in `checksums.rs`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DriverTriplet<'a> {
    pub os: &'a str,
    pub arch: &'a str,
    pub version: &'a str,
}

impl<'a> DriverTriplet<'a> {
    pub fn dll_prefix(&self) -> &'static str {
        match self.os {
            WINDOWS_TARGET_OS => "",
            _ => "lib",
        }
    }

    pub fn dll_suffix(&self) -> &'static str {
        match self.os {
            MACOS_TARGET_OS => ".dylib",
            WINDOWS_TARGET_OS => ".dll",
            _ if self.os.starts_with("manylinux") => ".so",
            _ => {
                debug_assert!(false, "unsupported target OS: {}", self.os);
                ".so"
            }
        }
    }
}

#[derive(Debug)]
pub enum InstallError {
    /// Generic HTTP error. Set up of the client of request failed.
    Http(ureq::Error),
    /// Error while generating a random file name (very unlikely).
    GetRandom(getrandom::Error),
    /// Unable to determine the cache directory for ADBC driver installation.
    DetermineCacheDir,
    /// Generic IO error.
    Io(io::Error),
    /// Error while decompressing the CDN driver file.
    ZstdDecompress(usize),
    /// Error while creating the destination directory for the driver file.
    CreateDir(io::Error, PathBuf),
    /// Error while creating the destination file for the driver.
    CreateFIle(io::Error, PathBuf),
    /// Error while writing the driver file.
    WriteFile(io::Error),
    /// Error while syncing the driver file to storage media.
    SyncFile(io::Error),
    /// Error while atomically renaming the driver file to its final name.
    RenameFile(io::Error),
    /// SHA256 checksum mismatch: expected {}, got {} (URL: {}).
    ChecksumMismatch(String, String, String),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallError::Http(error) => write!(f, "HTTP error: {error}"),
            InstallError::GetRandom(error) => write!(f, "getrandom error: {error}"),
            InstallError::DetermineCacheDir => write!(
                f,
                "Unable to determine cache directory for ADBC driver installation in this platform."
            ),

            InstallError::Io(error) => write!(f, "IO error: {error}"),
            InstallError::ZstdDecompress(code) => {
                let msg = zstd_safe::get_error_name(*code);
                write!(f, "Decompression error: {msg}")
            }
            InstallError::CreateDir(error, path_buf) => {
                write!(
                    f,
                    "Unable to create directory {}: {}",
                    path_buf.display(),
                    error
                )
            }
            InstallError::CreateFIle(error, path_buf) => {
                write!(f, "Unable to create file {}: {}", path_buf.display(), error)
            }
            InstallError::WriteFile(error) => write!(f, "Unable to write file: {error}"),
            InstallError::SyncFile(error) => write!(f, "Unable to sync file: {error}"),
            InstallError::RenameFile(error) => write!(f, "Unable to rename file: {error}"),
            InstallError::ChecksumMismatch(expected, got, url) => {
                write!(
                    f,
                    "SHA-256 checksum mismatch: expected {expected}, got {got} (URL: {url})"
                )
            }
        }
    }
}

impl InstallError {
    fn status_for_io_error(e: &io::Error) -> Status {
        match e.kind() {
            io::ErrorKind::NotFound => Status::NotFound,
            io::ErrorKind::PermissionDenied => Status::Unauthorized,
            io::ErrorKind::ConnectionRefused => Status::Unauthorized,
            io::ErrorKind::ConnectionReset => Status::Cancelled,
            io::ErrorKind::HostUnreachable => Status::IO,
            io::ErrorKind::NetworkUnreachable => Status::IO,
            io::ErrorKind::ConnectionAborted => Status::Cancelled,
            io::ErrorKind::NotConnected => Status::InvalidState,
            io::ErrorKind::AddrInUse => Status::IO,
            io::ErrorKind::AddrNotAvailable => Status::IO,
            io::ErrorKind::NetworkDown => Status::IO,
            io::ErrorKind::BrokenPipe => Status::IO,
            io::ErrorKind::AlreadyExists => Status::AlreadyExists,
            io::ErrorKind::WouldBlock => Status::IO,
            io::ErrorKind::NotADirectory => Status::InvalidArguments,
            io::ErrorKind::IsADirectory => Status::InvalidArguments,
            io::ErrorKind::DirectoryNotEmpty => Status::InvalidState,
            io::ErrorKind::ReadOnlyFilesystem => Status::InvalidState,
            io::ErrorKind::StaleNetworkFileHandle => Status::InvalidState,
            io::ErrorKind::InvalidInput => Status::InvalidArguments,
            io::ErrorKind::InvalidData => Status::InvalidData,
            io::ErrorKind::TimedOut => Status::Timeout,
            io::ErrorKind::WriteZero => Status::IO,
            io::ErrorKind::StorageFull => Status::IO,
            io::ErrorKind::NotSeekable => Status::IO,
            io::ErrorKind::FileTooLarge => Status::IO,
            io::ErrorKind::ResourceBusy => Status::IO,
            io::ErrorKind::ExecutableFileBusy => Status::IO,
            io::ErrorKind::Deadlock => Status::InvalidState,
            io::ErrorKind::TooManyLinks => Status::IO,
            io::ErrorKind::ArgumentListTooLong => Status::InvalidArguments,
            io::ErrorKind::Interrupted => Status::Cancelled,
            io::ErrorKind::Unsupported => Status::NotImplemented,
            io::ErrorKind::UnexpectedEof => Status::InvalidData,
            io::ErrorKind::OutOfMemory => Status::Internal,
            io::ErrorKind::Other => Status::Internal,
            _ => Status::Internal,
        }
    }

    pub fn to_adbc_error(&self) -> Error {
        let status = match self {
            InstallError::Http(e) => match e {
                ureq::Error::StatusCode(_) => Status::Internal,
                ureq::Error::Http(_) => Status::Internal,
                ureq::Error::BadUri(_) => Status::InvalidArguments,
                ureq::Error::Protocol(_) => Status::InvalidData,
                ureq::Error::Io(e) => Self::status_for_io_error(e),
                ureq::Error::Timeout(_) => Status::Timeout,
                ureq::Error::HostNotFound => Status::NotFound,
                ureq::Error::RedirectFailed => Status::InvalidState,
                ureq::Error::InvalidProxyUrl => Status::InvalidArguments,
                ureq::Error::ConnectionFailed => Status::Cancelled,
                ureq::Error::BodyExceedsLimit(_) => Status::InvalidData,
                ureq::Error::TooManyRedirects => Status::Internal,
                ureq::Error::Tls(_) => Status::Internal,
                ureq::Error::Pem(_) => Status::InvalidData,
                ureq::Error::Rustls(_) => Status::Internal,
                ureq::Error::RequireHttpsOnly(_) => Status::InvalidArguments,
                ureq::Error::LargeResponseHeader(_, _) => Status::InvalidData,
                ureq::Error::Decompress(_, _) => Status::InvalidData,
                ureq::Error::Json(_) => Status::InvalidData,
                ureq::Error::ConnectProxyFailed(_) => Status::Cancelled,
                ureq::Error::TlsRequired => Status::InvalidArguments,
                ureq::Error::Other(_) => Status::Internal,
                _ => Status::Internal,
            },
            InstallError::GetRandom(_) => Status::Internal,
            InstallError::DetermineCacheDir => Status::Internal,
            InstallError::Io(e) => Self::status_for_io_error(e),
            InstallError::ZstdDecompress(_) => Status::InvalidData,
            InstallError::CreateDir(_, _) => Status::IO,
            InstallError::CreateFIle(_, _) => Status::IO,
            InstallError::WriteFile(_) => Status::IO,
            InstallError::SyncFile(_) => Status::IO,
            InstallError::RenameFile(_) => Status::IO,
            InstallError::ChecksumMismatch(_, _, _) => Status::InvalidData,
        };
        let message = format!("Driver installation error: {self}");
        Error::with_message_and_status(message, status)
    }
}

/// The CDN drivers are downloaded from, resolved once per process.
///
/// `DBT_ADBC_USE_STAGING_CDN=1` switches debug builds to the staging mirror
/// (written by `release.adbc.yml` on a `dry_run`) to exercise a driver before
/// it is promoted from `main`. Release builds ignore it, so a shipped binary
/// can't be pointed at pre-release artifacts.
///
/// This also moves `adbc-sync`, which hashes whatever it downloads.
pub fn cdn_host() -> &'static str {
    const PUBLIC_DBT_CDN: &str = "public.cdn.getdbt.com";
    #[cfg(not(debug_assertions))]
    {
        PUBLIC_DBT_CDN
    }
    #[cfg(debug_assertions)]
    {
        const STAGING_DBT_CDN: &str = "public.staging.cdn.getdbt.com";
        static HOST: OnceLock<&str> = OnceLock::new();
        HOST.get_or_init(|| {
            if env_var::env_var_bool_or_warn("DBT_ADBC_USE_STAGING_CDN") {
                STAGING_DBT_CDN
            } else {
                PUBLIC_DBT_CDN
            }
        })
    }
}

pub fn format_driver_url(backend_name: &str, triplet: DriverTriplet) -> String {
    let host = cdn_host();

    // %-encode most non-alphanumeric characters in the version string
    const NON_ALPHANUMERIC: &AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_');
    format!(
        "https://{}/fs/adbc/{}/adbc_driver_{}-{}-{}-{}{}.zst",
        host,
        backend_name,
        backend_name,
        percent_encoding::utf8_percent_encode(triplet.version, NON_ALPHANUMERIC),
        triplet.arch,
        triplet.os,
        triplet.dll_suffix(),
    )
}

/// Format the full path to the driver file in the OS cache directory.
///
/// Examples for each platform:
///
/// Linux
///
/// ${XDG_CACHE_HOME}/com.getdbt/adbc/x86_64-unknown-linux-musl/libadbc_driver_snowflake-0.17.0+dbt0.0.1.so
/// or ${HOME}/.cache/com.getdbt/adbc/x86_64-unknown-linux-musl/libadbc_driver_snowflake-0.17.0+dbt0.0.1.so
///
/// macOS
///
/// ${HOME}/Library/Caches/com.getdbt/adbc/aarch64-macos/libadbc_driver_snowflake-0.17.0+dbt0.0.1.dylib
///
/// Windows
///
/// ${FOLDERID_LocalAppData}/com.getdbt/adbc/x86_64-pc-windows-msvc/adbc_driver_snowflake-0.17.0+dbt0.0.1.dll
pub fn format_driver_path(name: &str, triplet: DriverTriplet) -> Result<PathBuf, InstallError> {
    const APP_ID: &str = "com.getdbt";
    dirs::cache_dir()
        .map(|cache_dir| {
            let driver_relpath = format!(
                "{}-{}/{}",
                triplet.arch,
                triplet.os,
                DriverFilenameDisplay { name, triplet }
            );
            cache_dir.join(APP_ID).join("adbc").join(driver_relpath)
        })
        .ok_or(InstallError::DetermineCacheDir)
}

/// ADBC users can call this function to pre-install the driver for the given backend.
///
/// Instead of relying on the automatic installation at connection creation time.
pub fn pre_install_driver(http_agent: &ureq::Agent, backend: Backend) -> Result<(), InstallError> {
    if !is_installable_driver(backend) {
        return Ok(());
    }
    let (backend_name, triplet) = driver_parameters(backend);
    install_driver_internal(http_agent, backend_name, triplet)
}

/// Pre-install all supported drivers for the current platform.
pub fn pre_install_all_drivers() -> Result<(), InstallError> {
    let http_agent = build_http_agent();
    for backend in INSTALLABLE_DRIVERS.iter() {
        pre_install_driver(&http_agent, *backend)?;
    }
    Ok(())
}

pub fn is_installable_driver(backend: Backend) -> bool {
    // NOTE(serramatutu): Allow us to debug drivers locally without needing them to be installable via CDN.
    #[cfg(debug_assertions)]
    {
        if env_var::env_var_bool("DISABLE_CDN_DRIVER_CACHE")
            .unwrap()
            .unwrap_or(false)
        {
            return true;
        }
    }

    INSTALLABLE_DRIVERS.contains(&backend)
}

/// Return the backend name and selected driver version.
///
/// Pre-condition: is_installable_driver(backend)
pub fn backend_name_and_version(backend: Backend) -> (&'static str, &'static str) {
    debug_assert!(is_installable_driver(backend));
    // Logged because the channel is otherwise invisible: both drivers report
    // the same `Backend`, so this is the only place that says which one a run
    // actually resolved.
    let (name, version) = if let Some((name, version)) = foundry_driver(backend) {
        (name, version)
    } else {
        canonical_name_and_version(backend)
    };
    tracing::debug!("{backend} driver resolved: {name} {version}");
    (name, version)
}

/// The canonical driver for `backend`, ignoring any opted-in channel.
fn canonical_name_and_version(backend: Backend) -> (&'static str, &'static str) {
    match backend {
        Backend::Snowflake => ("snowflake", SNOWFLAKE_DRIVER_VERSION),
        Backend::BigQuery => ("bigquery", BIGQUERY_DRIVER_VERSION),
        Backend::Postgres => ("postgresql", POSTGRES_DRIVER_VERSION),
        Backend::Databricks => ("databricks", DATABRICKS_DRIVER_VERSION),
        Backend::Redshift => ("redshift", REDSHIFT_DRIVER_VERSION),
        Backend::Spark => ("spark", SPARK_DRIVER_VERSION),
        Backend::Salesforce => ("salesforce", SALESFORCE_DRIVER_VERSION),
        Backend::DuckDB => ("duckdb", DUCKDB_DRIVER_VERSION),
        Backend::DuckDBExtended => ("duckdb_extended", DUCKDB_EXTENDED_DRIVER_VERSION),
        Backend::LakeCompute => ("dbt", LAKE_COMPUTE_DRIVER_VERSION),
        Backend::SQLServer => ("mssql", MSSQLSERVER_DRIVER_VERSION),
        Backend::ClickHouse => ("clickhouse", CLICKHOUSE_DRIVER_VERSION),
        Backend::Athena | Backend::Exasol | Backend::GizmoSQL | Backend::Generic { .. } => {
            unreachable!("driver_parameters() called with backend={:?}", backend)
        }
    }
}

/// Return the backend name is the [DriverTriplet] for the given backend for this machine.
///
/// Pre-condition: is_installable_driver(backend)
pub fn driver_parameters(backend: Backend) -> (&'static str, DriverTriplet<'static>) {
    #[cfg(target_os = "linux")]
    const OS_ALTERNATES: &[&str] = &[MANYLINUX_2_17_TARGET_OS, MANYLINUX_2_28_TARGET_OS];
    #[cfg(target_os = "macos")]
    const OS_ALTERNATES: &[&str] = &[MACOS_TARGET_OS];
    #[cfg(target_os = "windows")]
    const OS_ALTERNATES: &[&str] = &[WINDOWS_TARGET_OS];

    let (backend_name, version) = backend_name_and_version(backend);
    for os in OS_ALTERNATES {
        let triplet = DriverTriplet {
            os,
            arch: env::consts::ARCH,
            version,
        };
        if find_expected_checksum(backend_name, triplet).is_some() {
            return (backend_name, triplet);
        }
    }
    // If this is reached, there is a bug somewhere else.
    debug_assert!(
        false,
        "No known driver triplet for backend {} on this platform",
        backend_name
    );
    (
        backend_name,
        DriverTriplet {
            os: OS_ALTERNATES[0],
            arch: env::consts::ARCH,
            version,
        },
    )
}

/// Find the expected SHA-256 checksum for the compressed driver file.
pub fn find_expected_checksum(backend_name: &str, triplet: DriverTriplet) -> Option<&'static str> {
    let checksums = checksums::SORTED_CDN_DRIVER_CHECKSUMS.as_ref();
    #[cfg(debug_assertions)]
    for i in 0..checksums.len() - 1 {
        debug_assert!(
            checksums[i] < checksums[i + 1],
            "SORTED_CDN_DRIVER_CHECKSUMS must be sorted"
        );
    }
    let query = (backend_name, triplet.os, triplet.arch, triplet.version);
    checksums
        .binary_search_by(|(elem, _)| elem.cmp(&query))
        .ok()
        .map(|index| checksums[index].1)
}

pub fn install_driver_internal(
    http_agent: &ureq::Agent,
    backend_name: &str,
    triplet: DriverTriplet,
) -> Result<(), InstallError> {
    let full_driver_path = format_driver_path(backend_name, triplet)?;
    let url = format_driver_url(backend_name, triplet);
    let checksum = find_expected_checksum(backend_name, triplet);
    download_zst_driver_file(http_agent, &url, &full_driver_path, checksum)
}

/// Unguessable temporary file name generator.
fn tmpname(
    prefix: impl AsRef<str>,
    rand_len: usize,
    suffix: impl AsRef<str>,
) -> Result<OsString, getrandom::Error> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

    // get random data directly from the OS entropy source
    let mut seeds = vec![0; rand_len];
    getrandom::getrandom(&mut seeds)?;

    let mut name_bytes = Vec::with_capacity(rand_len);
    for b in seeds {
        name_bytes.push(CHARS[(b as usize) % CHARS.len()]);
    }
    // safe to .unwrap() as we know the bytes are valid UTF-8
    let name = std::str::from_utf8(&name_bytes).unwrap();
    let full_name = format!("{}{}{}", prefix.as_ref(), name, suffix.as_ref());
    Ok(OsString::from(full_name))
}

/// [zstd_safe::WriteBuf] implementation backed by a [Vec<u8>] buffer.
struct ZstdWriteBuffer {
    inner: Vec<u8>,
}

impl ZstdWriteBuffer {
    fn with_fixed_capacity(capacity: usize) -> Self {
        let buffer = Vec::with_capacity(capacity);
        Self { inner: buffer }
    }
}

unsafe impl zstd_safe::WriteBuf for ZstdWriteBuffer {
    fn as_slice(&self) -> &[u8] {
        &self.inner
    }

    fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.inner.as_mut_ptr()
    }

    unsafe fn filled_until(&mut self, n: usize) {
        // SAFETY: n <= self.capacity() (invariant)
        unsafe {
            self.inner.set_len(n);
        }
    }
}

const DRIVER_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// Build a TLS configuration that uses the OS certificate store.
///
/// [1]: https://github.com/dbt-labs/dbt-fusion/issues/147
pub fn build_tls_config() -> TlsConfig {
    TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::PlatformVerifier)
        .build()
}

/// Configure the HTTP agent for driver downloads.
pub fn build_http_agent() -> ureq::Agent {
    let http_config = ureq::Agent::config_builder()
        .tls_config(build_tls_config())
        .timeout_global(Some(DRIVER_DOWNLOAD_TIMEOUT))
        .build();
    ureq::Agent::new_with_config(http_config)
}

/// Download a Zstandard-compressed file from the given URL and save (atomically and durably)
/// it to the fully-qualified destination path.
pub fn download_zst_driver_file(
    http_agent: &ureq::Agent,
    url: &str,
    destination: &Path,
    expected_sha256sum: Option<&str>,
) -> Result<(), InstallError> {
    debug_assert!(
        destination.is_absolute(),
        "destination path must be absolute"
    );
    let mut response = http_agent.get(url).call().map_err(InstallError::Http)?;

    // Generate a random file name and create an empty temporary file
    let tmp_path = {
        let tmp_name = tmpname(".", 15, ".download").map_err(InstallError::GetRandom)?;
        // ensure the destination exists and create it if necessary
        let parent = destination.parent().ok_or_else(|| {
            let message = format!(
                "destination path must be an absolute path: {}",
                destination.display()
            );
            let error = io::Error::new(io::ErrorKind::InvalidInput, message);
            InstallError::Io(error)
        })?;
        std::fs::create_dir_all(parent).map_err(|e| InstallError::CreateDir(e, parent.into()))?;
        // write the file in the same directory as the destination to ensure
        // the rename operation is atomic (happens on the same filesystem)
        parent.join(tmp_name)
    };
    let mut tmp = std::fs::File::create(&tmp_path)
        .map_err(|e| InstallError::CreateFIle(e, tmp_path.clone()))?;

    // Download and Zstandard decompression buffers
    const DECOMPRESSION_FACTOR: usize = 4;
    let mut zstd_insize_hint = zstd_safe::DCtx::in_size(); // ~200KB
    let mut download_buffer = Vec::with_capacity(zstd_insize_hint);
    let write_buffer_capacity = (DECOMPRESSION_FACTOR * download_buffer.capacity()).max(64 * 1024);
    let mut inner_write_buffer = ZstdWriteBuffer::with_fixed_capacity(write_buffer_capacity);
    let mut write_buffer = zstd_safe::OutBuffer::around(&mut inner_write_buffer);

    // Zstandard decompression context and SHA-256 hasher
    let mut dctx = zstd_safe::DCtx::create();
    let mut hasher = Sha256::new();

    // Try to decompress all the data in the download_buffer, if not possible, the remaining data
    // will be moved to the beginning of the Vec which will be resized to the remaining size.
    let mut decompress_step = |download_buffer: &mut Vec<u8>,
                               write_buffer: &mut zstd_safe::OutBuffer<'_, ZstdWriteBuffer>|
     -> Result<usize, InstallError> {
        debug_assert!(!download_buffer.is_empty());
        // To simplify things, we keep the compressed data always at the beginning of the
        // download_buffer, so an InBuffer around it can be created every time we use it.
        let mut in_buffer = zstd_safe::InBuffer::around(download_buffer.as_slice());

        // let prev_write_buffer_pos = write_buffer.pos();
        let next_insize_hint = dctx
            .decompress_stream(write_buffer, &mut in_buffer)
            .map_err(InstallError::ZstdDecompress)?;
        if in_buffer.pos() == 0 {
            return Ok(0);
        }
        let remaining = in_buffer.pos()..download_buffer.len();
        // println!(
        //     "Decompressed {} bytes to {} bytes ({} bytes remaining in the download_buffer)",
        //     in_buffer.pos,
        //     write_buffer.pos() - prev_write_buffer_pos,
        //     remaining.len(),
        // );

        // Update the hasher with the downloaded data that was decompressed.
        hasher.update(&download_buffer[0..in_buffer.pos()]);

        // Move remaining data to the beginning of the download buffer.
        // (only happens when there isn't enough space in the write buffer)
        let remaining_len = remaining.len();
        download_buffer.copy_within(remaining, 0);
        download_buffer.truncate(remaining_len);
        Ok(next_insize_hint)
    };

    let mut download_stream = response.body_mut().as_reader();
    loop {
        if download_buffer.len() < zstd_insize_hint {
            // Ensure there is `zstd_insize_hint` bytes of capacity in `download_buffer`.
            download_buffer.reserve(zstd_insize_hint - download_buffer.len());
            // Try to read what is needed to fill `download_buffer` with at most
            // `zstd_insize_hint` bytes.
            {
                let initial_pos = download_buffer.len();
                // SAFETY: capacity guaranteed above, truncated after the read.
                unsafe { download_buffer.set_len(zstd_insize_hint) };
                debug_assert!(download_buffer.len() > initial_pos);
                let chunk_n = download_stream.read(&mut download_buffer[initial_pos..]);
                match chunk_n {
                    Ok(n) => {
                        // set download_buffer length to include only valid data
                        download_buffer.truncate(initial_pos + n);
                        if n == 0 {
                            break;
                        }
                    }
                    Err(e) => {
                        download_buffer.truncate(initial_pos);
                        return Err(InstallError::Io(e));
                    }
                }
            }
        }
        // Keep asking for more data until we have a good amount of data to decompress.
        if download_buffer.len() < zstd_insize_hint {
            continue;
        }
        // Decompress when we have the right amount of data in the download buffer.
        debug_assert!(!download_buffer.is_empty());
        let hint = decompress_step(&mut download_buffer, &mut write_buffer)?;
        if hint > 0 {
            zstd_insize_hint = hint;
        }
        // If the write buffer is full, drain it to the temporary file.
        if write_buffer.pos() == write_buffer.capacity() {
            tmp.write_all(write_buffer.as_slice())
                .map_err(InstallError::WriteFile)?;
            unsafe {
                write_buffer.set_pos(0);
            }
        }
    }
    // Final download_buffer drain
    while !download_buffer.is_empty() {
        let _ = decompress_step(&mut download_buffer, &mut write_buffer)?;
        tmp.write_all(write_buffer.as_slice())
            .map_err(InstallError::WriteFile)?;
        unsafe {
            write_buffer.set_pos(0);
        }
    }
    // Final write_buffer drain.
    if write_buffer.pos() > 0 {
        tmp.write_all(write_buffer.as_slice())
            .map_err(InstallError::WriteFile)?;
    }

    // Finalize the hash computation and compare it with the expected value.
    let sha256sum = hasher.finalize();
    if let Some(expected) = expected_sha256sum {
        debug_assert!(
            expected.len() == 64,
            "expected SHA-256 checksum must be 64 hex characters"
        );
        const HEX: &[u8] = b"0123456789abcdef";
        let mut got = [0; 64];
        for (i, b) in sha256sum.iter().enumerate() {
            got[i * 2] = HEX[(b >> 4) as usize];
            got[i * 2 + 1] = HEX[(b & 0xf) as usize];
        }
        let got = std::str::from_utf8(&got).unwrap();
        if got != expected {
            return Err(InstallError::ChecksumMismatch(
                expected.to_string(),
                got.to_string(),
                url.to_string(),
            ));
        }
    }

    // fsync() the temp file and atomically rename it to the destination.
    tmp.sync_data().map_err(InstallError::SyncFile)?;
    finalize_install(&tmp_path, destination)
}

/// Move a fully-downloaded temporary file over the destination.
///
/// If the rename fails but the destination exists, another process installed the
/// same (version-qualified) driver concurrently and we can use theirs. This is the
/// common case on Windows, where renaming over a DLL another process has already
/// `LoadLibrary`'d fails with "Access is denied" (os error 5).
fn finalize_install(tmp_path: &Path, destination: &Path) -> Result<(), InstallError> {
    if let Err(e) = std::fs::rename(tmp_path, destination) {
        let installed_by_other_process = destination.try_exists().unwrap_or(false);
        let _ = std::fs::remove_file(tmp_path);
        if !installed_by_other_process {
            return Err(InstallError::RenameFile(e));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    #[test]
    fn finalize_install_tolerates_concurrent_install() {
        let dir = env::temp_dir().join(tmpname("finalize-install-", 12, "").unwrap());
        std::fs::create_dir_all(&dir).unwrap();
        let dst = dir.join("driver.bin");
        let missing_tmp = dir.join("nonexistent.download");

        // Rename failure with no destination in place is a real error.
        assert!(finalize_install(&missing_tmp, &dst).is_err());

        // Another process won the race: destination exists, so we accept it.
        std::fs::write(&dst, b"theirs").unwrap();
        assert!(finalize_install(&missing_tmp, &dst).is_ok());
        assert_eq!(std::fs::read(&dst).unwrap(), b"theirs");

        // Normal path still renames.
        let tmp = dir.join("ours.download");
        std::fs::write(&tmp, b"ours").unwrap();
        assert!(finalize_install(&tmp, &dst).is_ok());
        assert_eq!(std::fs::read(&dst).unwrap(), b"ours");
        assert!(!tmp.exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn remove_test_var(name: &str) {
        #[allow(
            clippy::disallowed_methods,
            reason = "installability tests serialize env-var mutations with ENV_LOCK"
        )]
        unsafe {
            env::remove_var(name);
        }
    }

    #[test]
    fn test_format_driver_url() {
        let triplet = DriverTriplet {
            os: "manylinux_2_17-linux-gnu",
            arch: "x86_64",
            version: "0.17.0+dbt0.2.0",
        };
        let url = format_driver_url("snowflake", triplet);
        assert_eq!(
            url,
            "https://public.cdn.getdbt.com/fs/adbc/snowflake/adbc_driver_snowflake-0.17.0%2Bdbt0.2.0-x86_64-manylinux_2_17-linux-gnu.so.zst",
        );
    }

    #[test]
    fn test_format_driver_path() {
        let triplet = DriverTriplet {
            os: "manylinux_2_17-linux-gnu",
            arch: "x86_64",
            version: "0.17.0+dbt0.2.0",
        };
        let path = format_driver_path("snowflake", triplet).unwrap();

        let dbt_cache_dir = dirs::cache_dir().unwrap().join("com.getdbt").join("adbc");
        let expected = PathBuf::from(format!(
            "{}/x86_64-manylinux_2_17-linux-gnu/libadbc_driver_snowflake-0.17.0+dbt0.2.0.so",
            dbt_cache_dir.display(),
        ));
        assert_eq!(path, expected);
    }

    #[test]
    fn driver_triplet_uses_platform_dll_prefixes_and_suffixes() {
        let cases = [
            (
                DriverTriplet {
                    os: MANYLINUX_2_17_TARGET_OS,
                    arch: "x86_64",
                    version: "1",
                },
                "lib",
                ".so",
            ),
            (
                DriverTriplet {
                    os: MANYLINUX_2_28_TARGET_OS,
                    arch: "x86_64",
                    version: "1",
                },
                "lib",
                ".so",
            ),
            (
                DriverTriplet {
                    os: MACOS_TARGET_OS,
                    arch: "aarch64",
                    version: "1",
                },
                "lib",
                ".dylib",
            ),
            (
                DriverTriplet {
                    os: WINDOWS_TARGET_OS,
                    arch: "x86_64",
                    version: "1",
                },
                "",
                ".dll",
            ),
            (
                DriverTriplet {
                    os: "manylinux_2_28-linux-gnu",
                    arch: "aarch64",
                    version: "1",
                },
                "lib",
                ".so",
            ),
        ];

        for (triplet, prefix, suffix) in cases {
            assert_eq!(triplet.dll_prefix(), prefix);
            assert_eq!(triplet.dll_suffix(), suffix);
        }
    }

    #[test]
    fn installable_driver_set_matches_cdn_backends_by_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        remove_test_var("DISABLE_CDN_DRIVER_CACHE");

        for backend in [
            Backend::Snowflake,
            Backend::BigQuery,
            Backend::Postgres,
            Backend::Databricks,
            Backend::Redshift,
            Backend::DuckDB,
            Backend::DuckDBExtended,
            Backend::Salesforce,
            Backend::Spark,
            Backend::SQLServer,
            Backend::ClickHouse,
        ] {
            assert!(
                is_installable_driver(backend),
                "{backend:?} should be CDN-installable"
            );
        }

        for backend in [
            Backend::Athena,
            Backend::Exasol,
            Backend::GizmoSQL,
            Backend::Generic {
                library_name: "adbc_driver_sqlite",
                entrypoint: Some(b"SqliteDriverInit"),
            },
        ] {
            assert!(
                !is_installable_driver(backend),
                "{backend:?} should use system/manual loading"
            );
        }
    }

    #[test]
    fn backend_names_and_current_platform_triplet_are_stable() {
        assert_eq!(
            backend_name_and_version(Backend::DuckDB),
            ("duckdb", DUCKDB_DRIVER_VERSION)
        );
        assert_eq!(
            backend_name_and_version(Backend::DuckDBExtended),
            ("duckdb_extended", DUCKDB_EXTENDED_DRIVER_VERSION)
        );
        assert_eq!(
            backend_name_and_version(Backend::SQLServer),
            ("mssql", MSSQLSERVER_DRIVER_VERSION)
        );
        assert_eq!(
            backend_name_and_version(Backend::Postgres),
            ("postgresql", POSTGRES_DRIVER_VERSION)
        );

        let (backend_name, triplet) = driver_parameters(Backend::Snowflake);
        assert_eq!(backend_name, "snowflake");
        assert_eq!(triplet.arch, env::consts::ARCH);
        assert_eq!(triplet.version, SNOWFLAKE_DRIVER_VERSION);

        #[cfg(target_os = "linux")]
        assert!(triplet.os == MANYLINUX_2_17_TARGET_OS || triplet.os == MANYLINUX_2_28_TARGET_OS);
        #[cfg(target_os = "macos")]
        assert_eq!(triplet.os, MACOS_TARGET_OS);
        #[cfg(target_os = "windows")]
        assert_eq!(triplet.os, WINDOWS_TARGET_OS);
    }

    #[test]
    fn checksum_lookup_requires_an_exact_backend_triplet() {
        let (backend_name, version) = backend_name_and_version(Backend::Snowflake);
        let triplet = DriverTriplet {
            os: MACOS_TARGET_OS,
            arch: "aarch64",
            version,
        };

        assert!(find_expected_checksum(backend_name, triplet).is_some());

        let unknown_version = DriverTriplet {
            os: MACOS_TARGET_OS,
            arch: "aarch64",
            version: "0.0.0",
        };
        assert!(find_expected_checksum(backend_name, unknown_version).is_none());
        assert!(find_expected_checksum("unknown", triplet).is_none());
    }

    #[test]
    fn install_error_maps_io_status_and_preserves_context() {
        let cases = [
            (io::ErrorKind::NotFound, Status::NotFound),
            (io::ErrorKind::PermissionDenied, Status::Unauthorized),
            (io::ErrorKind::ConnectionRefused, Status::Unauthorized),
            (io::ErrorKind::ConnectionReset, Status::Cancelled),
            (io::ErrorKind::HostUnreachable, Status::IO),
            (io::ErrorKind::NetworkUnreachable, Status::IO),
            (io::ErrorKind::ConnectionAborted, Status::Cancelled),
            (io::ErrorKind::NotConnected, Status::InvalidState),
            (io::ErrorKind::AddrInUse, Status::IO),
            (io::ErrorKind::AddrNotAvailable, Status::IO),
            (io::ErrorKind::NetworkDown, Status::IO),
            (io::ErrorKind::BrokenPipe, Status::IO),
            (io::ErrorKind::AlreadyExists, Status::AlreadyExists),
            (io::ErrorKind::WouldBlock, Status::IO),
            (io::ErrorKind::NotADirectory, Status::InvalidArguments),
            (io::ErrorKind::IsADirectory, Status::InvalidArguments),
            (io::ErrorKind::DirectoryNotEmpty, Status::InvalidState),
            (io::ErrorKind::ReadOnlyFilesystem, Status::InvalidState),
            (io::ErrorKind::StaleNetworkFileHandle, Status::InvalidState),
            (io::ErrorKind::InvalidInput, Status::InvalidArguments),
            (io::ErrorKind::InvalidData, Status::InvalidData),
            (io::ErrorKind::TimedOut, Status::Timeout),
            (io::ErrorKind::WriteZero, Status::IO),
            (io::ErrorKind::StorageFull, Status::IO),
            (io::ErrorKind::NotSeekable, Status::IO),
            (io::ErrorKind::FileTooLarge, Status::IO),
            (io::ErrorKind::ResourceBusy, Status::IO),
            (io::ErrorKind::ExecutableFileBusy, Status::IO),
            (io::ErrorKind::Deadlock, Status::InvalidState),
            (io::ErrorKind::TooManyLinks, Status::IO),
            (io::ErrorKind::ArgumentListTooLong, Status::InvalidArguments),
            (io::ErrorKind::Interrupted, Status::Cancelled),
            (io::ErrorKind::Unsupported, Status::NotImplemented),
            (io::ErrorKind::UnexpectedEof, Status::InvalidData),
            (io::ErrorKind::OutOfMemory, Status::Internal),
        ];

        for (kind, status) in cases {
            let error = InstallError::Io(io::Error::from(kind)).to_adbc_error();
            assert_eq!(error.status, status, "{kind:?}");
            assert!(error.message.contains("Driver installation error"));
        }

        let checksum = InstallError::ChecksumMismatch(
            "expected".to_string(),
            "actual".to_string(),
            "https://example.invalid/driver.zst".to_string(),
        )
        .to_adbc_error();
        assert_eq!(checksum.status, Status::InvalidData);
        assert!(checksum.message.contains("expected expected"));
        assert!(checksum.message.contains("got actual"));
    }

    /// Check that the expected SHA-256 checksum is found for each backend, version, target_os and
    /// arch combinations.
    ///
    /// IMPORTANT: If this is failing, it probably means you need to run
    /// `./scripts/gen_cdn_driver_checksums.sh` and try again.
    ///
    /// This test also guarantees that the driver exists on the CDN because the
    /// `gen_cdn_driver_checksums.sh` script can only generate checksums for the drivers it can
    /// download.
    #[test]
    fn test_all_checksums_are_listed() {
        let target_os_and_archs = [
            (
                &[MANYLINUX_2_17_TARGET_OS, MANYLINUX_2_28_TARGET_OS] as &[&str],
                vec!["x86_64", "aarch64"],
            ),
            (&[MACOS_TARGET_OS], vec!["x86_64", "aarch64"]),
            (&[WINDOWS_TARGET_OS], vec!["x86_64"]),
        ];
        // Both channels. Going around `backend_name_and_version` keeps this
        // independent of any opt-in variable set in the environment, which
        // would otherwise hide one channel or the other.
        let drivers = INSTALLABLE_DRIVERS
            .iter()
            .map(|backend| (backend, canonical_name_and_version(*backend)))
            .chain(INSTALLABLE_DRIVERS.iter().filter_map(|backend| {
                driver_channel::published_foundry_driver(*backend).map(|pair| (backend, pair))
            }));

        for (backend, (backend_name, version)) in drivers {
            for (os_alternates, archs) in target_os_and_archs.iter() {
                for arch in archs {
                    let mut count_hits = 0;
                    for os in *os_alternates {
                        match (backend, *os, *arch) {
                            // no driver available for Intel Macs connecting to MS SQL or ClickHouse
                            (
                                Backend::SQLServer | Backend::ClickHouse,
                                MACOS_TARGET_OS,
                                "x86_64",
                            ) => {
                                count_hits += 1;
                                continue;
                            }
                            _ => {
                                let triplet = DriverTriplet { os, arch, version };
                                let checksum = find_expected_checksum(backend_name, triplet);
                                if checksum.is_some() {
                                    count_hits += 1;
                                }
                            }
                        }
                    }
                    assert!(
                        count_hits > 0,
                        "Missing checksum for `{}`, version: {version},
 os: {os_alternates:?}, arch: {arch}",
                        backend_name
                    );
                }
            }
        }
    }
}
