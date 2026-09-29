use async_trait::async_trait;
use dbt_common::ErrorCode;
use dbt_common::err;
use dbt_common::fs_err;
use dbt_common::tracing::dbt_emit::{emit_warn_log_message, println};
use dbt_common::tracing::event_info::store_event_attributes;
use dbt_telemetry::PackageUpdate;
use rand::Rng as _;
#[cfg(not(target_os = "windows"))]
use run_script::ScriptOptions;
use std::env;
#[cfg(not(target_os = "windows"))]
use std::path::Path;
use std::process::Command;

#[cfg(target_os = "windows")]
use std::fs::File;
#[cfg(target_os = "windows")]
use std::io::Write as _;
#[cfg(target_os = "windows")]
use std::process::Stdio;
#[cfg(target_os = "windows")]
use uuid::Uuid;

#[cfg(not(target_os = "windows"))]
use async_compression::tokio::bufread::GzipDecoder;
#[cfg(not(target_os = "windows"))]
use futures::StreamExt;

use dbt_common::FsResult;
use dbt_dist::version::{VersionsHttpClient, cdn_base_url, resolve_target_version};
use std::future::Future;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Retry policy for CDN fetches
// ---------------------------------------------------------------------------

/// Total attempts (1 initial + 4 retries) for transient CDN failures.
const CDN_MAX_ATTEMPTS: u32 = 5;
/// Initial backoff; doubles each retry with ≤25% random jitter (500ms, ~1s, ~2s, ~4s).
const CDN_BACKOFF_INITIAL: Duration = Duration::from_millis(500);
/// Per-attempt time budget: caps hung TLS handshakes / stuck body reads so the
/// "bounded retries" guarantee actually holds. Total worst-case wall time is
/// `CDN_MAX_ATTEMPTS * CDN_REQUEST_TIMEOUT` plus backoff (~157s in the extreme).
const CDN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

fn error_code_for_status(status: reqwest::StatusCode) -> ErrorCode {
    if status == reqwest::StatusCode::NOT_FOUND {
        ErrorCode::FileNotFound
    } else {
        ErrorCode::IoError
    }
}

fn is_retryable_reqwest_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request() || err.is_body()
}

fn fmt_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

/// Run `fetch` up to `max_attempts` times with a per-attempt `request_timeout`,
/// retrying only on transient failures. The inner closure returns
/// `Err((retryable, code, message))` — `retryable=false` short-circuits the loop.
/// Backoff doubles each retry with up to 25% random jitter.
async fn fetch_with_retries<T, F, Fut>(
    url: &str,
    max_attempts: u32,
    initial_backoff: Duration,
    request_timeout: Duration,
    mut fetch: F,
) -> FsResult<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, (bool, ErrorCode, String)>>,
{
    debug_assert!(max_attempts >= 1, "max_attempts must be >= 1");
    let max_attempts = max_attempts.max(1);
    let mut delay = initial_backoff;
    for attempt in 1..=max_attempts {
        let outcome = match tokio::time::timeout(request_timeout, fetch()).await {
            Ok(inner) => inner,
            Err(_) => Err((
                true,
                ErrorCode::IoError,
                format!("timed out after {request_timeout:?}"),
            )),
        };
        match outcome {
            Ok(v) => return Ok(v),
            Err((retryable, code, msg)) => {
                if !retryable || attempt == max_attempts {
                    return err!(
                        code,
                        "HTTP GET {url} failed after {attempt} attempt(s): {msg}"
                    );
                }
                let jitter_ms = rand::rng().random_range(0u64..=(delay.as_millis() / 4) as u64);
                let sleep_dur = delay + Duration::from_millis(jitter_ms);
                emit_warn_log_message(
                    ErrorCode::IoError,
                    format!(
                        "attempt {attempt}/{max_attempts} failed: {msg} — retrying in {}",
                        fmt_duration(sleep_dur)
                    ),
                );
                tokio::time::sleep(sleep_dur).await;
                delay = delay.saturating_mul(2);
            }
        }
    }
    unreachable!("loop always returns once max_attempts >= 1")
}

// ---------------------------------------------------------------------------
// HTTP abstraction (enables network-free testing)
// ---------------------------------------------------------------------------

/// Thin abstraction over HTTP GET, used by the update flow. `get_text`
/// comes from `VersionsHttpClient` -- dbt-dist's version-resolution logic
/// (shared with `dbt-dist`'s own upgrade flow) is generic over that trait,
/// and `UpdateHttpClient: VersionsHttpClient` lets a `&dyn UpdateHttpClient`
/// satisfy it directly, no adapter needed. The real implementation wraps
/// `reqwest` with retries; tests inject a mock.
#[async_trait]
pub trait UpdateHttpClient: VersionsHttpClient {
    async fn get_bytes(&self, url: &str) -> FsResult<Vec<u8>>;
}

/// Production implementation backed by `reqwest`.
pub struct ReqwestUpdateClient;

