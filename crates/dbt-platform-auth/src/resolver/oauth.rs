use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

use crate::credential::OAuthSession;
use crate::session_cache::OAuthSessionCache;
use crate::{AuthError, Credential};

// ── Public constants ──────────────────────────────────────────────────────────

/// Default loopback port for the OAuth redirect listener.
pub const LOOPBACK_PORT: u16 = 29527;

/// Default timeout for the interactive browser flow. Matches the VSCode extension (10 min).
pub const INTERACTIVE_TIMEOUT: Duration = Duration::from_secs(600);

/// Default OAuth scopes requested from dbt platform.
pub const OAUTH_SCOPES: &str = "account:read identity:read offline_access";

/// dbt platform registration endpoint. Users are redirected here to start login.
/// Override with `DBT_CLOUD_REGISTER_URL` for testing against non-production environments.
pub const REGISTER_URL: &str = "https://us1.dbt.com/register";

// ── Opener / abort handle ─────────────────────────────────────────────────────

/// Callback invoked with the authorization URL. Implementations open a browser or
/// modify the URL before doing so.
pub type Opener = Box<dyn Fn(&str) + Send + Sync>;

/// A handle that can abort an in-progress [`OAuthInteractiveResolver`] flow from
/// another thread. Dropping the handle without calling [`abort`](OAuthAbortHandle::abort)
/// has no effect.
pub struct OAuthAbortHandle(tokio::sync::oneshot::Sender<()>);

impl OAuthAbortHandle {
    pub fn new(tx: tokio::sync::oneshot::Sender<()>) -> Self {
        Self(tx)
    }

    /// Signal the in-progress flow to stop. The resolver will return
    /// [`AuthError::Aborted`].
    pub fn abort(self) {
        let _ = self.0.send(());
    }
}

// ── Passive resolver ──────────────────────────────────────────────────────────

/// Resolves credentials from a cached OAuth session — no user interaction.
///
/// Checks the OAuth session cache (default: `~/.dbt/oauth_sessions.json`) for a
/// non-expired session and returns it directly. If the access token is expired
/// but a refresh token is present, this resolver will attempt to exchange the
/// refresh token for new credentials before returning.
///
/// # Client ID and account matching
///
/// Only sessions for the given `client_id` are considered. When `account_id` is
/// set, only sessions for that dbt platform account are considered — a cache
/// holding sessions for several accounts then resolves the configured one
/// instead of whichever session happens to come first. If no session matches
/// the requested account, [`AuthError::NotAuthenticated`] is returned so the
/// caller's chain can fall through to another resolver rather than silently
/// authenticating against the wrong account.
///
/// Accounts are compared as strings, the form every caller's configuration
/// carries them in; the session's numeric id is rendered for the comparison.
///
/// # Scope checking
///
/// When `scopes` is non-empty, the resolver only returns a session whose granted
/// scopes are a superset of the requested scopes. If a valid session exists but
/// its scopes are insufficient, [`AuthError::InadequateScopes`] is returned.
/// An empty `scopes` list skips the check.
///
/// # Non-interactive contract
///
/// This resolver never opens a browser or prompts the user. For an interactive
/// login flow, see [`OAuthInteractiveResolver`].
pub struct OAuthPassiveResolver {
    /// OAuth client ID for this application.
    pub client_id: String,
    /// Scopes required by the caller. Session must grant all of these.
    pub scopes: Vec<String>,
    /// dbt platform account the session must belong to. `None` accepts any account.
    pub account_id: Option<String>,
    /// Override for the session cache path. Defaults to `~/.dbt/oauth_sessions.json`.
    pub cache_path: Option<PathBuf>,
    /// HTTP client used for token refresh requests.
    pub http: reqwest::Client,
    /// Override the token endpoint base URL (scheme + host). Defaults to
    /// `https://{account_host}` derived from the cached session. Useful for testing.
    pub token_endpoint_override: Option<String>,
}

impl OAuthPassiveResolver {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            scopes: vec![],
            account_id: None,
            cache_path: None,
            http: reqwest::Client::default(),
            token_endpoint_override: None,
        }
    }

    fn effective_cache_path(&self) -> Option<PathBuf> {
        self.cache_path
            .clone()
            .or_else(|| dirs::home_dir().map(|h| h.join(".dbt").join("oauth_sessions.json")))
    }

    fn refresh_token_url(&self, account_host: &str) -> String {
        let base = match &self.token_endpoint_override {
            Some(b) => b.trim_end_matches('/').to_owned(),
            None => format!("https://{account_host}"),
        };
        format!("{base}/oauth/token")
    }

    pub async fn resolve(&self) -> Result<Credential, AuthError> {
        let path = match self.effective_cache_path() {
            Some(p) => p,
            None => return Err(AuthError::NotAuthenticated),
        };

        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AuthError::NotAuthenticated);
            }
            Err(e) => return Err(AuthError::InaccessibleSource(e)),
        };

        let cache: OAuthSessionCache =
            serde_json::from_slice(&bytes).map_err(|e| AuthError::Malformed(e.to_string()))?;

        let now = SystemTime::now();

        let matching: Vec<&OAuthSession> = cache
            .sessions
            .iter()
            .filter(|s| s.client_id == self.client_id)
            .filter(|s| {
                self.account_id
                    .as_deref()
                    .is_none_or(|want| s.account_id.to_string() == want)
            })
            .collect();

        if matching.is_empty() {
            return Err(AuthError::NotAuthenticated);
        }

        let non_expired: Vec<&OAuthSession> = matching
            .iter()
            .copied()
            .filter(|s| s.expires_at > now)
            .collect();

        if !non_expired.is_empty() {
            if let Some(session) = non_expired
                .iter()
                .copied()
                .find(|s| scopes_adequate(&self.scopes, &s.scopes))
            {
                return Ok(Credential::OAuth(session.clone()));
            }
            return Err(AuthError::InadequateScopes {
                requested: self.scopes.clone(),
                cached: non_expired[0].scopes.clone(),
            });
        }

        // All matching sessions are expired. Try to refresh if a refresh token is available.
        if let Some(session) = matching.iter().copied().find(|s| s.refresh_token.is_some()) {
            return self.refresh_session(&path, session).await;
        }
        Err(AuthError::AuthenticationExpired)
    }

    /// Refreshes a matching cached session even when its access token has not
    /// yet expired, for callers handling a rejected token.
    pub async fn force_refresh(&self) -> Result<Credential, AuthError> {
        let path = match self.effective_cache_path() {
            Some(p) => p,
            None => return Err(AuthError::NotAuthenticated),
        };

        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(AuthError::NotAuthenticated);
            }
            Err(e) => return Err(AuthError::InaccessibleSource(e)),
        };
        let cache: OAuthSessionCache =
            serde_json::from_slice(&bytes).map_err(|e| AuthError::Malformed(e.to_string()))?;
        let matching: Vec<&OAuthSession> = cache
            .sessions
            .iter()
            .filter(|s| s.client_id == self.client_id)
            .filter(|s| {
                self.account_id
                    .as_deref()
                    .is_none_or(|want| s.account_id.to_string() == want)
            })
            .collect();

        if matching.is_empty() {
            return Err(AuthError::NotAuthenticated);
        }
        let session = matching
            .into_iter()
            .find(|session| session.refresh_token.is_some())
            .ok_or(AuthError::AuthenticationExpired)?;

        self.refresh_session(&path, session).await
    }

    async fn refresh_session(
        &self,
        path: &PathBuf,
        session: &OAuthSession,
    ) -> Result<Credential, AuthError> {
        let refresh_tok = session
            .refresh_token
            .as_deref()
            .ok_or(AuthError::AuthenticationExpired)?;
        let token_url = self.refresh_token_url(&session.account_host);
        let token =
            exchange_refresh_token(&self.http, &token_url, &self.client_id, refresh_tok).await?;

        let (user_id, account_id, account_host) = decode_access_token(&token.access_token)
            .map_err(|e| AuthError::RefreshFailed(e.to_string()))?;
        if account_id != session.account_id {
            return Err(AuthError::RefreshFailed(
                "refreshed credential belongs to a different account".into(),
            ));
        }
        let hosts_match = match (
            normalized_account_host(&session.account_host),
            normalized_account_host(&account_host),
        ) {
            (Some(expected), Some(actual)) => expected == actual,
            _ => false,
        };
        if !hosts_match {
            return Err(AuthError::RefreshFailed(
                "refreshed credential belongs to a different account host".into(),
            ));
        }

        let scopes: Vec<String> = token
            .scope
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_owned)
            .collect();

        let expires_in = token.expires_in.unwrap_or(3600.0);
        let expires_at = SystemTime::now() + Duration::from_secs(expires_in as u64);

        let new_session = OAuthSession {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            id_token: token.id_token,
            scopes,
            expires_at,
            account_host,
            account_id,
            user_id,
            client_id: self.client_id.clone(),
        };

        if !scopes_adequate(&self.scopes, &new_session.scopes) {
            return Err(AuthError::InadequateScopes {
                requested: self.scopes.clone(),
                cached: new_session.scopes,
            });
        }

        upsert_session(path, new_session.clone())?;

        Ok(Credential::OAuth(new_session))
    }
}

