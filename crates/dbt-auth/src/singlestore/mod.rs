use std::borrow::Cow;

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

impl Auth for SingleStoreAuth {
    fn backend(&self) -> Backend {
        Backend::SingleStore
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        let mut builder = database::Builder::new(self.backend());

        let user = config.require_string("user")?;
        let host = config.require_string("host")?;
        let port = config.get_string("port").unwrap_or(Cow::Borrowed("3306"));
        let dbname = config.require_string("database")?;

        let password = config
            .get_string("password")
            .or_else(|| config.get_string("token"))
            .or_else(|| config.get_string("jwt"));
        let is_token_auth =
            config.get_string("token").is_some() || config.get_string("jwt").is_some();

        let encoded_user = utf8_percent_encode(&user, NON_ALPHANUMERIC);
        let encoded_dbname = utf8_percent_encode(&dbname, NON_ALPHANUMERIC);

        let ssl_ca = config
            .get_string("ssl_ca")
            .or_else(|| config.get_string("sslrootcert"));
        let ssl_cert = config.get_string("ssl_cert");
        let ssl_key = config.get_string("ssl_key");

        if ssl_cert.is_some() != ssl_key.is_some() {
            return Err(AuthError::Config(
                "both ssl_cert and ssl_key must be provided for client certificate authentication"
                    .to_string(),
            ));
        }

        let mut query_params: Vec<(&str, String)> = Vec::new();

        let ssl_mode = config
            .get_string("ssl_mode")
            .or_else(|| config.get_string("sslmode"));

        let mut tls_enforced = false;
        if let Some(mode) = ssl_mode {
            let mode_clean = mode.trim();
            match mode_clean.to_ascii_lowercase().as_str() {
                "disable" | "disabled" | "false" | "off" => {
                    if ssl_ca.is_some() || ssl_cert.is_some() || ssl_key.is_some() {
                        return Err(AuthError::Config(
                            "ssl_ca, ssl_cert, and ssl_key cannot be used with ssl_mode: disabled"
                                .to_string(),
                        ));
                    }
                    query_params.push(("tls", "false".to_string()));
                }
                "prefer" | "preferred" => {
                    query_params.push(("tls", "preferred".to_string()));
                }
                "require" | "required" | "true" => {
                    query_params.push(("tls", "skip-verify".to_string()));
                    tls_enforced = true;
                }
                "verify_ca" | "verify-ca" | "verifyca" => {
                    if ssl_ca.is_some() {
                        query_params.push(("tls", "verify_ca".to_string()));
                    } else {
                        query_params.push(("tls", "true".to_string()));
                    }
                    tls_enforced = true;
                }
                "verify_full" | "verify-full" | "verifyfull" | "verifyidentity"
                | "verify-identity" => {
                    query_params.push(("tls", "true".to_string()));
                    tls_enforced = true;
                }
                other => {
                    return Err(AuthError::Config(format!(
                        "unsupported ssl_mode '{other}': expected disabled, preferred, required, verify_ca, or verify_full"
                    )));
                }
            }
        } else if ssl_ca.is_some() || ssl_cert.is_some() || ssl_key.is_some() {
            // Certificates provided without explicit ssl_mode default to verify_ca with TLS enforced.
            query_params.push(("tls", "verify_ca".to_string()));
            tls_enforced = true;
        }

        if let Some(ref ca) = ssl_ca {
            query_params.push(("ssl_ca", ca.to_string()));
        }
        if let Some(ref cert) = ssl_cert {
            query_params.push(("ssl_cert", cert.to_string()));
        }
        if let Some(ref key) = ssl_key {
            query_params.push(("ssl_key", key.to_string()));
        }

        if is_token_auth && !tls_enforced {
            return Err(AuthError::Config(
                "JWT authentication requires TLS to be enabled (ssl_mode must be required, verify_ca, or verify_full)"
                    .to_string(),
            ));
        }

        // Map allow_cleartext_plugin to driver's allowCleartextPasswords parameter.
        // Cleartext transmission is only permitted when TLS is strictly enforced.
        // For JWT auth, cleartext transmission defaults to enabled over TLS.
        let allow_cleartext = config
            .get_bool("allow_cleartext_plugin")
            .or_else(|| config.get_bool("cleartext_plugin"))
            .or_else(|| {
                config
                    .get_string("allow_cleartext_plugin")
                    .or_else(|| config.get_string("cleartext_plugin"))
                    .and_then(|s| match s.trim().to_ascii_lowercase().as_str() {
                        "true" | "1" => Some(true),
                        "false" | "0" => Some(false),
                        _ => None,
                    })
            })
            .or(if is_token_auth { Some(true) } else { None });

        if let Some(cleartext) = allow_cleartext {
            if cleartext {
                if !tls_enforced {
                    return Err(AuthError::Config(
                        "allow_cleartext_plugin requires TLS to be enabled (ssl_mode must be required, verify_ca, or verify_full)"
                            .to_string(),
                    ));
                }
                query_params.push(("allowCleartextPasswords", "true".to_string()));
            } else {
                query_params.push(("allowCleartextPasswords", "false".to_string()));
            }
        }

        let connect_timeout = config
            .get_string("connect_timeout")
            .or_else(|| config.get_string("timeout"));
        if let Some(timeout) = connect_timeout {
            let timeout_trimmed = timeout.trim();
            if !timeout_trimmed.is_empty() {
                let timeout_val = if timeout_trimmed.chars().all(|c| c.is_ascii_digit()) {
                    format!("{timeout_trimmed}s")
                } else {
                    timeout_trimmed.to_string()
                };
                query_params.push(("timeout", timeout_val));
            }
        }

        let user_auth_part = match password {
            Some(ref pass) if !pass.is_empty() => {
                let encoded_password = utf8_percent_encode(pass, NON_ALPHANUMERIC);
                format!("{encoded_user}:{encoded_password}")
            }
            _ => format!("{encoded_user}"),
        };

        let mut uri = format!("mysql://{user_auth_part}@{host}:{port}/{encoded_dbname}");
        if !query_params.is_empty() {
            let query_string = query_params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            uri.push('?');
            uri.push_str(&query_string);
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

    fn make_config(pairs: &[(&str, &str)]) -> AdapterConfig {
        let mapping = Mapping::from_iter(
            pairs
                .iter()
                .map(|(k, v)| (dbt_yaml::Value::from(*k), dbt_yaml::Value::from(*v))),
        );
        AdapterConfig::new(mapping)
    }

    #[test]
    fn test_singlestore_auth_ssl_modes() {
        let auth = SingleStoreAuth::new(Box::new(NoopAuthWarningPrinter));

        // 1. Default (no ssl_mode)
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://user:pass@localhost:3306/db"
        );

        // 2. Disabled
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_mode", "disabled"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://user:pass@localhost:3306/db?tls=false"
        );

        // 3. Preferred
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_mode", "preferred"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://user:pass@localhost:3306/db?tls=preferred"
        );

