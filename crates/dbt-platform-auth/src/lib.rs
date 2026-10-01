mod chain;
mod credential;
mod error;
pub mod resolver;
mod session_cache;

/// Serializes all tests that read or write process-global environment variables,
/// across every module in this crate, preventing false failures from parallel execution.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub use chain::{AuthChain, AuthChainBuilder, OAUTH_CLIENT_ID};
pub use credential::{Credential, OAuthSession};
pub use error::AuthError;
pub use resolver::{
    AuthResolver, OAuthAbortHandle, OAuthInteractiveResolverBuilder, Opener, ResolverKind,
};
pub use session_cache::OAuthSessionCache;
