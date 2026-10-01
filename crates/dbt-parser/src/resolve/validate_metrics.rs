use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::manifest::metric::MetricTimeWindow;
use dbt_schemas::schemas::properties::MetricsProperties;
use regex::Regex;

/// Validates all aspects of a metric according to the rules ported from Python dbt.
/// This is a unified validation function that checks:
/// - Metric name validity
/// - Window validity (if present)
///
/// # Arguments
/// * `props` - The metric properties to validate
///
/// # Returns
/// * `Ok(())` if the metric is valid
/// * `Err(FsError)` with DbtYamlValidationError code if any validation fails
pub fn validate_metric(props: &MetricsProperties) -> FsResult<()> {
    // Validate metric name
    validate_metric_name(&props.name)?;

    // Validate window if present
    if let Some(window) = &props.window {
        validate_metric_window(window)?;
    }

    Ok(())
}

/// Validates a metric name according to the rules ported from Python dbt:
/// - Cannot contain spaces
/// - Cannot be longer than 250 characters
/// - Must begin with a letter
/// - Must contain only letters, numbers, and underscores
///
/// # Arguments
/// * `name` - The metric name to validate
///
/// # Returns
/// * `Ok(())` if the name is valid
/// * `Err(FsError)` with DbtYamlValidationError code if the name is invalid
///
/// # Example
/// ```rust
/// use dbt_parser::resolve::validate_metric_name;
/// assert!(validate_metric_name("valid_metric_name").is_ok());
/// assert!(validate_metric_name("invalid name").is_err()); // contains spaces
/// ```
pub fn validate_metric_name(name: &str) -> FsResult<()> {
    let mut errors = Vec::new();

    // Check for spaces
    if name.contains(' ') {
        errors.push("cannot contain spaces");
    }

    // Check length (BigQuery and Snowflake limitation)
    if name.len() > 250 {
        errors.push("cannot contain more than 250 characters");
    }

    // Check if it starts with a letter
    let starts_with_letter = Regex::new(r"^[A-Za-z]").expect("valid regex");
    if !starts_with_letter.is_match(name) {
        errors.push("must begin with a letter");
    }

    // Check if it contains only valid characters (letters, numbers, underscores)
    let valid_chars = Regex::new(r"^[\w]+$").expect("valid regex");
    if !valid_chars.is_match(name) {
        errors.push("must contain only letters, numbers and underscores");
    }

    if !errors.is_empty() {
        return Err(fs_err!(
            ErrorCode::DbtYamlValidationError,
            "The metric name '{}' is invalid. It {}",
            name,
            errors.join(", ")
        ));
    }

    Ok(())
}

