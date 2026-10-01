use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter, auth_configure_pipeline};
use database::Builder as DatabaseBuilder;
use dbt_adbc::bigquery::auth_type;
use dbt_adbc::{Backend, bigquery, database};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::Path;
use url::Url;

type YmlValue = dbt_yaml::Value;

#[derive(Deserialize, Serialize)]
struct KeyFileJson {
    #[serde(rename = "type")]
    pub file_type: String,
    pub project_id: String,
    pub private_key_id: String,
    pub private_key: String,
    pub client_email: String,
    pub client_id: String,
    pub auth_uri: String,
    pub token_uri: String,
    pub auth_provider_x509_cert_url: String,
    pub client_x509_cert_url: String,
}

pub struct BigqueryAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl BigqueryAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

enum BigqueryAuthIR<'a> {
    /// Interactive `gcloud` login flow (driver default auth).
    Oauth,
    /// Service account credentials loaded from a JSON file path.
    ServiceAccount { keyfile: &'a str },
    /// Service account credentials provided inline as YAML mapping or base64-encoded JSON.
    ServiceAccountJson { keyfile_json: &'a YmlValue },
    /// OAuth secrets flow that exchanges a refresh token for access tokens.
    OauthSecretsRefresh {
        refresh_token: &'a str,
        client_id: &'a str,
        client_secret: &'a str,
        token_uri: &'a str,
    },
    /// OAuth secrets flow using a temporary access token directly.
    OauthSecretsTemporary { access_token: &'a str },
    /// Workload Identity Federation via an external OAuth identity provider.
    ExternalOauthWif {
        workload_pool_provider_path: &'a str,
        service_account_impersonation_url: Option<&'a str>,
        request_url: &'a str,
        request_data: &'a str,
    },
}

impl<'a> BigqueryAuthIR<'a> {
    pub fn apply(
        self,
        mut builder: DatabaseBuilder,
        _warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<DatabaseBuilder, AuthError> {
        match self {
            Self::Oauth => {
                builder.with_named_option(bigquery::AUTH_TYPE, auth_type::DEFAULT)?;
            }
            Self::ServiceAccount { keyfile } => {
                let expanded_path = shellexpand::tilde(keyfile).to_string();
                if Path::new(&expanded_path).exists() {
                    builder
                        .with_named_option(bigquery::AUTH_TYPE, auth_type::JSON_CREDENTIAL_FILE)?;
                    builder
                        .with_named_option(bigquery::AUTH_CREDENTIALS, expanded_path.as_str())?;
                } else {
                    return Err(AuthError::config(format!(
                        "Keyfile '{keyfile}' does not exist"
                    )));
                }
            }
            Self::ServiceAccountJson { keyfile_json } => {
                let keyfile_json_string = keyfile_json_to_credential_string(keyfile_json)?;
                builder
                    .with_named_option(bigquery::AUTH_TYPE, auth_type::JSON_CREDENTIAL_STRING)?;
                builder.with_named_option(bigquery::AUTH_CREDENTIALS, keyfile_json_string)?;
            }
            Self::OauthSecretsRefresh {
                refresh_token,
                client_id,
                client_secret,
                token_uri,
            } => {
                builder.with_named_option(bigquery::AUTH_TYPE, auth_type::USER_AUTHENTICATION)?;
                builder.with_named_option(bigquery::AUTH_CLIENT_ID, client_id)?;
                builder.with_named_option(bigquery::AUTH_CLIENT_SECRET, client_secret)?;
                builder.with_named_option(bigquery::AUTH_REFRESH_TOKEN, refresh_token)?;
                builder.with_named_option(bigquery::AUTH_ACCESS_TOKEN_ENDPOINT, token_uri)?;
            }
            Self::OauthSecretsTemporary { access_token } => {
                builder
                    .with_named_option(bigquery::AUTH_TYPE, auth_type::TEMPORARY_ACCESS_TOKEN)?;
                builder.with_named_option(bigquery::AUTH_ACCESS_TOKEN, access_token)?;
            }
            Self::ExternalOauthWif {
                workload_pool_provider_path,
                service_account_impersonation_url,
                request_url,
                request_data,
            } => {
                builder.with_named_option(bigquery::AUTH_TYPE, auth_type::EXTERNAL_ACCOUNT)?;
                builder.with_named_option(
                    bigquery::AUTH_EXTERNAL_ACCOUNT_AUDIENCE,
                    workload_pool_provider_path,
                )?;
                builder
                    .with_named_option(bigquery::AUTH_EXTERNAL_ACCOUNT_REQUEST_URL, request_url)?;
                builder.with_named_option(
                    bigquery::AUTH_EXTERNAL_ACCOUNT_REQUEST_DATA,
                    request_data,
                )?;
                if let Some(impersonation_url) = service_account_impersonation_url {
                    builder.with_named_option(
                        bigquery::AUTH_EXTERNAL_ACCOUNT_IMPERSONATION_URL,
                        impersonation_url,
                    )?;
                }
            }
        }

        Ok(builder)
    }
}

/// Identity provider `type`s accepted in `token_endpoint` for `external-oauth-wif`.
const SUPPORTED_TOKEN_ENDPOINT_TYPES: &[&str] = &["entra"];

// The bigquery ADBC driver appends "bigquery/v2/" to the hostname provided
// in `api_endpoint`.
//
// Here, we reject anything that can't be transformed into "http(s)://host[:port]/"
// to fit that routine's preconditions.
fn validate_api_endpoint<'a>(
    endpoint: &'a str,
    warning_printer: &dyn AuthWarningPrinter,
) -> Result<Cow<'a, str>, AuthError> {
    let url = Url::parse(endpoint).map_err(|_| {
        AuthError::config(format!(
            "'api_endpoint' must start with 'http://' or 'https://': {endpoint:?}"
        ))
    })?;

    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(AuthError::config(format!(
            "'api_endpoint' must start with 'http://' or 'https://': {endpoint:?}"
        )));
    }

    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(AuthError::config(format!(
            "'api_endpoint' must be a bare host (with optional port), e.g. 'https://your-proxy.example.com/': {endpoint:?}"
        )));
    }

    if !endpoint.ends_with('/') {
        warning_printer.warn(&format!(
            "'api_endpoint' is missing a trailing '/', which dbt appends automatically. We suggest setting it to '{endpoint}/' instead."
        ));
        Ok(Cow::Owned(format!("{endpoint}/")))
    } else {
        Ok(Cow::Borrowed(endpoint))
    }
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<BigqueryAuthIR<'a>, AuthError> {
    let method = config
        .get_str("method")
        .ok_or_else(|| AuthError::config("Missing required 'method' field in BigQuery config"))?;

    match method {
        "oauth" => Ok(BigqueryAuthIR::Oauth),
        "service-account" => {
            let keyfile = config.get_str("keyfile").ok_or_else(|| {
                AuthError::config("Missing required field 'keyfile' for method 'service-account'")
            })?;
            Ok(BigqueryAuthIR::ServiceAccount { keyfile })
        }
        "service-account-json" => {
            let keyfile_json = config.require("keyfile_json")?;
            Ok(BigqueryAuthIR::ServiceAccountJson { keyfile_json })
        }
        "oauth-secrets" => {
            if let Some(refresh_token) = config.get_str("refresh_token") {
                let client_id = config.require_str("client_id")?;
                let client_secret = config.require_str("client_secret")?;
                let token_uri = config.require_str("token_uri")?;
                Ok(BigqueryAuthIR::OauthSecretsRefresh {
                    refresh_token,
                    client_id,
                    client_secret,
                    token_uri,
                })
            } else if let Some(access_token) = config.get_str("token") {
                Ok(BigqueryAuthIR::OauthSecretsTemporary { access_token })
            } else {
                Err(AuthError::config(
                    "For method 'oauth-secrets', either 'refresh_token', 'client_secret', ... or 'token' must be provided",
                ))
            }
        }
        "external-oauth-wif" => {
            let workload_pool_provider_path = config
                .get_str("workload_pool_provider_path")
                .ok_or_else(|| {
                    AuthError::config(
                        "Missing required field 'workload_pool_provider_path' for method 'external-oauth-wif'",
                    )
                })?;
            let service_account_impersonation_url =
                config.get_str("service_account_impersonation_url");
            let token_endpoint = config.require("token_endpoint")?;

            let token_endpoint_map = token_endpoint
                .as_mapping()
                .ok_or_else(|| AuthError::config("'token_endpoint' must be a YAML mapping"))?;

            let token_endpoint_type = token_endpoint_map
                .get("type")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    AuthError::config("Missing required key in token_endpoint: 'type'")
                })?;
            if !SUPPORTED_TOKEN_ENDPOINT_TYPES.contains(&token_endpoint_type) {
                return Err(AuthError::config(format!(
                    "Unsupported identity provider type: {token_endpoint_type}. Supported types: {}",
                    SUPPORTED_TOKEN_ENDPOINT_TYPES.join(", ")
                )));
            }

            let request_url = token_endpoint_map
                .get("request_url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    AuthError::config("Missing required key in token_endpoint: 'request_url'")
                })?;
            let request_data = token_endpoint_map
                .get("request_data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    AuthError::config("Missing required key in token_endpoint: 'request_data'")
                })?;

            Ok(BigqueryAuthIR::ExternalOauthWif {
                workload_pool_provider_path,
                service_account_impersonation_url,
                request_url,
                request_data,
            })
        }
        unknown_method => Err(AuthError::config(format!(
            "Unknown or unimplemented authentication method '{unknown_method}' for BigQuery"
        ))),
    }
}

