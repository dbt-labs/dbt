use std::fs;

use crate::{
    AdapterConfig, Auth, AuthError, AuthWarningPrinter, PrivateKeySource, auth_configure_pipeline,
};
use database::Builder as DatabaseBuilder;

use dbt_adbc::salesforce::auth_type;
use dbt_adbc::{Backend, database, salesforce};

pub struct SalesforceAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl SalesforceAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

/// Salesforce authentication methods
enum AuthMethod<'a> {
    /// JWT Bearer authentication for Salesforce
    JwtBearer { private_key: PrivateKeySource<'a> },
    /// Username/Password authentication
    UsernamePassword {
        client_secret: &'a str,
        password: &'a str,
    },
}

impl<'a> AuthMethod<'a> {
    pub fn apply(
        self,
        mut builder: DatabaseBuilder,
        _warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<DatabaseBuilder, AuthError> {
        match self {
            AuthMethod::JwtBearer { private_key } => {
                builder.with_named_option(salesforce::AUTH_TYPE, auth_type::JWT)?;
                let private_key = match private_key {
                    PrivateKeySource::Raw(key) => key.to_string(),
                    PrivateKeySource::FilePath(path) => fs::read_to_string(path)?,
                };
                builder.with_named_option(salesforce::JWT_PRIVATE_KEY, private_key)?;
            }
            AuthMethod::UsernamePassword {
                client_secret,
                password,
            } => {
                builder.with_named_option(salesforce::AUTH_TYPE, auth_type::USERNAME_PASSWORD)?;
                builder.with_named_option(salesforce::CLIENT_SECRET, client_secret)?;
                builder.with_named_option(salesforce::PASSWORD, password)?;
            }
        }

        Ok(builder)
    }
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<AuthMethod<'a>, AuthError> {
    let method = config.require_str("method")?;

    let auth_method = match method {
        "jwt_bearer" => {
            let private_key = config.get_str("private_key");
            let private_key_path = config.get_str("private_key_path");

            // Validate that exactly one private key source is provided
            let private_key = match (private_key, private_key_path) {
                (Some(_), Some(_)) => {
                    return Err(AuthError::config(
                        "Cannot specify both 'private_key' and 'private_key_path'. Choose one.",
                    ));
                }
                (None, None) => {
                    return Err(AuthError::config(
                        "JWT authentication requires either 'private_key' or 'private_key_path'.",
                    ));
                }
                (Some(private_key), None) => PrivateKeySource::Raw(private_key),
                (None, Some(private_key_path)) => PrivateKeySource::FilePath(private_key_path),
            };
            AuthMethod::JwtBearer { private_key }
        }
        "username_password" => AuthMethod::UsernamePassword {
            client_secret: config.require_str("client_secret")?,
            password: config.require_str("password")?,
        },
        unsupported_method => {
            return Err(AuthError::config(format!(
                "Unsupported authentication method '{unsupported_method}' for Salesforce.
 Supported methods: 'jwt_bearer', 'username_password'"
            )));
        }
    };

    Ok(auth_method)
}

fn apply_connection_args(
    config: &AdapterConfig,
    mut builder: DatabaseBuilder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabaseBuilder, AuthError> {
    for (opt_name, adbc_opt_name) in [
        ("username", salesforce::USERNAME),
        ("client_id", salesforce::CLIENT_ID),
        ("login_url", salesforce::LOGIN_URL),
    ] {
        let opt_value = config.require_str(opt_name)?;
        builder.with_named_option(adbc_opt_name, opt_value)?;
    }

    Ok(builder)
}

impl Auth for SalesforceAuth {
    fn backend(&self) -> Backend {
        Backend::Salesforce
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        auth_configure_pipeline!(self, &config, parse_auth, apply_connection_args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_options::other_option_value;
    use dbt_test_primitives::assert_contains;
    use dbt_yaml::Mapping;

    #[test]
    fn test_jwt_bearer_private_key() {
        let config = Mapping::from_iter([
            ("method".into(), "jwt_bearer".into()),
            ("username".into(), "user@example.com".into()),
            ("client_id".into(), "client".into()),
            ("login_url".into(), "https://login.salesforce.com".into()),
            ("private_key".into(), "KEYDATA".into()),
        ]);

        let builder = SalesforceAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, salesforce::AUTH_TYPE).unwrap(),
            auth_type::JWT
        );
        assert_eq!(
            other_option_value(&builder, salesforce::JWT_PRIVATE_KEY).unwrap(),
            "KEYDATA"
        );
    }

    #[test]
    fn test_jwt_bearer_conflicting_keys_error() {
        let config = Mapping::from_iter([
            ("method".into(), "jwt_bearer".into()),
            ("username".into(), "user@example.com".into()),
            ("client_id".into(), "client".into()),
            ("login_url".into(), "https://login.salesforce.com".into()),
            ("private_key".into(), "KEYDATA".into()),
            ("private_key_path".into(), "/tmp/key.pem".into()),
        ]);

        let err = SalesforceAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("configure should fail");
        assert_contains!(
            err.msg(),
            "Cannot specify both 'private_key' and 'private_key_path'. Choose one."
        );
    }
}