#[async_trait]
impl VersionsHttpClient for ReqwestUpdateClient {
    async fn get_text(&self, url: &str) -> FsResult<String> {
        fetch_with_retries(
            url,
            CDN_MAX_ATTEMPTS,
            CDN_BACKOFF_INITIAL,
            CDN_REQUEST_TIMEOUT,
            || async {
                let response = reqwest::get(url).await.map_err(|e| {
                    (
                        is_retryable_reqwest_error(&e),
                        ErrorCode::IoError,
                        format!("GET failed: {e}"),
                    )
                })?;
                let status = response.status();
                if !status.is_success() {
                    return Err((
                        is_retryable_status(status),
                        error_code_for_status(status),
                        format!("returned status {status}"),
                    ));
                }
                response.text().await.map_err(|e| {
                    (
                        is_retryable_reqwest_error(&e),
                        ErrorCode::IoError,
                        format!("failed to read response body: {e}"),
                    )
                })
            },
        )
        .await
    }
}

#[async_trait]
impl UpdateHttpClient for ReqwestUpdateClient {
    async fn get_bytes(&self, url: &str) -> FsResult<Vec<u8>> {
        fetch_with_retries(
            url,
            CDN_MAX_ATTEMPTS,
            CDN_BACKOFF_INITIAL,
            CDN_REQUEST_TIMEOUT,
            || async {
                let response = reqwest::get(url).await.map_err(|e| {
                    (
                        is_retryable_reqwest_error(&e),
                        ErrorCode::IoError,
                        format!("GET failed: {e}"),
                    )
                })?;
                let status = response.status();
                if !status.is_success() {
                    return Err((
                        is_retryable_status(status),
                        error_code_for_status(status),
                        format!("returned status {status}"),
                    ));
                }
                response.bytes().await.map(|b| b.to_vec()).map_err(|e| {
                    (
                        is_retryable_reqwest_error(&e),
                        ErrorCode::IoError,
                        format!("failed to read response bytes: {e}"),
                    )
                })
            },
        )
        .await
    }
}

/// Set `DBT_NATIVE_UPDATE=1` to use the optimized native Rust download path
/// instead of shelling out to install.sh. This reduces I/O pressure by eliminating
/// subprocess spawning, temp directories, and redundant HTTP requests.
#[cfg(not(target_os = "windows"))]
const NATIVE_UPDATE_ENV: &str = "DBT_NATIVE_UPDATE";

#[cfg(not(target_os = "windows"))]
fn use_native_update() -> bool {
    #[allow(clippy::disallowed_methods)]
    env::var(NATIVE_UPDATE_ENV).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

// ---------------------------------------------------------------------------
// Shared helpers (used by both code paths)
// ---------------------------------------------------------------------------

/// Returns the target triple for the current platform, matching the CDN naming convention.
#[doc(hidden)]
pub fn current_target_triple() -> FsResult<&'static str> {
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    {
        Ok("x86_64-unknown-linux-gnu")
    }
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    {
        Ok("aarch64-unknown-linux-gnu")
    }
    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    {
        Ok("x86_64-apple-darwin")
    }
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        Ok("aarch64-apple-darwin")
    }
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    {
        Ok("x86_64-pc-windows-msvc")
    }
    #[cfg(not(any(
        all(target_arch = "x86_64", target_os = "linux"),
        all(target_arch = "aarch64", target_os = "linux"),
        all(target_arch = "x86_64", target_os = "macos"),
        all(target_arch = "aarch64", target_os = "macos"),
        all(target_arch = "x86_64", target_os = "windows"),
    )))]
    {
        err!(ErrorCode::IoError, "Unsupported platform for system update")
    }
}

/// Check the installed version of a binary by running `<binary> --version`.
#[cfg(not(target_os = "windows"))]
fn installed_version(binary_path: &Path) -> Option<String> {
    Command::new(binary_path)
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.split_whitespace().nth(1).map(String::from)
        })
}

// ---------------------------------------------------------------------------
// Native update path (behind DBT_NATIVE_UPDATE=1)
// ---------------------------------------------------------------------------