/// Derive the project ID from the config.
///
/// The project ID is optional as in some auth methods it is inferred from the credentials.
fn project_id(config: &AdapterConfig) -> Result<Option<String>, AuthError> {
    let project_id = if let Some(execution_project) = config.get_string("execution_project") {
        Some(execution_project)
    } else if let Some(project) = config.get_string("project") {
        if config.get_string("database").is_some() {
            return Err(AuthError::config(
                "Don't specify 'database' when 'project' is specified",
            ));
        }
        Some(project)
    } else {
        config.get_string("database") // use "database" as GCP project ID
    };

    Ok(project_id.map(|s| s.into_owned()))
}

/// Derive the dataset ID from the config.
fn dataset_id(config: &AdapterConfig) -> Result<String, AuthError> {
    let dataset = config.get_string("dataset");
    let schema = config.get_string("schema");
    let dataset_id = if let Some(d) = dataset {
        if schema.is_some() {
            return Err(AuthError::config(
                "Don't specify both 'dataset' and 'schema' in BigQuery config, they are aliases",
            ));
        }
        d
    } else if let Some(s) = schema {
        s
    } else {
        return Err(AuthError::config(
            "Missing required field 'dataset' or 'schema'",
        ));
    };

    Ok(dataset_id.into_owned())
}

