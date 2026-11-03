//! Optional private artifact transport. Public URLs remain the artifact identity.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use aws_config::BehaviorVersion;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Region, retry::RetryConfig, timeout::TimeoutConfig};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::Instrument;

pub const SOURCE_FAILURE: &str = "DBT_ARTIFACT_SOURCE_FAILED";
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(45);
const CONFIG_LIMIT: u64 = 64 * 1024;
const PRODUCTION_CDN_HOST: &str = "public.cdn.getdbt.com";
const STAGING_CDN_HOST: &str = "public.staging.cdn.getdbt.com";

fn production_cdn_host() -> String {
    PRODUCTION_CDN_HOST.into()
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub environment: String,
    pub cloud: String,
    pub region: String,
    pub deployment: String,
    pub account: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    version: u32,
    force_cloudfront: bool,
    scopes: Vec<Rule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    environment: String,
    cloud: String,
    region: String,
    deployment: String,
    accounts: Vec<String>,
    enabled: bool,
    buckets: Vec<Source>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct Source {
    bucket: String,
    region: String,
    #[serde(default = "production_cdn_host")]
    cdn_host: String,
}

impl Source {
    fn is_valid(&self) -> bool {
        let valid_bucket = (3..=63).contains(&self.bucket.len())
            && self
                .bucket
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
            && self.bucket.starts_with(|c: char| c.is_ascii_alphanumeric())
            && self.bucket.ends_with(|c: char| c.is_ascii_alphanumeric())
            && !self.bucket.contains("..")
            && !self.bucket.contains(".-")
            && !self.bucket.contains("-.")
            && self.bucket.parse::<std::net::Ipv4Addr>().is_err();
        let valid_region = !self.region.is_empty()
            && self
                .region
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        valid_bucket
            && valid_region
            && matches!(
                self.cdn_host.as_str(),
                PRODUCTION_CDN_HOST | STAGING_CDN_HOST
            )
    }

    fn matches_url(&self, url: &str) -> bool {
        url::Url::parse(url)
            .ok()
            .is_some_and(|url| url.host_str() == Some(self.cdn_host.as_str()))
    }
}

impl Rule {
    fn matches(&self, scope: &Scope) -> bool {
        self.environment == scope.environment
            && self.cloud == scope.cloud
            && self.region == scope.region
            && self.deployment == scope.deployment
            && (self.accounts.contains(&scope.account)
                || (self.environment == "dev" && self.accounts.is_empty()))
    }

    fn source(&self) -> Option<Source> {
        if self.environment.is_empty()
            || !matches!(self.cloud.as_str(), "aws" | "azure" | "gcp")
            || self.region.is_empty()
            || self.deployment.is_empty()
            || self.buckets.iter().any(|source| !source.is_valid())
        {
            return None;
        }
        let source_region = if self.cloud == "aws" {
            self.region.as_str()
        } else {
            "us-east-1"
        };
        let mut matching = self
            .buckets
            .iter()
            .filter(|source| source.region == source_region);
        let source = matching.next()?;
        // Multiple buckets in one region are ambiguous; CloudFront is the fallback.
        if matching.next().is_some() {
            return None;
        }
        Some(source.clone())
    }
}

/// Re-open the path on every attempt, including projected-volume symlink updates.
pub fn source_from_file(path: &Path, scope: &Scope, force_cloudfront: bool) -> Option<Source> {
    if force_cloudfront {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(CONFIG_LIMIT + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > CONFIG_LIMIT {
        return None;
    }
    let config: Config = serde_json::from_slice(&bytes).ok()?;
    if config.version != 1 || config.force_cloudfront {
        return None;
    }
    let mut matching = config.scopes.iter().filter(|rule| rule.matches(scope));
    let rule = matching.next()?;
    // Overlapping rules are ambiguous, even if one would enable S3.
    if matching.next().is_some() || !rule.enabled {
        return None;
    }
    rule.source()
}

#[allow(clippy::disallowed_methods)]
fn configured_source() -> Option<Source> {
    let force = std::env::var("DBT_ARTIFACT_FORCE_CLOUDFRONT").unwrap_or_default();
    let scope: Scope = serde_json::from_str(&std::env::var("DBT_ARTIFACT_SCOPE").ok()?).ok()?;
    let path = std::env::var("DBT_ARTIFACT_SOURCE_CONFIG").ok()?;
    source_from_file(Path::new(&path), &scope, !force.is_empty() && force != "0")
}

fn artifact_key(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some(PRODUCTION_CDN_HOST | STAGING_CDN_HOST))
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let key = percent_encoding::percent_decode_str(url.path().strip_prefix('/')?)
        .decode_utf8()
        .ok()?
        .into_owned();
    let binary_key = key.strip_prefix("fs/cli/").is_some_and(|filename| {
        let archive = filename.strip_suffix(".sha256").unwrap_or(filename);
        let version = archive
            .strip_prefix("fs-v")
            .or_else(|| archive.strip_prefix("fs-db-runner-v"));
        archive.ends_with(".tar.gz")
            && version.is_some_and(|value| value.starts_with(|c: char| c.is_ascii_digit()))
            && filename
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-' | b'_'))
    });
    if key == "fs/versions.json" || binary_key {
        if key
            .split('/')
            .any(|part| part == ".." || part == "." || part.is_empty())
        {
            return None;
        }
        Some(key)
    } else {
        None
    }
}

fn configured_source_for_url(url: &str) -> Option<(String, Source)> {
    let key = artifact_key(url)?;
    let source = configured_source()?;
    if !source.matches_url(url) {
        return None;
    }
    Some((key, source))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DownloadError(pub &'static str);

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{SOURCE_FAILURE}: {}", self.0)
    }
}
impl std::error::Error for DownloadError {}

fn expected_checksum(checksum: &str) -> Result<String, DownloadError> {
    let expected = checksum.split_whitespace().next().unwrap_or_default();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(DownloadError("invalid_checksum"));
    }
    Ok(expected.to_ascii_lowercase())
}

pub fn verify_checksum(bytes: &[u8], checksum: &str) -> Result<(), DownloadError> {
    if format!("{:x}", Sha256::digest(bytes)) != expected_checksum(checksum)? {
        return Err(DownloadError("checksum_mismatch"));
    }
    Ok(())
}

/// Record a public artifact transport result without logging request URLs or credentials.
pub fn record_download(
    artifact: &str,
    source: &str,
    duration: Duration,
    bytes: usize,
    reason: Option<&str>,
) {
    let Some(key) = artifact_key(artifact) else {
        return;
    };
    record_download_result(&key, source, None, duration, bytes, reason);
}

fn record_download_result(
    key: &str,
    source: &str,
    s3: Option<&Source>,
    duration: Duration,
    bytes: usize,
    reason: Option<&str>,
) {
    let artifact_type = if key.ends_with("/versions.json") {
        "manifest"
    } else if key.ends_with(".sha256") {
        "checksum"
    } else if key.contains("/cli/fs-db-runner") {
        "runner"
    } else {
        "main"
    };
    let bucket = s3.map_or("", |source| source.bucket.as_str());
    let region = s3.map_or("", |source| source.region.as_str());
    let duration_ms = duration.as_millis() as u64;
    let result = if reason.is_none() {
        "success"
    } else {
        "failure"
    };
    let reason = reason.unwrap_or("");
    // Message-only logging consumers must retain the transport provenance too.
    tracing::info!(
        source,
        artifact_type,
        bucket,
        region,
        key,
        duration_ms,
        bytes = bytes as u64,
        result,
        reason,
        "artifact_download source={source} artifact_type={artifact_type} result={result} bucket={bucket:?} region={region:?} key={key:?} bytes={bytes} duration_ms={duration_ms} reason={reason:?}"
    );
}

struct Transport {
    runtime: tokio::runtime::Runtime,
    clients: Arc<Mutex<HashMap<Source, Client>>>,
}

fn transport() -> Result<&'static Transport, DownloadError> {
    static TRANSPORT: OnceLock<Result<Transport, DownloadError>> = OnceLock::new();
    TRANSPORT
        .get_or_init(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("artifact-download")
                .build()
                .map_err(|_| DownloadError("runtime"))?;
            Ok(Transport {
                runtime,
                clients: Arc::default(),
            })
        })
        .as_ref()
        .map_err(|e| *e)
}

