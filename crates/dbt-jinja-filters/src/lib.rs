//! dbt specific Jinja filters.
//!
//! Also provides [`register_filter_types`] for static type-checking integration.

use minijinja::{
    Environment, Error as MinijinjaError, ErrorKind as MinijinjaErrorKind, Value, value::ValueKind,
};

/// Register all four dbt filters on the given [`Environment`].
pub fn register_filters(env: &mut Environment<'_>) {
    env.add_filter("as_native", as_native);
    env.add_filter("as_text", as_text);
    env.add_filter("as_bool", as_bool);
    env.add_filter("as_number", as_number);
}

/// Pass-through filter — returns the value unchanged.
pub fn as_native(value: Value) -> Result<Value, MinijinjaError> {
    Ok(value)
}

/// Convert a value to its text representation.
pub fn as_text(value: Value) -> Result<Value, MinijinjaError> {
    match value.kind() {
        ValueKind::Bool | ValueKind::Number => Ok(Value::from(format!("{value}"))),
        ValueKind::String => Ok(value),
        ValueKind::None => Ok(Value::from("")),
        _ => {
            if let Some(object) = value.as_object() {
                let debug = format!("{object:?}");
                Ok(Value::from(debug))
            } else {
                Err(MinijinjaError::new(
                    MinijinjaErrorKind::InvalidOperation,
                    format!("Failed applying 'as_text' filter to {}", value.kind()),
                ))
            }
        }
    }
}

/// Pass-through filter — returns the value unchanged.
///
/// dbt only coerces to a boolean while rendering *native* values; when rendering
/// text (SQL, macros) `as_bool` is the identity filter, so `{{ "true" | as_bool }}`
/// renders `true`, not `True`. Bool configs such as `enabled` accept the
/// resulting bool-like string when they are deserialized.
pub fn as_bool(value: Value) -> Result<Value, MinijinjaError> {
    Ok(value)
}

/// Convert a value to a number (i32).
pub fn as_number(value: Value) -> Result<Value, MinijinjaError> {
    match value.kind() {
        ValueKind::Undefined => Ok(Value::UNDEFINED),
        ValueKind::None => Ok(Value::from(0)),
        ValueKind::Bool | ValueKind::Number => Ok(value),
        ValueKind::String => {
            let str_ref = value.as_str().unwrap();
            match str_ref.parse::<i32>() {
                Ok(num) => Ok(Value::from(num)),
                Err(_) => Ok(value),
            }
        }
        ValueKind::Bytes => {
            let bytes_ref = value.as_bytes().unwrap();
            let string_from_bytes = String::from_utf8(bytes_ref.to_vec()).map_err(|_| {
                MinijinjaError::new(
                    MinijinjaErrorKind::InvalidOperation,
                    "Failed applying 'as_number' filter to bytes",
                )
            })?;
            let num = string_from_bytes.parse::<i32>().map_err(|_| {
                MinijinjaError::new(
                    MinijinjaErrorKind::InvalidOperation,
                    format!(
                        "Failed applying 'as_number' filter to bytes string '{string_from_bytes}'"
                    ),
                )
            })?;
            Ok(Value::from(num))
        }
        ValueKind::Seq | ValueKind::Map | ValueKind::Iterable | ValueKind::Plain => {
            Err(MinijinjaError::new(
                MinijinjaErrorKind::InvalidOperation,
                "Failed applying 'as_number' filter to map, seq, or iterable",
            ))
        }
        ValueKind::Invalid => Err(MinijinjaError::new(
            MinijinjaErrorKind::InvalidOperation,
            "Failed applying 'as_number' filter to invalid value",
        )),
        _ => Err(MinijinjaError::new(
            MinijinjaErrorKind::InvalidOperation,
            "Failed applying 'as_number' filter to unknown value",
        )),
    }
}

// ── Typecheck support ────────────────────────────────────────────────

use minijinja::{
    AsBoolFilterType, AsNativeFilterType, AsNumberFilterType, AsTextFilterType, DynTypeObject,
    compiler::typecheck::FunctionRegistry,
};
use std::sync::Arc;

