use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter, auth_configure_pipeline};
use dbt_adbc::{Backend, database, spark};
pub use dbt_yaml::Value as YmlValue;

use std::collections::HashMap;

#[derive(Debug)]
enum SparkPlatformHint {
    None,
    AwsEmrServerless,
    AwsEmrEks,
}

#[derive(Debug)]
enum ThriftTransportType {
    Binary,
    Http,
}

#[derive(Debug)]
enum ThriftAuthType<'a> {
    Plain { username: &'a str },
    NoSasl,
    Ldap,
    Kerberos { service_name: &'a str },
}

#[derive(Debug)]
enum LivyAuthType {
    Basic,
    AwsSigV4,
}

#[derive(Debug)]
enum SparkConnectAuthType<'a> {
    // Auth is handled at the network/platform layer (mTLS, VPN, service mesh, cloud IAM).
    // No application-level credentials are presented to Spark Connect.
    // FIXME: generalize to arbitrary HTTP/2 headers if there is demand.
    Ambient,
    Token { token: &'a str },
    TokenWithUser { username: &'a str, token: &'a str },
}

#[derive(Debug)]
enum SparkAuthIR<'a> {
    Thrift {
        host: &'a str,
        port: Option<u64>,
        auth: ThriftAuthType<'a>,
        transport: ThriftTransportType,
        session_params: HashMap<&'a str, String>,
    },
    Livy {
        host: &'a str,
        port: Option<u64>,
        auth: LivyAuthType,
        session_ttl: Option<String>,
        session_params: HashMap<&'a str, String>,
    },
    Connect {
        host: &'a str,
        port: Option<u64>,
        auth: SparkConnectAuthType<'a>,
        session_params: HashMap<&'a str, String>,
    },
}

fn apply_session_params(
    params: &HashMap<&str, String>,
    builder: &mut database::Builder,
) -> Result<(), AuthError> {
    for (key, value) in params {
        builder.with_named_option(spark::SESSION_CONFIG_PREFIX.to_owned() + key, value)?;
    }
    Ok(())
}

