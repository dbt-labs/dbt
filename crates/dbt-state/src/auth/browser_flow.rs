use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

use crate::service_client::RunCacheServiceError;

pub(crate) struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub(crate) fn generate_pkce() -> Pkce {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    Pkce {
        verifier,
        challenge,
    }
}

pub(crate) fn build_authorize_url(
    auth_url: &str,
    client_id: &str,
    redirect_uri: &str,
    scope: &str,
    state: &str,
    code_challenge: &str,
) -> Result<String, RunCacheServiceError> {
    let mut url = Url::parse(auth_url).map_err(|err| {
        RunCacheServiceError::Auth(format!("invalid RUN_CACHE_AUTH_URL '{auth_url}': {err}"))
    })?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", scope)
        .append_pair("state", state)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url.to_string())
}

pub(crate) fn generate_state() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

const SUCCESS_HTML: &str = "<!doctype html><html><head><meta charset=\"UTF-8\"/><title>dbt State - Login</title></head><body style=\"text-align:center;font-family:sans-serif\"><h1>Success</h1><p>You have logged in. You can close this window.</p></body></html>";
const ERROR_HTML_PREFIX: &str = "<!doctype html><html><head><meta charset=\"UTF-8\"/><title>dbt State - Login</title></head><body style=\"text-align:center;font-family:sans-serif\"><h1>Error</h1><p>";
const ERROR_HTML_SUFFIX: &str = "</p></body></html>";

#[derive(Debug, PartialEq)]
pub(crate) struct RedirectResult {
    pub code: String,
    pub state: String,
}

pub(crate) async fn accept_one_redirect(
    listener: &TcpListener,
    expected_state: &str,
    timeout: Duration,
    abort_rx: Option<tokio::sync::oneshot::Receiver<()>>,
) -> Result<RedirectResult, RunCacheServiceError> {
    let accept = async {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|err| RunCacheServiceError::Auth(format!("loopback accept failed: {err}")))?;
        handle_redirect(stream, expected_state).await
    };

    let abort = async move {
        match abort_rx {
            Some(rx) => {
                let _ = rx.await;
            }
            None => std::future::pending::<()>().await,
        }
    };

    tokio::select! {
        result = tokio::time::timeout(timeout, accept) => {
            match result {
                Ok(r) => r,
                Err(_) => Err(RunCacheServiceError::Timeout(timeout.as_secs())),
            }
        }
        _ = abort => Err(RunCacheServiceError::Aborted),
    }
}

async fn handle_redirect(
    stream: TcpStream,
    expected_state: &str,
) -> Result<RedirectResult, RunCacheServiceError> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await.map_err(|err| {
        RunCacheServiceError::Auth(format!("failed reading redirect request: {err}"))
    })?;

    let target = parse_request_target(&request_line)?;
    let query = parse_query(&target);

    if let Some(error) = query.get("error") {
        let description = query
            .get("error_description")
            .map(String::as_str)
            .unwrap_or("");
        let message = if description.is_empty() {
            error.clone()
        } else {
            format!("{error}: {description}")
        };
        let body = format!(
            "{ERROR_HTML_PREFIX}{}{ERROR_HTML_SUFFIX}",
            html_escape(&message)
        );
        let _ = write_response(&mut write_half, 500, &body).await;
        return Err(RunCacheServiceError::Auth(message));
    }

    let state = query.get("state").cloned().ok_or_else(|| {
        RunCacheServiceError::Auth("redirect missing state parameter".to_string())
    })?;
    if state != expected_state {
        let _ = write_response(
            &mut write_half,
            500,
            &format!("{ERROR_HTML_PREFIX}invalid state{ERROR_HTML_SUFFIX}"),
        )
        .await;
        return Err(RunCacheServiceError::Auth(
            "invalid OAuth state parameter".to_string(),
        ));
    }
    let code = query
        .get("code")
        .cloned()
        .ok_or_else(|| RunCacheServiceError::Auth("redirect missing code parameter".to_string()))?;

    let _ = write_response(&mut write_half, 200, SUCCESS_HTML).await;
    Ok(RedirectResult { code, state })
}

