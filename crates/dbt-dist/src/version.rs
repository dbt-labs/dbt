//! Fetching and resolving dbt's published version manifest (`versions.json`)
//! from the CDN -- the canonical source for "what does 'latest' mean right
//! now" and "what does alias/version X resolve to". Consolidated here
//! because both `dbt-dist` (this crate) and `dbt-main` need this exact
//! resolution, and `dbt-dist` can't depend on `dbt-main` (the dependency
//! already runs the other way).

use std::env;

use dbt_common::{ErrorCode, FsResult, err, fs_err};

/// Resolve the CDN base URL, allowing override via `DBT_CDN_URL`.
pub fn cdn_base_url() -> String {
    #[allow(clippy::disallowed_methods)]
    env::var("DBT_CDN_URL").unwrap_or_else(|_| dbt_common::constants::DBT_CDN_URL.to_string())
}

/// The capability [`resolve_target_version`] needs: fetch a URL's body as
/// text. Kept intentionally narrow -- rather than a richer HTTP client type
/// -- so a caller with its own client (e.g. dbt-main's retrying, mockable
/// update client) can implement this trait for it with a one-line
/// delegation, instead of this crate needing to know about retry policy,
/// mocking, or binary downloads.
#[async_trait::async_trait]
pub trait VersionsHttpClient: Send + Sync {
    async fn get_text(&self, url: &str) -> FsResult<String>;
}

/// Default production implementation: an unadorned GET, no retries.
/// Callers that need retry/backoff (e.g. dbt-main's update flow) should
/// implement [`VersionsHttpClient`] for their own client instead of using
/// this one.
pub struct ReqwestClient;

#[async_trait::async_trait]
impl VersionsHttpClient for ReqwestClient {
    async fn get_text(&self, url: &str) -> FsResult<String> {
        let response = reqwest::Client::new()
            .get(url)
            .header("User-Agent", "dbt-fusion")
            .send()
            .await
            .map_err(|e| fs_err!(ErrorCode::IoError, "GET {url} failed: {e}"))?;
        if !response.status().is_success() {
            return err!(
                ErrorCode::IoError,
                "GET {url} returned {}",
                response.status()
            );
        }
        response.text().await.map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "failed to read response body from {url}: {e}"
            )
        })
    }
}

/// Resolve a version from the versions manifest, mirroring install.sh's
/// `determine_version`:
///   1. No version requested  -> use `latest.tag`
///   2. Version is an alias key (e.g. "canary") -> resolve `versions[alias].tag`
///   3. Version is a literal semver -> use as-is
fn resolve_version_from_manifest(
    versions: &serde_json::Value,
    requested: Option<&str>,
) -> FsResult<String> {
    match requested {
        None => match versions
            .get("latest")
            .and_then(|obj| obj.get("tag"))
            .and_then(|t| t.as_str())
        {
            Some(t) => Ok(t.trim_start_matches('v').to_string()),
            None => err!(
                ErrorCode::IoError,
                "Could not resolve latest version from versions.json"
            ),
        },
        Some(v) => {
            if let Some(tag) = versions
                .get(v)
                .and_then(|obj| obj.get("tag"))
                .and_then(|t| t.as_str())
            {
                Ok(tag.trim_start_matches('v').to_string())
            } else {
                Ok(v.to_string())
            }
        }
    }
}

/// Fetch `versions.json` from the CDN and resolve `version` against it (see
/// [`resolve_version_from_manifest`]). Generic (rather than `&dyn
/// VersionsHttpClient`) so a caller whose own client trait has
/// `VersionsHttpClient` as a supertrait can pass a `&dyn` of *that* trait
/// directly -- `dyn Subtrait` satisfies a `Supertrait` bound without an
/// explicit upcast.
pub async fn resolve_target_version<C: VersionsHttpClient + ?Sized>(
    version: Option<&str>,
    client: &C,
) -> FsResult<String> {
    resolve_target_version_with_base_url(version, None, client).await
}

