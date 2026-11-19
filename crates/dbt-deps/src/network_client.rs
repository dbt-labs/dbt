use dbt_common::ErrorCode;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use reqwest::{Certificate, Client, IntoUrl};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware, RequestBuilder};
use reqwest_retry::{
    RetryTransientMiddleware, policies::ExponentialBackoff as RetryExponentialBackoff,
};
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::OnceCell;

const MAX_CLIENT_RETRIES: u32 = 3;

/// requests' extra CA bundle: a PEM file or a directory of them.
const REQUESTS_CA_BUNDLE_ENV: &str = "REQUESTS_CA_BUNDLE";

/// Fallback for `REQUESTS_CA_BUNDLE`, read only when it is unset or empty.
const CURL_CA_BUNDLE_ENV: &str = "CURL_CA_BUNDLE";

/// The retrying HTTP client, built on first use. Clones share one instance, so
/// the CA bundle is read at most once per run, and only if something does
/// network IO.
#[derive(Clone, Default)]
pub struct LazyHttpClient {
    inner: Arc<OnceCell<ClientWithMiddleware>>,
}

impl LazyHttpClient {
    /// Nothing is built until the first [`client`](Self::client).
    pub fn new() -> Self {
        Self::default()
    }

    /// Wrap an already-built client.
    pub fn from_existing(client: ClientWithMiddleware) -> Self {
        Self {
            inner: Arc::new(OnceCell::new_with(Some(client))),
        }
    }

    /// Start a GET request, building the client on first use.
    pub async fn get(&self, url: impl IntoUrl) -> RequestBuilder {
        self.client().await.get(url)
    }

    /// Start a POST request, building the client on first use.
    pub async fn post(&self, url: impl IntoUrl) -> RequestBuilder {
        self.client().await.post(url)
    }

    /// The client, built on first use on the blocking pool.
    pub async fn client(&self) -> &ClientWithMiddleware {
        self.client_with(retrying_http_client).await
    }

    /// Build with `build` on the blocking pool: it reads the CA bundle and
    /// loads the OS root store.
    async fn client_with(
        &self,
        build: impl FnOnce() -> ClientWithMiddleware + Send + 'static,
    ) -> &ClientWithMiddleware {
        self.inner
            .get_or_init(|| async {
                // The subscriber is global; the span keeps the warnings' parent context.
                let span = tracing::Span::current();
                match tokio::task::spawn_blocking(move || span.in_scope(build)).await {
                    Ok(client) => client,
                    Err(source) => {
                        // Only a panic in `build` or runtime shutdown lands here.
                        emit_warn_log_message(
                            ErrorCode::NetworkError,
                            format!(
                                "could not build the HTTP client on the blocking pool: {source}; \
                                using the default roots"
                            ),
                        );
                        with_retries(Client::new())
                    }
                }
            })
            .await
    }
}

/// Build the retrying HTTP client used for hub, tarball, and git-host
/// requests, trusting the default roots plus any from `REQUESTS_CA_BUNDLE`
/// or `CURL_CA_BUNDLE`. An unusable bundle, including one whose certificates
/// fail to build, is reported as a warning and skipped.
pub(crate) fn retrying_http_client() -> ClientWithMiddleware {
    retrying_http_client_with_env(|name| env::var_os(name))
}

fn retrying_http_client_with_env(
    get_env: impl Fn(&str) -> Option<OsString>,
) -> ClientWithMiddleware {
    let (bundle_source, certs) = match requests_ca_bundle(&get_env) {
        Some((env_var, path, certs)) => (Some((env_var, path)), certs),
        None => (None, Vec::new()),
    };
    let mut client_builder = Client::builder();
    for cert in certs {
        client_builder = client_builder.add_root_certificate(cert);
    }
    // rustls parses the certificates here, so invalid ones only fail at build.
    let client = match client_builder.build() {
        Ok(client) => client,
        Err(source) => {
            if let Some((env_var, path)) = &bundle_source {
                let reason = match std::error::Error::source(&source) {
                    Some(cause) => format!("{source}: {cause}"),
                    None => source.to_string(),
                };
                warn_unusable_ca_bundle(env_var, path, reason);
            }
            Client::new()
        }
    };
    with_retries(client)
}

fn with_retries(client: Client) -> ClientWithMiddleware {
    let retry_policy =
        RetryExponentialBackoff::builder().build_with_max_retries(MAX_CLIENT_RETRIES);
    ClientBuilder::new(client)
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build()
}

/// The first non-empty of `REQUESTS_CA_BUNDLE`, then `CURL_CA_BUNDLE`,
/// matching requests' own precedence between the two.
fn requests_ca_bundle_path(
    get_env: &impl Fn(&str) -> Option<OsString>,
) -> Option<(&'static str, PathBuf)> {
    non_empty_env_path(get_env, REQUESTS_CA_BUNDLE_ENV)
        .map(|path| (REQUESTS_CA_BUNDLE_ENV, path))
        .or_else(|| {
            non_empty_env_path(get_env, CURL_CA_BUNDLE_ENV).map(|path| (CURL_CA_BUNDLE_ENV, path))
        })
}

