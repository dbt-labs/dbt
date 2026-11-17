use super::common::{ConfigField, ConfigProcessor, FieldValue, InteractiveSetup};
use dbt_common::FsResult;
use dbt_schemas::schemas::profiles::AthenaDbConfig;
use dbt_schemas::schemas::serde::StringOrInteger;

// AthenaAuth precedence: explicit keys, then a named profile, then the default chain.
const AUTH_DEFAULT_CHAIN: i64 = 0;
const AUTH_PROFILE: i64 = 1;
const AUTH_ACCESS_KEY: i64 = 2;

// Prompts follow dbt-athena's profile_template.yml, plus an auth selector it lacks.
impl InteractiveSetup for AthenaDbConfig {
    fn get_fields() -> Vec<ConfigField> {
        vec![
            ConfigField::input("region_name", "AWS region of your Athena instance"),
            ConfigField::input(
                "s3_staging_dir",
                "S3 location for Athena query results (e.g. s3://bucket/prefix/)",
            ),
            ConfigField::optional_input(
                "s3_data_dir",
                "S3 location for table data (blank: under the staging dir)",
                None,
            ),
            ConfigField::optional_input("database", "Data catalog", Some("awsdatacatalog")),
            ConfigField::input("schema", "Schema (Athena database, lowercase)"),
            ConfigField::optional_input(
                "work_group",
                "Athena workgroup (blank: account default)",
                None,
            ),
            ConfigField::select(
                "auth_method",
                "Which authentication method would you like to use?",
                vec![
                    "AWS default credential chain (env vars, instance/role, SSO)",
                    "Named AWS profile",
                    "Access key pair",
                ],
                AUTH_DEFAULT_CHAIN as usize,
            ),
            ConfigField::input("aws_profile_name", "AWS profile name")
                .when_field_equals("auth_method", FieldValue::Integer(AUTH_PROFILE)),
            ConfigField::input("aws_access_key_id", "AWS access key ID")
                .when_field_equals("auth_method", FieldValue::Integer(AUTH_ACCESS_KEY)),
            ConfigField::password("aws_secret_access_key", "AWS secret access key")
                .when_field_equals("auth_method", FieldValue::Integer(AUTH_ACCESS_KEY)),
            ConfigField::password("aws_session_token", "AWS session token (blank if none)")
                .optional()
                .when_field_equals("auth_method", FieldValue::Integer(AUTH_ACCESS_KEY)),
        ]
    }

    fn set_field(&mut self, field_name: &str, value: FieldValue) -> FsResult<()> {
        match field_name {
            "region_name" => assign_string(&mut self.region_name, value, false),
            "s3_staging_dir" => assign_string(&mut self.s3_staging_dir, value, false),
            "s3_data_dir" => assign_string(&mut self.s3_data_dir, value, true),
            "database" => assign_string(&mut self.database, value, true),
            "schema" => assign_string(&mut self.schema, value, false),
            "work_group" => assign_string(&mut self.work_group, value, true),
            "auth_method" => apply_auth_method(self, value),
            "aws_profile_name" => assign_string(&mut self.aws_profile_name, value, false),
            "aws_access_key_id" => assign_string(&mut self.aws_access_key_id, value, false),
            "aws_secret_access_key" => assign_string(&mut self.aws_secret_access_key, value, false),
            "aws_session_token" => assign_string(&mut self.aws_session_token, value, true),
            _ => {}
        }
        Ok(())
    }

    fn get_field(&self, field_name: &str) -> Option<FieldValue> {
        let string = |v: &Option<String>| v.as_ref().map(|s| FieldValue::String(s.clone()));
        match field_name {
            "region_name" => string(&self.region_name),
            "s3_staging_dir" => string(&self.s3_staging_dir),
            "s3_data_dir" => string(&self.s3_data_dir),
            "database" => string(&self.database),
            "schema" => string(&self.schema),
            "work_group" => string(&self.work_group),
            "auth_method" => {
                if self.aws_access_key_id.is_some() {
                    Some(FieldValue::Integer(AUTH_ACCESS_KEY))
                } else if self.aws_profile_name.is_some() {
                    Some(FieldValue::Integer(AUTH_PROFILE))
                } else {
                    None
                }
            }
            "aws_profile_name" => string(&self.aws_profile_name),
            "aws_access_key_id" => string(&self.aws_access_key_id),
            "aws_secret_access_key" => string(&self.aws_secret_access_key),
            "aws_session_token" => string(&self.aws_session_token),
            _ => None,
        }
    }

    fn is_field_set(&self, field_name: &str) -> bool {
        match field_name {
            "auth_method" => self.aws_access_key_id.is_some() || self.aws_profile_name.is_some(),
            _ => self.get_field(field_name).is_some(),
        }
    }
}

fn assign_string(slot: &mut Option<String>, value: FieldValue, drop_blank: bool) {
    let FieldValue::String(s) = value else {
        return;
    };
    *slot = if drop_blank && s.is_empty() {
        None
    } else {
        Some(s)
    };
}

