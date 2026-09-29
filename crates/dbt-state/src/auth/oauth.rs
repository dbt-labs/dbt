use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;

use dbt_platform_auth::{AuthChain, AuthChainBuilder, AuthError, Credential, OAUTH_CLIENT_ID};

use crate::auth::browser_flow::{
    BrowserFlow, INTERACTIVE_TIMEOUT, InteractiveFlow, LOOPBACK_PORT, ORGS_SCOPE,
    TOKEN_HTTP_TIMEOUT, TokenResponse, send_token_form, send_token_request,
};
use crate::auth::scope::{Scope, determine_org_id, jwt_claims};
use crate::auth::token_store::{StoredToken, TokenStore};
use crate::service_client::RunCacheServiceError;
use crate::service_config::RunCacheServiceConfig;

const TOKEN_REFRESH_WINDOW: Duration = Duration::from_secs(300);

#[derive(Clone, Debug)]
pub struct CachedToken {
    pub id_token: String,
    pub scope: String,
    pub expires_at: Option<SystemTime>,
    pub refresh_token: Option<String>,
    /// dbt platform account whose credential was exchanged for this token.
    /// `None` when the token was not minted from a platform credential.
    pub platform_account_id: Option<String>,
}

impl CachedToken {
    pub fn is_fresh(&self) -> bool {
        match self.expires_at {
            Some(expires_at) => expires_at > SystemTime::now() + TOKEN_REFRESH_WINDOW,
            None => true,
        }
    }
}

pub struct OAuthTokenSource {
    http: reqwest::Client,
    token_url: String,
    client_id: String,
    client_secret: Option<String>,
    configured_org_id: Option<String>,
    platform_account_id: Option<String>,
    store: TokenStore,
    cached: Arc<Mutex<Option<CachedToken>>>,
    disk_loaded: Arc<AtomicBool>,
    interactive_flow: Arc<dyn InteractiveFlow>,
    auth_chain: AuthChain,
}

impl std::fmt::Debug for OAuthTokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthTokenSource")
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("configured_org_id", &self.configured_org_id)
            .field("platform_account_id", &self.platform_account_id)
            .finish()
    }
}

impl OAuthTokenSource {
    pub fn new(config: &RunCacheServiceConfig) -> Result<Self, RunCacheServiceError> {
        let http = reqwest::Client::builder()
            .connect_timeout(TOKEN_HTTP_TIMEOUT)
            .timeout(TOKEN_HTTP_TIMEOUT)
            .build()?;

        let store = TokenStore::discover().ok_or_else(|| {
            RunCacheServiceError::Auth(
                "could not resolve home directory for dbt State auth; \
                 set DBT_ENGINE_STATE_HOME to override"
                    .to_string(),
            )
        })?;

        let interactive_flow: Arc<dyn InteractiveFlow> = Arc::new(BrowserFlow {
            http: http.clone(),
            auth_url: config.oauth_auth_url.clone(),
            token_url: config.oauth_token_url.clone(),
            client_id: config.oauth_client_id.clone(),
            scope: ORGS_SCOPE.to_string(),
            timeout: INTERACTIVE_TIMEOUT,
            redirect_port: LOOPBACK_PORT,
            opener: BrowserFlow::default_opener(),
            abort_signal: std::sync::Mutex::new(None),
            available: BrowserFlow::has_attached_terminal(),
        });

        Ok(Self {
            http,
            token_url: config.oauth_token_url.clone(),
            client_id: config.oauth_client_id.clone(),
            client_secret: config.oauth_client_secret.clone(),
            configured_org_id: config.org_id.clone(),
            platform_account_id: config.platform_account_id.clone(),
            store,
            cached: Arc::new(Mutex::new(None)),
            disk_loaded: Arc::new(AtomicBool::new(false)),
            interactive_flow,
            auth_chain: platform_auth_chain(config.platform_account_id.as_deref()),
        })
    }

    /// Construct with explicit components. Used by tests and the integration suite.
    pub fn with_components(
        config: &RunCacheServiceConfig,
        store: TokenStore,
        interactive_flow: Arc<dyn InteractiveFlow>,
        auth_chain: AuthChain,
    ) -> Result<Self, RunCacheServiceError> {
        let http = reqwest::Client::builder()
            .connect_timeout(TOKEN_HTTP_TIMEOUT)
            .timeout(TOKEN_HTTP_TIMEOUT)
            .build()?;
        Ok(Self {
            http,
            token_url: config.oauth_token_url.clone(),
            client_id: config.oauth_client_id.clone(),
            client_secret: config.oauth_client_secret.clone(),
            configured_org_id: config.org_id.clone(),
            platform_account_id: config.platform_account_id.clone(),
            store,
            cached: Arc::new(Mutex::new(None)),
            disk_loaded: Arc::new(AtomicBool::new(false)),
            interactive_flow,
            auth_chain,
        })
    }

    pub async fn token(&self) -> Result<CachedToken, RunCacheServiceError> {
        let mut guard = self.cached.lock().await;

        if let Some(token) = guard.as_ref() {
            if token.is_fresh() {
                return Ok(token.clone());
            }
        }

        if let Some(token) = self.get_or_refresh_from_disk().await? {
            *guard = Some(token.clone());
            return Ok(token);
        }

        let (token, cache_to_disk) = self.acquire_fresh_token().await?;
        if cache_to_disk {
            let stored: StoredToken = (&token).into_stored(&self.token_type_or_default());
            if let Err(err) = self.store.save(&stored).await {
                tracing::warn!("failed to persist dbt State auth token: {err}");
            }
        }
        *guard = Some(token.clone());
        Ok(token)
    }

