use super::common::{ConfigField, ConfigProcessor, FieldValue, InteractiveSetup};
use dbt_common::FsResult;
use dbt_schemas::schemas::profiles::DatabricksDbConfig;
use dbt_schemas::schemas::serde::StringOrInteger;

impl InteractiveSetup for DatabricksDbConfig {
    fn get_fields() -> Vec<ConfigField> {
        vec![
            // Core connection settings
            ConfigField::input("host", "Host (Databricks instance hostname)"),
            ConfigField::input("http_path", "HTTP Path (SQL warehouse or cluster path)"),
            ConfigField::input("schema", "Schema"),
            ConfigField::optional_input("catalog", "Catalog (optional)", None),
            // Authentication
            ConfigField::select(
                "auth_method",
                "Which authentication method would you like to use?",
                vec!["Personal Access Token", "OAuth"],
                0,
            ),
            ConfigField::password("token", "Personal Access Token")
                .when_field_equals("auth_method", FieldValue::Integer(0)),
        ]
    }

    fn set_field(&mut self, field_name: &str, value: FieldValue) -> FsResult<()> {
        match field_name {
            "host" => {
                if let FieldValue::String(s) = value {
                    self.host = Some(s);
                }
            }
            "http_path" => {
                if let FieldValue::String(s) = value {
                    self.http_path = Some(s);
                }
            }
            "schema" => {
                if let FieldValue::String(s) = value {
                    self.schema = Some(s);
                }
            }
            "catalog" => {
                if let FieldValue::String(s) = value
                    && !s.is_empty()
                {
                    self.database = Some(s);
                }
            }
            "token" => {
                if let FieldValue::String(s) = value {
                    self.token = Some(s);
                    self.auth_type = Some("token".to_string());
                }
            }
            "auth_method" => {
                if let FieldValue::Integer(auth_method) = value {
                    match auth_method {
                        0 => {} // Personal Access Token - auth_type will be set when token is provided
                        1 => self.auth_type = Some("oauth".to_string()), // OAuth
                        _ => {}
                    }
                }
            }
            _ => {} // Ignore temporary fields
        }
        Ok(())
    }

    fn get_field(&self, field_name: &str) -> Option<FieldValue> {
        match field_name {
            "host" => self.host.as_ref().map(|s| FieldValue::String(s.clone())),
            "http_path" => self
                .http_path
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "schema" => self.schema.as_ref().map(|s| FieldValue::String(s.clone())),
            "catalog" => self
                .database
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "token" => self.token.as_ref().map(|s| FieldValue::String(s.clone())),
            "auth_method" => {
                if self.token.is_some() {
                    Some(FieldValue::Integer(0))
                } else if self.auth_type.as_ref().is_some_and(|a| a == "oauth") {
                    Some(FieldValue::Integer(1))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn is_field_set(&self, field_name: &str) -> bool {
        match field_name {
            "host" => self.host.is_some(),
            "http_path" => self.http_path.is_some(),
            "schema" => self.schema.is_some(),
            "catalog" => self.database.is_some(),
            "token" => self.token.is_some(),
            _ => false,
        }
    }
}

pub fn setup_databricks_profile(
    existing_config: Option<&DatabricksDbConfig>,
) -> FsResult<Box<DatabricksDbConfig>> {
    let default_config = DatabricksDbConfig {
        database: None,
        schema: None,
        host: None,
        http_path: None,
        token: None,
        client_id: None,
        client_secret: None,
        azure_client_id: None,
        azure_client_secret: None,
        azure_tenant_id: None,
        oauth_redirect_url: None,
        oauth_scopes: None,
        query_tags: None,
        session_properties: None,
        connection_parameters: None,
        auth_type: None,
        compute: None,
        connect_retries: None,
        connect_timeout: None,
        retry_all: None,
        connect_max_idle: None,
        threads: None,
    };
    let mut config = ConfigProcessor::process_config(existing_config.or(Some(&default_config)))?;

    if config.threads.is_none() {
        config.threads = Some(StringOrInteger::Integer(16));
    }

    Ok(Box::new(config))
}