// ── Interactive resolver ──────────────────────────────────────────────────────

/// Resolves credentials by initiating an interactive OAuth authorization code flow.
///
/// Opens a browser to the dbt platform registration endpoint and spins up a local
/// redirect server to capture the authorization code. On success the resulting
/// session is persisted to the OAuth session cache and returned as a credential.
///
/// Construct via [`OAuthInteractiveResolver::builder`] (passing a `client_id`). An
/// [`OAuthAbortHandle`] can be wired in via [`OAuthInteractiveResolverBuilder::abort_signal`]
/// to cancel the flow from another thread, causing [`resolve`](Self::resolve) to return
/// [`AuthError::Aborted`].
pub struct OAuthInteractiveResolver {
    client_id: String,
    opener: Opener,
    timeout: Duration,
    redirect_port: u16,
    register_url: String,
    scopes: String,
    source_application: Option<String>,
    http: reqwest::Client,
    cache_path: Option<PathBuf>,
    // Wrapped in Mutex so resolve() can take the receiver with &self.
    abort_signal: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl OAuthInteractiveResolver {
    /// Construct with all default settings for `client_id` (uses [`LOOPBACK_PORT`],
    /// [`INTERACTIVE_TIMEOUT`], and the system browser).
    pub fn new(client_id: impl Into<String>) -> Self {
        OAuthInteractiveResolverBuilder::new(client_id).build()
    }

    /// Start building a custom resolver.
    pub fn builder(client_id: impl Into<String>) -> OAuthInteractiveResolverBuilder {
        OAuthInteractiveResolverBuilder::new(client_id)
    }

    /// Default opener: opens the URL in the system browser.
    pub fn default_opener() -> Opener {
        Box::new(|url: &str| {
            if let Err(err) = open::that_detached(url) {
                tracing::warn!(
                    "failed to open browser automatically: {err}. \
                    Open the following URL manually:\n{url}"
                );
            }
        })
    }

    fn effective_cache_path(&self) -> Option<PathBuf> {
        self.cache_path
            .clone()
            .or_else(|| dirs::home_dir().map(|h| h.join(".dbt").join("oauth_sessions.json")))
    }

    pub async fn resolve(&self) -> Result<Credential, AuthError> {
        let cache_path = match self.effective_cache_path() {
            Some(p) => p,
            None => {
                return Err(AuthError::Interactive(
                    "cannot determine home directory for session cache".into(),
                ));
            }
        };

        let redirect_uri = format!("http://localhost:{}/", self.redirect_port);

        // If a session already exists for this client, include its scopes in the new
        // request so that a re-auth never silently drops previously-granted scopes.
        let effective_scopes = {
            let cached_scopes = std::fs::read(&cache_path)
                .ok()
                .and_then(|b| serde_json::from_slice::<OAuthSessionCache>(&b).ok())
                .and_then(|c| {
                    c.sessions
                        .into_iter()
                        .find(|s| s.client_id == self.client_id)
                })
                .map(|s| s.scopes)
                .unwrap_or_default();
            merge_scopes(&self.scopes, &cached_scopes)
        };

        let listener = TcpListener::bind(("127.0.0.1", self.redirect_port))
            .await
            .map_err(|e| {
                AuthError::Interactive(format!("loopback port {} in use: {e}", self.redirect_port))
            })?;

        let pkce = generate_pkce();
        let state = generate_state();

        let auth_url = build_register_url(
            &self.register_url,
            &redirect_uri,
            &self.client_id,
            &effective_scopes,
            &state,
            &pkce.challenge,
            self.source_application.as_deref(),
        )?;

        (self.opener)(&auth_url);

        let abort_rx = self.abort_signal.lock().unwrap().take();
        let redirect = accept_one_redirect(&listener, &state, self.timeout, abort_rx).await?;

        // The dbt platform appends account_url to the callback URL. The server validates
        // the token exchange redirect_uri against this full callback URL (minus code/state),
        // not the bare redirect_uri from the auth request. Mirror VSCE's buildRedirectUriFromCallback.
        let exchange_redirect_uri = {
            let mut url = Url::parse(&redirect_uri)
                .map_err(|e| AuthError::Interactive(format!("malformed redirect URI: {e}")))?;
            url.query_pairs_mut()
                .append_pair("account_url", &redirect.account_url);
            url.to_string()
        };

        let token = exchange_code(
            &self.http,
            &redirect.account_url,
            &self.client_id,
            &redirect.code,
            &exchange_redirect_uri,
            &pkce.verifier,
        )
        .await?;

        let (user_id, account_id, account_host) = decode_access_token(&token.access_token)?;

        let scopes: Vec<String> = token
            .scope
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_owned)
            .collect();

        let expires_in = token.expires_in.unwrap_or(3600.0);
        let expires_at = SystemTime::now() + Duration::from_secs(expires_in as u64);

        let session = OAuthSession {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            id_token: token.id_token,
            scopes,
            expires_at,
            account_host,
            account_id,
            user_id,
            client_id: self.client_id.clone(),
        };

        upsert_session(&cache_path, session.clone())?;

        Ok(Credential::OAuth(session))
    }
}

// ── Builder ───────────────────────────────────────────────────────────────────

/// Builder for [`OAuthInteractiveResolver`]. `client_id` is required at construction;
/// all other fields are optional and fall back to defaults.
pub struct OAuthInteractiveResolverBuilder {
    client_id: String,
    opener: Option<Opener>,
    timeout: Option<Duration>,
    redirect_port: Option<u16>,
    register_url: Option<String>,
    scopes: Option<String>,
    source_application: Option<String>,
    http: Option<reqwest::Client>,
    cache_path: Option<PathBuf>,
    abort_signal: Option<tokio::sync::oneshot::Receiver<()>>,
}

