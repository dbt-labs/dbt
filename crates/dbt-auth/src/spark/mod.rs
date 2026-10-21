use crate::config::yml_value_to_string;
use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter, auth_configure_pipeline};
use dbt_adbc::{Backend, database, spark};

pub use dbt_yaml::Value as YmlValue;

use url::Url;

const FABRIC_DEFAULT_ENDPOINT: &str = "https://api.fabric.microsoft.com/v1";
const FABRIC_LIVY_API_VERSION: &str = "2023-12-01";
// based on: dbt-labs/adbc-spark: go/sparkutil/options.go
const FABRIC_DEFAULT_TOKEN_SCOPE: &str = "https://api.fabric.microsoft.com/.default";

// https://github.com/dbt-labs/adbc-spark/blob/ee15c868eb93e18fbe81bb973e24cd1deed8bfbc/go/connectclient/client/channel/channel.go#L160-L162
const SPARK_CONNECT_DEFAULT_PORT: u64 = 15002;
// https://github.com/dbt-labs/dbt-spark/blob/42700b5d0eee9b296942c22ae2aa33240177d082/dbt/adapters/spark/connections.py#L77
const THRIFT_BINARY_DEFAULT_PORT: u64 = 443;

#[derive(Debug)]
enum SparkPlatformHint {
    None,
    AwsEmrServerless,
    AwsEmrEks,
    FabricLakehouse,
}

#[derive(Debug)]
enum PortType {
    SchemeDefault,
    Explicit(u64),
}

#[derive(Debug)]
enum ThriftTransportType {
    Binary { port: u64 },
    Http { port: PortType },
}

