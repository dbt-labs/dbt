use crate::{AuthError, Credential};

/// Resolves credentials from environment variables.
///
/// Reads:
/// - `DBT_CLOUD_ACCOUNT_HOST`
/// - `DBT_CLOUD_TOKEN`
/// - `DBT_CLOUD_ACCOUNT_ID`
///
/// All three must be present and non-empty. The token prefix determines the credential
/// type: `dbtu_` → `Pat`, anything else → `ServiceToken`.
pub struct EnvVarResolver;

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl EnvVarResolver {
    pub(crate) async fn resolve(&self) -> Result<Credential, AuthError> {
        self.resolve_with_env(non_empty_env)
    }

    fn resolve_with_env(
        &self,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Credential, AuthError> {
        let host = env("DBT_CLOUD_ACCOUNT_HOST");
        let token = env("DBT_CLOUD_TOKEN");
        let account_id = env("DBT_CLOUD_ACCOUNT_ID");

        match (host, token, account_id) {
            (Some(host), Some(token), Some(id)) => {
                let account_id = id.parse::<u64>().map_err(|_| {
                    AuthError::Malformed(format!(
                        "DBT_CLOUD_ACCOUNT_ID {id:?} is not a valid integer"
                    ))
                })?;
                Ok(Credential::from_token(token, host, account_id))
            }
            _ => Err(AuthError::NotAuthenticated),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn run(vars: &[(&str, &str)]) -> Result<Credential, AuthError> {
        let map: HashMap<&str, &str> = vars.iter().copied().collect();
        EnvVarResolver.resolve_with_env(|name| map.get(name).map(|v| (*v).to_string()))
    }

    #[test]
    fn happy_path_service_token() {
        let cred = run(&[
            ("DBT_CLOUD_ACCOUNT_HOST", "ab123.us1.dbt.com"),
            ("DBT_CLOUD_TOKEN", "dbtc_abc123"),
            ("DBT_CLOUD_ACCOUNT_ID", "42"),
        ])
        .unwrap();
        assert!(matches!(cred, Credential::ServiceToken { .. }));
        assert_eq!(cred.token(), "dbtc_abc123");
        assert_eq!(cred.account_host(), "ab123.us1.dbt.com");
        assert_eq!(cred.account_id(), 42);
    }

    #[test]
    fn happy_path_pat() {
        let cred = run(&[
            ("DBT_CLOUD_ACCOUNT_HOST", "ab123.us1.dbt.com"),
            ("DBT_CLOUD_TOKEN", "dbtu_user_token"),
            ("DBT_CLOUD_ACCOUNT_ID", "7"),
        ])
        .unwrap();
        assert!(matches!(cred, Credential::Pat { .. }));
        assert_eq!(cred.token(), "dbtu_user_token");
    }

    #[test]
    fn missing_env_vars_returns_not_authenticated() {
        let err = run(&[]).unwrap_err();
        assert!(matches!(err, AuthError::NotAuthenticated));
    }

    #[test]
    fn non_numeric_account_id_returns_malformed() {
        let err = run(&[
            ("DBT_CLOUD_ACCOUNT_HOST", "ab123.us1.dbt.com"),
            ("DBT_CLOUD_TOKEN", "dbtc_abc"),
            ("DBT_CLOUD_ACCOUNT_ID", "not-a-number"),
        ])
        .unwrap_err();
        assert!(matches!(err, AuthError::Malformed(_)));
    }

    /// Verifies the resolver reads the actual process environment end-to-end.
    #[test]
    fn real_env_integration() {
        let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
        unsafe {
            #[allow(clippy::disallowed_methods)]
            std::env::set_var("DBT_CLOUD_ACCOUNT_HOST", "ab123.us1.dbt.com");
            #[allow(clippy::disallowed_methods)]
            std::env::set_var("DBT_CLOUD_TOKEN", "dbtc_real");
            #[allow(clippy::disallowed_methods)]
            std::env::set_var("DBT_CLOUD_ACCOUNT_ID", "99");
        }
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(EnvVarResolver.resolve());
        unsafe {
            #[allow(clippy::disallowed_methods)]
            std::env::remove_var("DBT_CLOUD_ACCOUNT_HOST");
            #[allow(clippy::disallowed_methods)]
            std::env::remove_var("DBT_CLOUD_TOKEN");
            #[allow(clippy::disallowed_methods)]
            std::env::remove_var("DBT_CLOUD_ACCOUNT_ID");
        }
        let cred = result.unwrap();
        assert_eq!(cred.token(), "dbtc_real");
        assert_eq!(cred.account_id(), 99);
    }
}
