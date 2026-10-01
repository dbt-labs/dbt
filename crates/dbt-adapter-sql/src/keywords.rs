use dbt_adapter_core::AdapterType;

/// Returns a sorted slice of reserved keywords for the given adapter type.
pub fn reserved_keywords(backend: AdapterType) -> &'static [&'static str] {
    use AdapterType::*;
    use dbt_sql_keywords::*;
    match backend {
        Snowflake => snowflake::RESERVED_KEYWORDS,
        Bigquery => bigquery::RESERVED_KEYWORDS,
        Databricks => databricks::RESERVED_KEYWORDS,
        Redshift => redshift::RESERVED_KEYWORDS,
        DuckDB => duckdb::RESERVED_KEYWORDS,
        Trino => trino::RESERVED_KEYWORDS,
        // TODO: fill in other dialects' keywords and define a default fallback
        _ => &[],
    }
}

/// Returns a sorted slice of strict non-reserved keywords for the given adapter type.
pub fn strict_non_reserved_keywords(backend: AdapterType) -> &'static [&'static str] {
    use AdapterType::*;
    use dbt_sql_keywords::*;
    match backend {
        Snowflake => snowflake::STRICT_NON_RESERVED_KEYWORDS,
        Bigquery => bigquery::STRICT_NON_RESERVED_KEYWORDS,
        Databricks => databricks::STRICT_NON_RESERVED_KEYWORDS,
        Redshift => redshift::STRICT_NON_RESERVED_KEYWORDS,
        DuckDB => duckdb::STRICT_NON_RESERVED_KEYWORDS,
        Trino => trino::STRICT_NON_RESERVED_KEYWORDS,
        // TODO: fill in other dialects' keywords and define a default fallback
        _ => &[],
    }
}

/// Returns a sorted slice of non-reserved keywords for the given adapter type.
#[allow(dead_code)]
pub fn non_reserved_keywords(backend: AdapterType) -> &'static [&'static str] {
    use AdapterType::*;
    use dbt_sql_keywords::*;
    match backend {
        Snowflake => snowflake::NON_RESERVED_KEYWORDS,
        Bigquery => bigquery::NON_RESERVED_KEYWORDS,
        Databricks => databricks::NON_RESERVED_KEYWORDS,
        Redshift => redshift::NON_RESERVED_KEYWORDS,
        DuckDB => duckdb::NON_RESERVED_KEYWORDS,
        Trino => trino::NON_RESERVED_KEYWORDS,
        _ => &[],
    }
}

/// Returns the uppercase version of the given token if it is a keyword.
///
/// This function makes no string allocations and callers don't need to allocate
/// a new uppercase string for every token they want to check.
///
/// IMPORTANT: Being a "keyword" here means being present in either RESERVED_KEYWORDS or
/// STRICT_NON_RESERVED_KEYWORDS lists of the given backend. But NOT the NON_RESERVED list.
pub fn is_keyword_ignore_ascii_case(token: &str, backend: AdapterType) -> Option<&'static str> {
    use dbt_sql_keywords::is_keyword_ignore_ascii_case;
    is_keyword_ignore_ascii_case(token, reserved_keywords(backend))
        .or_else(|| is_keyword_ignore_ascii_case(token, strict_non_reserved_keywords(backend)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_keyword_ignore_ascii_case() {
        fn is_kw(ident: &str) -> Option<&'static str> {
            is_keyword_ignore_ascii_case(ident, AdapterType::Bigquery)
        }

        assert_eq!(is_kw("select"), Some("SELECT"));
        assert_eq!(is_kw("SeLeCt"), Some("SELECT"));
        assert_eq!(is_kw("SELECTED"), None);
        assert_eq!(is_kw("SEL"), None);
        assert_eq!(is_kw("ASELECT"), None);
        assert_eq!(is_kw("ZSELECT"), None);
        assert_eq!(is_kw("null"), Some("NULL"));
        assert_eq!(is_kw("NULLs"), Some("NULLS"));
        assert_eq!(is_kw("nulos"), None);
        for kw in dbt_sql_keywords::bigquery::RESERVED_KEYWORDS {
            assert_eq!(is_kw(kw), Some(*kw));
            assert_eq!(is_kw(kw.to_ascii_lowercase().as_str()), Some(*kw));
            let not_kw = format!("X{kw}");
            assert_eq!(is_kw(&not_kw), None);
            let not_kw = format!("{kw}X");
            assert_eq!(is_kw(&not_kw), None);
            let not_kw = format!("☃{kw}☃");
            assert_eq!(is_kw(&not_kw), None);
        }
    }
}