    async fn get_or_refresh_from_disk(&self) -> Result<Option<CachedToken>, RunCacheServiceError> {
        if self.disk_loaded.swap(true, Ordering::AcqRel) {
            return Ok(None);
        }
        let Some(stored) = self.store.load().await? else {
            return Ok(None);
        };
        let cached = stored.clone().into_cached();

        // A token minted against a different dbt platform account cannot serve
        // this configuration, and refreshing it would only extend the wrong
        // identity — drop it and mint a new one from the configured account.
        if self.platform_account_changed(&cached) {
            tracing::debug!(
                "discarding cached dbt State token minted for dbt platform account {:?}; \
                 configuration selects {:?}",
                cached.platform_account_id,
                self.platform_account_id
            );
            let _ = self.store.delete().await;
            return Ok(None);
        }

        let is_fresh = cached.is_fresh();

        // If a configured org_id isn't in the cached scope, force a refresh in case the
        // user's permissions changed since the cache was last saved.
        let needs_scope_refresh = !self.scope_satisfies_config(&cached.scope);

        if is_fresh && !needs_scope_refresh {
            return Ok(Some(cached));
        }

        let Some(refresh_token) = stored.refresh_token.clone() else {
            // Without a refresh token we can't pick up scope changes. If the cached token
            // is still valid, return it and let resolve_org_id surface any scope mismatch;
            // otherwise fall through to re-login.
            return Ok(if is_fresh { Some(cached) } else { None });
        };

        match self
            .fetch_refresh(&refresh_token, stored.platform_account_id.clone())
            .await
        {
            Ok(token) => Ok(Some(token)),
            Err(err) => {
                if is_fresh {
                    // The token itself is still valid; return it and let resolve_org_id
                    // surface any scope mismatch as a proper error instead of triggering
                    // browser re-auth.
                    Ok(Some(cached))
                } else {
                    // The token is expired and refresh failed; clear the cache and fall
                    // through to re-login.
                    tracing::warn!(
                        "dbt State token refresh failed, falling back to re-login: {err}"
                    );
                    let _ = self.store.delete().await;
                    Ok(None)
                }
            }
        }
    }

    /// Acquire a freshly minted token along with whether it should be persisted
    /// to disk. Tokens minted from re-derivable credentials (client credentials
    /// or platform token exchange) are not cached in headless environments (no
    /// interactive user present, e.g. CI), where the cache offers no reuse
    /// benefit and would leave credentials behind on ephemeral runners.
    /// Interactively-minted tokens are always cached so the user isn't
    /// re-prompted on the next run.
    async fn acquire_fresh_token(&self) -> Result<(CachedToken, bool), RunCacheServiceError> {
        let interactive = self.interactive_flow.is_available();
        let (response, cache_to_disk, platform_account_id) = if self.client_secret.is_some() {
            (self.fetch_client_credentials().await?, interactive, None)
        } else {
            match self.auth_chain.resolve().await {
                Ok(credential) => {
                    // Only the cached-session resolver honors the configured
                    // account, so a later resolver in the chain can hand back a
                    // credential for a different one. Exchanging it would
                    // authenticate as the wrong account, which is precisely what
                    // the configuration asks us not to do.
                    let account_id = credential.account_id().to_string();
                    if let Some(configured) = self.platform_account_id.as_deref() {
                        if configured != account_id {
                            return Err(RunCacheServiceError::PlatformAccountUnavailable {
                                configured: configured.to_string(),
                                found: Some(account_id),
                            });
                        }
                    }
                    (
                        self.fetch_platform_token_exchange(&credential).await?,
                        interactive,
                        Some(account_id),
                    )
                }
                Err(AuthError::NotAuthenticated) => {
                    // The standalone browser flow authenticates against dbt
                    // State directly, so it cannot produce a credential for the
                    // configured platform account. Report the missing session
                    // instead of logging in as somebody else.
                    if let Some(configured) = self.platform_account_id.as_deref() {
                        return Err(RunCacheServiceError::PlatformAccountUnavailable {
                            configured: configured.to_string(),
                            found: None,
                        });
                    }
                    // No platform credentials and no configured account — fall
                    // back to dbt State standalone authentication, unless
                    // there's nobody around to complete a browser login (CI,
                    // batch replay). Checking this up front avoids waiting out
                    // the full interactive timeout just to fail anyway.
                    if !interactive {
                        return Err(RunCacheServiceError::NoInteractiveTerminal);
                    }
                    (self.interactive_flow.run().await?, true, None)
                }
                Err(err) => {
                    return Err(RunCacheServiceError::Auth(format!(
                        "failed to resolve dbt Platform credential for dbt State token exchange: {err}"
                    )));
                }
            }
        };
        Ok((
            self.process_response(response, platform_account_id)?,
            cache_to_disk,
        ))
    }

    /// Whether a cached token was minted against a different dbt platform
    /// account than the one this configuration selects. Unknown provenance
    /// (`None`, e.g. a standalone dbt State login or a file written before the
    /// account was recorded) is never treated as a mismatch.
    ///
    /// A token is only ever minted from a credential for the configured account
    /// (see [`Self::acquire_fresh_token`]), so a mismatch here means the
    /// configuration changed since the token was minted — discarding it converges
    /// rather than repeating on every run.
    fn platform_account_changed(&self, cached: &CachedToken) -> bool {
        match (
            self.platform_account_id.as_deref(),
            cached.platform_account_id.as_deref(),
        ) {
            (Some(configured), Some(minted)) => configured != minted,
            _ => false,
        }
    }