impl OAuthInteractiveResolverBuilder {
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            opener: None,
            timeout: None,
            redirect_port: None,
            register_url: None,
            scopes: None,
            source_application: None,
            http: None,
            cache_path: None,
            abort_signal: None,
        }
    }

    pub fn opener(mut self, v: Opener) -> Self {
        self.opener = Some(v);
        self
    }

    pub fn timeout(mut self, v: Duration) -> Self {
        self.timeout = Some(v);
        self
    }

    pub fn redirect_port(mut self, v: u16) -> Self {
        self.redirect_port = Some(v);
        self
    }

    pub fn register_url(mut self, v: impl Into<String>) -> Self {
        self.register_url = Some(v.into());
        self
    }

    pub fn scopes(mut self, v: impl Into<String>) -> Self {
        self.scopes = Some(v.into());
        self
    }

    /// Set the `_dbtsrc` query parameter sent with the authorization URL to
    /// identify the application initiating the login flow.
    pub fn source_application(mut self, v: impl Into<String>) -> Self {
        self.source_application = Some(v.into());
        self
    }

    pub fn http(mut self, v: reqwest::Client) -> Self {
        self.http = Some(v);
        self
    }

    pub fn cache_path(mut self, v: PathBuf) -> Self {
        self.cache_path = Some(v);
        self
    }

    /// Supply an abort signal. Create the paired [`OAuthAbortHandle`] via
    /// `tokio::sync::oneshot::channel()` and pass the sender to
    /// `OAuthAbortHandle::new(tx)`.
    pub fn abort_signal(mut self, rx: tokio::sync::oneshot::Receiver<()>) -> Self {
        self.abort_signal = Some(rx);
        self
    }

    pub fn build(self) -> OAuthInteractiveResolver {
        let client_id = std::env::var("DBT_OAUTH_CLIENT_ID").unwrap_or(self.client_id);
        let register_url = self
            .register_url
            .or_else(|| std::env::var("DBT_CLOUD_REGISTER_URL").ok())
            .unwrap_or_else(|| REGISTER_URL.to_owned());
        OAuthInteractiveResolver {
            client_id,
            opener: self
                .opener
                .unwrap_or_else(OAuthInteractiveResolver::default_opener),
            timeout: self.timeout.unwrap_or(INTERACTIVE_TIMEOUT),
            redirect_port: self.redirect_port.unwrap_or(LOOPBACK_PORT),
            register_url,
            scopes: self.scopes.unwrap_or_else(|| OAUTH_SCOPES.to_owned()),
            source_application: self.source_application,
            http: self.http.unwrap_or_default(),
            cache_path: self.cache_path,
            abort_signal: std::sync::Mutex::new(self.abort_signal),
        }
    }
}

// ── Scope helpers ─────────────────────────────────────────────────────────────

/// Returns `true` if every scope in `requested` is present in `cached`.
/// An empty `requested` list always returns `true`.
fn scopes_adequate(requested: &[String], cached: &[String]) -> bool {
    if requested.is_empty() {
        return true;
    }
    let cached_set: std::collections::HashSet<&str> = cached.iter().map(String::as_str).collect();
    requested.iter().all(|s| cached_set.contains(s.as_str()))
}

/// Returns the union of `requested` (space-delimited) and `cached`, preserving
/// `requested`-first ordering and deduplicating.
fn merge_scopes(requested: &str, cached: &[String]) -> String {
    let mut seen = std::collections::HashSet::<&str>::new();
    let mut merged: Vec<&str> = Vec::new();
    for scope in requested
        .split_whitespace()
        .chain(cached.iter().map(String::as_str))
    {
        if seen.insert(scope) {
            merged.push(scope);
        }
    }
    merged.join(" ")
}

// ── Private helpers ───────────────────────────────────────────────────────────

struct Pkce {
    verifier: String,
    challenge: String,
}

fn generate_pkce() -> Pkce {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    Pkce {
        verifier,
        challenge,
    }
}

fn generate_state() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn build_register_url(
    base: &str,
    redirect_url: &str,
    client_id: &str,
    scope: &str,
    state: &str,
    code_challenge: &str,
    source_application: Option<&str>,
) -> Result<String, AuthError> {
    let mut url = Url::parse(base)
        .map_err(|e| AuthError::Interactive(format!("invalid register URL '{base}': {e}")))?;
    url.query_pairs_mut()
        .append_pair("redirect_url", redirect_url)
        .append_pair("client_id", client_id)
        .append_pair("code_challenge", code_challenge)
        .append_pair("state", state)
        .append_pair("scope", scope)
        .append_pair("response_type", "code")
        .append_pair("code_challenge_method", "S256");
    if let Some(src) = source_application {
        url.query_pairs_mut().append_pair("_dbtsrc", src);
    }
    Ok(url.to_string())
}

#[derive(Debug)]
struct RedirectResult {
    code: String,
    #[allow(dead_code)]
    state: String,
    account_url: String,
}

async fn accept_one_redirect(
    listener: &TcpListener,
    expected_state: &str,
    timeout: Duration,
    abort_rx: Option<tokio::sync::oneshot::Receiver<()>>,
) -> Result<RedirectResult, AuthError> {
    let accept = async {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|e| AuthError::Interactive(format!("loopback accept failed: {e}")))?;
        handle_redirect(stream, expected_state).await
    };

    if let Some(mut rx) = abort_rx {
        tokio::select! {
            result = tokio::time::timeout(timeout, accept) => {
                match result {
                    Ok(r) => r,
                    Err(_) => Err(AuthError::Interactive(format!(
                        "interactive authentication timed out after {}s",
                        timeout.as_secs()
                    ))),
                }
            }
            _ = &mut rx => Err(AuthError::Aborted),
        }
    } else {
        match tokio::time::timeout(timeout, accept).await {
            Ok(r) => r,
            Err(_) => Err(AuthError::Interactive(format!(
                "interactive authentication timed out after {}s",
                timeout.as_secs()
            ))),
        }
    }
}

const MAX_ERROR_LEN: usize = 200;
const MAX_ERROR_DESC_LEN: usize = 1000;

/// Truncates `s` to at most `max_chars` characters, respecting UTF-8 boundaries.
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

async fn handle_redirect(
    stream: TcpStream,
    expected_state: &str,
) -> Result<RedirectResult, AuthError> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .await
        .map_err(|e| AuthError::Interactive(format!("failed reading redirect request: {e}")))?;

    let target = parse_request_target(&request_line)?;
    let query = parse_query(&target);

    if let Some(error) = query.get("error") {
        let error = if error.is_empty() {
            "unspecified error"
        } else {
            error.as_str()
        };
        let description = query
            .get("error_description")
            .map(String::as_str)
            .filter(|d| !d.is_empty())
            .unwrap_or("no description provided");
        let message = format!(
            "{}: {}",
            truncate_chars(error, MAX_ERROR_LEN),
            truncate_chars(description, MAX_ERROR_DESC_LEN),
        );
        let _ = write_http_response(&mut write_half, 500, &error_html(&message)).await;
        return Err(AuthError::Interactive(message));
    }

    let state = query
        .get("state")
        .cloned()
        .ok_or_else(|| AuthError::Interactive("redirect missing state parameter".into()))?;
    if state != expected_state {
        let _ = write_http_response(&mut write_half, 500, &error_html("invalid state")).await;
        return Err(AuthError::Interactive(
            "invalid OAuth state parameter".into(),
        ));
    }

    let code = query
        .get("code")
        .cloned()
        .ok_or_else(|| AuthError::Interactive("redirect missing code parameter".into()))?;

    let account_url = query
        .get("account_url")
        .cloned()
        .ok_or_else(|| AuthError::Interactive("redirect missing account_url parameter".into()))?;

    let _ = write_http_response(&mut write_half, 200, &success_html()).await;
    Ok(RedirectResult {
        code,
        state,
        account_url,
    })
}

fn parse_request_target(request_line: &str) -> Result<String, AuthError> {
    let mut parts = request_line.split_whitespace();
    let _method = parts.next();
    let target = parts
        .next()
        .ok_or_else(|| AuthError::Interactive("malformed redirect request line".into()))?;
    Ok(target.to_string())
}

