use super::common::{ConfigField, ConfigProcessor, FieldValue, InteractiveSetup};
use dbt_common::FsResult;
use dbt_schemas::schemas::profiles::SnowflakeDbConfig;
use dbt_schemas::schemas::serde::StringOrInteger;
use std::path::PathBuf;

impl InteractiveSetup for SnowflakeDbConfig {
    fn get_fields() -> Vec<ConfigField> {
        vec![
            // Connection settings
            ConfigField::input("account", "Account"),
            ConfigField::input("user", "User"),
            ConfigField::input("database", "Database"),
            ConfigField::input("warehouse", "Warehouse"),
            ConfigField::input("schema", "Schema"),
            ConfigField::optional_input("role", "Role (optional)", None),
            // Authentication
            ConfigField::select(
                "auth_method",
                "Which authentication method would you like to use?",
                vec!["Password", "Key pair", "SSO", "Password with MFA"],
                0,
            ),
            ConfigField::password("password", "Password")
                .when_field_equals("auth_method", FieldValue::Integer(0)),
            ConfigField::confirm(
                "use_key_path",
                "Do you want to use a private key file path (vs. inline key)?",
                true,
            )
            .when_field_equals("auth_method", FieldValue::Integer(1)),
            ConfigField::input("private_key_path", "Private key path")
                .when_field_equals("use_key_path", FieldValue::Boolean(true)),
            ConfigField::password("private_key", "Private key (PEM format)")
                .when_field_equals("use_key_path", FieldValue::Boolean(false)),
            ConfigField::confirm(
                "needs_passphrase",
                "Does your private key require a passphrase?",
                false,
            )
            .when_field_equals("auth_method", FieldValue::Integer(1)),
            ConfigField::password("private_key_passphrase", "Private key passphrase")
                .when_field_equals("needs_passphrase", FieldValue::Boolean(true)),
        ]
    }

    fn set_field(&mut self, field_name: &str, value: FieldValue) -> FsResult<()> {
        match field_name {
            "account" => {
                if let FieldValue::String(s) = value {
                    self.account = Some(s);
                }
            }
            "user" => {
                if let FieldValue::String(s) = value {
                    self.user = Some(s);
                }
            }
            "database" => {
                if let FieldValue::String(s) = value {
                    self.database = Some(s);
                }
            }
            "warehouse" => {
                if let FieldValue::String(s) = value {
                    self.warehouse = Some(s);
                }
            }
            "schema" => {
                if let FieldValue::String(s) = value {
                    self.schema = Some(s);
                }
            }
            "role" => {
                if let FieldValue::String(s) = value
                    && !s.is_empty()
                {
                    self.role = Some(s);
                }
            }
            "password" => {
                if let FieldValue::String(s) = value {
                    self.password = Some(s);
                }
            }
            "private_key_path" => {
                if let FieldValue::String(s) = value {
                    self.private_key_path = Some(PathBuf::from(s));
                }
            }
            "private_key" => {
                if let FieldValue::String(s) = value {
                    self.private_key = Some(s);
                }
            }
            "private_key_passphrase" => {
                if let FieldValue::String(s) = value {
                    self.private_key_passphrase = Some(s);
                }
            }
            "auth_method" => {
                if let FieldValue::Integer(auth_method) = value {
                    match auth_method {
                        2 => self.authenticator = Some("externalbrowser".to_string()),
                        3 => self.authenticator = Some("username_password_mfa".to_string()),
                        _ => self.authenticator = None,
                    }
                }
            }
            _ => {} // Ignore temporary fields
        }
        Ok(())
    }

    fn get_field(&self, field_name: &str) -> Option<FieldValue> {
        match field_name {
            "account" => self.account.as_ref().map(|s| FieldValue::String(s.clone())),
            "user" => self.user.as_ref().map(|s| FieldValue::String(s.clone())),
            "database" => self
                .database
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "warehouse" => self
                .warehouse
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "schema" => self.schema.as_ref().map(|s| FieldValue::String(s.clone())),
            "role" => self.role.as_ref().map(|s| FieldValue::String(s.clone())),
            "authenticator" => self
                .authenticator
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "auth_method" => {
                if self.password.is_some()
                    && self.private_key_path.is_none()
                    && self.private_key.is_none()
                    && self
                        .authenticator
                        .as_ref()
                        .is_none_or(|a| a != "externalbrowser" && a != "username_password_mfa")
                {
                    Some(FieldValue::Integer(0))
                } else if self.private_key_path.is_some() || self.private_key.is_some() {
                    Some(FieldValue::Integer(1))
                } else if self
                    .authenticator
                    .as_ref()
                    .is_some_and(|a| a == "externalbrowser")
                {
                    Some(FieldValue::Integer(2))
                } else if self
                    .authenticator
                    .as_ref()
                    .is_some_and(|a| a == "username_password_mfa")
                {
                    Some(FieldValue::Integer(3))
                } else {
                    None
                }
            }
            "use_key_path" => {
                if self.private_key_path.is_some() {
                    Some(FieldValue::Boolean(true))
                } else if self.private_key.is_some() {
                    Some(FieldValue::Boolean(false))
                } else {
                    None
                }
            }
            "needs_passphrase" => self
                .private_key_passphrase
                .as_ref()
                .map(|_| FieldValue::Boolean(true)),
            _ => None,
        }
    }

    fn is_field_set(&self, field_name: &str) -> bool {
        match field_name {
            "account" => self.account.is_some(),
            "user" => self.user.is_some(),
            "database" => self.database.is_some(),
            "warehouse" => self.warehouse.is_some(),
            "schema" => self.schema.is_some(),
            "role" => self.role.is_some(),
            "authenticator" => self.authenticator.is_some(),
            _ => false,
        }
    }
}

pub fn setup_snowflake_profile(
    existing_config: Option<&SnowflakeDbConfig>,
) -> FsResult<Box<SnowflakeDbConfig>> {
    let default_config = SnowflakeDbConfig::default();
    let mut config = ConfigProcessor::process_config(existing_config.or(Some(&default_config)))?;

    if config.threads.is_none() {
        config.threads = Some(StringOrInteger::Integer(16));
    }

    Ok(Box::new(config))
}