/// Register filter types into the given [`FunctionRegistry`] for static type-checking.
pub fn register_filter_types(registry: &mut FunctionRegistry) {
    registry.insert(
        "as_bool".to_string(),
        DynTypeObject::new(Arc::new(AsBoolFilterType)),
    );
    registry.insert(
        "as_number".to_string(),
        DynTypeObject::new(Arc::new(AsNumberFilterType)),
    );
    registry.insert(
        "as_text".to_string(),
        DynTypeObject::new(Arc::new(AsTextFilterType)),
    );
    registry.insert(
        "as_native".to_string(),
        DynTypeObject::new(Arc::new(AsNativeFilterType)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::Environment;

    fn render(template: &str) -> String {
        let mut env = Environment::new();
        register_filters(&mut env);
        env.render_str(template, (), &[]).unwrap()
    }

    fn render_err(template: &str) -> String {
        let mut env = Environment::new();
        register_filters(&mut env);
        env.render_str(template, (), &[])
            .unwrap_err()
            .detail()
            .unwrap_or_default()
            .to_string()
    }

    // ── as_native ────────────────────────────────────────────────────

    #[test]
    fn as_native_string() {
        assert_eq!(render(r#"{{ "hello" | as_native }}"#), "hello");
    }

    #[test]
    fn as_native_number() {
        assert_eq!(render(r#"{{ 42 | as_native }}"#), "42");
    }

    #[test]
    fn as_native_bool() {
        assert_eq!(render(r#"{{ true | as_native }}"#), "True");
    }

    // ── as_text ──────────────────────────────────────────────────────

    #[test]
    fn as_text_string() {
        assert_eq!(render(r#"{{ "hello" | as_text }}"#), "hello");
    }

    #[test]
    fn as_text_number() {
        assert_eq!(render(r#"{{ 42 | as_text }}"#), "42");
    }

    #[test]
    fn as_text_bool() {
        assert_eq!(render(r#"{{ true | as_text }}"#), "True");
    }

    #[test]
    fn as_text_none() {
        assert_eq!(render(r#"{{ none | as_text }}"#), "");
    }

    // ── as_bool ──────────────────────────────────────────────────────

    // Bool-like strings pass through unchanged — dbt only converts them when
    // rendering native values, not when rendering text.
    #[test]
    fn as_bool_true_string() {
        assert_eq!(render(r#"{{ "true" | as_bool }}"#), "true");
        assert_eq!(render(r#"{{ "True" | as_bool }}"#), "True");
    }

    #[test]
    fn as_bool_false_string() {
        assert_eq!(render(r#"{{ "false" | as_bool }}"#), "false");
        assert_eq!(render(r#"{{ "False" | as_bool }}"#), "False");
    }

    #[test]
    fn as_bool_non_bool_string() {
        assert_eq!(render(r#"{{ "hello" | as_bool }}"#), "hello");
    }

    #[test]
    fn as_bool_none() {
        assert_eq!(render(r#"{{ none | as_bool }}"#), "None");
    }

    #[test]
    fn as_bool_number() {
        assert_eq!(render(r#"{{ 1 | as_bool }}"#), "1");
        assert_eq!(render(r#"{{ 0 | as_bool }}"#), "0");
    }

    #[test]
    fn as_bool_bool() {
        assert_eq!(render(r#"{{ true | as_bool }}"#), "True");
        assert_eq!(render(r#"{{ false | as_bool }}"#), "False");
    }

    #[test]
    fn as_bool_list() {
        assert_eq!(render(r#"{{ [1, 2] | as_bool }}"#), "[1, 2]");
        assert_eq!(render(r#"{{ [] | as_bool }}"#), "[]");
    }

    // ── as_number ────────────────────────────────────────────────────

    #[test]
    fn as_number_string_int() {
        assert_eq!(render(r#"{{ "42" | as_number }}"#), "42");
    }

    #[test]
    fn as_number_string_non_numeric() {
        // Non-numeric strings pass through unchanged
        assert_eq!(render(r#"{{ "hello" | as_number }}"#), "hello");
    }

    #[test]
    fn as_number_none() {
        assert_eq!(render(r#"{{ none | as_number }}"#), "0");
    }

    #[test]
    fn as_number_bool() {
        assert_eq!(render(r#"{{ true | as_number }}"#), "True");
    }

    #[test]
    fn as_number_number() {
        assert_eq!(render(r#"{{ 99 | as_number }}"#), "99");
    }

    #[test]
    fn as_number_list_error() {
        let msg = render_err(r#"{{ [1] | as_number }}"#);
        assert!(msg.contains("as_number"), "expected as_number error: {msg}");
    }

    // ── chaining ─────────────────────────────────────────────────────

    #[test]
    fn chain_as_text_as_number() {
        assert_eq!(render(r#"{{ 42 | as_text | as_number }}"#), "42");
    }

    #[test]
    fn chain_as_number_as_text() {
        assert_eq!(render(r#"{{ "99" | as_number | as_text }}"#), "99");
    }

    #[test]
    fn chain_as_bool_as_text() {
        assert_eq!(render(r#"{{ "true" | as_bool | as_text }}"#), "true");
    }
}
