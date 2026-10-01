// Demonstrates the interactive OAuth login flow that `dbt login` performs.
//
// Builds an OAuthInteractiveResolver directly, wires up a Ctrl-C abort handle
// so the user can cancel mid-flow, then calls resolve(). On success the session
// is automatically written to ~/.dbt/oauth_sessions.json and the credential is
// returned. Subsequent runs can obtain the same credential without a browser
// via OAuthPassiveResolver (or the default AuthChain).
//
// Run with:
//   cargo run --example dbt_login -p dbt-platform-auth
use dbt_platform_auth::resolver::OAuthInteractiveResolver;
use dbt_platform_auth::{AuthError, OAUTH_CLIENT_ID, OAuthAbortHandle};

#[tokio::main]
async fn main() {
    // Wire up a Ctrl-C handler that aborts the in-progress browser flow.
    let (abort_tx, abort_rx) = tokio::sync::oneshot::channel::<()>();
    let abort_handle = OAuthAbortHandle::new(abort_tx);

    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to listen for Ctrl-C");
        abort_handle.abort();
    });

    // Build the resolver. All settings have sensible defaults; override only
    // what you need (e.g. register_url for staging, redirect_port if 29527 is
    // taken).
    let resolver = OAuthInteractiveResolver::builder(OAUTH_CLIENT_ID)
        .abort_signal(abort_rx)
        // .register_url("https://us2.staging.dbt.com/register") // staging
        // .redirect_port(29528)                                  // custom port
        .build();

    match resolver.resolve().await {
        Ok(cred) => {
            // Session has been written to ~/.dbt/oauth_sessions.json.
            println!("Login successful.");
            println!("  account_host: {}", cred.account_host());
            println!("  account_id:   {}", cred.account_id());
            println!(
                "  token:        {}...",
                &cred.token()[..8.min(cred.token().len())]
            );
        }
        Err(AuthError::Aborted) => {
            eprintln!("Login cancelled.");
            std::process::exit(130);
        }
        Err(AuthError::Interactive(msg)) => {
            eprintln!("Login failed: {msg}");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Unexpected error: {e}");
            std::process::exit(1);
        }
    }
}
