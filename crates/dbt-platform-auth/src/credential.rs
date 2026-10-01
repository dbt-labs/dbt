use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// An active OAuth session for a dbt platform user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthSession {
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub scopes: Vec<String>,
    /// RSA-signed JWT retained for claims inspection; omitted in most cases.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    #[serde(with = "serde_unix_secs")]
    pub expires_at: SystemTime,
    /// Cell-scoped host, e.g. `"ab123.us1.dbt.com"`.
    pub account_host: String,
    pub account_id: u64,
    pub user_id: u64,
    /// OAuth client ID used to obtain this token.
    pub client_id: String,
}

mod serde_unix_secs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        t.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(UNIX_EPOCH + Duration::from_secs(secs))
    }
}

/// A credential that can authenticate requests to dbt platform.
#[derive(Debug, Clone)]
pub enum Credential {
    ServiceToken {
        token: String,
        account_host: String,
        account_id: u64,
    },
    Pat {
        token: String,
        account_host: String,
        account_id: u64,
    },
    OAuth(OAuthSession),
}

impl Credential {
    /// Build a `Credential` from a raw token string, classifying by prefix:
    /// `dbtu_` → `Pat`, anything else → `ServiceToken`.
    pub(crate) fn from_token(token: String, account_host: String, account_id: u64) -> Self {
        if token.starts_with("dbtu_") {
            Credential::Pat {
                token,
                account_host,
                account_id,
            }
        } else {
            Credential::ServiceToken {
                token,
                account_host,
                account_id,
            }
        }
    }

    pub fn token(&self) -> &str {
        match self {
            Credential::ServiceToken { token, .. } => token,
            Credential::Pat { token, .. } => token,
            Credential::OAuth(s) => &s.access_token,
        }
    }

    pub fn account_host(&self) -> &str {
        match self {
            Credential::ServiceToken { account_host, .. } => account_host,
            Credential::Pat { account_host, .. } => account_host,
            Credential::OAuth(s) => &s.account_host,
        }
    }

    pub fn account_id(&self) -> u64 {
        match self {
            Credential::ServiceToken { account_id, .. } => *account_id,
            Credential::Pat { account_id, .. } => *account_id,
            Credential::OAuth(s) => s.account_id,
        }
    }
}
