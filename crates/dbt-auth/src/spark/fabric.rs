//! Microsoft Fabric Lakehouse (Spark via the Fabric Livy API).
//!
//! This is a platform specialization of the Spark Livy transport: the driver
//! options emitted here are the same `spark.*` options as [`super`], plus the
//! `spark.livy.azure.*` Entra ID options and a Fabric-specific Livy base URL
//! built from the workspace and lakehouse ids.
//!
//! Profile shape follows the v1 `dbt-fabricspark` adapter:
//! `endpoint`, `workspaceid`, `lakehouseid`, `authentication: CLI|SPN`, ...

use crate::{AdapterConfig, AuthError, AuthWarningPrinter};
use dbt_adbc::{database, spark};

use std::collections::HashMap;

const DEFAULT_ENDPOINT: &str = "https://api.fabric.microsoft.com/v1";
const LIVY_API_VERSION: &str = "2023-12-01";

/// Returns true when the config is shaped like a `fabricspark` profile.
///
/// Fabric Lakehouse profiles are identified by their required GUID fields,
/// which no plain Spark profile has.
pub(super) fn is_fabric_spark_config(config: &AdapterConfig) -> bool {
    config.get_str("lakehouseid").is_some() || config.get_str("workspaceid").is_some()
}

#[derive(Debug)]
enum AzureCredential<'a> {
    /// The local Azure CLI (`az login`) context.
    Cli,
    /// The azidentity `DefaultAzureCredential` chain.
    Default,
    /// Credentials from `AZURE_*` environment variables only.
    Environment,
    /// An Azure managed identity; `client_id` selects a user-assigned identity.
    ManagedIdentity { client_id: Option<&'a str> },
    /// Service principal with a client secret (profile: `SPN`).
    ClientSecret {
        tenant_id: &'a str,
        client_id: &'a str,
        client_secret: &'a str,
    },
}

#[derive(Debug)]
pub(super) struct FabricSparkAuthIR<'a> {
    endpoint: &'a str,
    workspace_id: &'a str,
    lakehouse_id: &'a str,
    credential: AzureCredential<'a>,
    token_scope: Option<&'a str>,
    session_params: HashMap<&'a str, String>,
}

impl<'a> FabricSparkAuthIR<'a> {
    pub(super) fn apply(
        self,
        mut builder: database::Builder,
        _warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<database::Builder, AuthError> {
        builder.with_named_option(spark::TRANSPORT_API, spark::transport_api::LIVY)?;
        builder.with_named_option(spark::livy::SESSION_KIND, spark::livy::session_kind::SQL)?;

        // The endpoint ("https://api.fabric.microsoft.com/v1") is split into
        // the host ("https://api.fabric.microsoft.com", scheme preserved so
        // the driver uses TLS) and a path prefix ("/v1") that heads the Livy
        // base URL. Owned strings are built here at the builder boundary.
        let endpoint = self.endpoint.trim_end_matches('/');
        let path_start = endpoint
            .find("://")
            .map(|scheme_end| scheme_end + "://".len())
            .and_then(|authority_start| {
                endpoint[authority_start..]
                    .find('/')
                    .map(|p| authority_start + p)
            })
            .unwrap_or(endpoint.len());
        let (host, path_prefix) = endpoint.split_at(path_start);

        builder.with_named_option(spark::HOST, host)?;
        builder.with_named_option(
            spark::livy::BASE_URL,
            format!(
                "{}/workspaces/{}/lakehouses/{}/livyapi/versions/{}",
                path_prefix, self.workspace_id, self.lakehouse_id, LIVY_API_VERSION
            ),
        )?;

        builder.with_named_option(spark::AUTH_TYPE, spark::auth_type::AZURE_TOKEN)?;
        let credential = match self.credential {
            AzureCredential::Cli => spark::livy::azure::credential::AZ_CLI,
            AzureCredential::Default => spark::livy::azure::credential::DEFAULT,
            AzureCredential::Environment => spark::livy::azure::credential::ENVIRONMENT,
            // Credential parameters ride in username/password, following the
            // MSSQL driver's fedauth conventions.
            AzureCredential::ManagedIdentity { client_id } => {
                if let Some(client_id) = client_id {
                    builder.with_named_option(spark::USERNAME, client_id)?;
                }
                spark::livy::azure::credential::MANAGED_IDENTITY
            }
            AzureCredential::ClientSecret {
                tenant_id,
                client_id,
                client_secret,
            } => {
                builder
                    .with_named_option(spark::USERNAME, format!("{client_id}@{tenant_id}"))?;
                builder.with_named_option(spark::PASSWORD, client_secret)?;
                spark::livy::azure::credential::SERVICE_PRINCIPAL
            }
        };
        builder.with_named_option(spark::livy::azure::CREDENTIAL, credential)?;

        if let Some(scope) = self.token_scope {
            builder.with_named_option(spark::livy::azure::TOKEN_SCOPE, scope)?;
        }

        super::apply_session_params(&self.session_params, &mut builder)?;

        Ok(builder)
    }
}

pub(super) fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<FabricSparkAuthIR<'a>, AuthError> {
    if let Some(method) = config.get_str("method")
        && method != "livy"
    {
        return Err(AuthError::config(
            "'method' must be 'livy' for Fabric Lakehouse (fabricspark) profiles",
        ));
    }