/// Download the tarball, extract the binary, and install atomically.
#[cfg(not(target_os = "windows"))]
async fn download_and_install(
    package: &str,
    version: &str,
    target: &str,
    dest_dir: &Path,
    client: &dyn UpdateHttpClient,
) -> FsResult<()> {
    let base_url = cdn_base_url();

    let archive_prefix = match package {
        "dbt" => "fs",
        "dbt-db-runner" => "fs-db-runner",
        _ => {
            return err!(
                ErrorCode::InvalidArgument,
                "Unknown package: {package}. Expected 'dbt' or 'dbt-db-runner'."
            );
        }
    };

    let tarball_url = format!("{base_url}/cli/{archive_prefix}-v{version}-{target}.tar.gz");

    println(format!("Downloading {package} v{version} for {target}..."));

    let bytes = client.get_bytes(&tarball_url).await?;

    let reader = tokio::io::BufReader::new(bytes.as_slice());
    let decoder = GzipDecoder::new(reader);
    let mut archive = tokio_tar::Archive::new(decoder);

    let dest_binary = dest_dir.join(package);
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to create install directory {}: {e}",
            dest_dir.display()
        )
    })?;

    let tmp_file = tempfile::NamedTempFile::new_in(dest_dir).map_err(|e| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to create temp file in {}: {e}",
            dest_dir.display()
        )
    })?;

    let mut found = false;
    let mut entries = archive.entries().map_err(|e| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to read archive entries for {package} v{version}: {e}"
        )
    })?;
    while let Some(entry_result) = entries.next().await {
        let mut entry = entry_result.map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to read archive entry for {package} v{version}: {e}"
            )
        })?;
        let path = entry.path().map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to read entry path in archive for {package} v{version}: {e}"
            )
        })?;

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        if file_name != package {
            continue;
        }

        let mut tmp_writer = tokio::fs::File::create(tmp_file.path())
            .await
            .map_err(|e| {
                fs_err!(
                    ErrorCode::IoError,
                    "Failed to create temp file for {package} binary: {e}"
                )
            })?;
        tokio::io::copy(&mut entry, &mut tmp_writer)
            .await
            .map_err(|e| {
                fs_err!(
                    ErrorCode::IoError,
                    "Failed to write {package} binary to temp file: {e}"
                )
            })?;

        tmp_writer.sync_all().await.map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to sync {package} binary temp file: {e}"
            )
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(tmp_file.path(), std::fs::Permissions::from_mode(0o755))
                .map_err(|e| {
                    fs_err!(
                        ErrorCode::IoError,
                        "Failed to set permissions on {package} binary: {e}"
                    )
                })?;
        }

        found = true;
        break;
    }

    if !found {
        return err!(
            ErrorCode::IoError,
            "Binary '{package}' not found in downloaded archive"
        );
    }

    if dest_binary.exists() {
        std::fs::remove_file(&dest_binary).map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to remove existing binary {}: {e}",
                dest_binary.display()
            )
        })?;
    }

    tmp_file.persist(&dest_binary).map_err(|e| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to persist {package} binary to {}: {}",
            dest_binary.display(),
            e.error
        )
    })?;

    println(format!(
        "Successfully installed {package} v{version} to {}",
        dest_binary.display()
    ));
    Ok(())
}

/// Check installed version and download if update is needed.
#[cfg(not(target_os = "windows"))]
async fn update_package_if_needed(
    package: &str,
    version: &str,
    target: &str,
    dest_dir: &Path,
    client: &dyn UpdateHttpClient,
) -> FsResult<()> {
    match installed_version(&dest_dir.join(package)) {
        Some(v) if v == version => {
            println(format!(
                "{package} v{version} is already installed, skipping."
            ));
            return Ok(());
        }
        Some(v) => println(format!("{package} current: {v}, updating to {version}")),
        None => {}
    }
    download_and_install(package, version, target, dest_dir, client).await
}

/// Native update entry point (Unix only, gated by DBT_NATIVE_UPDATE).
///
/// Mirrors the install.sh `install_packages` logic:
///   --package dbt     → install dbt; update the runner only when already present (default)
///   --package all     → install dbt and its companion runner
#[cfg(not(target_os = "windows"))]
#[doc(hidden)]
pub async fn exec_update_native(
    version: Option<&str>,
    package: Option<&str>,
    dest_dir: &Path,
    client: &dyn UpdateHttpClient,
) -> FsResult<()> {
    let target = current_target_triple()?;
    let package = package.unwrap_or("dbt");

    println(format!("Updating {package} for {target} (native)"));

    let target_version = resolve_target_version(version, client).await?;
    println(format!("Target version: {target_version}"));

    let install_dbt = package == "dbt" || package == "all";

    if !install_dbt {
        return err!(
            ErrorCode::InvalidArgument,
            "Unknown package: {package}. Expected 'dbt' or 'all'."
        );
    }

    if install_dbt {
        let runner_path = dest_dir.join("dbt-db-runner");
        if (package == "all" || runner_path.exists())
            && let Err(error) =
                update_package_if_needed("dbt-db-runner", &target_version, target, dest_dir, client)
                    .await
        {
            if error.code != ErrorCode::FileNotFound {
                return Err(error);
            }
            if runner_path.exists() {
                std::fs::remove_file(&runner_path).map_err(|remove_error| {
                    fs_err!(
                        ErrorCode::IoError,
                        "Failed to remove unavailable dbt-db-runner at {}: {remove_error}",
                        runner_path.display()
                    )
                })?;
            }
            println(format!(
                "dbt-db-runner is not available for version {target_version}; installing dbt without it."
            ));
        }
        update_package_if_needed("dbt", &target_version, target, dest_dir, client).await?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Legacy update path (default -- shells out to install.sh / install.ps1)
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "windows"))]
async fn exec_update_legacy(
    version: Option<&str>,
    package: Option<&str>,
    curr_path: &str,
    client: &dyn UpdateHttpClient,
) -> FsResult<()> {
    let script_url = format!("{}/install/install.sh", cdn_base_url());
    let script = client.get_text(&script_url).await?;

    let options = ScriptOptions::new();

    let mut script_args = vec![
        String::from("--update"),
        String::from("--to"),
        curr_path.to_string(),
    ];

    if let Some(version) = version {
        script_args.push(String::from("--version"));
        script_args.push(version.to_string());
    }

    if let Some(package) = package {
        script_args.push(String::from("--package"));
        script_args.push(package.to_string());
    }

    let (code, output, error) = match run_script::run(&script, &script_args, &options) {
        Ok(result) => result,
        Err(e) => {
            return err!(ErrorCode::IoError, "Failed to run install script: {}", e);
        }
    };

    let output = output.trim();
    let error = error.trim();

    if code != 0 {
        let err_msg = if !error.is_empty() {
            format!("{error}\nFailed to update dbt: {output}")
        } else {
            output.to_string()
        };
        return err!(ErrorCode::IoError, "{}", err_msg);
    } else {
        println(output);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

#[tracing::instrument(
    skip_all,
    fields(
        _e = ?store_event_attributes(PackageUpdate {
            version: version.unwrap_or_default().to_string(),
            package: package.unwrap_or("dbt").to_string(),
            exe_path: None,
        }),
    )
)]
pub async fn exec_update(
    version: Option<&str>,
    package: Option<&str>,
    force: bool,
    command_name: &str,
) -> FsResult<()> {
    exec_update_with_client(version, package, force, command_name, &ReqwestUpdateClient).await
}

