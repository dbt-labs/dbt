use std::time::SystemTime;

use dbt_common::{FsError, FsResult};
use dbt_platform_auth::{AuthChainBuilder, AuthError, Credential, OAUTH_CLIENT_ID, ResolverKind};

pub async fn execute_login_status() -> FsResult<()> {
    match AuthChainBuilder::new(OAUTH_CLIENT_ID)
        .build()
        .resolve_with_source()
        .await
    {
        Ok((cred, source)) => {
            let via = match source {
                ResolverKind::EnvVar => "environment variables",
                ResolverKind::CloudYaml => "dbt_cloud.yml",
                ResolverKind::OAuthPassive | ResolverKind::OAuthInteractive => "OAuth",
            };
            println!("Status: authenticated (via {via})");

            match &cred {
                Credential::OAuth(session) => {
                    println!("  account host:  {}", session.account_host);
                    println!("  account ID:    {}", session.account_id);
                    println!("  user ID:       {}", session.user_id);
                    let expiry = format_expiry(session.expires_at);
                    println!("  expires:       {expiry}");
                }
                Credential::ServiceToken {
                    account_host,
                    account_id,
                    ..
                } => {
                    println!("  type:          service token");
                    println!("  account host:  {account_host}");
                    println!("  account ID:    {account_id}");
                }
                Credential::Pat {
                    account_host,
                    account_id,
                    ..
                } => {
                    println!("  type:          personal access token");
                    println!("  account host:  {account_host}");
                    println!("  account ID:    {account_id}");
                }
            }
            Ok(())
        }
        Err(AuthError::NotAuthenticated) => {
            println!("Status: unauthenticated");
            println!("  sources checked (in order):");
            println!(
                "    1. env vars:       DBT_CLOUD_ACCOUNT_HOST, DBT_CLOUD_TOKEN, DBT_CLOUD_ACCOUNT_ID"
            );
            println!(
                "    2. OAuth session:  ~/.dbt/oauth_sessions.json  (run `dbt login` to create one)"
            );
            println!("    3. dbt_cloud.yml:  ./dbt_cloud.yml or ~/.dbt/dbt_cloud.yml");
            Err(FsError::exit_with_status(1))
        }
        Err(AuthError::AuthenticationExpired) => {
            println!("Status: unauthenticated (credentials expired)");
            println!("  run `dbt login` to re-authenticate");
            Err(FsError::exit_with_status(1))
        }
        Err(AuthError::InaccessibleSource(e)) => {
            println!("Status: unauthenticated (could not read credential source: {e})");
            Err(FsError::exit_with_status(1))
        }
        Err(AuthError::Malformed(msg)) => {
            println!("Status: unauthenticated (credential source is invalid: {msg})");
            Err(FsError::exit_with_status(1))
        }
        Err(e) => {
            println!("Status: unauthenticated ({e})");
            Err(FsError::exit_with_status(1))
        }
    }
}

fn format_expiry(expires_at: SystemTime) -> String {
    let now = SystemTime::now();
    match expires_at.duration_since(now) {
        Ok(remaining) => {
            let total_secs = remaining.as_secs();
            if total_secs == 0 {
                return "expired — run `dbt login` to re-authenticate".to_string();
            }
            let hours = total_secs / 3600;
            let mins = (total_secs % 3600) / 60;
            if hours > 0 {
                format!("in {hours}h {mins}m")
            } else {
                format!("in {mins}m")
            }
        }
        Err(_) => "expired — run `dbt login` to re-authenticate".to_string(),
    }
}
