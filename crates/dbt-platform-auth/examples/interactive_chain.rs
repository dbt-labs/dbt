// Demonstrates building an interactive AuthChain via AuthChainBuilder.
//
// Resolution order:
//   1. EnvVarResolver       — env vars
//   2. OAuthPassiveResolver — ~/.dbt/oauth_sessions.json (cached session or refresh)
//   3. CloudYamlResolver    — dbt_cloud.yml
//   4. OAuthInteractiveResolver — browser-based login
use dbt_platform_auth::{AuthChainBuilder, AuthError, Credential, OAUTH_CLIENT_ID};

#[tokio::main]
async fn main() {
    match AuthChainBuilder::new(OAUTH_CLIENT_ID)
        .source_application("core_v2")
        .interactive()
        .build()
        .resolve()
        .await
    {
        Ok(cred) => {
            let kind = match &cred {
                Credential::ServiceToken { .. } => "ServiceToken",
                Credential::Pat { .. } => "Pat",
                Credential::OAuth(_) => "OAuth",
            };
            println!("resolved credential");
            println!("  type:         {kind}");
            println!("  account_host: {}", cred.account_host());
            println!("  account_id:   {}", cred.account_id());
            println!(
                "  token:        {}...",
                &cred.token()[..8.min(cred.token().len())]
            );
        }
        Err(AuthError::NotAuthenticated) => {
            eprintln!("login failed: could not obtain credentials from any source");
            std::process::exit(1);
        }
        Err(AuthError::AuthenticationExpired) => {
            eprintln!("credentials have expired — run `dbt login` to re-authenticate");
            std::process::exit(1);
        }
        Err(AuthError::InaccessibleSource(e)) => {
            eprintln!("could not read credential source: {e}");
            std::process::exit(1);
        }
        Err(AuthError::Malformed(msg)) => {
            eprintln!("credential source is invalid: {msg}");
            std::process::exit(1);
        }
        Err(AuthError::Interactive(msg)) => {
            eprintln!("interactive auth failed: {msg}");
            std::process::exit(1);
        }
        Err(AuthError::Aborted) => {
            eprintln!("authentication was cancelled");
            std::process::exit(1);
        }
        Err(AuthError::InadequateScopes { requested, cached }) => {
            eprintln!(
                "cached session missing required scopes (have {cached:?}, need {requested:?})"
            );
            std::process::exit(1);
        }
        Err(AuthError::RefreshFailed(msg)) => {
            eprintln!("token refresh failed: {msg}");
            std::process::exit(1);
        }
    }
}
