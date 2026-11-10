use crate::{AuthError, Credential};
use dbt_schemas::schemas::DbtCloudConfig;
use dbt_schemas::schemas::project::ProjectDbtCloudConfig;
use serde::Deserialize;
use std::io::ErrorKind;
use std::path::PathBuf;

/// Resolves credentials from `~/.dbt/dbt_cloud.yml`.
///
/// Resolution order for the active project and host (lowest to highest priority):
/// 1. `dbt_cloud.yml` `context` block (`active-project`, `active-host`)
/// 2. `dbt_project.yml` `dbt-cloud` block (`project-id`, `account-host`)
/// 3. Environment variables (`DBT_CLOUD_PROJECT_ID`, `DBT_CLOUD_ACCOUNT_HOST`)
///
/// Credentials (token, account-id) are always read from the matched `projects`
/// entry in `dbt_cloud.yml` — there is no way to supply them via `dbt_project.yml`
/// or environment variables in this resolver (use [`super::EnvVarResolver`] for that).
///
/// The project entry is matched by **both** `project-id` and `account-host`.
#[derive(Default)]
pub struct CloudYamlResolver {
    /// Override path for `dbt_cloud.yml`. Defaults to `~/.dbt/dbt_cloud.yml`.
    pub path: Option<PathBuf>,
    /// Override path for `dbt_project.yml`. Defaults to `dbt_project.yml` in CWD.
    pub dbt_project_path: Option<PathBuf>,
}

