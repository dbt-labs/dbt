use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use dbt_platform_auth::{AuthChainBuilder, AuthError, Credential, ResolverKind};

use crate::error::{PlatformError, Result};

/// Identifies this SDK to the dbt platform in the OAuth `_dbtsrc` parameter and
/// the HTTP `User-Agent` header.
pub(crate) const SOURCE_APPLICATION: &str = "dbt-platform-sdk";

/// How long before an OAuth token's stated expiry we treat it as already
/// expired, so a refresh happens ahead of the request rather than racing it.
const EXPIRY_SKEW: Duration = Duration::from_secs(60);

/// Resolver knobs captured from [`crate::PlatformClientBuilder`], reused every
/// time a credential is (re-)resolved for refresh.
#[derive(Clone, Default)]
pub(crate) struct ResolverConfig {
    pub interactive: bool,
    pub allow: Option<Vec<ResolverKind>>,
    pub deny: Option<Vec<ResolverKind>>,
}

/// How a [`PlatformAuth`] obtains and renews credentials.
enum Mode {
    /// Re-resolvable via the credential chain. Every (re-)resolution re-runs the
    /// chain scoped to the requested account and, if a single-account ambient
    /// source shadows it, retries with those sources denied — so a connection
    /// survives token refresh exactly the way it was first established.
    Chain { config: ResolverConfig },
    /// A caller-supplied credential with no resolver behind it — cannot refresh.
    Static,
}

/// Owns credential resolution and renewal for one or more accounts, shared by
/// every [`crate::PlatformClient`] derived from the same connection.
///
/// A credential is cached per account and reused until it is within
/// [`EXPIRY_SKEW`] of expiry (OAuth) — PATs and service tokens never expire, so
/// they are cached indefinitely. Re-resolving an expired OAuth credential drives
/// the auth module's refresh-token exchange transparently.
pub(crate) struct PlatformAuth {
    mode: Mode,
    cache: Mutex<HashMap<u64, Credential>>,
}

impl PlatformAuth {
    /// Build a refreshing authenticator seeded with the credential just resolved
    /// for the primary account.
    pub(crate) fn chain(primary: Credential, config: ResolverConfig) -> Self {
        let mut cache = HashMap::new();
        cache.insert(primary.account_id(), primary);
        Self {
            mode: Mode::Chain { config },
            cache: Mutex::new(cache),
        }
    }

    /// Resolve the primary credential when first connecting, optionally scoping
    /// to `account`. Used to seed [`chain`](Self::chain); unlike a follow-up
    /// [`credential`](Self::credential) lookup there is no cache or primary
    /// account yet, so every resolver source is eligible.
    pub(crate) async fn resolve_initial(
        account: Option<u64>,
        config: &ResolverConfig,
    ) -> Result<Credential> {
        match account {
            Some(account) => resolve_scoped(account, config).await,
            None => resolve_chain(None, config).await,
        }
    }

    /// Build a non-refreshing authenticator around a caller-supplied credential.
    pub(crate) fn static_credential(credential: Credential) -> Self {
        let mut cache = HashMap::new();
        cache.insert(credential.account_id(), credential);
        Self {
            mode: Mode::Static,
            cache: Mutex::new(cache),
        }
    }

    /// Return a currently-valid credential for `account`, refreshing or
    /// re-resolving if the cached one is missing or near expiry. Set
    /// `force_refresh` to bypass the cache (used to recover from a `401`).
    pub(crate) async fn credential(&self, account: u64, force_refresh: bool) -> Result<Credential> {
        if !force_refresh {
            if let Some(cred) = self.cached_valid(account) {
                return Ok(cred);
            }
        }
        let cred = self.resolve(account).await?;
        self.cache.lock().unwrap().insert(account, cred.clone());
        Ok(cred)
    }

    /// The cached credential for `account`, if present and not near expiry.
    fn cached_valid(&self, account: u64) -> Option<Credential> {
        let cache = self.cache.lock().unwrap();
        cache.get(&account).filter(|c| is_valid(c)).cloned()
    }

    /// Resolve a fresh credential for `account` through the credential chain,
    /// applying the same shadowing retry as the initial connect so a refresh
    /// cannot fall back to an ambient source for the wrong account.
    async fn resolve(&self, account: u64) -> Result<Credential> {
        match &self.mode {
            Mode::Chain { config } => resolve_scoped(account, config).await,
            // No chain to re-run: the only credential we have is whatever was
            // supplied up front, already cached. If it is gone or expired we
            // cannot recover it.
            Mode::Static => Err(PlatformError::AccountUnavailable {
                requested: account,
                resolved: None,
            }),
        }
    }
}