impl<'a> SparkAuthIR<'a> {
    pub fn apply(
        self,
        mut builder: database::Builder,
        _warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<database::Builder, AuthError> {
        match self {
            SparkAuthIR::Thrift {
                host,
                port,
                auth,
                transport,
                session_params,
            } => {
                builder.with_named_option(spark::HOST, host)?;
                if let Some(port) = port {
                    builder.with_named_option(spark::PORT, port.to_string())?;
                }

                let auth_type = match auth {
                    ThriftAuthType::Plain { username } => {
                        builder.with_named_option(spark::USERNAME, username)?;
                        spark::auth_type::PLAIN
                    }
                    ThriftAuthType::NoSasl => spark::auth_type::NOSASL,
                    ThriftAuthType::Ldap => spark::auth_type::LDAP,
                    ThriftAuthType::Kerberos { service_name } => {
                        builder.with_named_option(spark::KERBEROS_SERVICE_NAME, service_name)?;
                        spark::auth_type::KERBEROS
                    }
                };
                builder.with_named_option(spark::AUTH_TYPE, auth_type)?;

                let transport_api = match transport {
                    ThriftTransportType::Http => spark::transport_api::THRIFT_HTTP,
                    ThriftTransportType::Binary => spark::transport_api::THRIFT_BINARY,
                };
                builder.with_named_option(spark::TRANSPORT_API, transport_api)?;

                apply_session_params(&session_params, &mut builder)?;
            }

            SparkAuthIR::Livy {
                host,
                port,
                auth,
                session_ttl,
                session_params,
            } => {
                builder.with_named_option(spark::TRANSPORT_API, spark::transport_api::LIVY)?;
                builder
                    .with_named_option(spark::livy::SESSION_KIND, spark::livy::session_kind::SQL)?;

                builder.with_named_option(spark::HOST, host)?;
                if let Some(port) = port {
                    builder.with_named_option(spark::PORT, port.to_string())?;
                }

                let auth_type = match auth {
                    LivyAuthType::Basic => spark::auth_type::BASIC,
                    LivyAuthType::AwsSigV4 => spark::auth_type::AWS_SIGV4,
                };
                builder.with_named_option(spark::AUTH_TYPE, auth_type)?;

                if let Some(ttl) = session_ttl {
                    builder.with_named_option(spark::livy::SESSION_TTL, ttl)?;
                }

                apply_session_params(&session_params, &mut builder)?;
            }

            SparkAuthIR::Connect {
                host,
                port,
                auth,
                session_params,
            } => {
                builder.with_named_option(spark::TRANSPORT_API, spark::transport_api::CONNECT)?;

                builder.with_named_option(spark::HOST, host)?;
                if let Some(port) = port {
                    builder.with_named_option(spark::PORT, port.to_string())?;
                }

                match auth {
                    SparkConnectAuthType::Ambient => {
                        builder.with_named_option(spark::AUTH_TYPE, spark::auth_type::NONE)?;
                    }
                    SparkConnectAuthType::Token { token } => {
                        builder.with_named_option(spark::AUTH_TYPE, spark::auth_type::TOKEN)?;
                        builder.with_named_option(spark::PASSWORD, token)?;
                    }
                    SparkConnectAuthType::TokenWithUser { username, token } => {
                        builder.with_named_option(spark::AUTH_TYPE, spark::auth_type::TOKEN)?;
                        builder.with_named_option(spark::PASSWORD, token)?;
                        builder.with_named_option(spark::USERNAME, username)?;
                    }
                }

                apply_session_params(&session_params, &mut builder)?;
            }
        };

        Ok(builder)
    }
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<SparkAuthIR<'a>, AuthError> {
    let method = config
        .get_str("method")
        .ok_or_else(|| AuthError::config("'method' is a required Spark configuration"))?;

    let host = config
        .get_str("host")
        .ok_or_else(|| AuthError::config("'host' is a required Spark configuration"))?;

    let port = match config.get("port") {
        None => Ok(None),
        Some(YmlValue::String(v, _)) => v
            .parse::<u64>()
            .map_err(|_| AuthError::config("'port' must be number"))
            .map(Some),
        Some(YmlValue::Number(v, _)) => v
            .as_u64()
            .map(|v| Ok(Some(v)))
            .unwrap_or_else(|| Err(AuthError::config("'port' must be number"))),
        _ => return Err(AuthError::config("'port' must be number")),
    }?;

    let auth = config.get_str("auth");

    let platform_hint = match config.get_str("platform_hint") {
        None => SparkPlatformHint::None,
        Some(hint) => match hint {
            "aws_emr_serverless" => SparkPlatformHint::AwsEmrServerless,
            "aws_emr_eks" => SparkPlatformHint::AwsEmrEks,
            _ => return Err(AuthError::config("invalid platform hint")),
        },
    };

    let mut session_params = HashMap::new();
    let server_side_parameters = config
        .get("server_side_parameters")
        .or_else(|| config.get("conf"));

    if let Some(ssp) = server_side_parameters {
        let YmlValue::Mapping(ssp, _) = ssp else {
            return Err(AuthError::config(
                "'server_side_parameters' must be mapping",
            ));
        };

        for (key, value) in ssp {
            let YmlValue::String(key, _) = key else {
                return Err(AuthError::config(
                    "'server_side_parameters' key must be string",
                ));
            };

            let value = match value {
                YmlValue::String(v, _) => v.to_string(),
                YmlValue::Number(v, _) => v.to_string(),
                _ => {
                    return Err(AuthError::config(
                        "'server_side_parameters' value must be string or number",
                    ));
                }
            };

            session_params.insert(key.as_str(), value);
        }
    }

    let ir = match method {
        "thrift" | "http" => SparkAuthIR::Thrift {
            host,
            port,
            session_params,
            transport: match method {
                "thrift" => ThriftTransportType::Binary,
                "http" => ThriftTransportType::Http,
                _ => unreachable!(),
            },
            auth: match auth {
                Some("NONE") | Some("PLAIN") | None => {
                    let username = config.get_str("user").ok_or_else(|| {
                        AuthError::config("'user' is required when auth is 'PLAIN' or 'NONE'")
                    })?;
                    Ok(ThriftAuthType::Plain { username })
                }
                Some("NOSASL") => Ok(ThriftAuthType::NoSasl),
                Some("LDAP") => Ok(ThriftAuthType::Ldap),
                Some("KERBEROS") => {
                    let service_name =
                        config.get_str("kerberos_service_name").ok_or_else(|| {
                            AuthError::config(
                                "'kerberos_service_name' is required when auth is 'KERBEROS'",
                            )
                        })?;
                    Ok(ThriftAuthType::Kerberos { service_name })
                }
                Some(_) => Err(AuthError::config("invalid 'auth' for Spark Thrift")),
            }?,
        },
        "livy" => {
            let session_ttl = session_params.remove("livy.server.session.ttl");

            let auth = match auth {
                Some("BASIC") | None => Ok(LivyAuthType::Basic),
                Some("AWS_SIGV4") => Ok(LivyAuthType::AwsSigV4),
                Some(_) => Err(AuthError::config("invalid 'auth' for Spark Livy")),
            }?;

            match platform_hint {
                SparkPlatformHint::None => {}
                SparkPlatformHint::AwsEmrServerless => {
                    if !session_params.contains_key("emr-serverless.session.executionRoleArn") {
                        return Err(AuthError::config(
                            "AWS EMR Serverless requires 'emr-serverless.session.executionRoleArn' Spark session option",
                        ));
                    }

                    if !matches!(auth, LivyAuthType::AwsSigV4) {
                        return Err(AuthError::config(
                            "AWS EMR Serverless requires AWS_SIGV4 auth type",
                        ));
                    }
                }
                SparkPlatformHint::AwsEmrEks => {
                    if !session_params.contains_key("spark.kubernetes.namespace") {
                        return Err(AuthError::config(
                            "AWS EMR on EKS requires 'spark.kubernetes.namespace' Spark session option",
                        ));
                    }

                    if session_ttl.is_some() {
                        return Err(AuthError::config(
                            "AWS EMR on EKS does not support Livy session TTL",
                        ));
                    }
                }
            }

            SparkAuthIR::Livy {
                host,
                port,
                auth,
                session_ttl,
                session_params,
            }
        }
        "spark-connect" | "sc" | "connect" => SparkAuthIR::Connect {
            host,
            port,
            session_params,
            auth: match auth {
                None | Some("NONE") | Some("PLAIN") => Ok(SparkConnectAuthType::Ambient),
                Some("TOKEN") => {
                    let token = config
                        .get_str("password")
                        .or_else(|| config.get_str("token"))
                        .ok_or_else(|| {
                            AuthError::config("'token' or 'password' required when auth is 'TOKEN'")
                        })?;
                    match config.get_str("user") {
                        Some(username) => {
                            Ok(SparkConnectAuthType::TokenWithUser { username, token })
                        }
                        None => Ok(SparkConnectAuthType::Token { token }),
                    }
                }
                Some(_) => Err(AuthError::config("invalid 'auth' for Spark Connect")),
            }?,
        },
        _ => return Err(AuthError::config("unsupported Spark method")),
    };

    Ok(ir)
}

fn apply_connection_args(
    _config: &AdapterConfig,
    builder: database::Builder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<database::Builder, AuthError> {
    Ok(builder)
}

pub struct SparkAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl SparkAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for SparkAuth {
    fn backend(&self) -> Backend {
        Backend::Spark
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        auth_configure_pipeline!(self, &config, parse_auth, apply_connection_args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_options::other_option_value;
    use dbt_yaml::Mapping;

    #[test]
    fn thrift_auth_ok() {
        let cases = [
            (
                "NONE",
                spark::auth_type::PLAIN,
                [("user", "serramatutu")].as_slice(),
            ),
            ("NOSASL", spark::auth_type::NOSASL, [].as_slice()),
            ("LDAP", spark::auth_type::LDAP, [].as_slice()),
            (
                "KERBEROS",
                spark::auth_type::KERBEROS,
                [("kerberos_service_name", "asdf")].as_slice(),
            ),
        ];

        for (auth, option, extra_keys) in cases {
            let config = Mapping::from_iter(
                [
                    ("host".into(), "myhost".into()),
                    ("method".into(), "thrift".into()),
                    ("auth".into(), auth.into()),
                ]
                .into_iter()
                .chain(
                    extra_keys
                        .iter()
                        .map(|(k, v)| ((*k).to_string().into(), (*v).to_string().into())),
                ),
            );

            let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
                .configure(&AdapterConfig::new(config))
                .expect("configure");

            assert_eq!(other_option_value(&builder, spark::AUTH_TYPE), Some(option));
        }
    }

    #[test]
    fn thrift_auth_err() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "thrift".into()),
            ("auth".into(), "invalid".into()),
        ]);
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert_eq!(err.msg(), "invalid 'auth' for Spark Thrift");
    }