/// Clear the other methods; leftover keys override a chosen profile.
fn apply_auth_method(config: &mut AthenaDbConfig, value: FieldValue) {
    let FieldValue::Integer(method) = value else {
        return;
    };
    if method != AUTH_PROFILE {
        config.aws_profile_name = None;
    }
    if method != AUTH_ACCESS_KEY {
        config.aws_access_key_id = None;
        config.aws_secret_access_key = None;
        config.aws_session_token = None;
    }
}

pub fn setup_athena_profile(
    existing_config: Option<&AthenaDbConfig>,
) -> FsResult<Box<AthenaDbConfig>> {
    let default_config = AthenaDbConfig::default();
    let mut config = ConfigProcessor::process_config(existing_config.or(Some(&default_config)))?;

    if config.threads.is_none() {
        config.threads = Some(StringOrInteger::Integer(default_threads()));
    }

    Ok(Box::new(config))
}

/// dbt-athena's `profile_template.yml` defaults `threads` to 1.
pub(crate) fn default_threads() -> i64 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::apply_values;
    use std::collections::HashMap;

    fn values(pairs: &[(&str, FieldValue)]) -> HashMap<String, FieldValue> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn s(v: &str) -> FieldValue {
        FieldValue::String(v.to_string())
    }

    #[test]
    fn default_chain_sets_no_credentials() {
        let config = apply_values(
            &AthenaDbConfig::default(),
            &values(&[
                ("region_name", s("us-east-2")),
                ("s3_staging_dir", s("s3://bucket/results/")),
                ("s3_data_dir", s("")),
                ("database", s("awsdatacatalog")),
                ("schema", s("analytics")),
                ("work_group", s("")),
                ("auth_method", FieldValue::Integer(AUTH_DEFAULT_CHAIN)),
                // Would only apply when the matching method is selected.
                ("aws_profile_name", s("ignored")),
                ("aws_access_key_id", s("ignored")),
            ]),
        )
        .unwrap();

        assert_eq!(config.region_name.as_deref(), Some("us-east-2"));
        assert_eq!(
            config.s3_staging_dir.as_deref(),
            Some("s3://bucket/results/")
        );
        assert_eq!(config.s3_data_dir, None);
        assert_eq!(config.database.as_deref(), Some("awsdatacatalog"));
        assert_eq!(config.schema.as_deref(), Some("analytics"));
        assert_eq!(config.work_group, None);
        assert_eq!(config.aws_profile_name, None);
        assert_eq!(config.aws_access_key_id, None);
        assert_eq!(config.aws_secret_access_key, None);
    }

    #[test]
    fn profile_method_shows_only_the_profile_prompt() {
        let config = apply_values(
            &AthenaDbConfig::default(),
            &values(&[
                ("auth_method", FieldValue::Integer(AUTH_PROFILE)),
                ("aws_profile_name", s("analytics-sso")),
                ("aws_access_key_id", s("ignored")),
            ]),
        )
        .unwrap();

        assert_eq!(config.aws_profile_name.as_deref(), Some("analytics-sso"));
        assert_eq!(config.aws_access_key_id, None);
    }

    #[test]
    fn access_key_method_collects_the_pair_and_optional_token() {
        let config = apply_values(
            &AthenaDbConfig::default(),
            &values(&[
                ("auth_method", FieldValue::Integer(AUTH_ACCESS_KEY)),
                ("aws_access_key_id", s("AKIAEXAMPLE")),
                ("aws_secret_access_key", s("secret")),
                ("aws_session_token", s("")),
                ("aws_profile_name", s("ignored")),
            ]),
        )
        .unwrap();

        assert_eq!(config.aws_access_key_id.as_deref(), Some("AKIAEXAMPLE"));
        assert_eq!(config.aws_secret_access_key.as_deref(), Some("secret"));
        assert_eq!(config.aws_session_token, None);
        assert_eq!(config.aws_profile_name, None);
    }

    #[test]
    fn switching_method_clears_the_previous_credentials() {
        let existing = AthenaDbConfig {
            aws_access_key_id: Some("AKIAEXAMPLE".to_string()),
            aws_secret_access_key: Some("secret".to_string()),
            ..AthenaDbConfig::default()
        };
        assert_eq!(
            existing.get_field("auth_method"),
            Some(FieldValue::Integer(AUTH_ACCESS_KEY))
        );

        let config = apply_values(
            &existing,
            &values(&[
                ("auth_method", FieldValue::Integer(AUTH_PROFILE)),
                ("aws_profile_name", s("analytics-sso")),
            ]),
        )
        .unwrap();

        assert_eq!(config.aws_profile_name.as_deref(), Some("analytics-sso"));
        assert_eq!(config.aws_access_key_id, None);
        assert_eq!(config.aws_secret_access_key, None);
    }
}