#[derive(Debug)]
enum ThriftAuthType<'a> {
    Plain { username: &'a str },
    NoSasl,
    Ldap,
    Kerberos { service_name: &'a str },
}

#[derive(Debug)]
enum LivySessionTtl<'a> {
    ServerManaged,
    Explicit(&'a str),
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
enum ManagedIdentityType<'a> {
    /// The system-assigned identity of the host resource.
    System,
    /// A user-assigned identity, selected by its client ID.
    User { client_id: &'a str },
}

#[derive(Debug)]
enum FabricAuthType<'a> {
    /// The local Azure CLI (`az login`) context.
    Cli,
    /// The azidentity `DefaultAzureCredential` chain.
    Default,
    /// Credentials from `AZURE_*` environment variables only.
    Environment,
    /// An Azure managed identity.
    ManagedIdentity(ManagedIdentityType<'a>),
    /// Service principal with a client secret (profile: `SPN`).
    ClientSecret {
        tenant_id: &'a str,
        client_id: &'a str,
        client_secret: &'a str,
    },
}

#[derive(Debug)]
struct SessionParamsView<'a>(Vec<(&'a str, &'a YmlValue)>);

impl<'a> SessionParamsView<'a> {
    const SKIP_FIELDS: &'static [&'static str] = &[spark::session_config::LIVY_SESSION_TTL];

    fn try_new(config: &'a AdapterConfig) -> Result<Self, AuthError> {
        let Some(ssp) = config.get("server_side_parameters") else {
            return Ok(Self(Vec::new()));
        };
        let YmlValue::Mapping(ssp, _) = ssp else {
            return Err(AuthError::config(
                "'server_side_parameters' must be mapping",
            ));
        };

        let mut params = Vec::with_capacity(ssp.len());

        for (key, value) in ssp {
            let Some(key) = key.as_str() else {
                return Err(AuthError::config(
                    "'server_side_parameters' key must be string",
                ));
            };

            if let YmlValue::Sequence(..) | YmlValue::Mapping(..) | YmlValue::Tagged(..) = value {
                return Err(AuthError::config(
                    "'server_side_parameters' value must be a scalar",
                ));
            }

            if !Self::SKIP_FIELDS.contains(&key) {
                params.push((key, value));
            }
        }

        Ok(Self(params))
    }

    fn apply(self, builder: &mut database::Builder) -> Result<(), AuthError> {
        for (key, value) in self.0 {
            builder.with_named_option(
                spark::SESSION_CONFIG_PREFIX.to_owned() + key,
                yml_value_to_string(value),
            )?;
        }
        Ok(())
    }
}

#[derive(Debug)]
enum SparkAuthIR<'a> {
    Thrift {
        host: &'a str,
        auth: ThriftAuthType<'a>,
        transport: ThriftTransportType,
        session_params: SessionParamsView<'a>,
    },
    Livy {
        host: &'a str,
        port: PortType,
        auth: LivyAuthType,
        session_ttl: LivySessionTtl<'a>,
        session_params: SessionParamsView<'a>,
    },
    Connect {
        host: &'a str,
        port: u64,
        auth: SparkConnectAuthType<'a>,
        session_params: SessionParamsView<'a>,
    },
    /// Microsoft Fabric Lakehouse. Successor to the `dbt-fabricspark` adapter.
    Fabric {
        endpoint: Url,
        workspace_id: &'a str,
        lakehouse_id: &'a str,
        auth: FabricAuthType<'a>,
        token_scope: &'a str,
        session_params: SessionParamsView<'a>,
    },
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
                auth,
                transport,
                session_params,
            } => {
                builder.with_named_option(spark::HOST, host)?;

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

                match transport {
                    ThriftTransportType::Binary { port } => {
                        builder.with_named_option(
                            spark::TRANSPORT_API,
                            spark::transport_api::THRIFT_BINARY,
                        )?;
                        builder.with_named_option(spark::PORT, port.to_string())?;
                    }
                    ThriftTransportType::Http { port } => {
                        builder.with_named_option(
                            spark::TRANSPORT_API,
                            spark::transport_api::THRIFT_HTTP,
                        )?;
                        match port {
                            PortType::SchemeDefault => {}
                            PortType::Explicit(port) => {
                                builder.with_named_option(spark::PORT, port.to_string())?;
                            }
                        }
                    }
                }

                session_params.apply(&mut builder)?;
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
                match port {
                    PortType::SchemeDefault => {}
                    PortType::Explicit(port) => {
                        builder.with_named_option(spark::PORT, port.to_string())?;
                    }
                }

                let auth_type = match auth {
                    LivyAuthType::Basic => spark::auth_type::BASIC,
                    LivyAuthType::AwsSigV4 => spark::auth_type::AWS_SIGV4,
                };
                builder.with_named_option(spark::AUTH_TYPE, auth_type)?;

                match session_ttl {
                    LivySessionTtl::ServerManaged => {}
                    LivySessionTtl::Explicit(ttl) => {
                        builder.with_named_option(spark::livy::SESSION_TTL, ttl)?;
                    }
                }

                session_params.apply(&mut builder)?;
            }

            SparkAuthIR::Connect {
                host,
                port,
                auth,
                session_params,
            } => {
                builder.with_named_option(spark::TRANSPORT_API, spark::transport_api::CONNECT)?;

                builder.with_named_option(spark::HOST, host)?;
                builder.with_named_option(spark::PORT, port.to_string())?;

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

                session_params.apply(&mut builder)?;
            }

            SparkAuthIR::Fabric {
                endpoint,
                workspace_id,
                lakehouse_id,
                auth,
                token_scope,
                session_params,
            } => {
                builder.with_named_option(spark::TRANSPORT_API, spark::transport_api::LIVY)?;
                builder
                    .with_named_option(spark::livy::SESSION_KIND, spark::livy::session_kind::SQL)?;

                let host = endpoint.origin().ascii_serialization();
                let path_prefix = endpoint.path().trim_end_matches('/');
                builder.with_named_option(spark::HOST, host)?;
                builder.with_named_option(
                    spark::livy::BASE_URL,
                    format!(
                        "{path_prefix}/workspaces/{workspace_id}/lakehouses/{lakehouse_id}/livyapi/versions/{FABRIC_LIVY_API_VERSION}"
                    ),
                )?;

                builder.with_named_option(spark::AUTH_TYPE, spark::auth_type::AZURE_TOKEN)?;
                builder.with_named_option(spark::livy::azure::TOKEN_SCOPE, token_scope)?;
                match auth {
                    FabricAuthType::Cli => {
                        builder.with_named_option(
                            spark::livy::azure::CREDENTIAL,
                            spark::livy::azure::credential::AZ_CLI,
                        )?;
                    }
                    FabricAuthType::Default => {
                        builder.with_named_option(
                            spark::livy::azure::CREDENTIAL,
                            spark::livy::azure::credential::DEFAULT,
                        )?;
                    }
                    FabricAuthType::Environment => {
                        builder.with_named_option(
                            spark::livy::azure::CREDENTIAL,
                            spark::livy::azure::credential::ENVIRONMENT,
                        )?;
                    }
                    FabricAuthType::ManagedIdentity(identity) => {
                        builder.with_named_option(
                            spark::livy::azure::CREDENTIAL,
                            spark::livy::azure::credential::MANAGED_IDENTITY,
                        )?;
                        match identity {
                            ManagedIdentityType::System => {}
                            ManagedIdentityType::User { client_id } => {
                                builder.with_named_option(spark::USERNAME, client_id)?;
                            }
                        }
                    }
                    FabricAuthType::ClientSecret {
                        tenant_id,
                        client_id,
                        client_secret,
                    } => {
                        builder.with_named_option(
                            spark::livy::azure::CREDENTIAL,
                            spark::livy::azure::credential::SERVICE_PRINCIPAL,
                        )?;
                        builder.with_named_option(
                            spark::USERNAME,
                            format!("{client_id}@{tenant_id}"),
                        )?;
                        builder.with_named_option(spark::PASSWORD, client_secret)?;
                    }
                }

                session_params.apply(&mut builder)?;
            }
        };

        Ok(builder)
    }
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    warning_printer: &dyn AuthWarningPrinter,
) -> Result<SparkAuthIR<'a>, AuthError> {
    let method = config
        .get_str("method")
        .ok_or_else(|| AuthError::config("'method' is a required Spark configuration"))?;

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

    let platform_hint = match config.get_str("platform_hint") {
        None => SparkPlatformHint::None,
        Some(hint) => match hint {
            "aws_emr_serverless" => SparkPlatformHint::AwsEmrServerless,
            "aws_emr_eks" => SparkPlatformHint::AwsEmrEks,
            "fabric_lakehouse" => SparkPlatformHint::FabricLakehouse,
            _ => return Err(AuthError::config("invalid platform hint")),
        },
    };

    let ir = match method {
        "thrift" | "http" => {
            let auth = match config.get_str("auth") {
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
            }?;

            let transport = match method {
                "thrift" => ThriftTransportType::Binary {
                    port: match port {
                        Some(port) => port,
                        // https://github.com/dbt-labs/adbc-spark/blob/ee15c868eb93e18fbe81bb973e24cd1deed8bfbc/go/internal/thriftimpl/client.go#L143
                        None => {
                            warning_printer.warn(
                                "No 'port' is set for method 'thrift', so port 443 will be used. Set the 'port' field to choose the port explicitly.",
                            );
                            THRIFT_BINARY_DEFAULT_PORT
                        }
                    },
                },
                "http" => ThriftTransportType::Http {
                    port: match port {
                        Some(port) => PortType::Explicit(port),
                        // https://github.com/dbt-labs/adbc-spark/blob/ee15c868eb93e18fbe81bb973e24cd1deed8bfbc/go/internal/thriftimpl/client.go#L115-L119
                        // https://github.com/dbt-labs/adbc-spark/blob/ee15c868eb93e18fbe81bb973e24cd1deed8bfbc/go/client.go#L434
                        None => PortType::SchemeDefault,
                    },
                },
                _ => unreachable!(),
            };

            SparkAuthIR::Thrift {
                host: config
                    .get_str("host")
                    .ok_or_else(|| AuthError::config("'host' is a required Spark configuration"))?,
                transport,
                auth,
                session_params: SessionParamsView::try_new(config)?,
            }
        }
        "livy" => {
            macro_rules! livy_common {
                ($session_ttl:ident, $auth:ident, $port:ident) => {
                    let ssp = config
                        .get("server_side_parameters")
                        .and_then(YmlValue::as_mapping);

                    let $session_ttl =
                        match ssp.and_then(|ssp| ssp.get(spark::session_config::LIVY_SESSION_TTL)) {
                            // https://github.com/dbt-labs/adbc-spark/blob/ee15c868eb93e18fbe81bb973e24cd1deed8bfbc/go/internal/livyimpl/client.go#L377-L379
                            None => LivySessionTtl::ServerManaged,
                            Some(ttl) => LivySessionTtl::Explicit(ttl.as_str().ok_or_else(|| {
                                AuthError::config("'livy.server.session.ttl' must be a string")
                            })?),
                        };

                    let $auth = match config.get_str("auth") {
                        Some("BASIC") | None => Ok(LivyAuthType::Basic),
                        Some("AWS_SIGV4") => Ok(LivyAuthType::AwsSigV4),
                        Some(_) => Err(AuthError::config("invalid 'auth' for Spark Livy")),
                    }?;

                    let $port = match port {
                        Some(port) => PortType::Explicit(port),
                        // https://github.com/dbt-labs/adbc-spark/blob/ee15c868eb93e18fbe81bb973e24cd1deed8bfbc/go/client.go#L228-L234
                        // https://github.com/dbt-labs/adbc-spark/blob/ee15c868eb93e18fbe81bb973e24cd1deed8bfbc/go/client.go#L217
                        None => PortType::SchemeDefault,
                    };
                };
            }

            match platform_hint {
                SparkPlatformHint::FabricLakehouse => {
                    if config.get("host").is_some() || config.get("port").is_some() {
                        return Err(AuthError::config(
                            "'host' and 'port' are not supported when platform_hint is 'fabric_lakehouse'; use 'endpoint'",
                        ));
                    }
                    let workspace_id = config.get_str("workspaceid").ok_or_else(|| {
                        AuthError::config(
                            "'workspaceid' is required when platform_hint is 'fabric_lakehouse'",
                        )
                    })?;
                    let lakehouse_id = config.get_str("lakehouseid").ok_or_else(|| {
                        AuthError::config(
                            "'lakehouseid' is required when platform_hint is 'fabric_lakehouse'",
                        )
                    })?;
                    let endpoint = Url::parse(
                        config
                            .get_str("endpoint")
                            .unwrap_or(FABRIC_DEFAULT_ENDPOINT),
                    )
                    .map_err(|e| {
                        AuthError::config(format!(
                            "invalid 'endpoint' for Fabric (a scheme such as https:// is required): {e}"
                        ))
                    })?;
                    if endpoint.scheme() != "https" {
                        return Err(AuthError::config("'endpoint' for Fabric must use https"));
                    }

                    let auth = match config
                        .get_str("authentication")
                        .map(str::to_ascii_lowercase)
                        .as_deref()
                    {
                        None | Some("cli") => FabricAuthType::Cli,
                        Some("spn") | Some("client_secret") => FabricAuthType::ClientSecret {
                            tenant_id: config.get_str("tenant_id").ok_or_else(|| {
                                AuthError::config(
                                    "'tenant_id' is required when authentication is 'SPN'",
                                )
                            })?,
                            client_id: config.get_str("client_id").ok_or_else(|| {
                                AuthError::config(
                                    "'client_id' is required when authentication is 'SPN'",
                                )
                            })?,
                            client_secret: config.get_str("client_secret").ok_or_else(|| {
                                AuthError::config(
                                    "'client_secret' is required when authentication is 'SPN'",
                                )
                            })?,
                        },
                        Some("default") => FabricAuthType::Default,
                        Some("environment") => FabricAuthType::Environment,
                        Some("managed_identity") => {
                            FabricAuthType::ManagedIdentity(match config.get_str("client_id") {
                                Some(client_id) => ManagedIdentityType::User { client_id },
                                None => ManagedIdentityType::System,
                            })
                        }
                        Some(_) => {
                            return Err(AuthError::config(
                                "invalid 'authentication' for fabricspark: must be one of \
[CLI, SPN, default, environment, managed_identity]",
                            ));
                        }
                    };

                    // FIXME(versusfacit): `session.ttl` is dropped for Fabric (see `SKIP_FIELDS`)
                    // and never applied. Investigate whether Fabric's Livy API honors `ttl` on
                    // session creation; if so, add a `LivySessionTtl` field here like Livy.
                    SparkAuthIR::Fabric {
                        endpoint,
                        workspace_id,
                        lakehouse_id,
                        auth,
                        token_scope: config
                            .get_str("token_scope")
                            .unwrap_or(FABRIC_DEFAULT_TOKEN_SCOPE),
                        session_params: SessionParamsView::try_new(config)?,
                    }
                }
                SparkPlatformHint::None => {
                    livy_common!(session_ttl, auth, port);

                    if config.get("workspaceid").is_some() || config.get("lakehouseid").is_some() {
                        return Err(AuthError::config(
                            "'workspaceid' and 'lakehouseid' are set. If you're trying to connect to a Fabric Lakehouse, add 'platform_hint: fabric_lakehouse' alongside 'method: livy'.",
                        ));
                    }

                    SparkAuthIR::Livy {
                        host: config.get_str("host").ok_or_else(|| {
                            AuthError::config("'host' is a required Spark configuration")
                        })?,
                        port,
                        auth,
                        session_ttl,
                        session_params: SessionParamsView::try_new(config)?,
                    }
                }
                SparkPlatformHint::AwsEmrServerless => {
                    livy_common!(session_ttl, auth, port);

                    let ssp = config
                        .get("server_side_parameters")
                        .and_then(YmlValue::as_mapping);

                    if !ssp.is_some_and(|ssp| {
                        ssp.contains_key(spark::session_config::EMR_SERVERLESS_EXECUTION_ROLE_ARN)
                    }) {
                        return Err(AuthError::config(
                            "AWS EMR Serverless requires 'emr-serverless.session.executionRoleArn' Spark session option",
                        ));
                    }

                    if !matches!(auth, LivyAuthType::AwsSigV4) {
                        return Err(AuthError::config(
                            "AWS EMR Serverless requires AWS_SIGV4 auth type",
                        ));
                    }

                    SparkAuthIR::Livy {
                        host: config.get_str("host").ok_or_else(|| {
                            AuthError::config("'host' is a required Spark configuration")
                        })?,
                        port,
                        auth,
                        session_ttl,
                        session_params: SessionParamsView::try_new(config)?,
                    }
                }
                SparkPlatformHint::AwsEmrEks => {
                    livy_common!(session_ttl, auth, port);

                    let ssp = config
                        .get("server_side_parameters")
                        .and_then(YmlValue::as_mapping);

                    if !ssp.is_some_and(|ssp| {
                        ssp.contains_key(spark::session_config::KUBERNETES_NAMESPACE)
                    }) {
                        return Err(AuthError::config(
                            "AWS EMR on EKS requires 'spark.kubernetes.namespace' Spark session option",
                        ));
                    }

                    if matches!(session_ttl, LivySessionTtl::Explicit(_)) {
                        return Err(AuthError::config(
                            "AWS EMR on EKS does not support Livy session TTL",
                        ));
                    }

                    SparkAuthIR::Livy {
                        host: config.get_str("host").ok_or_else(|| {
                            AuthError::config("'host' is a required Spark configuration")
                        })?,
                        port,
                        auth,
                        session_ttl,
                        session_params: SessionParamsView::try_new(config)?,
                    }
                }
            }
        }
        "spark-connect" | "sc" | "connect" => {
            let host = config
                .get_str("host")
                .ok_or_else(|| AuthError::config("'host' is a required Spark configuration"))?;

            // https://github.com/dbt-labs/adbc-spark/blob/ee15c868eb93e18fbe81bb973e24cd1deed8bfbc/go/internal/connectimpl/client.go#L106-L118
            let (host, port) = match (host.rsplit_once(':'), port) {
                (None, Some(port)) => (host, port),
                // https://github.com/dbt-labs/adbc-spark/blob/ee15c868eb93e18fbe81bb973e24cd1deed8bfbc/go/connectclient/client/channel/channel.go#L160-L162
                (None, None) => (host, SPARK_CONNECT_DEFAULT_PORT),
                (Some((host, host_port)), None) => {
                    let port = host_port.parse::<u64>().map_err(|_| {
                        AuthError::config("the port embedded in 'host' must be a number")
                    })?;
                    warning_printer.warn(
                        "The port is set inside 'host'. Move it into the 'port' field instead.",
                    );
                    (host, port)
                }
                (Some(_), Some(_)) => {
                    return Err(AuthError::config(
                        "'host' contains a port and 'port' is also set. Remove the port from 'host'.",
                    ));
                }
            };

            let auth = match config.get_str("auth") {
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
            }?;

            SparkAuthIR::Connect {
                host,
                port,
                auth,
                session_params: SessionParamsView::try_new(config)?,
            }
        }
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

    fn fabric_base_config() -> Mapping {
        Mapping::from_iter([
            ("method".into(), "livy".into()),
            ("platform_hint".into(), "fabric_lakehouse".into()),
            ("workspaceid".into(), "ws-guid".into()),
            ("lakehouseid".into(), "lh-guid".into()),
            ("schema".into(), "dbo".into()),
        ])
    }

    #[test]
    fn fabric_defaults_to_cli_credential_and_fabric_endpoint() {
        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(fabric_base_config()))
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
            other_option_value(&builder, spark::livy::azure::TOKEN_SCOPE),
            Some("https://api.fabric.microsoft.com/.default")
        );
        assert_eq!(
            other_option_value(&builder, spark::TRANSPORT_API),
            Some(spark::transport_api::LIVY)
        );
    }

    #[test]
    fn fabric_spn_credential() {
        let mut config = fabric_base_config();
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
        let mut config = fabric_base_config();
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
        let config = Mapping::from_iter([
            ("method".into(), "livy".into()),
            ("platform_hint".into(), "fabric_lakehouse".into()),
            ("lakehouseid".into(), "lh-guid".into()),
        ]);
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert!(err.msg().contains("workspaceid"));
    }

    #[test]
    fn fabric_custom_endpoint_and_scope() {
        let mut config = fabric_base_config();
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
    fn fabric_default_credential() {
        let mut config = fabric_base_config();
        config.insert("authentication".into(), "default".into());

        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, spark::livy::azure::CREDENTIAL),
            Some(spark::livy::azure::credential::DEFAULT)
        );
    }

    #[test]
    fn fabric_environment_credential() {
        let mut config = fabric_base_config();
        config.insert("authentication".into(), "environment".into());

        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, spark::livy::azure::CREDENTIAL),
            Some(spark::livy::azure::credential::ENVIRONMENT)
        );
    }

    #[test]
    fn fabric_system_assigned_managed_identity() {
        let mut config = fabric_base_config();
        config.insert("authentication".into(), "managed_identity".into());

        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, spark::livy::azure::CREDENTIAL),
            Some(spark::livy::azure::credential::MANAGED_IDENTITY)
        );
        assert_eq!(other_option_value(&builder, spark::USERNAME), None);
    }

    #[test]
    fn fabric_user_assigned_managed_identity() {
        let mut config = fabric_base_config();
        config.insert("authentication".into(), "managed_identity".into());
        config.insert("client_id".into(), "client".into());

        let builder = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, spark::livy::azure::CREDENTIAL),
            Some(spark::livy::azure::credential::MANAGED_IDENTITY)
        );
        assert_eq!(
            other_option_value(&builder, spark::USERNAME),
            Some("client")
        );
    }

    #[test]
    fn fabric_endpoint_must_be_https() {
        let mut config = fabric_base_config();
        config.insert(
            "endpoint".into(),
            "http://api.fabric.microsoft.com/v1".into(),
        );
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert!(err.msg().contains("must use https"));
    }

    #[test]
    fn fabric_rejects_host_and_port() {
        for key in ["host", "port"] {
            let mut config = fabric_base_config();
            config.insert(key.into(), "443".into());
            let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
                .configure(&AdapterConfig::new(config))
                .expect_err("configure should fail");
            assert!(err.msg().contains("'host' and 'port' are not supported"));
        }
    }

    #[test]
    fn fabric_endpoint_without_scheme_err() {
        let mut config = fabric_base_config();
        config.insert("endpoint".into(), "api.fabric.microsoft.com/v1".into());
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert!(err.msg().contains("invalid 'endpoint'"));
    }

    #[test]
    fn fabric_invalid_authentication_err() {
        let mut config = fabric_base_config();
        config.insert("authentication".into(), "carrier_pigeon".into());
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert!(err.msg().contains("invalid 'authentication'"));
    }

    #[test]
    fn plain_spark_profiles_are_not_routed_to_fabric() {
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

    struct RecordingWarningPrinter(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

    impl AuthWarningPrinter for RecordingWarningPrinter {
        fn warn(&self, message: &str) {
            self.0.lock().unwrap().push(message.to_string());
        }
    }

    fn configure_recording_warnings(config: Mapping) -> (database::Builder, Vec<String>) {
        let warnings = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let builder = SparkAuth::new(Box::new(RecordingWarningPrinter(warnings.clone())))
            .configure(&AdapterConfig::new(config))
            .expect("configure");
        let warnings = warnings.lock().unwrap().clone();
        (builder, warnings)
    }

    #[test]
    fn connect_default_port() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "spark-connect".into()),
        ]);
        let (builder, warnings) = configure_recording_warnings(config);
        assert_eq!(other_option_value(&builder, spark::HOST), Some("myhost"));
        assert_eq!(other_option_value(&builder, spark::PORT), Some("15002"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn connect_port_in_host_is_split_with_warning() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost:15003".into()),
            ("method".into(), "spark-connect".into()),
        ]);
        let (builder, warnings) = configure_recording_warnings(config);
        assert_eq!(other_option_value(&builder, spark::HOST), Some("myhost"));
        assert_eq!(other_option_value(&builder, spark::PORT), Some("15003"));
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn connect_port_in_host_and_port_field_err() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost:15003".into()),
            ("port".into(), 15004.into()),
            ("method".into(), "spark-connect".into()),
        ]);
        let err = SparkAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert!(err.msg().contains("'port' is also set"));
    }

    #[test]
    fn thrift_binary_default_port_with_warning() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "thrift".into()),
            ("auth".into(), "NOSASL".into()),
        ]);
        let (builder, warnings) = configure_recording_warnings(config);
        assert_eq!(other_option_value(&builder, spark::PORT), Some("443"));
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn thrift_binary_explicit_port_no_warning() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("port".into(), 10000.into()),
            ("method".into(), "thrift".into()),
            ("auth".into(), "NOSASL".into()),
        ]);
        let (builder, warnings) = configure_recording_warnings(config);
        assert_eq!(other_option_value(&builder, spark::PORT), Some("10000"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn thrift_http_no_port_uses_scheme_default() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "http".into()),
            ("auth".into(), "NOSASL".into()),
        ]);
        let (builder, warnings) = configure_recording_warnings(config);
        assert_eq!(other_option_value(&builder, spark::PORT), None);
        assert!(warnings.is_empty());
    }

    #[test]
    fn livy_no_port_uses_scheme_default() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "livy".into()),
        ]);
        let (builder, warnings) = configure_recording_warnings(config);
        assert_eq!(other_option_value(&builder, spark::PORT), None);
        assert!(warnings.is_empty());
    }

    #[test]
    fn livy_no_ttl_is_server_managed() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "livy".into()),
        ]);
        let (builder, _) = configure_recording_warnings(config);
        assert_eq!(other_option_value(&builder, spark::livy::SESSION_TTL), None);
    }

    #[test]
    fn livy_ttl_not_forwarded_as_session_config() {
        let config = Mapping::from_iter([
            ("host".into(), "myhost".into()),
            ("method".into(), "livy".into()),
            (
                "server_side_parameters".into(),
                Mapping::from_iter([
                    ("livy.server.session.ttl".into(), "1h".into()),
                    ("spark.executor.memory".into(), "4g".into()),
                ])
                .into(),
            ),
        ]);
        let (builder, _) = configure_recording_warnings(config);
        assert_eq!(
            other_option_value(&builder, spark::livy::SESSION_TTL),
            Some("1h")
        );
        assert_eq!(
            other_option_value(&builder, "spark.opt.livy.server.session.ttl"),
            None
        );
        assert_eq!(
            other_option_value(&builder, "spark.opt.spark.executor.memory"),
            Some("4g")
        );
    }
}
