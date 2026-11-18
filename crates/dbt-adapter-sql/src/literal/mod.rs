use dbt_adapter_core::AdapterType;

/// Escape `s` for inclusion inside a single-quoted SQL *string literal*, so a
/// value can never terminate the literal early. The literal-value member of
/// the family next to [`sanitize_identifier`](crate::ident::sanitize_identifier)
/// (strip) and [`quote_identifier`](crate::ident::quote_identifier) (quote),
/// which handle identifiers.
pub fn escape_string_literal(s: &str, backend: AdapterType) -> String {
    match backend {
        AdapterType::Snowflake | AdapterType::Redshift | AdapterType::Postgres => {
            s.replace('\\', "\\\\").replace('\'', "''")
        }
        AdapterType::Bigquery
        | AdapterType::Databricks
        | AdapterType::Spark
        | AdapterType::ClickHouse => s.replace('\\', "\\\\").replace('\'', "\\'"),
        AdapterType::Salesforce
        | AdapterType::DuckDB
        | AdapterType::Fabric
        | AdapterType::Exasol
        | AdapterType::Athena
        | AdapterType::Starburst
        | AdapterType::Trino
        | AdapterType::Datafusion
        | AdapterType::Dremio
        | AdapterType::Oracle
        | AdapterType::LakeCompute => s.replace('\'', "''"),
    }
}

/// Render `s` as a single-quoted SQL string literal, escaped with
/// [`escape_string_literal`].
pub fn quote_string_literal(s: &str, backend: AdapterType) -> String {
    format!("'{}'", escape_string_literal(s, backend))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unescape_doubled_quote_and_backslash(value: &str) -> String {
        let mut chars = value.chars().peekable();
        let mut result = String::new();
        while let Some(c) = chars.next() {
            if c == '\\' && chars.peek() == Some(&'\\') {
                chars.next();
                result.push('\\');
            } else if c == '\'' && chars.peek() == Some(&'\'') {
                chars.next();
                result.push('\'');
            } else {
                result.push(c);
            }
        }
        result
    }

    fn unescape_backslash_literal(value: &str) -> String {
        let mut chars = value.chars();
        let mut result = String::new();
        while let Some(c) = chars.next() {
            if c == '\\' {
                result.push(chars.next().expect("escaped character"));
            } else {
                result.push(c);
            }
        }
        result
    }

    #[test]
    fn quote_string_literal_wraps_and_escapes() {
        let q = |s: &str| quote_string_literal(s, AdapterType::DuckDB);
        assert_eq!(q("plain"), "'plain'");
        assert_eq!(q("o'brien"), "'o''brien'");
        assert_eq!(q(""), "''");
    }

    #[test]
    fn escape_string_literal_doubles_single_quotes() {
        let e = |s: &str| escape_string_literal(s, AdapterType::DuckDB);
        assert_eq!(e("o'brien"), "o''brien");
        assert_eq!(e("'; DROP TABLE x; --"), "''; DROP TABLE x; --");
        assert_eq!(e("plain"), "plain");
    }

    #[test]
    fn escape_string_literal_escapes_snowflake_backslashes() {
        assert_eq!(
            escape_string_literal(r"a\b'c", AdapterType::Snowflake),
            r"a\\b''c"
        );
    }

    #[test]
    fn escape_string_literal_uses_backslash_escapes_for_hive_style_literals() {
        assert_eq!(
            escape_string_literal(r"a\b'c", AdapterType::Databricks),
            r"a\\b\'c"
        );
    }

    #[test]
    fn escape_string_literal_uses_backslash_escapes_for_bigquery_and_clickhouse() {
        for backend in [AdapterType::Bigquery, AdapterType::ClickHouse] {
            assert_eq!(escape_string_literal(r"a\b'c", backend), r"a\\b\'c");
        }
    }

    #[test]
    fn escape_string_literal_doubles_redshift_backslashes_and_quotes() {
        assert_eq!(
            escape_string_literal(r"a\b'c", AdapterType::Redshift),
            r"a\\b''c"
        );
    }

    #[test]
    fn escape_string_literal_round_trips_adversarial_values() {
        let values = [
            "'",
            "\\",
            "$$",
            "; --",
            "/* comment */",
            "line\nbreak",
            r"\' ; SELECT 1; --",
            r"'\'\\''",
        ];

        for backend in [
            AdapterType::Snowflake,
            AdapterType::Redshift,
            AdapterType::Postgres,
        ] {
            for value in values {
                let escaped = escape_string_literal(value, backend);
                assert_eq!(unescape_doubled_quote_and_backslash(&escaped), value);
                let escaped_twice = escape_string_literal(&escaped, backend);
                assert_eq!(
                    unescape_doubled_quote_and_backslash(&escaped_twice),
                    escaped
                );
            }
        }

        for backend in [
            AdapterType::Bigquery,
            AdapterType::Databricks,
            AdapterType::Spark,
            AdapterType::ClickHouse,
        ] {
            for value in values {
                let escaped = escape_string_literal(value, backend);
                assert_eq!(unescape_backslash_literal(&escaped), value);
            }
        }
    }
}
