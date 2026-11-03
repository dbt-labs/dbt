use super::common::*;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::profiles::SingleStoreDbConfig;
use dbt_schemas::schemas::serde::StringOrInteger;

fn parse_string_field(value: FieldValue) -> Option<String> {
    match value {
        FieldValue::String(val) if !val.is_empty() => Some(val),
        _ => None,
    }
}

fn parse_port_field(value: FieldValue) -> Option<StringOrInteger> {
    match value {
        FieldValue::Integer(val) => Some(StringOrInteger::Integer(val)),
        FieldValue::String(val) => val.parse::<i64>().ok().map(StringOrInteger::Integer),
        _ => None,
    }
}

fn parse_bool_field(value: FieldValue) -> Option<bool> {
    match value {
        FieldValue::Boolean(val) => Some(val),
        FieldValue::String(val) => Some(!matches!(
            val.to_ascii_lowercase().as_str(),
            "false" | "0" | "no" | "off"
        )),
        _ => None,
    }
}

impl InteractiveSetup for SingleStoreDbConfig {
    fn get_fields() -> Vec<ConfigField> {
        vec![
            ConfigField {
                name: "host".to_string(),
                field_type: FieldType::Input { default: None },
                condition: FieldCondition::Always,
                prompt: "Host (hostname)".to_string(),
                required: true,
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
                required: false,
            },
            ConfigField {
                name: "port".to_string(),
                field_type: FieldType::Input {
                    default: Some("3306".to_string()),
                },
                condition: FieldCondition::Always,
                prompt: "Port".to_string(),
                required: true,
            },
            ConfigField {
                name: "database".to_string(),
                field_type: FieldType::Input { default: None },
                condition: FieldCondition::Always,
                prompt: "Database name".to_string(),
                required: true,
            },
            ConfigField {
                name: "schema".to_string(),
                field_type: FieldType::Input { default: None },
                condition: FieldCondition::Always,
                prompt: "Schema (defaults to database name in SingleStore)".to_string(),
                required: false,
            },
            ConfigField {
                name: "ssl_mode".to_string(),
                field_type: FieldType::Input { default: None },
                condition: FieldCondition::Always,
                prompt: "SSL mode (e.g. Disabled, Preferred, Required, VerifyCa, VerifyIdentity)"
                    .to_string(),
                required: false,
            },
        ]
    }

    fn set_field(&mut self, field_name: &str, value: FieldValue) -> FsResult<()> {
        match field_name {
            "host" => self.host = parse_string_field(value),
            "user" => self.user = parse_string_field(value),
            "password" => self.password = parse_string_field(value),
            "token" | "jwt" => self.token = parse_string_field(value),
            "port" => self.port = parse_port_field(value),
            "database" => self.database = parse_string_field(value),
            "schema" => self.schema = parse_string_field(value),
            "threads" => self.threads = parse_port_field(value),
            "retries" => self.retries = parse_port_field(value),
            "connect_timeout" | "timeout" => self.connect_timeout = parse_port_field(value),
            "ssl_mode" | "sslmode" => self.ssl_mode = parse_string_field(value),
            "ssl_ca" | "sslrootcert" => self.ssl_ca = parse_string_field(value),
            "ssl_cert" => self.ssl_cert = parse_string_field(value),
            "ssl_key" => self.ssl_key = parse_string_field(value),
            "allow_cleartext_plugin" | "cleartext_plugin" => {
                self.allow_cleartext_plugin = parse_bool_field(value)
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
            "user" => self.user.as_ref().map(|v| FieldValue::String(v.clone())),
            "password" => self
                .password
                .as_ref()
                .map(|v| FieldValue::String(v.clone())),
            "token" | "jwt" => self.token.as_ref().map(|v| FieldValue::String(v.clone())),
            "port" => self.port.as_ref().map(|v| match v {
                StringOrInteger::String(s) => FieldValue::String(s.clone()),
                StringOrInteger::Integer(i) => FieldValue::Integer(*i),
            }),
            "database" => self
                .database
                .as_ref()
                .map(|v| FieldValue::String(v.clone())),
            "schema" => self.schema.as_ref().map(|v| FieldValue::String(v.clone())),
            "threads" => self.threads.as_ref().map(|v| match v {
                StringOrInteger::String(s) => FieldValue::String(s.clone()),
                StringOrInteger::Integer(i) => FieldValue::Integer(*i),
            }),
            "retries" => self.retries.as_ref().map(|v| match v {
                StringOrInteger::String(s) => FieldValue::String(s.clone()),
                StringOrInteger::Integer(i) => FieldValue::Integer(*i),
            }),
            "connect_timeout" | "timeout" => self.connect_timeout.as_ref().map(|v| match v {
                StringOrInteger::String(s) => FieldValue::String(s.clone()),
                StringOrInteger::Integer(i) => FieldValue::Integer(*i),
            }),
            "ssl_mode" | "sslmode" => self
                .ssl_mode
                .as_ref()
                .map(|v| FieldValue::String(v.clone())),
            "ssl_ca" | "sslrootcert" => self.ssl_ca.as_ref().map(|v| FieldValue::String(v.clone())),
            "ssl_cert" => self
                .ssl_cert
                .as_ref()
                .map(|v| FieldValue::String(v.clone())),
            "ssl_key" => self.ssl_key.as_ref().map(|v| FieldValue::String(v.clone())),
            "allow_cleartext_plugin" | "cleartext_plugin" => {
                self.allow_cleartext_plugin.map(FieldValue::Boolean)
            }
            _ => None,
        }
    }

    fn is_field_set(&self, field_name: &str) -> bool {
        match field_name {
            "host" => self.host.is_some(),
            "user" => self.user.is_some(),
            "password" => self.password.is_some(),
            "token" | "jwt" => self.token.is_some(),
            "port" => self.port.is_some(),
            "database" => self.database.is_some(),
            "schema" => self.schema.is_some(),
            "threads" => self.threads.is_some(),
            "retries" => self.retries.is_some(),
            "connect_timeout" | "timeout" => self.connect_timeout.is_some(),
            "ssl_mode" | "sslmode" => self.ssl_mode.is_some(),
            "ssl_ca" | "sslrootcert" => self.ssl_ca.is_some(),
            "ssl_cert" => self.ssl_cert.is_some(),
            "ssl_key" => self.ssl_key.is_some(),
            "allow_cleartext_plugin" | "cleartext_plugin" => self.allow_cleartext_plugin.is_some(),
            _ => false,
        }
    }
}

pub fn setup_singlestore_profile(
    existing_config: Option<&SingleStoreDbConfig>,
) -> FsResult<Box<SingleStoreDbConfig>> {
    let default_config = SingleStoreDbConfig::default();
    let mut config = ConfigProcessor::process_config(existing_config.or(Some(&default_config)))?;

    if config.threads.is_none() {
        config.threads = Some(StringOrInteger::Integer(16));
    }
    if config.schema.is_none() {
        config.schema = config.database.clone();
    }

    Ok(Box::new(config))
}
