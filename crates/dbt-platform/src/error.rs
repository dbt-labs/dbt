use dbt_cloud_api::apis::Error as CloudApiError;
use dbt_platform_auth::AuthError;

/// Errors surfaced by the dbt platform metadata SDK.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// Credential resolution failed. Inspect [`AuthError::login_hint`] for a
    /// user-facing next step.
    #[error("authentication failed: {0}")]
    Auth(#[from] AuthError),

    /// A dbt Cloud Admin API request returned an error status or an
    /// unexpected payload. `status` is present when the failure carried an HTTP
    /// status, letting callers branch (not-found vs. unauthorized vs. retry)
    /// without string-matching the message.
    #[error("dbt Cloud API request failed: {message}")]
    Api {
        status: Option<reqwest::StatusCode>,
        message: String,
    },

    /// A specific account was requested (e.g. via [`for_account`]) but no cached
    /// credential authenticates the caller for it. `resolved` is the account the
    /// credential chain returned instead, when it returned one at all.
    ///
    /// [`for_account`]: crate::PlatformClient::for_account
    #[error(
        "not authenticated for account {requested}{}",
        .resolved.map(|a| format!(" (resolved account {a} instead)")).unwrap_or_default()
    )]
    AccountUnavailable {
        requested: u64,
        resolved: Option<u64>,
    },

    /// The Discovery (metadata) GraphQL API returned one or more errors.
    #[error("Discovery API returned errors: {0}")]
    Discovery(String),

    /// The underlying HTTP request could not be completed.
    #[error("HTTP transport error: {0}")]
    Transport(#[from] reqwest::Error),

    /// A response body could not be (de)serialized.
    #[error("failed to (de)serialize payload: {0}")]
    Serde(#[from] serde_json::Error),
}

impl PlatformError {
    /// A short, user-facing action to resolve this error, when one applies.
    pub fn login_hint(&self) -> Option<&'static str> {
        match self {
            PlatformError::Auth(e) => e.login_hint(),
            _ => None,
        }
    }

    /// The HTTP status of an [`Api`](PlatformError::Api) failure, when known.
    pub fn status(&self) -> Option<reqwest::StatusCode> {
        match self {
            PlatformError::Api { status, .. } => *status,
            _ => None,
        }
    }

    /// Whether this is an Admin API failure with the given status.
    pub fn is_not_found(&self) -> bool {
        self.status() == Some(reqwest::StatusCode::NOT_FOUND)
    }

    /// Whether this is an Admin API authorization failure (401/403).
    pub fn is_unauthorized(&self) -> bool {
        matches!(
            self.status(),
            Some(reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN)
        )
    }
}

impl<T: std::fmt::Debug> From<CloudApiError<T>> for PlatformError {
    fn from(e: CloudApiError<T>) -> Self {
        match e {
            CloudApiError::ResponseError(content) => PlatformError::Api {
                status: Some(content.status),
                message: content.content,
            },
            other => PlatformError::Api {
                status: None,
                message: other.to_string(),
            },
        }
    }
}

/// Convenience alias for fallible SDK operations.
pub type Result<T> = std::result::Result<T, PlatformError>;