        // 4. Required (maps to tls=skip-verify)
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_mode", "required"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://user:pass@localhost:3306/db?tls=skip-verify"
        );

        // 5. Verify-CA (maps to tls=true)
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_mode", "verify_ca"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://user:pass@localhost:3306/db?tls=true"
        );

        // 6. Verify-Full (maps to tls=true)
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_mode", "verify_full"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://user:pass@localhost:3306/db?tls=true"
        );

        // 7. Unknown ssl_mode errors
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_mode", "nonsense"),
        ]);
        let err = auth.configure(&config).unwrap_err();
        match err {
            AuthError::Config(msg) => assert!(msg.contains("unsupported ssl_mode 'nonsense'")),
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn test_singlestore_auth_cleartext_plugin() {
        let auth = SingleStoreAuth::new(Box::new(NoopAuthWarningPrinter));

        // 1. Cleartext without TLS -> error
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("allow_cleartext_plugin", "true"),
        ]);
        let err = auth.configure(&config).unwrap_err();
        match err {
            AuthError::Config(msg) => assert!(msg.contains("requires TLS to be enabled")),
            other => panic!("expected Config error, got {other:?}"),
        }

        // 2. Cleartext with preferred -> error (preferred can fallback to plaintext)
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_mode", "preferred"),
            ("allow_cleartext_plugin", "true"),
        ]);
        let err = auth.configure(&config).unwrap_err();
        match err {
            AuthError::Config(msg) => assert!(msg.contains("requires TLS to be enabled")),
            other => panic!("expected Config error, got {other:?}"),
        }

        // 3. Cleartext with required TLS -> success with allowCleartextPasswords
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_mode", "required"),
            ("allow_cleartext_plugin", "true"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://user:pass@localhost:3306/db?tls=skip-verify&allowCleartextPasswords=true"
        );
    }

    #[test]
    fn test_singlestore_auth_passwordless() {
        let auth = SingleStoreAuth::new(Box::new(NoopAuthWarningPrinter));

        // 1. No password provided
        let config = make_config(&[("user", "user"), ("host", "localhost"), ("database", "db")]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://user@localhost:3306/db"
        );

        // 2. Empty string password
        let config = make_config(&[
            ("user", "user"),
            ("password", ""),
            ("host", "localhost"),
            ("database", "db"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://user@localhost:3306/db"
        );
    }

    #[test]
    fn test_singlestore_auth_jwt_token() {
        let auth = SingleStoreAuth::new(Box::new(NoopAuthWarningPrinter));

        // 1. JWT token with TLS -> auto-enables cleartext passwords
        let config = make_config(&[
            ("user", "svc"),
            ("token", "my.jwt.token"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_mode", "verify_full"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://svc:my%2Ejwt%2Etoken@localhost:3306/db?tls=true&allowCleartextPasswords=true"
        );

        // 2. JWT token with alias "jwt"
        let config = make_config(&[
            ("user", "svc"),
            ("jwt", "my.jwt.token"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_mode", "required"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://svc:my%2Ejwt%2Etoken@localhost:3306/db?tls=skip-verify&allowCleartextPasswords=true"
        );

        // 3. JWT token without TLS -> error
        let config = make_config(&[
            ("user", "svc"),
            ("token", "my.jwt.token"),
            ("host", "localhost"),
            ("database", "db"),
        ]);
        let err = auth.configure(&config).unwrap_err();
        match err {
            AuthError::Config(msg) => {
                assert!(msg.contains("JWT authentication requires TLS to be enabled"))
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn test_singlestore_auth_connect_timeout() {
        let auth = SingleStoreAuth::new(Box::new(NoopAuthWarningPrinter));

        // 1. Integer seconds connect_timeout
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("connect_timeout", "15"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://user:pass@localhost:3306/db?timeout=15s"
        );

        // 2. String duration with unit timeout
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("timeout", "30s"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://user:pass@localhost:3306/db?timeout=30s"
        );
    }

    #[test]
    fn test_singlestore_auth_private_ca() {
        let auth = SingleStoreAuth::new(Box::new(NoopAuthWarningPrinter));

        // 1. Custom ssl_ca with default ssl_mode (defaults to tls=verify_ca)
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_ca", "/etc/ssl/ca.pem"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://user:pass@localhost:3306/db?tls=verify_ca&ssl_ca=/etc/ssl/ca.pem"
        );

        // 2. Custom ssl_ca with ssl_cert and ssl_key
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_mode", "verify_full"),
            ("ssl_ca", "/etc/ssl/ca.pem"),
            ("ssl_cert", "/etc/ssl/cert.pem"),
            ("ssl_key", "/etc/ssl/key.pem"),
        ]);
        let builder = auth.configure(&config).unwrap();
        assert_eq!(
            builder.uri.as_ref().unwrap().as_str(),
            "mysql://user:pass@localhost:3306/db?tls=true&ssl_ca=/etc/ssl/ca.pem&ssl_cert=/etc/ssl/cert.pem&ssl_key=/etc/ssl/key.pem"
        );

        // 3. ssl_cert without ssl_key -> error
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_cert", "/etc/ssl/cert.pem"),
        ]);
        let err = auth.configure(&config).unwrap_err();
        match err {
            AuthError::Config(msg) => {
                assert!(msg.contains("both ssl_cert and ssl_key must be provided"))
            }
            other => panic!("expected Config error, got {other:?}"),
        }

        // 4. ssl_ca with disabled ssl_mode -> error
        let config = make_config(&[
            ("user", "user"),
            ("password", "pass"),
            ("host", "localhost"),
            ("database", "db"),
            ("ssl_mode", "disabled"),
            ("ssl_ca", "/etc/ssl/ca.pem"),
        ]);
        let err = auth.configure(&config).unwrap_err();
        match err {
            AuthError::Config(msg) => {
                assert!(msg.contains(
                    "ssl_ca, ssl_cert, and ssl_key cannot be used with ssl_mode: disabled"
                ))
            }
            other => panic!("expected Config error, got {other:?}"),
        }
    }
}