    let workspace_id = config.get_str("workspaceid").ok_or_else(|| {
        AuthError::config("'workspaceid' is a required fabricspark configuration")
    })?;
    let lakehouse_id = config.get_str("lakehouseid").ok_or_else(|| {
        AuthError::config("'lakehouseid' is a required fabricspark configuration")
    })?;

    Ok(FabricSparkAuthIR {
        endpoint: config.get_str("endpoint").unwrap_or(DEFAULT_ENDPOINT),
        workspace_id,
        lakehouse_id,
        credential: parse_credential(config)?,
        token_scope: config.get_str("token_scope"),
        session_params: parse_session_params(config)?,
    })
}

/// Parses `authentication` (v1 dbt-fabricspark used `CLI` and `SPN`; the
/// remaining values map directly onto the driver's credential kinds).
fn parse_credential<'a>(config: &'a AdapterConfig) -> Result<AzureCredential<'a>, AuthError> {
    let Some(authentication) = config.get_str("authentication") else {
        return Ok(AzureCredential::Cli);
    };

    if authentication.eq_ignore_ascii_case("cli") {
        Ok(AzureCredential::Cli)
    } else if authentication.eq_ignore_ascii_case("spn")
        || authentication.eq_ignore_ascii_case("client_secret")
    {
        Ok(AzureCredential::ClientSecret {
            tenant_id: require_spn_field(config, "tenant_id")?,
            client_id: require_spn_field(config, "client_id")?,
            client_secret: require_spn_field(config, "client_secret")?,
        })
    } else if authentication.eq_ignore_ascii_case("default") {
        Ok(AzureCredential::Default)
    } else if authentication.eq_ignore_ascii_case("environment") {
        Ok(AzureCredential::Environment)
    } else if authentication.eq_ignore_ascii_case("managed_identity") {
        Ok(AzureCredential::ManagedIdentity {
            client_id: config.get_str("client_id"),
        })
    } else {
        Err(AuthError::config(
            "invalid 'authentication' for fabricspark: must be one of \
[CLI, SPN, default, environment, managed_identity]",
        ))
    }
}

fn require_spn_field<'a>(
    config: &'a AdapterConfig,
    field: &'static str,
) -> Result<&'a str, AuthError> {
    config.get_str(field).ok_or_else(|| {
        AuthError::config(format!(
            "'{field}' is required when authentication is 'SPN'"
        ))
    })
}

fn parse_session_params(config: &AdapterConfig) -> Result<HashMap<&str, String>, AuthError> {
    let mut session_params = HashMap::new();
    let Some(ssp) = config.get("server_side_parameters") else {
        return Ok(session_params);
    };
    let super::YmlValue::Mapping(ssp, _) = ssp else {
        return Err(AuthError::config(
            "'server_side_parameters' must be mapping",
        ));
    };
    for (key, value) in ssp {
        let super::YmlValue::String(key, _) = key else {
            return Err(AuthError::config(
                "'server_side_parameters' key must be string",
            ));
        };
        let value = match value {
            super::YmlValue::String(v, _) => v.to_string(),
            super::YmlValue::Number(v, _) => v.to_string(),
            _ => {
                return Err(AuthError::config(
                    "'server_side_parameters' value must be string or number",
                ));
            }
        };
        session_params.insert(key.as_str(), value);
    }
    Ok(session_params)
}

