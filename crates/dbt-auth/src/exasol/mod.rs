use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter, auth_configure_pipeline};
use database::Builder as DatabaseBuilder;
use dbt_adbc::{Backend, database};
use std::borrow::Cow;

const DEFAULT_HOST: &str = "localhost";
const DEFAULT_PORT: &str = "8563";

#[derive(Debug)]
enum ExasolAuthIR<'a> {
    UserPass {
        user: &'a str,
        password: Cow<'a, str>,
        host: &'a str,
        port: Cow<'a, str>,
        schema: Option<&'a str>,
        encryption: bool,
        certificate_validation: bool,
        certificate_fingerprint: Option<&'a str>,
        connection_timeout: Option<Cow<'a, str>>,
        query_timeout: Option<Cow<'a, str>>,
        idle_timeout: Option<Cow<'a, str>>,
    },
}

impl<'a> ExasolAuthIR<'a> {
    pub fn apply(
        self,
        mut builder: DatabaseBuilder,
        _warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<DatabaseBuilder, AuthError> {
        match self {
            Self::UserPass {
                user,
                password,
                host,
                port,
                schema,
                encryption,
                certificate_validation,
                certificate_fingerprint,
                connection_timeout,
                query_timeout,
                idle_timeout,
            } => {
                let mut uri = format!("exasol://{host}:{port}");

                if let Some(schema) = schema {
                    uri.push('/');
                    uri.push_str(schema);
                }

                let mut params: Vec<String> = Vec::new();

                if !encryption {
                    params.push("tls=0".to_string());
                }

                if !certificate_validation {
                    params.push("validateservercertificate=0".to_string());
                }

                if let Some(fingerprint) = certificate_fingerprint {
                    params.push(format!("certificatefingerprint={fingerprint}"));
                }

                if let Some(timeout) = &connection_timeout {
                    params.push(format!("timeout={timeout}"));
                }

                if let Some(timeout) = &query_timeout {
                    params.push(format!("query_timeout={timeout}"));
                }

                if let Some(timeout) = &idle_timeout {
                    params.push(format!("idle_timeout={timeout}"));
                }

                if !params.is_empty() {
                    uri.push('?');
                    uri.push_str(&params.join("&"));
                }

                builder.with_parse_uri(uri)?;
                builder.with_username(user);
                builder.with_password(password.as_ref());
            }
        }

        Ok(builder)
    }
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<ExasolAuthIR<'a>, AuthError> {
    let encryption = config
        .get_string("encryption")
        .map(|s| s != "false" && s != "0" && s != "False")
        .unwrap_or(true);

    let certificate_validation = config
        .get_string("certificate_validation")
        .map(|s| s != "false" && s != "0" && s != "False")
        .unwrap_or(true);

    let user = config
        .get_str("user")
        .ok_or_else(|| AuthError::config("Exasol requires 'user' in profile configuration"))?;

    let password = config
        .get_string("password")
        .ok_or_else(|| AuthError::config("Exasol requires 'password' in profile configuration"))?;

    Ok(ExasolAuthIR::UserPass {
        user,
        password,
        host: config.get_str("host").unwrap_or(DEFAULT_HOST),
        port: config
            .get_string("port")
            .unwrap_or(Cow::Borrowed(DEFAULT_PORT)),
        schema: config.get_str("schema"),
        encryption,
        certificate_validation,
        certificate_fingerprint: config.get_str("certificate_fingerprint"),
        connection_timeout: config.get_string("connection_timeout"),
        query_timeout: config.get_string("query_timeout"),
        idle_timeout: config.get_string("idle_timeout"),
    })
}

fn apply_connection_args(
    _config: &AdapterConfig,
    builder: DatabaseBuilder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabaseBuilder, AuthError> {
    Ok(builder)
}

pub struct ExasolAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl ExasolAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for ExasolAuth {
    fn backend(&self) -> Backend {
        Backend::Exasol
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        auth_configure_pipeline!(self, &config, parse_auth, apply_connection_args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_options::uri_value;
    use dbt_test_primitives::assert_contains;
    use dbt_yaml::Mapping;
    use dbt_yaml::Value as YmlValue;

    #[test]
    fn test_defaults_with_required_fields() {
        let config = Mapping::from_iter([
            ("user".into(), "sys".into()),
            ("password".into(), "exasol".into()),
        ]);

        let builder = ExasolAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "exasol://localhost:8563");
    }

    #[test]
    fn test_custom_host_and_port() {
        let config = Mapping::from_iter([
            ("host".into(), "exasol.prod.internal".into()),
            ("port".into(), "9563".into()),
            ("user".into(), "analytics".into()),
            ("password".into(), "secret".into()),
        ]);

        let builder = ExasolAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "exasol://exasol.prod.internal:9563");
    }

    #[test]
    fn test_schema_in_uri() {
        let config = Mapping::from_iter([
            ("user".into(), "sys".into()),
            ("password".into(), "exasol".into()),
            ("schema".into(), "MY_SCHEMA".into()),
        ]);

        let builder = ExasolAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "exasol://localhost:8563/MY_SCHEMA");
    }

    #[test]
    fn test_encryption_disabled() {
        let config = Mapping::from_iter([
            ("user".into(), "sys".into()),
            ("password".into(), "exasol".into()),
            ("encryption".into(), "false".into()),
        ]);

        let builder = ExasolAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "tls=0");
    }

