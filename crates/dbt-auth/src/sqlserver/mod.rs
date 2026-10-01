#![allow(dead_code, unused_mut, reason = "TODO: implement")]

use std::borrow::Cow;

use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter};

use dbt_adbc::{
    Backend,
    database::{self, Builder as DatabaseBuilder},
};

const DEFAULT_AUTH: &str = "ActiveDirectoryServicePrincipal";
const DEFAULT_PORT: &str = "1433";

/// Parsed Microsoft Entra authentication settings for SQL Server / Fabric.
///
/// Each variant maps profile fields onto [`go-mssqldb`](https://github.com/microsoft/go-mssqldb)
/// connection URI query parameters consumed by the MSSQL ADBC driver. See upstream
/// [dbt-fabric `fabric_credentials.py`](https://github.com/microsoft/dbt-fabric/blob/main/dbt/adapters/fabric/fabric_credentials.py)
/// for supported `authentication` profile values.
#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
enum SQLServerAuthIR<'a> {
    /// Unattended service-principal auth (default for Fabric).
    ///
    /// Profile: `authentication: ActiveDirectoryServicePrincipal` (alias: `ServicePrincipal`),
    /// `client_id`, `client_secret`, optional `tenant_id`.
    ///
    /// URI: `fedauth=ActiveDirectoryServicePrincipal`, `user id={client_id}[@{tenant_id}]`,
    /// `password={client_secret}`.
    ActiveDirectoryServicePrincipal {
        /// When set, appended to `client_id` as `{client_id}@{tenant_id}` in `user id`.
        tenant_id: Option<&'a str>,
        /// Entra application (client) ID.
        client_id: &'a str,
        /// Entra application client secret.
        client_secret: &'a str,
    },

    /// Pre-minted Entra bearer token supplied directly in the profile.
    ///
    /// Profile: `authentication: ActiveDirectoryAccessToken`, `access_token`.
    ///
    /// URI: `fedauth=ActiveDirectoryServicePrincipalAccessToken`, `password={access_token}`.
    ///
    /// Note: the profile name and the `fedauth` value differ on purpose; the driver spells
    /// this workflow `ActiveDirectoryServicePrincipalAccessToken` and reads the token from
    /// `password`.
    ///
    /// No credential exchange happens: the token is forwarded to the TDS FEDAUTH handshake
    /// verbatim, so an expired token fails at connection time and is never refreshed.
    ActiveDirectoryAccessToken {
        /// Entra bearer token (`access_token` in profile, `password` in URI).
        access_token: &'a str,
    },

    /// user/password auth via the resource-owner password credentials flow.
    ///
    /// Profile: `authentication: ActiveDirectoryPassword`, `UID`, `PWD`, and `client_id`.
    ///
    /// URI: `fedauth=ActiveDirectoryPassword`, `user id={UID}`, `password={PWD}`,
    /// `applicationclientid={client_id}`.
    ///
    /// Note: `client_id` here is the Entra app used to obtain the user token
    /// (`applicationclientid` in go-mssqldb), not the service principal identity.
    /// The Entra user must also be provisioned in the target warehouse
    /// (`CREATE USER ... FROM EXTERNAL PROVIDER`) and hold a Fabric workspace role.
    ActiveDirectoryPassword {
        /// Entra app registration allowed to request tokens for SQL (`applicationclientid`).
        client_id: &'a str,
        /// Entra user principal name (`UID` in profile).
        user: &'a str,
        /// Entra user password (`PWD` in profile).
        password: &'a str,
    },

    /// Auth via [`EnvironmentCredential`](https://pkg.go.dev/github.com/Azure/azure-sdk-for-go/sdk/azidentity#EnvironmentCredential).
    ///
    /// Profile: `authentication: environment`. No secrets in the profile; credentials come
    /// from standard `AZURE_*` environment variables (for example `AZURE_CLIENT_ID`,
    /// `AZURE_CLIENT_SECRET`, `AZURE_TENANT_ID`).
    ///
    /// URI: `fedauth=ActiveDirectoryEnvironment`.
    ActiveDirectoryEnvironment,
}

