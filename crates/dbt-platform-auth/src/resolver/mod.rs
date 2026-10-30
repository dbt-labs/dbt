mod cloud_yaml;
mod env_var;
mod oauth;

pub use cloud_yaml::CloudYamlResolver;
pub use env_var::EnvVarResolver;
pub use oauth::{
    INTERACTIVE_TIMEOUT, LOGIN_ACCOUNT_ID_ENV, LOGIN_HOST_ENV, OAUTH_SCOPES, OAuthAbortHandle,
    OAuthInteractiveResolver, OAuthInteractiveResolverBuilder, OAuthPassiveResolver, Opener,
};

use crate::{AuthError, Credential};

/// Discriminant for filtering resolvers in an [`crate::AuthChain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResolverKind {
    EnvVar,
    CloudYaml,
    OAuthPassive,
    OAuthInteractive,
}

/// A single step in the credential resolution chain.
pub enum AuthResolver {
    EnvVar(EnvVarResolver),
    CloudYaml(CloudYamlResolver),
    OAuthPassive(OAuthPassiveResolver),
    OAuthInteractive(OAuthInteractiveResolver),
}

impl AuthResolver {
    pub(crate) fn kind(&self) -> ResolverKind {
        match self {
            AuthResolver::EnvVar(_) => ResolverKind::EnvVar,
            AuthResolver::CloudYaml(_) => ResolverKind::CloudYaml,
            AuthResolver::OAuthPassive(_) => ResolverKind::OAuthPassive,
            AuthResolver::OAuthInteractive(_) => ResolverKind::OAuthInteractive,
        }
    }

    pub(crate) async fn resolve(&self) -> Result<Credential, AuthError> {
        match self {
            AuthResolver::EnvVar(r) => r.resolve().await,
            AuthResolver::CloudYaml(r) => r.resolve().await,
            AuthResolver::OAuthPassive(r) => r.resolve().await,
            AuthResolver::OAuthInteractive(r) => r.resolve().await,
        }
    }
}