    #[test]
    fn test_encryption_disabled_as_yaml_boolean() {
        let config: Mapping = dbt_yaml::from_str(
            r#"
user: sys
password: exasol
encryption: false
"#,
        )
        .expect("parse yaml");

        let builder = ExasolAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "tls=0");
    }

    #[test]
    fn test_certificate_validation_disabled() {
        let config = Mapping::from_iter([
            ("user".into(), "sys".into()),
            ("password".into(), "exasol".into()),
            ("certificate_validation".into(), "false".into()),
        ]);

        let builder = ExasolAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "validateservercertificate=0");
    }

    #[test]
    fn test_certificate_fingerprint() {
        let config = Mapping::from_iter([
            ("user".into(), "sys".into()),
            ("password".into(), "exasol".into()),
            ("certificate_fingerprint".into(), "AB:CD:EF:01:23:45".into()),
        ]);

        let builder = ExasolAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "certificatefingerprint=AB:CD:EF:01:23:45");
    }

    #[test]
    fn test_connection_timeout() {
        let config = Mapping::from_iter([
            ("user".into(), "sys".into()),
            ("password".into(), "exasol".into()),
            ("connection_timeout".into(), YmlValue::number(30i64.into())),
        ]);

        let builder = ExasolAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "timeout=30");
    }

    #[test]
    fn test_query_and_idle_timeout() {
        let config = Mapping::from_iter([
            ("user".into(), "sys".into()),
            ("password".into(), "exasol".into()),
            ("query_timeout".into(), YmlValue::number(300i64.into())),
            ("idle_timeout".into(), "600".into()),
        ]);

        let builder = ExasolAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "query_timeout=300");
        assert_contains!(&uri, "idle_timeout=600");
    }

    #[test]
    fn test_numeric_port() {
        let config = Mapping::from_iter([
            ("user".into(), "sys".into()),
            ("password".into(), "exasol".into()),
            ("port".into(), YmlValue::number(9563i64.into())),
        ]);

        let builder = ExasolAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "exasol://localhost:9563");
    }

    #[test]
    fn test_missing_user_returns_error() {
        let config = Mapping::from_iter([("password".into(), "exasol".into())]);

        let result = ExasolAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config));
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_password_returns_error() {
        let config = Mapping::from_iter([("user".into(), "sys".into())]);

        let result = ExasolAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config));
        assert!(result.is_err());
    }

    #[test]
    fn test_encryption_enabled_by_default_no_tls_param() {
        let config = Mapping::from_iter([
            ("user".into(), "sys".into()),
            ("password".into(), "exasol".into()),
        ]);

        let builder = ExasolAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        // When encryption is enabled (default), no tls param should be present
        assert!(!uri.contains("tls="));
    }
}
