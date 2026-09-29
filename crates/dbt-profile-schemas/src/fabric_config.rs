use crate::common::{ConfigField, ConfigProcessor, FieldValue, InteractiveSetup};

use dbt_common::FsResult;
use dbt_schemas::schemas::profiles::FabricDbConfig;

// Index → authentication string. Order is the order the user sees in the select prompt.
//
// Values come from `dbt-fabric/dbt/adapters/fabric/fabric_connection_manager.py`
const AUTH_METHODS: &[(&str, &str)] = &[
    (
        "ActiveDirectoryServicePrincipal",
        "Active Directory Service Principal",
    ),
    // TODO: this is supported in `dbt-auth`
    // but it is disabled for now since I didn't find the right credentials to test it
    // ("ActiveDirectoryPassword", "Active Directory Password")
    (
        "environment",
        "Environment (DefaultAzureCredential env vars, see https://learn.microsoft.com/en-us/python/api/azure-identity/azure.identity.environmentcredential, explain the available combinations of environment variables you can use to authenticate.)",
    ),
    (
        "ActiveDirectoryAccessToken",
        "Active Directory Access Token",
    ),
    // ("CLI", "Azure CLI"),
    // (
    //     "ActiveDirectoryInteractive",
    //     "Active Directory Interactive (browser)",
    // ),
    // ("sql", "SQL Authentication"),
    // ("auto", "Auto (DefaultAzureCredential chain)"),
];

impl InteractiveSetup for FabricDbConfig {
    fn get_fields() -> Vec<ConfigField> {
        let auth_labels = auth_label_options();
        // Default to Service Principal (matches the credentials most CI/dev
        // users have on hand; CLI is a close second).
        let auth_default = 0;

        // Index lookups for auth-dependent fields.
        let sp_idx = auth_index("ActiveDirectoryServicePrincipal").unwrap_or(0);
        let token_idx = auth_index("ActiveDirectoryAccessToken").unwrap_or(0);
        // let adpw_idx = auth_index("ActiveDirectoryPassword").unwrap_or(0);

        vec![
            // Core connection settings
            ConfigField::input(
                "host",
                "Host (Fabric SQL endpoint, e.g. <guid>.datawarehouse.fabric.microsoft.com)",
            ),
            ConfigField::input("database", "Database (warehouse displayName)"),
            ConfigField::input("schema", "Schema"),
            // Authentication
            ConfigField::select(
                "auth_method",
                "Which authentication method would you like to use?",
                auth_labels,
                auth_default,
            ),
            // Service Principal fields
            ConfigField::input("tenant_id", "Tenant ID")
                .when_field_equals("auth_method", FieldValue::Integer(sp_idx)),
            ConfigField::input("client_id", "Client ID (app registration)")
                .when_field_equals("auth_method", FieldValue::Integer(sp_idx)),
            ConfigField::password("client_secret", "Client secret")
                .when_field_equals("auth_method", FieldValue::Integer(sp_idx)),
            // Access Token field
            ConfigField::password("access_token", "Access token")
                .when_field_equals("auth_method", FieldValue::Integer(token_idx)),
            // TODO: this is supported in `dbt-auth`
            // but it is disabled for now since I didn't find the right credentials to test it
            // Active Directory Password
            // ConfigField::input("user_adpw", "Username (Entra account)")
            //     .when_field_equals("auth_method", FieldValue::Integer(adpw_idx)),
            // ConfigField::password("password_adpw", "Password")
            //     .when_field_equals("auth_method", FieldValue::Integer(adpw_idx)),
            // ConfigField::input("client_id", "Client ID (app registration)")
            //     .when_field_equals("auth_method", FieldValue::Integer(adpw_idx)),
        ]
    }

