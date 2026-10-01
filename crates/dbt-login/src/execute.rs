use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use dbt_common::cancellation::CancellationToken;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_platform_auth::resolver::{INTERACTIVE_TIMEOUT, OAUTH_SCOPES, OAuthInteractiveResolver};
use dbt_platform_auth::{AuthError, Credential, OAUTH_CLIENT_ID, OAuthSessionCache};
use dbt_state::auth::scope::{Scope, determine_org_id};
use dbt_state::auth::{
    BrowserFlow, InteractiveFlow, LOOPBACK_PORT, ORGS_SCOPE, StoredToken, TokenStore,
};
use dbt_state::service_client::RunCacheServiceError;
use dbt_state::service_config::{
    DEFAULT_OAUTH_AUTH_URL, DEFAULT_OAUTH_CLIENT_ID, DEFAULT_OAUTH_TOKEN_URL,
};
use uuid::Uuid;
use vortex_events::{LoginErrorCode, LoginType, login_event};

use crate::LoginHooks;
use crate::state_guidance::{run_state_guidance, run_state_guidance_after_state_login};

/// Returns the space-separated scope string for the interactive login flow:
/// the default `OAUTH_SCOPES` unioned with any scopes from `DBT_OAUTH_SCOPES`.
/// De-duplicates while preserving first-seen order.
fn interactive_login_scopes(default_scopes: &str, env_scopes: Option<&str>) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for tok in default_scopes
        .split_whitespace()
        .chain(env_scopes.unwrap_or("").split_whitespace())
    {
        if !tok.is_empty() && !seen.contains(&tok) {
            seen.push(tok);
        }
    }
    seen.join(" ")
}

/// Read the access token from the most recently stored platform OAuth session
/// (if any). Used to populate JWT-derived telemetry fields on login failure.
fn read_cached_access_token() -> Option<String> {
    let home = dirs::home_dir()?;
    let path = home.join(".dbt").join("oauth_sessions.json");
    let bytes = std::fs::read(path).ok()?;
    let cache: OAuthSessionCache = serde_json::from_slice(&bytes).ok()?;
    cache
        .sessions
        .into_iter()
        .find(|s| s.client_id == OAUTH_CLIENT_ID)
        .map(|s| s.access_token)
}

/// Maps a platform login failure to its `LoginErrorCode` and, when available,
/// an error message. `AuthError::Aborted` is excluded because it signals that
/// the *other* login callback (dbt State) completed first, not a login failure —
/// call sites must handle it separately and never reach this function with it.
fn login_error_fields(e: &AuthError) -> (LoginErrorCode, Option<String>) {
    match e {
        AuthError::NotAuthenticated => (LoginErrorCode::NotAuthenticated, None),
        AuthError::AuthenticationExpired => (LoginErrorCode::AuthenticationExpired, None),
        AuthError::InaccessibleSource(_) => (LoginErrorCode::InaccessibleSource, None),
        AuthError::Malformed(_) => (LoginErrorCode::MalformedConfig, None),
        AuthError::Interactive(message) => {
            (LoginErrorCode::InteractiveFailure, Some(message.clone()))
        }
        AuthError::Aborted => unreachable!("AuthError::Aborted is handled before this is called"),
        AuthError::InadequateScopes { .. } => (LoginErrorCode::InadequateScopes, None),
        AuthError::RefreshFailed(_) => (LoginErrorCode::RefreshFailed, None),
    }
}

