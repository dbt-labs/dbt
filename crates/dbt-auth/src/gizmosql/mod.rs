//! GizmoSQL authentication: translates a `profiles.yml` target into ADBC database
//! options for the GizmoSQL driver (`adbc_driver_gizmosql`), which speaks Arrow
//! Flight SQL to a DuckDB-backed GizmoSQL server.
//!
//! Profile keys mirror the Python `dbt-gizmosql` 1.x adapter so existing
//! profiles keep working unchanged:
//!
//! ```yaml
//! type: gizmosql
//! host: gizmosql.example.com
//! port: 31337              # default
//! username: dbt            # alias: user
//! password: secret         # alias: pass
//! database: dbt            # the DuckDB catalog to use; alias: catalog, dbname
//! schema: main
//! use_encryption: true     # default; alias: use_tls
//! tls_skip_verify: false   # default; alias: disable_certificate_verification
//! auth_type: password      # or `external` for the OAuth/SSO browser flow
//! ```
use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter, auth_configure_pipeline};
use database::Builder as DatabaseBuilder;
use dbt_adbc::{Backend, database};
use std::borrow::Cow;

const DEFAULT_PORT: &str = "31337";

/// Upstream Flight SQL driver option: skip TLS certificate verification.
pub const OPTION_TLS_SKIP_VERIFY: &str = "adbc.flight.sql.client_option.tls_skip_verify";
/// GizmoSQL driver option: `password` (default) or `external` (OAuth/SSO browser flow).
pub const OPTION_AUTH_TYPE: &str = "adbc.gizmosql.auth_type";

const AUTH_TYPE_PASSWORD: &str = "password";
const AUTH_TYPE_EXTERNAL: &str = "external";

#[derive(Debug)]
enum GizmoSQLAuthIR<'a> {
    Connect {
        host: &'a str,
        port: Cow<'a, str>,
        username: Option<Cow<'a, str>>,
        password: Option<Cow<'a, str>>,
        use_encryption: bool,
        tls_skip_verify: bool,
        auth_type: Option<Cow<'a, str>>,
    },
}