fn non_empty_env_path(
    get_env: &impl Fn(&str) -> Option<OsString>,
    env_var: &str,
) -> Option<PathBuf> {
    let value = get_env(env_var)?;
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

/// Extra roots from `REQUESTS_CA_BUNDLE` or `CURL_CA_BUNDLE`, with the variable
/// and path they came from. A set but unusable path is reported as a warning
/// and yields no certificates; this never fails the run.
fn requests_ca_bundle(
    get_env: &impl Fn(&str) -> Option<OsString>,
) -> Option<(&'static str, PathBuf, Vec<Certificate>)> {
    let (env_var, path) = requests_ca_bundle_path(get_env)?;
    let certs = read_ca_bundle(env_var, &path);
    Some((env_var, path, certs))
}

fn read_ca_bundle(env_var: &'static str, path: &Path) -> Vec<Certificate> {
    if path.is_dir() {
        let entries = match std::fs::read_dir(path) {
            Ok(entries) => entries,
            Err(source) => {
                warn_unusable_ca_bundle(env_var, path, source);
                return Vec::new();
            }
        };
        let files: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|p| p.is_file())
            .collect();
        if files.is_empty() {
            emit_warn_log_message(
                ErrorCode::CaBundleUnusable,
                format!(
                    "{env_var} points at {} but it contains no certificate files; \
                    it will not be trusted",
                    path.display()
                ),
            );
        }
        files
            .iter()
            .flat_map(|file| read_ca_bundle_file(env_var, file))
            .collect()
    } else {
        // A FIFO or `/dev/stdin` would block `read` forever.
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => read_ca_bundle_file(env_var, path),
            Ok(_) => {
                warn_unusable_ca_bundle(env_var, path, "not a regular file");
                Vec::new()
            }
            Err(source) => {
                warn_unusable_ca_bundle(env_var, path, source);
                Vec::new()
            }
        }
    }
}

fn read_ca_bundle_file(env_var: &'static str, path: &Path) -> Vec<Certificate> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) => {
            warn_unusable_ca_bundle(env_var, path, source);
            return Vec::new();
        }
    };
    match Certificate::from_pem_bundle(&bytes) {
        Ok(certs) if !certs.is_empty() => certs,
        Ok(_) => {
            emit_warn_log_message(
                ErrorCode::CaBundleUnusable,
                format!(
                    "{env_var} points at {} but it contains no certificates; \
                    it will not be trusted",
                    path.display()
                ),
            );
            Vec::new()
        }
        Err(source) => {
            warn_unusable_ca_bundle(env_var, path, source);
            Vec::new()
        }
    }
}

