mod token_service;

use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter, auth_configure_pipeline};
use database::Builder as DatabaseBuilder;

use crate::redshift::token_service::{TokenEndpoint, create_token_service_client};
use adbc_core::constants::ADBC_OPTION_USERNAME;
use dbt_adbc::redshift::{
    AUTH_IDC_CLIENT_DISPLAY_NAME, AUTH_IDC_REGION, AUTH_IDP_LISTEN_PORT, AUTH_IDP_RESPONSE_TIMEOUT,
    AUTH_ISSUER_URL, AUTH_PROVIDER, AUTH_PROVIDER_BROWSER_IDC, AUTH_PROVIDER_IDP_TOKEN, AUTH_TOKEN,
    AUTH_TOKEN_TYPE,
};
use dbt_adbc::{
    Backend, database,
    redshift::{
        AWS_ACCESS_KEY_ID, AWS_PROFILE, AWS_REGION, AWS_SECRET_ACCESS_KEY, CLUSTER_IDENTIFIER,
        CLUSTER_TYPE, CONNECT_TIMEOUT, WORK_GROUP_NAME,
        cluster_type::{REDSHIFT, SERVERLESS},
    },
};
use percent_encoding::utf8_percent_encode;

// Reference: https://docs.aws.amazon.com/redshift/latest/dg/r_names.html
const CONN_ARG_SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'.')
    .remove(b'-')
    .remove(b'_')
    .add(b' ');

fn encode_conn_arg(value: &str) -> String {
    utf8_percent_encode(value, CONN_ARG_SET).to_string()
}

fn build_connection_uri(
    host: &str,
    port: &str,
    database: &str,
    credentials: Option<(&str, &str)>,
) -> String {
    let host = encode_conn_arg(host);
    let port = encode_conn_arg(port);
    let database = encode_conn_arg(database);

    match credentials {
        Some((user, password)) => {
            let user = encode_conn_arg(user);
            let password = encode_conn_arg(password);
            format!("postgresql://{user}:{password}@{host}:{port}/{database}")
        }
        None => format!("postgresql://{host}:{port}/{database}"),
    }
}

#[derive(Debug)]
enum RedshiftIamTarget<'a> {
    // Serverless GetCredentials derives the DB user from the IAM identity, so no
    // `user`; the workgroup is optional (dbt-redshift derives it from the host).
    Serverless { work_group: Option<&'a str> },
    // Provisioned GetClusterCredentials mints creds for a specific DbUser.
    Provisioned { cluster_id: &'a str, user: &'a str },
}

#[derive(Debug)]
enum RedshiftIamCredentials<'a> {
    AccessKeys {
        access_key_id: &'a str,
        secret_access_key: &'a str,
    },
    Profile {
        iam_profile: &'a str,
    },
}

#[derive(Debug)]
enum RedshiftAuthIR<'a> {
    Database {
        user: &'a str,
        password: &'a str,
        host: &'a str,
        // `port` is normalized (rather than borrowed) because it's the one field
        // that legitimately arrives as a YAML int (e.g. `port: 5439`).
        port: String,
        database: &'a str,
    },
    Iam {
        region: &'a str,
        target: RedshiftIamTarget<'a>,
        credentials: RedshiftIamCredentials<'a>,
        host: &'a str,
        port: String,
        database: &'a str,
    },
    BrowserIdentityCenter {
        idc_region: &'a str,
        issuer_url: &'a str,
        idp_listen_port: String,
        idc_client_display_name: &'a str,
        idp_response_timeout: String,
        host: &'a str,
        port: String,
        database: &'a str,
    },
    OAuthTokenIdentityCenter {
        token_endpoint: TokenEndpoint,
        host: &'a str,
        port: String,
        database: &'a str,
    },
}

