use super::common::*;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::profiles::ExasolDbConfig;
use dbt_schemas::schemas::serde::StringOrInteger;

impl InteractiveSetup for ExasolDbConfig {
    fn get_fields() -> Vec<ConfigField> {
        vec![
            ConfigField {
                name: "host".to_string(),
                field_type: FieldType::Input {
                    default: Some("localhost".to_string()),
                },
                condition: FieldCondition::Always,
                prompt: "Host (hostname or connection string)".to_string(),
                required: true,
            },
            ConfigField {
                name: "port".to_string(),
                field_type: FieldType::Input {
                    default: Some("8563".to_string()),
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
                prompt: "Schema (created on first run if missing)".to_string(),
                required: true,
            },
            ConfigField {
                name: "certificate_validation".to_string(),
                field_type: FieldType::Confirm { default: true },
                condition: FieldCondition::Always,
                prompt:
                    "Validate the server TLS certificate? (answer no for Docker/self-signed setups)"
                        .to_string(),
                required: true,
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
            "certificate_validation" => {
                if let FieldValue::Boolean(val) = value {
                    self.certificate_validation = Some(val);
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
            "certificate_validation" => self.certificate_validation.map(FieldValue::Boolean),
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
            "certificate_validation" => self.certificate_validation.is_some(),
            _ => false,
        }
    }
}

pub fn setup_exasol_profile(
    existing_config: Option<&ExasolDbConfig>,
) -> FsResult<Box<ExasolDbConfig>> {
    let default_config = ExasolDbConfig::default();
    let mut config = ConfigProcessor::process_config(existing_config.or(Some(&default_config)))?;

    if config.threads.is_none() {
        config.threads = Some(StringOrInteger::Integer(16));
    }

    Ok(Box::new(config))
}
