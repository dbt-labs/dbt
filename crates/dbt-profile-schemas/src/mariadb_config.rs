use super::common::*;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::profiles::MariaDbConfig;
use dbt_schemas::schemas::serde::StringOrInteger;

impl InteractiveSetup for MariaDbConfig {
    fn get_fields() -> Vec<ConfigField> {
        vec![
            ConfigField {
                name: "host".to_string(),
                field_type: FieldType::Input {
                    default: Some("localhost".to_string()),
                },
                condition: FieldCondition::Always,
                prompt: "Host".to_string(),
                required: true,
            },
            ConfigField {
                name: "port".to_string(),
                field_type: FieldType::Input {
                    default: Some("3306".to_string()),
                },
                condition: FieldCondition::Always,
                prompt: "Port".to_string(),
                required: false,
            },
            ConfigField {
                name: "user".to_string(),
                field_type: FieldType::Input { default: None },
                condition: FieldCondition::Always,
                prompt: "Username".to_string(),
                required: true,
            },
            ConfigField {
                name: "password".to_string(),
                field_type: FieldType::Password,
                condition: FieldCondition::Always,
                prompt: "Password".to_string(),
                required: true,
            },
            ConfigField {
                name: "schema".to_string(),
                field_type: FieldType::Input { default: None },
                condition: FieldCondition::Always,
                prompt: "Schema (MariaDB database, created on first run if missing)".to_string(),
                required: true,
            },
            ConfigField {
                name: "ssl_mode".to_string(),
                field_type: FieldType::Input {
                    default: Some("preferred".to_string()),
                },
                condition: FieldCondition::Always,
                prompt: "TLS mode (preferred, required, verify_ca, disabled)".to_string(),
                required: false,
            },
        ]
    }

    fn set_field(&mut self, field_name: &str, value: FieldValue) -> FsResult<()> {
        match field_name {
            "host" => {
                if let FieldValue::String(val) = value {
                    self.host = Some(val);
                }
            }
            "port" => match value {
                FieldValue::String(val) => {
                    if let Ok(port) = val.parse::<i64>() {
                        self.port = Some(StringOrInteger::Integer(port));
                    }
                }
                FieldValue::Integer(val) => {
                    self.port = Some(StringOrInteger::Integer(val));
                }
                _ => {}
            },
            "user" => {
                if let FieldValue::String(val) = value {
                    self.user = Some(val);
                }
            }
            "password" => {
                if let FieldValue::String(val) = value {
                    self.password = Some(val);
                }
            }
            "schema" => {
                if let FieldValue::String(val) = value {
                    self.schema = Some(val);
                }
            }
            "ssl_mode" => {
                if let FieldValue::String(val) = value {
                    self.ssl_mode = Some(val);
                }
            }
            _ => {
                return Err(fs_err!(
                    ErrorCode::InvalidArgument,
                    "Unknown field: {}",
                    field_name
                ));
            }
        }
        Ok(())
    }

    fn get_field(&self, field_name: &str) -> Option<FieldValue> {
        match field_name {
            "host" => self.host.as_ref().map(|v| FieldValue::String(v.clone())),
            "port" => self.port.as_ref().map(|v| match v {
                StringOrInteger::String(s) => FieldValue::String(s.clone()),
                StringOrInteger::Integer(i) => FieldValue::Integer(*i),
            }),
            "user" => self.user.as_ref().map(|v| FieldValue::String(v.clone())),
            "password" => self
                .password
                .as_ref()
                .map(|v| FieldValue::String(v.clone())),
            "schema" => self.schema.as_ref().map(|v| FieldValue::String(v.clone())),
            "ssl_mode" => self
                .ssl_mode
                .as_ref()
                .map(|v| FieldValue::String(v.clone())),
            _ => None,
        }
    }

    fn is_field_set(&self, field_name: &str) -> bool {
        match field_name {
            "host" => self.host.is_some(),
            "port" => self.port.is_some(),
            "user" => self.user.is_some(),
            "password" => self.password.is_some(),
            "schema" => self.schema.is_some(),
            "ssl_mode" => self.ssl_mode.is_some(),
            _ => false,
        }
    }
}

pub(crate) fn setup_mariadb_profile(
    existing_config: Option<&MariaDbConfig>,
) -> FsResult<Box<MariaDbConfig>> {
    let default_config = MariaDbConfig::default();
    let mut config = ConfigProcessor::process_config(existing_config.or(Some(&default_config)))?;

    if config.threads.is_none() {
        config.threads = Some(StringOrInteger::Integer(4));
    }

    Ok(Box::new(config))
}