    /// Whether a token whose scope is `scope_str` can serve the configured org_id.
    /// When no org_id is configured this is always true — disambiguation is then
    /// deferred to `resolve_org_id`.
    fn scope_satisfies_config(&self, scope_str: &str) -> bool {
        match self.configured_org_id.as_deref() {
            None => true,
            Some(configured) => Scope::from_string(scope_str)
                .map(|scope| scope.is_org_id_in_scope(configured))
                .unwrap_or(false),
        }
    }

    /// Resolve the organization ID for a token from its scope and the configured
    /// org_id. The org_id is always derived from the live token scope rather than a
    /// value cached at write time, so the result is consistent with the current
    /// configuration. Ambiguous or disabled scopes surface here as errors instead of
    /// being treated as token acquisition failures.
    pub fn resolve_org_id(&self, token: &CachedToken) -> Result<String, RunCacheServiceError> {
        let scope = Scope::from_string(&token.scope)?;
        determine_org_id(&scope, self.configured_org_id.as_deref())
    }

    fn token_type_or_default(&self) -> String {
        "Bearer".to_string()
    }

    async fn fetch_client_credentials(&self) -> Result<TokenResponse, RunCacheServiceError> {
        let client_secret = self
            .client_secret
            .as_deref()
            .ok_or_else(|| RunCacheServiceError::Auth("missing client secret".to_string()))?;
        let request = self
            .http
            .post(&self.token_url)
            .basic_auth(&self.client_id, Some(client_secret));
        let form = [("grant_type", "client_credentials"), ("scope", ORGS_SCOPE)];
        send_token_request(request, &form).await
    }

    async fn fetch_platform_token_exchange(
        &self,
        credential: &Credential,
    ) -> Result<TokenResponse, RunCacheServiceError> {
        let form = [
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            ("subject_token_type", "dbt"),
            ("subject_token", credential.token()),
            ("dbt_hostname", credential.account_host()),
            ("client_id", self.client_id.as_str()),
        ];
        send_token_form(&self.http, &self.token_url, &form)
            .await
            .map_err(|err| {
                if let RunCacheServiceError::Auth(message) = err {
                    return RunCacheServiceError::Auth(format!(
                        "Unable to exchange dbt platform token for a dbt State authentication token: {message}"
                    ));
                }
                err
            })
    }

    async fn fetch_refresh(
        &self,
        refresh_token: &str,
        platform_account_id: Option<String>,
    ) -> Result<CachedToken, RunCacheServiceError> {
        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", self.client_id.as_str()),
        ];
        if let Some(secret) = self.client_secret.as_deref() {
            form.push(("client_secret", secret));
        }
        let token_resp = send_token_form(&self.http, &self.token_url, &form).await?;
        let cached = self.process_response(token_resp, platform_account_id)?;
        let stored = (&cached).into_stored(&self.token_type_or_default());
        if let Err(err) = self.store.save(&stored).await {
            tracing::warn!("failed to persist refreshed dbt State auth token: {err}");
        }
        Ok(cached)
    }

    fn process_response(
        &self,
        response: TokenResponse,
        platform_account_id: Option<String>,
    ) -> Result<CachedToken, RunCacheServiceError> {
        let claims = jwt_claims(&response.id_token)?;
        let scope_str = claims.scope.ok_or_else(|| {
            RunCacheServiceError::Auth("OAuth token is missing scope".to_string())
        })?;
        // Validate the scope is well-formed. Org resolution is deferred to
        // `resolve_org_id` so that an ambiguous or disabled scope surfaces a proper
        // error at request time instead of being treated as a token acquisition
        // failure (which would delete the cache and re-open the browser).
        Scope::from_string(&scope_str)?;
        let expires_at = expires_at_from(&response);

        Ok(CachedToken {
            id_token: response.id_token,
            scope: scope_str,
            expires_at,
            refresh_token: response.refresh_token,
            platform_account_id,
        })
    }
}

/// Builds the dbt platform credential chain used for token exchange, pinned to
/// the configured platform account when one is set. Without the pin, a
/// `~/.dbt/oauth_sessions.json` holding sessions for several accounts resolves
/// whichever session comes first, which may not be the account the project is
/// configured against.
fn platform_auth_chain(platform_account_id: Option<&str>) -> AuthChain {
    let builder = AuthChainBuilder::new(OAUTH_CLIENT_ID);
    match platform_account_id {
        Some(account_id) => builder.account_id(account_id).build(),
        None => builder.build(),
    }
}

fn expires_at_from(response: &TokenResponse) -> Option<SystemTime> {
    if let Some(seconds) = response.expires_at {
        return Some(epoch_seconds_to_system_time(seconds));
    }
    response
        .expires_in
        .map(|secs| SystemTime::now() + duration_from_seconds(secs))
}

fn epoch_seconds_to_system_time(seconds: f64) -> SystemTime {
    UNIX_EPOCH + duration_from_seconds(seconds)
}

fn duration_from_seconds(seconds: f64) -> Duration {
    if seconds.is_finite() && seconds > 0.0 {
        Duration::from_secs_f64(seconds)
    } else {
        Duration::ZERO
    }
}

trait IntoStored {
    fn into_stored(self, token_type: &str) -> StoredToken;
}

impl IntoStored for &CachedToken {
    fn into_stored(self, token_type: &str) -> StoredToken {
        StoredToken {
            scope: self.scope.clone(),
            token_type: token_type.to_string(),
            id_token: self.id_token.clone(),
            expires_at: self
                .expires_at
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64()),
            access_token: None,
            refresh_token: self.refresh_token.clone(),
            platform_account_id: self.platform_account_id.clone(),
        }
    }
}