/// As [`resolve_target_version`], but resolves the CDN base URL from
/// `base_url_override` when given, instead of [`cdn_base_url`]'s
/// environment-variable lookup. Needed by integration tests that point at a
/// mock server: mutating `DBT_CDN_URL` directly would race under parallel
/// test execution, so they pass the mock server's URL explicitly instead.
pub async fn resolve_target_version_with_base_url<C: VersionsHttpClient + ?Sized>(
    version: Option<&str>,
    base_url_override: Option<&str>,
    client: &C,
) -> FsResult<String> {
    let base_url = base_url_override.map_or_else(cdn_base_url, str::to_string);
    let versions_url = format!("{base_url}/versions.json");

    let body = client.get_text(&versions_url).await?;

    let versions: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to parse versions manifest JSON: {e}"
        )
    })?;

    resolve_version_from_manifest(&versions, version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MockHttpClient {
        responses: HashMap<String, String>,
        requests: Mutex<Vec<String>>,
    }

    impl MockHttpClient {
        fn new() -> Self {
            Self {
                responses: HashMap::new(),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn with_text(mut self, url: impl Into<String>, body: &str) -> Self {
            self.responses.insert(url.into(), body.to_string());
            self
        }
    }

    #[async_trait::async_trait]
    impl VersionsHttpClient for MockHttpClient {
        async fn get_text(&self, url: &str) -> FsResult<String> {
            self.requests.lock().unwrap().push(url.to_string());
            self.responses
                .get(url)
                .cloned()
                .ok_or_else(|| fs_err!(ErrorCode::IoError, "MockHttpClient: no response for {url}"))
        }
    }

    fn test_versions_json() -> serde_json::Value {
        serde_json::json!({
            "latest":  { "tag": "v2.0.0-preview.154", "date": "2026-03-13" },
            "dev":     { "tag": "v2.0.0-preview.157", "date": "2026-03-18" },
            "canary":  { "tag": "v2.0.0-preview.157", "date": "2026-03-18" }
        })
    }

    #[test]
    fn cdn_base_url_defaults_to_public_cdn() {
        // SAFETY: test-only env mutation, not run concurrently with anything
        // that reads DBT_CDN_URL outside this test.
        unsafe {
            env::remove_var("DBT_CDN_URL");
        }
        assert_eq!(cdn_base_url(), dbt_common::constants::DBT_CDN_URL);
    }

    #[test]
    fn resolve_version_no_version_uses_latest() {
        let versions = test_versions_json();
        let version = resolve_version_from_manifest(&versions, None).unwrap();
        assert_eq!(version, "2.0.0-preview.154");
    }

    #[test]
    fn resolve_version_alias_dev() {
        let versions = test_versions_json();
        let version = resolve_version_from_manifest(&versions, Some("dev")).unwrap();
        assert_eq!(version, "2.0.0-preview.157");
    }

    #[test]
    fn resolve_version_alias_canary() {
        let versions = test_versions_json();
        let version = resolve_version_from_manifest(&versions, Some("canary")).unwrap();
        assert_eq!(version, "2.0.0-preview.157");
    }

    #[test]
    fn resolve_version_literal_passthrough() {
        let versions = test_versions_json();
        let version = resolve_version_from_manifest(&versions, Some("2.0.0-preview.100")).unwrap();
        assert_eq!(version, "2.0.0-preview.100");
    }

    #[test]
    fn resolve_version_strips_v_prefix() {
        let versions = serde_json::json!({ "latest": { "tag": "v3.0.0" } });
        let version = resolve_version_from_manifest(&versions, None).unwrap();
        assert_eq!(version, "3.0.0");
    }

    #[test]
    fn resolve_version_no_latest_tag_errors() {
        let versions = serde_json::json!({});
        let result = resolve_version_from_manifest(&versions, None);
        assert!(result.is_err());
    }

    #[dbt_runtime::test]
    async fn resolve_target_version_via_mock() {
        let manifest = serde_json::to_string(&test_versions_json()).unwrap();
        let client =
            MockHttpClient::new().with_text(format!("{}/versions.json", cdn_base_url()), &manifest);

        let version = resolve_target_version(None, &client).await.unwrap();
        assert_eq!(version, "2.0.0-preview.154");
    }

    #[dbt_runtime::test]
    async fn resolve_target_version_with_alias() {
        let manifest = serde_json::to_string(&test_versions_json()).unwrap();
        let client =
            MockHttpClient::new().with_text(format!("{}/versions.json", cdn_base_url()), &manifest);

        let version = resolve_target_version(Some("canary"), &client)
            .await
            .unwrap();
        assert_eq!(version, "2.0.0-preview.157");
    }

    #[dbt_runtime::test]
    async fn resolve_target_version_with_base_url_ignores_cdn_base_url() {
        let manifest = serde_json::to_string(&test_versions_json()).unwrap();
        let client =
            MockHttpClient::new().with_text("https://mock.example/versions.json", &manifest);

        let version =
            resolve_target_version_with_base_url(None, Some("https://mock.example"), &client)
                .await
                .unwrap();
        assert_eq!(version, "2.0.0-preview.154");
    }
}
