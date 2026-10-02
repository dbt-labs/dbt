use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter};
use dbt_adbc::{Backend, database};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

pub struct SingleStoreAuth {
    #[allow(dead_code)]
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl SingleStoreAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

fn apply_ssl_option(
    builder: &mut database::Builder,
    query_params: &mut Vec<String>,
    name: &str,
    param_name: &str,
    value: Option<impl AsRef<str>>,
) -> Result<(), AuthError> {
    if let Some(val) = value {
        let s = val.as_ref();
        query_params.push(format!("{param_name}={s}"));
        builder.with_named_option(name, s.to_string())?;
    }
    Ok(())
}

impl Auth for SingleStoreAuth {
    fn backend(&self) -> Backend {
        Backend::SingleStore
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        let mut builder = database::Builder::new(self.backend());

        let user = config.require_string("user")?;
        let password = config.require_string("password")?;
        let host = config.require_string("host")?;
        let port = config.require_string("port")?;
        let dbname = config.require_string("database")?;

        let encoded_user = utf8_percent_encode(&user, NON_ALPHANUMERIC);
        let encoded_password = utf8_percent_encode(&password, NON_ALPHANUMERIC);

        let mut uri = format!("mysql://{encoded_user}:{encoded_password}@{host}:{port}/{dbname}");

        let mut query_params = Vec::new();

        let ssl_mode = config
            .get_string("ssl_mode")
            .or_else(|| config.get_string("sslmode"));
        apply_ssl_option(&mut builder, &mut query_params, "ssl_mode", "ssl-mode", ssl_mode)?;

        let ssl_ca = config
            .get_string("ssl_ca")
            .or_else(|| config.get_string("sslrootcert"));
        apply_ssl_option(&mut builder, &mut query_params, "ssl_ca", "ssl-ca", ssl_ca)?;

        apply_ssl_option(
            &mut builder,
            &mut query_params,
            "ssl_cert",
            "ssl-cert",
            config.get_string("ssl_cert"),
        )?;
        apply_ssl_option(
            &mut builder,
            &mut query_params,
            "ssl_key",
            "ssl-key",
            config.get_string("ssl_key"),
        )?;

        let allow_cleartext_plugin = config
            .get_bool("allow_cleartext_plugin")
            .or_else(|| config.get_bool("cleartext_plugin"))
            .unwrap_or(true);
        builder.with_named_option("allow_cleartext_plugin", allow_cleartext_plugin.to_string())?;

        if !query_params.is_empty() {
            uri.push('?');
            uri.push_str(&query_params.join("&"));
        }

        builder.with_parse_uri(uri)?;

        Ok(builder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoopAuthWarningPrinter;
    use dbt_yaml::Mapping;

    #[test]
    fn test_singlestore_auth_uri_construction() {
        let auth = SingleStoreAuth::new(Box::new(NoopAuthWarningPrinter));
        let config = Mapping::from_iter([
            ("user".into(), "user@domain".into()),
            ("password".into(), "p@ss:w/ord".into()),
            ("host".into(), "singlestore.internal".into()),
            ("port".into(), "3306".into()),
            ("database".into(), "analytics".into()),
            ("ssl_mode".into(), "Required".into()),
            ("ssl_ca".into(), "/etc/ssl/ca.pem".into()),
        ]);

        let builder = auth
            .configure(&AdapterConfig::new(config))
            .expect("configure failed");
        let uri = builder.uri.expect("URI missing");
        let uri_str = uri.as_str();

        assert!(uri_str.starts_with(
            "mysql://user%40domain:p%40ss%3Aw%2Ford@singlestore.internal:3306/analytics"
        ));
        assert!(uri_str.contains("ssl-mode=Required"));
        assert!(uri_str.contains("ssl-ca=/etc/ssl/ca.pem"));
    }
}