fn parse_request_target(request_line: &str) -> Result<String, RunCacheServiceError> {
    let mut parts = request_line.split_whitespace();
    let _method = parts.next();
    let target = parts
        .next()
        .ok_or_else(|| RunCacheServiceError::Auth("malformed redirect request line".to_string()))?;
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

async fn write_response(
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

pub const LOOPBACK_PORT: u16 = 29525;
pub const INTERACTIVE_TIMEOUT: Duration = Duration::from_secs(300);
pub const TOKEN_HTTP_TIMEOUT: Duration = Duration::from_secs(15);
pub const ORGS_SCOPE: &str = "runcache:scope:orgs";
const TOKEN_HTTP_MAX_RETRIES: usize = 2;

#[derive(Debug, Deserialize, Clone)]
pub struct TokenResponse {
    pub id_token: String,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub expires_at: Option<f64>,
    #[serde(default)]
    pub expires_in: Option<f64>,
}

#[async_trait]
pub trait InteractiveFlow: Send + Sync {
    async fn run(&self) -> Result<TokenResponse, RunCacheServiceError>;

    /// Whether this flow can actually complete in the current environment.
    ///
    /// Browser-based flows need a human to complete a login in a browser; in a
    /// non-interactive environment (CI, batch replay) there is nobody to do
    /// that, so callers should treat the flow as unavailable up front instead
    /// of waiting out the full timeout before failing.
    fn is_available(&self) -> bool {
        true
    }
}

pub type Opener = Box<dyn Fn(&str) + Send + Sync>;

/// A handle that can abort an in-progress [`BrowserFlow`] from another thread.
/// Dropping without calling [`abort`](BrowserAbortHandle::abort) has no effect.
pub struct BrowserAbortHandle(tokio::sync::oneshot::Sender<()>);

impl BrowserAbortHandle {
    pub fn new(tx: tokio::sync::oneshot::Sender<()>) -> Self {
        Self(tx)
    }

    /// Signal the in-progress flow to stop. The flow will return
    /// [`RunCacheServiceError::Aborted`].
    pub fn abort(self) {
        let _ = self.0.send(());
    }
}

pub struct BrowserFlow {
    pub http: reqwest::Client,
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    pub scope: String,
    pub timeout: Duration,
    pub redirect_port: u16,
    pub opener: Opener,
    // Wrapped in Mutex so run() can take the receiver with &self.
    pub abort_signal: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    /// Whether a human is actually available to complete a browser login right
    /// now. Production callers should compute this from the environment (see
    /// [`BrowserFlow::has_attached_terminal`]); tests that simulate the browser
    /// completing the redirect (via a custom `opener`) can set this to `true`
    /// unconditionally to exercise the flow regardless of the test runner's
    /// own TTY state.
    pub available: bool,
}

impl BrowserFlow {
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

    /// Whether an interactive terminal is attached, i.e. there is plausibly a
    /// human present who could complete a browser login.
    pub fn has_attached_terminal() -> bool {
        std::io::IsTerminal::is_terminal(&std::io::stdout())
    }
}

pub(crate) async fn send_token_form(
    http: &reqwest::Client,
    token_url: &str,
    form: &[(&str, &str)],
) -> Result<TokenResponse, RunCacheServiceError> {
    send_token_request(http.post(token_url), form).await
}

pub(crate) async fn send_token_request(
    request: reqwest::RequestBuilder,
    form: &[(&str, &str)],
) -> Result<TokenResponse, RunCacheServiceError> {
    let mut retries_left = TOKEN_HTTP_MAX_RETRIES;
    let mut request = request;
    loop {
        let retry_request = (retries_left > 0).then(|| request.try_clone()).flatten();
        match send_token_form_once(request, form).await {
            Err(err) if retries_left > 0 && is_retryable_token_error(&err) => {
                retries_left -= 1;
                let Some(next_request) = retry_request else {
                    return Err(RunCacheServiceError::Auth(
                        "OAuth token request could not be retried".to_string(),
                    ));
                };
                request = next_request;
            }
            result => return result,
        }
    }
}

async fn send_token_form_once(
    request: reqwest::RequestBuilder,
    form: &[(&str, &str)],
) -> Result<TokenResponse, RunCacheServiceError> {
    let response = request
        .form(form)
        .send()
        .await
        .map_err(RunCacheServiceError::from)?
        .error_for_status()
        .map_err(RunCacheServiceError::from)?;
    let body = response.text().await.map_err(RunCacheServiceError::from)?;
    serde_json::from_str::<TokenResponse>(&body)
        .map_err(|err| RunCacheServiceError::Auth(format!("invalid OAuth token response: {err}")))
}

pub(crate) fn is_retryable_token_error(err: &RunCacheServiceError) -> bool {
    let RunCacheServiceError::AuthRequest(err) = err else {
        return false;
    };
    if err.is_timeout() || err.is_connect() {
        return true;
    }
    err.status()
        .map(|status| {
            status == reqwest::StatusCode::REQUEST_TIMEOUT
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error()
        })
        .unwrap_or(false)
}

#[async_trait]
impl InteractiveFlow for BrowserFlow {
    fn is_available(&self) -> bool {
        self.available
    }

    async fn run(&self) -> Result<TokenResponse, RunCacheServiceError> {
        let redirect_uri = format!("http://127.0.0.1:{}/handler", self.redirect_port);
        let listener = TcpListener::bind(("127.0.0.1", self.redirect_port))
            .await
            .map_err(|err| {
                RunCacheServiceError::Auth(format!(
                    "loopback port {} in use: {err}",
                    self.redirect_port
                ))
            })?;

        let pkce = generate_pkce();
        let state = generate_state();
        let authorize_url = build_authorize_url(
            &self.auth_url,
            &self.client_id,
            &redirect_uri,
            &self.scope,
            &state,
            &pkce.challenge,
        )?;

        (self.opener)(&authorize_url);

        let abort_rx = self
            .abort_signal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let redirect = accept_one_redirect(&listener, &state, self.timeout, abort_rx).await?;

        let form = [
            ("grant_type", "authorization_code"),
            ("code", redirect.code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", self.client_id.as_str()),
            ("code_verifier", pkce.verifier.as_str()),
        ];

        send_token_form(&self.http, &self.token_url, &form).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn authorize_url_includes_all_required_params() {
        let url = build_authorize_url(
            "https://auth.example.com",
            "client-id-x",
            "http://127.0.0.1:29525/handler",
            "runcache:scope:orgs",
            "state-abc",
            "challenge-xyz",
        )
        .unwrap();

        let parsed = Url::parse(&url).unwrap();
        let pairs: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(pairs.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(
            pairs.get("client_id").map(String::as_str),
            Some("client-id-x")
        );
        assert_eq!(
            pairs.get("redirect_uri").map(String::as_str),
            Some("http://127.0.0.1:29525/handler")
        );
        assert_eq!(
            pairs.get("scope").map(String::as_str),
            Some("runcache:scope:orgs")
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
    }

    #[test]
    fn authorize_url_rejects_invalid_base() {
        let err = build_authorize_url(
            "not a url",
            "client-id",
            "http://127.0.0.1:29525/handler",
            "scope",
            "state",
            "challenge",
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid RUN_CACHE_AUTH_URL"));
    }

    #[test]
    fn state_has_at_least_22_chars() {
        // 16 bytes base64url-encoded = 22 chars
        assert!(generate_state().len() >= 22);
    }

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
    async fn parses_code_and_state() {
        let (listener, addr) = bind_local().await;
        let handle = tokio::spawn(async move {
            accept_one_redirect(&listener, "expected-state", Duration::from_secs(5), None).await
        });
        send_get(addr, "/handler?code=auth-code&state=expected-state").await;
        let result = handle.await.unwrap().unwrap();
        assert_eq!(result.code, "auth-code");
        assert_eq!(result.state, "expected-state");
    }

    #[dbt_runtime::test]
    async fn rejects_mismatched_state() {
        let (listener, addr) = bind_local().await;
        let handle = tokio::spawn(async move {
            accept_one_redirect(&listener, "expected-state", Duration::from_secs(5), None).await
        });
        send_get(addr, "/handler?code=auth-code&state=other-state").await;
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
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            accept_one_redirect(&listener, "any", Duration::from_secs(60), Some(rx)).await
        });
        let abort_handle = BrowserAbortHandle::new(tx);
        abort_handle.abort();
        let err = handle.await.unwrap().unwrap_err();
        assert!(matches!(err, RunCacheServiceError::Aborted));
    }
}
