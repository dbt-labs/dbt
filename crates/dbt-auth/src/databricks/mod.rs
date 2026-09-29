use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter};
use database::Builder as DatabaseBuilder;
use dbt_yaml::Value;
use std::borrow::Cow;
use std::time::Duration;

use dbt_adbc::{Backend, database, databricks};

/// User agent name provided to dbx for Fusion.
///
/// Official guidance is <isv-name+product-name> but dbt Core provides 'dbt' only
/// and we follow suit.
///
/// Ref: https://github.com/databricks/databricks-sql-go/blob/56b8a73b09908454e3070fe513ff2563c85ba214/connector.go#L214
const USER_AGENT_NAME: &str = "dbt";
const QUERY_TAG_OPTION_PREFIX: &str = "databricks.query_tag.";
const DBT_CORE_VERSION: &str = "@@dbt_core_version";
const DBT_MODEL_NAME: &str = "@@dbt_model_name";
const DBT_MATERIALIZED: &str = "@@dbt_materialized";
const RESERVED_QUERY_TAG_KEYS: [&str; 3] = [DBT_CORE_VERSION, DBT_MODEL_NAME, DBT_MATERIALIZED];

/// Supported Databricks authentication types.
/// When `auth_type` is absent, defaults to token-based (PAT) authentication.
/// When `auth_type` is present, only `oauth` is a valid value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DatabricksAuthType {
    /// OAuth authentication
    OAuth,
    /// Personal Access Token
    Token,
}

impl DatabricksAuthType {
    /// Parse auth_type from config.
    /// - Absent or "token": defaults to token-based authentication
    /// - "oauth": OAuth authentication
    /// - Any other value: error
    fn from_config(config: &AdapterConfig) -> Result<Self, AuthError> {
        match config.get_string("auth_type") {
            Some(s) if s.eq_ignore_ascii_case("oauth") => Ok(DatabricksAuthType::OAuth),
            Some(s) if s.eq_ignore_ascii_case("token") => Ok(DatabricksAuthType::Token),
            Some(invalid) => Err(AuthError::config(format!(
                "Invalid auth_type '{}'. Valid values are: 'oauth', 'token'.",
                invalid
            ))),
            None => Ok(DatabricksAuthType::Token),
        }
    }
}

#[derive(Debug)]
enum DatabricksAuthIR<'a> {
    OAuthM2M {
        client_id: &'a str,
        client_secret: &'a str,
    },
    ExternalBrowserOAuth {
        client_id: Option<&'a str>,
    },
    // Token is minted against Azure AD, not the Databricks OIDC endpoint (cf. OAuthM2M).
    AzureClientSecret {
        azure_client_id: &'a str,
        azure_client_secret: &'a str,
    },
    Token {
        token: &'a str,
    },
}