fn s3_error(code: Option<&str>) -> DownloadError {
    match code {
        // Without ListBucket, S3 also returns AccessDenied for a missing key.
        Some("AccessDenied") => DownloadError("access_denied_or_missing"),
        Some("InvalidAccessKeyId" | "ExpiredToken" | "InvalidToken") => DownloadError("auth"),
        Some("NoSuchKey" | "NoSuchBucket") => DownloadError("missing_object"),
        _ => DownloadError("request"),
    }
}

async fn get_object(
    clients: Arc<Mutex<HashMap<Source, Client>>>,
    source: &Source,
    key: &str,
) -> Result<aws_sdk_s3::primitives::ByteStream, DownloadError> {
    let client = {
        let mut clients = clients.lock().await;
        if let Some(client) = clients.get(source) {
            client.clone()
        } else {
            // The SDK's shared identity cache refreshes temporary credentials.
            let config = aws_config::defaults(BehaviorVersion::latest())
                .region(Region::new(source.region.clone()))
                .retry_config(RetryConfig::standard().with_max_attempts(2))
                .timeout_config(
                    TimeoutConfig::builder()
                        .operation_timeout(Duration::from_secs(30))
                        .operation_attempt_timeout(Duration::from_secs(15))
                        .build(),
                )
                .load()
                .await;
            let client = Client::new(&config);
            // Bound the cache when a source configuration changes repeatedly.
            if clients.len() >= 8 {
                clients.clear();
            }
            clients.insert(source.clone(), client.clone());
            client
        }
    };
    let response = client
        .get_object()
        .bucket(&source.bucket)
        .key(key)
        .send()
        .await
        .map_err(|error| {
            use aws_sdk_s3::error::ProvideErrorMetadata;
            s3_error(error.as_service_error().and_then(|error| error.code()))
        })?;
    Ok(response.body)
}