/// Thin wrapper to deserialize just the `dbt-cloud` block from `dbt_project.yml`.
#[derive(Debug, Deserialize)]
struct DbtProjectFile {
    #[serde(rename = "dbt-cloud")]
    dbt_cloud: Option<ProjectDbtCloudConfig>,
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl CloudYamlResolver {
    fn default_cloud_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".dbt").join("dbt_cloud.yml"))
    }

    fn default_project_path() -> PathBuf {
        PathBuf::from("dbt_project.yml")
    }

    pub(crate) async fn resolve(&self) -> Result<Credential, AuthError> {
        self.resolve_with_env(non_empty_env).await
    }

    async fn resolve_with_env(
        &self,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Credential, AuthError> {
        let cloud_path = self
            .path
            .clone()
            .or_else(Self::default_cloud_path)
            .ok_or(AuthError::NotAuthenticated)?;

        let content = match std::fs::read_to_string(&cloud_path) {
            Ok(s) => s,
            Err(e) if e.kind() == ErrorKind::NotFound => return Err(AuthError::NotAuthenticated),
            Err(e) => return Err(AuthError::InaccessibleSource(e)),
        };

        let config: DbtCloudConfig = dbt_yaml::from_str(&content).map_err(|e| {
            AuthError::Malformed(format!("failed to parse {}: {e}", cloud_path.display()))
        })?;

        let project_block = self.read_project_block();

        let active_project_id = env("DBT_CLOUD_PROJECT_ID")
            .or_else(|| project_block.as_ref().and_then(|b| b.project_id_str()))
            .unwrap_or_else(|| config.context.active_project.clone());

        let active_host = env("DBT_CLOUD_ACCOUNT_HOST")
            .or_else(|| project_block.as_ref().and_then(|b| b.account_host.clone()))
            .unwrap_or_else(|| config.context.active_host.clone());

        let project = config
            .projects
            .iter()
            .find(|p| p.project_id == active_project_id && p.account_host == active_host);

        let Some(project) = project else {
            return Err(AuthError::NotAuthenticated);
        };

        if project.token_value.is_empty() {
            return Err(AuthError::Malformed(
                "token-value is empty in dbt_cloud.yml; re-download from the dbt platform UI"
                    .to_string(),
            ));
        }

        let account_id = project.account_id.parse::<u64>().map_err(|_| {
            AuthError::Malformed(format!(
                "account-id {:?} in {} is not a valid integer",
                project.account_id,
                cloud_path.display()
            ))
        })?;

        Ok(Credential::from_token(
            project.token_value.clone(),
            project.account_host.clone(),
            account_id,
        ))
    }

    fn read_project_block(&self) -> Option<ProjectDbtCloudConfig> {
        let path = self
            .dbt_project_path
            .clone()
            .unwrap_or_else(Self::default_project_path);

        let content = std::fs::read_to_string(&path).ok()?;
        let file: DbtProjectFile = dbt_yaml::from_str(&content).ok()?;
        file.dbt_cloud
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Write as _;
    use tempfile::NamedTempFile;

    fn write_yaml(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn env_map(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|&(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    fn resolver_at(path: &std::path::Path) -> CloudYamlResolver {
        CloudYamlResolver {
            path: Some(path.to_path_buf()),
            dbt_project_path: None,
        }
    }

    fn resolver_at_with_project(
        cloud_path: &std::path::Path,
        project_path: &std::path::Path,
    ) -> CloudYamlResolver {
        CloudYamlResolver {
            path: Some(cloud_path.to_path_buf()),
            dbt_project_path: Some(project_path.to_path_buf()),
        }
    }

    fn valid_cloud_yaml(project_id: &str, host: &str, token: &str) -> String {
        format!(
            r#"
version: "1"
context:
  active-project: "{project_id}"
  active-host: "{host}"
projects:
  - project-name: "My Project"
    project-id: "{project_id}"
    account-name: "acme"
    account-id: "42"
    account-host: "{host}"
    token-name: "my-token"
    token-value: "{token}"
"#
        )
    }

    #[dbt_runtime::test]
    async fn happy_path_service_token() {
        let f = write_yaml(&valid_cloud_yaml(
            "proj-1",
            "ab123.us1.dbt.com",
            "dbtc_abc123",
        ));
        let cred = resolver_at(f.path())
            .resolve_with_env(no_env)
            .await
            .unwrap();
        assert!(matches!(cred, Credential::ServiceToken { .. }));
        assert_eq!(cred.token(), "dbtc_abc123");
        assert_eq!(cred.account_host(), "ab123.us1.dbt.com");
        assert_eq!(cred.account_id(), 42);
    }

    #[dbt_runtime::test]
    async fn happy_path_pat() {
        let f = write_yaml(&valid_cloud_yaml(
            "proj-1",
            "ab123.us1.dbt.com",
            "dbtu_user_token",
        ));
        let cred = resolver_at(f.path())
            .resolve_with_env(no_env)
            .await
            .unwrap();
        assert!(matches!(cred, Credential::Pat { .. }));
        assert_eq!(cred.token(), "dbtu_user_token");
    }

    #[dbt_runtime::test]
    async fn missing_cloud_yaml_returns_not_authenticated() {
        let r = CloudYamlResolver {
            path: Some(PathBuf::from("/nonexistent/path/dbt_cloud.yml")),
            dbt_project_path: None,
        };
        let err = r.resolve_with_env(no_env).await.unwrap_err();
        assert!(matches!(err, AuthError::NotAuthenticated));
    }

    #[dbt_runtime::test]
    async fn no_matching_project_returns_not_authenticated() {
        let f = write_yaml(
            r#"
version: "1"
context:
  active-project: "proj-99"
  active-host: "ab123.us1.dbt.com"
projects:
  - project-name: "My Project"
    project-id: "proj-1"
    account-name: "acme"
    account-id: "42"
    account-host: "ab123.us1.dbt.com"
    token-name: "my-token"
    token-value: "dbtc_abc123"
"#,
        );
        let err = resolver_at(f.path())
            .resolve_with_env(no_env)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::NotAuthenticated));
    }

    #[dbt_runtime::test]
    async fn host_mismatch_returns_not_authenticated() {
        let f = write_yaml(
            r#"
version: "1"
context:
  active-project: "proj-1"
  active-host: "other.dbt.com"
projects:
  - project-name: "My Project"
    project-id: "proj-1"
    account-name: "acme"
    account-id: "42"
    account-host: "ab123.us1.dbt.com"
    token-name: "my-token"
    token-value: "dbtc_abc123"
"#,
        );
        let err = resolver_at(f.path())
            .resolve_with_env(no_env)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::NotAuthenticated));
    }

    #[dbt_runtime::test]
    async fn invalid_yaml_returns_malformed() {
        let f = write_yaml("not: valid: yaml: [[[");
        let err = resolver_at(f.path())
            .resolve_with_env(no_env)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::Malformed(_)));
    }

    #[dbt_runtime::test]
    async fn empty_token_value_returns_malformed() {
        let f = write_yaml(
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
    token-value: ""
"#,
        );
        let err = resolver_at(f.path())
            .resolve_with_env(no_env)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::Malformed(_)));
    }

    #[dbt_runtime::test]
    async fn non_numeric_account_id_returns_malformed() {
        let f = write_yaml(
            r#"
version: "1"
context:
  active-project: "proj-1"
  active-host: "ab123.us1.dbt.com"
projects:
  - project-name: "My Project"
    project-id: "proj-1"
    account-name: "acme"
    account-id: "not-a-number"
    account-host: "ab123.us1.dbt.com"
    token-name: "my-token"
    token-value: "dbtc_abc123"
"#,
        );
        let err = resolver_at(f.path())
            .resolve_with_env(no_env)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::Malformed(_)));
    }

    #[dbt_runtime::test]
    async fn dbt_project_yml_overrides_active_project() {
        let cloud = write_yaml(
            r#"
version: "1"
context:
  active-project: "proj-1"
  active-host: "ab123.us1.dbt.com"
projects:
  - project-id: "proj-1"
    project-name: "Default"
    account-name: "acme"
    account-id: "1"
    account-host: "ab123.us1.dbt.com"
    token-name: "tok"
    token-value: "dbtc_default"
  - project-id: "proj-2"
    project-name: "Override"
    account-name: "acme"
    account-id: "2"
    account-host: "ab123.us1.dbt.com"
    token-name: "tok"
    token-value: "dbtc_override"
"#,
        );
        let project = write_yaml(
            r#"
name: my_project
dbt-cloud:
  project-id: "proj-2"
"#,
        );
        let cred = resolver_at_with_project(cloud.path(), project.path())
            .resolve_with_env(no_env)
            .await
            .unwrap();
        assert_eq!(cred.token(), "dbtc_override");
        assert_eq!(cred.account_id(), 2);
    }

    #[dbt_runtime::test]
    async fn dbt_project_yml_integer_project_id() {
        let cloud = write_yaml(
            r#"
version: "1"
context:
  active-project: "proj-1"
  active-host: "ab123.us1.dbt.com"
projects:
  - project-id: "proj-1"
    project-name: "Default"
    account-name: "acme"
    account-id: "1"
    account-host: "ab123.us1.dbt.com"
    token-name: "tok"
    token-value: "dbtc_default"
  - project-id: "123456"
    project-name: "Integer ID"
    account-name: "acme"
    account-id: "9"
    account-host: "ab123.us1.dbt.com"
    token-name: "tok"
    token-value: "dbtc_intid"
"#,
        );
        let project = write_yaml(
            r#"
name: my_project
dbt-cloud:
  project-id: 123456
"#,
        );
        let cred = resolver_at_with_project(cloud.path(), project.path())
            .resolve_with_env(no_env)
            .await
            .unwrap();
        assert_eq!(cred.token(), "dbtc_intid");
        assert_eq!(cred.account_id(), 9);
    }

    #[dbt_runtime::test]
    async fn dbt_project_yml_missing_is_ignored() {
        let cloud = write_yaml(&valid_cloud_yaml("proj-1", "ab123.us1.dbt.com", "dbtc_abc"));
        let r = CloudYamlResolver {
            path: Some(cloud.path().to_path_buf()),
            dbt_project_path: Some(PathBuf::from("/nonexistent/dbt_project.yml")),
        };
        let cred = r.resolve_with_env(no_env).await.unwrap();
        assert_eq!(cred.token(), "dbtc_abc");
    }

    #[dbt_runtime::test]
    async fn env_var_overrides_active_project() {
        let cloud = write_yaml(
            r#"
version: "1"
context:
  active-project: "proj-1"
  active-host: "ab123.us1.dbt.com"
projects:
  - project-id: "proj-1"
    project-name: "Default"
    account-name: "acme"
    account-id: "1"
    account-host: "ab123.us1.dbt.com"
    token-name: "tok"
    token-value: "dbtc_default"
  - project-id: "proj-2"
    project-name: "EnvOverride"
    account-name: "acme"
    account-id: "2"
    account-host: "ab123.us1.dbt.com"
    token-name: "tok"
    token-value: "dbtc_env"
"#,
        );
        let cred = resolver_at(cloud.path())
            .resolve_with_env(env_map(&[("DBT_CLOUD_PROJECT_ID", "proj-2")]))
            .await
            .unwrap();
        assert_eq!(cred.token(), "dbtc_env");
        assert_eq!(cred.account_id(), 2);
    }

    #[dbt_runtime::test]
    async fn env_var_overrides_host() {
        let cloud = write_yaml(
            r#"
version: "1"
context:
  active-project: "proj-1"
  active-host: "ab123.us1.dbt.com"
projects:
  - project-id: "proj-1"
    project-name: "US"
    account-name: "acme"
    account-id: "1"
    account-host: "ab123.us1.dbt.com"
    token-name: "tok"
    token-value: "dbtc_us"
  - project-id: "proj-1"
    project-name: "EMEA"
    account-name: "acme"
    account-id: "2"
    account-host: "emea.dbt.com"
    token-name: "tok"
    token-value: "dbtc_emea"
"#,
        );
        let cred = resolver_at(cloud.path())
            .resolve_with_env(env_map(&[("DBT_CLOUD_ACCOUNT_HOST", "emea.dbt.com")]))
            .await
            .unwrap();
        assert_eq!(cred.token(), "dbtc_emea");
        assert_eq!(cred.account_id(), 2);
    }

    #[dbt_runtime::test]
    async fn env_var_takes_priority_over_dbt_project_yml() {
        let cloud = write_yaml(
            r#"
version: "1"
context:
  active-project: "proj-1"
  active-host: "ab123.us1.dbt.com"
projects:
  - project-id: "proj-1"
    project-name: "Default"
    account-name: "acme"
    account-id: "1"
    account-host: "ab123.us1.dbt.com"
    token-name: "tok"
    token-value: "dbtc_default"
  - project-id: "proj-env"
    project-name: "EnvWins"
    account-name: "acme"
    account-id: "3"
    account-host: "ab123.us1.dbt.com"
    token-name: "tok"
    token-value: "dbtc_env_wins"
"#,
        );
        let project = write_yaml(
            r#"
name: my_project
dbt-cloud:
  project-id: "proj-project-yml"
"#,
        );
        let cred = resolver_at_with_project(cloud.path(), project.path())
            .resolve_with_env(env_map(&[("DBT_CLOUD_PROJECT_ID", "proj-env")]))
            .await
            .unwrap();
        assert_eq!(cred.token(), "dbtc_env_wins");
    }

    /// Verifies the resolver reads the actual process environment end-to-end.
    #[test]
    fn real_env_integration() {
        let _guard = crate::TEST_ENV_LOCK.lock().unwrap();
        let cloud = write_yaml(&valid_cloud_yaml(
            "proj-1",
            "ab123.us1.dbt.com",
            "dbtc_real",
        ));
        unsafe {
            #[allow(clippy::disallowed_methods)]
            std::env::set_var("DBT_CLOUD_PROJECT_ID", "proj-1");
        }
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(resolver_at(cloud.path()).resolve());
        unsafe {
            #[allow(clippy::disallowed_methods)]
            std::env::remove_var("DBT_CLOUD_PROJECT_ID");
        }
        let cred = result.unwrap();
        assert_eq!(cred.token(), "dbtc_real");
    }
}