fn warn_unusable_ca_bundle(env_var: &'static str, path: &Path, source: impl std::fmt::Display) {
    emit_warn_log_message(
        ErrorCode::CaBundleUnusable,
        format!(
            "could not use the CA bundle at {} (set via {env_var}): {source}; \
            it will not be trusted",
            path.display()
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsStr;
    use std::io::Write;

    // A throwaway self-signed cert; only its PEM structure is exercised here,
    // not the chain of trust.
    const TEST_CA_PEM: &str = "-----BEGIN CERTIFICATE-----
MIICtDCCAZwCCQD4ESq+sOx/iDANBgkqhkiG9w0BAQsFADAbMRkwFwYDVQQDDBBk
YnQtZGVwcy10ZXN0LWNhMCAXDTI2MTAwNTE4NDIzNFoYDzIxMjYwOTExMTg0MjM0
WjAbMRkwFwYDVQQDDBBkYnQtZGVwcy10ZXN0LWNhMIIBIjANBgkqhkiG9w0BAQEF
AAOCAQ8AMIIBCgKCAQEAxnmlnYOeeZvN2HDYO0WATEOFNQ1wvBkedWifV7HHnyvc
laOX5FtgJ6qjgVsW9HBpLUOx2giZvk00uN5DRgZjQXtX/+6q12GBP7/9PAwwCXJf
meg31DY6UT5R6GrKdPFprtdtZpIUmEatpIOKqabi6c3hiHIBG9ggmCz4ms8WNQnV
Qt9otNJQ+RWucH44PcugPk+QQLK9rp8gnDc6FLUquNLbeUAD4GYhEVxIa7kaDQql
2vTne0Ee/4P24rWMH5er/rgP+Ro+huKTpuNHNbDWGPdp/oTcnRoTf7bhZffGb2LA
EpGxwp6gRZa1qxYaNdC/6PWBwpKjdTqbPOF1LDKm8QIDAQABMA0GCSqGSIb3DQEB
CwUAA4IBAQBdiAuKoAjWplCWW9VTo4/gqaDCYQMUTP386YlQ79VIDXNp8vmrc+CE
VGBwhreNYOxpWrRhVwrck07AvSanyFYlzYKSRLkbrSWWDNZRllm4o4VXO2yd9Y39
JD++3fydV9cbs+SBfEXYx5Shl+d9UMvvww/jPuQ9sY1L6+ZovZAntXHROsJrERWA
Ozo/nWcETJethMR8C2ROeRv7ixQzYqKCouztzj7hUTHUnLn/bDD6geE+oKrMvCT/
hK6KvglWXvMZMiIO2EZxXbpN4CwkDZmjMBIE2/P6dcj22WV3jpM8nqE60RXzoCgJ
MFgx6QM6GqRxycDQporNH6e3HuCoobF9
-----END CERTIFICATE-----
";

    fn env(vars: &[(&str, &OsStr)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_os_string()))
            .collect();
        move |name: &str| map.get(name).map(|value| value.to_os_string())
    }

    fn bundle_certs(get_env: impl Fn(&str) -> Option<OsString>) -> Vec<Certificate> {
        requests_ca_bundle(&get_env).map_or_else(Vec::new, |(_, _, certs)| certs)
    }

    #[test]
    fn no_bundle_env_vars_yields_no_extra_certificates() {
        assert!(bundle_certs(env(&[])).is_empty());
    }

    #[test]
    fn reads_requests_ca_bundle_file() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(TEST_CA_PEM.as_bytes()).unwrap();
        let certs = bundle_certs(env(&[(REQUESTS_CA_BUNDLE_ENV, file.path().as_os_str())]));
        assert_eq!(certs.len(), 1);
    }

    #[test]
    fn reads_requests_ca_bundle_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ca.pem"), TEST_CA_PEM).unwrap();
        let certs = bundle_certs(env(&[(REQUESTS_CA_BUNDLE_ENV, dir.path().as_os_str())]));
        assert_eq!(certs.len(), 1);
    }

    #[test]
    fn falls_back_to_curl_ca_bundle_when_requests_ca_bundle_unset_or_empty() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(TEST_CA_PEM.as_bytes()).unwrap();
        for requests_ca_bundle in [None, Some("")] {
            let mut vars = vec![(CURL_CA_BUNDLE_ENV, file.path().as_os_str())];
            if let Some(value) = requests_ca_bundle {
                vars.push((REQUESTS_CA_BUNDLE_ENV, OsStr::new(value)));
            }
            let certs = bundle_certs(env(&vars));
            assert_eq!(certs.len(), 1, "REQUESTS_CA_BUNDLE={requests_ca_bundle:?}");
        }
    }

    #[test]
    fn requests_ca_bundle_wins_over_curl_ca_bundle_even_when_unusable() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(TEST_CA_PEM.as_bytes()).unwrap();
        let certs = bundle_certs(env(&[
            (
                REQUESTS_CA_BUNDLE_ENV,
                OsStr::new("/no/such/requests-ca-bundle-dbt-deps-test.pem"),
            ),
            (CURL_CA_BUNDLE_ENV, file.path().as_os_str()),
        ]));
        assert!(certs.is_empty());
    }

    #[test]
    fn reports_empty_requests_ca_bundle_directory() {
        let dir = tempfile::tempdir().unwrap();
        let certs = bundle_certs(env(&[(REQUESTS_CA_BUNDLE_ENV, dir.path().as_os_str())]));
        assert!(certs.is_empty());
    }

    #[test]
    fn malformed_requests_ca_bundle_does_not_panic() {
        // Parses as PEM but is not valid X.509, so reqwest only rejects it at `build()`.
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n")
            .unwrap();
        let _client = retrying_http_client_with_env(env(&[(
            REQUESTS_CA_BUNDLE_ENV,
            file.path().as_os_str(),
        )]));
    }

    #[cfg(unix)]
    #[test]
    fn non_regular_requests_ca_bundle_yields_no_certificates_instead_of_blocking() {
        // `read` on a FIFO with no writer would block forever.
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("ca.fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(bundle_certs(env(&[(REQUESTS_CA_BUNDLE_ENV, fifo.as_os_str())])).is_empty());
        assert!(bundle_certs(env(&[(REQUESTS_CA_BUNDLE_ENV, OsStr::new("/dev/null"))])).is_empty());
    }

    #[tokio::test]
    async fn lazy_client_reads_the_bundle_on_first_get_only_and_shares_it_across_clones() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let bundle_reads = Arc::new(AtomicUsize::new(0));
        let build = {
            let bundle_reads = bundle_reads.clone();
            move || {
                retrying_http_client_with_env(|_| {
                    bundle_reads.fetch_add(1, Ordering::SeqCst);
                    None
                })
            }
        };

        let lazy = LazyHttpClient::new();
        let clone = lazy.clone();
        assert_eq!(bundle_reads.load(Ordering::SeqCst), 0);

        lazy.client_with(build.clone()).await;
        assert_eq!(bundle_reads.load(Ordering::SeqCst), 2); // both env vars looked up
        clone.client_with(build).await;
        assert_eq!(bundle_reads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn from_existing_client_never_builds() {
        let client = LazyHttpClient::from_existing(with_retries(Client::new()));
        client
            .client_with(|| panic!("an existing client must not be rebuilt"))
            .await;
    }
}
