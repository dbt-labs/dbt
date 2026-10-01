// some constants in the auto-generated keyword list are shadowed by manually curated lists
#![allow(dead_code)]

mod generated;

pub mod bigquery;
pub mod bigqueryuntyped;
pub mod databricks;
pub mod duckdb;
pub mod exasol;
pub mod mssql;
pub mod redshift;
pub mod snowflake;
pub mod trino;

/// Compares an uppercase keyword with a token in a case-insensitive manner.
///
/// This function exists because we don't want to heap-allocate a new uppercase
/// string for every token we want to check.
///
/// PRE-CONDITION: `kw` is uppercase.
#[inline]
fn keyword_cmp_ignore_ascii_case(kw: &str, token: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    let mut a = kw.as_bytes();
    let mut b = token.as_bytes();
    while let ([first_a, rest_a @ ..], [first_b, rest_b @ ..]) = (a, b) {
        // first_a is already uppercase because kw is uppercase
        match first_a.cmp(&first_b.to_ascii_uppercase()) {
            Less => return Less,
            Greater => return Greater,
            Equal => {
                a = rest_a;
                b = rest_b;
            }
        }
    }
    a.len().cmp(&b.len())
}

/// Returns the uppercase version of the given token if it is a reserved keyword.
///
/// This function relies on binary-search and makes no string allocations. Callers
/// don't need to allocate a new uppercase string for every token they want to check.
///
/// It assumes that `sorted_uppercase_keywords` is sorted and are all uppercase ASCII.
/// As such, it's meant to be wrapped by a function that provides the appropriate
/// keyword list for a given SQL dialect.
///
/// If these requirements are not fulfilled, the returned result is unspecified
/// and meaningless.
#[inline(always)]
pub fn is_keyword_ignore_ascii_case(
    token: &str,
    sorted_uppercase_keywords: &[&'static str],
) -> Option<&'static str> {
    sorted_uppercase_keywords
        .binary_search_by(|kw| keyword_cmp_ignore_ascii_case(kw, token))
        .ok()
        .map(|idx| sorted_uppercase_keywords[idx])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keyword_cmp_ignore_ascii_case() {
        use std::cmp::Ordering::*;
        assert_eq!(keyword_cmp_ignore_ascii_case("SELECT", "select"), Equal);
        assert_eq!(keyword_cmp_ignore_ascii_case("SELECT", "SeLeCt"), Equal);
        assert_eq!(keyword_cmp_ignore_ascii_case("SELECT", "SELECTED"), Less);
        assert_eq!(keyword_cmp_ignore_ascii_case("SELECT", "SEL"), Greater);
        assert_eq!(keyword_cmp_ignore_ascii_case("SELECT", "ASELECT"), Greater);
        assert_eq!(keyword_cmp_ignore_ascii_case("SELECT", "ZSELECT"), Less);
    }

    struct KeywordLists<'a> {
        pub name: &'static str,
        pub reserved: &'a [&'static str],
        pub strict_non_reserved: &'a [&'static str],
        pub non_reserved: &'a [&'static str],
    }

    static KEYWORD_LISTS: [KeywordLists; 8] = [
        KeywordLists {
            name: "bigquery",
            reserved: bigquery::RESERVED_KEYWORDS,
            strict_non_reserved: bigquery::STRICT_NON_RESERVED_KEYWORDS,
            non_reserved: bigquery::NON_RESERVED_KEYWORDS,
        },
        KeywordLists {
            name: "databricks",
            reserved: databricks::RESERVED_KEYWORDS,
            strict_non_reserved: databricks::STRICT_NON_RESERVED_KEYWORDS,
            non_reserved: databricks::NON_RESERVED_KEYWORDS,
        },
        KeywordLists {
            name: "duckdb",
            reserved: duckdb::RESERVED_KEYWORDS,
            strict_non_reserved: duckdb::STRICT_NON_RESERVED_KEYWORDS,
            non_reserved: duckdb::NON_RESERVED_KEYWORDS,
        },
        KeywordLists {
            name: "exasol",
            reserved: exasol::RESERVED_KEYWORDS,
            strict_non_reserved: exasol::STRICT_NON_RESERVED_KEYWORDS,
            non_reserved: exasol::NON_RESERVED_KEYWORDS,
        },
        KeywordLists {
            name: "mssql",
            reserved: mssql::RESERVED_KEYWORDS,
            strict_non_reserved: mssql::STRICT_NON_RESERVED_KEYWORDS,
            non_reserved: mssql::NON_RESERVED_KEYWORDS,
        },
        KeywordLists {
            name: "redshift",
            reserved: redshift::RESERVED_KEYWORDS,
            strict_non_reserved: redshift::STRICT_NON_RESERVED_KEYWORDS,
            non_reserved: redshift::NON_RESERVED_KEYWORDS,
        },
        KeywordLists {
            name: "snowflake",
            reserved: snowflake::RESERVED_KEYWORDS,
            strict_non_reserved: snowflake::STRICT_NON_RESERVED_KEYWORDS,
            non_reserved: snowflake::NON_RESERVED_KEYWORDS,
        },
        KeywordLists {
            name: "trino",
            reserved: trino::RESERVED_KEYWORDS,
            strict_non_reserved: trino::STRICT_NON_RESERVED_KEYWORDS,
            non_reserved: trino::NON_RESERVED_KEYWORDS,
        },
    ];

    fn assert_sorted(name: &str, list: &[&str]) {
        for i in 1..list.len() {
            assert!(
                list[i - 1] < list[i],
                "{name} is not sorted: '{}' >= '{}'",
                list[i - 1],
                list[i]
            );
        }
    }

    #[test]
    fn test_keywords_are_sorted() {
        for lists in &KEYWORD_LISTS {
            let name = format!("{}.{}", lists.name, "RESERVED_KEYWORDS");
            assert_sorted(&name, lists.reserved);
            let name = format!("{}.{}", lists.name, "STRICT_NON_RESERVED_KEYWORDS");
            assert_sorted(&name, lists.strict_non_reserved);
            let name = format!("{}.{}", lists.name, "NON_RESERVED_KEYWORDS");
            assert_sorted(&name, lists.non_reserved);
        }
    }

    fn is_keyword(keyword_lists: &KeywordLists<'_>, token: &str) -> Option<&'static str> {
        is_keyword_ignore_ascii_case(token, keyword_lists.reserved)
            .or_else(|| is_keyword_ignore_ascii_case(token, keyword_lists.strict_non_reserved))
            // only in tests, we include non_reserved keywords as well
            .or_else(|| is_keyword_ignore_ascii_case(token, keyword_lists.non_reserved))
    }

    fn assert_is_keyword(keyword_lists: &KeywordLists<'_>, token: &str, expected: &'static str) {
        match is_keyword(keyword_lists, token) {
            Some(keyword) => assert_eq!(
                keyword, expected,
                "Expected '{}' to be a reserved keyword in {}, but got {:?}",
                token, keyword_lists.name, keyword_lists.name
            ),
            None => panic!(
                "Expected '{}' to be a reserved keyword in {}, but got None",
                token, keyword_lists.name
            ),
        }
    }

    fn assert_is_not_keyword(keyword_lists: &KeywordLists<'_>, token: &str) {
        assert!(
            is_keyword(keyword_lists, token).is_none(),
            "Expected '{}' to NOT be a reserved keyword in {}, but it was",
            token,
            keyword_lists.name
        );
    }

    #[test]
    fn test_is_keyword_ignore_ascii_case() {
        for lists in &KEYWORD_LISTS {
            assert_is_keyword(lists, "select", "SELECT");
            assert_is_keyword(lists, "SeLeCt", "SELECT");
            assert_is_not_keyword(lists, "SELECTED");
            assert_is_not_keyword(lists, "SEL");
            assert_is_not_keyword(lists, "ASELECT");
            assert_is_not_keyword(lists, "ZSELECT");
            assert_is_keyword(lists, "null", "NULL");
            assert_is_not_keyword(lists, "nulos");

            for kw in lists.reserved {
                assert_is_keyword(lists, kw, kw);
                assert_is_keyword(lists, kw.to_ascii_lowercase().as_str(), kw);
                let not_kw = format!("X{kw}");
                assert_is_not_keyword(lists, &not_kw);
                let not_kw = format!("{kw}X");
                assert_is_not_keyword(lists, &not_kw);
                let not_kw = format!("☃{kw}☃");
                assert_is_not_keyword(lists, &not_kw);
            }
        }
    }
}
