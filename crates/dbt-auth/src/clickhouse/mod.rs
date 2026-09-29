use crate::config::yml_value_to_string;
use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter, auth_configure_pipeline};
use database::Builder as DatabaseBuilder;

use dbt_adbc::{Backend, clickhouse, database};
use dbt_yaml::Value as YmlValue;
use std::borrow::Cow;

const DEFAULT_HOST: &str = "localhost";
const DEFAULT_HTTP_PORT: &str = "8123";
const DEFAULT_HTTPS_PORT: &str = "8443";
const DEFAULT_USER: &str = "default";
const APP_NAME: &str = "dbt";

#[derive(Debug)]
enum ClickHouseAuthIR<'a> {
    UserPass {
        user: &'a str,
        password: Cow<'a, str>,
        host: &'a str,
        port: Cow<'a, str>,
        secure: bool,
    },
}

impl<'a> ClickHouseAuthIR<'a> {
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
                secure,
            } => {
                let scheme = if secure { "https" } else { "http" };
                builder.with_parse_uri(format!("{scheme}://{host}:{port}"))?;
                builder.with_username(user);
                builder.with_password(password.as_ref());
            }
        }

        Ok(builder)
    }
}

/// Accepts YAML bool or string bool ("true"/"True"/"1").
fn bool_flag(config: &AdapterConfig, key: &str) -> bool {
    config
        .get_string(key)
        .map(|s| s == "true" || s == "1" || s == "True")
        .unwrap_or(false)
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<ClickHouseAuthIR<'a>, AuthError> {
    let secure = bool_flag(config, "secure");

    let default_port = if secure {
        DEFAULT_HTTPS_PORT
    } else {
        DEFAULT_HTTP_PORT
    };

    Ok(ClickHouseAuthIR::UserPass {
        user: config.get_str("user").unwrap_or(DEFAULT_USER),
        password: config.get_string("password").unwrap_or(Cow::Borrowed("")),
        host: config.get_str("host").unwrap_or(DEFAULT_HOST),
        port: config
            .get_string("port")
            .unwrap_or(Cow::Borrowed(default_port)),
        secure,
    })
}