trait FromStored {
    fn into_cached(self) -> CachedToken;
}

impl FromStored for StoredToken {
    fn into_cached(self) -> CachedToken {
        CachedToken {
            id_token: self.id_token,
            scope: self.scope,
            expires_at: self.expires_at.map(epoch_seconds_to_system_time),
            refresh_token: self.refresh_token,
            platform_account_id: self.platform_account_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service_config::DEFAULT_OAUTH_AUTH_URL;
    use async_trait::async_trait;
    use dbt_platform_auth::resolver::{AuthResolver, CloudYamlResolver};
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde::Serialize;
    use std::sync::Mutex as StdMutex;
    use tempfile::TempDir;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Builds an `AuthChain` with no resolvers — `resolve()` deterministically
    /// returns `NotAuthenticated`. Used by tests that exercise the
    /// interactive-flow or disk-cache paths and must not be influenced by the
    /// test process's env vars or `~/.dbt/*` files.
    fn empty_auth_chain() -> AuthChain {
        AuthChain::new(vec![])
    }

    /// Writes a `dbt_cloud.yml` into `dir` describing the given platform
    /// credential and returns an `AuthChain` that resolves it via
    /// `CloudYamlResolver`.
    ///
    /// This feeds the credential through a file rather than `DBT_CLOUD_*`
    /// process env vars on purpose: mutating the environment from a test is
    /// unsound in the multi-threaded test binary (`std::env::set_var` races
    /// with any other thread reading the environment — e.g. a concurrent
    /// `reqwest::Client` build), which made unrelated auth tests flaky.
    fn cloud_yaml_auth_chain(
        dir: &TempDir,
        token: &str,
        host: &str,
        account_id: &str,
    ) -> AuthChain {
        let cloud_path = dir.path().join("dbt_cloud.yml");
        let yaml = format!(
            r#"version: "1"
context:
  active-project: "test-project"
  active-host: "{host}"
projects:
  - project-name: "Test Project"
    project-id: "test-project"
    account-name: "acme"
    account-id: "{account_id}"
    account-host: "{host}"
    token-name: "test-token"
    token-value: "{token}"
"#
        );
        std::fs::write(&cloud_path, yaml).unwrap();
        AuthChain::new(vec![AuthResolver::CloudYaml(CloudYamlResolver {
            path: Some(cloud_path),
            // Point at a path that does not exist so no ambient `dbt_project.yml`
            // in the working directory can influence resolution.
            dbt_project_path: Some(dir.path().join("no_dbt_project.yml")),
        })])
    }

    fn make_jwt(scope: &str) -> String {
        #[derive(Serialize)]
        struct Claims<'a> {
            scope: &'a str,
        }
        encode(
            &Header::default(),
            &Claims { scope },
            &EncodingKey::from_secret(b"test-secret"),
        )
        .unwrap()
    }

    fn config_with(
        server_url: &str,
        secret: Option<&str>,
        org_id: Option<&str>,
    ) -> RunCacheServiceConfig {
        let mut config = RunCacheServiceConfig::disabled();
        config.enabled = true;
        config.oauth_token_url = format!("{server_url}/token");
        config.oauth_auth_url = DEFAULT_OAUTH_AUTH_URL.to_string();
        config.oauth_client_id = "test-client".to_string();
        config.oauth_client_secret = secret.map(str::to_string);
        config.org_id = org_id.map(str::to_string);
        config.timeout = Duration::from_secs(5);
        config
    }

    fn token_store_in(dir: &TempDir) -> TokenStore {
        let auth_home = dir.path().join(".dbt");
        TokenStore::discover_from(Some(auth_home.to_string_lossy().into_owned()), None).unwrap()
    }

    struct FakeFlow {
        responses: StdMutex<Vec<TokenResponse>>,
        available: bool,
    }

    impl FakeFlow {
        fn new(responses: Vec<TokenResponse>) -> Arc<Self> {
            Arc::new(Self {
                responses: StdMutex::new(responses),
                available: true,
            })
        }

        /// A flow that reports itself as unable to run (e.g. no interactive
        /// terminal), mirroring `BrowserFlow` in a non-interactive environment.
        fn unavailable() -> Arc<Self> {
            Arc::new(Self {
                responses: StdMutex::new(vec![]),
                available: false,
            })
        }
    }

    #[async_trait]
    impl InteractiveFlow for FakeFlow {
        fn is_available(&self) -> bool {
            self.available
        }

        async fn run(&self) -> Result<TokenResponse, RunCacheServiceError> {
            let mut q = self.responses.lock().unwrap();
            if q.is_empty() {
                return Err(RunCacheServiceError::Auth("FakeFlow drained".to_string()));
            }
            Ok(q.remove(0))
        }
    }

    fn token_response(scope: &str, expires_in: f64, refresh: Option<&str>) -> TokenResponse {
        TokenResponse {
            id_token: make_jwt(scope),
            access_token: Some("access".to_string()),
            refresh_token: refresh.map(str::to_string),
            scope: Some(scope.to_string()),
            token_type: Some("Bearer".to_string()),
            expires_at: None,
            expires_in: Some(expires_in),
        }
    }

    #[dbt_runtime::test]
    async fn m2m_happy_path_sends_basic_auth() {
        let server = MockServer::start().await;
        let scope = "runcache:scope:org:dev:admin";
        let token_resp = serde_json::json!({
            "id_token": make_jwt(scope),
            "scope": scope,
            "token_type": "Bearer",
            "expires_in": 3600,
        });
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=client_credentials"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_resp))
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let config = config_with(&server.uri(), Some("secret-x"), Some("dev"));
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            empty_auth_chain(),
        )
        .unwrap();

        let token = source.token().await.unwrap();
        assert_eq!(source.resolve_org_id(&token).unwrap(), "dev");

        // Inspect server requests to confirm Basic auth header.
        let requests = server.received_requests().await.unwrap();
        let auth_header = requests[0]
            .headers
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(auth_header.starts_with("Basic "));
    }

