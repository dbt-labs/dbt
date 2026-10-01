use std::env;
use std::str::FromStr;
use url::Url;

use gcloud_auth::{project::Config, token::DefaultTokenSourceProvider};
use token_source::TokenSourceProvider;

/// Special mechanism for handling Redis auth.
///
/// For standard username/password use Standard and the URI
/// will be built based on vars that are set on RedisConfig.
///
/// GcpIam will retrieve an auth token via GCP APIs to use
/// as the Redis password.
#[derive(Clone, Debug, PartialEq)]
pub enum AuthMethod {
    Standard,
    GcpIam,
}

impl FromStr for AuthMethod {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "gcp_iam" => Ok(AuthMethod::GcpIam),
            _ => Ok(AuthMethod::Standard),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RedisConfig {
    pub host: String,
    pub port: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub use_tls: bool,
    pub tls_ca_data: Option<String>,
    pub key_prefix: String,
    pub tls_check_hostnames: bool,
    pub auth_method: AuthMethod,
}

impl RedisConfig {
    pub async fn uri(&self) -> Result<String, String> {
        let scheme = if self.use_tls { "rediss" } else { "redis" };
        let mut redis_uri = Url::parse(&format!("{}://{}:{}", scheme, self.host, self.port))
            .map_err(|e| format!("Failed to construct redis uri: {e}"))?;

        if let Some(user) = &self.username {
            redis_uri
                .set_username(user.as_str())
                .map_err(|e| format!("Failed to construct redis uri: {e:?}"))?;
        }
        match self.auth_method {
            AuthMethod::GcpIam => {
                let token = self.get_gcp_token().await?;
                redis_uri
                    .set_password(Some(&token))
                    .map_err(|e| format!("Failed to construct redis uri: {e:?}"))?;
            }
            AuthMethod::Standard => {
                redis_uri
                    .set_password(self.password.as_deref())
                    .map_err(|e| format!("Failed to construct redis uri: {e:?}"))?;
            }
        }
        Ok(redis_uri.to_string())
    }

    async fn get_gcp_token(&self) -> Result<String, String> {
        let cfg =
            Config::default().with_scopes(&["https://www.googleapis.com/auth/cloud-platform"]);
        let tsp = DefaultTokenSourceProvider::new(cfg)
            .await
            .map_err(|e| format!("Error creating GCP token source provider: {e}"))?;
        let token = tsp
            .token_source()
            .token()
            .await
            .map_err(|e| format!("Error retrieving GCP token: {e}"))?;
        Ok(token
            .strip_prefix("Bearer ")
            .unwrap_or(&token)
            .trim()
            .to_string())
    }

    pub fn from_env() -> Self {
        Self {
            host: env::var("_DBT_REDIS_HOST").unwrap_or_else(|_| "localhost".to_string()),
            port: env::var("_DBT_REDIS_PORT").unwrap_or_else(|_| "6379".to_string()),
            username: env::var("_DBT_REDIS_USERNAME").ok(),
            password: env::var("_DBT_REDIS_PASSWORD").ok(),
            use_tls: env::var("_DBT_REDIS_USE_TLS").is_ok_and(|v| v == "1"),
            tls_ca_data: env::var("_DBT_REDIS_TLS_CA_DATA").ok(),
            // used in dev to prevent key collisions in shared redis cluster
            key_prefix: env::var("_DBT_REDIS_KEY_PREFIX").unwrap_or_else(|_| String::new()),
            tls_check_hostnames: env::var("_DBT_REDIS_TLS_CHECK_HOSTNAMES").is_ok_and(|v| v == "1"),
            auth_method: env::var("_DBT_REDIS_AUTH_METHOD")
                .ok()
                .map(|v| AuthMethod::from_str(&v).unwrap_or(AuthMethod::Standard))
                .unwrap_or(AuthMethod::Standard),
        }
    }
}
