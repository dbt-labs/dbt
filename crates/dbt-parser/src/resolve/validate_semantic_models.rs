use dbt_common::{ErrorCode, FsError, FsResult, fs_err};
use dbt_schemas::schemas::properties::ModelProperties;
use std::collections::HashSet;

/// Validates time spine configuration for semantic models according to the rules ported from Python dbt.
/// This checks:
/// - Standard granularity column exists in model columns
/// - Standard granularity column has a granularity defined
/// - Custom granularity columns exist in model columns
///
/// # Arguments
/// * `model_props` - The model properties to validate
///
/// # Returns
/// * `Ok(vec![])` if the time spine is valid (empty error list)
/// * `Ok(vec![errors])` if there are validation failures
pub fn validate_time_spine(model_props: &ModelProperties) -> FsResult<Vec<FsError>> {
    // Only validate if time_spine is present
    let time_spine = match &model_props.time_spine {
        Some(ts) => ts,
        None => return Ok(vec![]), // No time spine to validate
    };

    // Get columns for validation - use base columns for now
    // TODO: In Python, this calls get_columns_for_version when latest_version is set,
    // but for now we'll use base columns since we don't have version-specific column filtering implemented yet
    let columns = model_props.columns.clone().unwrap_or_default();

    // Create a mapping of column names to columns for easy lookup
    let column_names_to_columns: std::collections::HashMap<String, _> =
        columns.iter().map(|col| (col.name.clone(), col)).collect();

    let mut errors = Vec::new();

    // Validate standard granularity column exists
    let standard_column = match column_names_to_columns.get(&time_spine.standard_granularity_column)
    {
        Some(col) => Some(col),
        None => {
            let version_msg = if model_props.latest_version.is_some() {
                " for latest version"
            } else {
                ""
            };
            errors.push(*fs_err!(
                ErrorCode::DbtYamlValidationError,
                "Time spine standard granularity column must be defined on the model. Got invalid \
                column name '{}' for model '{}'. Valid names{}: {:?}.",
                time_spine.standard_granularity_column,
                model_props.name,
                version_msg,
                column_names_to_columns.keys().collect::<Vec<_>>()
            ));
            None
        }
    };

    // Validate standard granularity column has a granularity defined (only if column exists)
    if let Some(standard_col) = standard_column
        && standard_col.granularity.is_none()
    {
        errors.push(*fs_err!(
            ErrorCode::DbtYamlValidationError,
            "Time spine standard granularity column must have a granularity defined. Please add one for \
            column '{}' in model '{}'.",
            time_spine.standard_granularity_column,
            model_props.name
        ));
    }

    // Validate custom granularity columns exist in the model
    if let Some(custom_granularities) = &time_spine.custom_granularities {
        let mut custom_granularity_columns_not_found = Vec::new();

        for custom_granularity in custom_granularities {
            // Use column_name if provided, otherwise use name
            let column_name = custom_granularity
                .column_name
                .as_ref()
                .unwrap_or(&custom_granularity.name);

            if !column_names_to_columns.contains_key(column_name) {
                custom_granularity_columns_not_found.push(column_name.clone());
            }
        }

        if !custom_granularity_columns_not_found.is_empty() {
            errors.push(*fs_err!(
                ErrorCode::DbtYamlValidationError,
                "Time spine custom granularity columns do not exist in the model '{}'. \
                Columns not found: {:?}; Available columns: {:?}",
                model_props.name,
                custom_granularity_columns_not_found,
                column_names_to_columns.keys().collect::<Vec<_>>()
            ));
        }
    }

    Ok(errors)
}

