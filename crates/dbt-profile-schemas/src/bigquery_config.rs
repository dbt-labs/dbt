use super::common::{ConfigField, ConfigProcessor, FieldValue, InteractiveSetup};
use dbt_common::FsResult;
use dbt_schemas::schemas::profiles::BigqueryDbConfig;
use dbt_schemas::schemas::serde::StringOrInteger;

impl InteractiveSetup for BigqueryDbConfig {
    fn get_fields() -> Vec<ConfigField> {
        vec![
            // Core connection settings
            ConfigField::input("project", "Project ID"),
            ConfigField::input("dataset", "Dataset"),
            ConfigField::optional_input(
                "location",
                "Location (e.g., us-east1, europe-west1)",
                None,
            ),
            // Authentication
            ConfigField::select(
                "auth_method",
                "Which authentication method would you like to use?",
                vec!["Service Account (JSON file)", "gcloud oauth"],
                0,
            ),
            ConfigField::input("keyfile", "Path to service account JSON file")
                .when_field_equals("auth_method", FieldValue::Integer(0)),
        ]
    }

    fn set_field(&mut self, field_name: &str, value: FieldValue) -> FsResult<()> {
        match field_name {
            "project" => {
                if let FieldValue::String(s) = value {
                    self.database = Some(s);
                }
            }
            "dataset" => {
                if let FieldValue::String(s) = value {
                    self.schema = Some(s);
                }
            }
            "location" => {
                if let FieldValue::String(s) = value
                    && !s.is_empty()
                {
                    self.location = Some(s);
                }
            }
            "keyfile" => {
                if let FieldValue::String(s) = value {
                    self.keyfile = Some(s);
                    self.method = Some("service-account".to_string());
                }
            }
            "auth_method" => {
                if let FieldValue::Integer(auth_method) = value {
                    match auth_method {
                        0 => {} // Service account - method will be set when keyfile is provided
                        1 => self.method = Some("oauth".to_string()), // gcloud oauth
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
            "project" => self
                .database
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "dataset" => self.schema.as_ref().map(|s| FieldValue::String(s.clone())),
            "location" => self
                .location
                .as_ref()
                .map(|s| FieldValue::String(s.clone())),
            "keyfile" => self.keyfile.as_ref().map(|s| FieldValue::String(s.clone())),
            "auth_method" => {
                if self.keyfile.is_some() {
                    Some(FieldValue::Integer(0))
                } else if self.method.as_ref().is_some_and(|m| m == "oauth") {
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
            "project" => self.database.is_some(),
            "dataset" => self.schema.is_some(),
            "location" => self.location.is_some(),
            "keyfile" => self.keyfile.is_some(),
            _ => false,
        }
    }
}

pub fn setup_bigquery_profile(
    existing_config: Option<&BigqueryDbConfig>,
) -> FsResult<Box<BigqueryDbConfig>> {
    let default_config = BigqueryDbConfig {
        threads: None,
        profile_type: None,
        database: None,
        schema: None,
        timeout_seconds: None,
        priority: None,
        method: None,
        maximum_bytes_billed: None,
        impersonate_service_account: None,
        refresh_token: None,
        client_id: None,
        client_secret: None,
        token_uri: None,
        token: None,
        keyfile: None,
        quota_project: None,
        retries: None,
        location: None,
        scopes: None,
        keyfile_json: None,
        execution_project: None,
        api_endpoint: None,
        compute_region: None,
        dataproc_batch: None,
        dataproc_cluster_name: None,
        gcs_bucket: None,
        submission_method: None,
        job_creation_timeout_seconds: None,
        job_execution_timeout_seconds: None,
        reservation: None,
        job_retries: None,
        job_retry_deadline_seconds: None,
        target_name: None,
        workload_pool_provider_path: None,
        service_account_impersonation_url: None,
        token_endpoint: None,
    };
    let mut config = ConfigProcessor::process_config(existing_config.or(Some(&default_config)))?;

    if config.threads.is_none() {
        config.threads = Some(StringOrInteger::Integer(16));
    }

    Ok(Box::new(config))
}