fn parse_query(target: &str) -> HashMap<String, String> {
    let url = Url::parse(&format!("http://127.0.0.1{target}")).ok();
    let mut map = HashMap::new();
    if let Some(url) = url {
        for (k, v) in url.query_pairs() {
            map.insert(k.into_owned(), v.into_owned());
        }
    }
    map
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

async fn write_http_response(
    stream: &mut tokio::net::tcp::OwnedWriteHalf,
    status: u16,
    body: &str,
) -> std::io::Result<()> {
    let status_text = if status == 200 {
        "OK"
    } else {
        "Internal Server Error"
    };
    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

// Shared page chrome: full <head>, styles, body open, .main open, logo.
// Does NOT close .main — page-specific tail does that.
const PAGE_START: &str = r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="UTF-8"/>
  <title>dbt - Login</title>
  <style>
    *, *::before, *::after { margin: 0; padding: 0; box-sizing: border-box; }
    body {
      background: #1c1c1c;
      color: #fff;
      font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
      min-height: 100vh;
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
    }
    .main { max-width: 32rem; width: 100%; padding: 2rem 1rem; }
    .logo { display: flex; align-items: center; gap: 10px; margin-bottom: 56px; }
    .logo-text { font-size: 22px; font-weight: 600; }
    h1 { font-size: 28px; font-weight: 700; margin-bottom: 10px; }
    .subtitle { font-size: 16px; color: #c8c8c8; margin-bottom: 40px; }
    .links { text-align: center; }
    .links p { color: #c8c8c8; font-size: 15px; margin-bottom: 20px; }
    .links a { color: #fff; }
    .banner {
      display: flex; align-items: flex-start; gap: 12px;
      background: rgba(220,38,38,0.10);
      border: 1px solid rgba(220,38,38,0.35);
      border-left: 3px solid #dc2626;
      border-radius: 6px;
      padding: 14px 16px;
      max-width: 480px;
      margin-bottom: 1.5rem;
    }
    .banner-icon { flex-shrink: 0; color: #f87171; margin-top: 1px; }
    .banner-title { font-size: 14px; font-weight: 600; color: #fca5a5; margin-bottom: 4px; }
    .banner-message { font-size: 14px; color: #fca5a5; }
  </style>
</head>
<body>
  <div class="main">
    <div class="logo">
      <svg width="30" height="30" viewBox="0 0 30 30" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M24.9764 0.341745C26.2996 -0.423111 27.7247 0.188774 28.7935 1.25957C29.9132 2.38136 30.3712 3.60513 29.6587 4.98187C29.4042 5.49177 26.4014 10.6928 25.4853 12.1715C24.9764 12.9874 24.7219 14.0072 24.7219 14.976C24.7219 15.9448 24.9764 16.9646 25.4853 17.8315C26.4014 19.2592 29.4042 24.5112 29.6587 25.0211C30.3712 26.4488 29.8623 27.5196 28.8444 28.6414C27.6738 29.8142 26.5032 30.4261 25.0782 29.6612C24.5692 29.3553 10.3186 21.1458 10.3186 21.1458C10.5731 22.8285 11.4892 24.3582 12.6598 25.276C12.5071 25.327 5.45938 29.405 4.97461 29.6612C3.63859 30.3673 2.4203 29.8952 1.36105 28.8964C0.164945 27.7684 -0.420273 26.4488 0.343153 25.0721C0.597628 24.5622 3.60044 19.3102 4.46565 17.8824C4.97461 17.0156 5.27998 16.0468 5.27998 15.027C5.27998 14.0072 4.97461 13.0384 4.46565 12.2225C3.60044 10.6928 0.597628 5.38979 0.343153 4.93088C-0.420273 3.55414 0.192658 2.07531 1.25926 1.1066C2.46833 0.00850936 3.60044 -0.32113 4.97461 0.341745C5.38177 0.494716 19.5815 8.90813 19.5815 8.90813C19.4288 7.27644 18.6145 5.79772 17.2912 4.77791C17.393 4.72692 24.4674 0.545706 24.9764 0.341745ZM15.5099 17.5255L17.5457 15.4859C17.8002 15.2309 17.8002 14.823 17.5457 14.5171L15.5099 12.4775C15.2045 12.1715 14.7974 12.1715 14.492 12.4775L12.4562 14.5171C12.2017 14.772 12.2017 15.2309 12.4562 15.4859L14.492 17.5255C14.7465 17.7805 15.2045 17.7805 15.5099 17.5255Z" fill="#FE6702"/></svg>
      <span class="logo-text">dbt</span>
    </div>"##;

const SUCCESS_TAIL: &str = r#"
    <h1>You&#8217;re all set</h1>
    <p class="subtitle">Your account is ready. You can close this tab and return to the CLI</p>
  </div>
  <div class="links">
    <p>Need a service token for authentication? <a href="https://cloud.getdbt.com/settings">Go to Account settings</a></p>
    <p>Still need help? <a href="https://docs.getdbt.com">Contact us</a></p>
  </div>
</body>
</html>"#;

const ERROR_BANNER_START: &str = r#"
    <div class="banner">
      <svg class="banner-icon" width="18" height="18" viewBox="0 0 20 20" fill="currentColor"><path fill-rule="evenodd" d="M10 18a8 8 0 100-16 8 8 0 000 16zM8.707 7.293a1 1 0 00-1.414 1.414L8.586 10l-1.293 1.293a1 1 0 101.414 1.414L10 11.414l1.293 1.293a1 1 0 001.414-1.414L11.414 10l1.293-1.293a1 1 0 00-1.414-1.414L10 8.586 8.707 7.293z" clip-rule="evenodd"/></svg>
      <div>
        <div class="banner-title">Authentication failed</div>
        <div class="banner-message">"#;

const ERROR_BANNER_END: &str = r#"</div>
      </div>
    </div>
    <p class="subtitle">You can close this tab and return to the CLI.</p>
  </div>
</body>
</html>"#;

fn success_html() -> String {
    [PAGE_START, SUCCESS_TAIL].concat()
}

fn error_html(message: &str) -> String {
    [
        PAGE_START,
        ERROR_BANNER_START,
        &html_escape(message),
        ERROR_BANNER_END,
    ]
    .concat()
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    expires_in: Option<f64>,
}

async fn exchange_code(
    http: &reqwest::Client,
    account_url: &str,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<TokenResponse, AuthError> {
    let token_url = format!("{}/oauth/token", account_url.trim_end_matches('/'));
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("code_verifier", code_verifier),
        ("redirect_uri", redirect_uri),
    ];
    let response = http
        .post(&token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| AuthError::Interactive(format!("token exchange request failed: {e}")))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| AuthError::Interactive(format!("reading token response failed: {e}")))?;
    if !status.is_success() {
        return Err(AuthError::Interactive(format!(
            "token exchange failed ({status}): {body}"
        )));
    }
    serde_json::from_str::<TokenResponse>(&body)
        .map_err(|e| AuthError::Interactive(format!("invalid token response: {e}")))
}

async fn exchange_refresh_token(
    http: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenResponse, AuthError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    let response = http
        .post(token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| AuthError::RefreshFailed(e.to_string()))?;

    // 4xx means the token was revoked or invalid; the user must interactively re-authenticate.
    if response.status().is_client_error() {
        return Err(AuthError::AuthenticationExpired);
    }

    let response = response
        .error_for_status()
        .map_err(|e| AuthError::RefreshFailed(format!("token refresh request failed: {e}")))?;

    let body = response
        .text()
        .await
        .map_err(|e| AuthError::RefreshFailed(format!("reading token response failed: {e}")))?;

    serde_json::from_str::<TokenResponse>(&body)
        .map_err(|e| AuthError::RefreshFailed(format!("invalid token response: {e}")))
}

fn jwt_claim_as_u64(payload: &serde_json::Value, key: &str) -> Option<u64> {
    let v = &payload[key];
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}

fn normalized_account_host(host: &str) -> Option<String> {
    Url::parse(&format!("https://{host}"))
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

fn decode_access_token(token: &str) -> Result<(u64, u64, String), AuthError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(AuthError::Interactive(
            "access token is not a valid JWT".into(),
        ));
    }

    // JWT uses base64url encoding without padding
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|e| AuthError::Interactive(format!("failed to decode JWT payload: {e}")))?;

    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes)
        .map_err(|e| AuthError::Interactive(format!("failed to parse JWT payload: {e}")))?;
    let user_id = jwt_claim_as_u64(&payload, "sub")
        .ok_or_else(|| AuthError::Interactive("JWT missing or invalid 'sub' claim".into()))?;

    let account_id = jwt_claim_as_u64(&payload, "https://dbt.com/account_id").ok_or_else(|| {
        AuthError::Interactive("JWT missing or invalid 'https://dbt.com/account_id' claim".into())
    })?;

    let iss = payload["iss"]
        .as_str()
        .ok_or_else(|| AuthError::Interactive("JWT missing 'iss' claim".into()))?;

    // Strip scheme from the issuer URL to get just the host
    let account_host = Url::parse(iss)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .ok_or_else(|| {
            AuthError::Interactive(format!("cannot extract host from JWT 'iss': {iss}"))
        })?;

    Ok((user_id, account_id, account_host))
}

fn upsert_session(cache_path: &PathBuf, session: OAuthSession) -> Result<(), AuthError> {
    let mut cache = if cache_path.exists() {
        let bytes = std::fs::read(cache_path).map_err(AuthError::InaccessibleSource)?;
        serde_json::from_slice::<OAuthSessionCache>(&bytes)
            .map_err(|e| AuthError::Malformed(e.to_string()))?
    } else {
        OAuthSessionCache {
            version: 1,
            sessions: Vec::new(),
        }
    };

    // Replace any existing session for the same (client_id, account_id) pair.
    let existing = cache
        .sessions
        .iter()
        .position(|s| s.client_id == session.client_id && s.account_id == session.account_id);
    if let Some(idx) = existing {
        cache.sessions[idx] = session;
    } else {
        cache.sessions.push(session);
    }

    let json =
        serde_json::to_vec_pretty(&cache).map_err(|e| AuthError::Malformed(e.to_string()))?;

    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(AuthError::InaccessibleSource)?;
    }

    write_secure(&json, cache_path).map_err(AuthError::InaccessibleSource)?;

    Ok(())
}

#[cfg(unix)]
fn write_secure(data: &[u8], path: &PathBuf) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    std::io::Write::write_all(&mut file, data)
}

#[cfg(windows)]
fn write_secure(data: &[u8], path: &PathBuf) -> std::io::Result<()> {
    let mut file = dbt_file_security::open_owner_only(path)?;
    std::io::Write::write_all(&mut file, data)
}

#[cfg(not(any(unix, windows)))]
fn write_secure(data: &[u8], path: &PathBuf) -> std::io::Result<()> {
    std::fs::write(path, data)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_cache::OAuthSessionCache;
    use std::io::Write as _;
    use std::time::{Duration, UNIX_EPOCH};
    use tempfile::NamedTempFile;
    use tokio::sync::oneshot;

    fn write_cache(cache: &OAuthSessionCache) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&serde_json::to_vec(cache).unwrap()).unwrap();
        f
    }

    fn resolver_at(path: PathBuf) -> OAuthPassiveResolver {
        OAuthPassiveResolver {
            client_id: "test_client".into(),
            scopes: vec![],
            account_id: None,
            cache_path: Some(path),
            http: reqwest::Client::new(),
            token_endpoint_override: None,
        }
    }

    /// Spin up a one-shot mock HTTP server that reads one request and writes back
    /// `status` with `body`. Returns the bound address.
    async fn mock_token_server(status: u16, body: String) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut reader = BufReader::new(read);
            // Drain headers, find Content-Length.
            let mut content_length: usize = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                if line == "\r\n" {
                    break;
                }
                let lower = line.to_lowercase();
                if let Some(rest) = lower.strip_prefix("content-length:") {
                    content_length = rest.trim().parse().unwrap_or(0);
                }
            }
            // Drain body so the client doesn't get a broken-pipe on write.
            {
                use tokio::io::AsyncReadExt as _;
                let mut buf = vec![0u8; content_length];
                let _ = reader.read_exact(&mut buf).await;
            }
            let status_text = if status == 200 { "OK" } else { "Unauthorized" };
            let resp = format!(
                "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            write.write_all(resp.as_bytes()).await.unwrap();
            write.shutdown().await.unwrap();
        });
        addr
    }

    async fn mock_token_server_with_request(
        status: u16,
        body: String,
    ) -> (std::net::SocketAddr, oneshot::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (request_sender, request_receiver) = oneshot::channel();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut reader = BufReader::new(read);
            let mut content_length: usize = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                if line == "\r\n" {
                    break;
                }
                let lower = line.to_lowercase();
                if let Some(rest) = lower.strip_prefix("content-length:") {
                    content_length = rest.trim().parse().unwrap_or(0);
                }
            }
            use tokio::io::AsyncReadExt as _;
            let mut request_body = vec![0u8; content_length];
            reader.read_exact(&mut request_body).await.unwrap();
            let _ = request_sender.send(String::from_utf8(request_body).unwrap());
            let status_text = if status == 200 { "OK" } else { "Unauthorized" };
            let resp = format!(
                "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            write.write_all(resp.as_bytes()).await.unwrap();
            write.shutdown().await.unwrap();
        });
        (addr, request_receiver)
    }

    fn make_fake_jwt(user_id: u64, account_id: u64, account_host: &str) -> String {
        let payload = serde_json::json!({
            "sub": user_id.to_string(),
            "https://dbt.com/account_id": account_id.to_string(),
            "iss": format!("https://{account_host}")
        });
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let body_enc = URL_SAFE_NO_PAD.encode(payload.to_string());
        format!("{header}.{body_enc}.fakesig")
    }

    fn future_time() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(u32::MAX as u64)
    }

    fn past_time() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1)
    }

    fn make_session(expires_at: SystemTime, refresh_token: Option<String>) -> OAuthSession {
        OAuthSession {
            access_token: "tok_abc".into(),
            refresh_token,
            id_token: None,
            scopes: vec![],
            expires_at,
            account_host: "ab123.us1.dbt.com".into(),
            account_id: 42,
            user_id: 7,
            client_id: "test_client".into(),
        }
    }

    #[dbt_runtime::test]
    async fn returns_valid_session() {
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(future_time(), None)],
        };
        let f = write_cache(&cache);
        let cred = resolver_at(f.path().to_path_buf()).resolve().await.unwrap();
        assert!(matches!(cred, Credential::OAuth(_)));
        assert_eq!(cred.token(), "tok_abc");
        assert_eq!(cred.account_id(), 42);
    }

    #[dbt_runtime::test]
    async fn force_refreshes_selected_unexpired_session_and_leaves_other_accounts_untouched() {
        let new_access_token = make_fake_jwt(7, 42, "ab123.us1.dbt.com");
        let token_response = serde_json::json!({
            "access_token": new_access_token,
            "refresh_token": "selected_new_refresh",
            "scope": "account:read offline_access",
            "expires_in": 3600.0
        })
        .to_string();
        let (addr, mut request_body) = mock_token_server_with_request(200, token_response).await;

        let mut other = make_session(future_time(), Some("other_refresh".into()));
        other.account_id = 41;
        other.access_token = "other_access".into();
        let selected = make_session(future_time(), Some("selected_refresh".into()));
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![other.clone(), selected.clone()],
        };
        let f = write_cache(&cache);
        let mut resolver = resolver_at(f.path().to_path_buf());
        resolver.account_id = Some("42".into());
        resolver.token_endpoint_override = Some(format!("http://127.0.0.1:{}", addr.port()));

        let resolved = resolver.resolve().await.unwrap();
        assert_eq!(resolved.token(), selected.access_token);
        assert!(matches!(
            request_body.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));

        let refreshed = resolver.force_refresh().await.unwrap();
        let request_body = request_body.await.unwrap();
        assert!(request_body.contains("grant_type=refresh_token"));
        assert!(request_body.contains("refresh_token=selected_refresh"));
        assert!(request_body.contains("client_id=test_client"));
        assert_eq!(refreshed.token(), new_access_token);
        assert_eq!(refreshed.account_id(), 42);

        let bytes = std::fs::read(f.path()).unwrap();
        let updated: OAuthSessionCache = serde_json::from_slice(&bytes).unwrap();
        let updated_other = updated
            .sessions
            .iter()
            .find(|session| session.account_id == 41)
            .unwrap();
        let updated_selected = updated
            .sessions
            .iter()
            .find(|session| session.account_id == 42)
            .unwrap();
        assert_eq!(updated_other.access_token, other.access_token);
        assert_eq!(updated_other.refresh_token, other.refresh_token);
        assert_eq!(updated_selected.access_token, new_access_token);
        assert_eq!(
            updated_selected.refresh_token.as_deref(),
            Some("selected_new_refresh")
        );
    }

    #[dbt_runtime::test]
    async fn force_refresh_without_matching_session_returns_not_authenticated() {
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(future_time(), Some("refresh".into()))],
        };
        let f = write_cache(&cache);
        let mut resolver = resolver_at(f.path().to_path_buf());
        resolver.account_id = Some("77".into());

        let error = resolver.force_refresh().await.unwrap_err();
        assert!(matches!(error, AuthError::NotAuthenticated));
    }

    #[dbt_runtime::test]
    async fn force_refresh_without_refresh_token_returns_authentication_expired() {
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(future_time(), None)],
        };
        let f = write_cache(&cache);
        let mut resolver = resolver_at(f.path().to_path_buf());
        resolver.account_id = Some("42".into());

        let error = resolver.force_refresh().await.unwrap_err();
        assert!(matches!(error, AuthError::AuthenticationExpired));
    }

    #[dbt_runtime::test]
    async fn force_refresh_rejects_a_token_for_a_different_account() {
        let token_response = serde_json::json!({
            "access_token": make_fake_jwt(7, 41, "ab123.us1.dbt.com"),
            "refresh_token": "new_refresh",
            "expires_in": 3600.0
        })
        .to_string();
        let addr = mock_token_server(200, token_response).await;
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(future_time(), Some("refresh".into()))],
        };
        let f = write_cache(&cache);
        let before = std::fs::read(f.path()).unwrap();
        let mut resolver = resolver_at(f.path().to_path_buf());
        resolver.account_id = Some("42".into());
        resolver.token_endpoint_override = Some(format!("http://127.0.0.1:{}", addr.port()));

        let error = resolver.force_refresh().await.unwrap_err();
        assert!(
            matches!(error, AuthError::RefreshFailed(message) if message.contains("different account"))
        );
        assert_eq!(std::fs::read(f.path()).unwrap(), before);
    }

    #[dbt_runtime::test]
    async fn force_refresh_rejects_a_token_for_a_different_account_host() {
        let token_response = serde_json::json!({
            "access_token": make_fake_jwt(7, 42, "other.us1.dbt.com"),
            "refresh_token": "new_refresh",
            "expires_in": 3600.0
        })
        .to_string();
        let addr = mock_token_server(200, token_response).await;
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(future_time(), Some("refresh".into()))],
        };
        let f = write_cache(&cache);
        let before = std::fs::read(f.path()).unwrap();
        let mut resolver = resolver_at(f.path().to_path_buf());
        resolver.account_id = Some("42".into());
        resolver.token_endpoint_override = Some(format!("http://127.0.0.1:{}", addr.port()));

        let error = resolver.force_refresh().await.unwrap_err();
        assert!(
            matches!(error, AuthError::RefreshFailed(message) if message.contains("different account host"))
        );
        assert_eq!(std::fs::read(f.path()).unwrap(), before);
    }

    #[dbt_runtime::test]
    async fn force_refresh_accepts_an_equivalent_account_host() {
        let token_response = serde_json::json!({
            "access_token": make_fake_jwt(7, 42, "ab123.us1.dbt.com"),
            "refresh_token": "new_refresh",
            "expires_in": 3600.0
        })
        .to_string();
        let addr = mock_token_server(200, token_response).await;
        let mut session = make_session(future_time(), Some("refresh".into()));
        session.account_host = "AB123.US1.dbt.com".into();
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![session],
        };
        let f = write_cache(&cache);
        let mut resolver = resolver_at(f.path().to_path_buf());
        resolver.account_id = Some("42".into());
        resolver.token_endpoint_override = Some(format!("http://127.0.0.1:{}", addr.port()));

        let credential = resolver.force_refresh().await.unwrap();
        assert_eq!(credential.account_id(), 42);
    }

    #[dbt_runtime::test]
    async fn expired_refresh_rejects_a_token_for_a_different_account() {
        let token_response = serde_json::json!({
            "access_token": make_fake_jwt(7, 41, "ab123.us1.dbt.com"),
            "refresh_token": "new_refresh",
            "expires_in": 3600.0
        })
        .to_string();
        let addr = mock_token_server(200, token_response).await;
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(past_time(), Some("refresh".into()))],
        };
        let f = write_cache(&cache);
        let before = std::fs::read(f.path()).unwrap();
        let mut resolver = resolver_at(f.path().to_path_buf());
        resolver.token_endpoint_override = Some(format!("http://127.0.0.1:{}", addr.port()));

        let error = resolver.resolve().await.unwrap_err();
        assert!(
            matches!(error, AuthError::RefreshFailed(message) if message.contains("different account"))
        );
        assert_eq!(std::fs::read(f.path()).unwrap(), before);
    }

    #[dbt_runtime::test]
    async fn expired_refresh_rejects_a_token_for_a_different_account_host() {
        let token_response = serde_json::json!({
            "access_token": make_fake_jwt(7, 42, "other.us1.dbt.com"),
            "refresh_token": "new_refresh",
            "expires_in": 3600.0
        })
        .to_string();
        let addr = mock_token_server(200, token_response).await;
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(past_time(), Some("refresh".into()))],
        };
        let f = write_cache(&cache);
        let before = std::fs::read(f.path()).unwrap();
        let mut resolver = resolver_at(f.path().to_path_buf());
        resolver.token_endpoint_override = Some(format!("http://127.0.0.1:{}", addr.port()));

        let error = resolver.resolve().await.unwrap_err();
        assert!(
            matches!(error, AuthError::RefreshFailed(message) if message.contains("different account host"))
        );
        assert_eq!(std::fs::read(f.path()).unwrap(), before);
    }

    #[dbt_runtime::test]
    async fn expired_refresh_accepts_an_equivalent_account_host() {
        let token_response = serde_json::json!({
            "access_token": make_fake_jwt(7, 42, "ab123.us1.dbt.com"),
            "refresh_token": "new_refresh",
            "expires_in": 3600.0
        })
        .to_string();
        let addr = mock_token_server(200, token_response).await;
        let mut session = make_session(past_time(), Some("refresh".into()));
        session.account_host = "AB123.US1.dbt.com".into();
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![session],
        };
        let f = write_cache(&cache);
        let mut resolver = resolver_at(f.path().to_path_buf());
        resolver.token_endpoint_override = Some(format!("http://127.0.0.1:{}", addr.port()));

        let credential = resolver.resolve().await.unwrap();
        assert_eq!(credential.account_id(), 42);
    }

    #[dbt_runtime::test]
    async fn missing_cache_file_returns_not_authenticated() {
        let r = resolver_at(PathBuf::from("/nonexistent/oauth_sessions.json"));
        let err = r.resolve().await.unwrap_err();
        assert!(matches!(err, AuthError::NotAuthenticated));
    }

    #[dbt_runtime::test]
    async fn malformed_cache_returns_malformed() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"not json {{{{").unwrap();
        let err = resolver_at(f.path().to_path_buf())
            .resolve()
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::Malformed(_)));
    }

    #[dbt_runtime::test]
    async fn empty_sessions_returns_not_authenticated() {
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![],
        };
        let f = write_cache(&cache);
        let err = resolver_at(f.path().to_path_buf())
            .resolve()
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::NotAuthenticated));
    }

    #[dbt_runtime::test]
    async fn expired_session_no_refresh_returns_authentication_expired() {
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(past_time(), None)],
        };
        let f = write_cache(&cache);
        let err = resolver_at(f.path().to_path_buf())
            .resolve()
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::AuthenticationExpired));
    }

    #[dbt_runtime::test]
    async fn expired_session_with_refresh_token_network_error_returns_refresh_failed() {
        // Point token_endpoint_override at a port that immediately closes the connection
        // so the HTTP call fails with a network error → RefreshFailed (not AuthenticationExpired,
        // which would indicate the token was rejected by the server).
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            drop(stream); // close without responding
        });

        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(past_time(), Some("refresh_tok".into()))],
        };
        let f = write_cache(&cache);
        let mut r = resolver_at(f.path().to_path_buf());
        r.token_endpoint_override = Some(format!("http://127.0.0.1:{}", addr.port()));
        let err = r.resolve().await.unwrap_err();
        assert!(
            matches!(err, AuthError::RefreshFailed(_)),
            "expected RefreshFailed, got {err:?}"
        );
    }

    #[dbt_runtime::test]
    async fn expired_session_refresh_token_revoked_returns_authentication_expired() {
        let addr = mock_token_server(401, r#"{"error":"invalid_grant"}"#.into()).await;

        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(past_time(), Some("stale_tok".into()))],
        };
        let f = write_cache(&cache);
        let mut r = resolver_at(f.path().to_path_buf());
        r.token_endpoint_override = Some(format!("http://127.0.0.1:{}", addr.port()));
        let err = r.resolve().await.unwrap_err();
        assert!(
            matches!(err, AuthError::AuthenticationExpired),
            "expected AuthenticationExpired, got {err:?}"
        );
    }

    #[dbt_runtime::test]
    async fn expired_session_refresh_succeeds_returns_new_credential_and_updates_cache() {
        let new_access_token = make_fake_jwt(7, 42, "ab123.us1.dbt.com");
        let token_response = serde_json::json!({
            "access_token": new_access_token,
            "refresh_token": "new_refresh_tok",
            "scope": "account:read offline_access",
            "expires_in": 3600.0
        })
        .to_string();

        let addr = mock_token_server(200, token_response).await;

        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(past_time(), Some("old_refresh_tok".into()))],
        };
        let f = write_cache(&cache);
        let mut r = resolver_at(f.path().to_path_buf());
        r.token_endpoint_override = Some(format!("http://127.0.0.1:{}", addr.port()));
        let cred = r.resolve().await.unwrap();
        assert_eq!(cred.token(), new_access_token);
        assert_eq!(cred.account_id(), 42);

        // Verify cache was updated with the new session.
        let bytes = std::fs::read(f.path()).unwrap();
        let updated: OAuthSessionCache = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(updated.sessions.len(), 1);
        assert_eq!(updated.sessions[0].access_token, new_access_token);
        assert_eq!(
            updated.sessions[0].refresh_token.as_deref(),
            Some("new_refresh_tok")
        );
    }

    #[dbt_runtime::test]
    async fn expired_session_refresh_succeeds_but_inadequate_scopes_returns_inadequate_scopes() {
        let new_access_token = make_fake_jwt(7, 42, "ab123.us1.dbt.com");
        // Server grants only account:read, but caller requires offline_access too.
        let token_response = serde_json::json!({
            "access_token": new_access_token,
            "scope": "account:read",
            "expires_in": 3600.0
        })
        .to_string();

        let addr = mock_token_server(200, token_response).await;

        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(past_time(), Some("old_tok".into()))],
        };
        let f = write_cache(&cache);
        let mut r = resolver_at(f.path().to_path_buf());
        r.scopes = vec!["account:read".into(), "offline_access".into()];
        r.token_endpoint_override = Some(format!("http://127.0.0.1:{}", addr.port()));
        let err = r.resolve().await.unwrap_err();
        assert!(
            matches!(err, AuthError::InadequateScopes { .. }),
            "expected InadequateScopes, got {err:?}"
        );
    }

    // ── Scope tests ───────────────────────────────────────────────────────────

    #[dbt_runtime::test]
    async fn session_with_superset_scopes_is_returned() {
        let mut session = make_session(future_time(), None);
        session.scopes = vec![
            "account:read".into(),
            "offline_access".into(),
            "extra".into(),
        ];
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![session],
        };
        let f = write_cache(&cache);
        let mut r = resolver_at(f.path().to_path_buf());
        r.scopes = vec!["account:read".into(), "offline_access".into()];
        let cred = r.resolve().await.unwrap();
        assert_eq!(cred.token(), "tok_abc");
    }

    #[dbt_runtime::test]
    async fn session_with_insufficient_scopes_returns_inadequate_scopes() {
        let mut session = make_session(future_time(), None);
        session.scopes = vec!["account:read".into()];
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![session],
        };
        let f = write_cache(&cache);
        let mut r = resolver_at(f.path().to_path_buf());
        r.scopes = vec!["account:read".into(), "offline_access".into()];
        let err = r.resolve().await.unwrap_err();
        assert!(matches!(err, AuthError::InadequateScopes { .. }));
    }

    #[dbt_runtime::test]
    async fn first_session_with_adequate_scopes_is_preferred() {
        // Session A: valid, missing offline_access
        // Session B: valid, has all required scopes
        let mut session_a = make_session(future_time(), None);
        session_a.account_id = 1;
        session_a.scopes = vec!["account:read".into()];

        let mut session_b = make_session(future_time(), None);
        session_b.account_id = 2;
        session_b.access_token = "tok_b".into();
        session_b.scopes = vec!["account:read".into(), "offline_access".into()];

        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![session_a, session_b],
        };
        let f = write_cache(&cache);
        let mut r = resolver_at(f.path().to_path_buf());
        r.scopes = vec!["account:read".into(), "offline_access".into()];
        let cred = r.resolve().await.unwrap();
        assert_eq!(cred.token(), "tok_b");
    }

    #[dbt_runtime::test]
    async fn no_session_for_client_id_returns_not_authenticated() {
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(future_time(), None)],
        };
        let f = write_cache(&cache);
        // Different client_id — should not match
        let r = OAuthPassiveResolver {
            client_id: "other_client".into(),
            scopes: vec![],
            account_id: None,
            cache_path: Some(f.path().to_path_buf()),
            http: reqwest::Client::new(),
            token_endpoint_override: None,
        };
        let err = r.resolve().await.unwrap_err();
        assert!(matches!(err, AuthError::NotAuthenticated));
    }

    #[dbt_runtime::test]
    async fn configured_account_id_selects_matching_session() {
        let mut session_a = make_session(future_time(), None);
        session_a.account_id = 1;

        let mut session_b = make_session(future_time(), None);
        session_b.account_id = 2;
        session_b.access_token = "tok_b".into();

        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![session_a, session_b],
        };
        let f = write_cache(&cache);
        let mut r = resolver_at(f.path().to_path_buf());
        r.account_id = Some("2".to_string());
        let cred = r.resolve().await.unwrap();
        assert_eq!(cred.token(), "tok_b");
        assert_eq!(cred.account_id(), 2);
    }

    #[dbt_runtime::test]
    async fn configured_account_id_without_session_returns_not_authenticated() {
        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![make_session(future_time(), None)],
        };
        let f = write_cache(&cache);
        let mut r = resolver_at(f.path().to_path_buf());
        r.account_id = Some("999".to_string());
        let err = r.resolve().await.unwrap_err();
        assert!(matches!(err, AuthError::NotAuthenticated));
    }

    #[dbt_runtime::test]
    async fn configured_account_id_ignores_refresh_token_of_other_account() {
        // The other account's session is expired but refreshable; without an
        // account filter it would be refreshed and returned.
        let mut other = make_session(past_time(), Some("refresh_tok".into()));
        other.account_id = 1;

        let mut wanted = make_session(future_time(), None);
        wanted.account_id = 2;
        wanted.access_token = "tok_wanted".into();

        let cache = OAuthSessionCache {
            version: 1,
            sessions: vec![other, wanted],
        };
        let f = write_cache(&cache);
        let mut r = resolver_at(f.path().to_path_buf());
        r.account_id = Some("2".to_string());
        let cred = r.resolve().await.unwrap();
        assert_eq!(cred.token(), "tok_wanted");
    }

    // ── Scope helper unit tests ───────────────────────────────────────────────

    #[test]
    fn scopes_adequate_empty_request_always_passes() {
        assert!(scopes_adequate(&[], &[]));
        assert!(scopes_adequate(&[], &["account:read".to_owned()]));
    }

    #[test]
    fn scopes_adequate_exact_match_passes() {
        let req = vec!["account:read".to_owned(), "offline_access".to_owned()];
        let cached = vec!["account:read".to_owned(), "offline_access".to_owned()];
        assert!(scopes_adequate(&req, &cached));
    }

    #[test]
    fn scopes_adequate_superset_cached_passes() {
        let req = vec!["account:read".to_owned()];
        let cached = vec!["account:read".to_owned(), "offline_access".to_owned()];
        assert!(scopes_adequate(&req, &cached));
    }

    #[test]
    fn scopes_adequate_missing_scope_fails() {
        let req = vec!["account:read".to_owned(), "offline_access".to_owned()];
        let cached = vec!["account:read".to_owned()];
        assert!(!scopes_adequate(&req, &cached));
    }

    #[test]
    fn merge_scopes_deduplicates_and_keeps_requested_first() {
        let merged = merge_scopes(
            "account:read offline_access",
            &["offline_access".to_owned(), "extra".to_owned()],
        );
        assert_eq!(merged, "account:read offline_access extra");
    }

    #[test]
    fn merge_scopes_with_empty_cached_is_identity() {
        let merged = merge_scopes("account:read offline_access", &[]);
        assert_eq!(merged, "account:read offline_access");
    }

    #[test]
    fn merge_scopes_with_empty_requested_returns_cached() {
        let merged = merge_scopes("", &["account:read".to_owned()]);
        assert_eq!(merged, "account:read");
    }

    // ── PKCE / URL builder tests ──────────────────────────────────────────────

    #[test]
    fn pkce_verifier_is_url_safe_and_correct_length() {
        let pkce = generate_pkce();
        assert!(pkce.verifier.len() >= 43);
        assert!(pkce.verifier.len() <= 128);
        assert!(
            pkce.verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn pkce_challenge_matches_sha256_of_verifier() {
        let pkce = generate_pkce();
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier.as_bytes()));
        assert_eq!(pkce.challenge, expected);
    }

    #[test]
    fn state_has_at_least_22_chars() {
        assert!(generate_state().len() >= 22);
    }

    #[test]
    fn register_url_includes_all_required_params() {
        let url = build_register_url(
            REGISTER_URL,
            "http://localhost:29527/",
            "client-id-x",
            "account:read offline_access",
            "state-abc",
            "challenge-xyz",
            None,
        )
        .unwrap();
        let parsed = Url::parse(&url).unwrap();
        let pairs: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(
            pairs.get("redirect_url").map(String::as_str),
            Some("http://localhost:29527/")
        );
        assert_eq!(
            pairs.get("client_id").map(String::as_str),
            Some("client-id-x")
        );
        assert_eq!(pairs.get("state").map(String::as_str), Some("state-abc"));
        assert_eq!(
            pairs.get("code_challenge").map(String::as_str),
            Some("challenge-xyz")
        );
        assert_eq!(
            pairs.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(pairs.get("response_type").map(String::as_str), Some("code"));
        assert!(!pairs.contains_key("_dbtsrc"));
    }

    #[test]
    fn register_url_includes_dbtsrc_when_source_application_set() {
        let url = build_register_url(
            REGISTER_URL,
            "http://localhost:29527/",
            "client-id-x",
            "account:read offline_access",
            "state-abc",
            "challenge-xyz",
            Some("core_v2"),
        )
        .unwrap();
        let parsed = Url::parse(&url).unwrap();
        let pairs: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(pairs.get("_dbtsrc").map(String::as_str), Some("core_v2"));
    }

    #[test]
    fn decode_access_token_extracts_claims() {
        // Build a fake JWT with the required claims (no signature validation)
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let payload = serde_json::json!({
            "sub": "5001",
            "https://dbt.com/account_id": "1001",
            "iss": "https://ab123.us1.dbt.com"
        });
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let body = URL_SAFE_NO_PAD.encode(payload.to_string());
        let fake_jwt = format!("{header}.{body}.fakesig");

        let (user_id, account_id, account_host) = decode_access_token(&fake_jwt).unwrap();
        assert_eq!(user_id, 5001);
        assert_eq!(account_id, 1001);
        assert_eq!(account_host, "ab123.us1.dbt.com");
    }

    // ── Redirect listener tests ───────────────────────────────────────────────

    use tokio::net::TcpStream;

    async fn bind_local() -> (TcpListener, std::net::SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        (listener, addr)
    }

    async fn send_get(addr: std::net::SocketAddr, target: &str) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let req = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
    }

    #[dbt_runtime::test]
    async fn parses_code_state_and_account_url() {
        let (listener, addr) = bind_local().await;
        let handle = tokio::spawn(async move {
            accept_one_redirect(&listener, "expected-state", Duration::from_secs(5), None).await
        });
        send_get(
            addr,
            "/handler?code=auth-code&state=expected-state&account_url=https%3A%2F%2Fab123.us1.dbt.com",
        )
        .await;
        let result = handle.await.unwrap().unwrap();
        assert_eq!(result.code, "auth-code");
        assert_eq!(result.account_url, "https://ab123.us1.dbt.com");
    }

    #[dbt_runtime::test]
    async fn rejects_mismatched_state() {
        let (listener, addr) = bind_local().await;
        let handle = tokio::spawn(async move {
            accept_one_redirect(&listener, "expected-state", Duration::from_secs(5), None).await
        });
        send_get(
            addr,
            "/handler?code=auth-code&state=other-state&account_url=https%3A%2F%2Fab123.us1.dbt.com",
        )
        .await;
        let err = handle.await.unwrap().unwrap_err();
        assert!(err.to_string().contains("invalid OAuth state parameter"));
    }

    #[dbt_runtime::test]
    async fn surfaces_error_query_param() {
        let (listener, addr) = bind_local().await;
        let handle = tokio::spawn(async move {
            accept_one_redirect(&listener, "expected-state", Duration::from_secs(5), None).await
        });
        send_get(
            addr,
            "/handler?error=access_denied&error_description=User%20canceled",
        )
        .await;
        let err = handle.await.unwrap().unwrap_err();
        assert!(err.to_string().contains("access_denied"));
        assert!(err.to_string().contains("User canceled"));
    }

    #[dbt_runtime::test]
    async fn times_out_when_no_request_arrives() {
        let (listener, _addr) = bind_local().await;
        let result = accept_one_redirect(&listener, "any", Duration::from_millis(100), None).await;
        let err = result.unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }

    #[dbt_runtime::test]
    async fn abort_signal_returns_aborted() {
        let (listener, _addr) = bind_local().await;
        let (tx, rx) = oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            accept_one_redirect(&listener, "any", Duration::from_secs(60), Some(rx)).await
        });
        // Immediately abort — no browser connection will arrive.
        let abort_handle = OAuthAbortHandle::new(tx);
        abort_handle.abort();
        let err = handle.await.unwrap().unwrap_err();
        assert!(matches!(err, AuthError::Aborted));
    }

    #[dbt_runtime::test]
    async fn upsert_replaces_existing_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth_sessions.json");

        let session1 = make_session(future_time(), None);
        upsert_session(&path, session1.clone()).unwrap();

        let mut session2 = session1;
        session2.access_token = "tok_updated".into();
        upsert_session(&path, session2).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let cache: OAuthSessionCache = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(cache.sessions.len(), 1);
        assert_eq!(cache.sessions[0].access_token, "tok_updated");
    }

    #[dbt_runtime::test]
    async fn upsert_appends_different_account() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("oauth_sessions.json");

        let session1 = make_session(future_time(), None);
        let mut session2 = make_session(future_time(), None);
        session2.account_id = 99;

        upsert_session(&path, session1).unwrap();
        upsert_session(&path, session2).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let cache: OAuthSessionCache = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(cache.sessions.len(), 2);
    }
}