pub async fn execute_login(
    hooks: Arc<dyn LoginHooks>,
    token: &CancellationToken,
    invocation_id: &Uuid,
) -> FsResult<()> {
    // Each opener captures its URL via a oneshot and returns immediately.
    // A separate task joins both URLs, combines them into a single browser open.
    let (state_url_tx, state_url_rx) = tokio::sync::oneshot::channel::<String>();
    let (platform_url_tx, platform_url_rx) = tokio::sync::oneshot::channel::<String>();

    let state_url_tx = Arc::new(Mutex::new(Some(state_url_tx)));
    let platform_url_tx = Arc::new(Mutex::new(Some(platform_url_tx)));

    let state_opener: dbt_state::auth::Opener = {
        let tx = state_url_tx.clone();
        Box::new(move |url: &str| {
            if let Some(sender) = tx.lock().unwrap().take() {
                let _ = sender.send(url.to_string());
            }
        })
    };

    let platform_opener: dbt_platform_auth::resolver::Opener = {
        let tx = platform_url_tx.clone();
        Box::new(move |url: &str| {
            if let Some(sender) = tx.lock().unwrap().take() {
                let _ = sender.send(url.to_string());
            }
        })
    };

    // Wait for both authorize URLs (with timeout), combine them into a single browser open:
    // the platform-auth URL with the base64-encoded state URL as a query param.
    let url_timeout = tokio::time::Duration::from_secs(30);
    tokio::spawn(async move {
        let state_url = match tokio::time::timeout(url_timeout, state_url_rx).await {
            Ok(Ok(url)) => url,
            _ => {
                tracing::warn!("timed out waiting for dbt State authorize URL");
                return;
            }
        };
        let platform_url = match tokio::time::timeout(url_timeout, platform_url_rx).await {
            Ok(Ok(url)) => url,
            _ => {
                tracing::warn!("timed out waiting for dbt platform authorize URL");
                return;
            }
        };

        let encoded_state = URL_SAFE_NO_PAD.encode(state_url.as_bytes());
        let combined = match url::Url::parse(&platform_url) {
            Ok(mut u) => {
                u.query_pairs_mut()
                    .append_pair("dbt_state_oauth", &encoded_state);
                u.to_string()
            }
            Err(_) => format!("{platform_url}&dbt_state_oauth={encoded_state}"),
        };

        println!("Opening your browser to complete login...");
        println!("{}", console::style(&combined).bold());
        if let Err(_err) = open::that_detached(&combined) {
            println!(
                "Cannot open browser. Please paste the URL above into your browser to authorize \
                the dbt CLI."
            );
        }
        println!();
        println!(
            "If you need to reset your password, complete the reset, then re-run {} to finish \
            authenticating.",
            console::style("dbt login").bold()
        );
        println!("\nWaiting for authentication.");
    });

    let (state_abort_tx, state_abort_rx) = tokio::sync::oneshot::channel::<()>();
    let (platform_abort_tx, platform_abort_rx) = tokio::sync::oneshot::channel::<()>();

    let state_flow = BrowserFlow {
        http: reqwest::Client::new(),
        auth_url: DEFAULT_OAUTH_AUTH_URL.to_string(),
        token_url: DEFAULT_OAUTH_TOKEN_URL.to_string(),
        client_id: DEFAULT_OAUTH_CLIENT_ID.to_string(),
        scope: ORGS_SCOPE.to_string(),
        timeout: INTERACTIVE_TIMEOUT,
        redirect_port: LOOPBACK_PORT,
        opener: state_opener,
        abort_signal: Mutex::new(Some(state_abort_rx)),
        // `dbt login` is an explicitly interactive command; always attempt it.
        available: true,
    };

    let env_scopes = std::env::var("DBT_OAUTH_SCOPES").ok();
    let requested_scopes = interactive_login_scopes(OAUTH_SCOPES, env_scopes.as_deref());

    let source_app = std::env::var("DBT_OAUTH_SOURCE_APP").unwrap_or_else(|_| "core_v2".to_owned());

    let platform_resolver = OAuthInteractiveResolver::builder(OAUTH_CLIENT_ID)
        .scopes(requested_scopes)
        .source_application(source_app)
        .opener(platform_opener)
        .abort_signal(platform_abort_rx)
        .build();

    let state_result = async {
        match state_flow.run().await {
            Ok(r) => Some(Ok(r)),
            Err(RunCacheServiceError::Aborted | RunCacheServiceError::Timeout(_)) => None,
            Err(e) => Some(Err(e)),
        }
    };

    tokio::select! {
        _ = async {
            loop {
                if token.is_cancelled() { break; }
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
        } => {
            let _ = state_abort_tx.send(());
            let _ = platform_abort_tx.send(());
            return Ok(());
        }
        Some(result) = state_result => {
            let response = result.map_err(|e| fs_err!(ErrorCode::AuthFailed, "{e}"))?;
            let stored = StoredToken::from_token_response(response)
                .map_err(|e| fs_err!(ErrorCode::AuthFailed, "{e}"))?;
            let store = TokenStore::discover().ok_or_else(|| {
                fs_err!(
                    ErrorCode::AuthFailed,
                    "could not resolve home directory for dbt State auth"
                )
            })?;
            store
                .save(&stored)
                .await
                .map_err(|e| fs_err!(ErrorCode::AuthFailed, "{e}"))?;
            run_state_guidance_after_state_login()?;
            // The org_id is resolved from the token scope only when it's unambiguous;
            // for multi-org tokens disambiguation is deferred to run time (via the
            // `state-org-id` config), so login still succeeds without selecting an org here.
            let org = Scope::from_string(&stored.scope)
                .ok()
                .and_then(|scope| determine_org_id(&scope, None).ok());
            match org {
                Some(org_id) => println!("dbt State login successful (org: {org_id})."),
                None => println!("dbt State login successful."),
            }
            // State login has no platform JWT; identity fields will be absent.
            login_event(invocation_id, true, LoginType::State, None, None, None);
        }
        result = platform_resolver.resolve() => {
            let cred = match result {
                Ok(c) => c,
                Err(AuthError::Aborted) => return Ok(()),
                Err(e) => {
                    eprintln!(
                        "Authentication failed. Re-run {} to try again.\n\n{e}",
                        console::style("dbt login").bold()
                    );
                    let cached_token = read_cached_access_token();
                    let (error_code, error_message) = login_error_fields(&e);
                    login_event(
                        invocation_id,
                        false,
                        LoginType::Unspecified,
                        cached_token.as_deref(),
                        Some(error_code),
                        error_message,
                    );
                    return Err(fs_err!(ErrorCode::AuthFailed, "authentication failed"));
                }
            };

            // Fire-off post-login hooks in parallel to the rest of the flow here.
            let post_login_fut = hooks.did_login();

            let http = reqwest::Client::new();
            run_state_guidance(&cred, &http).await?;

            // TODO: don't ignore errors here
            let _ = post_login_fut.await;

            println!("Congratulations! You are now signed in.");

            let access_token = if let Credential::OAuth(ref s) = cred {
                Some(s.access_token.as_str())
            } else {
                None
            };
            login_event(
                invocation_id,
                true,
                LoginType::Platform,
                access_token,
                None,
                None,
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use dbt_platform_auth::resolver::OAUTH_SCOPES;

    use super::interactive_login_scopes;

    #[test]
    fn test_no_env_scopes_returns_defaults() {
        assert_eq!(interactive_login_scopes(OAUTH_SCOPES, None), OAUTH_SCOPES);
    }

    #[test]
    fn test_empty_env_scopes_returns_defaults() {
        assert_eq!(
            interactive_login_scopes(OAUTH_SCOPES, Some("")),
            OAUTH_SCOPES
        );
    }

    #[test]
    fn test_extra_scopes_are_unioned_and_deduped() {
        let result =
            interactive_login_scopes(OAUTH_SCOPES, Some("jobs:run catalog:read account:read"));
        let result_scopes: Vec<&str> = result.split_whitespace().collect();
        // All default scopes must be present
        for s in OAUTH_SCOPES.split_whitespace() {
            assert!(
                result_scopes.contains(&s),
                "missing default scope {s} in {result}"
            );
        }
        // Extra scopes must be present
        assert!(result_scopes.contains(&"jobs:run"));
        assert!(result_scopes.contains(&"catalog:read"));
        // account:read appears exactly once (deduplication)
        assert_eq!(
            result_scopes
                .iter()
                .filter(|&&s| s == "account:read")
                .count(),
            1
        );
    }
}