impl<'a> RedshiftAuthIR<'a> {
    pub fn apply(
        self,
        mut builder: DatabaseBuilder,
        _warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<DatabaseBuilder, AuthError> {
        match self {
            Self::Database {
                user,
                password,
                host,
                port,
                database,
            } => {
                builder.with_named_option(CLUSTER_TYPE, REDSHIFT)?;

                let connection_str =
                    build_connection_uri(host, &port, database, Some((user, password)));
                builder.with_parse_uri(connection_str)?;
            }
            Self::Iam {
                region,
                target,
                credentials,
                host,
                port,
                database,
            } => {
                match target {
                    RedshiftIamTarget::Serverless { work_group } => {
                        builder.with_named_option(CLUSTER_TYPE, SERVERLESS)?;
                        if let Some(work_group) = work_group {
                            builder.with_named_option(WORK_GROUP_NAME, work_group)?;
                        }
                        // No username: serverless GetCredentials derives the DB user
                        // from the IAM identity and the driver rejects a non-empty one.
                    }
                    RedshiftIamTarget::Provisioned { cluster_id, user } => {
                        builder.with_named_option(CLUSTER_TYPE, REDSHIFT)?;
                        builder.with_named_option(CLUSTER_IDENTIFIER, cluster_id)?;
                        builder.with_named_option(ADBC_OPTION_USERNAME, user)?;
                    }
                }

                builder.with_named_option(AWS_REGION, region)?;

                // Either both access_key_id + secret_access_key, or fall back to iam_profile.
                // Mirrors dbt-redshift's __iam_user_kwargs in connections.py.
                // https://github.com/dbt-labs/dbt-adapters/blob/b43adf91c22bc91e7935014a687e37aa22115689/dbt-redshift/src/dbt/adapters/redshift/connections.py#L347-L348
                match credentials {
                    RedshiftIamCredentials::AccessKeys {
                        access_key_id,
                        secret_access_key,
                    } => {
                        builder.with_named_option(AWS_ACCESS_KEY_ID, access_key_id)?;
                        builder.with_named_option(AWS_SECRET_ACCESS_KEY, secret_access_key)?;
                    }
                    RedshiftIamCredentials::Profile { iam_profile } => {
                        builder.with_named_option(AWS_PROFILE, iam_profile)?;
                    }
                }

                let connection_str = build_connection_uri(host, &port, database, None);
                builder.with_parse_uri(connection_str)?;
            }
            Self::BrowserIdentityCenter {
                idc_region,
                issuer_url,
                idp_listen_port,
                idc_client_display_name,
                idp_response_timeout,
                host,
                port,
                database,
            } => {
                builder.with_named_option(AUTH_PROVIDER, AUTH_PROVIDER_BROWSER_IDC)?;
                builder.with_named_option(AUTH_IDC_REGION, idc_region)?;
                builder.with_named_option(AUTH_ISSUER_URL, issuer_url)?;
                builder.with_named_option(AUTH_IDP_LISTEN_PORT, idp_listen_port)?;
                builder.with_named_option(AUTH_IDC_CLIENT_DISPLAY_NAME, idc_client_display_name)?;
                builder.with_named_option(AUTH_IDP_RESPONSE_TIMEOUT, idp_response_timeout)?;

                let connection_str = build_connection_uri(host, &port, database, None);
                builder.with_parse_uri(connection_str)?;
            }
            Self::OAuthTokenIdentityCenter {
                token_endpoint,
                host,
                port,
                database,
            } => {
                builder.with_named_option(AUTH_PROVIDER, AUTH_PROVIDER_IDP_TOKEN)?;

                let client = create_token_service_client(token_endpoint).map_err(|e| {
                    AuthError::config(format!("Failed to create token service: {e}"))
                })?;
                let access_token = client.handle_request().map_err(|e| match e {
                    token_service::TokenServiceError::MissingToken => AuthError::config(
                        "access_token missing from IdP token request. \
Please confirm correct configuration of the token_endpoint \
field in profiles.yml and that your IdP can use a refresh token \
to obtain an OIDC-compliant access token.",
                    ),
                    e => AuthError::config(format!(
                        "Failed to fetch token service access token: {e}"
                    )),
                })?;
                builder.with_named_option(AUTH_TOKEN, access_token)?;
                builder.with_named_option(AUTH_TOKEN_TYPE, "EXT_JWT")?;

                let connection_str = build_connection_uri(host, &port, database, None);
                builder.with_parse_uri(connection_str)?;
            }
        }

        Ok(builder)
    }
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<RedshiftAuthIR<'a>, AuthError> {
    // todo: update with Redshift specific configs once available
    let method = config
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("database");

    // Shared required configs. `port` defaults to Redshift's standard 5439,
    // matching the `dbt init` wizard and profile_template.yml. It's the one
    // field normalized to an owned String rather than borrowed, since it
    // legitimately arrives as a YAML int (e.g. `port: 5439`).
    let host = config.require_str("host")?;
    let port = config
        .get_string("port")
        .map(|v| v.into_owned())
        .unwrap_or_else(|| "5439".to_string());
    let database = config
        .get_str("database")
        .or_else(|| config.get_str("dbname"))
        .ok_or_else(|| AuthError::config("missing required field 'database' (or 'dbname')"))?;

    match method {
        "database" => {
            let user = config.require_str("user")?;
            for key in ["iam_profile", "cluster_id"].iter() {
                if config.contains_key(key) {
                    return Err(AuthError::config(format!(
                        "Cannot set '{key}' when 'method' is set to 'database'"
                    )));
                };
            }

            let password = config.require_str("password")?;

            Ok(RedshiftAuthIR::Database {
                user,
                password,
                host,
                port,
                database,
            })
        }
        "iam" => {
            // Serverless is signalled either by the explicit `is_serverless` flag
            // or (matching dbt-redshift) a "redshift-serverless" host. The flag is
            // required for hosts that don't encode it, e.g. an SSM port-forward to
            // localhost. See dbt-core#14621.
            let is_serverless = config.get_bool("is_serverless").unwrap_or(false)
                || host.contains("redshift-serverless");

            let target = if is_serverless {
                RedshiftIamTarget::Serverless {
                    work_group: config.get_str("serverless_work_group"),
                }
            } else {
                RedshiftIamTarget::Provisioned {
                    cluster_id: config.require_str("cluster_id")?,
                    user: config.require_str("user")?,
                }
            };

            let region = config.require_str("region")?;

            // Either both access_key_id + secret_access_key, or fall back to iam_profile.
            let access_key_id = config.get_str("access_key_id");
            let secret_access_key = config.get_str("secret_access_key");
            let credentials = match (access_key_id, secret_access_key) {
                (Some(access_key_id), Some(secret_access_key)) => {
                    RedshiftIamCredentials::AccessKeys {
                        access_key_id,
                        secret_access_key,
                    }
                }
                (Some(_), None) | (None, Some(_)) => {
                    return Err(AuthError::config(
                        "'access_key_id' and 'secret_access_key' are both needed if providing explicit credentials",
                    ));
                }
                (None, None) => RedshiftIamCredentials::Profile {
                    iam_profile: config.require_str("iam_profile")?,
                },
            };

            Ok(RedshiftAuthIR::Iam {
                region,
                target,
                credentials,
                host,
                port,
                database,
            })
        }
        "browser_identity_center" => {
            let idc_region = config.require_str("idc_region")?;
            let issuer_url = config.require_str("issuer_url")?;
            let idp_listen_port = config
                .get_string("idp_listen_port")
                .map(|v| v.into_owned())
                .unwrap_or_else(|| "7890".to_string());
            let idc_client_display_name = config
                .get_str("idc_client_display_name")
                .unwrap_or("Amazon Redshift driver");
            let idp_response_timeout = config
                .get_string("idp_response_timeout")
                .map(|v| v.into_owned())
                .unwrap_or_else(|| "60".to_string());

            Ok(RedshiftAuthIR::BrowserIdentityCenter {
                idc_region,
                issuer_url,
                idp_listen_port,
                idc_client_display_name,
                idp_response_timeout,
                host,
                port,
                database,
            })
        }
        "oauth_token_identity_center" => {
            let token_endpoint_value = config.require("token_endpoint")?;
            let token_endpoint: TokenEndpoint = dbt_yaml::from_value::<TokenEndpoint>(
                token_endpoint_value.clone(),
            )
            .map_err(|e| AuthError::config(format!("Invalid token_endpoint structure: {e}")))?;

            Ok(RedshiftAuthIR::OAuthTokenIdentityCenter {
                token_endpoint,
                host,
                port,
                database,
            })
        }
        method => Err(AuthError::config(format!(
            "Unsupported auth method '{method}' for Redshift. Try 'database' or 'iam' instead."
        ))),
    }
}

fn apply_connection_args(
    config: &AdapterConfig,
    mut builder: DatabaseBuilder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabaseBuilder, AuthError> {
    if let Some(secs) = config.get("connect_timeout").and_then(|v| v.as_i64()) {
        if secs <= 0 {
            return Err(AuthError::config(format!(
                "connect_timeout must be a positive number of seconds, got {secs}"
            )));
        }
        builder.with_named_option(CONNECT_TIMEOUT, format!("{secs}s"))?;
    }

    Ok(builder)
}

pub struct RedshiftAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl RedshiftAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for RedshiftAuth {
    fn backend(&self) -> Backend {
        Backend::Redshift
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        auth_configure_pipeline!(self, &config, parse_auth, apply_connection_args)
    }
}

#[cfg(test)]
mod tests_adbc {
    use super::*;
    use crate::test_options::{other_option_value, uri_value};
    use dbt_adbc::redshift::cluster_type::{REDSHIFT, SERVERLESS};
    use dbt_yaml::Mapping;

    fn iam_base_config() -> Mapping {
        Mapping::from_iter([
            ("method".into(), "iam".into()),
            ("host".into(), "dbt-sandbox-host".into()),
            ("port".into(), "5439".into()),
            ("database".into(), "ci".into()),
            ("user".into(), "awsuser".into()),
            ("region".into(), "us-east-2".into()),
            ("cluster_id".into(), "dbt-sandbox".into()),
        ])
    }

    fn configure(config: Mapping) -> Result<database::Builder, AuthError> {
        RedshiftAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
    }

    fn assert_error_contains(config: Mapping, needle: &str) {
        match configure(config) {
            Err(e) => {
                let msg = format!("{e:?}");
                assert!(
                    msg.contains(needle),
                    "expected error message to contain {needle:?}, got {msg:?}"
                );
            }
            Ok(_) => panic!("Expected error containing {needle:?}, got Ok(_)"),
        }
    }

    #[test]
    fn test_iam_with_profile() {
        let mut config = iam_base_config();
        config.insert("iam_profile".into(), "sandbox-admin".into());

        let builder = configure(config).expect("configure");

        assert_eq!(
            other_option_value(&builder, AWS_PROFILE),
            Some("sandbox-admin")
        );
        assert_eq!(other_option_value(&builder, AWS_REGION), Some("us-east-2"));
        assert_eq!(other_option_value(&builder, CLUSTER_TYPE), Some(REDSHIFT));
        assert_eq!(
            other_option_value(&builder, CLUSTER_IDENTIFIER),
            Some("dbt-sandbox")
        );
        assert_eq!(
            other_option_value(&builder, ADBC_OPTION_USERNAME),
            Some("awsuser")
        );
        assert_eq!(other_option_value(&builder, AWS_ACCESS_KEY_ID), None);
        assert_eq!(other_option_value(&builder, AWS_SECRET_ACCESS_KEY), None);
        let uri = uri_value(&builder);
        assert!(
            uri.starts_with("postgresql://dbt-sandbox-host:5439/ci"),
            "unexpected URI: {uri}"
        );
    }

    #[test]
    fn test_iam_with_access_keys() {
        let mut config = iam_base_config();
        config.insert("access_key_id".into(), "AKIAEXAMPLE".into());
        config.insert("secret_access_key".into(), "sekret".into());

        let builder = configure(config).expect("configure");

        assert_eq!(
            other_option_value(&builder, AWS_ACCESS_KEY_ID),
            Some("AKIAEXAMPLE")
        );
        assert_eq!(
            other_option_value(&builder, AWS_SECRET_ACCESS_KEY),
            Some("sekret")
        );
        assert_eq!(other_option_value(&builder, AWS_PROFILE), None);
    }

    #[test]
    fn test_iam_access_keys_take_precedence_over_profile() {
        let mut config = iam_base_config();
        config.insert("iam_profile".into(), "sandbox-admin".into());
        config.insert("access_key_id".into(), "AKIAEXAMPLE".into());
        config.insert("secret_access_key".into(), "sekret".into());

        let builder = configure(config).expect("configure");

        assert_eq!(
            other_option_value(&builder, AWS_ACCESS_KEY_ID),
            Some("AKIAEXAMPLE")
        );
        assert_eq!(
            other_option_value(&builder, AWS_SECRET_ACCESS_KEY),
            Some("sekret")
        );
        assert_eq!(other_option_value(&builder, AWS_PROFILE), None);
    }

    #[test]
    fn test_iam_missing_secret_access_key_errors() {
        let mut config = iam_base_config();
        config.insert("access_key_id".into(), "AKIAEXAMPLE".into());
        assert_error_contains(
            config,
            "'access_key_id' and 'secret_access_key' are both needed",
        );
    }

    #[test]
    fn test_iam_missing_access_key_id_errors() {
        let mut config = iam_base_config();
        config.insert("secret_access_key".into(), "sekret".into());
        assert_error_contains(
            config,
            "'access_key_id' and 'secret_access_key' are both needed",
        );
    }

    #[test]
    fn test_iam_without_credentials_requires_iam_profile() {
        // No access keys, no iam_profile -> error citing iam_profile.
        let config = iam_base_config();
        assert_error_contains(config, "iam_profile");
    }

    #[test]
    fn test_iam_serverless_skips_cluster_id() {
        let mut config = iam_base_config();
        // The "redshift-serverless" substring is what triggers serverless detection
        // in `parse_auth()`.
        config.insert("host".into(), "example.redshift-serverless.host".into());
        config.remove("cluster_id");
        config.insert("iam_profile".into(), "sandbox-admin".into());

        let builder = configure(config).expect("configure");

        assert_eq!(other_option_value(&builder, CLUSTER_TYPE), Some(SERVERLESS));
        assert_eq!(other_option_value(&builder, CLUSTER_IDENTIFIER), None);
        // Serverless must not forward a username: the driver mints creds from the
        // IAM identity and rejects a non-empty username.
        assert_eq!(other_option_value(&builder, ADBC_OPTION_USERNAME), None);
    }

    #[test]
    fn test_iam_serverless_flag_without_serverless_host() {
        // Regression test for dbt-core#14621: `is_serverless: true` must flag the
        // connection as serverless even when the host doesn't contain
        // "redshift-serverless" (e.g. an SSM port-forward to localhost), must not
        // require cluster_id, and must not require or forward `user`.
        let mut config = iam_base_config();
        config.insert("host".into(), "127.0.0.1".into());
        config.remove("cluster_id");
        config.remove("user");
        config.insert("is_serverless".into(), true.into());
        config.insert("serverless_work_group".into(), "my-workgroup".into());
        config.insert("iam_profile".into(), "sandbox-admin".into());

        let builder = configure(config).expect("configure");

        assert_eq!(other_option_value(&builder, CLUSTER_TYPE), Some(SERVERLESS));
        assert_eq!(other_option_value(&builder, CLUSTER_IDENTIFIER), None);
        assert_eq!(
            other_option_value(&builder, WORK_GROUP_NAME),
            Some("my-workgroup")
        );
        assert_eq!(other_option_value(&builder, ADBC_OPTION_USERNAME), None);
    }

    #[test]
    fn test_iam_explicit_is_serverless_false_still_serverless_when_host_is_serverless() {
        // Gotcha: `is_serverless` is OR-ed with host detection, so an explicit
        // `is_serverless: false` does NOT override a "redshift-serverless" host --
        // the host still wins. cluster_id is therefore not required.
        let mut config = iam_base_config();
        config.insert("host".into(), "example.redshift-serverless.host".into());
        config.remove("cluster_id");
        config.insert("is_serverless".into(), false.into());
        config.insert("iam_profile".into(), "sandbox-admin".into());

        let builder = configure(config).expect("configure");

        assert_eq!(other_option_value(&builder, CLUSTER_TYPE), Some(SERVERLESS));
        assert_eq!(other_option_value(&builder, CLUSTER_IDENTIFIER), None);
        assert_eq!(other_option_value(&builder, ADBC_OPTION_USERNAME), None);
    }

    #[test]
    fn test_iam_serverless_flag_and_host_both_serverless() {
        // Both signals agree: still serverless, no double-counting side effects.
        let mut config = iam_base_config();
        config.insert("host".into(), "example.redshift-serverless.host".into());
        config.remove("cluster_id");
        config.remove("user");
        config.insert("is_serverless".into(), true.into());
        config.insert("iam_profile".into(), "sandbox-admin".into());

        let builder = configure(config).expect("configure");

        assert_eq!(other_option_value(&builder, CLUSTER_TYPE), Some(SERVERLESS));
        assert_eq!(other_option_value(&builder, CLUSTER_IDENTIFIER), None);
    }

    #[test]
    fn test_iam_serverless_without_explicit_work_group_leaves_it_unset() {
        // When serverless is detected but no `serverless_work_group` is given, the
        // driver derives the workgroup from the host, so we must not set it.
        let mut config = iam_base_config();
        config.insert("host".into(), "example.redshift-serverless.host".into());
        config.remove("cluster_id");
        config.insert("iam_profile".into(), "sandbox-admin".into());

        let builder = configure(config).expect("configure");

        assert_eq!(other_option_value(&builder, CLUSTER_TYPE), Some(SERVERLESS));
        assert_eq!(other_option_value(&builder, WORK_GROUP_NAME), None);
    }

    #[test]
    fn test_iam_explicit_is_serverless_false_is_provisioned() {
        // Explicit `false` + non-serverless host => provisioned: cluster_id and
        // user are still required and forwarded.
        let mut config = iam_base_config();
        config.insert("is_serverless".into(), false.into());
        config.insert("iam_profile".into(), "sandbox-admin".into());

        let builder = configure(config).expect("configure");

        assert_eq!(other_option_value(&builder, CLUSTER_TYPE), Some(REDSHIFT));
        assert_eq!(
            other_option_value(&builder, CLUSTER_IDENTIFIER),
            Some("dbt-sandbox")
        );
        assert_eq!(
            other_option_value(&builder, ADBC_OPTION_USERNAME),
            Some("awsuser")
        );
    }

    #[test]
    fn test_iam_provisioned_requires_cluster_id() {
        let mut config = iam_base_config();
        config.remove("cluster_id");
        config.insert("iam_profile".into(), "sandbox-admin".into());
        assert_error_contains(config, "cluster_id");
    }

    #[test]
    fn test_iam_requires_region() {
        let mut config = iam_base_config();
        config.remove("region");
        config.insert("iam_profile".into(), "sandbox-admin".into());
        assert_error_contains(config, "region");
    }

    #[test]
    fn test_iam_requires_user() {
        let mut config = iam_base_config();
        config.remove("user");
        config.insert("iam_profile".into(), "sandbox-admin".into());
        assert_error_contains(config, "user");
    }

    #[test]
    fn test_database_accepts_dbname_alias() {
        let config = Mapping::from_iter([
            ("method".into(), "database".into()),
            ("host".into(), "cluster-host".into()),
            ("port".into(), "5439".into()),
            ("dbname".into(), "dev".into()),
            ("user".into(), "admin".into()),
            ("password".into(), "secretpass".into()),
        ]);
        assert!(
            configure(config).is_ok(),
            "auth should succeed when profile uses 'dbname' instead of 'database'"
        );
    }

    #[test]
    fn test_database_method_rejects_iam_profile() {
        let config = Mapping::from_iter([
            ("method".into(), "database".into()),
            ("host".into(), "cluster-host".into()),
            ("port".into(), "5439".into()),
            ("database".into(), "dev".into()),
            ("user".into(), "admin".into()),
            ("password".into(), "secretpass".into()),
            ("iam_profile".into(), "default".into()),
        ]);
        assert_error_contains(config, "iam_profile");
    }

    #[test]
    fn test_database_auth_uri_contents() {
        let config = Mapping::from_iter([
            ("method".into(), "database".into()),
            ("host".into(), "redshift-cluster.aws.com".into()),
            ("port".into(), "5439".into()),
            ("database".into(), "dev".into()),
            ("user".into(), "admin".into()),
            ("password".into(), "secretpass".into()),
        ]);

        let builder = configure(config).expect("configure");

        let uri = uri_value(&builder);
        assert!(
            uri.starts_with("postgresql://admin:secretpass@redshift-cluster.aws.com:5439/dev"),
            "unexpected URI: {uri}"
        );
    }

    #[test]
    fn test_database_auth_accepts_integer_port() {
        let config: Mapping = dbt_yaml::from_str(
            r#"
method: database
host: redshift-cluster.aws.com
port: 5439
database: dev
user: admin
password: secretpass
"#,
        )
        .expect("parse yaml");

        let builder = configure(config).expect("configure");

        let uri = uri_value(&builder);
        assert!(
            uri.starts_with("postgresql://admin:secretpass@redshift-cluster.aws.com:5439/dev"),
            "unexpected URI: {uri}"
        );
    }

    #[test]
    fn test_database_auth_defaults_missing_port() {
        let config = Mapping::from_iter([
            ("method".into(), "database".into()),
            ("host".into(), "redshift-cluster.aws.com".into()),
            ("database".into(), "dev".into()),
            ("user".into(), "admin".into()),
            ("password".into(), "secretpass".into()),
        ]);

        let builder = configure(config).expect("configure");

        let uri = uri_value(&builder);
        assert!(
            uri.starts_with("postgresql://admin:secretpass@redshift-cluster.aws.com:5439/dev"),
            "unexpected URI: {uri}"
        );
    }

    #[test]
    fn test_browser_identity_center_defaults() {
        let config = Mapping::from_iter([
            ("method".into(), "browser_identity_center".into()),
            ("host".into(), "redshift-cluster.aws.com".into()),
            ("port".into(), "5439".into()),
            ("database".into(), "dev".into()),
            ("idc_region".into(), "us-east-1".into()),
            ("issuer_url".into(), "https://issuer.example.com".into()),
        ]);

        let builder = configure(config).expect("configure");

        assert_eq!(
            other_option_value(&builder, AUTH_PROVIDER),
            Some(AUTH_PROVIDER_BROWSER_IDC)
        );
        assert_eq!(
            other_option_value(&builder, AUTH_IDC_REGION),
            Some("us-east-1")
        );
        assert_eq!(
            other_option_value(&builder, AUTH_ISSUER_URL),
            Some("https://issuer.example.com")
        );
        assert_eq!(
            other_option_value(&builder, AUTH_IDP_LISTEN_PORT),
            Some("7890")
        );
        assert_eq!(
            other_option_value(&builder, AUTH_IDC_CLIENT_DISPLAY_NAME),
            Some("Amazon Redshift driver")
        );
        assert_eq!(
            other_option_value(&builder, AUTH_IDP_RESPONSE_TIMEOUT),
            Some("60")
        );
        let uri = uri_value(&builder);
        assert!(
            uri.starts_with("postgresql://redshift-cluster.aws.com:5439/dev"),
            "unexpected URI: {uri}"
        );
    }

    #[test]
    fn test_browser_identity_center_accepts_integer_ports() {
        let config: Mapping = dbt_yaml::from_str(
            r#"
method: browser_identity_center
host: redshift-cluster.aws.com
port: 5439
database: dev
idc_region: us-east-1
issuer_url: https://issuer.example.com
idp_listen_port: 8080
idp_response_timeout: 90
"#,
        )
        .expect("parse yaml");

        let builder = configure(config).expect("configure");

        assert_eq!(
            other_option_value(&builder, AUTH_IDP_LISTEN_PORT),
            Some("8080")
        );
        assert_eq!(
            other_option_value(&builder, AUTH_IDP_RESPONSE_TIMEOUT),
            Some("90")
        );
    }

    #[test]
    fn test_oauth_token_identity_center_invalid_token_endpoint() {
        let config = Mapping::from_iter([
            ("method".into(), "oauth_token_identity_center".into()),
            ("host".into(), "redshift-cluster.aws.com".into()),
            ("port".into(), "5439".into()),
            ("database".into(), "dev".into()),
            ("token_endpoint".into(), "invalid".into()),
        ]);
        assert_error_contains(config, "Invalid token_endpoint structure:");
    }
}
