use dbt_common::{AdapterError, AdapterErrorKind, AdapterResult};
use minijinja::{State, Value};

const DEFAULT_SNOWFLAKE_PYTHON_PACKAGE: &str = "snowflake-snowpark-python";

pub fn finalize_python_code(
    state: &State,
    model: &Value,
    compiled_code: &str,
) -> AdapterResult<String> {
    // Extract config from model
    let config = model
        .get_attr("config")
        .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, e.to_string()))?;

    let database = model
        .get_attr("database")
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .expect("database is required");
    let schema = model
        .get_attr("schema")
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .expect("schema is required");
    let alias = model
        .get_attr("alias")
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_default();

    // Get python_version from config, default to "3.11"
    let python_version = config
        .get_attr("python_version")
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| "3.11".to_string());

    // Get packages list
    let packages = config
        .get_attr("packages")
        .ok()
        .and_then(|v| v.try_iter().ok())
        .map(|iter| {
            iter.filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // Add default packages
    let mut all_packages = packages;
    if !all_packages
        .iter()
        .any(|p| p.starts_with(DEFAULT_SNOWFLAKE_PYTHON_PACKAGE))
    {
        all_packages.push(DEFAULT_SNOWFLAKE_PYTHON_PACKAGE.to_string());
    }
    let packages_str = all_packages
        .iter()
        .map(|p| format!("'{}'", p))
        .collect::<Vec<_>>()
        .join(", ");

    // Get imports, external_access_integrations, secrets
    let imports = config
        .get_attr("imports")
        .ok()
        .and_then(|v| v.try_iter().ok())
        .map(|iter| {
            iter.filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
                .join("', '")
        })
        .unwrap_or_default();

    let external_access_integrations = config
        .get_attr("external_access_integrations")
        .ok()
        .and_then(|v| v.try_iter().ok())
        .map(|iter| {
            iter.filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    let secrets = config
        .get_attr("secrets")
        .ok()
        .and_then(|secrets_dict| {
            // Iterate over dict keys, then get values
            if let Ok(keys_iter) = secrets_dict.try_iter() {
                let pairs: Vec<String> = keys_iter
                    .filter_map(|key| {
                        // Get the key as string
                        let key_str = key.as_str()?.to_string();
                        // Get the value for this key from the dict
                        let value = secrets_dict.get_item(&key).ok()?;
                        let val_str = value.as_str()?.to_string();
                        Some(format!("'{}' = {}", key_str, val_str))
                    })
                    .collect();
                if pairs.is_empty() {
                    None
                } else {
                    Some(pairs.join(", "))
                }
            } else {
                None
            }
        })
        .unwrap_or_default();

    // Build optional clauses
    let imports_clause = if !imports.is_empty() {
        format!("IMPORTS = ('{}')", imports)
    } else {
        String::new()
    };

    let external_access_clause = if !external_access_integrations.is_empty() {
        format!(
            "EXTERNAL_ACCESS_INTEGRATIONS = ({})",
            external_access_integrations
        )
    } else {
        String::new()
    };

    let secrets_clause = if !secrets.is_empty() {
        format!("SECRETS = ({})", secrets)
    } else {
        String::new()
    };

    let send_anonymous_usage_stats = state
        .lookup("flags", &[])
        .and_then(|flags| flags.get_attr("SEND_ANONYMOUS_USAGE_STATS").ok())
        .filter(|v| !v.is_undefined()) // Filter out undefined values - they should use the default
        .map(|value| value.is_true())
        .unwrap_or(true);

    let telemetry_snippet = if send_anonymous_usage_stats {
        "import sys\nsys._xoptions['snowflake_partner_attribution'].append(\"dbtLabs_dbtPython\")\n\n"
    } else {
        ""
    };

    // Build the common procedure code
    let common_procedure_code = format!(
        r#"
RETURNS STRING
LANGUAGE PYTHON
RUNTIME_VERSION = '{}'
PACKAGES = ({})
{}
{}
{}
HANDLER = 'main'
EXECUTE AS CALLER
AS
$$
{}
{}
$$"#,
        python_version,
        packages_str,
        external_access_clause,
        secrets_clause,
        imports_clause,
        telemetry_snippet,
        compiled_code
    );

    // Use anonymous sproc by default (matching dbt-snowflake behavior)
    let use_anonymous_sproc = config
        .get_attr("use_anonymous_sproc")
        .ok()
        .filter(|v| !v.is_undefined()) // Filter out undefined values - they should use the default
        .map(|v| v.is_true())
        .unwrap_or(true); // Default to true when not found or undefined

    let python_stored_procedure = if use_anonymous_sproc {
        let proc_name = format!("{}__dbt_sp", alias);
        format!(
            r#"
WITH {} AS PROCEDURE ()
{}
CALL {}();
"#,
            proc_name, common_procedure_code, proc_name
        )
    } else {
        let proc_name = format!("{}.{}.{}__dbt_sp", database, schema, alias);
        format!(
            r#"
CREATE OR REPLACE PROCEDURE {} ()
{};
CALL {}();
DROP PROCEDURE IF EXISTS {}();
"#,
            proc_name, common_procedure_code, proc_name, proc_name
        )
    };

    Ok(python_stored_procedure)
}