    #[dbt_runtime::test]
    async fn token_endpoint_retries_transient_failures() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let config = config_with(&server.uri(), Some("secret-x"), Some("dev"));
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            empty_auth_chain(),
        )
        .unwrap();

        let err = source.token().await.unwrap_err();
        assert!(matches!(err, RunCacheServiceError::AuthRequest(_)));
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[dbt_runtime::test]
    async fn token_endpoint_does_not_retry_auth_failures() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let config = config_with(&server.uri(), Some("secret-x"), Some("dev"));
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            empty_auth_chain(),
        )
        .unwrap();

        let err = source.token().await.unwrap_err();
        assert!(matches!(err, RunCacheServiceError::AuthRequest(_)));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[dbt_runtime::test]
    async fn browser_happy_path_includes_client_id_in_form() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        let config = config_with(&server.uri(), None, Some("dev"));

        let scope = "runcache:scope:org:dev:admin";
        let fake = FakeFlow::new(vec![token_response(scope, 3600.0, Some("refresh-xyz"))]);
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            fake,
            empty_auth_chain(),
        )
        .unwrap();

        let token = source.token().await.unwrap();
        assert_eq!(source.resolve_org_id(&token).unwrap(), "dev");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh-xyz"));
    }

    #[dbt_runtime::test]
    async fn unavailable_interactive_flow_fails_fast_instead_of_running() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        let config = config_with(&server.uri(), None, Some("dev"));

        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::unavailable(),
            empty_auth_chain(),
        )
        .unwrap();

        let err = source.token().await.unwrap_err();
        assert!(matches!(err, RunCacheServiceError::NoInteractiveTerminal));
        // `is_user_actionable_auth` gates whether a caller like
        // `validate_client_version_fail_open` treats the failure as
        // something to surface vs. silently skip. Not having a terminal to
        // complete a login is an environment limitation, not something the
        // user can act on in the moment, so this must not be actionable.
        assert!(!err.is_user_actionable_auth());
    }

    #[dbt_runtime::test]
    async fn refresh_path_replaces_stored_token() {
        let server = MockServer::start().await;
        let scope = "runcache:scope:org:dev:admin";
        let new_token = make_jwt(scope);
        let token_resp = serde_json::json!({
            "id_token": new_token,
            "scope": scope,
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": "refresh-new",
        });
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_resp))
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let store = token_store_in(&dir);
        // Seed disk with a stale token.
        let stale = StoredToken {
            scope: scope.to_string(),
            token_type: "Bearer".to_string(),
            id_token: make_jwt(scope),
            expires_at: Some(1.0), // ancient
            access_token: None,
            refresh_token: Some("refresh-old".to_string()),
            platform_account_id: None,
        };
        store.save(&stale).await.unwrap();

        let config = config_with(&server.uri(), None, Some("dev"));
        let source = OAuthTokenSource::with_components(
            &config,
            store,
            FakeFlow::new(vec![]),
            empty_auth_chain(),
        )
        .unwrap();

        let token = source.token().await.unwrap();
        assert_eq!(token.id_token, new_token);
        assert_eq!(token.refresh_token.as_deref(), Some("refresh-new"));
    }

    #[dbt_runtime::test]
    async fn org_disabled_returns_dedicated_error() {
        let server = MockServer::start().await;
        let scope = "runcache:scope:app:dev:developer"; // only app, no org
        let token_resp = serde_json::json!({
            "id_token": make_jwt(scope),
            "scope": scope,
            "token_type": "Bearer",
            "expires_in": 3600,
        });
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_resp))
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let config = config_with(&server.uri(), Some("secret"), Some("dev"));
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            empty_auth_chain(),
        )
        .unwrap();

        // The token is acquired and persisted successfully; the disabled-org error
        // surfaces from resolve_org_id, not from token acquisition.
        let token = source.token().await.unwrap();
        let err = source.resolve_org_id(&token).unwrap_err();
        assert!(matches!(err, RunCacheServiceError::OrgDisabled { ref org_id } if org_id == "dev"));
    }

    #[dbt_runtime::test]
    async fn disk_token_with_mismatched_org_is_refreshed() {
        // A configured org_id that isn't in the cached scope forces a refresh so a
        // permissions change is picked up without deleting the cache or re-opening the
        // browser.
        let server = MockServer::start().await;
        let scope_new = "runcache:scope:org:primary:admin";
        let token_resp = serde_json::json!({
            "id_token": make_jwt(scope_new),
            "scope": scope_new,
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": "refresh-new",
        });
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_resp))
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let store = token_store_in(&dir);
        // Disk grants only "other"; the configured org "primary" isn't in scope yet.
        store
            .save(&StoredToken {
                scope: "runcache:scope:org:other:admin".to_string(),
                token_type: "Bearer".to_string(),
                id_token: make_jwt("runcache:scope:org:other:admin"),
                expires_at: Some(9_999_999_999.0),
                access_token: None,
                refresh_token: Some("refresh-old".to_string()),
                platform_account_id: None,
            })
            .await
            .unwrap();

        let config = config_with(&server.uri(), Some("secret"), Some("primary"));
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            empty_auth_chain(),
        )
        .unwrap();

        let token = source.token().await.unwrap();
        assert_eq!(source.resolve_org_id(&token).unwrap(), "primary");
    }

    #[dbt_runtime::test]
    async fn fresh_disk_token_is_used_without_network_call() {
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        let store = token_store_in(&dir);
        let scope = "runcache:scope:org:dev:admin";
        store
            .save(&StoredToken {
                scope: scope.to_string(),
                token_type: "Bearer".to_string(),
                id_token: make_jwt(scope),
                expires_at: Some(9_999_999_999.0),
                access_token: None,
                refresh_token: None,
                platform_account_id: None,
            })
            .await
            .unwrap();

        let config = config_with(&server.uri(), None, Some("dev"));
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            empty_auth_chain(),
        )
        .unwrap();

        let token = source.token().await.unwrap();
        assert_eq!(source.resolve_org_id(&token).unwrap(), "dev");
        // No mock was set up; the FakeFlow would have errored. Implicit assertion.
    }

    #[dbt_runtime::test]
    async fn ambiguous_fresh_cached_token_raises_when_no_org_configured() {
        // A previous run used a configured org_id, so a valid multi-org token was cached.
        // A subsequent run without a configured org_id must fail rather than silently
        // reusing the org_id picked at write time.
        let dir = TempDir::new().unwrap();
        let store = token_store_in(&dir);
        let scope = "runcache:scope:org:org1:admin runcache:scope:org:org2:admin";
        store
            .save(&StoredToken {
                scope: scope.to_string(),
                token_type: "Bearer".to_string(),
                id_token: make_jwt(scope),
                expires_at: Some(9_999_999_999.0),
                access_token: None,
                refresh_token: Some("refresh".to_string()),
                platform_account_id: None,
            })
            .await
            .unwrap();

        let config = config_with("http://127.0.0.1:1", None, None); // no org configured
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            empty_auth_chain(),
        )
        .unwrap();

        let token = source.token().await.unwrap();
        let err = source.resolve_org_id(&token).unwrap_err();
        assert!(err.to_string().contains("state-org-id"));
    }

    #[dbt_runtime::test]
    async fn ambiguity_during_refresh_raises_without_browser_flow() {
        // A near-expiry token is refreshed and the refresh returns a multi-org scope.
        // With no configured org_id the ambiguity must surface from resolve_org_id, not
        // be treated as a token acquisition failure that re-opens the browser.
        let server = MockServer::start().await;
        let multi_org = "runcache:scope:org:org1:admin runcache:scope:org:org2:admin";
        let token_resp = serde_json::json!({
            "id_token": make_jwt(multi_org),
            "scope": multi_org,
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": "new-refresh",
        });
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_resp))
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let store = token_store_in(&dir);
        let near_expiry = (SystemTime::now() + Duration::from_secs(200))
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        store
            .save(&StoredToken {
                scope: "runcache:scope:org:org1:admin".to_string(),
                token_type: "Bearer".to_string(),
                id_token: make_jwt("runcache:scope:org:org1:admin"),
                expires_at: Some(near_expiry),
                access_token: None,
                refresh_token: Some("old-refresh".to_string()),
                platform_account_id: None,
            })
            .await
            .unwrap();

        let config = config_with(&server.uri(), None, None);
        // FakeFlow is empty: if the browser flow were reached it would error.
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            empty_auth_chain(),
        )
        .unwrap();

        let token = source.token().await.unwrap();
        let err = source.resolve_org_id(&token).unwrap_err();
        assert!(err.to_string().contains("state-org-id"));

        // The refreshed token was persisted (refresh_token rotated) — cache must remain.
        let saved = token_store_in(&dir).load().await.unwrap().unwrap();
        assert_eq!(saved.refresh_token.as_deref(), Some("new-refresh"));
    }

    #[dbt_runtime::test]
    async fn disabled_org_disk_token_surfaces_error_without_reauth() {
        // When the cached token grants no active access to the configured org (the user
        // was disabled), the cache must be kept and the disabled error surfaced from
        // resolve_org_id rather than re-triggering the browser flow.
        let dir = TempDir::new().unwrap();
        let store = token_store_in(&dir);
        let scope = "runcache:scope:app:org1:developer"; // app-only: org access disabled
        store
            .save(&StoredToken {
                scope: scope.to_string(),
                token_type: "Bearer".to_string(),
                id_token: make_jwt(scope),
                expires_at: Some(9_999_999_999.0),
                access_token: None,
                refresh_token: None,
                platform_account_id: None,
            })
            .await
            .unwrap();

        let config = config_with("http://127.0.0.1:1", None, Some("org1"));
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            empty_auth_chain(),
        )
        .unwrap();

        let token = source.token().await.unwrap();
        let err = source.resolve_org_id(&token).unwrap_err();
        assert!(
            matches!(err, RunCacheServiceError::OrgDisabled { ref org_id } if org_id == "org1")
        );

        // The cache must still be present — no deletion, no browser re-auth.
        assert!(token_store_in(&dir).load().await.unwrap().is_some());
    }

    #[dbt_runtime::test]
    async fn platform_token_exchange_succeeds_when_no_client_secret() {
        let server = MockServer::start().await;
        let scope = "runcache:scope:org:dev:admin";
        let token_resp = serde_json::json!({
            "id_token": make_jwt(scope),
            "scope": scope,
            "token_type": "Bearer",
            "expires_in": 3600,
        });
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains(
                "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange",
            ))
            .and(body_string_contains("subject_token_type=dbt"))
            .and(body_string_contains("subject_token=dbtc_platform_token"))
            .and(body_string_contains("dbt_hostname=ab123.us1.dbt.com"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_resp))
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let auth_chain =
            cloud_yaml_auth_chain(&dir, "dbtc_platform_token", "ab123.us1.dbt.com", "42");
        let config = config_with(&server.uri(), None, Some("dev"));
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            auth_chain,
        )
        .unwrap();

        let token = source.token().await.unwrap();
        assert_eq!(source.resolve_org_id(&token).unwrap(), "dev");
    }

    #[dbt_runtime::test]
    async fn cached_token_from_other_platform_account_is_discarded_and_reminted() {
        let server = MockServer::start().await;
        // Distinct scopes let the returned token identify its origin: only a
        // token-exchange response carries the admin scope. The mock matches the
        // exchange grant alone, so a refresh attempt would go unmatched and fail
        // the request rather than silently pass.
        let cached_scope = "runcache:scope:org:dev:developer";
        let minted_scope = "runcache:scope:org:dev:admin";
        let token_resp = serde_json::json!({
            "id_token": make_jwt(minted_scope),
            "scope": minted_scope,
            "token_type": "Bearer",
            "expires_in": 3600,
        });
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains(
                "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_resp))
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let store = token_store_in(&dir);
        // A still-fresh, refreshable token minted for account 1.
        store
            .save(&StoredToken {
                scope: cached_scope.to_string(),
                token_type: "Bearer".to_string(),
                id_token: make_jwt(cached_scope),
                expires_at: Some(9_999_999_999.0),
                access_token: None,
                refresh_token: Some("refresh-old".to_string()),
                platform_account_id: Some("1".to_string()),
            })
            .await
            .unwrap();

        // Configuration now selects account 2.
        let auth_chain =
            cloud_yaml_auth_chain(&dir, "dbtc_platform_token", "ab123.us1.dbt.com", "2");
        let mut config = config_with(&server.uri(), None, Some("dev"));
        config.platform_account_id = Some("2".to_string());
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            auth_chain,
        )
        .unwrap();

        let token = source.token().await.unwrap();
        // Freshly exchanged against account 2 — neither the cached token nor its
        // refresh token was reused.
        assert_eq!(token.scope, minted_scope);
        assert_eq!(token.refresh_token, None);
        assert_eq!(token.platform_account_id.as_deref(), Some("2"));
    }

    #[dbt_runtime::test]
    async fn credential_for_another_platform_account_is_rejected() {
        // The only resolvable credential belongs to account 111 while the
        // project configures account 42. Exchanging it would authenticate as
        // the wrong account, so dbt State reports the missing session instead —
        // an error that fails validation open, bypassing dbt State for the run.
        let dir = TempDir::new().unwrap();
        let auth_chain =
            cloud_yaml_auth_chain(&dir, "dbtc_platform_token", "ab123.us1.dbt.com", "111");
        // Unreachable token URL: no exchange may be attempted.
        let mut config = config_with("http://127.0.0.1:1", None, Some("dev"));
        config.platform_account_id = Some("42".to_string());
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![token_response(
                "runcache:scope:org:dev:admin",
                3600.0,
                None,
            )]),
            auth_chain,
        )
        .unwrap();

        let err = source.token().await.unwrap_err();
        assert!(matches!(
            err,
            RunCacheServiceError::PlatformAccountUnavailable {
                ref configured,
                found: Some(ref found),
            } if configured == "42" && found == "111"
        ));
        assert!(!err.is_user_actionable_auth(), "must fail validation open");
        assert!(token_store_in(&dir).load().await.unwrap().is_none());
    }

    #[dbt_runtime::test]
    async fn configured_account_without_any_credential_skips_standalone_login() {
        // With an account configured, the standalone dbt State browser login
        // cannot produce a credential for it, so it must not run.
        let dir = TempDir::new().unwrap();
        let mut config = config_with("http://127.0.0.1:1", None, Some("dev"));
        config.platform_account_id = Some("42".to_string());
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![token_response(
                "runcache:scope:org:dev:admin",
                3600.0,
                None,
            )]),
            empty_auth_chain(),
        )
        .unwrap();

        let err = source.token().await.unwrap_err();
        assert!(matches!(
            err,
            RunCacheServiceError::PlatformAccountUnavailable {
                ref configured,
                found: None,
            } if configured == "42"
        ));
        assert!(token_store_in(&dir).load().await.unwrap().is_none());
    }

    #[dbt_runtime::test]
    async fn cached_token_for_configured_platform_account_is_reused() {
        let dir = TempDir::new().unwrap();
        let store = token_store_in(&dir);
        let scope = "runcache:scope:org:dev:admin";
        let id_token = make_jwt(scope);
        store
            .save(&StoredToken {
                scope: scope.to_string(),
                token_type: "Bearer".to_string(),
                id_token: id_token.clone(),
                expires_at: Some(9_999_999_999.0),
                access_token: None,
                refresh_token: None,
                platform_account_id: Some("42".to_string()),
            })
            .await
            .unwrap();

        // Unreachable token URL: reuse must not hit the network.
        let mut config = config_with("http://127.0.0.1:1", None, Some("dev"));
        config.platform_account_id = Some("42".to_string());
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            empty_auth_chain(),
        )
        .unwrap();

        let token = source.token().await.unwrap();
        assert_eq!(token.id_token, id_token);
    }

    #[dbt_runtime::test]
    async fn cached_token_without_platform_account_is_kept() {
        // Tokens of unknown provenance (standalone dbt State login, or a file
        // written before the account was recorded) are not invalidated.
        let dir = TempDir::new().unwrap();
        let store = token_store_in(&dir);
        let scope = "runcache:scope:org:dev:admin";
        let id_token = make_jwt(scope);
        store
            .save(&StoredToken {
                scope: scope.to_string(),
                token_type: "Bearer".to_string(),
                id_token: id_token.clone(),
                expires_at: Some(9_999_999_999.0),
                access_token: None,
                refresh_token: None,
                platform_account_id: None,
            })
            .await
            .unwrap();

        let mut config = config_with("http://127.0.0.1:1", None, Some("dev"));
        config.platform_account_id = Some("42".to_string());
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            empty_auth_chain(),
        )
        .unwrap();

        let token = source.token().await.unwrap();
        assert_eq!(token.id_token, id_token);
        assert!(token_store_in(&dir).load().await.unwrap().is_some());
    }

    #[dbt_runtime::test]
    async fn platform_token_exchange_skipped_when_client_secret_set() {
        let server = MockServer::start().await;
        let scope = "runcache:scope:org:dev:admin";
        let token_resp = serde_json::json!({
            "id_token": make_jwt(scope),
            "scope": scope,
            "token_type": "Bearer",
            "expires_in": 3600,
        });
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_resp))
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let config = config_with(&server.uri(), Some("client-secret"), Some("dev"));
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            empty_auth_chain(),
        )
        .unwrap();

        let token = source.token().await.unwrap();
        assert_eq!(source.resolve_org_id(&token).unwrap(), "dev");

        // Verify only client_credentials was used — no token-exchange.
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        let body = std::str::from_utf8(&requests[0].body).unwrap();
        assert!(body.contains("grant_type=client_credentials"));
        assert!(!body.contains("token-exchange"));
    }

    #[dbt_runtime::test]
    async fn platform_token_exchange_propagates_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("token-exchange"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let auth_chain =
            cloud_yaml_auth_chain(&dir, "dbtc_platform_token", "ab123.us1.dbt.com", "42");
        let config = config_with(&server.uri(), None, Some("dev"));
        // FakeFlow would error if reached — exchange failure must propagate, not fall back.
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::new(vec![]),
            auth_chain,
        )
        .unwrap();

        let err = source.token().await.unwrap_err();
        assert!(matches!(err, RunCacheServiceError::AuthRequest(_)));
    }

    #[dbt_runtime::test]
    async fn headless_mint_does_not_cache_token_to_disk() {
        // Tokens minted from re-derivable client credentials are usable in memory but are
        // never written to disk in a headless environment (no interactive user present, e.g.
        // CI), where the cache offers no reuse benefit and would leave credentials behind on
        // ephemeral runners. A flow that reports itself unavailable models that environment.
        let server = MockServer::start().await;
        let scope = "runcache:scope:org:dev:admin";
        let token_resp = serde_json::json!({
            "id_token": make_jwt(scope),
            "scope": scope,
            "token_type": "Bearer",
            "expires_in": 3600,
        });
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=client_credentials"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_resp))
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let config = config_with(&server.uri(), Some("secret-x"), Some("dev"));
        let source = OAuthTokenSource::with_components(
            &config,
            token_store_in(&dir),
            FakeFlow::unavailable(),
            empty_auth_chain(),
        )
        .unwrap();

        let token = source.token().await.unwrap();
        assert_eq!(source.resolve_org_id(&token).unwrap(), "dev");
        assert!(token_store_in(&dir).load().await.unwrap().is_none());
        // Skipping the write must also skip creating the auth directory, so nothing is
        // left behind on the runner.
        assert!(!dir.path().join(".dbt").exists());
    }

    #[dbt_runtime::test]
    async fn headless_refresh_from_disk_persists_rotated_token() {
        // A token provisioned on disk (no client credentials) is the only auth path in a
        // headless environment once the browser flow is disabled. Refresh tokens rotate on
        // use, so a headless refresh must write the new token back or the next run would
        // refresh with an already-invalidated token and then delete the provisioned cache.
        // The unavailable flow models the headless environment; the refresh path persists
        // regardless.
        let server = MockServer::start().await;
        let scope = "runcache:scope:org:dev:admin";
        let token_resp = serde_json::json!({
            "id_token": make_jwt(scope),
            "scope": scope,
            "token_type": "Bearer",
            "expires_in": 3600,
            "refresh_token": "new-refresh",
        });
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_resp))
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let store = token_store_in(&dir);
        // Seed disk with a stale token carrying a refresh token.
        store
            .save(&StoredToken {
                scope: scope.to_string(),
                token_type: "Bearer".to_string(),
                id_token: make_jwt(scope),
                expires_at: Some(1.0), // ancient
                access_token: None,
                refresh_token: Some("old-refresh".to_string()),
                platform_account_id: None,
            })
            .await
            .unwrap();

        let config = config_with(&server.uri(), None, Some("dev"));
        let source = OAuthTokenSource::with_components(
            &config,
            store,
            FakeFlow::unavailable(),
            empty_auth_chain(),
        )
        .unwrap();

        let token = source.token().await.unwrap();
        assert_eq!(token.refresh_token.as_deref(), Some("new-refresh"));

        // The rotated refresh token must have been written back to disk.
        let saved = token_store_in(&dir).load().await.unwrap().unwrap();
        assert_eq!(saved.refresh_token.as_deref(), Some("new-refresh"));
    }
}
