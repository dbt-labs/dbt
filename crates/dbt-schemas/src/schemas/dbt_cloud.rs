use dbt_yaml::DbtSchema;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// Represents a dbt Cloud project configuration
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, DbtSchema)]
pub struct DbtCloudProject {
    #[serde(rename = "project-name")]
    pub project_name: String,
    #[serde(rename = "project-id")]
    pub project_id: String,
    #[serde(rename = "account-name")]
    pub account_name: String,
    #[serde(rename = "account-id")]
    pub account_id: String,
    #[serde(rename = "account-host")]
    pub account_host: String,
    #[serde(rename = "token-name")]
    pub token_name: String,
    #[serde(rename = "token-value")]
    pub token_value: String,
}

/// Represents the OAuth client credentials for dbt State authentication.
/// Sourced from the optional `state:` section in `~/.dbt/dbt_cloud.yml`.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, DbtSchema)]
pub struct DbtCloudState {
    #[serde(rename = "client-id")]
    pub client_id: String,
    #[serde(rename = "client-secret")]
    pub client_secret: String,
}

/// Represents the context section of the dbt Cloud configuration
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, DbtSchema)]
pub struct DbtCloudContext {
    #[serde(rename = "active-project")]
    pub active_project: String,
    #[serde(rename = "active-host")]
    pub active_host: String,
    #[serde(rename = "defer-env-id")]
    pub defer_env_id: Option<String>,
}

/// Represents the top-level dbt Cloud configuration file
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, DbtSchema)]
pub struct DbtCloudConfig {
    pub version: String,
    pub context: DbtCloudContext,
    pub projects: Vec<DbtCloudProject>,
    pub state: Option<DbtCloudState>,
}

impl DbtCloudConfig {
    pub fn get_project_by_id(&self, project_id: &str) -> Option<&DbtCloudProject> {
        self.projects.iter().find(|p| p.project_id == project_id)
    }
}

/// This is a helper object to combine values from dbt_project and dbt_cloud that are
/// related to the dbt Cloud config.
#[derive(Debug, Clone)]
pub struct DbtCloudProjectConfig {
    pub defer_env_id: Option<String>,
    pub project: Option<DbtCloudProject>,
}

/// Resolved credentials for authenticating with dbt Cloud APIs.
/// All fields are required — if any credential is missing, this struct is not constructed.
#[derive(Debug, Clone)]
pub struct CloudCredentials {
    pub account_id: String,
    pub host: String,
    pub token: String,
}

/// Fully-resolved dbt Cloud configuration.
///
/// Precedence: env var > dbt_project.yml > dbt_cloud.yml, applied once at
/// construction time via `resolve_cloud_config()`.
#[derive(Clone, Debug, Default)]
pub struct ResolvedCloudConfig {
    pub credentials: Option<CloudCredentials>,
    pub project_id: Option<String>,
    /// dbt platform account id. Resolved independently of [`Self::credentials`],
    /// which additionally requires a host and a token.
    pub account_id: Option<String>,
    pub account_identifier: Option<String>,
    pub environment_id: Option<String>,
    pub defer_env_id: Option<String>,
    pub defer_job_id: Option<String>,
    pub state_org_id: Option<String>,
    pub job_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML_WITH_STATE: &str = r#"
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
    token-value: "dbtc_abc"
state:
  client-id: "client-abc"
  client-secret: "secret-xyz"
"#;

    const YAML_WITHOUT_STATE: &str = r#"
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
    token-value: "dbtc_abc"
"#;

    #[test]
    fn parses_state_section_when_present() {
        let cfg: DbtCloudConfig = dbt_yaml::from_str(YAML_WITH_STATE).unwrap();
        let state = cfg.state.expect("state should be Some");
        assert_eq!(state.client_id, "client-abc");
        assert_eq!(state.client_secret, "secret-xyz");
    }

    #[test]
    fn state_is_none_when_section_absent() {
        let cfg: DbtCloudConfig = dbt_yaml::from_str(YAML_WITHOUT_STATE).unwrap();
        assert!(cfg.state.is_none());
    }
}