async fn download(
    clients: Arc<Mutex<HashMap<Source, Client>>>,
    source: Source,
    key: String,
) -> Result<Vec<u8>, DownloadError> {
    let started = Instant::now();
    let result = tokio::time::timeout(DOWNLOAD_TIMEOUT, async {
        let body = get_object(clients, &source, &key)
            .await?
            .collect()
            .await
            .map_err(|_| DownloadError("body"))?;
        Ok(body.into_bytes().to_vec())
    })
    .await
    .unwrap_or(Err(DownloadError("timeout")));
    record_download_result(
        &key,
        "s3",
        Some(&source),
        started.elapsed(),
        result.as_ref().map_or(0, |bytes| bytes.len()),
        result.as_ref().err().map(|error| error.0),
    );
    result
}

/// None means use the caller's existing public transport. No credentials are loaded.
pub async fn fetch(url: &str) -> Option<Result<Vec<u8>, DownloadError>> {
    let (key, source) = configured_source_for_url(url)?;
    Some(match transport() {
        Ok(transport) => transport
            .runtime
            .spawn(download(transport.clients.clone(), source, key).in_current_span())
            .await
            .unwrap_or(Err(DownloadError("runtime"))),
        Err(error) => Err(error),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tracing_subscriber::prelude::*;

    #[test]
    fn s3_access_denied_does_not_claim_auth_or_missing_object() {
        assert_eq!(
            s3_error(Some("AccessDenied")),
            DownloadError("access_denied_or_missing")
        );
        for code in ["InvalidAccessKeyId", "ExpiredToken", "InvalidToken"] {
            assert_eq!(s3_error(Some(code)), DownloadError("auth"));
        }
        assert_eq!(s3_error(Some("NoSuchKey")), DownloadError("missing_object"));
        assert_eq!(s3_error(None), DownloadError("request"));
    }

    #[derive(Clone, Default)]
    struct Messages(Arc<std::sync::Mutex<Vec<String>>>);

    impl tracing::field::Visit for Messages {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" {
                self.0.lock().unwrap().push(format!("{value:?}"));
            }
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Messages {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            event.record(&mut self.clone());
        }
    }

    #[test]
    fn download_messages_preserve_provenance_without_structured_fields() {
        let messages = Messages::default();
        let source = Source {
            bucket: "private-test".into(),
            region: "us-east-1".into(),
            cdn_host: production_cdn_host(),
        };
        let subscriber = tracing_subscriber::registry().with(messages.clone());
        tracing::subscriber::with_default(subscriber, || {
            for (key, artifact_type) in [
                ("fs/versions.json", "manifest"),
                ("fs/cli/fs-v2.0.2-linux.tar.gz", "main"),
                ("fs/cli/fs-db-runner-v2.0.2-linux.tar.gz", "runner"),
                ("fs/cli/fs-v2.0.2-linux.tar.gz.sha256", "checksum"),
            ] {
                record_download_result(
                    key,
                    "s3",
                    Some(&source),
                    Duration::from_millis(42),
                    123,
                    None,
                );
                let captured = messages.0.lock().unwrap();
                let message = captured.last().unwrap();
                for expected in [
                    "artifact_download source=s3".to_string(),
                    format!("artifact_type={artifact_type}"),
                    "result=success".into(),
                    "bucket=\"private-test\"".into(),
                    "region=\"us-east-1\"".into(),
                    format!("key={key:?}"),
                    "bytes=123".into(),
                    "duration_ms=42".into(),
                ] {
                    assert!(message.contains(&expected), "{message}");
                }
            }
            record_download_result(
                "fs/versions.json",
                "s3",
                Some(&source),
                Duration::ZERO,
                0,
                Some("auth"),
            );
            record_download(
                "https://public.cdn.getdbt.com/fs/versions.json",
                "cloudfront",
                Duration::ZERO,
                123,
                None,
            );
        });
        let captured = messages.0.lock().unwrap();
        assert_eq!(captured.len(), 6);
        assert!(captured[4].contains("source=s3 artifact_type=manifest result=failure"));
        assert!(captured[4].contains("reason=\"auth\""));
        assert!(captured[5].contains("source=cloudfront artifact_type=manifest result=success"));
        assert!(captured[5].contains("key=\"fs/versions.json\""));
        assert!(!captured[5].contains("private-test"));
    }

    #[test]
    fn unsupported_public_urls_are_not_recorded() {
        let messages = Messages::default();
        let subscriber = tracing_subscriber::registry().with(messages.clone());
        tracing::subscriber::with_default(subscriber, || {
            for url in [
                "https://public.cdn.getdbt.com/fs/versions.json?token=secret",
                "https://user:secret@public.cdn.getdbt.com/fs/versions.json",
                "https://public.cdn.getdbt.com/fs/cli/fs-v2.0.2%0Aforged-linux.tar.gz",
                "https://public.cdn.getdbt.com/fs/install/install.sh",
                "https://public.cdn.getdbt.com/fs/install/install.ps1",
            ] {
                record_download(url, "cloudfront", Duration::ZERO, 1, None);
            }
        });
        let captured = messages.0.lock().unwrap();
        assert!(captured.is_empty(), "{captured:?}");
    }

    fn scope() -> Scope {
        Scope {
            environment: "dev".into(),
            cloud: "aws".into(),
            region: "us-east-1".into(),
            deployment: "local".into(),
            account: "1".into(),
        }
    }
    fn config() -> serde_json::Value {
        json!({"version":1,"force_cloudfront":false,"scopes":[{"environment":"dev","cloud":"aws","region":"us-east-1","deployment":"local","accounts":[],"enabled":true,"buckets":[{"bucket":"private-test","region":"us-east-1"}]}]})
    }

    #[test]
    fn controls_reload_and_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        assert!(source_from_file(&path, &scope(), false).is_none());
        std::fs::write(&path, config().to_string()).unwrap();
        assert!(source_from_file(&path, &scope(), false).is_some());
        assert!(source_from_file(&path, &scope(), true).is_none());
        for mutation in [
            "global",
            "disabled",
            "overlap",
            "region",
            "duplicate_region",
            "bucket",
            "account",
            "invalid",
        ] {
            let mut value = config();
            match mutation {
                "global" => value["force_cloudfront"] = json!(true),
                "disabled" => value["scopes"][0]["enabled"] = json!(false),
                "overlap" => {
                    let duplicate = value["scopes"][0].clone();
                    value["scopes"].as_array_mut().unwrap().push(duplicate);
                }
                "region" => value["scopes"][0]["buckets"][0]["region"] = json!("us-west-2"),
                "duplicate_region" => {
                    let duplicate = value["scopes"][0]["buckets"][0].clone();
                    value["scopes"][0]["buckets"]
                        .as_array_mut()
                        .unwrap()
                        .push(duplicate);
                }
                "bucket" => value["scopes"][0]["buckets"][0]["bucket"] = json!("INVALID"),
                "account" => value["scopes"][0]["accounts"] = json!(["2"]),
                _ => value["version"] = json!(2),
            }
            std::fs::write(&path, value.to_string()).unwrap();
            assert!(
                source_from_file(&path, &scope(), false).is_none(),
                "{mutation}"
            );
        }
        std::fs::write(&path, "invalid").unwrap();
        assert!(source_from_file(&path, &scope(), false).is_none());
    }

    #[test]
    fn oversized_config_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut valid = config().to_string();
        valid.push_str(&" ".repeat(CONFIG_LIMIT as usize - valid.len()));
        std::fs::write(&path, &valid).unwrap();
        assert!(source_from_file(&path, &scope(), false).is_some());

        std::fs::write(&path, format!("{valid}unexpected trailing content")).unwrap();
        assert!(source_from_file(&path, &scope(), false).is_none());
    }

    #[test]
    fn selects_the_required_regional_bucket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut value = config();
        value["scopes"][0]["buckets"] = json!([
            {"bucket":"private-east","region":"us-east-1"},
            {"bucket":"private-west","region":"us-west-2"}
        ]);

        std::fs::write(&path, value.to_string()).unwrap();
        assert_eq!(
            source_from_file(&path, &scope(), false),
            Some(Source {
                bucket: "private-east".into(),
                region: "us-east-1".into(),
                cdn_host: production_cdn_host(),
            })
        );

        let mut west_scope = scope();
        west_scope.region = "us-west-2".into();
        value["scopes"][0]["region"] = json!("us-west-2");
        std::fs::write(&path, value.to_string()).unwrap();
        assert_eq!(
            source_from_file(&path, &west_scope, false),
            Some(Source {
                bucket: "private-west".into(),
                region: "us-west-2".into(),
                cdn_host: production_cdn_host(),
            })
        );

        let mut azure_scope = scope();
        azure_scope.cloud = "azure".into();
        azure_scope.region = "eastus2".into();
        value["scopes"][0]["cloud"] = json!("azure");
        value["scopes"][0]["region"] = json!("eastus2");
        std::fs::write(&path, value.to_string()).unwrap();
        assert_eq!(
            source_from_file(&path, &azure_scope, false),
            Some(Source {
                bucket: "private-east".into(),
                region: "us-east-1".into(),
                cdn_host: production_cdn_host(),
            })
        );
    }

    #[test]
    fn staging_cdn_requires_an_explicit_matching_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut value = config();
        std::fs::write(&path, value.to_string()).unwrap();
        let production = source_from_file(&path, &scope(), false).unwrap();
        let staging_url = "https://public.staging.cdn.getdbt.com/fs/versions.json";
        assert!(artifact_key(staging_url).is_some());
        assert!(!production.matches_url(staging_url));

        value["scopes"][0]["buckets"][0]["cdn_host"] = json!(STAGING_CDN_HOST);
        std::fs::write(&path, value.to_string()).unwrap();
        let staging = source_from_file(&path, &scope(), false).unwrap();
        assert!(staging.matches_url(staging_url));
        assert!(!staging.matches_url("https://public.cdn.getdbt.com/fs/versions.json"));

        value["scopes"][0]["buckets"][0]["cdn_host"] = json!("untrusted.example.com");
        std::fs::write(&path, value.to_string()).unwrap();
        assert!(source_from_file(&path, &scope(), false).is_none());
    }

    #[test]
    fn only_artifact_urls_are_redirected_and_keys_are_decoded_once() {
        assert_eq!(
            artifact_key("https://public.cdn.getdbt.com/fs/cli/fs-v2.0.2%2Bbuild-linux.tar.gz"),
            Some("fs/cli/fs-v2.0.2+build-linux.tar.gz".into())
        );
        assert_eq!(
            artifact_key(
                "https://public.cdn.getdbt.com/fs/cli/fs-db-runner-v2.0.2-linux.tar.gz.sha256"
            ),
            Some("fs/cli/fs-db-runner-v2.0.2-linux.tar.gz.sha256".into())
        );
        for url in [
            "http://public.cdn.getdbt.com/fs/versions.json",
            "https://example.com/fs/versions.json",
            "https://public.cdn.getdbt.com/fs/install/install.sh",
            "https://public.cdn.getdbt.com/fs/cli/fs-v2.0.2-linux.zip",
            "https://public.cdn.getdbt.com/fs/cli/fs-v2.0.2-linux.tar.gz/other",
            "https://public.cdn.getdbt.com/fs/cli/fs-v2.0.2%0Aforged-linux.tar.gz",
            "https://public.cdn.getdbt.com/fs/adbc/snowflake/adbc_driver_snowflake-1.0%2Bdbt-linux.so.zst",
            "https://public.cdn.getdbt.com/fs/adbc/snowflake/driver.zst",
            "https://public.cdn.getdbt.com/fs/adbc/snowflake/latest.json",
            "https://public.cdn.getdbt.com/fs/adbc/snowflake/adbc_driver_snowflake-1.0/other.zst",
            "https://public.cdn.getdbt.com/fs/adbc/a%2F..%2Fb",
            "https://public.cdn.getdbt.com/fs/versions.json?token=secret",
        ] {
            assert!(artifact_key(url).is_none(), "{url}");
        }
    }

    #[test]
    fn verifies_payload_and_checksum_format() {
        let hash = format!("{:x}", Sha256::digest(b"trusted"));
        assert!(verify_checksum(b"trusted", &format!("{hash}  archive.tar.gz\n")).is_ok());
        assert_eq!(
            verify_checksum(b"corrupt", &hash),
            Err(DownloadError("checksum_mismatch"))
        );
        assert_eq!(
            verify_checksum(b"trusted", ""),
            Err(DownloadError("invalid_checksum"))
        );
    }

    #[tokio::test]
    async fn sdk_signs_temporary_credentials_and_preserves_object_keys() {
        use std::io::{Read, Write};
        use tracing::instrument::WithSubscriber;
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", server.local_addr().unwrap());
        let requests = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..1 {
                let (mut stream, _) = server.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let size = stream.read(&mut buffer).unwrap();
                    assert!(size > 0);
                    request.extend_from_slice(&buffer[..size]);
                }
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\ntrusted",
                    )
                    .unwrap();
                requests.push(String::from_utf8(request).unwrap().to_ascii_lowercase());
            }
            requests
        });
        let config = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "test-access",
                "test-secret",
                Some("test-session".into()),
                None,
                "test",
            ))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .build();
        let source = Source {
            bucket: "private-test".into(),
            region: "us-east-1".into(),
            cdn_host: production_cdn_host(),
        };
        let clients = Arc::new(Mutex::new(HashMap::from([(
            source.clone(),
            Client::from_conf(config),
        )])));
        let messages = Messages::default();
        let result = download(
            clients,
            source,
            "fs/cli/fs-v2.0.2+build-linux.tar.gz".into(),
        )
        .with_subscriber(tracing_subscriber::registry().with(messages.clone()))
        .await
        .unwrap();
        assert_eq!(result, b"trusted");
        let requests = requests.join().unwrap();
        assert!(requests[0].contains("/private-test/fs/cli/fs-v2.0.2%2bbuild-linux.tar.gz"));
        for request in requests {
            assert!(request.contains("authorization: aws4-hmac-sha256"));
            assert!(request.contains("x-amz-security-token: test-session"));
            assert!(!request.contains("test-secret"));
        }
        let captured = messages.0.lock().unwrap();
        let downloads: Vec<_> = captured
            .iter()
            .filter(|message| message.starts_with("artifact_download "))
            .collect();
        assert_eq!(downloads.len(), 1);
        for message in downloads {
            for expected in [
                "source=s3 artifact_type=main result=success",
                "bucket=\"private-test\"",
                "region=\"us-east-1\"",
                "key=\"fs/cli/fs-v2.0.2+build-linux.tar.gz\"",
                "bytes=7",
            ] {
                assert!(message.contains(expected), "{message}");
            }
            assert!(!message.contains("test-secret"));
            assert!(!message.contains("test-session"));
        }
    }
}