impl<'a> DatabricksAuthIR<'a> {
    pub fn apply(
        self,
        mut builder: DatabaseBuilder,
        _warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<DatabaseBuilder, AuthError> {
        match self {
            Self::OAuthM2M {
                client_id,
                client_secret,
            } => {
                builder.with_named_option(databricks::CLIENT_ID, client_id)?;
                builder.with_named_option(databricks::CLIENT_SECRET, client_secret)?;
                builder
                    .with_named_option(databricks::AUTH_TYPE, databricks::auth_type::OAUTH_M2M)?;
            }
            Self::ExternalBrowserOAuth { client_id } => {
                if let Some(client_id) = client_id {
                    builder.with_named_option(databricks::CLIENT_ID, client_id)?;
                }
                builder.with_named_option(
                    databricks::AUTH_TYPE,
                    databricks::auth_type::EXTERNAL_BROWSER,
                )?;
            }
            Self::AzureClientSecret {
                azure_client_id,
                azure_client_secret,
            } => {
                builder.with_named_option(databricks::AZURE_CLIENT_ID, azure_client_id)?;
                builder.with_named_option(databricks::AZURE_CLIENT_SECRET, azure_client_secret)?;
                builder.with_named_option(
                    databricks::AUTH_TYPE,
                    databricks::auth_type::AZURE_CLIENT_SECRET,
                )?;
            }
            Self::Token { token } => {
                builder.with_named_option(databricks::TOKEN, token)?;
                builder.with_named_option(databricks::AUTH_TYPE, databricks::auth_type::PAT)?;
            }
        }

        Ok(builder)
    }
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabricksAuthIR<'a>, AuthError> {
    // FIXME: dbt-databricks historically has allowed garbage in the auth_type field and only responds to
    // auth_type 'oauth'. Everything else means token
    match DatabricksAuthType::from_config(config) {
        Ok(DatabricksAuthType::OAuth) => {
            // Token-first: if a preminted bearer token is in the payload, use it as
            // PAT regardless of which OAuth-app fields accompany it. This matches
            // dbt-databricks' `_ensure_config`, which does `if self.token:` — a
            // truthy check that treats empty/null tokens as absent and falls
            // through to M2M / external-browser.
            // https://github.com/databricks/dbt-databricks/blob/c1c74df4bc01e155dabcc07f23a5a414e04aad62/dbt/adapters/databricks/credentials.py#L360-L361
            //
            // dbt Studio's U2M flow completes the OAuth handshake on the cloud
            // side and forwards `auth_type=oauth` together with the OAuth-app
            // `client_id`/`client_secret` (which are *not* a Databricks
            // service-principal credential) and the resulting access token.
            // Without this short-circuit we would attempt an M2M
            // client-credentials grant against Databricks with the cloud-app
            // secret, which fails with `invalid_client`.
            // https://github.com/dbt-labs/dbt-cloud/blob/228283facb9103a2053d83e5b085f6a7b771e686/sinter/services/profile/util/adapters/adapter_profile_helper.py#L189-L190
            //
            // The value must be non-empty: dbt Cloud's Databricks credentials
            // schema defaults `token` to `""` when a customer selects OAuth or
            // relies on extended attributes to inject the real token. Treating
            // that empty placeholder as a real PAT would send an empty
            // `databricks.access_token` to the ADBC driver, which rejects it
            // with "access token is required when using auth type 'pat'".
            // https://app.notion.com/p/dbtlabs/Databricks-OAuth-for-Deployment-Environments-22bbb38ebda780dc9608ef05cfd757ff?source=copy_link
            if let Some(token) = config.get_str("token").filter(|s| !s.is_empty()) {
                Ok(DatabricksAuthIR::Token { token })
            } else if config.contains_key("azure_client_secret") {
                Ok(DatabricksAuthIR::AzureClientSecret {
                    azure_client_id: config.require_str("azure_client_id")?,
                    azure_client_secret: config.require_str("azure_client_secret")?,
                })
            } else if config.contains_key("client_secret") {
                Ok(DatabricksAuthIR::OAuthM2M {
                    client_id: config.require_str("client_id")?,
                    client_secret: config.require_str("client_secret")?,
                })
            } else {
                Ok(DatabricksAuthIR::ExternalBrowserOAuth {
                    client_id: if config.contains_key("client_id") {
                        Some(config.require_str("client_id")?)
                    } else {
                        None
                    },
                })
            }
        }
        Ok(DatabricksAuthType::Token) | Err(_) => Ok(DatabricksAuthIR::Token {
            token: config.require_str("token")?,
        }),
    }
}

fn apply_connection_args(
    config: &AdapterConfig,
    mut builder: DatabaseBuilder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabaseBuilder, AuthError> {
    let http_path = resolve_http_path(config)?;

    validate_config(config)?;

    // all of the following options are required for any Databricks connection
    builder.with_named_option(databricks::USER_AGENT, USER_AGENT_NAME)?;
    builder.with_named_option(databricks::HOST, config.require_string("host")?)?;
    builder.with_named_option(databricks::SCHEMA, config.require_string("schema")?)?;
    builder.with_named_option(databricks::CATALOG, config.require_string("database")?)?;
    builder.with_named_option(databricks::HTTP_PATH, http_path)?;
    apply_query_tag_defaults(config, &mut builder)?;

    // `connect_timeout` (seconds) is the documented Databricks connection timeout. Pass it to the
    // driver as a real connection-establishment deadline so a cold-starting warehouse has time to
    // come up instead of failing once the driver's fixed retry budget is exhausted. Formatted as a
    // Go duration string the driver parses (e.g. "600s").
    if let Some(secs) = config.get("connect_timeout").and_then(|v| v.as_i64()) {
        if secs <= 0 {
            return Err(AuthError::config(format!(
                "connect_timeout must be a positive number of seconds, got {secs}"
            )));
        }
        builder.with_named_option(databricks::CONNECT_TIMEOUT, format!("{secs}s"))?;
    }

    // Azure SP: the tenant is a connection detail, not an auth credential, so it lives here
    // rather than in the auth IR. Resolve it (explicit `azure_tenant_id`, else discover from
    // the workspace) and pass it to the driver, which requires it.
    if config.contains_key("azure_client_secret") {
        let tenant_id = match config.get_str("azure_tenant_id") {
            Some(tenant_id) => tenant_id.to_string(),
            None => discover_azure_tenant_id(config.require_string("host")?.as_ref())?,
        };
        builder.with_named_option(databricks::AZURE_TENANT_ID, tenant_id)?;
    }

    Ok(builder)
}

fn apply_query_tag_defaults(
    config: &AdapterConfig,
    builder: &mut DatabaseBuilder,
) -> Result<(), AuthError> {
    builder.with_named_option(
        format!("{QUERY_TAG_OPTION_PREFIX}{DBT_CORE_VERSION}"),
        env!("CARGO_PKG_VERSION"),
    )?;

    let Some(value) = config.get("query_tags") else {
        return Ok(());
    };
    match value {
        Value::Null(_) => Ok(()),
        Value::String(query_tags, _) if query_tags.is_empty() => Ok(()),
        Value::String(query_tags, _) => {
            let query_tags: dbt_yaml::Mapping =
                serde_json::from_str(query_tags).map_err(|error| {
                    if error.classify() == serde_json::error::Category::Data {
                        AuthError::config("query_tags must be a JSON object (dictionary)")
                    } else {
                        AuthError::config(format!("Invalid JSON in query_tags: {error}"))
                    }
                })?;
            apply_query_tag_mapping(&query_tags, builder)
        }
        Value::Mapping(query_tags, _) => apply_query_tag_mapping(query_tags, builder),
        _ => Err(AuthError::config(
            "query_tags must be a JSON object (dictionary)",
        )),
    }
}

fn apply_query_tag_mapping(
    query_tags: &dbt_yaml::Mapping,
    builder: &mut DatabaseBuilder,
) -> Result<(), AuthError> {
    let mut reserved = query_tags
        .keys()
        .filter_map(Value::as_str)
        .filter(|key| RESERVED_QUERY_TAG_KEYS.contains(key))
        .collect::<Vec<_>>();
    reserved.sort_unstable();
    if !reserved.is_empty() {
        return Err(AuthError::config(format!(
            "Connection config: Cannot use reserved query tag keys: {}. Reserved keys are: {}",
            reserved.join(", "),
            RESERVED_QUERY_TAG_KEYS.join(", ")
        )));
    }

    for (key, value) in query_tags {
        let key = key.as_str().ok_or_else(|| {
            AuthError::config("query_tags keys must be strings. Only string keys are supported.")
        })?;
        let value = value.as_str().ok_or_else(|| {
            AuthError::config(format!(
                "Connection config: query_tags values must be strings for key '{key}'. Only string values are supported."
            ))
        })?;
        builder.with_named_option(format!("{QUERY_TAG_OPTION_PREFIX}{key}"), value.to_owned())?;
    }

    Ok(())
}

/// Resolve the Microsoft Entra ID tenant for an Azure Databricks workspace from the
/// unauthenticated `<host>/aad/auth` redirect. Mirrors databricks-sdk-py's public
/// `load_azure_tenant_id`; the Go SDK's equivalent is unexported and its Azure
/// client-secret credentials won't activate without a tenant, so we resolve it here
/// (in dbt-auth) rather than depend on the driver. Used only when `azure_tenant_id`
/// is not supplied explicitly.
fn discover_azure_tenant_id(host: &str) -> Result<String, AuthError> {
    let login_url = format!("https://{host}/aad/auth");
    // The tenant is in the 3xx Location header; do not follow the redirect, and
    // treat a 3xx as a normal response rather than an error.
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let response = agent.get(&login_url).call().map_err(|e| {
        AuthError::config(format!(
            "azure tenant discovery request to {login_url} failed \
             (set 'azure_tenant_id' explicitly): {e}"
        ))
    })?;
    let location = response
        .headers()
        .get(ureq::http::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            AuthError::config(format!(
                "could not resolve azure tenant id from {login_url}; \
                 set 'azure_tenant_id' explicitly"
            ))
        })?;
    parse_azure_tenant_from_location(location)
}

/// Extract the tenant id from an Entra ID authorize URL of the form
/// `https://login.microsoftonline.com/<tenant-id>/oauth2/authorize?...` (the login
/// domain varies by Azure cloud, e.g. `login.microsoftonline.us`).
fn parse_azure_tenant_from_location(location: &str) -> Result<String, AuthError> {
    let url = url::Url::parse(location)
        .map_err(|e| AuthError::config(format!("could not parse Location '{location}': {e}")))?;
    url.path_segments()
        .and_then(|mut segments| segments.next())
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.to_string())
        .ok_or_else(|| AuthError::config(format!("could not extract tenant id from '{location}'")))
}

