use super::common::{ConfigField, ConfigProcessor, FieldValue, InteractiveSetup};
use dbt_common::FsResult;
use dbt_schemas::schemas::profiles::RedshiftDbConfig;
use dbt_schemas::schemas::serde::StringOrInteger;

impl InteractiveSetup for RedshiftDbConfig {
    fn get_fields() -> Vec<ConfigField> {
        vec![
            // Core connection settings
            ConfigField::input("host", "Host"),
            ConfigField::input("user", "User"),
            ConfigField::input("database", "Database"),
            ConfigField::input("schema", "Schema"),
            ConfigField::optional_input("port", "Port (default: 5439)", Some("5439")),
            // Authentication
            ConfigField::select(
                "auth_method",
                "Which authentication method would you like to use?",
                vec!["Password", "IAM Profile"],
                0,
            ),
            ConfigField::password("password", "Password")
                .when_field_equals("auth_method", FieldValue::Integer(0)),
            ConfigField::input("iam_profile", "IAM Profile")
                .when_field_equals("auth_method", FieldValue::Integer(1)),
            ConfigField::input("cluster_id", "Cluster ID")
                .when_field_equals("auth_method", FieldValue::Integer(1)),
            ConfigField::input("region", "Region (e.g., us-east-1)")
                .when_field_equals("auth_method", FieldValue::Integer(1)),
            ConfigField::confirm("ra3_node", "Use RA3 node type?", false).optional(),
            ConfigField::confirm(
                "datasharing",
                "Enable datasharing for cross-database operations?",
                false,
            )
            .optional(),
        ]
    }

    fn set_field(&mut self, field_name: &str, value: FieldValue) -> FsResult<()> {
        match field_name {
            "host" => {
                if let FieldValue::String(s) = value {
                    self.host = Some(s);
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
            "schema" => {
                if let FieldValue::String(s) = value {
                    self.schema = Some(s);
                }
            }
            "port" => {
                if let FieldValue::String(s) = value
                    && !s.is_empty()
                {
                    if let Ok(n) = s.parse::<i64>() {
                        self.port = Some(StringOrInteger::Integer(n));
                    } else {
                        self.port = Some(StringOrInteger::String(s));
                    }
                }
            }
            "password" => {
                if let FieldValue::String(s) = value {
                    self.password = Some(s);
                    self.method = Some("database".to_string());
                }
            }
            "iam_profile" => {
                if let FieldValue::String(s) = value {
                    self.iam_profile = Some(s);
                    self.method = Some("iam".to_string());
                }
            }
            "cluster_id" => {
                if let FieldValue::String(s) = value {
                    self.cluster_id = Some(s);
                }
            }
            "region" => {
                if let FieldValue::String(s) = value {
                    self.region = Some(s);
                }
            }
            "ra3_node" => {
                if let FieldValue::Boolean(b) = value {
                    self.ra3_node = Some(b);
                }
            }
            "datasharing" => {
                if let FieldValue::Boolean(b) = value {
                    self.datasharing = Some(b);
                }
            }
            "auth_method" => {
                if let FieldValue::Integer(auth_method) = value {
                    match auth_method {
                        0 => {} // Password - method will be set when password is provided
                        1 => {} // IAM Profile - method will be set when iam_profile is provided
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
            "user" => self.user.as_ref().map(|s| FieldValue::String(s.clone())),
            "database" => self
                .database
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "schema" => self.schema.as_ref().map(|s| FieldValue::String(s.clone())),
            "port" => self.port.as_ref().map(|p| match p {
                StringOrInteger::String(s) => FieldValue::String(s.clone()),
                StringOrInteger::Integer(i) => FieldValue::String(i.to_string()),
            }),
            "password" => self
                .password
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "iam_profile" => self
                .iam_profile
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "cluster_id" => self
                .cluster_id
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "region" => self.region.as_ref().map(|s| FieldValue::String(s.clone())),
            "ra3_node" => self.ra3_node.map(FieldValue::Boolean),
            "datasharing" => self.datasharing.map(FieldValue::Boolean),
            "auth_method" => {
                if self.password.is_some() {
                    Some(FieldValue::Integer(0))
                } else if self.iam_profile.is_some() {
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
            "user" => self.user.is_some(),
            "database" => self.database.is_some(),
            "schema" => self.schema.is_some(),
            "port" => self.port.is_some(),
            "password" => self.password.is_some(),
            "iam_profile" => self.iam_profile.is_some(),
            "cluster_id" => self.cluster_id.is_some(),
            "region" => self.region.is_some(),
            "ra3_node" => self.ra3_node.is_some(),
            "datasharing" => self.datasharing.is_some(),
            _ => false,
        }
    }
}

pub fn setup_redshift_profile(
    existing_config: Option<&RedshiftDbConfig>,
) -> FsResult<Box<RedshiftDbConfig>> {
    let default_config = RedshiftDbConfig {
        port: None,
        database: None,
        schema: None,
        connect_timeout: None,
        sslmode: None,
        role: None,
        autocreate: None,
        db_groups: None,
        ra3_node: None,
        datasharing: None,
        drop_without_cascade: None,
        autocommit: None,
        retries: None,
        method: None,
        host: None,
        user: None,
        password: None,
        iam_profile: None,
        access_key_id: None,
        secret_access_key: None,
        cluster_id: None,
        region: None,
        is_serverless: None,
        serverless_work_group: None,
        serverless_acct_id: None,
        threads: None,
        token_endpoint: None,
        idc_client_display_name: None,
        idc_region: None,
        idp_response_timeout: None,
        issuer_url: None,
        idp_listen_port: None,
    };
    let mut config = ConfigProcessor::process_config(existing_config.or(Some(&default_config)))?;

    if config.threads.is_none() {
        config.threads = Some(StringOrInteger::Integer(16));
    }

    Ok(Box::new(config))
}
