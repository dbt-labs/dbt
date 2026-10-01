//! Formatting helpers for generated Jinja macro arguments.

use std::{collections::BTreeMap, sync::LazyLock};

use regex::Regex;
use serde_json::Value as JsonValue;

/// Formats a JSON value as a Jinja macro argument literal.
pub fn format_value_for_jinja(
    value: &JsonValue,
    jinja_set_vars: &BTreeMap<String, String>,
) -> String {
    match value {
        JsonValue::String(s) => {
            // Strings shaped like a single bare call to one of dbt-Core's renderable
            // functions (`env_var(...)`, `ref(...)`, `var(...)`, `source(...)`, `doc(...)`)
            // are emitted unquoted so Jinja evaluates them when the generated test SQL is
            // rendered. Core does the same in `add_rendered_test_kwargs` (clients/jinja.py)
            // by re-wrapping such values in `{{ }}` before native rendering.
            if s.starts_with("get_where_subquery(")
                || LOOKS_LIKE_FUNC.is_match(s)
                || jinja_set_vars.iter().any(|(var_name, _)| var_name == s)
            {
                s.to_string()
            } else if s.starts_with("{{") && s.ends_with("}}") {
                s[2..s.len() - 2].trim().to_string()
            } else {
                let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{escaped}\"")
            }
        }
        JsonValue::Array(arr) => {
            let formatted_elements: Vec<String> = arr
                .iter()
                .map(|elem| format_value_for_jinja(elem, jinja_set_vars))
                .collect();
            format!("[{}]", formatted_elements.join(","))
        }
        JsonValue::Object(obj) => {
            let mut keys: Vec<&String> = obj.keys().collect();
            keys.sort();
            let formatted_pairs: Vec<String> = keys
                .iter()
                .map(|k| {
                    let formatted_val = format_value_for_jinja(&obj[*k], jinja_set_vars);
                    format!("\"{k}\":{formatted_val}")
                })
                .collect();
            format!("{{{}}}", formatted_pairs.join(","))
        }
        _ => json_to_jinja_literal(value),
    }
}

/// Matches strings that are a single bare call to one of dbt-Core's whitelisted
/// renderable test-arg functions (e.g. `var('foo')`, `env_var('FOO', 'default')`).
/// Mirrors `looks_like_func` in dbt-core/clients/jinja.py - used by
/// `add_rendered_test_kwargs` to decide which test-arg strings get wrapped in
/// `{{ }}` and rendered through the native Jinja env. The end-of-string anchor
/// is significant: shapes like `var('x') ~ 'y'` that have content after the
/// closing paren are intentionally excluded so we don't diverge from Core by
/// accepting expressions Core rejects.
static LOOKS_LIKE_FUNC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(env_var|ref|var|source|doc)\s*\(.+\)\s*$").expect("valid regex")
});

fn json_to_jinja_literal(v: &JsonValue) -> String {
    match v {
        JsonValue::Null => "none".to_string(),
        JsonValue::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        JsonValue::Number(n) => n.to_string(),
        JsonValue::String(s) => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string()),
        JsonValue::Array(arr) => {
            let mut out = String::from("[");
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&json_to_jinja_literal(item));
            }
            out.push(']');
            out
        }
        JsonValue::Object(map) => {
            // Deterministic ordering for stable SQL/tests
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = String::from("{");
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let key = serde_json::to_string(k).unwrap_or_else(|_| "\"\"".to_string());
                out.push_str(&key);
                out.push(':');
                out.push_str(&json_to_jinja_literal(&map[*k]));
            }
            out.push('}');
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_value_for_jinja_strings() {
        let vars = BTreeMap::new();

        assert_eq!(
            format_value_for_jinja(&serde_json::json!("plain"), &vars),
            "\"plain\""
        );
        assert_eq!(
            format_value_for_jinja(&serde_json::json!("ref('orders')"), &vars),
            "ref('orders')"
        );
        assert_eq!(
            format_value_for_jinja(&serde_json::json!("source('raw', 'orders')"), &vars),
            "source('raw', 'orders')"
        );
        assert_eq!(
            format_value_for_jinja(&serde_json::json!("var('threshold')"), &vars),
            "var('threshold')"
        );
        assert_eq!(
            format_value_for_jinja(&serde_json::json!("env_var('DBT_ENV')"), &vars),
            "env_var('DBT_ENV')"
        );
        assert_eq!(
            format_value_for_jinja(&serde_json::json!("doc('orders')"), &vars),
            "doc('orders')"
        );
        assert_eq!(
            format_value_for_jinja(
                &serde_json::json!("get_where_subquery(ref('orders'))"),
                &vars
            ),
            "get_where_subquery(ref('orders'))"
        );
        assert_eq!(
            format_value_for_jinja(&serde_json::json!("{{ var('threshold') }}"), &vars),
            "var('threshold')"
        );
    }

    #[test]
    fn test_format_value_for_jinja_arrays_and_objects() {
        let vars = BTreeMap::new();

        assert_eq!(
            format_value_for_jinja(&serde_json::json!(["x", null]), &vars),
            "[\"x\",none]"
        );
        assert_eq!(
            format_value_for_jinja(
                &serde_json::json!({
                    "b": 2,
                    "a": "ref('orders')",
                }),
                &vars
            ),
            "{\"a\":ref('orders'),\"b\":2}"
        );
    }

    #[test]
    fn test_format_value_for_jinja_jinja_set_vars() {
        let mut vars = BTreeMap::new();
        vars.insert(
            "dbt_custom_arg_values_0".to_string(),
            "{% raw %}{{true}}{% endraw %}".to_string(),
        );

        assert_eq!(
            format_value_for_jinja(
                &serde_json::json!(["dbt_custom_arg_values_0", "regular_string"]),
                &vars
            ),
            "[dbt_custom_arg_values_0,\"regular_string\"]"
        );
    }
}