impl<'a> GizmoSQLAuthIR<'a> {
    pub fn apply(
        self,
        mut builder: DatabaseBuilder,
        warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<DatabaseBuilder, AuthError> {
        match self {
            Self::Connect {
                host,
                port,
                username,
                password,
                use_encryption,
                tls_skip_verify,
                auth_type,
            } => {
                // `gizmosql://` is the driver's own URI scheme: TLS by default, with
                // `?transport=tcp` for plaintext. Using it rather than the underlying
                // `grpc+tls://` / `grpc+tcp://` forms keeps the transport a driver
                // concern.
                let mut uri = format!("gizmosql://{host}:{port}");
                if !use_encryption {
                    uri.push_str("?transport=tcp");
                }
                builder.with_parse_uri(uri)?;

                if auth_type.as_deref() == Some(AUTH_TYPE_EXTERNAL) {
                    // The OAuth/SSO flow supplies the credentials itself (the driver
                    // presents the identity token as basic auth); anything in the
                    // profile would be ignored, so never send it and say so.
                    if username.is_some() || password.is_some() {
                        warning_printer.warn(
                            "GizmoSQL: 'username' and 'password' are ignored when \
                             auth_type is 'external'",
                        );
                    }
                } else if let Some(username) = username {
                    builder.with_username(username.as_ref());
                    // Flight SQL basic auth always sends a password; an empty one
                    // matches the Python adapter's behavior.
                    builder.with_password(password.as_deref().unwrap_or(""));
                }

                if tls_skip_verify {
                    builder.with_named_option(OPTION_TLS_SKIP_VERIFY, "true")?;
                }

                if let Some(auth_type) = auth_type {
                    builder.with_named_option(OPTION_AUTH_TYPE, auth_type.as_ref())?;
                }
            }
        }

        Ok(builder)
    }
}

/// Read a boolean profile field, accepting YAML booleans as well as the string
/// spellings dbt users commonly write (`"true"`, `"False"`, `"1"`, `"0"`).
fn get_flag(config: &AdapterConfig, field: &str, default: bool) -> Result<bool, AuthError> {
    if let Some(value) = config.get_bool(field) {
        return Ok(value);
    }
    match config.get_string(field) {
        None => Ok(default),
        Some(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            other => Err(AuthError::config(format!(
                "GizmoSQL profile field '{field}' must be a boolean, got '{other}'"
            ))),
        },
    }
}

/// Read a boolean profile field, falling back to its Python-adapter alias when
/// the canonical key is absent.
fn get_flag_or_alias(
    config: &AdapterConfig,
    field: &str,
    alias: &str,
    default: bool,
) -> Result<bool, AuthError> {
    if config.get(field).is_some() {
        get_flag(config, field, default)
    } else {
        get_flag(config, alias, default)
    }
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<GizmoSQLAuthIR<'a>, AuthError> {
    let host = config
        .get_str("host")
        .filter(|h| !h.is_empty())
        .ok_or_else(|| AuthError::config("GizmoSQL requires 'host' in profile configuration"))?;

    let port = config
        .get_string("port")
        .unwrap_or(Cow::Borrowed(DEFAULT_PORT));

    // `username` is canonical (matching the Python adapter's `GizmoSQLCredentials`);
    // `user` is accepted as an alias.
    let username = config
        .get_string("username")
        .or_else(|| config.get_string("user"))
        .filter(|u| !u.is_empty());
    let password = config
        .get_string("password")
        .or_else(|| config.get_string("pass"));

    let use_encryption = get_flag_or_alias(config, "use_encryption", "use_tls", true)?;
    let tls_skip_verify = get_flag_or_alias(
        config,
        "tls_skip_verify",
        "disable_certificate_verification",
        false,
    )?;

    let auth_type = config.get_string("auth_type");
    if let Some(auth_type) = &auth_type
        && auth_type != AUTH_TYPE_PASSWORD
        && auth_type != AUTH_TYPE_EXTERNAL
    {
        return Err(AuthError::config(format!(
            "GizmoSQL 'auth_type' must be '{AUTH_TYPE_PASSWORD}' or '{AUTH_TYPE_EXTERNAL}', got '{auth_type}'"
        )));
    }

    if auth_type.as_deref() != Some(AUTH_TYPE_EXTERNAL) && username.is_none() {
        return Err(AuthError::config(
            "GizmoSQL requires 'username' in profile configuration (or auth_type: external)",
        ));
    }

    Ok(GizmoSQLAuthIR::Connect {
        host,
        port,
        username,
        password,
        use_encryption,
        tls_skip_verify,
        auth_type,
    })
}

fn apply_connection_args(
    _config: &AdapterConfig,
    builder: DatabaseBuilder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabaseBuilder, AuthError> {
    Ok(builder)
}

pub struct GizmoSQLAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl GizmoSQLAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for GizmoSQLAuth {
    fn backend(&self) -> Backend {
        Backend::GizmoSQL
    }

    fn configure(&self, config: &AdapterConfig) -> Result<DatabaseBuilder, AuthError> {
        auth_configure_pipeline!(self, config, parse_auth, apply_connection_args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_options::{other_option_value, uri_value};
    use adbc_core::options::{OptionDatabase, OptionValue};
    use dbt_test_primitives::assert_contains;
    use dbt_yaml::Mapping;
    use dbt_yaml::Value as YmlValue;

    fn configure(config: Mapping) -> DatabaseBuilder {
        GizmoSQLAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure")
    }

    /// Collects warnings into a shared log so tests can assert on them.
    struct RecordingWarningPrinter(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

    impl AuthWarningPrinter for RecordingWarningPrinter {
        fn warn(&self, message: &str) {
            self.0.lock().unwrap().push(message.to_string());
        }
    }

    fn option_value(builder: &DatabaseBuilder, key: OptionDatabase) -> Option<String> {
        builder.clone().into_iter().find_map(|(k, v)| {
            if k == key {
                match v {
                    OptionValue::String(s) => Some(s),
                    _ => panic!("Expected OptionValue to be String"),
                }
            } else {
                None
            }
        })
    }

    #[test]
    fn test_defaults_with_required_fields() {
        let builder = configure(Mapping::from_iter([
            ("host".into(), "gizmosql.example.com".into()),
            ("username".into(), "dbt".into()),
            ("password".into(), "secret".into()),
        ]));

        assert_eq!(uri_value(&builder), "gizmosql://gizmosql.example.com:31337");
        assert_eq!(
            option_value(&builder, OptionDatabase::Username).as_deref(),
            Some("dbt")
        );
        assert_eq!(
            option_value(&builder, OptionDatabase::Password).as_deref(),
            Some("secret")
        );
        assert!(other_option_value(&builder, OPTION_TLS_SKIP_VERIFY).is_none());
        assert!(other_option_value(&builder, OPTION_AUTH_TYPE).is_none());
    }

    #[test]
    fn test_custom_port_as_string_and_number() {
        let builder = configure(Mapping::from_iter([
            ("host".into(), "localhost".into()),
            ("port".into(), "9494".into()),
            ("username".into(), "dbt".into()),
        ]));
        assert_eq!(uri_value(&builder), "gizmosql://localhost:9494");

        let builder = configure(Mapping::from_iter([
            ("host".into(), "localhost".into()),
            ("port".into(), YmlValue::number(9494i64.into())),
            ("username".into(), "dbt".into()),
        ]));
        assert_eq!(uri_value(&builder), "gizmosql://localhost:9494");
    }

    #[test]
    fn test_encryption_disabled_uses_plaintext_scheme() {
        for value in [
            YmlValue::from(false),
            "false".into(),
            "False".into(),
            "0".into(),
        ] {
            let builder = configure(Mapping::from_iter([
                ("host".into(), "localhost".into()),
                ("username".into(), "dbt".into()),
                ("use_encryption".into(), value),
            ]));
            assert_eq!(
                uri_value(&builder),
                "gizmosql://localhost:31337?transport=tcp"
            );
        }
    }

    #[test]
    fn test_encryption_yaml_boolean_from_str() {
        let config: Mapping = dbt_yaml::from_str(
            r#"
host: localhost
username: dbt
password: dbt
use_encryption: false
"#,
        )
        .expect("parse yaml");
        let builder = configure(config);
        assert_eq!(
            uri_value(&builder),
            "gizmosql://localhost:31337?transport=tcp"
        );
    }

    #[test]
    fn test_user_and_pass_aliases() {
        let builder = configure(Mapping::from_iter([
            ("host".into(), "localhost".into()),
            ("user".into(), "alias_user".into()),
            ("pass".into(), "alias_pass".into()),
        ]));
        assert_eq!(
            option_value(&builder, OptionDatabase::Username).as_deref(),
            Some("alias_user")
        );
        assert_eq!(
            option_value(&builder, OptionDatabase::Password).as_deref(),
            Some("alias_pass")
        );
    }

    #[test]
    fn test_missing_password_sends_empty_password() {
        let builder = configure(Mapping::from_iter([
            ("host".into(), "localhost".into()),
            ("username".into(), "dbt".into()),
        ]));
        assert_eq!(
            option_value(&builder, OptionDatabase::Password).as_deref(),
            Some("")
        );
    }

    #[test]
    fn test_tls_skip_verify_and_alias() {
        let builder = configure(Mapping::from_iter([
            ("host".into(), "localhost".into()),
            ("username".into(), "dbt".into()),
            ("tls_skip_verify".into(), YmlValue::from(true)),
        ]));
        assert_eq!(
            other_option_value(&builder, OPTION_TLS_SKIP_VERIFY),
            Some("true")
        );

        let builder = configure(Mapping::from_iter([
            ("host".into(), "localhost".into()),
            ("username".into(), "dbt".into()),
            ("disable_certificate_verification".into(), "true".into()),
        ]));
        assert_eq!(
            other_option_value(&builder, OPTION_TLS_SKIP_VERIFY),
            Some("true")
        );
    }

    #[test]
    fn test_auth_type_external_does_not_require_username() {
        let builder = configure(Mapping::from_iter([
            ("host".into(), "gizmosql.example.com".into()),
            ("auth_type".into(), "external".into()),
        ]));
        assert_eq!(
            other_option_value(&builder, OPTION_AUTH_TYPE),
            Some("external")
        );
        assert!(option_value(&builder, OptionDatabase::Username).is_none());
        assert!(option_value(&builder, OptionDatabase::Password).is_none());
    }

    #[test]
    fn test_auth_type_external_ignores_credentials_and_warns() {
        let warnings_log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let auth = GizmoSQLAuth::new(Box::new(RecordingWarningPrinter(warnings_log.clone())));
        let builder = auth
            .configure(&AdapterConfig::new(Mapping::from_iter([
                ("host".into(), "gizmosql.example.com".into()),
                ("auth_type".into(), "external".into()),
                ("username".into(), "dbt".into()),
                ("password".into(), "secret".into()),
            ])))
            .expect("configure");

        // The OAuth flow supplies its own credentials; the profile's must not be sent.
        assert!(option_value(&builder, OptionDatabase::Username).is_none());
        assert!(option_value(&builder, OptionDatabase::Password).is_none());
        assert_eq!(
            other_option_value(&builder, OPTION_AUTH_TYPE),
            Some("external")
        );

        let warnings = warnings_log.lock().unwrap().clone();
        assert_eq!(warnings.len(), 1, "expected one warning, got {warnings:?}");
        assert_contains!(&warnings[0], "ignored when auth_type is 'external'");
    }

    #[test]
    fn test_auth_type_password_is_passed_through() {
        let builder = configure(Mapping::from_iter([
            ("host".into(), "localhost".into()),
            ("username".into(), "dbt".into()),
            ("auth_type".into(), "password".into()),
        ]));
        assert_eq!(
            other_option_value(&builder, OPTION_AUTH_TYPE),
            Some("password")
        );
    }

    #[test]
    fn test_invalid_auth_type_returns_error() {
        let result = GizmoSQLAuth::new(Box::new(crate::NoopAuthWarningPrinter)).configure(
            &AdapterConfig::new(Mapping::from_iter([
                ("host".into(), "localhost".into()),
                ("username".into(), "dbt".into()),
                ("auth_type".into(), "kerberos".into()),
            ])),
        );
        let err = result.expect_err("invalid auth_type must fail");
        assert_contains!(err.msg(), "auth_type");
    }

    #[test]
    fn test_invalid_boolean_returns_error() {
        let result = GizmoSQLAuth::new(Box::new(crate::NoopAuthWarningPrinter)).configure(
            &AdapterConfig::new(Mapping::from_iter([
                ("host".into(), "localhost".into()),
                ("username".into(), "dbt".into()),
                ("use_encryption".into(), "maybe".into()),
            ])),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_host_returns_error() {
        let result = GizmoSQLAuth::new(Box::new(crate::NoopAuthWarningPrinter)).configure(
            &AdapterConfig::new(Mapping::from_iter([("username".into(), "dbt".into())])),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_username_returns_error() {
        let result = GizmoSQLAuth::new(Box::new(crate::NoopAuthWarningPrinter)).configure(
            &AdapterConfig::new(Mapping::from_iter([("host".into(), "localhost".into())])),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_username_returns_error() {
        let result = GizmoSQLAuth::new(Box::new(crate::NoopAuthWarningPrinter)).configure(
            &AdapterConfig::new(Mapping::from_iter([
                ("host".into(), "localhost".into()),
                ("username".into(), "".into()),
            ])),
        );
        assert!(result.is_err());
    }
}