pub struct DatabricksAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl DatabricksAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for DatabricksAuth {
    fn backend(&self) -> Backend {
        Backend::Databricks
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        crate::auth_configure_pipeline!(self, &config, parse_auth, apply_connection_args)
    }
}

fn resolve_http_path(config: &AdapterConfig) -> Result<Cow<'_, str>, AuthError> {
    let mut http_path = config.require_string("http_path")?;
    let databricks_compute_config = config.get_string("databricks_compute");

    if let Some(databricks_compute) = databricks_compute_config {
        let compute = config.require("compute")?;

        if let Value::Mapping(map, ..) = compute
            && let Some((_, Value::Mapping(compute_map, ..))) = map
                .iter()
                .find(|(k, _)| matches!(k, Value::String(s, _) if *s == databricks_compute))
            && let Some(Value::String(path, _)) = compute_map.iter().find_map(|(k, v)| {
                if let Value::String(key, _) = k
                    && key == "http_path"
                {
                    return Some(v);
                }
                None
            })
        {
            http_path = Cow::from(path);
            return Ok(http_path);
        }

        return Err(AuthError::Config(format!(
            "Compute resource '{databricks_compute}' does not exist or does not specify http_path"
        )));
    }

    Ok(http_path)
}

fn validate_config(config: &AdapterConfig) -> Result<(), AuthError> {
    if !config.contains_key("http_path") {
        return Err(AuthError::config("http_path is required"));
    }
    if !config.contains_key("host") {
        return Err(AuthError::config("host is required".to_string()));
    }
    // FIXME: auth_type validation is lenient - unknown values default to token auth
    let _ = DatabricksAuthType::from_config(config);
    if !config.contains_key("client_id") && config.contains_key("client_secret") {
        return Err(AuthError::config(
            "The config 'client_id' is required to connect to Databricks when 'client_secret' is present",
        ));
    }
    let azure_client_no_secret =
        !config.contains_key("azure_client_id") && config.contains_key("azure_client_secret");
    let azure_secret_no_client =
        config.contains_key("azure_client_id") && !config.contains_key("azure_client_secret");
    if azure_client_no_secret || azure_secret_no_client {
        return Err(AuthError::config(
            "The config 'azure_client_id' and 'azure_client_secret' must be both present or both absent",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_options::other_option_value;
    use dbt_yaml::Mapping;
    use dbt_yaml::Value as YmlValue;

    fn base_config() -> Mapping {
        Mapping::from_iter([
            ("host".into(), "H".into()),
            ("schema".into(), "S".into()),
            ("database".into(), "C".into()),
            (
                "http_path".into(),
                "/sql/1.0/warehouses/warehouse-id".into(),
            ),
        ])
    }

    fn run_config_test(config: Mapping, expected: &[(&str, &str)]) -> Result<(), AuthError> {
        let auth = DatabricksAuth::new(Box::new(crate::NoopAuthWarningPrinter));
        let builder = auth.configure(&AdapterConfig::new(config))?;
        assert_eq!(builder.clone().into_iter().count(), expected.len() + 1);
        assert_eq!(
            other_option_value(&builder, "databricks.query_tag.@@dbt_core_version"),
            Some(env!("CARGO_PKG_VERSION"))
        );

        for &(key, expected_val) in expected {
            assert_eq!(
                other_option_value(&builder, key).unwrap_or_else(|| panic!("Missing key: {key}")),
                expected_val,
                "Value mismatch for key: {key}"
            );
        }
        Ok(())
    }

    #[test]
    fn test_query_tags_string_installs_ordered_database_defaults() {
        let mut config = base_config();
        config.insert("token".into(), "T".into());
        config.insert(
            "query_tags".into(),
            r#"{"z_team":"analytics","a_cost_center":"3000"}"#.into(),
        );

        let builder = DatabricksAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .unwrap();
        let query_tag_options = builder
            .other
            .iter()
            .filter_map(|(option, _)| match option {
                adbc_core::options::OptionDatabase::Other(name)
                    if name.starts_with("databricks.query_tag.") =>
                {
                    Some(name.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            query_tag_options,
            [
                "databricks.query_tag.@@dbt_core_version",
                "databricks.query_tag.z_team",
                "databricks.query_tag.a_cost_center",
            ]
        );
        assert_eq!(
            other_option_value(&builder, "databricks.query_tag.z_team"),
            Some("analytics")
        );
    }

    #[test]
    fn test_query_tags_mapping_installs_database_defaults_without_coercion() {
        let mut config = base_config();
        config.insert("token".into(), "T".into());
        config.insert(
            "query_tags".into(),
            YmlValue::mapping(Mapping::from_iter([
                ("team".into(), "analytics".into()),
                ("cost_center".into(), "3000".into()),
            ])),
        );

        let builder = DatabricksAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .unwrap();
        let query_tag_options = builder
            .other
            .iter()
            .filter_map(|(option, _)| match option {
                adbc_core::options::OptionDatabase::Other(name)
                    if name.starts_with(QUERY_TAG_OPTION_PREFIX) =>
                {
                    Some(name.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            query_tag_options,
            [
                "databricks.query_tag.@@dbt_core_version",
                "databricks.query_tag.team",
                "databricks.query_tag.cost_center",
            ]
        );
        assert_eq!(
            other_option_value(&builder, "databricks.query_tag.team"),
            Some("analytics")
        );
        assert_eq!(
            other_option_value(&builder, "databricks.query_tag.cost_center"),
            Some("3000")
        );
    }

    #[test]
    fn test_query_tags_null_and_empty_profiles_install_only_core_default() {
        for query_tags in [
            YmlValue::null(),
            YmlValue::string(String::new()),
            YmlValue::mapping(Mapping::new()),
        ] {
            let mut config = base_config();
            config.insert("token".into(), "T".into());
            config.insert("query_tags".into(), query_tags);

            let builder = DatabricksAuth::new(Box::new(crate::NoopAuthWarningPrinter))
                .configure(&AdapterConfig::new(config))
                .unwrap();
            let query_tag_options = builder
                .other
                .iter()
                .filter_map(|(option, _)| match option {
                    adbc_core::options::OptionDatabase::Other(name)
                        if name.starts_with(QUERY_TAG_OPTION_PREFIX) =>
                    {
                        Some(name.as_str())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();

            assert_eq!(
                query_tag_options,
                ["databricks.query_tag.@@dbt_core_version"]
            );
        }
    }

    #[test]
    fn test_query_tags_database_defaults_reject_invalid_profile_values() {
        let non_string_key: YmlValue = dbt_yaml::from_str("3000: analytics").unwrap();
        for (query_tags, expected) in [
            (
                YmlValue::sequence(vec!["not".into(), "an-object".into()]),
                "query_tags must be a JSON object (dictionary)",
            ),
            (
                YmlValue::mapping(Mapping::from_iter([(
                    "cost_center".into(),
                    YmlValue::number(3000.into()),
                )])),
                "query_tags values must be strings for key 'cost_center'",
            ),
            (
                YmlValue::mapping(Mapping::from_iter([(
                    "@@dbt_model_name".into(),
                    "override".into(),
                )])),
                "Cannot use reserved query tag keys: @@dbt_model_name",
            ),
            (
                non_string_key,
                "query_tags keys must be strings. Only string keys are supported.",
            ),
        ] {
            let mut config = base_config();
            config.insert("token".into(), "T".into());
            config.insert("query_tags".into(), query_tags);

            let error = DatabricksAuth::new(Box::new(crate::NoopAuthWarningPrinter))
                .configure(&AdapterConfig::new(config))
                .unwrap_err();
            assert!(error.msg().contains(expected), "{}", error.msg());
        }
    }

    #[test]
    fn test_query_tags_database_defaults_reject_invalid_json_strings() {
        for (query_tags, expected) in [
            (
                YmlValue::string(r#"["not","an","object"]"#.to_owned()),
                "query_tags must be a JSON object (dictionary)",
            ),
            (
                YmlValue::string(r#"{"invalid": json}"#.to_owned()),
                "Invalid JSON in query_tags",
            ),
        ] {
            let mut config = base_config();
            config.insert("token".into(), "T".into());
            config.insert("query_tags".into(), query_tags);

            let error = DatabricksAuth::new(Box::new(crate::NoopAuthWarningPrinter))
                .configure(&AdapterConfig::new(config))
                .unwrap_err();
            assert!(error.msg().contains(expected), "{}", error.msg());
        }
    }

    #[test]
    fn test_token_warehouse() {
        let mut config = base_config();
        config.insert("token".into(), "T".into());

        let expected = vec![
            (databricks::TOKEN, "T"),
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (databricks::HTTP_PATH, "/sql/1.0/warehouses/warehouse-id"),
            (databricks::CATALOG, "C"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (databricks::AUTH_TYPE, databricks::auth_type::PAT),
        ];
        run_config_test(config, &expected).unwrap();
    }

    #[test]
    fn test_connect_timeout_maps_to_driver_option() {
        // `connect_timeout` (seconds) becomes the driver connect-timeout option as a Go duration.
        let mut config = base_config();
        config.insert("token".into(), "T".into());
        config.insert("connect_timeout".into(), YmlValue::from(600i64));

        let expected = vec![
            (databricks::TOKEN, "T"),
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (databricks::HTTP_PATH, "/sql/1.0/warehouses/warehouse-id"),
            (databricks::CATALOG, "C"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (databricks::CONNECT_TIMEOUT, "600s"),
            (databricks::AUTH_TYPE, databricks::auth_type::PAT),
        ];
        run_config_test(config, &expected).unwrap();
    }

    #[test]
    fn test_connect_timeout_absent_sets_no_option() {
        // No `connect_timeout` in the profile => no connect-timeout option is emitted.
        let mut config = base_config();
        config.insert("token".into(), "T".into());

        let expected = vec![
            (databricks::TOKEN, "T"),
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (databricks::HTTP_PATH, "/sql/1.0/warehouses/warehouse-id"),
            (databricks::CATALOG, "C"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (databricks::AUTH_TYPE, databricks::auth_type::PAT),
        ];
        // run_config_test asserts the option count matches exactly, so CONNECT_TIMEOUT being
        // absent from `expected` also proves the option was not set.
        run_config_test(config, &expected).unwrap();
    }

    #[test]
    fn test_connect_timeout_zero_or_negative_errors() {
        for bad in [0i64, -5i64] {
            let mut config = base_config();
            config.insert("token".into(), "T".into());
            config.insert("connect_timeout".into(), YmlValue::from(bad));

            match run_config_test(config, &[]) {
                Err(AuthError::Config(_)) => {}
                other => {
                    panic!("expected AuthError::Config for connect_timeout={bad}, got {other:?}")
                }
            }
        }
    }

    #[test]
    fn test_oauth_fields_do_not_enable_oauth_without_auth_type() {
        // Without auth_type: oauth, we should still default to token auth even if oauth fields exist.
        let mut config = base_config();
        config.insert("token".into(), "T".into());
        config.insert("client_id".into(), "O".into());
        config.insert("client_secret".into(), "O".into());

        let expected = vec![
            (databricks::TOKEN, "T"),
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (databricks::HTTP_PATH, "/sql/1.0/warehouses/warehouse-id"),
            (databricks::CATALOG, "C"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (databricks::AUTH_TYPE, databricks::auth_type::PAT),
        ];
        run_config_test(config, &expected).unwrap();
    }

    #[test]
    fn test_token_cluster_with_optional_fields() {
        let config = Mapping::from_iter([
            ("host".into(), "H".into()),
            ("schema".into(), "S".into()),
            (
                "http_path".into(),
                "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id".into(),
            ),
            ("token".into(), "T".into()),
            ("database".into(), "C".into()),
        ]);

        let expected = vec![
            (databricks::TOKEN, "T"),
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (
                databricks::HTTP_PATH,
                "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id",
            ),
            (databricks::CATALOG, "C"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (databricks::AUTH_TYPE, databricks::auth_type::PAT),
        ];
        run_config_test(config, &expected).unwrap();
    }

    #[test]
    fn test_m2m_oauth() {
        let mut config = base_config();
        config.insert(
            "http_path".into(),
            "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id".into(),
        );
        config.insert("client_id".into(), "O".into());
        config.insert("client_secret".into(), "O".into());
        config.insert("auth_type".into(), "oauth".into());

        let expected = vec![
            (databricks::CLIENT_ID, "O"),
            (databricks::CLIENT_SECRET, "O"),
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (
                databricks::HTTP_PATH,
                "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id",
            ),
            (databricks::CATALOG, "C"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (databricks::AUTH_TYPE, databricks::auth_type::OAUTH_M2M),
        ];
        run_config_test(config, &expected).unwrap();
    }

    /// dbt Studio's U2M payload: `auth_type=oauth` plus the OAuth-app
    /// `client_id`/`client_secret` (used by Studio to mint the token, *not* a
    /// service-principal credential) plus the resulting preminted access token.
    /// The preminted token must short-circuit to PAT — attempting M2M with the
    /// cloud-app secret fails with `invalid_client`. Matches dbt-databricks'
    /// token-first dispatch in `_ensure_config`. Regression test for
    /// dbt-core#14588.
    #[test]
    fn test_studio_u2m_preminted_token_routes_to_pat() {
        let mut config = base_config();
        config.insert(
            "http_path".into(),
            "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id".into(),
        );
        config.insert("client_id".into(), "CLOUD_APP_CLIENT_ID".into());
        config.insert("client_secret".into(), "CLOUD_APP_CLIENT_SECRET".into());
        config.insert("auth_type".into(), "oauth".into());
        config.insert("token".into(), "PREMINTED_U2M_TOKEN".into());

        let expected = vec![
            (databricks::TOKEN, "PREMINTED_U2M_TOKEN"),
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (
                databricks::HTTP_PATH,
                "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id",
            ),
            (databricks::CATALOG, "C"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (databricks::AUTH_TYPE, databricks::auth_type::PAT),
        ];
        run_config_test(config, &expected).unwrap();
    }

    #[test]
    fn test_external_browser_oauth() {
        let mut config = base_config();
        config.insert(
            "http_path".into(),
            "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id".into(),
        );
        config.insert("client_id".into(), "O".into());
        config.insert("auth_type".into(), "oauth".into());
        let expected = vec![
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (
                databricks::HTTP_PATH,
                "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id",
            ),
            (databricks::CATALOG, "C"),
            (databricks::CLIENT_ID, "O"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (
                databricks::AUTH_TYPE,
                databricks::auth_type::EXTERNAL_BROWSER,
            ),
        ];
        run_config_test(config, &expected).unwrap();
    }

    /// Azure service principal (Microsoft Entra ID) via `azure_client_id`/
    /// `azure_client_secret` routes to `azure-client-secret` (distinct from M2M), and the
    /// tenant — a connection param, not an auth-IR field — is forwarded to the driver.
    /// Regression test for dbt-core#13986. (The no-tenant path resolves via `/aad/auth`
    /// discovery, which is a network call and is covered by `parse_azure_tenant_from_location`
    /// + live testing rather than here.)
    #[test]
    fn test_azure_client_secret_with_tenant() {
        let mut config = base_config();
        config.insert("auth_type".into(), "oauth".into());
        config.insert("azure_client_id".into(), "AID".into());
        config.insert("azure_client_secret".into(), "ASECRET".into());
        config.insert("azure_tenant_id".into(), "TENANT".into());

        let expected = vec![
            (databricks::AZURE_CLIENT_ID, "AID"),
            (databricks::AZURE_CLIENT_SECRET, "ASECRET"),
            (databricks::AZURE_TENANT_ID, "TENANT"),
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (databricks::HTTP_PATH, "/sql/1.0/warehouses/warehouse-id"),
            (databricks::CATALOG, "C"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (
                databricks::AUTH_TYPE,
                databricks::auth_type::AZURE_CLIENT_SECRET,
            ),
        ];
        run_config_test(config, &expected).unwrap();
    }

    /// Azure SP fields take priority over the Databricks M2M `client_id`/
    /// `client_secret` when both are present, matching dbt-databricks'
    /// `_ensure_config` (azure branch precedes the oauth-m2m branch).
    #[test]
    fn test_azure_client_secret_takes_priority_over_m2m() {
        let mut config = base_config();
        config.insert("auth_type".into(), "oauth".into());
        config.insert("azure_client_id".into(), "AID".into());
        config.insert("azure_client_secret".into(), "ASECRET".into());
        config.insert("azure_tenant_id".into(), "TENANT".into());
        config.insert("client_id".into(), "OID".into());
        config.insert("client_secret".into(), "OSECRET".into());

        let expected = vec![
            (databricks::AZURE_CLIENT_ID, "AID"),
            (databricks::AZURE_CLIENT_SECRET, "ASECRET"),
            (databricks::AZURE_TENANT_ID, "TENANT"),
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (databricks::HTTP_PATH, "/sql/1.0/warehouses/warehouse-id"),
            (databricks::CATALOG, "C"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (
                databricks::AUTH_TYPE,
                databricks::auth_type::AZURE_CLIENT_SECRET,
            ),
        ];
        run_config_test(config, &expected).unwrap();
    }

    #[test]
    fn test_external_browser_oauth_without_client_id() {
        let mut config = base_config();
        config.insert(
            "http_path".into(),
            "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id".into(),
        );
        config.insert("auth_type".into(), "oauth".into());
        let expected = vec![
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (
                databricks::HTTP_PATH,
                "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id",
            ),
            (databricks::CATALOG, "C"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (
                databricks::AUTH_TYPE,
                databricks::auth_type::EXTERNAL_BROWSER,
            ),
        ];
        run_config_test(config, &expected).unwrap();
    }

    /// Regression: `auth_type=oauth` + valid M2M `client_id`/`client_secret` +
    /// an *empty* `token` placeholder (e.g. dbt Cloud's default value or an
    /// unfilled extended-attribute override) must still dispatch to OAuth M2M.
    /// Treating the empty string as a real PAT would forward an empty
    /// `databricks.access_token` to the ADBC driver, which rejects it with
    /// "access token is required when using auth type 'pat'". Matches Python
    /// `_ensure_config`'s `if self.token:` truthy check.
    #[test]
    fn test_oauth_m2m_with_empty_token_placeholder_routes_to_m2m() {
        let mut config = base_config();
        config.insert(
            "http_path".into(),
            "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id".into(),
        );
        config.insert("client_id".into(), "M2M_CLIENT_ID".into());
        config.insert("client_secret".into(), "M2M_CLIENT_SECRET".into());
        config.insert("auth_type".into(), "oauth".into());
        config.insert("token".into(), "".into());

        let expected = vec![
            (databricks::CLIENT_ID, "M2M_CLIENT_ID"),
            (databricks::CLIENT_SECRET, "M2M_CLIENT_SECRET"),
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (
                databricks::HTTP_PATH,
                "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id",
            ),
            (databricks::CATALOG, "C"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (databricks::AUTH_TYPE, databricks::auth_type::OAUTH_M2M),
        ];
        run_config_test(config, &expected).unwrap();
    }

    /// Companion to the M2M empty-token regression: with `auth_type=oauth`,
    /// only `client_id`, and an empty `token` placeholder, we should fall
    /// through to the external-browser branch instead of trying PAT with an
    /// empty token.
    #[test]
    fn test_oauth_external_browser_with_empty_token_placeholder_routes_to_browser() {
        let mut config = base_config();
        config.insert(
            "http_path".into(),
            "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id".into(),
        );
        config.insert("client_id".into(), "CLIENT_ID".into());
        config.insert("auth_type".into(), "oauth".into());
        config.insert("token".into(), "".into());

        let expected = vec![
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (
                databricks::HTTP_PATH,
                "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id",
            ),
            (databricks::CATALOG, "C"),
            (databricks::CLIENT_ID, "CLIENT_ID"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (
                databricks::AUTH_TYPE,
                databricks::auth_type::EXTERNAL_BROWSER,
            ),
        ];
        run_config_test(config, &expected).unwrap();
    }

    /// U2M variant where only the OAuth-app `client_id` accompanies the
    /// preminted token (no `client_secret`). Token-first dispatch still routes
    /// to PAT, matching dbt-databricks' `_ensure_config`.
    #[test]
    fn test_oauth_with_preminted_token_and_only_client_id_routes_to_pat() {
        let mut config = base_config();
        config.insert(
            "http_path".into(),
            "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id".into(),
        );
        config.insert("client_id".into(), "CLIENT_ID".into());
        config.insert("auth_type".into(), "oauth".into());
        config.insert("token".into(), "PREMINTED_U2M_TOKEN".into());
        let expected = vec![
            (databricks::TOKEN, "PREMINTED_U2M_TOKEN"),
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (
                databricks::HTTP_PATH,
                "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id",
            ),
            (databricks::CATALOG, "C"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (databricks::AUTH_TYPE, databricks::auth_type::PAT),
        ];
        run_config_test(config, &expected).unwrap();
    }

    #[test]
    fn test_unknown_auth_type_defaults_to_token() {
        for auth_type in ["external_browser", "pat"] {
            let mut config = base_config();
            config.insert(
                "http_path".into(),
                "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id".into(),
            );
            config.insert("token".into(), "T".into());
            config.insert("auth_type".into(), auth_type.into());

            let expected = vec![
                (databricks::TOKEN, "T"),
                (databricks::SCHEMA, "S"),
                (databricks::HOST, "H"),
                (
                    databricks::HTTP_PATH,
                    "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id",
                ),
                (databricks::CATALOG, "C"),
                (databricks::USER_AGENT, USER_AGENT_NAME),
                (databricks::AUTH_TYPE, databricks::auth_type::PAT),
            ];
            run_config_test(config, &expected).unwrap();
        }
    }

    #[test]
    fn test_explicit_token_auth_type() {
        // Test that auth_type: "token" works the same as omitting auth_type
        let mut config = base_config();
        config.insert("token".into(), "T".into());
        config.insert("auth_type".into(), "token".into());

        let expected = vec![
            (databricks::TOKEN, "T"),
            (databricks::SCHEMA, "S"),
            (databricks::HOST, "H"),
            (databricks::HTTP_PATH, "/sql/1.0/warehouses/warehouse-id"),
            (databricks::CATALOG, "C"),
            (databricks::USER_AGENT, USER_AGENT_NAME),
            (databricks::AUTH_TYPE, databricks::auth_type::PAT),
        ];
        run_config_test(config, &expected).unwrap();
    }

    #[test]
    fn test_validate_config_errors_with_missing_client_id_and_present_client_secret() {
        let config = Mapping::from_iter([
            ("host".into(), "H".into()),
            (
                "http_path".into(),
                "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id".into(),
            ),
            ("schema".into(), "S".into()),
            ("database".into(), "C".into()),
            ("client_secret".into(), "some_secret".into()),
            ("auth_type".into(), "oauth".into()),
        ]);
        let result = validate_config(&AdapterConfig::new(config));
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().msg(),
            "The config 'client_id' is required to connect to Databricks when 'client_secret' is present"
        );
    }

    #[test]
    fn test_validate_config_errors_with_missing_azure_client_id_and_present_azure_client_secret() {
        let config = Mapping::from_iter([
            ("host".into(), "H".into()),
            (
                "http_path".into(),
                "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id".into(),
            ),
            ("schema".into(), "S".into()),
            ("database".into(), "C".into()),
            ("azure_client_secret".into(), "some_secret".into()),
            ("auth_type".into(), "oauth".into()),
        ]);
        let result = validate_config(&AdapterConfig::new(config));
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().msg(),
            "The config 'azure_client_id' and 'azure_client_secret' must be both present or both absent"
        );
    }

    #[test]
    fn test_validate_config_errors_with_present_azure_client_id_and_missing_azure_client_secret() {
        let config = Mapping::from_iter([
            ("host".into(), "H".into()),
            (
                "http_path".into(),
                "sql/protocolv1/o/1030i40i30i50i3/my-cluster-id".into(),
            ),
            ("schema".into(), "S".into()),
            ("database".into(), "C".into()),
            ("azure_client_id".into(), "some_id".into()),
            ("auth_type".into(), "oauth".into()),
        ]);
        let result = validate_config(&AdapterConfig::new(config));
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().msg(),
            "The config 'azure_client_id' and 'azure_client_secret' must be both present or both absent"
        );
    }

    #[test]
    fn test_missing_host_is_config_error() {
        let config = Mapping::from_iter([
            ("schema".into(), "S".into()),
            ("database".into(), "C".into()),
            (
                "http_path".into(),
                "/sql/1.0/warehouses/warehouse-id".into(),
            ),
            ("token".into(), "T".into()),
        ]);

        let err = DatabricksAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert_eq!(err.msg(), "host is required");
    }

    #[test]
    fn test_missing_http_path_is_yaml_error() {
        let config = Mapping::from_iter([
            ("host".into(), "H".into()),
            ("schema".into(), "S".into()),
            ("database".into(), "C".into()),
            ("token".into(), "T".into()),
        ]);

        let err = DatabricksAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");

        match err {
            AuthError::YAML(e) => assert!(e.to_string().contains("missing field `http_path`")),
            other => panic!("expected YAML missing-field error, got {other:?}"),
        }
    }

    #[test]
    fn test_missing_schema_is_yaml_error() {
        for (missing_key, expected_msg) in [
            ("schema", "missing field `schema`"),
            ("database", "missing field `database`"),
            ("token", "missing field `token`"),
        ] {
            let mut config = base_config();
            config.insert("token".into(), "T".into());
            config.remove(missing_key);

            let err = DatabricksAuth::new(Box::new(crate::NoopAuthWarningPrinter))
                .configure(&AdapterConfig::new(config))
                .expect_err("configure should fail");

            match err {
                AuthError::YAML(e) => assert!(e.to_string().contains(expected_msg)),
                other => panic!("expected YAML missing-field error, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_parse_auth_oauth_client_secret_non_string_is_yaml_error() {
        let config = Mapping::from_iter([
            ("auth_type".into(), "oauth".into()),
            ("client_id".into(), "CID".into()),
            ("client_secret".into(), YmlValue::number(1i64.into())),
        ]);

        let err = parse_auth(&AdapterConfig::new(config), &crate::NoopAuthWarningPrinter)
            .expect_err("expected parse_auth error");
        match err {
            AuthError::YAML(e) => assert!(e.to_string().contains("missing field `client_secret`")),
            other => panic!("expected YAML missing-field error, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_auth_oauth_client_id_non_string_is_yaml_error() {
        let config = Mapping::from_iter([
            ("auth_type".into(), "oauth".into()),
            ("client_id".into(), YmlValue::bool(true)),
        ]);

        let err = parse_auth(&AdapterConfig::new(config), &crate::NoopAuthWarningPrinter)
            .expect_err("expected parse_auth error");
        match err {
            AuthError::YAML(e) => assert!(e.to_string().contains("missing field `client_id`")),
            other => panic!("expected YAML missing-field error, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_http_path_no_extra_params() {
        let mapping = Mapping::from_iter([("http_path".into(), "/sql/extra/warehouse".into())]);
        let config = AdapterConfig::new(mapping);

        let path = resolve_http_path(&config).expect("expected to resolve http_path from config");

        assert_eq!(path, "/sql/extra/warehouse");
    }

    #[test]
    fn test_configure_fingerprint_differs_per_compute() {
        // Each named compute must produce a distinct fingerprint, or the pool
        // could reuse another compute's connection.
        fn fingerprint_with(compute: Option<&str>) -> u64 {
            let compute_config = Mapping::from_iter([
                (
                    "small".into(),
                    Mapping::from_iter([("http_path".into(), "/sql/1.0/warehouses/small".into())])
                        .into(),
                ),
                (
                    "large".into(),
                    Mapping::from_iter([("http_path".into(), "/sql/1.0/warehouses/large".into())])
                        .into(),
                ),
            ]);
            let mut mapping = base_config();
            mapping.insert("token".into(), "T".into());
            mapping.insert("compute".into(), compute_config.into());
            if let Some(name) = compute {
                mapping.insert("databricks_compute".into(), name.into());
            }
            let builder = DatabricksAuth::new(Box::new(crate::NoopAuthWarningPrinter))
                .configure(&AdapterConfig::new(mapping))
                .expect("configure should succeed");
            let opts = builder.into_iter().collect::<Vec<_>>();
            DatabaseBuilder::fingerprint(opts.iter()).as_u64()
        }

        let default_fp = fingerprint_with(None);
        let small_fp = fingerprint_with(Some("small"));
        let large_fp = fingerprint_with(Some("large"));

        assert_ne!(
            small_fp, large_fp,
            "different named compute must yield different connection fingerprints"
        );
        assert_ne!(default_fp, small_fp);
        assert_ne!(default_fp, large_fp);
    }

    #[test]
    fn test_resolve_http_path_uses_databricks_compute() {
        let compute1_config =
            Mapping::from_iter([("http_path".into(), "/sql/warehouse/specific_compute".into())]);
        let compute_config = Mapping::from_iter([("compute1".into(), compute1_config.into())]);
        let mapping = Mapping::from_iter([
            ("http_path".into(), "/sql/config/warehouse".into()),
            ("compute".into(), compute_config.into()),
            ("databricks_compute".into(), "compute1".into()),
        ]);
        let config = AdapterConfig::new(mapping);

        let path = resolve_http_path(&config)
            .expect("expected to resolve http_path from databricks compute config");

        assert_eq!(path, "/sql/warehouse/specific_compute");
    }

    #[test]
    fn test_resolve_http_path_compute_missing_http_path_errors() {
        let compute1_config = Mapping::from_iter([("name".into(), "compute1".into())]);
        let compute_config = Mapping::from_iter([("compute1".into(), compute1_config.into())]);
        let mapping = Mapping::from_iter([
            ("http_path".into(), "/sql/config/warehouse".into()),
            ("compute".into(), compute_config.into()),
            ("databricks_compute".into(), "compute1".into()),
        ]);
        let config = AdapterConfig::new(mapping);

        let err = resolve_http_path(&config).expect_err("expected missing http_path error");
        assert_eq!(
            err.msg(),
            "Compute resource 'compute1' does not exist or does not specify http_path"
        );
    }

    #[test]
    fn test_resolve_http_path_compute_wrong_shape_errors() {
        let compute_config = Mapping::from_iter([("compute1".into(), "not-a-mapping".into())]);
        let mapping = Mapping::from_iter([
            ("http_path".into(), "/sql/config/warehouse".into()),
            ("compute".into(), compute_config.into()),
            ("databricks_compute".into(), "compute1".into()),
        ]);
        let config = AdapterConfig::new(mapping);

        let err = resolve_http_path(&config).expect_err("expected compute shape error");
        assert_eq!(
            err.msg(),
            "Compute resource 'compute1' does not exist or does not specify http_path"
        );
    }

    #[test]
    fn test_resolve_http_path_compute_http_path_non_string_errors() {
        let compute1_config =
            Mapping::from_iter([("http_path".into(), YmlValue::number(1i64.into()))]);
        let compute_config = Mapping::from_iter([("compute1".into(), compute1_config.into())]);
        let mapping = Mapping::from_iter([
            ("http_path".into(), "/sql/config/warehouse".into()),
            ("compute".into(), compute_config.into()),
            ("databricks_compute".into(), "compute1".into()),
        ]);
        let config = AdapterConfig::new(mapping);

        let err = resolve_http_path(&config).expect_err("expected http_path type error");
        assert_eq!(
            err.msg(),
            "Compute resource 'compute1' does not exist or does not specify http_path"
        );
    }

    #[test]
    fn test_resolve_http_config_missing_errors() {
        let compute_config =
            Mapping::from_iter([("compute1".into(), "/sql/warehouse/specific_compute".into())]);
        let mapping = Mapping::from_iter([
            ("http_path".into(), "/sql/config/warehouse".into()),
            ("compute".into(), compute_config.into()),
            ("databricks_compute".into(), "compute2".into()),
        ]);
        let config = AdapterConfig::new(mapping);

        let result = resolve_http_path(&config);

        assert!(
            result.is_err(),
            "expected an error when http_path is missing"
        );
    }

    /// Tenant extraction from the `/aad/auth` redirect Location across Azure clouds
    /// and malformed inputs (the login domain varies by cloud).
    #[test]
    fn test_parse_azure_tenant_from_location() {
        let tenant = "11111111-2222-3333-4444-555555555555";

        // public cloud
        assert_eq!(
            parse_azure_tenant_from_location(&format!(
                "https://login.microsoftonline.com/{tenant}/oauth2/authorize?response_type=code"
            ))
            .unwrap(),
            tenant
        );
        // us gov cloud (different login domain)
        assert_eq!(
            parse_azure_tenant_from_location(&format!(
                "https://login.microsoftonline.us/{tenant}/oauth2/v2.0/authorize"
            ))
            .unwrap(),
            tenant
        );
        // no tenant path segment
        assert!(parse_azure_tenant_from_location("https://login.microsoftonline.com/").is_err());
        // unparseable
        assert!(parse_azure_tenant_from_location("not a url").is_err());
    }
}