fn validate_update_package(package: Option<&str>) -> FsResult<()> {
    if let Some(package) = package {
        if package == "dbt-lsp" {
            return err!(
                ErrorCode::InvalidArgument,
                "The standalone dbt-lsp package is no longer published. Update dbt and run `dbt lsp` instead."
            );
        }
        if package != "dbt" && package != "all" {
            return err!(
                ErrorCode::InvalidArgument,
                "Unknown package: {package}. Expected 'dbt' or 'all'."
            );
        }
    }
    Ok(())
}

/// Returns the error message explaining why an in-place self-update is refused,
/// or `None` if the update may proceed. Self-update is blocked when the binary
/// is owned by a package manager, unless `force` is set.
fn blocked_self_update_message(dist_info: &dbt_dist::DistInfo, force: bool) -> Option<String> {
    if dist_info.is_self_managed() || force {
        return None;
    }
    if let Some(message) = dist_info.unsupported_channel_message("update") {
        return Some(message);
    }
    let message = match &dist_info.upgrade_cmd {
        Some(command) => format!(
            "dbt was installed via {}. To upgrade, run:\n\n    {}\n\n\
             (Self-updating here would overwrite the binary {} manages. \
             Pass --force to self-update anyway.)",
            dist_info.install_label(),
            command,
            dist_info.install_label(),
        ),
        None => "dbt was installed by another package manager, so it can't self-update. \
             Please upgrade dbt using the package manager you installed it with, \
             or pass --force to self-update anyway."
            .to_string(),
    };
    Some(message)
}

/// Inner implementation that accepts an injectable HTTP client.
/// Production code calls this via `exec_update`; tests inject a mock.
pub(crate) async fn exec_update_with_client(
    version: Option<&str>,
    package: Option<&str>,
    force: bool,
    command_name: &str,
    client: &dyn UpdateHttpClient,
) -> FsResult<()> {
    validate_update_package(package)?;

    let dist_info = dbt_dist::DistInfo::current(command_name)?;
    if let Some(message) = blocked_self_update_message(&dist_info, force) {
        return err!(ErrorCode::NotSupported, "{}", message);
    }

    let curr_path = match env::current_exe() {
        Ok(exe_path) => {
            if let Some(parent_dir) = exe_path.parent() {
                match parent_dir.to_str() {
                    Some(dir_str) => dir_str.to_string(),
                    None => {
                        return err!(
                            ErrorCode::IoError,
                            "Error: Failed to convert exe parent path to string."
                        );
                    }
                }
            } else {
                return err!(
                    ErrorCode::IoError,
                    "Error: Failed to get parent directory of executable."
                );
            }
        }
        Err(e) => {
            return err!(
                ErrorCode::IoError,
                "Error: Failed to get current exe path. {e}"
            );
        }
    };

    #[cfg(not(target_os = "windows"))]
    {
        if use_native_update() {
            println(format!("Using native update path ({NATIVE_UPDATE_ENV}=1)"));
            let dest_dir = std::path::PathBuf::from(&curr_path);
            return exec_update_native(version, package, &dest_dir, client).await;
        }

        println(format!("ANALYZING Current exe at {curr_path}/"));

        #[cfg(target_arch = "x86_64")]
        #[cfg(target_os = "linux")]
        println("Updating Binary for Linux X86-64");

        #[cfg(target_arch = "x86_64")]
        #[cfg(target_os = "macos")]
        println("Updating Binary for Mac OS X86-64");

        #[cfg(target_arch = "aarch64")]
        #[cfg(target_os = "macos")]
        println("Updating Binary for Mac OS AARCH-64");

        #[cfg(target_arch = "aarch64")]
        #[cfg(target_os = "linux")]
        println("Updating Binary for Linux AARCH-64");

        return exec_update_legacy(version, package, &curr_path, client).await;
    }

    #[cfg(target_os = "windows")]
    {
        println(format!("ANALYZING Current exe at {curr_path}/"));
        println("Updating Binary for Windows X86-64");
        exec_update_windows(version, &curr_path, client).await
    }
}

