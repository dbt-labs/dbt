use crate::{ErrorCode, FsResult, fs_err};

/// Splits the input string into words, but treats every {..} as one word
pub fn split_into_whitespace_and_brackets(input: &str) -> Vec<String> {
    // Used to prepare programmatic commandline parsing
    let mut results = Vec::new();
    let input = input.replace('\n', " ");
    let mut braces_ct = 0;
    let mut segment = String::new();
    // splut input into sections of  any str, {, any string '}'
    for ch in input.chars() {
        match ch {
            '{' => {
                if braces_ct > 0 {
                    segment.push(ch);
                    braces_ct += 1;
                } else {
                    if !segment.is_empty() {
                        results.push(segment);
                        segment = String::new();
                    }
                    segment.push(ch);
                    braces_ct += 1;
                }
            }
            '}' => match braces_ct.cmp(&1) {
                std::cmp::Ordering::Greater => {
                    segment.push(ch);
                    braces_ct -= 1;
                }
                std::cmp::Ordering::Equal => {
                    segment.push(ch);
                    results.push(segment);
                    segment = String::new();
                    braces_ct -= 1;
                }
                std::cmp::Ordering::Less => {
                    segment.push(ch);
                }
            },
            _ => segment.push(ch),
        }
    }
    if !segment.is_empty() {
        results.push(segment);
    }
    let segments = results;
    let mut results = Vec::new();
    for segment in segments {
        // let segment = segment.trim().to_string();
        if segment.starts_with('{') {
            results.push(segment);
            continue;
        }
        let words = segment.split_whitespace();
        for word in words {
            results.push(word.to_string());
        }
    }
    results
}

/// Extracts the test-name component (3rd dot-separated part) from a dbt test unique_id.
///
/// Test unique_ids follow the format `test.<pkg>.<name>.<trailing_hash>`.
/// Returns `None` if the uid doesn't have at least three components.
pub fn test_name_from_uid(uid: &str) -> Option<&str> {
    let mut parts = uid.splitn(4, '.');
    let _ = (parts.next(), parts.next()); // skip "test" and package
    parts.next()
}

/// Truncates a test name to 63 characters if it's too long, following dbt-core's logic.
/// This is done by including the first 30 identifying chars plus a 32-character hash of the full contents.
/// See the function `synthesize_generic_test_name` in `dbt-core`:
/// https://github.com/dbt-labs/dbt-core/blob/9010537499980743503ed3b462eb1952be4d2b38/core/dbt/parser/generic_test_builders.py
pub fn maybe_truncate_test_name(test_identifier: &str, full_name: &str) -> String {
    if full_name.len() >= 64 {
        let test_trunc_identifier: String = test_identifier.chars().take(30).collect();
        let hash = md5::compute(full_name);
        let res: String = format!("{test_trunc_identifier}_{hash:x}");
        res
    } else {
        full_name.to_string()
    }
}

/// Returns true if `identifier` looks like a canonical dbt test temp identifier.
///
/// This is intentionally conservative and used by multiple crates:
/// - replay/alpha-conversion logic (record vs replay temp relation names)
/// - SQL diff canonicalization (ignoring drift for test temp relations)
///
/// Behavior:
/// - trims whitespace
/// - strips a single leading/trailing quote character if present (`"`, `'`, or `` ` ``)
/// - case-insensitive check for `test_` prefix
pub fn is_test_temp_identifier(identifier: &str) -> bool {
    let identifier = identifier.trim();

    // Strip one layer of quoting (quotes may be present depending on adapter / quoting policy).
    let identifier = identifier
        .strip_prefix('"')
        .or_else(|| identifier.strip_prefix('\''))
        .or_else(|| identifier.strip_prefix('`'))
        .unwrap_or(identifier);
    let identifier = identifier
        .strip_suffix('"')
        .or_else(|| identifier.strip_suffix('\''))
        .or_else(|| identifier.strip_suffix('`'))
        .unwrap_or(identifier);

    identifier.to_ascii_lowercase().starts_with("test_")
}

/// Parse a string config value as a boolean.
///
/// Accepts `"true"`/`"false"` case-insensitively with surrounding
/// whitespace trimmed. Returns `Ok(None)` for `None` input; returns an
/// `InvalidConfig` `FsError` naming `key` for unparseable values.
pub fn try_parse_bool_str(s: Option<&str>, key: &str) -> FsResult<Option<bool>> {
    match s {
        None => Ok(None),
        Some(v) if v.trim().eq_ignore_ascii_case("true") => Ok(Some(true)),
        Some(v) if v.trim().eq_ignore_ascii_case("false") => Ok(Some(false)),
        Some(_) => Err(fs_err!(
            ErrorCode::InvalidConfig,
            "Failed to parse {}, expected \"true\" or \"false\"",
            key
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_simple() {
        let input = "hello world";
        let expected = vec!["hello", "world"];
        assert_eq!(split_into_whitespace_and_brackets(input), expected);
    }

    #[test]
    fn test_split_with_braces() {
        let input = "hello {world}";
        let expected = vec!["hello", "{world}"];
        assert_eq!(split_into_whitespace_and_brackets(input), expected);
    }

    #[test]
    fn test_split_with_quotes() {
        let input = r#"hello "world""#;
        let expected = vec!["hello", "\"world\""];
        assert_eq!(split_into_whitespace_and_brackets(input), expected);
    }

    #[test]
    fn test_split_with_braces_and_quotes() {
        let input = r#"hello { "world "  }"#;
        let expected = vec!["hello", "{ \"world \"  }"];
        assert_eq!(split_into_whitespace_and_brackets(input), expected);
    }

    #[test]
    fn try_parse_bool_str_missing_input_yields_none() {
        assert_eq!(try_parse_bool_str(None, "ra3_node").unwrap(), None);
    }

    #[test]
    fn try_parse_bool_str_parses_true_in_any_casing() {
        for s in ["true", "True", "TRUE", "tRuE"] {
            assert_eq!(try_parse_bool_str(Some(s), "k").unwrap(), Some(true));
        }
    }

    #[test]
    fn try_parse_bool_str_parses_false_in_any_casing() {
        for s in ["false", "False", "FALSE", "fAlSe"] {
            assert_eq!(try_parse_bool_str(Some(s), "k").unwrap(), Some(false));
        }
    }

    #[test]
    fn try_parse_bool_str_trims_surrounding_whitespace() {
        assert_eq!(try_parse_bool_str(Some(" true"), "k").unwrap(), Some(true));
        assert_eq!(try_parse_bool_str(Some("true "), "k").unwrap(), Some(true));
        assert_eq!(
            try_parse_bool_str(Some("  TRUE  "), "k").unwrap(),
            Some(true)
        );
        assert_eq!(
            try_parse_bool_str(Some(" false "), "k").unwrap(),
            Some(false)
        );
    }

    #[test]
    fn try_parse_bool_str_unparseable_yields_invalid_config_error_naming_the_key() {
        let err = try_parse_bool_str(Some("yes"), "ra3_node").unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidConfig);
        assert!(
            err.to_string().contains("ra3_node"),
            "error should name the failing key, got: {err}"
        );
    }
}