pub(super) fn apply_connection_args(
    _config: &AdapterConfig,
    builder: database::Builder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<database::Builder, AuthError> {
    Ok(builder)
}

#[cfg(test)]
mod tests {
    use super::super::SparkAuth;
    use crate::test_options::other_option_value;
    use crate::{AdapterConfig, Auth};
    use dbt_adbc::spark;
    use dbt_yaml::Mapping;

    fn base_config() -> Mapping {
        Mapping::from_iter([
            ("workspaceid".into(), "ws-guid".into()),
            ("lakehouseid".into(), "lh-guid".into()),
            ("schema".into(), "dbo".into()),
        ])
    }

    #[test]
    fn fabric_defaults_to_cli_credential_and_fabric_endpoint() {
        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(base_config()))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, spark::HOST),
            Some("https://api.fabric.microsoft.com")
        );
        assert_eq!(
            other_option_value(&builder, spark::livy::BASE_URL),
            Some("/v1/workspaces/ws-guid/lakehouses/lh-guid/livyapi/versions/2023-12-01")
        );
        assert_eq!(
            other_option_value(&builder, spark::AUTH_TYPE),
            Some(spark::auth_type::AZURE_TOKEN)
        );
        assert_eq!(
            other_option_value(&builder, spark::livy::azure::CREDENTIAL),
            Some(spark::livy::azure::credential::AZ_CLI)
        );
        assert_eq!(
            other_option_value(&builder, spark::TRANSPORT_API),
            Some(spark::transport_api::LIVY)
        );
    }

    #[test]
    fn fabric_spn_credential() {
        let mut config = base_config();
        config.insert("authentication".into(), "SPN".into());
        config.insert("tenant_id".into(), "tenant".into());
        config.insert("client_id".into(), "client".into());
        config.insert("client_secret".into(), "hunter2".into());

        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, spark::livy::azure::CREDENTIAL),
            Some(spark::livy::azure::credential::SERVICE_PRINCIPAL)
        );
        // SPN parameters ride in username/password (MSSQL fedauth form).
        assert_eq!(
            other_option_value(&builder, spark::USERNAME),
            Some("client@tenant")
        );
        assert_eq!(
            other_option_value(&builder, spark::PASSWORD),
            Some("hunter2")
        );
    }

    #[test]
    fn fabric_spn_missing_secret_err() {
        let mut config = base_config();
        config.insert("authentication".into(), "SPN".into());
        config.insert("tenant_id".into(), "tenant".into());
        config.insert("client_id".into(), "client".into());

        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert!(err.msg().contains("client_secret"));
    }

    #[test]
    fn fabric_missing_workspace_err() {
        let config = Mapping::from_iter([("lakehouseid".into(), "lh-guid".into())]);
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert!(err.msg().contains("workspaceid"));
    }

    #[test]
    fn fabric_custom_endpoint_and_scope() {
        let mut config = base_config();
        config.insert(
            "endpoint".into(),
            "https://my.gateway.example.com/fabric/v1/".into(),
        );
        config.insert("token_scope".into(), "https://example.com/.default".into());

        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, spark::HOST),
            Some("https://my.gateway.example.com")
        );
        assert_eq!(
            other_option_value(&builder, spark::livy::BASE_URL),
            Some("/fabric/v1/workspaces/ws-guid/lakehouses/lh-guid/livyapi/versions/2023-12-01")
        );
        assert_eq!(
            other_option_value(&builder, spark::livy::azure::TOKEN_SCOPE),
            Some("https://example.com/.default")
        );
    }

    #[test]
    fn fabric_invalid_authentication_err() {
        let mut config = base_config();
        config.insert("authentication".into(), "carrier_pigeon".into());
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert!(err.msg().contains("invalid 'authentication'"));
    }

    #[test]
    fn plain_spark_profiles_are_not_routed_to_fabric() {
        // A plain Spark thrift profile must keep working through the
        // original Spark parsing path.
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "thrift".into()),
            ("auth".into(), "NOSASL".into()),
        ]);
        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");
        assert_eq!(
            other_option_value(&builder, spark::AUTH_TYPE),
            Some(spark::auth_type::NOSASL)
        );
    }
}