    #[test]
    fn livy_auth_ok() {
        let cases = [
            ("BASIC", spark::auth_type::BASIC),
            ("AWS_SIGV4", spark::auth_type::AWS_SIGV4),
        ];

        for (auth, option) in cases {
            let config = Mapping::from_iter([
                ("host".into(), "myhost".into()),
                ("method".into(), "livy".into()),
                ("auth".into(), auth.into()),
            ]);

            let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
                .configure(&AdapterConfig::new(config))
                .expect("configure");

            assert_eq!(other_option_value(&builder, spark::AUTH_TYPE), Some(option));
            assert_eq!(
                other_option_value(&builder, spark::livy::SESSION_KIND),
                Some(spark::livy::session_kind::SQL)
            );
        }
    }

    #[test]
    fn livy_auth_err() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "thrift".into()),
            ("auth".into(), "invalid".into()),
        ]);
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert_eq!(err.msg(), "invalid 'auth' for Spark Thrift");
    }

    #[test]
    fn livy_ttl() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "livy".into()),
            ("auth".into(), "BASIC".into()),
            (
                "server_side_parameters".into(),
                Mapping::from_iter([("livy.server.session.ttl".into(), "1h".into())]).into(),
            ),
        ]);

        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, spark::livy::SESSION_TTL),
            Some("1h")
        );
    }

    #[test]
    fn livy_ttl_conf_alias() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "livy".into()),
            ("auth".into(), "BASIC".into()),
            (
                "conf".into(),
                Mapping::from_iter([("livy.server.session.ttl".into(), "1h".into())]).into(),
            ),
        ]);

        let builder = SparkAuth {}
            .configure(&AdapterConfig::new(config))
            .expect("configure")
            .builder;

        assert_eq!(
            other_option_value(&builder, spark::livy::SESSION_TTL),
            Some("1h")
        );
    }

    #[test]
    fn connect_auth_ok_regression() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "spark-connect".into()),
            ("auth".into(), "NONE".into()),
            ("user".into(), "myuser".into()),
        ]);
        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");
        assert_eq!(
            other_option_value(&builder, spark::AUTH_TYPE),
            Some(spark::auth_type::NONE)
        );

        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "spark-connect".into()),
            ("auth".into(), "TOKEN".into()),
            ("password".into(), "mypass".into()),
            ("user".into(), "myuser".into()),
        ]);
        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");
        assert_eq!(
            other_option_value(&builder, spark::AUTH_TYPE),
            Some(spark::auth_type::TOKEN)
        );
        assert_eq!(
            other_option_value(&builder, spark::PASSWORD),
            Some("mypass")
        );
        assert_eq!(
            other_option_value(&builder, spark::USERNAME),
            Some("myuser")
        );
    }

    #[test]
    fn connect_ambient_auth() {
        for auth_val in [None, Some("NONE"), Some("PLAIN")] {
            let mut config = Mapping::from_iter([
                ("host".into(), "myhost".into()),
                ("method".into(), "spark-connect".into()),
            ]);
            if let Some(a) = auth_val {
                config.insert("auth".into(), a.into());
            }

            let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
                .configure(&AdapterConfig::new(config))
                .expect("configure");

            assert_eq!(
                other_option_value(&builder, spark::AUTH_TYPE),
                Some(spark::auth_type::NONE)
            );
            assert_eq!(other_option_value(&builder, spark::USERNAME), None);
            assert_eq!(other_option_value(&builder, spark::PASSWORD), None);
        }
    }

    #[test]
    fn connect_token_auth_no_user() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "spark-connect".into()),
            ("auth".into(), "TOKEN".into()),
            ("password".into(), "mypass".into()),
        ]);

        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, spark::AUTH_TYPE),
            Some(spark::auth_type::TOKEN)
        );
        assert_eq!(
            other_option_value(&builder, spark::PASSWORD),
            Some("mypass")
        );
        assert_eq!(other_option_value(&builder, spark::USERNAME), None);
    }

    #[test]
    fn connect_token_auth_with_user() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "spark-connect".into()),
            ("auth".into(), "TOKEN".into()),
            ("password".into(), "mypass".into()),
            ("user".into(), "myuser".into()),
        ]);

        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, spark::AUTH_TYPE),
            Some(spark::auth_type::TOKEN)
        );
        assert_eq!(
            other_option_value(&builder, spark::PASSWORD),
            Some("mypass")
        );
        assert_eq!(
            other_option_value(&builder, spark::USERNAME),
            Some("myuser")
        );
    }

    #[test]
    fn connect_token_auth_fallback_to_token_field() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "spark-connect".into()),
            ("auth".into(), "TOKEN".into()),
            ("token".into(), "mytoken".into()),
        ]);

        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, spark::AUTH_TYPE),
            Some(spark::auth_type::TOKEN)
        );
        assert_eq!(
            other_option_value(&builder, spark::PASSWORD),
            Some("mytoken")
        );
    }

    #[test]
    fn connect_token_auth_missing_token_err() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "spark-connect".into()),
            ("auth".into(), "TOKEN".into()),
        ]);

        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert!(err.msg().contains("'token' or 'password' required"));
    }

    #[test]
    fn connect_invalid_auth_err() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "spark-connect".into()),
            ("auth".into(), "KERBEROS".into()),
        ]);

        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert_eq!(err.msg(), "invalid 'auth' for Spark Connect");
    }

    #[test]
    fn invalid_platform_hint() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "thrift".into()),
            ("auth".into(), "NOSASL".into()),
            ("platform_hint".into(), "invalid".into()),
        ]);
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert_eq!(err.msg(), "invalid platform hint");
    }

    #[test]
    fn emr_serverless_no_execution_role_arn() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "livy".into()),
            ("auth".into(), "BASIC".into()),
            ("platform_hint".into(), "aws_emr_serverless".into()),
        ]);
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert!(
            err.msg()
                .contains("emr-serverless.session.executionRoleArn")
        );
    }

    #[test]
    fn emr_serverless_no_sigv4() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "livy".into()),
            ("auth".into(), "BASIC".into()),
            ("platform_hint".into(), "aws_emr_serverless".into()),
            (
                "server_side_parameters".into(),
                Mapping::from_iter([(
                    "emr-serverless.session.executionRoleArn".into(),
                    "arn::my_role".into(),
                )])
                .into(),
            ),
        ]);
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert!(err.msg().contains("AWS_SIGV4"));
    }

    #[test]
    fn emr_eks_no_kubernetes_namespace() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "livy".into()),
            ("auth".into(), "AWS_SIGV4".into()),
            ("platform_hint".into(), "aws_emr_eks".into()),
        ]);
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert!(err.msg().contains("spark.kubernetes.namespace"));
    }

    #[test]
    fn emr_eks_livy_ttl() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "livy".into()),
            ("auth".into(), "AWS_SIGV4".into()),
            ("platform_hint".into(), "aws_emr_eks".into()),
            (
                "server_side_parameters".into(),
                Mapping::from_iter([
                    ("spark.kubernetes.namespace".into(), "asdf".into()),
                    ("livy.server.session.ttl".into(), "1h".into()),
                ])
                .into(),
            ),
        ]);
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert!(err.msg().contains("TTL"));
    }
}
