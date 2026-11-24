use crate::Dialect;

/// Whether `function_name` is a table-valued function that only generates rows
/// from its arguments (it never reads warehouse state), for `dialect`.
///
/// Matching is case-insensitive. Qualified names (`db.sch.f`) are never pure.
pub fn is_pure_generator(dialect: Dialect, function_name: &str) -> bool {
    let name = function_name.trim().to_ascii_lowercase();
    let allowed: &[&str] = match dialect {
        Dialect::Databricks | Dialect::SparkSql | Dialect::SparkLp => &[
            "range",
            "explode",
            "explode_outer",
            "posexplode",
            "posexplode_outer",
            "inline",
            "inline_outer",
            "stack",
        ],
        Dialect::Snowflake => &["flatten", "generator"],
        Dialect::Postgresql | Dialect::Redshift => &["generate_series"],
        Dialect::Duckdb | Dialect::Trino => &["range", "generate_series", "sequence"],
        _ => &[],
    };
    allowed.contains(&name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_is_dialect_scoped_and_case_insensitive() {
        assert!(is_pure_generator(Dialect::Databricks, "EXPLODE"));
        assert!(is_pure_generator(Dialect::SparkSql, "stack"));
        assert!(is_pure_generator(Dialect::Snowflake, "Flatten"));
        assert!(is_pure_generator(Dialect::Snowflake, "generator"));
        assert!(is_pure_generator(Dialect::Redshift, "generate_series"));
        assert!(is_pure_generator(Dialect::Postgresql, "generate_series"));
        assert!(is_pure_generator(Dialect::Duckdb, "range"));
        assert!(is_pure_generator(Dialect::Trino, "sequence"));
        assert!(!is_pure_generator(Dialect::Snowflake, "explode"));
        assert!(!is_pure_generator(Dialect::Databricks, "flatten"));
        assert!(!is_pure_generator(Dialect::Redshift, "range"));
    }

    #[test]
    fn other_functions_and_qualified_names_are_not_pure() {
        assert!(!is_pure_generator(Dialect::Databricks, "table_changes"));
        assert!(!is_pure_generator(Dialect::Databricks, "read_files"));
        assert!(!is_pure_generator(Dialect::Snowflake, "result_scan"));
        assert!(!is_pure_generator(Dialect::Snowflake, "db.sch.flatten"));
        assert!(!is_pure_generator(Dialect::Databricks, "main.range"));
        assert!(!is_pure_generator(Dialect::Bigquery, "range"));
    }
}
