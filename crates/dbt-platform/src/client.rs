use std::future::Future;
use std::sync::Arc;

use dbt_cloud_api::apis::configuration::Configuration;
use dbt_platform_auth::{Credential, ResolverKind};

use crate::auth::{PlatformAuth, ResolverConfig, SOURCE_APPLICATION};
use crate::error::Result;

/// Default `User-Agent` sent with every request.
fn default_user_agent() -> String {
    format!("{}/{}", SOURCE_APPLICATION, env!("CARGO_PKG_VERSION"))
}

/// An authenticated handle to a single dbt platform account.
///
/// A client is scoped to one account, but several clients can share the same
/// underlying authentication: [`PlatformClient::for_account`] derives a sibling
/// scoped to another account the caller is authenticated for, reusing the shared
/// credential cache and HTTP client.
///
/// Construct one with [`PlatformClient::connect`] (default credential chain),
/// [`PlatformClient::builder`] for finer control, or
/// [`PlatformClient::from_credential`] when the caller already holds a
/// [`Credential`]. The credential is resolved lazily on each request and
/// refreshed when an OAuth token is near expiry, so a long-lived client keeps
/// working across token lifetimes.
#[derive(Clone)]
pub struct PlatformClient {
    auth: Arc<PlatformAuth>,
    account_id: u64,
    account_host: String,
    http: reqwest::Client,
    user_agent: String,
}

impl PlatformClient {
    /// Resolve credentials using the default chain and connect to the account
    /// they identify.
    pub async fn connect() -> Result<Self> {
        Self::builder().connect().await
    }

    /// Start configuring a client before connecting.
    pub fn builder() -> PlatformClientBuilder {
        PlatformClientBuilder::default()
    }

    /// Build a client from an already-resolved credential, bypassing the
    /// resolver chain. The credential cannot be refreshed, so an expiring OAuth
    /// token will eventually stop working; prefer [`connect`](Self::connect) when
    /// long-lived access is needed. Uses the default `User-Agent`.
    pub fn from_credential(credential: Credential) -> Self {
        let account_id = credential.account_id();
        let account_host = credential.account_host().to_owned();
        Self::new(
            Arc::new(PlatformAuth::static_credential(credential)),
            account_id,
            account_host,
            default_user_agent(),
        )
    }

    fn new(
        auth: Arc<PlatformAuth>,
        account_id: u64,
        account_host: String,
        user_agent: String,
    ) -> Self {
        Self {
            auth,
            account_id,
            account_host,
            http: reqwest::Client::new(),
            user_agent,
        }
    }

    /// Derive a sibling client scoped to another account the caller is
    /// authenticated for, sharing this client's credential cache and HTTP client.
    ///
    /// Returns [`PlatformError::AccountUnavailable`] if no cached credential
    /// authenticates the caller for `account_id`.
    ///
    /// [`PlatformError::AccountUnavailable`]: crate::PlatformError::AccountUnavailable
    pub async fn for_account(&self, account_id: u64) -> Result<Self> {
        let credential = self.auth.credential(account_id, false).await?;
        Ok(Self {
            auth: Arc::clone(&self.auth),
            account_id,
            account_host: credential.account_host().to_owned(),
            http: self.http.clone(),
            user_agent: self.user_agent.clone(),
        })
    }

    /// The dbt platform account id this client is scoped to.
    pub fn account_id(&self) -> u64 {
        self.account_id
    }

    /// The account id in the width the dbt Cloud Admin API expects. Centralizes
    /// the `u64` (domain) to `i64` (transport) conversion.
    pub(crate) fn account_id_i64(&self) -> i64 {
        self.account_id as i64
    }

    /// The cell-scoped host, e.g. `"ab123.us1.dbt.com"`.
    pub fn account_host(&self) -> &str {
        &self.account_host
    }

    /// The shared HTTP client.
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// The `User-Agent` this client sends.
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    /// URL of the internal Discovery (metadata) GraphQL endpoint on the account
    /// host. This shares the account host, so it works with any resolved
    /// credential — including OAuth sessions.
    pub(crate) fn discovery_graphql_url(&self) -> String {
        format!(
            "https://{}/api/private/discovery/internal/graphql",
            self.account_host
        )
    }

    /// A currently-valid bearer token for this client's account, refreshing if
    /// needed. `force_refresh` bypasses the cache to recover from a `401`.
    pub(crate) async fn token(&self, force_refresh: bool) -> Result<String> {
        let credential = self.auth.credential(self.account_id, force_refresh).await?;
        Ok(credential.token().to_owned())
    }

    /// Build a fresh Admin API [`Configuration`] carrying a currently-valid
    /// bearer token and this client's account-scoped base path.
    async fn configuration(&self, force_refresh: bool) -> Result<Configuration> {
        Ok(Configuration {
            base_path: format!("https://{}", self.account_host),
            user_agent: Some(self.user_agent.clone()),
            client: self.http.clone(),
            basic_auth: None,
            oauth_access_token: None,
            bearer_access_token: Some(self.token(force_refresh).await?),
            api_key: None,
        })
    }

