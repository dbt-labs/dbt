use std::sync::Arc;
use std::time::Duration;

use dbt_platform_auth::AuthChain;
use dbt_state::auth::{BrowserFlow, OAuthTokenSource, TokenStore};
use dbt_state::service_config::{DEFAULT_OAUTH_AUTH_URL, RunCacheServiceConfig};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde::Serialize;
use tempfile::TempDir;
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

#[dbt_runtime::test]
async fn browser_flow_end_to_end_writes_token_to_disk_with_600_perms() {
    let server = MockServer::start().await;
    let scope = "runcache:scope:org:dev:admin";
    let token_resp = serde_json::json!({
        "id_token": make_jwt(scope),
        "scope": scope,
        "token_type": "Bearer",
        "expires_in": 3600,
        "refresh_token": "refresh-end-to-end",
    });
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(token_resp))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let auth_home = dir.path().join(".dbt");
    let store =
        TokenStore::discover_from(Some(auth_home.to_string_lossy().into_owned()), None).unwrap();

    let mut config = RunCacheServiceConfig::disabled();
    config.enabled = true;
    config.oauth_token_url = format!("{}/token", server.uri());
    config.oauth_auth_url = DEFAULT_OAUTH_AUTH_URL.to_string();
    config.oauth_client_id = "test-client".to_string();
    config.org_id = Some("dev".to_string());
    config.timeout = Duration::from_secs(5);

    // Use a free port for the loopback server.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let http = reqwest::Client::new();
    let flow = Arc::new(BrowserFlow {
        http: http.clone(),
        auth_url: "http://auth.test".to_string(),
        token_url: format!("{}/token", server.uri()),
        client_id: "test-client".to_string(),
        scope: "runcache:scope:orgs".to_string(),
        timeout: Duration::from_secs(5),
        redirect_port: port,
        abort_signal: std::sync::Mutex::new(None),
        opener: Box::new(move |url: &str| {
            let url = url.to_string();
            tokio::spawn(async move {
                let parsed = Url::parse(&url).unwrap();
                let pairs: std::collections::HashMap<_, _> =
                    parsed.query_pairs().into_owned().collect();
                let state = pairs.get("state").cloned().unwrap();
                let redirect = pairs.get("redirect_uri").cloned().unwrap();
                let cb = format!("{redirect}?code=test-code&state={state}");
                // Give the listener a moment to be ready before we connect.
                tokio::time::sleep(Duration::from_millis(50)).await;
                reqwest::get(&cb).await.ok();
            });
        }),
        // The `opener` above simulates a browser completing the redirect, so
        // this should run regardless of whether the test runner has a TTY.
        available: true,
    });

    // Empty chain so AuthChain::resolve() deterministically returns
    // NotAuthenticated and the test exercises the browser flow instead of
    // whatever platform credentials may be present on the host/CI.
    let auth_chain = AuthChain::new(vec![]);
    let source = OAuthTokenSource::with_components(&config, store, flow, auth_chain).unwrap();
    let token = source.token().await.unwrap();
    assert_eq!(source.resolve_org_id(&token).unwrap(), "dev");
    assert_eq!(token.refresh_token.as_deref(), Some("refresh-end-to-end"));

    let saved = TokenStore::discover_from(Some(auth_home.to_string_lossy().into_owned()), None)
        .unwrap()
        .load()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.refresh_token.as_deref(), Some("refresh-end-to-end"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = auth_home.join("state_auth.json");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
