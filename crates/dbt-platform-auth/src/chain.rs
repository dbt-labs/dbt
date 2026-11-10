use std::collections::HashSet;

use crate::{
    AuthError, Credential,
    resolver::{
        AuthResolver, CloudYamlResolver, EnvVarResolver, OAuthInteractiveResolver,
        OAuthPassiveResolver, ResolverKind,
    },
};

/// OAuth client ID registered with dbt platform.
/// See lsp/src/registration/webAuth/constants.ts
pub const OAUTH_CLIENT_ID: &str = "854ad54c885f03bbe6ca7eb1e75593fb";

/// Returns the effective OAuth client ID, preferring `DBT_OAUTH_CLIENT_ID` if set.
fn effective_client_id() -> String {
    std::env::var("DBT_OAUTH_CLIENT_ID").unwrap_or_else(|_| OAUTH_CLIENT_ID.to_owned())
}

/// An ordered chain of credential resolvers tried in sequence.
///
/// `resolve` walks the chain until a resolver returns credentials. `NotAuthenticated`
/// from a resolver is silently skipped (try next). Any other error is recorded and the
/// chain continues; if no credentials are found, the first non-`NotAuthenticated` error
/// is returned — otherwise `NotAuthenticated`.
///
/// Prefer [`AuthChainBuilder`] for constructing an `AuthChain`. Use [`AuthChain::new`]
/// only when you have a fully custom resolver list that should not be affected by
/// builder configuration.
pub struct AuthChain {
    pub(crate) resolvers: Vec<AuthResolver>,
}

impl AuthChain {
    /// Construct from an explicit resolver list. Use this when you need precise
    /// control over which resolvers are present (e.g. in tests). For normal use
    /// prefer [`AuthChainBuilder`].
    pub fn new(resolvers: Vec<AuthResolver>) -> Self {
        Self { resolvers }
    }

    pub async fn resolve(&self) -> Result<Credential, AuthError> {
        self.resolve_with_source().await.map(|(c, _)| c)
    }

