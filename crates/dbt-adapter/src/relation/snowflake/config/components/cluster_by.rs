use dbt_common::{AdapterError, AdapterResult};
use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes};
use minijinja::Value;

use crate::relation::snowflake::config::{
    SnowflakeDescribeResults, get_string_by_name_from_record_batch,
};
use crate::{
    relation::config_v2::{
        ComponentConfig, ComponentConfigLoader, SimpleComponentConfigImpl, diff, impl_loader,
    },
    value::none_value,
};

pub(crate) const TYPE_NAME: &str = "cluster_by";

/// Component for Snowflake dynamic table cluster by
pub(crate) type ClusterBy = SimpleComponentConfigImpl<Option<String>>;

fn to_jinja(v: &Option<String>) -> Value {
    v.as_ref().map(Value::from).unwrap_or_else(none_value)
}

/// True when the `(` at index 0 is the one closed by the final character — i.e. paren depth
/// first returns to 0 exactly at the last character, ignoring parens inside a quoted
/// identifier or a single-quoted string literal (e.g. the `)` in `to_char(ts, 'A)B')`).
///
/// A mere `starts_with('(') && ends_with(')')` check is NOT a balance check: the leading and
/// trailing parens can belong to unrelated groups, e.g. `(a), to_date(ts)`.
fn has_balanced_outer_parens(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    if chars.first() != Some(&'(') || chars.last() != Some(&')') {
        return false;
    }
    let mut depth = 0usize;
    let mut quote_char: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote_char {
            if c == q {
                if chars.get(i + 1) == Some(&q) {
                    i += 1; // doubled escape -- stay inside the quoted span
                } else {
                    quote_char = None;
                }
            }
        } else {
            match c {
                '"' | '\'' => quote_char = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return i == chars.len() - 1;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    false
}

/// Only strips when the leading `(` is the one closed by the trailing `)`, so a single
/// clustering *expression* like `to_date(ts)` is left intact.
fn strip_outer_parens(value: &str) -> &str {
    if has_balanced_outer_parens(value) {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

/// Strip a leading `LINEAR` type descriptor, e.g. `LINEAR(id, val)` -> `(id, val)`.
///
/// `SHOW DYNAMIC TABLES` wraps its `cluster_by` column this way (confirmed live, account
/// `ktb38830`, 2026-08-27); `SHOW INTERACTIVE TABLES` does not (bare parens, no `LINEAR`
/// wrapper, confirmed by the same probe). Stripped only when the remainder is a balanced group
/// closing at end-of-string, so a column or expression named `linear` is left alone.
fn strip_linear_prefix(value: &str) -> &str {
    let Some(prefix) = value.get(..6) else {
        return value;
    };
    if !prefix.eq_ignore_ascii_case("linear") {
        return value;
    }
    let remainder = value[6..].trim_start();
    if has_balanced_outer_parens(remainder) {
        remainder
    } else {
        value
    }
}

/// Split on commas that are not nested inside parentheses or a quoted span, so a clustering
/// expression like `coalesce(a, b)` stays one key instead of becoming two, a quoted identifier
/// like `"a,b"` isn't split on its embedded comma, and neither is a string literal like
/// `to_char(ts, 'A,B')`.
fn split_keys(value: &str) -> Vec<&str> {
    let mut keys = Vec::new();
    let mut depth = 0usize;
    let mut quote_char: Option<char> = None;
    let mut start = 0usize;
    let chars: Vec<(usize, char)> = value.char_indices().collect();
    let mut idx = 0;
    while idx < chars.len() {
        let (i, c) = chars[idx];
        if let Some(q) = quote_char {
            if c == q {
                if chars.get(idx + 1).map(|&(_, next)| next) == Some(q) {
                    idx += 1; // doubled escape for a literal quote
                } else {
                    quote_char = None;
                }
            }
        } else {
            match c {
                '"' | '\'' => quote_char = Some(c),
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    keys.push(&value[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        idx += 1;
    }
    keys.push(&value[start..]);
    keys
}

/// Fold everything outside a quoted span. A double-quoted identifier (`"MixedCase"`) or a
/// single-quoted string literal (e.g. the date-format argument in `to_char(ts, 'MON')`) is
/// case-SENSITIVE data in Snowflake and is kept exactly as written, delimiters included;
/// everything else is folded, matching how Snowflake folds an unquoted identifier.
fn normalize_key(key: &str) -> String {
    let mut result = String::with_capacity(key.len());
    let mut quote_char: Option<char> = None;
    let chars: Vec<char> = key.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote_char {
            result.push(c);
            if c == q {
                if chars.get(i + 1) == Some(&q) {
                    result.push(chars[i + 1]);
                    i += 1; // doubled escape, stay inside the quoted span
                } else {
                    quote_char = None;
                }
            }
        } else if c == '"' || c == '\'' {
            quote_char = Some(c);
            result.push(c);
        } else {
            result.push(c.to_ascii_lowercase());
        }
        i += 1;
    }
    result
}

/// Reduce a clustering definition to its ordered list of keys.
///
/// Ordered, not a set: `(id, val)` and `(val, id)` are different clusterings.
fn cluster_keys(value: &str) -> Vec<String> {
    let value = strip_linear_prefix(value.trim());
    split_keys(strip_outer_parens(value))
        .into_iter()
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(normalize_key)
        .collect()
}

/// `SHOW` reports `cluster_by` parenthesized and in Snowflake's own casing: a table
/// configured with `["id", "val"]` is reported as `(id, val)`. Comparing the raw strings
/// therefore reports a change on every run — and because a `cluster_by` change forces a full
/// refresh on interactive tables, that means rebuilding the whole table on a no-op run.
fn diff_cluster_by(desired: &Option<String>, current: &Option<String>) -> Option<Option<String>> {
    diff::optional_by(desired, current, |desired, current| {
        cluster_keys(desired) == cluster_keys(current)
    })
}

fn new_component(cluster_by: Option<String>) -> ClusterBy {
    ClusterBy {
        type_name: TYPE_NAME,
        diff_fn: diff_cluster_by,
        to_jinja_fn: to_jinja,
        value: cluster_by,
    }
}

fn from_remote_state(results: &SnowflakeDescribeResults) -> AdapterResult<ClusterBy> {
    let batch = &results.record_batch;
    let cluster_by = match get_string_by_name_from_record_batch(batch, "cluster_by") {
        Ok(s) if !s.is_empty() && !s.eq_ignore_ascii_case("NONE") => Some(s),
        _ => None,
    };
    Ok(new_component(cluster_by))
}

fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> AdapterResult<ClusterBy> {
    let snowflake_config = relation_config
        .as_any()
        .downcast_ref::<DbtModel>()
        .ok_or_else(|| {
            AdapterError::new(
                dbt_common::AdapterErrorKind::UnexpectedResult,
                "relation config needs to be a model",
            )
        })?
        .__adapter_attr__
        .snowflake_attr
        .as_ref()
        .ok_or_else(|| {
            AdapterError::new(
                dbt_common::AdapterErrorKind::Configuration,
                "relation config needs to be Snowflake model",
            )
        })?;
    let cluster_by = snowflake_config
        .cluster_by
        .as_ref()
        .map(|c| c.fields().join(", "));
    Ok(new_component(cluster_by))
}

impl_loader!(ClusterBy, SnowflakeDescribeResults);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::snowflake::config::test_helpers;

    #[test]
    fn from_remote_state_none() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            cluster_by: None,
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_remote_state_some_string() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            cluster_by: Some(dbt_schemas::schemas::common::ClusterConfig::String(
                "id".to_owned(),
            )),
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_some());
        assert_eq!(loaded.value.unwrap(), "id")
    }

    #[test]
    fn from_remote_state_some_list() {
        let remote_state = test_helpers::make_remote_config(test_helpers::TestDynamicTableConfig {
            cluster_by: Some(dbt_schemas::schemas::common::ClusterConfig::List(vec![
                "id".to_owned(),
                "id2".to_owned(),
            ])),
            ..Default::default()
        });
        let loaded = from_remote_state(&remote_state).unwrap();
        assert!(loaded.value.is_some());
        assert_eq!(loaded.value.unwrap(), "id, id2")
    }

    #[test]
    fn cluster_keys_strips_the_readback_parens() {
        assert_eq!(cluster_keys("(id)"), vec!["id"]);
        assert_eq!(cluster_keys("(id, val)"), vec!["id", "val"]);
        // The local side is unparenthesized; both must reduce to the same keys.
        assert_eq!(cluster_keys("id, val"), cluster_keys("(id, val)"));
    }

    #[test]
    fn cluster_keys_is_case_insensitive_and_whitespace_insensitive() {
        assert_eq!(cluster_keys("(ID,VAL)"), cluster_keys("id, val"));
        assert_eq!(cluster_keys("  ( id ,  val )  "), cluster_keys("id, val"));
    }

    #[test]
    fn cluster_keys_keeps_expressions_with_commas_intact() {
        // A comma inside a function call is not a key separator.
        assert_eq!(
            cluster_keys("(coalesce(a, b), id)"),
            vec!["coalesce(a, b)", "id"]
        );
        assert_eq!(
            cluster_keys("coalesce(a, b), id"),
            cluster_keys("(coalesce(a, b), id)")
        );
    }

    #[test]
    fn cluster_keys_leaves_a_bare_expression_unwrapped() {
        // `to_date(ts)` is one expression, not a parenthesized key list — stripping its
        // parens would silently turn it into a different key.
        assert_eq!(cluster_keys("to_date(ts)"), vec!["to_date(ts)"]);
    }

    #[test]
    fn cluster_keys_is_order_sensitive() {
        // Clustering key order is part of the definition, so this IS a real change.
        assert_ne!(cluster_keys("(id, val)"), cluster_keys("(val, id)"));
        assert!(
            diff_cluster_by(&Some("val, id".to_owned()), &Some("(id, val)".to_owned())).is_some()
        );
    }

    #[test]
    fn diff_normalized_readback_is_no_change() {
        assert!(
            diff_cluster_by(&Some("id, val".to_owned()), &Some("(id, val)".to_owned())).is_none()
        );
        assert!(diff_cluster_by(&None, &None).is_none());
    }

    #[test]
    fn diff_genuine_change_carries_the_desired_value() {
        assert_eq!(
            diff_cluster_by(&Some("id, other".to_owned()), &Some("(id, val)".to_owned())),
            Some(Some("id, other".to_owned()))
        );
        assert_eq!(diff_cluster_by(&None, &Some("(id)".to_owned())), Some(None));
    }

    #[test]
    fn from_local_state_none() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            cluster_by: None,
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_none());
    }

    #[test]
    fn from_local_state_some_list() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            cluster_by: Some(dbt_schemas::schemas::common::ClusterConfig::List(vec![
                "id".to_owned(),
                "id2".to_owned(),
            ])),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_some());
        assert_eq!(loaded.value.unwrap(), "id, id2")
    }

    #[test]
    fn from_local_state_some_string() {
        let local_state = test_helpers::make_local_config(test_helpers::TestDynamicTableConfig {
            cluster_by: Some(dbt_schemas::schemas::common::ClusterConfig::String(
                "id".to_owned(),
            )),
            ..Default::default()
        });
        let loaded = from_local_config(&local_state).unwrap();
        assert!(loaded.value.is_some());
        assert_eq!(loaded.value.unwrap(), "id")
    }

    // --- LINEAR prefix ---

    #[test]
    fn cluster_keys_strips_linear_prefix() {
        assert_eq!(cluster_keys("LINEAR(id, val)"), cluster_keys("(id, val)"));
    }

    #[test]
    fn cluster_keys_linear_prefix_is_case_insensitive() {
        assert_eq!(
            cluster_keys("linear(id, val)"),
            cluster_keys("LINEAR(id, val)")
        );
    }

    #[test]
    fn cluster_keys_linear_named_column_alone_is_untouched() {
        assert_eq!(cluster_keys("linear"), vec!["linear"]);
    }

    #[test]
    fn cluster_keys_parenthesized_linear_named_column_still_unwraps() {
        assert_eq!(cluster_keys("(linear)"), vec!["linear"]);
    }

    #[test]
    fn cluster_keys_linear_function_call_as_one_of_several_keys_is_untouched() {
        // `linear(a)` here is a clustering EXPRESSION (a call to a function named `linear`),
        // not Snowflake's wrapper -- its leading paren group doesn't close at the final
        // character because `, b` follows it.
        assert_eq!(cluster_keys("linear(a), b"), vec!["linear(a)", "b"]);
    }

    #[test]
    fn diff_linear_wrapped_readback_is_not_a_change() {
        assert!(
            diff_cluster_by(
                &Some("id, val".to_owned()),
                &Some("LINEAR(id, val)".to_owned())
            )
            .is_none()
        );
    }

    // --- Quote-awareness ---

    #[test]
    fn cluster_keys_preserves_case_inside_quotes() {
        assert_eq!(
            cluster_keys(r#"("MixedCase")"#),
            vec![r#""MixedCase""#.to_owned()]
        );
    }

    #[test]
    fn cluster_keys_lowercases_outside_quotes() {
        assert_eq!(cluster_keys("(ID, VAL)"), cluster_keys("id, val"));
    }

    #[test]
    fn cluster_keys_quoted_comma_is_not_a_key_separator() {
        assert_eq!(cluster_keys(r#"("a,b")"#), vec![r#""a,b""#.to_owned()]);
    }

    #[test]
    fn cluster_keys_quoted_paren_does_not_confuse_outer_paren_balance() {
        assert_eq!(cluster_keys(r#"("c(d)")"#), vec![r#""c(d)""#.to_owned()]);
    }

    #[test]
    fn cluster_keys_quoted_multi_key_list() {
        assert_eq!(
            cluster_keys(r#"("MixedCase", id)"#),
            vec![r#""MixedCase""#.to_owned(), "id".to_owned()]
        );
    }

    #[test]
    fn diff_quoted_mixed_case_change_is_a_genuine_change() {
        // Quoted identifiers are case-SENSITIVE in Snowflake: `"MixedCase"` and `"mixedcase"`
        // are different columns, so this must be detected as a real change.
        assert_eq!(
            diff_cluster_by(
                &Some(r#""mixedcase""#.to_owned()),
                &Some(r#"("MixedCase")"#.to_owned())
            ),
            Some(Some(r#""mixedcase""#.to_owned()))
        );
    }

    #[test]
    fn diff_quoted_same_case_readback_is_not_a_change() {
        assert!(
            diff_cluster_by(
                &Some(r#""MixedCase""#.to_owned()),
                &Some(r#"("MixedCase")"#.to_owned())
            )
            .is_none()
        );
    }

    // --- Quote-awareness: single-quoted string literals inside expressions ---
    // A clustering expression like `to_char(ts, 'MON')` can carry a single-quoted string
    // literal as a function argument. Live-verified against Snowflake (account ktb38830,
    // 2026-09-02): `SHOW` echoes such a literal back verbatim, including exact case, so it
    // must be treated as case-SENSITIVE data, not folded like the surrounding expression text.

    #[test]
    fn case_is_preserved_inside_single_quotes() {
        assert_eq!(
            cluster_keys("TO_CHAR(TS, 'MON')"),
            vec!["to_char(ts, 'MON')"]
        );
    }

    #[test]
    fn diff_quoted_literal_case_change_is_a_genuine_change() {
        assert!(
            diff_cluster_by(
                &Some("to_char(ts, 'MON')".to_owned()),
                &Some("to_char(ts, 'mon')".to_owned())
            )
            .is_some()
        );
    }

    #[test]
    fn diff_quoted_literal_same_case_readback_is_not_a_change() {
        assert!(
            diff_cluster_by(
                &Some("to_char(ts, 'MON')".to_owned()),
                &Some("(to_char(ts, 'MON'))".to_owned())
            )
            .is_none()
        );
    }

    #[test]
    fn comma_inside_single_quoted_literal_is_not_a_key_separator() {
        assert_eq!(
            cluster_keys("(to_char(ts, 'A,B'))"),
            vec!["to_char(ts, 'A,B')"]
        );
    }

    #[test]
    fn paren_and_comma_inside_single_quoted_literal_do_not_corrupt_parsing() {
        // Live-verified against Snowflake (ktb38830, 2026-09-02): `to_char(ts, 'A)B,C')` is
        // valid DDL, and a dynamic table's `SHOW` echoes it back verbatim inside a LINEAR
        // wrapper -- the embedded `)` and `,` must not desync paren depth or split a key
        // that was never meant to split.
        assert_eq!(
            cluster_keys("a, to_char(ts, 'A)B,C')"),
            cluster_keys("LINEAR(a, to_char(ts, 'A)B,C'))")
        );
    }

    #[test]
    fn doubled_single_quote_escape_stays_inside_the_literal() {
        // `''` inside a string literal is an escaped literal quote, not the end of the
        // literal -- live-verified against Snowflake (ktb38830, 2026-09-02).
        assert_eq!(
            cluster_keys("to_char(ts, 'YYYY''MON')"),
            vec!["to_char(ts, 'YYYY''MON')"]
        );
    }

    #[test]
    fn quoted_identifier_and_quoted_literal_can_coexist_in_one_key() {
        assert_eq!(
            cluster_keys(r#"to_char("Col", 'MON')"#),
            vec![r#"to_char("Col", 'MON')"#]
        );
    }
}
