use super::common::*;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::profiles::GizmoSQLDbConfig;
use dbt_schemas::schemas::serde::StringOrInteger;

const AUTH_TYPE_PASSWORD: &str = "password";
const AUTH_TYPE_EXTERNAL: &str = "external";

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
                vec![AUTH_TYPE_PASSWORD, AUTH_TYPE_EXTERNAL],
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
            "host" => set_if_some(&mut self.host, as_string(value)),
            "port" => set_if_some(&mut self.port, as_port(value)),
            "auth_type" => set_if_some(&mut self.auth_type, as_auth_type(value)),
            "username" => set_if_some(&mut self.username, as_string(value)),
            "password" => set_if_some(&mut self.password, as_string(value)),
            "database" => set_if_some(&mut self.database, as_string(value)),
            "schema" => set_if_some(&mut self.schema, as_string(value)),
            "use_encryption" => set_if_some(&mut self.use_encryption, as_bool(value)),
            "tls_skip_verify" => set_if_some(&mut self.tls_skip_verify, as_bool(value)),
            "external_root" => set_if_some(&mut self.external_root, as_string(value)),
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
            "auth_type" => self.auth_type.as_deref().map(|auth_type| {
                FieldValue::Integer(if auth_type == AUTH_TYPE_EXTERNAL {
                    1
                } else {
                    0
                })
            }),
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

/// Overwrite `slot` only when the prompt produced a value of the right type, so a
/// mismatched answer leaves any existing profile value in place.
fn set_if_some<T>(slot: &mut Option<T>, value: Option<T>) {
    if value.is_some() {
        *slot = value;
    }
}

fn as_string(value: FieldValue) -> Option<String> {
    match value {
        FieldValue::String(val) => Some(val),
        _ => None,
    }
}

fn as_bool(value: FieldValue) -> Option<bool> {
    match value {
        FieldValue::Boolean(val) => Some(val),
        _ => None,
    }
}

fn as_port(value: FieldValue) -> Option<StringOrInteger> {
    match value {
        FieldValue::String(val) => val.parse::<i64>().ok().map(StringOrInteger::Integer),
        FieldValue::Integer(val) => Some(StringOrInteger::Integer(val)),
        _ => None,
    }
}

/// The select prompt answers with an option index (`0` password, `1` external);
/// an existing profile supplies the name itself.
fn as_auth_type(value: FieldValue) -> Option<String> {
    match value {
        FieldValue::Integer(1) => Some(AUTH_TYPE_EXTERNAL.to_string()),
        FieldValue::Integer(_) => Some(AUTH_TYPE_PASSWORD.to_string()),
        FieldValue::String(val) => Some(val),
        _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_field_maps_prompt_answers_onto_the_config() {
        let mut config = GizmoSQLDbConfig::default();
        config
            .set_field("host", FieldValue::String("localhost".to_string()))
            .unwrap();
        config
            .set_field("port", FieldValue::String("31337".to_string()))
            .unwrap();
        config
            .set_field("auth_type", FieldValue::Integer(1))
            .unwrap();
        config
            .set_field("use_encryption", FieldValue::Boolean(false))
            .unwrap();

        assert_eq!(config.host.as_deref(), Some("localhost"));
        assert_eq!(config.port, Some(StringOrInteger::Integer(31337)));
        assert_eq!(config.auth_type.as_deref(), Some(AUTH_TYPE_EXTERNAL));
        assert_eq!(config.use_encryption, Some(false));
    }

    #[test]
    fn set_field_keeps_existing_value_on_mismatched_answer() {
        let mut config = GizmoSQLDbConfig {
            host: Some("gizmosql.example.com".to_string()),
            port: Some(StringOrInteger::Integer(31337)),
            ..Default::default()
        };
        config.set_field("host", FieldValue::Boolean(true)).unwrap();
        config
            .set_field("port", FieldValue::String("not-a-port".to_string()))
            .unwrap();

        assert_eq!(config.host.as_deref(), Some("gizmosql.example.com"));
        assert_eq!(config.port, Some(StringOrInteger::Integer(31337)));
    }

    #[test]
    fn set_field_rejects_unknown_fields() {
        let mut config = GizmoSQLDbConfig::default();
        assert!(
            config
                .set_field("warehouse", FieldValue::String("x".to_string()))
                .is_err()
        );
    }
}
