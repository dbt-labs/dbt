use crate::common::{ConfigField, ConfigProcessor, FieldValue, InteractiveSetup};

use dbt_common::FsResult;
use dbt_schemas::schemas::profiles::SqlServerDbConfig;
use dbt_schemas::schemas::serde::StringOrInteger;

// Index → authentication string. Order is the order the user sees in the select prompt.
// SQL Server, unlike Fabric, defaults to native SQL auth (v1's `sqlserver_credentials.py`
// default), and every method below is implemented in `dbt-auth/src/sqlserver/mod.rs`
// (`parse_auth`), so none need to stay commented out pending untested credentials.
const AUTH_METHODS: &[(&str, &str)] = &[
    (
        "sql",
        "SQL Authentication (native SQL Server login, UID/PWD)",
    ),
    (
        "ActiveDirectoryServicePrincipal",
        "Active Directory Service Principal",
    ),
    ("ActiveDirectoryPassword", "Active Directory Password"),
    (
        "environment",
        "Environment (DefaultAzureCredential env vars, see https://learn.microsoft.com/en-us/python/api/azure-identity/azure.identity.environmentcredential, explain the available combinations of environment variables you can use to authenticate.)",
    ),
    (
        "ActiveDirectoryAccessToken",
        "Active Directory Access Token",
    ),
];

impl InteractiveSetup for SqlServerDbConfig {
    fn get_fields() -> Vec<ConfigField> {
        let auth_labels = auth_label_options();
        // Default to SQL Authentication (matches v1's default and the common
        // on-prem/Azure SQL Database case of a native login already on hand).
        let auth_default = auth_index("sql").unwrap_or(0) as usize;

        // Index lookups for auth-dependent fields.
        let sql_idx = auth_index("sql").unwrap_or(0);
        let sp_idx = auth_index("ActiveDirectoryServicePrincipal").unwrap_or(0);
        let adpw_idx = auth_index("ActiveDirectoryPassword").unwrap_or(0);
        let token_idx = auth_index("ActiveDirectoryAccessToken").unwrap_or(0);

        vec![
            // Core connection settings
            ConfigField::input(
                "host",
                "Host (server hostname, e.g. localhost or my-server.database.windows.net)",
            ),
            ConfigField::optional_input("port", "Port", Some("1433")),
            ConfigField::input("database", "Database"),
            ConfigField::input("schema", "Schema"),
            // Authentication
            ConfigField::select(
                "authentication",
                "Which authentication method would you like to use?",
                auth_labels,
                auth_default,
            ),
            // SQL Authentication fields
            ConfigField::input("user", "Username (SQL Server login)")
                .when_field_equals("authentication", FieldValue::Integer(sql_idx)),
            ConfigField::password("password", "Password")
                .when_field_equals("authentication", FieldValue::Integer(sql_idx)),
            // Active Directory Service Principal fields
            ConfigField::input("client_id", "Client ID (app registration)")
                .when_field_equals("authentication", FieldValue::Integer(sp_idx)),
            ConfigField::password("client_secret", "Client secret")
                .when_field_equals("authentication", FieldValue::Integer(sp_idx)),
            ConfigField::optional_input("tenant_id", "Tenant ID (optional)", None)
                .when_field_equals("authentication", FieldValue::Integer(sp_idx)),
            // Active Directory Password fields (`client_id` here is the Entra app used to
            // request the user's token, not the service-principal identity above — same
            // struct field, different auth branch).
            ConfigField::input(
                "client_id",
                "Client ID (Entra app registration used to request the user token)",
            )
            .when_field_equals("authentication", FieldValue::Integer(adpw_idx)),
            ConfigField::input("user", "Username (Entra user principal name)")
                .when_field_equals("authentication", FieldValue::Integer(adpw_idx)),
            ConfigField::password("password", "Password (Entra user password)")
                .when_field_equals("authentication", FieldValue::Integer(adpw_idx)),
            // Active Directory Access Token field
            ConfigField::password("access_token", "Access token")
                .when_field_equals("authentication", FieldValue::Integer(token_idx)),
            // `environment` needs no additional fields: credentials come from AZURE_* env vars.
        ]
    }