/// Mirrors dbclient.py `_conn_settings` as Database-level `clickhouse.setting.*`
/// options: profile `custom_settings` first, then setdefault-style defaults.
/// Divergence: no `session_id` — the driver generates one per connection.
fn apply_connection_args(
    config: &AdapterConfig,
    mut builder: DatabaseBuilder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabaseBuilder, AuthError> {
    let mut settings: Vec<(String, String)> = Vec::new();
    if let Some(custom) = config.get("custom_settings") {
        let YmlValue::Mapping(custom, _) = custom else {
            return Err(AuthError::config("custom_settings must be a map"));
        };
        for (name, value) in custom {
            let Some(name) = name.as_str() else {
                return Err(AuthError::config("custom_settings keys must be strings"));
            };
            settings.push((name.to_string(), yml_value_to_string(value).into_owned()));
        }
    }
    let mut setdefault = |name: &str, value: &str| {
        if !settings.iter().any(|(existing, _)| existing == name) {
            settings.push((name.to_string(), value.to_string()));
        }
    };
    let database_engine = config.get_str("database_engine").unwrap_or("");
    let cluster_mode = bool_flag(config, "cluster_mode");
    if cluster_mode || database_engine == "Replicated" {
        setdefault("database_replicated_enforce_synchronous_settings", "1");
        setdefault("insert_quorum", "auto");
    }
    if database_engine == "Shared" {
        setdefault("select_sequential_consistency", "1");
    }
    setdefault("mutations_sync", "3");
    setdefault("lightweight_deletes_sync", "3");
    setdefault("alter_sync", "3");
    setdefault("insert_distributed_sync", "1");
    for (name, value) in settings {
        builder.with_named_option(clickhouse::setting_key(&name), value)?;
    }
    builder.with_named_option(
        clickhouse::PRODUCT_INFO,
        format!("{APP_NAME}/{}", env!("CARGO_PKG_VERSION")),
    )?;
    Ok(builder)
}

pub struct ClickHouseAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl ClickHouseAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for ClickHouseAuth {
    fn backend(&self) -> Backend {
        Backend::ClickHouse
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        auth_configure_pipeline!(self, &config, parse_auth, apply_connection_args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_options::{other_option_value, uri_value};
    use dbt_test_primitives::assert_contains;
    use dbt_yaml::Mapping;
    use dbt_yaml::Value as YmlValue;

    #[test]
    fn test_defaults_produce_valid_uri() {
        let config = Mapping::new();

        let builder = ClickHouseAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "http://localhost:8123");
    }

    #[test]
    fn test_custom_host_and_port() {
        let config = Mapping::from_iter([
            ("host".into(), "ch.prod.internal".into()),
            ("port".into(), "9000".into()),
            ("user".into(), "alice".into()),
            ("password".into(), "secret".into()),
        ]);

        let builder = ClickHouseAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "http://ch.prod.internal:9000");
    }

    #[test]
    fn test_secure_uses_https_and_default_port_8443() {
        let config = Mapping::from_iter([
            ("host".into(), "ch.cloud".into()),
            ("secure".into(), "true".into()),
        ]);

        let builder = ClickHouseAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "https://ch.cloud:8443");
    }

    #[test]
    fn test_secure_as_yaml_boolean() {
        let config: Mapping = dbt_yaml::from_str(
            r#"
host: ch.cloud
secure: true
"#,
        )
        .expect("parse yaml");

        let builder = ClickHouseAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "https://ch.cloud:8443");
    }

    #[test]
    fn test_numeric_secure_1_enables_https() {
        let config = Mapping::from_iter([
            ("host".into(), "ch.local".into()),
            ("secure".into(), YmlValue::number(1i64.into())),
        ]);

        let builder = ClickHouseAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "https://ch.local:8443");
    }

    #[test]
    fn test_unexpected_secure_value_does_not_enable_https() {
        let config = Mapping::from_iter([
            ("host".into(), "ch.local".into()),
            ("secure".into(), YmlValue::number(42i64.into())),
        ]);

        let builder = ClickHouseAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "http://ch.local:8123");
    }

    fn setting_value<'a>(builder: &'a database::Builder, name: &str) -> Option<&'a str> {
        other_option_value(builder, &clickhouse::setting_key(name))
    }

    #[test]
    fn test_product_info_identifies_dbt() {
        let builder = ClickHouseAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(Mapping::new()))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, clickhouse::PRODUCT_INFO),
            Some(format!("dbt/{}", env!("CARGO_PKG_VERSION")).as_str())
        );
    }

    #[test]
    fn test_default_connection_settings() {
        let builder = ClickHouseAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(Mapping::new()))
            .expect("configure");

        assert_eq!(setting_value(&builder, "mutations_sync"), Some("3"));
        assert_eq!(
            setting_value(&builder, "lightweight_deletes_sync"),
            Some("3")
        );
        assert_eq!(setting_value(&builder, "alter_sync"), Some("3"));
        assert_eq!(
            setting_value(&builder, "insert_distributed_sync"),
            Some("1")
        );
        // conditional settings absent without cluster_mode/database_engine
        assert_eq!(setting_value(&builder, "insert_quorum"), None);
        assert_eq!(
            setting_value(&builder, "database_replicated_enforce_synchronous_settings"),
            None
        );
        assert_eq!(
            setting_value(&builder, "select_sequential_consistency"),
            None
        );
        assert_eq!(setting_value(&builder, "session_id"), None);
    }

    #[test]
    fn test_custom_settings_override_defaults() {
        let config: Mapping = dbt_yaml::from_str(
            r#"
custom_settings:
  mutations_sync: 0
  allow_experimental_object_type: true
  max_insert_threads: 4
"#,
        )
        .expect("parse yaml");

        let builder = ClickHouseAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(setting_value(&builder, "mutations_sync"), Some("0"));
        assert_eq!(
            setting_value(&builder, "allow_experimental_object_type"),
            Some("true")
        );
        assert_eq!(setting_value(&builder, "max_insert_threads"), Some("4"));
        assert_eq!(setting_value(&builder, "alter_sync"), Some("3"));
    }

    #[test]
    fn test_replicated_engine_and_cluster_mode_settings() {
        for config in [
            Mapping::from_iter([("database_engine".into(), "Replicated".into())]),
            Mapping::from_iter([("cluster_mode".into(), YmlValue::bool(true))]),
        ] {
            let builder = ClickHouseAuth::new(Box::new(crate::NoopAuthWarningPrinter))
                .configure(&AdapterConfig::new(config))
                .expect("configure");

            assert_eq!(
                setting_value(&builder, "database_replicated_enforce_synchronous_settings"),
                Some("1")
            );
            assert_eq!(setting_value(&builder, "insert_quorum"), Some("auto"));
        }
    }

    #[test]
    fn test_shared_engine_settings() {
        let config = Mapping::from_iter([("database_engine".into(), "Shared".into())]);

        let builder = ClickHouseAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            setting_value(&builder, "select_sequential_consistency"),
            Some("1")
        );
        assert_eq!(setting_value(&builder, "insert_quorum"), None);
    }
}