/// Validates semantic model properties according to the rules ported from Python dbt.
/// This is a unified validation function that checks:
/// - Latest version is in versions list
/// - No duplicate versions
/// - Time spine validation (if present)
///
/// # Arguments
/// * `model_props` - The model properties to validate
///
/// # Returns
/// * `Ok(vec![])` if the semantic model is valid (empty error list)
/// * `Ok(vec![errors])` if there are validation failures
pub fn validate_semantic_model(model_props: &ModelProperties) -> FsResult<Vec<FsError>> {
    let mut errors = Vec::new();

    // Validate latest_version is in versions list
    if let Some(latest_version) = &model_props.latest_version
        && let Some(versions) = &model_props.versions
    {
        let version_values: Vec<String> = versions.iter().filter_map(|v| v.get_version()).collect();

        let latest_version_str = match latest_version {
            dbt_schemas::schemas::serde::FloatOrString::Number(f) => f.to_string(),
            dbt_schemas::schemas::serde::FloatOrString::String(s) => s.clone(),
        };

        if !version_values.contains(&latest_version_str) {
            errors.push(*fs_err!(
                ErrorCode::DbtYamlValidationError,
                "latest_version: {} is not one of model '{}' versions: {:?}",
                latest_version_str,
                model_props.name,
                version_values
            ));
        }
    }

    // Validate no duplicate versions
    if let Some(versions) = &model_props.versions {
        let mut seen_versions = HashSet::new();
        for version in versions {
            if let Some(version_str) = version.get_version()
                && !seen_versions.insert(version_str.clone())
            {
                errors.push(*fs_err!(
                    ErrorCode::DbtYamlValidationError,
                    "Found duplicate version: '{}' in versions list of model '{}'",
                    version_str,
                    model_props.name
                ));
            }
        }
    }

    // Validate time spine if present - collect any errors from time spine validation
    let time_spine_errors = validate_time_spine(model_props)?;
    errors.extend(time_spine_errors);

    Ok(errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::common::Versions;
    use dbt_schemas::schemas::dbt_column::{ColumnProperties, Granularity};
    use dbt_schemas::schemas::properties::model_properties::{
        ModelPropertiesTimeSpine, TimeSpineCustomGranularity,
    };
    use dbt_schemas::schemas::serde::FloatOrString;

    fn create_test_model_properties(name: &str) -> ModelProperties {
        ModelProperties {
            name: name.to_string(),
            ..Default::default()
        }
    }

    fn create_test_column(name: &str, granularity: Option<Granularity>) -> ColumnProperties {
        ColumnProperties {
            name: name.to_string(),
            granularity,
            ..Default::default()
        }
    }

    fn create_test_version(version: &str) -> Versions {
        Versions {
            v: dbt_yaml::Value::String(version.to_string(), Default::default()),
            deprecation_date: None,
            defined_in: None,
            description: None,
            access: None,
            config: dbt_yaml::Verbatim::from(None),
            constraints: None,
            data_tests: None,
            tests: None,
            __additional_properties__: dbt_yaml::Verbatim::from(
                std::collections::HashMap::new(),
            ),
        }
    }

    #[test]
    fn test_validate_time_spine_no_time_spine() {
        let model_props = create_test_model_properties("test_model");
        let result = validate_time_spine(&model_props).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_validate_time_spine_valid() {
        let mut model_props = create_test_model_properties("test_model");

        // Add columns with granularity
        model_props.columns = Some(vec![
            create_test_column("date_col", Some(Granularity::day)),
            create_test_column("other_col", None),
        ]);

        // Add time spine
        model_props.time_spine = Some(ModelPropertiesTimeSpine {
            standard_granularity_column: "date_col".to_string(),
            custom_granularities: None,
        });

        let result = validate_time_spine(&model_props).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_validate_time_spine_standard_column_not_found() {
        let mut model_props = create_test_model_properties("test_model");

        // Add columns
        model_props.columns = Some(vec![create_test_column("other_col", None)]);

        // Add time spine with non-existent column
        model_props.time_spine = Some(ModelPropertiesTimeSpine {
            standard_granularity_column: "missing_col".to_string(),
            custom_granularities: None,
        });

        let errors = validate_time_spine(&model_props).unwrap();
        assert_eq!(errors.len(), 1);
        let error = &errors[0];
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(
            error
                .context
                .contains("Time spine standard granularity column must be defined on the model")
        );
        assert!(error.context.contains("missing_col"));
    }

    #[test]
    fn test_validate_time_spine_standard_column_no_granularity() {
        let mut model_props = create_test_model_properties("test_model");

        // Add columns without granularity
        model_props.columns = Some(vec![create_test_column("date_col", None)]);

        // Add time spine
        model_props.time_spine = Some(ModelPropertiesTimeSpine {
            standard_granularity_column: "date_col".to_string(),
            custom_granularities: None,
        });

        let errors = validate_time_spine(&model_props).unwrap();
        assert_eq!(errors.len(), 1);
        let error = &errors[0];
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(
            error
                .context
                .contains("Time spine standard granularity column must have a granularity defined")
        );
        assert!(error.context.contains("date_col"));
    }

    #[test]
    fn test_validate_time_spine_custom_granularity_columns_not_found() {
        let mut model_props = create_test_model_properties("test_model");

        // Add columns
        model_props.columns = Some(vec![create_test_column("date_col", Some(Granularity::day))]);

        // Add time spine with custom granularities referencing non-existent columns
        model_props.time_spine = Some(ModelPropertiesTimeSpine {
            standard_granularity_column: "date_col".to_string(),
            custom_granularities: Some(vec![
                TimeSpineCustomGranularity {
                    name: "custom1".to_string(),
                    column_name: Some("missing_col1".to_string()),
                },
                TimeSpineCustomGranularity {
                    name: "missing_col2".to_string(),
                    column_name: None,
                },
            ]),
        });

        let errors = validate_time_spine(&model_props).unwrap();
        assert_eq!(errors.len(), 1);
        let error = &errors[0];
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(
            error
                .context
                .contains("Time spine custom granularity columns do not exist in the model")
        );
        assert!(error.context.contains("missing_col1"));
        assert!(error.context.contains("missing_col2"));
    }

    #[test]
    fn test_validate_semantic_model_valid_versions() {
        let mut model_props = create_test_model_properties("test_model");

        model_props.latest_version = Some(FloatOrString::String("1.0".to_string()));
        model_props.versions = Some(vec![create_test_version("1.0"), create_test_version("2.0")]);

        let errors = validate_semantic_model(&model_props).unwrap();
        assert!(errors.is_empty());
    }

    #[test]
    fn test_validate_semantic_model_invalid_latest_version() {
        let mut model_props = create_test_model_properties("test_model");

        model_props.latest_version = Some(FloatOrString::String("3.0".to_string()));
        model_props.versions = Some(vec![create_test_version("1.0"), create_test_version("2.0")]);

        let errors = validate_semantic_model(&model_props).unwrap();
        assert_eq!(errors.len(), 1);
        let error = &errors[0];
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(
            error
                .context
                .contains("latest_version: 3.0 is not one of model")
        );
    }

    #[test]
    fn test_validate_semantic_model_duplicate_versions() {
        let mut model_props = create_test_model_properties("test_model");

        model_props.versions = Some(vec![
            create_test_version("1.0"),
            create_test_version("1.0"), // duplicate
            create_test_version("2.0"),
        ]);

        let errors = validate_semantic_model(&model_props).unwrap();
        assert_eq!(errors.len(), 1);
        let error = &errors[0];
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(error.context.contains("Found duplicate version: '1.0'"));
    }

    #[test]
    fn test_validate_semantic_model_multiple_errors() {
        let mut model_props = create_test_model_properties("test_model");

        // Set up multiple validation failures:
        // 1. Invalid latest_version
        model_props.latest_version = Some(FloatOrString::String("3.0".to_string()));
        model_props.versions = Some(vec![
            create_test_version("1.0"),
            create_test_version("1.0"), // 2. Duplicate version
            create_test_version("2.0"),
        ]);

        // 3. Invalid time spine setup
        model_props.columns = Some(vec![create_test_column("other_col", None)]);
        model_props.time_spine = Some(ModelPropertiesTimeSpine {
            standard_granularity_column: "missing_col".to_string(),
            custom_granularities: Some(vec![TimeSpineCustomGranularity {
                name: "missing_custom".to_string(),
                column_name: None,
            }]),
        });

        let errors = validate_semantic_model(&model_props).unwrap();
        assert_eq!(errors.len(), 4); // Should have 4 separate errors (latest_version, duplicate_version, missing_standard_column, missing_custom_column)

        // Check for each type of error
        let error_messages: Vec<String> = errors.iter().map(|e| e.context.clone()).collect();
        let combined_errors = error_messages.join(" ");

        assert!(combined_errors.contains("latest_version: 3.0 is not one of model"));
        assert!(combined_errors.contains("Found duplicate version: '1.0'"));
        assert!(combined_errors.contains("Time spine standard granularity column must be defined"));
    }

    #[test]
    fn test_validate_time_spine_multiple_errors() {
        let mut model_props = create_test_model_properties("test_model");

        // Set up multiple time spine validation failures:
        // 1. Missing standard granularity column
        // 2. Missing custom granularity columns
        model_props.columns = Some(vec![create_test_column("other_col", None)]);
        model_props.time_spine = Some(ModelPropertiesTimeSpine {
            standard_granularity_column: "missing_standard".to_string(),
            custom_granularities: Some(vec![
                TimeSpineCustomGranularity {
                    name: "missing_custom1".to_string(),
                    column_name: None,
                },
                TimeSpineCustomGranularity {
                    name: "custom2".to_string(),
                    column_name: Some("missing_custom2".to_string()),
                },
            ]),
        });

        let errors = validate_time_spine(&model_props).unwrap();
        assert_eq!(errors.len(), 2); // Should have 2 separate errors

        // Check for each type of error
        let error_messages: Vec<String> = errors.iter().map(|e| e.context.clone()).collect();
        let combined_errors = error_messages.join(" ");

        assert!(combined_errors.contains("Time spine standard granularity column must be defined"));
        assert!(combined_errors.contains("Time spine custom granularity columns do not exist"));
        assert!(combined_errors.contains("missing_custom1"));
        assert!(combined_errors.contains("missing_custom2"));
    }
}