    fn set_field(&mut self, field_name: &str, value: FieldValue) -> FsResult<()> {
        match field_name {
            "host" => set_string(&mut self.host, value),
            "port" => set_port(&mut self.port, value),
            "database" => set_string(&mut self.database, value),
            "schema" => set_string(&mut self.schema, value),
            "authentication" => set_auth_method(&mut self.authentication, value),
            "user" => set_string(&mut self.user, value),
            "password" => set_string(&mut self.password, value),
            "client_id" => set_string(&mut self.client_id, value),
            "client_secret" => set_string(&mut self.client_secret, value),
            "tenant_id" => set_string(&mut self.tenant_id, value),
            "access_token" => set_string(&mut self.access_token, value),
            _ => {} // Ignore temporary or unrecognized fields
        }
        Ok(())
    }

    fn get_field(&self, field_name: &str) -> Option<FieldValue> {
        match field_name {
            "host" => self.host.as_ref().map(|s| FieldValue::String(s.clone())),
            "port" => self.port.as_ref().map(|v| match v {
                StringOrInteger::String(s) => FieldValue::String(s.clone()),
                StringOrInteger::Integer(i) => FieldValue::Integer(*i),
            }),
            "database" => self
                .database
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "schema" => self.schema.as_ref().map(|s| FieldValue::String(s.clone())),
            "authentication" => self
                .authentication
                .as_deref()
                .and_then(auth_index)
                .map(FieldValue::Integer),
            "user" => self.user.as_ref().map(|s| FieldValue::String(s.clone())),
            "password" => self
                .password
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
            "tenant_id" => self
                .tenant_id
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "access_token" => self
                .access_token
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            _ => None,
        }
    }

    fn is_field_set(&self, field_name: &str) -> bool {
        match field_name {
            "host" => self.host.is_some(),
            "port" => self.port.is_some(),
            "database" => self.database.is_some(),
            "schema" => self.schema.is_some(),
            "authentication" => self
                .authentication
                .as_deref()
                .map(auth_index)
                .is_some_and(|o| o.is_some()),
            "user" => self.user.is_some(),
            "password" => self.password.is_some(),
            "client_id" => self.client_id.is_some(),
            "client_secret" => self.client_secret.is_some(),
            "tenant_id" => self.tenant_id.is_some(),
            "access_token" => self.access_token.is_some(),
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

pub fn default_sqlserver_config() -> SqlServerDbConfig {
    SqlServerDbConfig {
        authentication: Some("sql".to_string()),
        encrypt: Some(true),
        trust_cert: Some(false),
        ..Default::default()
    }
}

pub fn setup_sqlserver_profile(
    existing_config: Option<&SqlServerDbConfig>,
) -> FsResult<Box<SqlServerDbConfig>> {
    let default_config = default_sqlserver_config();
    let config = ConfigProcessor::process_config(existing_config.or(Some(&default_config)))?;
    Ok(Box::new(config))
}

fn set_string(target: &mut Option<String>, value: FieldValue) {
    if let FieldValue::String(value) = value {
        *target = Some(value);
    }
}

fn set_port(target: &mut Option<StringOrInteger>, value: FieldValue) {
    let port = match value {
        FieldValue::Integer(port) => Some(port),
        FieldValue::String(port) => port.parse().ok(),
        _ => None,
    };
    if let Some(port) = port {
        *target = Some(StringOrInteger::Integer(port));
    }
}

fn set_auth_method(target: &mut Option<String>, value: FieldValue) {
    let FieldValue::Integer(index) = value else {
        return;
    };
    if let Some((method, _)) = AUTH_METHODS.get(index as usize) {
        *target = Some((*method).to_string());
    }
}