    fn set_field(&mut self, field_name: &str, value: FieldValue) -> FsResult<()> {
        match field_name {
            "host" => {
                if let FieldValue::String(s) = value {
                    self.host = Some(s);
                }
            }
            "database" => {
                if let FieldValue::String(s) = value {
                    self.database = Some(s);
                }
            }
            "schema" => {
                if let FieldValue::String(s) = value {
                    self.schema = Some(s);
                }
            }
            "auth_method" => {
                if let FieldValue::Integer(i) = value
                    && let Some((val, _)) = AUTH_METHODS.get(i as usize)
                {
                    self.authentication = Some((*val).to_string());
                }
            }
            "tenant_id" => {
                if let FieldValue::String(s) = value {
                    self.tenant_id = Some(s);
                }
            }
            "client_id" => {
                if let FieldValue::String(s) = value {
                    self.client_id = Some(s);
                }
            }
            "client_secret" => {
                if let FieldValue::String(s) = value {
                    self.client_secret = Some(s);
                }
            }
            "access_token" => {
                if let FieldValue::String(s) = value {
                    self.access_token = Some(s);
                }
            }
            "user_adpw" => {
                if let FieldValue::String(s) = value
                    && !s.is_empty()
                {
                    self.user = Some(s);
                }
            }
            "password_adpw" => {
                if let FieldValue::String(s) = value {
                    self.password = Some(s);
                }
            }
            _ => {} // Ignore temporary or unrecognized fields
        }
        Ok(())
    }

    fn get_field(&self, field_name: &str) -> Option<FieldValue> {
        match field_name {
            "host" => self.host.as_ref().map(|s| FieldValue::String(s.clone())),
            "database" => self
                .database
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "schema" => self.schema.as_ref().map(|s| FieldValue::String(s.clone())),
            "auth_method" => self
                .authentication
                .as_deref()
                .and_then(auth_index)
                .map(FieldValue::Integer),
            "tenant_id" => self
                .tenant_id
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "client_id" => self
                .client_id
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "client_secret" => self
                .client_secret
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "access_token" => self
                .access_token
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "user_adpw" => self.user.as_ref().map(|s| FieldValue::String(s.clone())),
            "password_adpw" => self
                .password
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            _ => None,
        }
    }

    fn is_field_set(&self, field_name: &str) -> bool {
        match field_name {
            "host" => self.host.is_some(),
            "database" => self.database.is_some(),
            "schema" => self.schema.is_some(),
            "auth_method" => self
                .authentication
                .as_deref()
                .map(auth_index)
                .is_some_and(|o| o.is_some()),
            "tenant_id" => self.tenant_id.is_some(),
            "client_id" => self.client_id.is_some(),
            "client_secret" => self.client_secret.is_some(),
            "access_token" => self.access_token.is_some(),
            "user_adpw" => self.user.is_some(),
            "password_adpw" => self.password.is_some(),
            _ => false,
        }
    }
}

fn auth_index(value: &str) -> Option<i64> {
    AUTH_METHODS
        .iter()
        .position(|(v, _)| v.eq_ignore_ascii_case(value))
        .map(|i| i as i64)
}

fn auth_label_options() -> Vec<&'static str> {
    AUTH_METHODS.iter().map(|(_, label)| *label).collect()
}

pub fn default_fabric_config() -> FabricDbConfig {
    FabricDbConfig {
        driver: None,
        host: None,
        database: None,
        schema: None,
        user: None,
        password: None,
        windows_login: None,
        trace_flag: None,
        tenant_id: None,
        client_id: None,
        client_secret: None,
        access_token: None,
        access_token_expires_on: None,
        authentication: Some("ActiveDirectoryServicePrincipal".to_string()),
        encrypt: Some(true),
        trust_cert: Some(false),
        retries: None,
        schema_authorization: None,
        login_timeout: None,
        query_timeout: None,
        workspace_id: None,
        warehouse_snapshot_name: None,
        warehouse_snapshot_id: None,
        snapshot_timestamp: None,
        api_url: None,
    }
}

pub fn setup_fabric_profile(
    existing_config: Option<&FabricDbConfig>,
) -> FsResult<Box<FabricDbConfig>> {
    let default_config = default_fabric_config();
    let config = ConfigProcessor::process_config(existing_config.or(Some(&default_config)))?;
    Ok(Box::new(config))
}