#[cfg(target_os = "windows")]
async fn exec_update_windows(
    version: Option<&str>,
    curr_path: &str,
    client: &dyn UpdateHttpClient,
) -> FsResult<()> {
    let script_url = format!("{}/install/install.ps1", cdn_base_url());
    let script = client.get_text(&script_url).await?;

    let temp_dir = env::temp_dir();
    let unique_id = Uuid::new_v4().to_string();
    let script_path = temp_dir.join(format!("install_{unique_id}.ps1"));

    let mut file = match File::create(&script_path) {
        Ok(file) => file,
        Err(e) => {
            return err!(
                ErrorCode::IoError,
                "Failed to create temporary script file: {}",
                e
            );
        }
    };

    if let Err(e) = file.write_all(script.as_bytes()) {
        return err!(
            ErrorCode::IoError,
            "Failed to write to temporary script file: {}",
            e
        );
    }

    drop(file);

    let path_str = script_path
        .to_string_lossy()
        .to_string()
        .replace("\\", "\\\\");

    println("Update process started. The dbt executable will be updated after this process exits.");

    let ps_command = format!(
        "& '{}' -Update -To '{}'{}",
        path_str.replace("\\", "\\\\"),
        curr_path.replace("\\", "\\\\"),
        version.map_or(String::new(), |v| format!(" -Version '{v}'"))
    );

    let ps_exe = if env::var("PSModulePath").is_ok_and(|path| path.contains("PowerShell/7")) {
        "pwsh"
    } else {
        "powershell"
    };

    match Command::new(ps_exe)
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &ps_command,
        ])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(_child) => {
            std::thread::sleep(Duration::from_millis(100));
            std::process::exit(0);
        }
        Err(e) => {
            err!(ErrorCode::IoError, "Failed to start update process: {}", e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use dbt_common::{ErrorCode, FsResult, err};
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MockHttpClient {
        responses: HashMap<String, Vec<u8>>,
        errors: HashMap<String, ErrorCode>,
        requests: Mutex<Vec<String>>,
    }

    impl MockHttpClient {
        fn new() -> Self {
            Self {
                responses: HashMap::new(),
                errors: HashMap::new(),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn with_text(mut self, url: impl Into<String>, body: &str) -> Self {
            self.responses.insert(url.into(), body.as_bytes().to_vec());
            self
        }

        fn with_error(mut self, url: impl Into<String>, code: ErrorCode) -> Self {
            self.errors.insert(url.into(), code);
            self
        }

        #[cfg(not(target_os = "windows"))]
        fn with_bytes(mut self, url: impl Into<String>, data: Vec<u8>) -> Self {
            self.responses.insert(url.into(), data);
            self
        }

        #[cfg(not(target_os = "windows"))]
        fn requested_urls(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl VersionsHttpClient for MockHttpClient {
        async fn get_text(&self, url: &str) -> FsResult<String> {
            self.requests.lock().unwrap().push(url.to_string());
            if let Some(code) = self.errors.get(url) {
                return err!(*code, "MockHttpClient: injected error for {url}");
            }
            match self.responses.get(url) {
                Some(bytes) => Ok(String::from_utf8(bytes.clone()).unwrap()),
                None => err!(ErrorCode::IoError, "MockHttpClient: no response for {url}"),
            }
        }
    }

    #[async_trait]
    impl UpdateHttpClient for MockHttpClient {
        async fn get_bytes(&self, url: &str) -> FsResult<Vec<u8>> {
            self.requests.lock().unwrap().push(url.to_string());
            if let Some(code) = self.errors.get(url) {
                return err!(*code, "MockHttpClient: injected error for {url}");
            }
            match self.responses.get(url) {
                Some(bytes) => Ok(bytes.clone()),
                None => err!(ErrorCode::IoError, "MockHttpClient: no response for {url}"),
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn build_fake_tarball(binary_name: &str, content: &[u8]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, binary_name, content)
            .unwrap();
        let tar_bytes = builder.into_inner().unwrap();

        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        std::io::Write::write_all(&mut encoder, &tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[cfg(not(target_os = "windows"))]
    fn native_update_client(
        version: &str,
        dbt_tarball: Vec<u8>,
        runner_tarball: Vec<u8>,
    ) -> MockHttpClient {
        let target = current_target_triple().unwrap();
        let base = cdn_base_url();
        let manifest = serde_json::to_string(&test_versions_json()).unwrap();

        MockHttpClient::new()
            .with_text(format!("{base}/versions.json"), &manifest)
            .with_bytes(
                format!("{base}/cli/fs-db-runner-v{version}-{target}.tar.gz"),
                runner_tarball,
            )
            .with_bytes(
                format!("{base}/cli/fs-v{version}-{target}.tar.gz"),
                dbt_tarball,
            )
    }

    fn test_versions_json() -> serde_json::Value {
        serde_json::json!({
            "latest":  { "tag": "v2.0.0-preview.154", "date": "2026-03-13" },
            "dev":     { "tag": "v2.0.0-preview.157", "date": "2026-03-18" },
            "canary":  { "tag": "v2.0.0-preview.157", "date": "2026-03-18" }
        })
    }

    #[test]
    fn test_current_target_triple_returns_known_triple() {
        let triple = current_target_triple().unwrap();
        let known = [
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
            "x86_64-pc-windows-msvc",
        ];
        assert!(
            known.contains(&triple),
            "unexpected target triple: {triple}"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_installed_version_nonexistent_binary() {
        let result = installed_version(Path::new("/nonexistent/binary/dbt"));
        assert!(result.is_none());
    }

    #[dbt_runtime::test]
    async fn test_resolve_target_version_via_mock() {
        let manifest = serde_json::to_string(&test_versions_json()).unwrap();
        let client =
            MockHttpClient::new().with_text(format!("{}/versions.json", cdn_base_url()), &manifest);

        let version = resolve_target_version(None, &client).await.unwrap();
        assert_eq!(version, "2.0.0-preview.154");
    }

    #[dbt_runtime::test]
    async fn test_resolve_target_version_with_alias() {
        let manifest = serde_json::to_string(&test_versions_json()).unwrap();
        let client =
            MockHttpClient::new().with_text(format!("{}/versions.json", cdn_base_url()), &manifest);

        let version = Some("canary");
        let version = resolve_target_version(version, &client).await.unwrap();
        assert_eq!(version, "2.0.0-preview.157");
    }

    #[cfg(not(target_os = "windows"))]
    #[dbt_runtime::test]
    async fn test_native_update_installs_only_dbt_by_default() {
        let version = "2.0.0-preview.154";
        let binary_content = b"#!/bin/sh\necho fake-dbt";

        let client = native_update_client(
            version,
            build_fake_tarball("dbt", binary_content),
            build_fake_tarball("dbt-db-runner", b"runner-binary"),
        );

        let tmp = tempfile::tempdir().unwrap();
        exec_update_native(None, Some("dbt"), tmp.path(), &client)
            .await
            .unwrap();

        let installed = tmp.path().join("dbt");
        assert!(installed.exists(), "binary should be installed");
        assert_eq!(std::fs::read(&installed).unwrap(), binary_content);
        assert!(!tmp.path().join("dbt-db-runner").exists());

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&installed).unwrap().permissions().mode();
        assert_ne!(mode & 0o111, 0, "binary should be executable");
    }

    #[cfg(not(target_os = "windows"))]
    #[dbt_runtime::test]
    async fn test_native_update_all_installs_dbt_and_runner() {
        let version = "2.0.0-preview.157";

        let dbt_tarball = build_fake_tarball("dbt", b"dbt-binary");
        let runner_tarball = build_fake_tarball("dbt-db-runner", b"runner-binary");

        let client = native_update_client(version, dbt_tarball, runner_tarball);

        let tmp = tempfile::tempdir().unwrap();
        exec_update_native(Some("canary"), Some("all"), tmp.path(), &client)
            .await
            .unwrap();

        assert!(tmp.path().join("dbt").exists(), "dbt should be installed");
        assert!(
            tmp.path().join("dbt-db-runner").exists(),
            "dbt-db-runner should be installed"
        );
        assert!(
            !tmp.path().join("dbt-lsp").exists(),
            "dbt-lsp should not be installed"
        );
        assert_eq!(
            std::fs::read(tmp.path().join("dbt")).unwrap(),
            b"dbt-binary"
        );
        assert_eq!(
            client.requested_urls(),
            vec![
                format!("{}/versions.json", cdn_base_url()),
                format!(
                    "{}/cli/fs-db-runner-v{version}-{}.tar.gz",
                    cdn_base_url(),
                    current_target_triple().unwrap()
                ),
                format!(
                    "{}/cli/fs-v{version}-{}.tar.gz",
                    cdn_base_url(),
                    current_target_triple().unwrap()
                ),
            ]
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[dbt_runtime::test]
    async fn test_native_update_allows_literal_version_without_runner() {
        let version = "2.0.0-preview.100";
        let target = current_target_triple().unwrap();
        let base = cdn_base_url();
        let versions_url = format!("{base}/versions.json");
        let runner_url = format!("{base}/cli/fs-db-runner-v{version}-{target}.tar.gz");
        let dbt_url = format!("{base}/cli/fs-v{version}-{target}.tar.gz");
        let manifest = serde_json::to_string(&test_versions_json()).unwrap();

        let client = MockHttpClient::new()
            .with_text(&versions_url, &manifest)
            .with_error(&runner_url, ErrorCode::FileNotFound)
            .with_bytes(&dbt_url, build_fake_tarball("dbt", b"dbt-binary"));

        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("dbt-db-runner"), b"stale-runner").unwrap();
        exec_update_native(Some(version), Some("dbt"), tmp.path(), &client)
            .await
            .unwrap();

        assert!(tmp.path().join("dbt").exists());
        assert!(!tmp.path().join("dbt-db-runner").exists());
        assert_eq!(
            client.requested_urls(),
            vec![versions_url, runner_url, dbt_url]
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[dbt_runtime::test]
    async fn test_native_update_allows_alias_without_runner() {
        let version = "2.0.0-preview.194";
        let target = current_target_triple().unwrap();
        let base = cdn_base_url();
        let versions_url = format!("{base}/versions.json");
        let runner_url = format!("{base}/cli/fs-db-runner-v{version}-{target}.tar.gz");
        let dbt_url = format!("{base}/cli/fs-v{version}-{target}.tar.gz");
        let manifest = serde_json::json!({
            "extended": { "tag": format!("v{version}"), "date": "2026-07-07" }
        })
        .to_string();

        let client = MockHttpClient::new()
            .with_text(&versions_url, &manifest)
            .with_error(&runner_url, ErrorCode::FileNotFound)
            .with_bytes(&dbt_url, build_fake_tarball("dbt", b"dbt-binary"));

        let tmp = tempfile::tempdir().unwrap();
        exec_update_native(Some("extended"), Some("all"), tmp.path(), &client)
            .await
            .unwrap();

        assert!(tmp.path().join("dbt").exists());
        assert!(!tmp.path().join("dbt-db-runner").exists());
        assert_eq!(
            client.requested_urls(),
            vec![versions_url, runner_url, dbt_url]
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[dbt_runtime::test]
    async fn test_native_update_propagates_non_404_runner_failure() {
        let version = "2.0.0-preview.194";
        let target = current_target_triple().unwrap();
        let base = cdn_base_url();
        let versions_url = format!("{base}/versions.json");
        let runner_url = format!("{base}/cli/fs-db-runner-v{version}-{target}.tar.gz");
        let manifest = serde_json::to_string(&test_versions_json()).unwrap();

        let client = MockHttpClient::new()
            .with_text(&versions_url, &manifest)
            .with_error(&runner_url, ErrorCode::IoError);

        let tmp = tempfile::tempdir().unwrap();
        let runner_path = tmp.path().join("dbt-db-runner");
        std::fs::write(&runner_path, b"existing-runner").unwrap();

        let result = exec_update_native(Some(version), Some("dbt"), tmp.path(), &client).await;

        assert!(result.is_err());
        assert_eq!(std::fs::read(runner_path).unwrap(), b"existing-runner");
    }

    fn dist_info_for_test(
        channel: Option<dbt_dist::Channel>,
        upgrade_cmd: Option<&str>,
    ) -> dbt_dist::DistInfo {
        dbt_dist::DistInfo {
            path: "/usr/local/bin/dbt".to_string(),
            channel,
            distribution: None,
            generation: dbt_dist::Generation::V2,
            py_package_manager: None,
            py_venv_root: None,
            version: None,
            is_prerelease: None,
            upgrade_cmd: upgrade_cmd.map(str::to_string),
            uninstall_cmd: None,
        }
    }

    #[test]
    fn test_blocked_self_update_message() {
        use dbt_dist::Channel;

        // Self-managed installs are never blocked.
        for channel in [Channel::Standalone, Channel::Unclaimed] {
            let info = dist_info_for_test(Some(channel), Some("dbt system update"));
            assert!(blocked_self_update_message(&info, false).is_none());
        }

        // Package-manager installs are blocked with a channel-specific command.
        let info = dist_info_for_test(Some(Channel::Brew), Some("brew upgrade dbt"));
        let msg = blocked_self_update_message(&info, false).unwrap();
        assert!(msg.contains("brew upgrade dbt"), "got: {msg}");
        assert!(msg.contains("--force"), "got: {msg}");

        // An unresolved channel has no command but still mentions --force.
        let info = dist_info_for_test(None, None);
        let msg = blocked_self_update_message(&info, false).unwrap();
        assert!(msg.contains("--force"), "got: {msg}");

        // A recognized-but-unsupported manager names itself and points at
        // the install docs instead of a specific command.
        let info = dist_info_for_test(Some(Channel::Unsupported("Chocolatey".to_string())), None);
        let msg = blocked_self_update_message(&info, false).unwrap();
        assert!(msg.contains("Chocolatey"), "got: {msg}");
        assert!(
            msg.contains("https://docs.getdbt.com/docs/local/install-dbt?version=2"),
            "got: {msg}"
        );

        // --force overrides the block for every channel.
        for (channel, command) in [
            (Channel::Brew, "brew upgrade dbt"),
            (Channel::Pypi, "pip install --upgrade dbt"),
            (Channel::Winget, "winget upgrade --id dbtLabs.dbt --exact"),
        ] {
            let info = dist_info_for_test(Some(channel.clone()), Some(command));
            assert!(
                blocked_self_update_message(&info, true).is_none(),
                "--force should bypass block for {channel:?}"
            );
        }
        let info = dist_info_for_test(None, None);
        assert!(blocked_self_update_message(&info, true).is_none());
    }

    #[test]
    fn test_update_dbt_lsp_package_errors() {
        let result = validate_update_package(Some("dbt-lsp"));
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("standalone dbt-lsp package is no longer published"),
            "expected dbt-lsp package error, got: {err_msg}"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[dbt_runtime::test]
    async fn test_native_update_unknown_package_errors() {
        let manifest = serde_json::to_string(&test_versions_json()).unwrap();
        let client =
            MockHttpClient::new().with_text(format!("{}/versions.json", cdn_base_url()), &manifest);

        let tmp = tempfile::tempdir().unwrap();
        let result = exec_update_native(None, Some("bogus"), tmp.path(), &client).await;
        assert!(result.is_err());
    }

    #[cfg(not(target_os = "windows"))]
    #[dbt_runtime::test]
    async fn test_native_update_missing_binary_in_archive_errors() {
        let version = "2.0.0-preview.154";
        let client = native_update_client(
            version,
            build_fake_tarball("wrong-name", b"data"),
            build_fake_tarball("dbt-db-runner", b"runner-binary"),
        );

        let tmp = tempfile::tempdir().unwrap();
        let result = exec_update_native(None, Some("dbt"), tmp.path(), &client).await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("not found in downloaded archive"),
            "expected 'not found' error, got: {err_msg}"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[dbt_runtime::test]
    async fn test_native_update_overwrites_existing_binaries() {
        let version = "2.0.0-preview.154";

        let client = native_update_client(
            version,
            build_fake_tarball("dbt", b"new-binary"),
            build_fake_tarball("dbt-db-runner", b"runner-binary"),
        );

        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("dbt"), b"old-binary").unwrap();
        std::fs::write(tmp.path().join("dbt-db-runner"), b"old-runner").unwrap();

        exec_update_native(None, Some("dbt"), tmp.path(), &client)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(tmp.path().join("dbt")).unwrap(),
            b"new-binary"
        );
        assert_eq!(
            std::fs::read(tmp.path().join("dbt-db-runner")).unwrap(),
            b"runner-binary"
        );
    }

    #[dbt_runtime::test]
    async fn test_manifest_fetch_failure_propagates() {
        let client = MockHttpClient::new();
        let error = resolve_target_version(None, &client).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::IoError);
    }

    // Generous per-attempt timeout for unit tests — we never want the test
    // to actually exercise the timeout path unless explicitly testing it.
    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    #[dbt_runtime::test]
    async fn test_retries_succeeds_after_transient_failure() {
        let mut attempts: u32 = 0;
        let result: FsResult<&'static str> = fetch_with_retries(
            "http://test",
            3,
            Duration::from_millis(1),
            TEST_TIMEOUT,
            || {
                attempts += 1;
                let n = attempts;
                async move {
                    if n < 3 {
                        Err((
                            true,
                            ErrorCode::IoError,
                            format!("simulated transient failure {n}"),
                        ))
                    } else {
                        Ok("ok")
                    }
                }
            },
        )
        .await;
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(attempts, 3);
    }

    #[dbt_runtime::test]
    async fn test_retries_gives_up_after_max_attempts() {
        let mut attempts: u32 = 0;
        let result: FsResult<()> = fetch_with_retries(
            "http://test",
            3,
            Duration::from_millis(1),
            TEST_TIMEOUT,
            || {
                attempts += 1;
                async { Err((true, ErrorCode::IoError, "still failing".to_string())) }
            },
        )
        .await;
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("after 3 attempt"),
            "expected exhaustion message, got: {msg}"
        );
        assert_eq!(attempts, 3);
    }

    #[dbt_runtime::test]
    async fn test_retries_does_not_retry_on_non_retryable_error() {
        let mut attempts: u32 = 0;
        let result: FsResult<()> = fetch_with_retries(
            "http://test",
            5,
            Duration::from_millis(1),
            TEST_TIMEOUT,
            || {
                attempts += 1;
                async { Err((false, ErrorCode::FileNotFound, "404 not found".to_string())) }
            },
        )
        .await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::FileNotFound);
        let msg = format!("{error}");
        assert!(msg.contains("404 not found"), "got: {msg}");
        assert_eq!(attempts, 1, "non-retryable error should not retry");
    }

    #[dbt_runtime::test]
    async fn test_retries_treats_per_attempt_timeout_as_transient() {
        // A hung fetch is bounded by request_timeout per attempt and counts as
        // a retryable failure. The inner sleep is canceled when the per-attempt
        // timeout fires, so total wall-clock is ~3 * 50ms regardless of the
        // declared inner sleep.
        let mut attempts: u32 = 0;
        let result: FsResult<()> = fetch_with_retries(
            "http://test",
            3,
            Duration::from_millis(1),
            Duration::from_millis(50),
            || {
                attempts += 1;
                async {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok(())
                }
            },
        )
        .await;
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("timed out"), "got: {msg}");
        assert_eq!(attempts, 3, "timeout should be retried up to max_attempts");
    }

    #[test]
    fn test_is_retryable_status_classifies_correctly() {
        assert!(is_retryable_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(is_retryable_status(reqwest::StatusCode::BAD_GATEWAY));
        assert!(is_retryable_status(
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(is_retryable_status(reqwest::StatusCode::GATEWAY_TIMEOUT));
        assert!(is_retryable_status(reqwest::StatusCode::TOO_MANY_REQUESTS));
        assert!(!is_retryable_status(reqwest::StatusCode::NOT_FOUND));
        assert!(!is_retryable_status(reqwest::StatusCode::FORBIDDEN));
        assert!(!is_retryable_status(reqwest::StatusCode::BAD_REQUEST));
        assert_eq!(
            error_code_for_status(reqwest::StatusCode::NOT_FOUND),
            ErrorCode::FileNotFound
        );
        assert_eq!(
            error_code_for_status(reqwest::StatusCode::SERVICE_UNAVAILABLE),
            ErrorCode::IoError
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[dbt_runtime::test]
    async fn test_native_update_records_correct_urls() {
        let version = "2.0.0-preview.154";
        let target = current_target_triple().unwrap();
        let base = cdn_base_url();

        let manifest = serde_json::to_string(&test_versions_json()).unwrap();
        let versions_url = format!("{base}/versions.json");
        let dbt_url = format!("{base}/cli/fs-v{version}-{target}.tar.gz");

        let client = MockHttpClient::new()
            .with_text(&versions_url, &manifest)
            .with_bytes(&dbt_url, build_fake_tarball("dbt", b"bin"));

        let tmp = tempfile::tempdir().unwrap();
        exec_update_native(None, Some("dbt"), tmp.path(), &client)
            .await
            .unwrap();

        let urls = client.requested_urls();
        assert_eq!(urls, vec![versions_url, dbt_url]);
    }
}