/// Resolve a credential for `account` through the chain under `config`. If a
/// single-account ambient source (env var, `dbt_cloud.yml`) shadows the request
/// by resolving a *different* account, retry with those sources denied so the
/// account-scoped OAuth session can win. Shared by the initial connect and every
/// refresh so a connection re-resolves exactly the way it was established, rather
/// than silently falling back to another account's ambient credential.
async fn resolve_scoped(account: u64, config: &ResolverConfig) -> Result<Credential> {
    let credential = resolve_chain(Some(account), config).await?;
    if credential.account_id() == account {
        return Ok(credential);
    }
    // The resolved credential is for a different account: a single-account
    // ambient source that does not honor the requested account shadowed an
    // account-scoped OAuth session later in the chain — e.g. `DBT_CLOUD_*` for
    // another account winning over the session the user selected. Retry with the
    // ambient sources excluded so the session for the requested account wins.
    let shadowed = credential.account_id();
    let scoped = config.clone().deny_ambient_sources();
    match resolve_chain(Some(account), &scoped).await {
        Ok(credential) if credential.account_id() == account => Ok(credential),
        Ok(credential) => Err(PlatformError::AccountUnavailable {
            requested: account,
            resolved: Some(credential.account_id()),
        }),
        // Only the shadowing ambient credential existed; there is no session for
        // the requested account. Report it as unavailable (naming the account we
        // did find) rather than a bare "not authenticated".
        Err(PlatformError::Auth(AuthError::NotAuthenticated)) => {
            Err(PlatformError::AccountUnavailable {
                requested: account,
                resolved: Some(shadowed),
            })
        }
        Err(e) => Err(e),
    }
}

/// Build the credential chain scoped to `account` (unscoped when `None`) under
/// `config` and run it to the first successful credential. Shared by the initial
/// connect and every refresh so they configure the chain identically.
async fn resolve_chain(account: Option<u64>, config: &ResolverConfig) -> Result<Credential> {
    let mut chain = AuthChainBuilder::default().source_application(SOURCE_APPLICATION);
    if let Some(account) = account {
        chain = chain.account_id(account.to_string());
    }
    if config.interactive {
        chain = chain.interactive();
    }
    if let Some(allow) = &config.allow {
        chain = chain.allow_only(allow);
    }
    if let Some(deny) = &config.deny {
        chain = chain.deny(deny);
    }
    Ok(chain.build().resolve().await?)
}

impl ResolverConfig {
    /// Drop the single-account ambient sources (env var, `dbt_cloud.yml`) from an
    /// allow/deny set, leaving only the account-aware OAuth resolvers.
    fn deny_ambient_sources(mut self) -> Self {
        let ambient = [ResolverKind::EnvVar, ResolverKind::CloudYaml];
        match &mut self.allow {
            Some(allow) => allow.retain(|k| !ambient.contains(k)),
            None => {
                let deny = self.deny.get_or_insert_with(Vec::new);
                for kind in ambient {
                    if !deny.contains(&kind) {
                        deny.push(kind);
                    }
                }
            }
        }
        self
    }
}

