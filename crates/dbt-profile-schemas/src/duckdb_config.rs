use super::common::*;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::profiles::DuckDbConfig;
use dbt_schemas::schemas::serde::StringOrInteger;

impl InteractiveSetup for DuckDbConfig {
    fn get_fields() -> Vec<ConfigField> {
        vec![
            ConfigField {
                name: "path".to_string(),
                field_type: FieldType::Input {
                    default: Some("dev.duckdb".to_string()),
                },
                condition: FieldCondition::Always,
                prompt: "Path (DuckDB database file)".to_string(),
                required: true,
            },
            ConfigField {
                name: "schema".to_string(),
                field_type: FieldType::Input {
                    default: Some("main".to_string()),
                },
                condition: FieldCondition::Always,
                prompt: "Schema (dbt schema)".to_string(),
                required: true,
            },
        ]
    }

    fn set_field(&mut self, field_name: &str, value: FieldValue) -> FsResult<()> {
        match field_name {
            "path" => {
                if let FieldValue::String(val) = value {
                    self.path = Some(val);
                }
            }
            "schema" => {
                if let FieldValue::String(val) = value {
                    self.schema = Some(val);
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
            "path" => self.path.as_ref().map(|v| FieldValue::String(v.clone())),
            "schema" => self.schema.as_ref().map(|v| FieldValue::String(v.clone())),
            _ => None,
        }
    }

    fn is_field_set(&self, field_name: &str) -> bool {
        match field_name {
            "path" => self.path.is_some(),
            "schema" => self.schema.is_some(),
            _ => false,
        }
    }
}

/// Threads default for DuckDB profiles. Matches the value in the dbt DuckDB setup docs.
/// DuckDB itself allows concurrent writers within one process, so this is not a limit.
pub(crate) const DUCKDB_DEFAULT_THREADS: i64 = 4;

pub(crate) fn setup_duckdb_profile(
    existing_config: Option<&DuckDbConfig>,
) -> FsResult<Box<DuckDbConfig>> {
    let default_config = DuckDbConfig::default();
    let mut config = ConfigProcessor::process_config(existing_config.or(Some(&default_config)))?;

    if config.threads.is_none() {
        config.threads = Some(StringOrInteger::Integer(DUCKDB_DEFAULT_THREADS));
    }

    Ok(Box::new(config))
}