    /// Like [`resolve`], but also returns which resolver produced the credential.
    pub async fn resolve_with_source(&self) -> Result<(Credential, ResolverKind), AuthError> {
        let mut first_error: Option<AuthError> = None;
        for resolver in &self.resolvers {
            match resolver.resolve().await {
                Ok(cred) => return Ok((cred, resolver.kind())),
                Err(AuthError::NotAuthenticated) => {}
                Err(e) => {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
        }
        Err(first_error.unwrap_or(AuthError::NotAuthenticated))
    }
}

/// Builds an [`AuthChain`] with the standard resolver set, optionally extended
/// with an interactive OAuth resolver.
///
/// **Default resolver order:** `EnvVar` → `OAuthPassive` → `CloudYaml`
///
/// Call `.interactive()` to append an [`OAuthInteractiveResolver`] as the final
/// fallback. Call `.source_application()` to set the `_dbtsrc` tracking parameter
/// sent with the authorization URL; required when using `.interactive()`.
pub struct AuthChainBuilder {
    client_id: String,
    source_application: Option<String>,
    include_interactive: bool,
    account_id: Option<String>,
    allow_kinds: Option<HashSet<ResolverKind>>,
    deny_kinds: Option<HashSet<ResolverKind>>,
}

impl Default for AuthChainBuilder {
    fn default() -> Self {
        Self::new(effective_client_id())
    }
}

impl AuthChainBuilder {
    /// Start with the default resolver chain using the given OAuth `client_id`.
    pub fn new(client_id: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            source_application: None,
            include_interactive: false,
            account_id: None,
            allow_kinds: None,
            deny_kinds: None,
        }
    }

    /// Set the `_dbtsrc` query parameter sent with the authorization URL to
    /// identify the application initiating the login flow.
    pub fn source_application(mut self, v: impl Into<String>) -> Self {
        self.source_application = Some(v.into());
        self
    }

    /// Restrict cached OAuth session lookup to the given dbt platform account.
    /// Without this, a session cache holding sessions for several accounts
    /// resolves whichever session comes first.
    pub fn account_id(mut self, account_id: impl Into<String>) -> Self {
        self.account_id = Some(account_id.into());
        self
    }

    /// Append an [`OAuthInteractiveResolver`] as the final fallback, enabling
    /// browser-based login when no cached credentials are found.
    pub fn interactive(mut self) -> Self {
        self.include_interactive = true;
        self
    }

    /// Retain only resolvers whose kind is in `kinds` (allowlist).
    pub fn allow_only(mut self, kinds: &[ResolverKind]) -> Self {
        self.allow_kinds = Some(kinds.iter().copied().collect());
        self
    }

    /// Remove resolvers whose kind is in `kinds` (denylist).
    pub fn deny(mut self, kinds: &[ResolverKind]) -> Self {
        self.deny_kinds = Some(kinds.iter().copied().collect());
        self
    }

    pub fn build(self) -> AuthChain {
        let client_id = std::env::var("DBT_OAUTH_CLIENT_ID").unwrap_or(self.client_id);
        let mut passive = OAuthPassiveResolver::new(&client_id);
        passive.account_id = self.account_id.clone();
        let mut resolvers = vec![
            AuthResolver::EnvVar(EnvVarResolver),
            AuthResolver::OAuthPassive(passive),
            AuthResolver::CloudYaml(CloudYamlResolver::default()),
        ];
        if self.include_interactive {
            let mut builder = OAuthInteractiveResolver::builder(&client_id);
            if let Some(src_app) = self.source_application {
                builder = builder.source_application(src_app);
            }
            resolvers.push(AuthResolver::OAuthInteractive(builder.build()));
        }
        if let Some(allowed) = &self.allow_kinds {
            resolvers.retain(|r| allowed.contains(&r.kind()));
        }
        if let Some(denied) = &self.deny_kinds {
            resolvers.retain(|r| !denied.contains(&r.kind()));
        }
        AuthChain { resolvers }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::CloudYamlResolver;
    use std::io::Write as _;
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    fn write_yaml(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    fn valid_yaml() -> &'static str {
        r#"
version: "1"
context:
  active-project: "proj-1"
  active-host: "ab123.us1.dbt.com"
projects:
  - project-name: "My Project"
    project-id: "proj-1"
    account-name: "acme"
    account-id: "42"
    account-host: "ab123.us1.dbt.com"
    token-name: "my-token"
    token-value: "dbtc_abc123"
"#
    }

    #[dbt_runtime::test]
    async fn chain_returns_first_successful_credential() {
        let f = write_yaml(valid_yaml());
        let chain = AuthChain::new(vec![AuthResolver::CloudYaml(CloudYamlResolver {
            dbt_project_path: None,
            path: Some(f.path().to_path_buf()),
        })]);

        let cred = chain.resolve().await.unwrap();
        assert_eq!(cred.token(), "dbtc_abc123");
        assert_eq!(cred.account_id(), 42);
    }

    #[dbt_runtime::test]
    async fn chain_returns_not_authenticated_when_all_fail() {
        let chain = AuthChain::new(vec![AuthResolver::CloudYaml(CloudYamlResolver {
            dbt_project_path: None,
            path: Some(PathBuf::from("/nonexistent/dbt_cloud.yml")),
        })]);

        let err = chain.resolve().await.unwrap_err();
        assert!(matches!(err, AuthError::NotAuthenticated));
    }

    #[dbt_runtime::test]
    async fn chain_continues_past_errors_and_returns_first_error_if_no_credentials() {
        let bad = write_yaml("not: valid: yaml: [[[");

        let chain = AuthChain::new(vec![
            AuthResolver::CloudYaml(CloudYamlResolver {
                dbt_project_path: None,
                path: Some(bad.path().to_path_buf()),
            }),
            AuthResolver::CloudYaml(CloudYamlResolver {
                dbt_project_path: None,
                path: Some(PathBuf::from("/nonexistent/dbt_cloud.yml")),
            }),
        ]);

        let err = chain.resolve().await.unwrap_err();
        assert!(matches!(err, AuthError::Malformed(_)));
    }

    #[dbt_runtime::test]
    async fn chain_continues_past_error_and_succeeds_on_next_resolver() {
        let bad = write_yaml("not: valid: yaml: [[[");
        let good = write_yaml(valid_yaml());

        let chain = AuthChain::new(vec![
            AuthResolver::CloudYaml(CloudYamlResolver {
                dbt_project_path: None,
                path: Some(bad.path().to_path_buf()),
            }),
            AuthResolver::CloudYaml(CloudYamlResolver {
                dbt_project_path: None,
                path: Some(good.path().to_path_buf()),
            }),
        ]);

        let cred = chain.resolve().await.unwrap();
        assert_eq!(cred.token(), "dbtc_abc123");
    }

    #[test]
    fn builder_default_produces_three_non_interactive_resolvers() {
        let chain = AuthChainBuilder::new("test-client").build();
        assert_eq!(chain.resolvers.len(), 3);
        assert!(matches!(chain.resolvers[0], AuthResolver::EnvVar(_)));
        assert!(matches!(chain.resolvers[1], AuthResolver::OAuthPassive(_)));
        assert!(matches!(chain.resolvers[2], AuthResolver::CloudYaml(_)));
    }

    #[test]
    fn builder_interactive_appends_interactive_resolver() {
        let chain = AuthChainBuilder::new("test-client")
            .source_application("test-app")
            .interactive()
            .build();
        assert_eq!(chain.resolvers.len(), 4);
        assert!(matches!(
            chain.resolvers[3],
            AuthResolver::OAuthInteractive(_)
        ));
    }

    #[test]
    fn builder_without_interactive_excludes_interactive_resolver() {
        let chain = AuthChainBuilder::new("test-client").build();
        assert!(
            !chain
                .resolvers
                .iter()
                .any(|r| matches!(r, AuthResolver::OAuthInteractive(_)))
        );
    }

    #[test]
    fn builder_account_id_is_applied_to_passive_resolver() {
        let chain = AuthChainBuilder::new("test-client").account_id("7").build();
        let passive = chain
            .resolvers
            .iter()
            .find_map(|r| match r {
                AuthResolver::OAuthPassive(p) => Some(p),
                _ => None,
            })
            .unwrap();
        assert_eq!(passive.account_id.as_deref(), Some("7"));
    }

    #[test]
    fn builder_without_account_id_leaves_passive_resolver_unrestricted() {
        let chain = AuthChainBuilder::new("test-client").build();
        let passive = chain
            .resolvers
            .iter()
            .find_map(|r| match r {
                AuthResolver::OAuthPassive(p) => Some(p),
                _ => None,
            })
            .unwrap();
        assert_eq!(passive.account_id, None);
    }

    #[test]
    fn builder_allow_only_filters_correctly() {
        let chain = AuthChainBuilder::new("test-client")
            .allow_only(&[ResolverKind::EnvVar])
            .build();
        assert_eq!(chain.resolvers.len(), 1);
        assert!(matches!(chain.resolvers[0], AuthResolver::EnvVar(_)));
    }

    #[test]
    fn builder_deny_filters_correctly() {
        let chain = AuthChainBuilder::new("test-client")
            .deny(&[ResolverKind::OAuthPassive])
            .build();
        assert_eq!(chain.resolvers.len(), 2);
        assert!(
            !chain
                .resolvers
                .iter()
                .any(|r| matches!(r, AuthResolver::OAuthPassive(_)))
        );
    }
}