/// Whether a cached credential is still safe to use. OAuth tokens are checked
/// against their expiry (minus [`EXPIRY_SKEW`]); other credential kinds do not
/// expire.
fn is_valid(credential: &Credential) -> bool {
    match credential {
        Credential::OAuth(session) => session.expires_at > SystemTime::now() + EXPIRY_SKEW,
        Credential::Pat { .. } | Credential::ServiceToken { .. } => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_platform_auth::OAuthSession;

    fn pat() -> Credential {
        Credential::Pat {
            token: "dbtu_secret".to_owned(),
            account_host: "ab123.us1.dbt.com".to_owned(),
            account_id: 1,
        }
    }

    fn oauth(expires_at: SystemTime) -> Credential {
        Credential::OAuth(OAuthSession {
            access_token: "tok".to_owned(),
            refresh_token: None,
            id_token: None,
            scopes: vec![],
            expires_at,
            account_host: "ab123.us1.dbt.com".to_owned(),
            account_id: 1,
            user_id: 7,
            client_id: "client".to_owned(),
        })
    }

    #[test]
    fn pats_never_expire() {
        assert!(is_valid(&pat()));
    }

    #[test]
    fn oauth_within_skew_of_expiry_is_invalid() {
        // Expires in 30s — inside the 60s skew, so treat as already expired.
        assert!(!is_valid(&oauth(
            SystemTime::now() + Duration::from_secs(30)
        )));
    }

    #[test]
    fn oauth_well_before_expiry_is_valid() {
        assert!(is_valid(&oauth(
            SystemTime::now() + Duration::from_secs(3600)
        )));
    }

    #[test]
    fn static_auth_caches_the_supplied_credential() {
        let auth = PlatformAuth::static_credential(pat());
        assert!(auth.cached_valid(1).is_some());
        assert!(auth.cached_valid(2).is_none());
    }

    #[test]
    fn deny_ambient_sources_adds_env_and_yaml_to_empty_denylist() {
        let config = ResolverConfig::default().deny_ambient_sources();
        let deny = config.deny.unwrap();
        assert!(deny.contains(&ResolverKind::EnvVar));
        assert!(deny.contains(&ResolverKind::CloudYaml));
    }

    #[test]
    fn deny_ambient_sources_prunes_them_from_an_allowlist() {
        let config = ResolverConfig {
            allow: Some(vec![
                ResolverKind::EnvVar,
                ResolverKind::OAuthPassive,
                ResolverKind::CloudYaml,
            ]),
            ..Default::default()
        }
        .deny_ambient_sources();
        assert_eq!(config.allow.unwrap(), vec![ResolverKind::OAuthPassive]);
    }

    #[dbt_runtime::test]
    async fn static_auth_cannot_resolve_an_uncached_account() {
        let auth = PlatformAuth::static_credential(pat());
        let err = auth.credential(999, false).await.unwrap_err();
        assert!(matches!(
            err,
            PlatformError::AccountUnavailable {
                requested: 999,
                resolved: None
            }
        ));
    }

    // ── Initial resolution against a real credential chain ──────────────────
    //
    // These exercise `resolve_initial`'s shadowing retry through the actual
    // `AuthChainBuilder`, driving it entirely with the `DBT_CLOUD_*` /
    // `DBT_OAUTH_CLIENT_ID` env vars the chain reads. They also redirect `$HOME`
    // so neither the OAuth session cache nor `dbt_cloud.yml` under a developer's
    // real home can leak a credential into the chain. All of that is
    // process-global, so they serialize on a shared lock and restore the
    // environment (and delete the temp home) on drop.
    //
    // The retry's happy path — an account-scoped OAuth session winning once the
    // shadowing ambient source is denied — is not re-tested here: it depends on
    // the passive OAuth resolver reading a session cache keyed off
    // `dirs::home_dir()` (which ignores `$HOME` on Windows), and that resolver's
    // account-scoped selection is already covered in `dbt-platform-auth`.

    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// A scoped, process-global environment: a fresh `$HOME` plus whichever
    /// `DBT_*` vars a test sets, all reverted when the guard drops.
    struct EnvSandbox {
        _lock: std::sync::MutexGuard<'static, ()>,
        home: std::path::PathBuf,
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvSandbox {
        const VARS: [&'static str; 5] = [
            "HOME",
            "DBT_OAUTH_CLIENT_ID",
            "DBT_CLOUD_ACCOUNT_HOST",
            "DBT_CLOUD_TOKEN",
            "DBT_CLOUD_ACCOUNT_ID",
        ];

        // `set_var` is disallowed workspace-wide to catch stray `GOLDIE_UPDATE`
        // usages; scoped, lock-guarded test env manipulation is the sanctioned
        // exemption (see clippy.toml).
        #[allow(clippy::disallowed_methods)]
        fn new() -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let unique = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let home = std::env::temp_dir().join(format!(
                "dbt-platform-resolve-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(home.join(".dbt")).unwrap();
            let saved = Self::VARS
                .iter()
                .map(|k| (*k, std::env::var_os(k)))
                .collect();
            // SAFETY: `ENV_LOCK` serializes every sandbox and nothing else in the
            // test touches these vars while one is held.
            unsafe { std::env::set_var("HOME", &home) };
            Self {
                _lock: lock,
                home,
                saved,
            }
        }

        #[allow(clippy::disallowed_methods)]
        fn set(&self, key: &str, value: &str) {
            // SAFETY: see `new` — the sandbox holds `ENV_LOCK` for its lifetime.
            unsafe { std::env::set_var(key, value) };
        }

        /// Seed an env-var credential for `account`, the ambient source that
        /// sorts ahead of the account-scoped OAuth resolver in the chain.
        fn set_env_credential(&self, account: u64) {
            self.set("DBT_CLOUD_ACCOUNT_HOST", "acme.us1.dbt.com");
            self.set("DBT_CLOUD_TOKEN", "dbtc_ambient");
            self.set("DBT_CLOUD_ACCOUNT_ID", &account.to_string());
        }
    }

    impl Drop for EnvSandbox {
        #[allow(clippy::disallowed_methods)]
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                // SAFETY: still under `ENV_LOCK`, restoring the pre-test values.
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    /// When the shadowing env-var credential is the only thing present, the
    /// requested account is genuinely unreachable — report it as unavailable
    /// (naming the account we did find) rather than authenticating as the wrong
    /// one or surfacing a bare "not authenticated".
    #[dbt_runtime::test]
    async fn resolve_initial_reports_unavailable_when_only_a_mismatched_env_var_exists() {
        let sandbox = EnvSandbox::new();
        sandbox.set("DBT_OAUTH_CLIENT_ID", "test-client");
        sandbox.set_env_credential(5);

        let err = PlatformAuth::resolve_initial(Some(999), &ResolverConfig::default())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            PlatformError::AccountUnavailable {
                requested: 999,
                resolved: Some(5)
            }
        ));
    }

    /// An env-var credential for the *requested* account is still honored — the
    /// fix only excludes ambient sources on the mismatch retry, not up front.
    #[dbt_runtime::test]
    async fn resolve_initial_still_accepts_an_env_var_for_the_requested_account() {
        let sandbox = EnvSandbox::new();
        sandbox.set("DBT_OAUTH_CLIENT_ID", "test-client");
        sandbox.set_env_credential(5);

        let credential = PlatformAuth::resolve_initial(Some(5), &ResolverConfig::default())
            .await
            .expect("an env var for the requested account is still valid");
        assert_eq!(credential.account_id(), 5);
    }
}
