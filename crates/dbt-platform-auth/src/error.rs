/// Errors that can occur during credential resolution.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("not authenticated: no credentials found")]
    NotAuthenticated,

    #[error("authentication expired")]
    AuthenticationExpired,

    /// The credential source (file, socket, etc.) could not be read.
    #[error("inaccessible source: {0}")]
    InaccessibleSource(#[source] std::io::Error),

    /// A credential source was readable but contained invalid data.
    #[error("malformed config: {0}")]
    Malformed(String),

    /// The interactive browser-based OAuth flow failed (port busy, timeout, token exchange error, etc.).
    #[error("interactive auth failed: {0}")]
    Interactive(String),

    /// The interactive flow was cancelled by the caller via [`crate::OAuthAbortHandle`].
    #[error("interactive auth aborted")]
    Aborted,

    /// A cached session was found but its granted scopes do not cover all requested scopes.
    #[error("inadequate scopes: cached session has {cached:?} but {requested:?} are required")]
    InadequateScopes {
        requested: Vec<String>,
        cached: Vec<String>,
    },

    /// The refresh token exchange request failed due to a network or server error.
    /// Distinct from [`AuthenticationExpired`], which is returned when the server rejects
    /// the token (4xx) — indicating the token was revoked and the user must re-authenticate.
    #[error("token refresh failed: {0}")]
    RefreshFailed(String),
}

impl AuthError {
    /// A short, consistent action the user can take to resolve this error. Centralizes
    /// the "run `dbt login`" messaging so every caller of [`crate::AuthChain::resolve`]
    /// gives the user the same guidance. `None` when the error isn't resolved by a login
    /// action (e.g. a malformed config file).
    pub fn login_hint(&self) -> Option<&'static str> {
        match self {
            AuthError::NotAuthenticated => Some("run `dbt login` to authenticate"),
            AuthError::AuthenticationExpired => Some("run `dbt login` to re-authenticate"),
            _ => None,
        }
    }
}
