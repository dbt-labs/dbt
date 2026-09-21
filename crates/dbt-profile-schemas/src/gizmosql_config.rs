use super::common::*;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::profiles::GizmoSQLDbConfig;
use dbt_schemas::schemas::serde::StringOrInteger;

impl InteractiveSetup for GizmoSQLDbConfig {
    fn get_fields() -> Vec<ConfigField> {
        vec![
            ConfigField {
                name: "host".to_string(),
                field_type: FieldType::Input {
                    default: Some("localhost".to_string()),
                },
                condition: FieldCondition::Always,
                prompt: "Host (hostname of the GizmoSQL server)".to_string(),
                required: true,
            },
            ConfigField {
                name: "port".to_string(),
                field_type: FieldType::Input {
                    default: Some("31337".to_string()),
                },
                condition: FieldCondition::Always,
                prompt: "Port (Arrow Flight SQL port)".to_string(),
                required: false,
            },
            ConfigField::select(
                "auth_type",
                "Authentication (password, or external for the OAuth/SSO browser flow)",
                vec!["password", "external"],
                0,
            ),
            ConfigField::input("username", "Username")
                .when_field_equals("auth_type", FieldValue::Integer(0)),
            ConfigField::password("password", "Password")
                .when_field_equals("auth_type", FieldValue::Integer(0)),
            ConfigField {
                name: "database".to_string(),
                field_type: FieldType::Input { default: None },
                condition: FieldCondition::Always,
                prompt: "Database (the DuckDB catalog name on the server)".to_string(),
                required: true,
            },
            ConfigField {
                name: "schema".to_string(),
                field_type: FieldType::Input {
                    default: Some("main".to_string()),
                },
                condition: FieldCondition::Always,
                prompt: "Schema (created on first run if missing)".to_string(),
                required: true,
            },
            ConfigField {
                name: "use_encryption".to_string(),
                field_type: FieldType::Confirm { default: true },
                condition: FieldCondition::Always,
                prompt: "Connect over TLS?".to_string(),
                required: true,
            },
            ConfigField {
                name: "tls_skip_verify".to_string(),
                field_type: FieldType::Confirm { default: false },
                condition: FieldCondition::Always,
                prompt: "Skip TLS certificate verification? (answer yes for self-signed setups)"
                    .to_string(),
                required: true,
            },
            ConfigField::input(
                "external_root",
                "Root path on the server for external materializations (optional)",
            )
            .optional(),
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
            "auth_type" => match value {
                FieldValue::Integer(index) => {
                    self.auth_type =
                        Some(if index == 1 { "external" } else { "password" }.to_string());
                }
                FieldValue::String(val) => {
                    self.auth_type = Some(val);
                }
                _ => {}
            },
            "username" => {
                if let FieldValue::String(val) = value {
                    self.username = Some(val);
                }
            }
            "password" => {
                if let FieldValue::String(val) = value {
                    self.password = Some(val);
                }
            }
            "database" => {
                if let FieldValue::String(val) = value {
                    self.database = Some(val);
                }
            }
            "schema" => {
                if let FieldValue::String(val) = value {
                    self.schema = Some(val);
                }
            }
            "use_encryption" => {
                if let FieldValue::Boolean(val) = value {
                    self.use_encryption = Some(val);
                }
            }
            "tls_skip_verify" => {
                if let FieldValue::Boolean(val) = value {
                    self.tls_skip_verify = Some(val);
                }
            }
            "external_root" => {
                if let FieldValue::String(val) = value {
                    self.external_root = Some(val);
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
            "auth_type" => self
                .auth_type
                .as_deref()
                .map(|auth_type| FieldValue::Integer(if auth_type == "external" { 1 } else { 0 })),
            "username" => self
                .username
                .as_ref()
                .map(|v| FieldValue::String(v.clone())),
            "password" => self
                .password
                .as_ref()
                .map(|v| FieldValue::String(v.clone())),
            "database" => self
                .database
                .as_ref()
                .map(|v| FieldValue::String(v.clone())),
            "schema" => self.schema.as_ref().map(|v| FieldValue::String(v.clone())),
            "use_encryption" => self.use_encryption.map(FieldValue::Boolean),
            "tls_skip_verify" => self.tls_skip_verify.map(FieldValue::Boolean),
            "external_root" => self
                .external_root
                .as_ref()
                .map(|v| FieldValue::String(v.clone())),
            _ => None,
        }
    }

    fn is_field_set(&self, field_name: &str) -> bool {
        match field_name {
            "host" => self.host.is_some(),
            "port" => self.port.is_some(),
            "auth_type" => self.auth_type.is_some(),
            "username" => self.username.is_some(),
            "password" => self.password.is_some(),
            "database" => self.database.is_some(),
            "schema" => self.schema.is_some(),
            "use_encryption" => self.use_encryption.is_some(),
            "tls_skip_verify" => self.tls_skip_verify.is_some(),
            "external_root" => self.external_root.is_some(),
            _ => false,
        }
    }
}

pub fn setup_gizmosql_profile(
    existing_config: Option<&GizmoSQLDbConfig>,
) -> FsResult<Box<GizmoSQLDbConfig>> {
    let default_config = GizmoSQLDbConfig::default();
    let mut config = ConfigProcessor::process_config(existing_config.or(Some(&default_config)))?;

    if config.threads.is_none() {
        config.threads = Some(StringOrInteger::Integer(16));
    }

    Ok(Box::new(config))
}
