use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use crate::auth::browser_flow::TokenResponse;
use crate::auth::scope::{Scope, jwt_claims};
use crate::service_client::RunCacheServiceError;

const AUTH_DIR_NAME: &str = ".dbt";
const AUTH_FILE_NAME: &str = "state_auth.json";
const HOME_ENV: &str = "DBT_ENGINE_STATE_HOME";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoredToken {
    pub scope: String,
    pub token_type: String,
    pub id_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// dbt platform account whose credential was exchanged for this token.
    /// `None` for tokens not minted from a platform credential (standalone
    /// browser login, client credentials) and for files written before this
    /// field existed; such tokens are never invalidated on account change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_account_id: Option<String>,
}

impl StoredToken {
    /// Build a `StoredToken` from a raw OAuth `TokenResponse`.
    ///
    /// Decodes the JWT claims to extract and validate the scope, then stores the
    /// raw token fields. The organization ID is intentionally not derived or
    /// persisted here — it is always resolved from the live token scope and the
    /// current configuration at request time (see `OAuthTokenSource::resolve_org_id`).
    pub fn from_token_response(response: TokenResponse) -> Result<Self, RunCacheServiceError> {
        let claims = jwt_claims(&response.id_token)?;
        let scope_str = claims.scope.ok_or_else(|| {
            RunCacheServiceError::Auth("OAuth token is missing scope".to_string())
        })?;
        // Validate the scope is well-formed; org resolution is deferred to the caller.
        Scope::from_string(&scope_str)?;

        let expires_at = expires_at_from(&response);

        Ok(Self {
            scope: scope_str,
            token_type: "Bearer".to_string(),
            id_token: response.id_token,
            expires_at,
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            platform_account_id: None,
        })
    }
}

fn expires_at_from(response: &TokenResponse) -> Option<f64> {
    if let Some(secs) = response.expires_at {
        return Some(secs);
    }
    response.expires_in.and_then(|secs| {
        if secs.is_finite() && secs > 0.0 {
            (SystemTime::now() + std::time::Duration::from_secs_f64(secs))
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs_f64())
        } else {
            None
        }
    })
}

#[derive(Debug, Clone)]
pub struct TokenStore {
    path: PathBuf,
}

impl TokenStore {
    /// Resolve the on-disk auth file path. Honors `DBT_ENGINE_STATE_HOME`, otherwise
    /// uses `dirs::home_dir()`. Returns `None` when neither resolves.
    pub fn discover() -> Option<Self> {
        Self::discover_from(std::env::var(HOME_ENV).ok(), dirs::home_dir())
    }

    pub fn discover_from(env_home: Option<String>, fallback_home: Option<PathBuf>) -> Option<Self> {
        // Treat an empty or whitespace-only DBT_ENGINE_STATE_HOME as unset to avoid
        // resolving the auth file to a relative path in the current working
        // directory.
        let env_home = env_home.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let path = if let Some(env) = env_home {
            // DBT_ENGINE_STATE_HOME is the auth directory directly (matches Python semantics).
            PathBuf::from(env).join(AUTH_FILE_NAME)
        } else {
            fallback_home?.join(AUTH_DIR_NAME).join(AUTH_FILE_NAME)
        };
        Some(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn load(&self) -> Result<Option<StoredToken>, RunCacheServiceError> {
        let contents = match fs::read_to_string(&self.path).await {
            Ok(contents) => contents,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(RunCacheServiceError::Auth(format!(
                    "failed to read {}: {err}",
                    self.path.display()
                )));
            }
        };

        match serde_json::from_str::<StoredToken>(&contents) {
            Ok(token) => Ok(Some(token)),
            Err(_) => {
                let _ = fs::remove_file(&self.path).await;
                Ok(None)
            }
        }
    }

    pub async fn save(&self, token: &StoredToken) -> Result<(), RunCacheServiceError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).await.map_err(|err| {
                RunCacheServiceError::Auth(format!("failed to create {}: {err}", parent.display()))
            })?;
        }

        let json = serde_json::to_string(token).map_err(|err| {
            RunCacheServiceError::Auth(format!("failed to serialize auth token: {err}"))
        })?;

        let write_err = |err| {
            RunCacheServiceError::Auth(format!("failed to write {}: {err}", self.path.display()))
        };

        let mut file = open_for_write(&self.path).await.map_err(&write_err)?;
        file.write_all(json.as_bytes()).await.map_err(&write_err)?;
        file.flush().await.map_err(&write_err)?;
        Ok(())
    }

    pub async fn delete(&self) -> Result<(), RunCacheServiceError> {
        match fs::remove_file(&self.path).await {
            Ok(_) => Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) => Err(RunCacheServiceError::Auth(format!(
                "failed to delete {}: {err}",
                self.path.display()
            ))),
        }
    }
}

#[cfg(unix)]
async fn open_for_write(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .await
}

#[cfg(not(unix))]
async fn open_for_write(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_token() -> StoredToken {
        StoredToken {
            scope: "runcache:scope:org:dev:admin".to_string(),
            token_type: "Bearer".to_string(),
            id_token: "fake.jwt.token".to_string(),
            expires_at: Some(1_700_000_000.0),
            access_token: Some("access".to_string()),
            refresh_token: Some("refresh".to_string()),
            platform_account_id: Some("42".to_string()),
        }
    }

    fn store_in(dir: &TempDir) -> TokenStore {
        let auth_home = dir.path().join(".dbt");
        TokenStore::discover_from(Some(auth_home.to_string_lossy().into_owned()), None).unwrap()
    }

    #[dbt_runtime::test]
    async fn load_returns_none_when_missing() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        assert_eq!(store.load().await.unwrap(), None);
    }

    #[dbt_runtime::test]
    async fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        store.save(&sample_token()).await.unwrap();
        assert_eq!(store.load().await.unwrap(), Some(sample_token()));
    }

    #[dbt_runtime::test]
    async fn malformed_json_returns_none_and_deletes_file() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        fs::create_dir_all(store.path().parent().unwrap())
            .await
            .unwrap();
        fs::write(store.path(), b"not json").await.unwrap();
        assert_eq!(store.load().await.unwrap(), None);
        assert!(!store.path().exists());
    }

    #[test]
    fn discover_prefers_env_home_over_fallback() {
        let dir = TempDir::new().unwrap();
        let store = TokenStore::discover_from(
            Some(dir.path().to_string_lossy().into_owned()),
            Some(PathBuf::from("/should/not/be/used")),
        )
        .unwrap();
        assert_eq!(store.path(), dir.path().join(AUTH_FILE_NAME));
    }

    #[test]
    fn discover_returns_none_when_no_home_resolves() {
        assert!(TokenStore::discover_from(None, None).is_none());
    }

    #[cfg(unix)]
    #[dbt_runtime::test]
    async fn saved_file_has_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        store.save(&sample_token()).await.unwrap();
        let mode = fs::metadata(store.path())
            .await
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "expected 0600, got {:o}", mode & 0o777);
    }

    #[dbt_runtime::test]
    async fn delete_removes_file() {
        let dir = TempDir::new().unwrap();
        let store = store_in(&dir);
        store.save(&sample_token()).await.unwrap();
        store.delete().await.unwrap();
        assert_eq!(store.load().await.unwrap(), None);
    }
}
