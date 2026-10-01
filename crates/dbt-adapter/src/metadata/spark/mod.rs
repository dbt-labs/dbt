use crate::column::Column;

/// Keep only the leading column names from a Spark `describe extended` result,
/// discarding all columns from a separator column name (empty string or starts
/// with `#`)
///
/// Mirrors dbt-core's `SparkAdapter.parse_describe_extended` from dbt-adapters/dbt-spark
pub(crate) fn truncate_at_describe_extended_separator(columns: Vec<Column>) -> Vec<Column> {
    let end = columns
        .iter()
        .position(|c| c.name().is_empty() || c.name().starts_with('#'))
        .unwrap_or(columns.len());
    columns.into_iter().take(end).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_adapter_core::AdapterType;

    #[test]
    fn test_truncate_at_describe_extended_separator() {
        let mk = |name: &str| {
            Column::new(
                AdapterType::Spark,
                name.to_string(),
                "string".to_string(),
                None,
                None,
                None,
            )
        };

        // Real columns, then the blank separator, then the extended-metadata section.
        let columns = vec![
            mk("id"),
            mk("name"),
            mk(""),
            mk("# Detailed Table Information"),
            mk("Catalog"),
            mk("Database"),
        ];
        let kept = truncate_at_describe_extended_separator(columns);
        let names: Vec<&str> = kept.iter().map(|c| c.name()).collect();
        assert_eq!(names, vec!["id", "name"]);

        // No separator: every column is kept.
        let columns = vec![mk("id"), mk("name")];
        assert_eq!(truncate_at_describe_extended_separator(columns).len(), 2);

        // A leading '#' row truncates to empty.
        let columns = vec![mk("# Detailed Table Information")];
        assert!(truncate_at_describe_extended_separator(columns).is_empty());

        // Empty input stays empty.
        assert!(truncate_at_describe_extended_separator(vec![]).is_empty());
    }
}