impl<'a> SQLServerAuthIR<'a> {
    pub fn apply(
        self,
        mut builder: DatabaseBuilder,
        _warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<DatabaseBuilder, AuthError> {
        // nearly all auth parameters are set in the URI
        // There are quite a few parameters that can be set
        // See: https://github.com/microsoft/go-mssqldb/tree/main?tab=readme-ov-file#connection-parameters-and-dsn
        match self {
            Self::ActiveDirectoryServicePrincipal {
                tenant_id,
                client_id,
                client_secret,
            } => {
                if let Some(uri) = builder.uri.as_mut() {
                    let uid: Cow<str> = tenant_id.map_or_else(
                        || client_id.into(),
                        |tenant_id| format!("{client_id}@{tenant_id}").into(),
                    );

                    uri.query_pairs_mut()
                        .append_pair("fedauth", "ActiveDirectoryServicePrincipal")
                        .append_pair("user id", &uid)
                        .append_pair("password", client_secret)
                        .finish();
                }
            }
            Self::ActiveDirectoryAccessToken { access_token } => {
                if let Some(uri) = builder.uri.as_mut() {
                    // go-mssqldb reads the bearer token from `password` and requires no
                    // `user id` for this workflow.
                    uri.query_pairs_mut()
                        .append_pair("fedauth", "ActiveDirectoryServicePrincipalAccessToken")
                        .append_pair("password", access_token)
                        .finish();
                }
            }
            Self::ActiveDirectoryPassword {
                client_id,
                user,
                password,
            } => {
                if let Some(uri) = builder.uri.as_mut() {
                    uri.query_pairs_mut()
                        .append_pair("fedauth", "ActiveDirectoryPassword")
                        .append_pair("user id", user)
                        .append_pair("password", password)
                        .append_pair("applicationclientid", client_id)
                        .finish();
                }
            }
            Self::ActiveDirectoryEnvironment => {
                if let Some(uri) = builder.uri.as_mut() {
                    uri.query_pairs_mut()
                        .append_pair("fedauth", "ActiveDirectoryEnvironment")
                        .finish();
                }
            }
        }
        Ok(builder)
    }
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<SQLServerAuthIR<'a>, AuthError> {
    let mut authentication = config.get_str("authentication").unwrap_or(DEFAULT_AUTH);

    // https://github.com/microsoft/dbt-fabric/blob/0de219082282724a789b0d1b18509d39899da8e1/dbt/adapters/fabric/fabric_credentials.py#L78-L79
    if authentication.eq_ignore_ascii_case("serviceprincipal") {
        authentication = "ActiveDirectoryServicePrincipal";
    }

    // dbt-fabric matches this method case-insensitively:
    // `credentials.authentication.lower() == "activedirectoryaccesstoken"`
    if authentication.eq_ignore_ascii_case("ActiveDirectoryAccessToken") {
        authentication = "ActiveDirectoryAccessToken";
    }

    match authentication {
        "ActiveDirectoryServicePrincipal" => Ok(SQLServerAuthIR::ActiveDirectoryServicePrincipal {
            tenant_id: config.get_str("tenant_id"),
            client_id: config.require_str("client_id")?,
            client_secret: config.require_str("client_secret")?,
        }),
        "ActiveDirectoryAccessToken" => {
            let access_token = config.require_str("access_token")?;
            // The driver rejects an empty token with a message that names the `password`
            // URI parameter, which does not exist in the user's profile. Fail earlier with
            // the profile field name instead.
            if access_token.is_empty() {
                return Err(AuthError::config(
                    "`access_token` must not be empty when using ActiveDirectoryAccessToken authentication",
                ));
            }
            Ok(SQLServerAuthIR::ActiveDirectoryAccessToken { access_token })
        }
        "ActiveDirectoryPassword" => Ok(SQLServerAuthIR::ActiveDirectoryPassword {
            user: config.require_str("UID")?,
            password: config.require_str("PWD")?,
            client_id: config.require_str("client_id")?,
        }),
        "environment" => Ok(SQLServerAuthIR::ActiveDirectoryEnvironment),
        "ActiveDirectoryInteractive" | "ActiveDirectoryIntegrated" | "CLI" | "auto" => {
            unimplemented!("authentication method {} not implemented", authentication)
        }
        _ => Err(AuthError::config(format!(
            "Invalid authentication method: {authentication} must be one of: [ActiveDirectoryServicePrincipal, ActiveDirectoryAccessToken, ActiveDirectoryPassword, environment]"
        ))),
    }
}

fn apply_connection_args(
    config: &AdapterConfig,
    mut builder: DatabaseBuilder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabaseBuilder, AuthError> {
    let host = config.require_str("host")?;
    let port = config
        .get_string("port")
        .unwrap_or_else(|| DEFAULT_PORT.into());

    // both "mssql://" and "sqlserver://" are supported by the driver,
    // but it seems like "sqlserver://" is the preferred scheme according to the underlying Go driver docs.
    //
    // See: https://github.com/microsoft/go-mssqldb?tab=readme-ov-file#deprecated
    //
    // TODO: we probably want to be a bit smarter about constructing the URI, but this is a start
    builder.with_parse_uri(format!("sqlserver://{host}:{port}"))?;

    if let Some(uri) = builder.uri.as_mut() {
        uri.query_pairs_mut()
            .append_pair("database", config.require_str("database")?)
            .finish();
    }

    // TODO: other parameters, i.e.
    // - connection timeout
    // - dial timeout
    // - encrypt
    // - app name
    // - log
    // - retries
    //
    // See: https://github.com/microsoft/go-mssqldb/tree/main?tab=readme-ov-file#less-common-parameters
    Ok(builder)
}

pub struct SQLServerAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl SQLServerAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for SQLServerAuth {
    fn backend(&self) -> Backend {
        Backend::SQLServer
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        let authentication_args = parse_auth(config, self.warning_printer.as_ref())?;
        let builder = database::Builder::new(self.backend());
        let builder = apply_connection_args(config, builder, self.warning_printer.as_ref())?;
        let builder = authentication_args.apply(builder, self.warning_printer.as_ref())?;
        Ok(builder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_options::uri_value;
    use dbt_test_primitives::assert_contains;
    use dbt_yaml::Mapping;

    fn make_config(pairs: impl IntoIterator<Item = (&'static str, &'static str)>) -> AdapterConfig {
        AdapterConfig::new(Mapping::from_iter(
            pairs.into_iter().map(|(k, v)| (k.into(), v.into())),
        ))
    }

    #[test]
    fn test_service_principal_with_tenant_id() {
        let config = make_config([
            ("authentication", "serviceprincipal"),
            ("host", "myserver.database.windows.net"),
            ("database", "mydb"),
            ("tenant_id", "my-tenant"),
            ("client_id", "my-client"),
            ("client_secret", "my-secret"),
        ]);

        let outcome = SQLServerAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&config)
            .expect("configure");
        let uri = uri_value(&outcome);

        assert_contains!(&uri, "sqlserver://myserver.database.windows.net:1433");
        assert_contains!(&uri, "database=mydb");
        assert_contains!(&uri, "fedauth=ActiveDirectoryServicePrincipal");
        assert_contains!(&uri, "user+id=my-client%40my-tenant");
        assert_contains!(&uri, "password=my-secret");
    }

    #[test]
    fn test_service_principal_without_tenant_id() {
        let config = make_config([
            ("authentication", "ActiveDirectoryServicePrincipal"),
            ("host", "myserver.database.windows.net"),
            ("database", "mydb"),
            ("client_id", "my-client"),
            ("client_secret", "my-secret"),
        ]);

        let outcome = SQLServerAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&config)
            .expect("configure");
        let uri = uri_value(&outcome);

        assert_contains!(&uri, "fedauth=ActiveDirectoryServicePrincipal");
        assert_contains!(&uri, "user+id=my-client");
        assert_contains!(&uri, "password=my-secret");
    }

    #[test]
    fn test_active_directory_password() {
        let config = make_config([
            ("authentication", "ActiveDirectoryPassword"),
            ("host", "myserver.database.windows.net"),
            ("database", "mydb"),
            ("client_id", "my-client"),
            ("UID", "alice@example.com"),
            ("PWD", "hunter2"),
        ]);

        let outcome = SQLServerAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&config)
            .expect("configure");
        let uri = uri_value(&outcome);

        assert_contains!(&uri, "sqlserver://myserver.database.windows.net:1433");
        assert_contains!(&uri, "fedauth=ActiveDirectoryPassword");
        assert_contains!(&uri, "user+id=alice%40example.com");
        assert_contains!(&uri, "password=hunter2");
        assert_contains!(&uri, "applicationclientid=my-client");
    }

    #[test]
    fn test_environment_auth() {
        let config = make_config([
            ("authentication", "environment"),
            ("host", "myserver.database.windows.net"),
            ("database", "mydb"),
        ]);

        let outcome = SQLServerAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&config)
            .expect("configure");
        let uri = uri_value(&outcome);

        assert_contains!(&uri, "sqlserver://myserver.database.windows.net:1433");
        assert_contains!(&uri, "database=mydb");
        assert_contains!(&uri, "fedauth=ActiveDirectoryEnvironment");
    }