/// Validates a metric window string according to the rules ported from Python dbt:
/// - Must be in the format "<count> <granularity>" (e.g., "28 days")
/// - Count must be a positive integer
/// - Granularity must be a valid time granularity (day, week, month, etc.)
/// - Granularity can be plural (days, weeks, months) and will be converted to singular
///
/// # Arguments
/// * `window` - The window string to validate
///
/// # Returns
/// * `Ok(())` if the window is valid
/// * `Err(FsError)` with DbtYamlValidationError code if the window is invalid
pub fn validate_metric_window(window: &str) -> FsResult<()> {
    match MetricTimeWindow::from_string(window.to_string()) {
        Ok(_) => Ok(()),
        Err(err) => Err(fs_err!(ErrorCode::DbtYamlValidationError, "{}", err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_metric_names() {
        assert!(validate_metric_name("valid_metric").is_ok());
        assert!(validate_metric_name("ValidMetric").is_ok());
        assert!(validate_metric_name("metric123").is_ok());
        assert!(validate_metric_name("metric_with_underscores").is_ok());
        assert!(validate_metric_name("a").is_ok());
        assert!(validate_metric_name("A").is_ok());
    }

    #[test]
    fn test_invalid_metric_names_with_spaces() {
        let result = validate_metric_name("invalid name");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(error.context.contains("cannot contain spaces"));
    }

    #[test]
    fn test_invalid_metric_names_too_long() {
        let long_name = "a".repeat(251);
        let result = validate_metric_name(&long_name);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(
            error
                .context
                .contains("cannot contain more than 250 characters")
        );
    }

    #[test]
    fn test_invalid_metric_names_not_starting_with_letter() {
        let result = validate_metric_name("123metric");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(error.context.contains("must begin with a letter"));
    }

    #[test]
    fn test_invalid_metric_names_invalid_chars() {
        let result = validate_metric_name("metric@name");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(
            error
                .context
                .contains("must contain only letters, numbers and underscores")
        );
    }

    #[test]
    fn test_invalid_metric_names_with_hyphens() {
        let result = validate_metric_name("metric-with-hyphens");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(
            error
                .context
                .contains("must contain only letters, numbers and underscores")
        );
    }

    #[test]
    fn test_multiple_validation_errors() {
        let result = validate_metric_name("123 invalid@name");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        let context = &error.context;
        assert!(context.contains("cannot contain spaces"));
        assert!(context.contains("must begin with a letter"));
        assert!(context.contains("must contain only letters, numbers and underscores"));
    }

    #[test]
    fn test_valid_window_strings() {
        assert!(validate_metric_window("1 day").is_ok());
        assert!(validate_metric_window("28 days").is_ok());
        assert!(validate_metric_window("7 weeks").is_ok());
        assert!(validate_metric_window("12 months").is_ok());
        assert!(validate_metric_window("1 year").is_ok());
        assert!(validate_metric_window("24 hours").is_ok());
        assert!(validate_metric_window("60 minutes").is_ok());
        assert!(validate_metric_window("3600 seconds").is_ok());
        assert!(validate_metric_window("4 quarters").is_ok());
    }

    #[test]
    fn test_invalid_window_wrong_format() {
        let result = validate_metric_window("invalid");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(
            error
                .context
                .contains("Should be of the form `<count> <granularity>`")
        );
    }

    #[test]
    fn test_invalid_window_too_many_parts() {
        let result = validate_metric_window("1 2 3 days");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(
            error
                .context
                .contains("Should be of the form `<count> <granularity>`")
        );
    }

    #[test]
    fn test_invalid_window_non_numeric_count() {
        let result = validate_metric_window("abc days");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(error.context.contains("Invalid count (abc)"));
    }

    #[test]
    fn test_invalid_window_negative_count() {
        let result = validate_metric_window("-5 days");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(error.context.contains("Invalid count (-5)"));
    }

    #[test]
    fn test_invalid_window_invalid_granularity() {
        let result = validate_metric_window("7 fortnights");
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(error.context.contains("Invalid granularity (fortnights)"));
    }

    #[test]
    fn test_window_case_insensitive() {
        assert!(validate_metric_window("1 DAY").is_ok());
        assert!(validate_metric_window("1 Day").is_ok());
        assert!(validate_metric_window("1 DAYS").is_ok());
        assert!(validate_metric_window("28 Days").is_ok());
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::collections::HashSet;

    /// Simulates how duplicate checking works in the resolve functions
    fn check_duplicate_names(names: &[&str]) -> FsResult<()> {
        let mut seen_names = HashSet::new();
        for &name in names {
            // Validate individual name first
            validate_metric_name(name)?;

            // Check for duplicates
            if !seen_names.insert(name.to_string()) {
                return Err(fs_err!(
                    ErrorCode::DbtYamlValidationError,
                    "Duplicate metric name '{}' found in package",
                    name
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn test_no_duplicate_names() {
        let names = vec!["metric_one", "metric_two", "metric_three"];
        assert!(check_duplicate_names(&names).is_ok());
    }

    #[test]
    fn test_duplicate_names_detected() {
        let names = vec!["metric_one", "metric_two", "metric_one"];
        let result = check_duplicate_names(&names);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(error.context.contains("Duplicate metric name 'metric_one'"));
    }

    #[test]
    fn test_invalid_name_caught_before_duplicate_check() {
        let names = vec!["valid_metric", "invalid name", "invalid name"];
        let result = check_duplicate_names(&names);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        // Should fail on the individual name validation, not duplicate
        assert!(error.context.contains("invalid name"));
        assert!(error.context.contains("cannot contain spaces"));
    }

    #[test]
    fn test_unified_validation_valid_metric() {
        let props = MetricsProperties {
            name: "valid_metric".to_string(),
            window: Some("28 days".to_string()),
            ..Default::default()
        };

        assert!(validate_metric(&props).is_ok());
    }

    #[test]
    fn test_unified_validation_invalid_metric_name() {
        let props = MetricsProperties {
            name: "123invalid".to_string(),
            window: Some("28 days".to_string()),
            ..Default::default()
        };

        let result = validate_metric(&props);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(error.context.contains("must begin with a letter"));
    }

    #[test]
    fn test_unified_validation_invalid_window() {
        let props = MetricsProperties {
            name: "valid_metric".to_string(),
            window: Some("invalid window".to_string()),
            ..Default::default()
        };

        let result = validate_metric(&props);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::DbtYamlValidationError);
        assert!(error.context.contains("Invalid count (invalid)"));
    }

    #[test]
    fn test_unified_validation_no_window() {
        let props = MetricsProperties {
            name: "valid_metric".to_string(),
            ..Default::default()
        };
        assert!(validate_metric(&props).is_ok());
    }
}
