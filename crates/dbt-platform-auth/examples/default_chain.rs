use dbt_platform_auth::{AuthChainBuilder, AuthError, Credential, OAUTH_CLIENT_ID};

#[tokio::main]
async fn main() {
    match AuthChainBuilder::new(OAUTH_CLIENT_ID)
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
            eprintln!("not authenticated: no credentials found");
            eprintln!("sources checked (in order):");
            eprintln!(
                "  1. env vars:        DBT_CLOUD_ACCOUNT_HOST, DBT_CLOUD_TOKEN, DBT_CLOUD_ACCOUNT_ID"
            );
            eprintln!(
                "  2. OAuth session:   ~/.dbt/oauth_sessions.json  (run `dbt login` to create one)"
            );
            eprintln!("  3. dbt_cloud.yml:   ./dbt_cloud.yml or ~/.dbt/dbt_cloud.yml");
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