    /// Run an Admin API call with a freshly-built [`Configuration`], retrying once
    /// with a force-refreshed token if the first attempt fails with `401`/`403`.
    pub(crate) async fn with_config<F, Fut, T>(&self, call: F) -> Result<T>
    where
        F: Fn(Configuration) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let configuration = self.configuration(false).await?;
        match call(configuration).await {
            Err(e) if e.is_unauthorized() => {
                let configuration = self.configuration(true).await?;
                call(configuration).await
            }
            other => other,
        }
    }
}

/// Builder for [`PlatformClient`].
///
/// Mirrors the knobs on [`AuthChainBuilder`](dbt_platform_auth::AuthChainBuilder):
/// restrict which credential sources are tried, scope the initial OAuth session
/// lookup to a specific account, or opt into an interactive browser login as the
/// final fallback.
#[derive(Default)]
pub struct PlatformClientBuilder {
    account_id: Option<u64>,
    interactive: bool,
    allow: Option<Vec<ResolverKind>>,
    deny: Option<Vec<ResolverKind>>,
    user_agent: Option<String>,
}

impl PlatformClientBuilder {
    /// Scope the initial credential resolution to the given account id. The
    /// resulting client can still reach other authenticated accounts via
    /// [`PlatformClient::for_account`].
    pub fn account_id(mut self, account_id: u64) -> Self {
        self.account_id = Some(account_id);
        self
    }

    /// Enable browser-based login when no cached credentials are found.
    pub fn interactive(mut self) -> Self {
        self.interactive = true;
        self
    }

    /// Try only the given credential sources.
    pub fn allow_only(mut self, kinds: &[ResolverKind]) -> Self {
        self.allow = Some(kinds.to_vec());
        self
    }

    /// Skip the given credential sources.
    pub fn deny(mut self, kinds: &[ResolverKind]) -> Self {
        self.deny = Some(kinds.to_vec());
        self
    }

    /// Override the `User-Agent` sent with requests.
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    /// Resolve credentials and build the client.
    pub async fn connect(self) -> Result<PlatformClient> {
        let config = ResolverConfig {
            interactive: self.interactive,
            allow: self.allow,
            deny: self.deny,
        };
        let credential = PlatformAuth::resolve_initial(self.account_id, &config).await?;
        let account_id = credential.account_id();
        let account_host = credential.account_host().to_owned();
        let user_agent = self.user_agent.unwrap_or_else(default_user_agent);
        Ok(PlatformClient::new(
            Arc::new(PlatformAuth::chain(credential, config)),
            account_id,
            account_host,
            user_agent,
        ))
    }

    /// Build a client from an already-resolved credential. Only
    /// [`user_agent`](Self::user_agent) applies here; the resolver knobs
    /// (`account_id`, `interactive`, `allow_only`, `deny`) govern credential
    /// resolution, which this path bypasses. The credential cannot be refreshed.
    pub fn from_credential(self, credential: Credential) -> PlatformClient {
        let account_id = credential.account_id();
        let account_host = credential.account_host().to_owned();
        PlatformClient::new(
            Arc::new(PlatformAuth::static_credential(credential)),
            account_id,
            account_host,
            self.user_agent.unwrap_or_else(default_user_agent),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pat(account_host: &str, account_id: u64) -> Credential {
        Credential::Pat {
            token: "dbtu_secret".to_owned(),
            account_host: account_host.to_owned(),
            account_id,
        }
    }

    #[test]
    fn client_is_scoped_to_the_credentials_account() {
        let client = PlatformClient::from_credential(pat("ab123.us1.dbt.com", 42));
        assert_eq!(client.account_id(), 42);
        assert_eq!(client.account_id_i64(), 42);
        assert_eq!(client.account_host(), "ab123.us1.dbt.com");
    }

    #[test]
    fn discovery_url_is_on_the_account_host() {
        let client = PlatformClient::from_credential(pat("ab123.us1.dbt.com", 1));
        assert_eq!(
            client.discovery_graphql_url(),
            "https://ab123.us1.dbt.com/api/private/discovery/internal/graphql"
        );
    }

    #[test]
    fn default_user_agent_names_the_sdk() {
        let client = PlatformClient::from_credential(pat("host", 1));
        assert!(client.user_agent().starts_with(SOURCE_APPLICATION));
    }

    #[test]
    fn user_agent_override_is_honored() {
        let client = PlatformClient::builder()
            .user_agent("custom-agent/1.0")
            .from_credential(pat("host", 1));
        assert_eq!(client.user_agent(), "custom-agent/1.0");
    }

    #[dbt_runtime::test]
    async fn token_reads_through_to_the_static_credential() {
        let client = PlatformClient::from_credential(pat("host", 1));
        assert_eq!(client.token(false).await.unwrap(), "dbtu_secret");
    }
}