fn resolve_impersonate_scopes(config: &AdapterConfig) -> String {
    let mut scopes = bigquery::IMPERSONATE_DEFAULT_SCOPES.join(",");
    if let Some(impersonate_scopes) = config.get("scopes")
        && let Some(impersonate_scopes) = match impersonate_scopes {
            YmlValue::Sequence(scope_seq, _) => {
                let mut scopes = Vec::with_capacity(scope_seq.len());
                for item in scope_seq {
                    if let YmlValue::String(scope, _) = item {
                        scopes.push(scope.to_string())
                    }
                }
                Some(scopes)
            }
            _ => None,
        }
    {
        scopes = impersonate_scopes.join(",");
    }
    scopes
}

fn apply_connection_args(
    config: &AdapterConfig,
    mut builder: DatabaseBuilder,
    warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabaseBuilder, AuthError> {
    if let Some(project_id) = project_id(config)? {
        builder.with_named_option(bigquery::PROJECT_ID, project_id)?;
    }

    if let Some(quota_project) = config.get_string("quota_project") {
        builder.with_named_option(bigquery::AUTH_QUOTA_PROJECT, quota_project)?;
    }

    let dataset_id = dataset_id(config)?;
    builder.with_named_option(bigquery::DATASET_ID, dataset_id)?;

    if let Some(api_endpoint) = config.get_str("api_endpoint") {
        let api_endpoint = validate_api_endpoint(api_endpoint, warning_printer)?;
        builder.with_named_option(bigquery::API_ENDPOINT, api_endpoint.as_ref())?;
    }

    if let Some(location) = config.get_str("location") {
        builder.with_named_option(bigquery::LOCATION, location)?;
    }

    if let Some(impersonate_principal) = config.get_str("impersonate_service_account") {
        builder.with_named_option(
            bigquery::IMPERSONATE_TARGET_PRINCIPAL,
            impersonate_principal,
        )?;
    }

    let scopes = resolve_impersonate_scopes(config);
    builder.with_named_option(bigquery::IMPERSONATE_SCOPES, scopes)?;

    Ok(builder)
}

fn keyfile_json_to_credential_string(keyfile_json: &YmlValue) -> Result<String, AuthError> {
    let keyfile_yaml = match keyfile_json {
        YmlValue::Mapping(_, _) => keyfile_json.clone(),
        YmlValue::String(json_str, _) => {
            // Attempt to decode as base 64. Otherwise, assume that this is
            // a JSON string.
            use base64::prelude::*;
            let keyfile_yaml: YmlValue = if let Ok(decoded) = BASE64_STANDARD.decode(json_str) {
                serde_json::from_slice(&decoded)?
            } else {
                serde_json::from_str(json_str)?
            };
            if keyfile_yaml.is_mapping() {
                keyfile_yaml
            } else {
                return Err(AuthError::config(
                    "'keyfile_json' must be a JSON object when provided as a string",
                ));
            }
        }
        _ => {
            return Err(AuthError::config(
                "'keyfile_json' must be a YAML mapping or string",
            ));
        }
    };

    let mut keyfile_json: KeyFileJson = dbt_yaml::from_value(keyfile_yaml).map_err(|e| {
        AuthError::config(format!(
            "Error parsing 'keyfile_json' in BigQuery configuration: {e}"
        ))
    })?;
    keyfile_json.private_key = keyfile_json.private_key.replace("\\n", "\n");

    let keyfile_json_string: String = serde_json::to_value(keyfile_json)
        .map_err(|e| AuthError::config(e.to_string()))?
        .to_string();

    Ok(keyfile_json_string)
}

impl Auth for BigqueryAuth {
    fn backend(&self) -> Backend {
        Backend::BigQuery
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        auth_configure_pipeline!(self, &config, parse_auth, apply_connection_args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_options::other_option_value;
    use adbc_core::options::OptionDatabase;
    use dbt_test_primitives::assert_contains;
    use dbt_yaml::Mapping;

    fn base_config_oauth() -> Mapping {
        Mapping::from_iter([
            ("method".into(), "oauth".into()),
            ("database".into(), "my_db".into()),
            ("schema".into(), "my_schema".into()),
        ])
    }

    fn base_config_keyfile() -> Mapping {
        Mapping::from_iter([
            ("method".into(), "service-account".into()),
            ("database".into(), "my_db".into()),
            ("schema".into(), "my_schema".into()),
            ("keyfile".into(), "akeyfilethatdoesnotexist.json".into()),
        ])
    }

    fn base_config_keyfile_json() -> Mapping {
        Mapping::from_iter([
            ("method".into(), "service-account-json".into()),
            ("database".into(), "my_db".into()),
            ("schema".into(), "my_schema".into()),
            (
                "keyfile_json".into(),
                r#"{
                    "type": "service_account",
                    "project_id": "bq-project",
                    "private_key_id": "xyz123",
                    "private_key": "-----BEGIN PRIVATE KEY-----\nXYZ\n-----END PRIVATE KEY-----",
                    "client_email": "xyz@123.iam.gserviceaccount.com",
                    "client_id": "111222333",
                    "auth_uri": "https://accounts.google.com/o/oauth2/auth",
                    "token_uri": "https://oauth2.googleapis.com/token",
                    "auth_provider_x509_cert_url": "https://www.googleapis.com/oauth2/v1/certs",
                    "client_x509_cert_url": "https://www.googleapis.com/robot/v1/metadata/x509/fde-bigquery%40fde-testing-450816.iam.gserviceaccount.com"
                }"#.into(),
            ),
        ])
    }

    fn base_config_keyfile_json_base64() -> Mapping {
        Mapping::from_iter([
            ("method".into(), "service-account-json".into()),
            ("database".into(), "my_db".into()),
            ("schema".into(), "my_schema".into()),
            (
                "keyfile_json".into(),
                (
                    "ewogICJ0eXBlIjogInNlcnZpY2VfYWNjb3VudCIsCiAgInByb2plY3RfaWQiOiAiYnEtcHJvamVjdCIsCiAgInByaXZhdGVfa2V5X2lkIjogInh5ejEyMyIsCiAgInByaXZhdGVfa2V5IjogIi0tLS0tQkVHSU4gUFJJVkFURSBLRVktLS0tLVxuWFlaXG4tLS0tLUVORCBQUklWQVRFIEtFWS0tLS0tIiwKICAiY2xpZW50X2VtYWlsIjogInh5ekAxMjMuaWFtLmdzZXJ2aWNlYWNjb3VudC5jb20iLAogICJjbGllbnRfaWQiOiAiMTExMjIyMzMzIiwKICAiYXV0aF91cmkiOiAiaHR0cHM6Ly9hY2NvdW50cy5nb29nbGUuY29tL28vb2F1dGgyL2F1dGgiLAogICJ0b2tlbl91cmkiOiAiaHR0cHM6Ly9vYXV0aDIuZ29vZ2xlYXBpcy5jb20vdG9rZW4iLAogICJhdXRoX3Byb3ZpZGVyX3g1MDlfY2VydF91cmwiOiAiaHR0cHM6Ly93d3cuZ29vZ2xlYXBpcy5jb20vb2F1dGgyL3YxL2NlcnRzIiwKICAiY2xpZW50X3g1MDlfY2VydF91cmwiOiAiaHR0cHM6Ly93d3cuZ29vZ2xlYXBpcy5jb20vcm9ib3QvdjEvbWV0YWRhdGEveDUwOS9mZGUtYmlncXVlcnklNDBmZGUtdGVzdGluZy00NTA4MTYuaWFtLmdzZXJ2aWNlYWNjb3VudC5jb20iCn0="
                ).into(),
            ),
        ])
    }

    fn try_configure(config: Mapping) -> Result<database::Builder, AuthError> {
        let auth = BigqueryAuth::new(Box::new(crate::NoopAuthWarningPrinter));
        let adapter_config = AdapterConfig::new(config);
        auth.configure(&adapter_config)
    }

    #[test]
    fn test_auth_config_from_adapter_config_mismatch() {
        let mut config = base_config_keyfile();
        config.insert("method".into(), "service-account-json".into());
        let result = try_configure(config);
        assert!(result.is_err(), "Expected error with mismatch");
    }

    #[test]
    fn test_auth_config_from_adapter_config_keyfile() {
        let config = base_config_keyfile();
        let err = try_configure(config).unwrap_err();
        assert_contains!(
            err.msg(),
            "Keyfile 'akeyfilethatdoesnotexist.json' does not exist"
        );
    }

    #[test]
    fn test_auth_config_from_adapter_config_keyfile_json() {
        let config = base_config_keyfile_json();
        match try_configure(config) {
            Ok(builder) => {
                assert_eq!(
                    other_option_value(&builder, bigquery::AUTH_TYPE).unwrap(),
                    auth_type::JSON_CREDENTIAL_STRING
                );
                let keyfile_json =
                    other_option_value(&builder, bigquery::AUTH_CREDENTIALS).unwrap();
                assert!(keyfile_json.contains(r#""type":"service_account""#));
                assert_contains!(keyfile_json, "BEGIN PRIVATE KEY");
                assert_contains!(keyfile_json, "END PRIVATE KEY");
            }
            Err(err) => {
                panic!("Auth config mapping failed with error: {err:?}")
            }
        }
    }

    #[test]
    fn test_auth_config_from_adapter_config_keyfile_json_base64() {
        let config = base_config_keyfile_json_base64();
        match try_configure(config) {
            Ok(builder) => {
                assert_eq!(
                    other_option_value(&builder, bigquery::AUTH_TYPE).unwrap(),
                    auth_type::JSON_CREDENTIAL_STRING
                );
                let keyfile_json =
                    other_option_value(&builder, bigquery::AUTH_CREDENTIALS).unwrap();
                assert!(keyfile_json.contains(r#""type":"service_account""#));
                assert_contains!(keyfile_json, "BEGIN PRIVATE KEY");
                assert_contains!(keyfile_json, "END PRIVATE KEY");
            }
            Err(err) => {
                panic!("Auth config mapping failed with error: {err:?}")
            }
        }
    }

    #[test]
    fn test_builder_from_auth_config_keyfile_json() {
        let yaml_doc = r#"
method: service-account-json
database: my_db
schema: my_schema
api_endpoint: https://bigquery.googleapis.com/
keyfile_json:
    type: service_account
    project_id: bq-project
    private_key_id: xyz123
    private_key: |
        -----BEGIN PRIVATE KEY-----
        XYZ
        -----END PRIVATE KEY-----
    client_email: xyz@123.iam.gserviceaccount.com
    client_id: "111222333"
    auth_uri: https://accounts.google.com/o/oauth2/auth
    token_uri: https://oauth2.googleapis.com/token
    auth_provider_x509_cert_url: https://www.googleapis.com/oauth2/v1/certs
    client_x509_cert_url: https://www.googleapis.com/robot/v1/metadata/x509/fde-bigquery%40fde-testing-450816.iam.gserviceaccount.com
location: my_location
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let builder = try_configure(config).unwrap();
        let credentials = other_option_value(&builder, bigquery::AUTH_CREDENTIALS)
            .expect("Expected AUTH_CREDENTIALS option to be set");
        assert!(credentials.contains(r#""type":"service_account""#));
        assert_contains!(credentials, "BEGIN PRIVATE KEY");
        assert_contains!(credentials, "END PRIVATE KEY");

        assert_eq!(
            other_option_value(&builder, bigquery::PROJECT_ID).unwrap(),
            "my_db"
        );
        assert_eq!(
            other_option_value(&builder, bigquery::DATASET_ID).unwrap(),
            "my_schema"
        );
        assert_eq!(
            other_option_value(&builder, bigquery::API_ENDPOINT).unwrap(),
            "https://bigquery.googleapis.com/"
        );
        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_TYPE).unwrap(),
            auth_type::JSON_CREDENTIAL_STRING
        );
        assert_eq!(
            other_option_value(&builder, bigquery::LOCATION).unwrap(),
            "my_location"
        );
        let default_scopes = bigquery::IMPERSONATE_DEFAULT_SCOPES.join(",");
        assert_eq!(
            other_option_value(&builder, bigquery::IMPERSONATE_SCOPES).unwrap(),
            default_scopes.as_str()
        );

        let mut keys: Vec<&str> = builder
            .other
            .iter()
            .filter_map(|(k, _)| match k {
                OptionDatabase::Other(name) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        keys.sort_unstable();
        let mut expected = vec![
            bigquery::AUTH_CREDENTIALS,
            bigquery::AUTH_TYPE,
            bigquery::DATASET_ID,
            bigquery::API_ENDPOINT,
            bigquery::IMPERSONATE_SCOPES,
            bigquery::LOCATION,
            bigquery::PROJECT_ID,
        ];
        expected.sort_unstable();
        assert_eq!(keys, expected);
    }

    #[test]
    fn test_builder_from_auth_config_oauth_with_api_endpoint() {
        let yaml_doc = r#"
database: my_db
schema: my_schema
method: oauth
api_endpoint: https://definitely-not-bigquery.invalid/
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let builder = try_configure(config).unwrap();
        assert_eq!(
            other_option_value(&builder, bigquery::API_ENDPOINT).unwrap(),
            "https://definitely-not-bigquery.invalid/"
        );
        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_TYPE).unwrap(),
            auth_type::DEFAULT
        );
    }

    #[test]
    fn test_builder_from_auth_config_oauth_api_endpoint_normalizes_missing_trailing_slash() {
        let yaml_doc = r#"
database: my_db
schema: my_schema
method: oauth
api_endpoint: https://definitely-not-bigquery.invalid
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let builder = try_configure(config).unwrap();
        assert_eq!(
            other_option_value(&builder, bigquery::API_ENDPOINT).unwrap(),
            "https://definitely-not-bigquery.invalid/"
        );
    }

    #[test]
    fn test_builder_from_auth_config_oauth_api_endpoint_rejects_path() {
        let yaml_doc = r#"
database: my_db
schema: my_schema
method: oauth
api_endpoint: https://bigquery.googleapis.com/bigquery/v2/
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let err = try_configure(config).unwrap_err();
        assert_contains!(err.msg(), "bare host");
    }

    #[test]
    fn test_builder_from_auth_config_oauth_api_endpoint_rejects_missing_scheme() {
        let yaml_doc = r#"
database: my_db
schema: my_schema
method: oauth
api_endpoint: definitely-not-bigquery.invalid/
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let err = try_configure(config).unwrap_err();
        assert_contains!(err.msg(), "http://' or 'https://'");
    }

    #[test]
    fn test_builder_from_auth_config_oauth_secrets_temporary_token() {
        let yaml_doc = r#"
method: oauth-secrets
database: my_db
schema: my_schema
token: 12345abcde
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();

        let builder = try_configure(config).unwrap();
        let acces_token = other_option_value(&builder, bigquery::AUTH_ACCESS_TOKEN)
            .expect("Expected AUTH_ACCESS_TOKEN option to be set");
        assert_eq!(acces_token, "12345abcde");
    }

    #[test]
    fn test_auth_config_from_adapter_config_oauth() {
        let config = base_config_oauth();
        let builder = try_configure(config).unwrap();
        let auth_type = other_option_value(&builder, bigquery::AUTH_TYPE)
            .expect("Expected AUTH_TYPE option to be set");
        assert_eq!(auth_type, auth_type::DEFAULT);
    }

    #[test]
    fn test_builder_from_auth_config_oauth() {
        let yaml_doc = r#"
database: my_db
schema: my_schema
method: oauth
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let builder = try_configure(config).unwrap();
        let auth_type = other_option_value(&builder, bigquery::AUTH_TYPE)
            .expect("Expected AUTH_TYPE option to be set");
        assert_eq!(auth_type, auth_type::DEFAULT);

        assert!(other_option_value(&builder, bigquery::AUTH_CREDENTIALS).is_none());
        assert!(other_option_value(&builder, bigquery::AUTH_REFRESH_TOKEN).is_none());
    }

    #[test]
    fn test_builder_from_auth_config_oauth_with_custom_scopes() {
        let yaml_doc = r#"
database: my_db
schema: my_schema
method: oauth
scopes:
    - https://www.googleapis.com/auth/bigquery
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let builder = try_configure(config).unwrap();
        let auth_type = other_option_value(&builder, bigquery::AUTH_TYPE)
            .expect("Expected AUTH_TYPE option to be set");
        assert_eq!(auth_type, auth_type::DEFAULT);
        let scopes = other_option_value(&builder, bigquery::IMPERSONATE_SCOPES)
            .expect("Expected IMPERSONATE_SCOPES option to be set");
        assert_eq!(scopes, "https://www.googleapis.com/auth/bigquery");
    }

    #[test]
    fn test_builder_from_auth_config_oauth_with_impersonation() {
        let yaml_doc = r#"
database: my_db
schema: my_schema
method: oauth
impersonate_service_account: user@project.iam.gserviceaccount.com
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let builder = try_configure(config).unwrap();
        let auth_type = other_option_value(&builder, bigquery::AUTH_TYPE)
            .expect("Expected AUTH_TYPE option to be set");
        assert_eq!(auth_type, auth_type::DEFAULT);
        let scopes = other_option_value(&builder, bigquery::IMPERSONATE_TARGET_PRINCIPAL)
            .expect("Expected IMPERSONATE_TARGET_PRINCIPAL option to be set");
        assert_eq!(scopes, "user@project.iam.gserviceaccount.com");
    }

    #[test]
    fn test_auth_config_oauth_allow_redundant_fields() {
        let mut config = base_config_oauth();
        config.insert("keyfile".into(), YmlValue::from("some.json"));

        try_configure(config)
            .expect("Expected no error when extra fields are supplied for OAuth method");
    }

    #[test]
    fn test_auth_config_from_adapter_config_keyfile_success() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut tmp = NamedTempFile::new().expect("create tmp file");
        writeln!(tmp, "{{}}").expect("write tmp file");
        let path = tmp.path().to_str().expect("utf8 path");

        let config = Mapping::from_iter([
            ("method".into(), "service-account".into()),
            ("database".into(), "my_db".into()),
            ("schema".into(), "my_schema".into()),
            ("keyfile".into(), path.into()),
        ]);

        let builder = try_configure(config).expect("configure");
        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_TYPE).unwrap(),
            auth_type::JSON_CREDENTIAL_FILE
        );
        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_CREDENTIALS).unwrap(),
            path
        );
    }

    #[test]
    fn test_builder_from_auth_config_oauth_secrets_refresh_token() {
        let yaml_doc = r#"
method: oauth-secrets
database: my_db
schema: my_schema
refresh_token: token
client_id: id
client_secret: secret
token_uri: https://oauth2.googleapis.com/token
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let builder = try_configure(config).unwrap();

        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_TYPE).unwrap(),
            auth_type::USER_AUTHENTICATION
        );
        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_CLIENT_ID).unwrap(),
            "id"
        );
        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_CLIENT_SECRET).unwrap(),
            "secret"
        );
        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_REFRESH_TOKEN).unwrap(),
            "token"
        );
        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_ACCESS_TOKEN_ENDPOINT).unwrap(),
            "https://oauth2.googleapis.com/token"
        );
    }

    #[test]
    fn test_auth_config_missing_method_errors() {
        let config = Mapping::from_iter([
            ("database".into(), "my_db".into()),
            ("schema".into(), "my_schema".into()),
        ]);
        let err = try_configure(config).unwrap_err();
        assert_contains!(
            err.msg(),
            "Missing required 'method' field in BigQuery config"
        );
    }

    #[test]
    fn test_auth_config_dataset_and_schema_conflict_errors() {
        let config = Mapping::from_iter([
            ("method".into(), "oauth".into()),
            ("database".into(), "my_db".into()),
            ("dataset".into(), "my_dataset".into()),
            ("schema".into(), "my_schema".into()),
        ]);
        let err = try_configure(config).unwrap_err();
        assert_contains!(err.msg(), "Don't specify both 'dataset' and 'schema'");
    }

    #[test]
    fn test_auth_config_project_and_database_conflict_errors() {
        let config = Mapping::from_iter([
            ("method".into(), "oauth".into()),
            ("project".into(), "my_project".into()),
            ("database".into(), "my_db".into()),
            ("schema".into(), "my_schema".into()),
        ]);
        let err = try_configure(config).unwrap_err();
        assert_contains!(
            err.msg(),
            "Don't specify 'database' when 'project' is specified"
        );
    }

    const WIF_PROVIDER: &str = "//iam.googleapis.com/projects/123/locations/global/workloadIdentityPools/pool/providers/prov";

    #[test]
    fn test_builder_from_auth_config_external_oauth_wif() {
        let yaml_doc = r#"
method: external-oauth-wif
database: my_db
schema: my_schema
workload_pool_provider_path: //iam.googleapis.com/projects/123/locations/global/workloadIdentityPools/pool/providers/prov
token_endpoint:
    type: entra
    request_url: https://login.microsoftonline.com/tenant/oauth2/v2.0/token
    request_data: "grant_type=client_credentials&client_id=abc&client_secret=secret&scope=https://example/.default"
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let builder = try_configure(config).unwrap();

        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_TYPE).unwrap(),
            auth_type::EXTERNAL_ACCOUNT
        );
        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_EXTERNAL_ACCOUNT_AUDIENCE).unwrap(),
            WIF_PROVIDER
        );
        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_EXTERNAL_ACCOUNT_REQUEST_URL).unwrap(),
            "https://login.microsoftonline.com/tenant/oauth2/v2.0/token"
        );
        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_EXTERNAL_ACCOUNT_REQUEST_DATA).unwrap(),
            "grant_type=client_credentials&client_id=abc&client_secret=secret&scope=https://example/.default"
        );
        assert!(other_option_value(&builder, bigquery::AUTH_CREDENTIALS).is_none());
        assert!(
            other_option_value(&builder, bigquery::AUTH_EXTERNAL_ACCOUNT_IMPERSONATION_URL)
                .is_none()
        );
    }

    #[test]
    fn test_builder_from_auth_config_external_oauth_wif_impersonation() {
        let yaml_doc = r#"
method: external-oauth-wif
database: my_db
schema: my_schema
workload_pool_provider_path: //iam.googleapis.com/projects/123/locations/global/workloadIdentityPools/pool/providers/prov
service_account_impersonation_url: https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/sa@p.iam.gserviceaccount.com:generateAccessToken
token_endpoint:
    type: entra
    request_url: https://login.microsoftonline.com/tenant/oauth2/v2.0/token
    request_data: "grant_type=client_credentials"
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let builder = try_configure(config).unwrap();
        assert_eq!(
            other_option_value(&builder, bigquery::AUTH_EXTERNAL_ACCOUNT_IMPERSONATION_URL)
                .unwrap(),
            "https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/sa@p.iam.gserviceaccount.com:generateAccessToken"
        );
    }

    #[test]
    fn test_external_oauth_wif_missing_type_errors() {
        let yaml_doc = r#"
method: external-oauth-wif
database: my_db
schema: my_schema
workload_pool_provider_path: //iam.googleapis.com/x
token_endpoint:
    request_url: https://idp/token
    request_data: "grant_type=client_credentials"
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let err = try_configure(config).unwrap_err();
        assert_contains!(err.msg(), "Missing required key in token_endpoint: 'type'");
    }

    #[test]
    fn test_external_oauth_wif_unsupported_type_errors() {
        let yaml_doc = r#"
method: external-oauth-wif
database: my_db
schema: my_schema
workload_pool_provider_path: //iam.googleapis.com/x
token_endpoint:
    type: github
    request_url: https://idp/token
    request_data: "grant_type=client_credentials"
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let err = try_configure(config).unwrap_err();
        assert_contains!(err.msg(), "Unsupported identity provider type: github");
        assert_contains!(err.msg(), "entra");
    }

    #[test]
    fn test_external_oauth_wif_missing_request_url_errors() {
        let yaml_doc = r#"
method: external-oauth-wif
database: my_db
schema: my_schema
workload_pool_provider_path: //iam.googleapis.com/x
token_endpoint:
    type: entra
    request_data: "grant_type=client_credentials"
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let err = try_configure(config).unwrap_err();
        assert_contains!(
            err.msg(),
            "Missing required key in token_endpoint: 'request_url'"
        );
    }

    #[test]
    fn test_external_oauth_wif_missing_request_data_errors() {
        let yaml_doc = r#"
method: external-oauth-wif
database: my_db
schema: my_schema
workload_pool_provider_path: //iam.googleapis.com/x
token_endpoint:
    type: entra
    request_url: https://idp/token
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let err = try_configure(config).unwrap_err();
        assert_contains!(
            err.msg(),
            "Missing required key in token_endpoint: 'request_data'"
        );
    }

    #[test]
    fn test_external_oauth_wif_missing_provider_path_errors() {
        let yaml_doc = r#"
method: external-oauth-wif
database: my_db
schema: my_schema
token_endpoint:
    type: entra
    request_url: https://idp/token
    request_data: "grant_type=client_credentials"
"#;
        let config = dbt_yaml::from_str::<Mapping>(yaml_doc).unwrap();
        let err = try_configure(config).unwrap_err();
        assert_contains!(err.msg(), "workload_pool_provider_path");
    }
}