    #[test]
    fn test_default_port_is_1433() {
        let config = make_config([
            ("authentication", "environment"),
            ("host", "myserver.database.windows.net"),
            ("database", "mydb"),
        ]);

        let outcome = SQLServerAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&config)
            .expect("configure");
        let uri = uri_value(&outcome);

        assert_contains!(&uri, ":1433");
    }

    #[test]
    fn test_custom_port() {
        let config = make_config([
            ("authentication", "environment"),
            ("host", "myserver.database.windows.net"),
            ("port", "1434"),
            ("database", "mydb"),
        ]);

        let outcome = SQLServerAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&config)
            .expect("configure");
        let uri = uri_value(&outcome);

        assert_contains!(&uri, ":1434");
    }

    #[test]
    fn test_service_principal_alias() {
        // "ServicePrincipal" is an alias for "ActiveDirectoryServicePrincipal"
        let config = make_config([
            ("authentication", "ServicePrincipal"),
            ("host", "myserver.database.windows.net"),
            ("database", "mydb"),
            ("client_id", "my-client"),
            ("client_secret", "my-secret"),
        ]);

        let outcome = SQLServerAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&config)
            .expect("configure");
        let uri = uri_value(&outcome);

        assert_contains!(&uri, "fedauth=ActiveDirectoryServicePrincipal");
    }

    const FAKE_ACCESS_TOKEN: &str = "eyJ0eXAiOiJKV1QifQ.eyJhdWQiOiJkYiJ9.c2lnbmF0dXJl";

    #[test]
    fn test_active_directory_access_token() {
        let config = make_config([
            ("authentication", "ActiveDirectoryAccessToken"),
            ("host", "myserver.database.windows.net"),
            ("database", "mydb"),
            ("access_token", FAKE_ACCESS_TOKEN),
        ]);

        let outcome = SQLServerAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&config)
            .expect("configure");
        let uri = uri_value(&outcome);

        assert_contains!(&uri, "sqlserver://myserver.database.windows.net:1433");
        assert_contains!(&uri, "database=mydb");
        // dbt-fabric spells the profile value `ActiveDirectoryAccessToken`, but the driver
        // only understands the go-mssqldb spelling.
        assert_contains!(&uri, "fedauth=ActiveDirectoryServicePrincipalAccessToken");
        assert_contains!(&uri, &format!("password={FAKE_ACCESS_TOKEN}"));
        // This workflow takes no identity parameters.
        assert!(!uri.contains("user+id"), "unexpected `user id` in {uri}");
        assert!(
            !uri.contains("applicationclientid"),
            "unexpected `applicationclientid` in {uri}"
        );
    }

    #[test]
    fn test_access_token_is_case_insensitive() {
        // dbt-fabric lowercases `authentication` before comparing.
        let config = make_config([
            ("authentication", "activedirectoryaccesstoken"),
            ("host", "myserver.database.windows.net"),
            ("database", "mydb"),
            ("access_token", FAKE_ACCESS_TOKEN),
        ]);

        let outcome = SQLServerAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&config)
            .expect("configure");
        let uri = uri_value(&outcome);

        assert_contains!(&uri, "fedauth=ActiveDirectoryServicePrincipalAccessToken");
    }

    #[test]
    fn test_access_token_missing() {
        let config = make_config([
            ("authentication", "ActiveDirectoryAccessToken"),
            ("host", "myserver.database.windows.net"),
            ("database", "mydb"),
        ]);

        let err = SQLServerAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&config)
            .expect_err("missing access_token must fail");

        match err {
            AuthError::YAML(e) => {
                assert_contains!(e.to_string(), "missing field `access_token`")
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn test_access_token_empty() {
        let config = make_config([
            ("authentication", "ActiveDirectoryAccessToken"),
            ("host", "myserver.database.windows.net"),
            ("database", "mydb"),
            ("access_token", ""),
        ]);

        let err = SQLServerAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&config)
            .expect_err("empty access_token must fail");

        assert_contains!(err.msg(), "`access_token` must not be empty");
    }

    #[test]
    fn test_invalid_authentication_lists_access_token() {
        let config = make_config([
            ("authentication", "NotAnAuthMethod"),
            ("host", "myserver.database.windows.net"),
            ("database", "mydb"),
        ]);

        let err = SQLServerAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&config)
            .expect_err("invalid authentication must fail");

        assert_contains!(err.msg(), "ActiveDirectoryAccessToken");
    }
}
