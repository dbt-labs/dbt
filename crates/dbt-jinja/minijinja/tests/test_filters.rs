#![cfg(feature = "builtins")]
use minijinja::value::Value;
use minijinja::{args, Environment};
use similar_asserts::assert_eq;

use minijinja::filters::{abs, indent};

#[test]
fn test_filter_with_non() {
    fn filter(value: Option<String>) -> String {
        format!("[{}]", value.unwrap_or_default())
    }

    let mut env = Environment::new();
    env.add_filter("filter", filter);
    let state = env.empty_state();

    let rv = state
        .apply_filter("filter", args!(Value::UNDEFINED))
        .unwrap();
    assert_eq!(rv, Value::from("[]"));

    let rv = state
        .apply_filter("filter", args!(Value::from(())))
        .unwrap();
    assert_eq!(rv, Value::from("[]"));

    let rv = state
        .apply_filter("filter", args!(Value::from("wat")))
        .unwrap();
    assert_eq!(rv, Value::from("[wat]"));
}

#[test]
fn test_indent_one_empty_line() {
    let teststring = String::from("\n");
    let args = vec![Value::from(teststring), Value::from(2)];
    assert_eq!(indent(&args).unwrap(), String::from(""));
}

#[test]
fn test_indent_one_line() {
    let teststring = String::from("test\n");
    let args = vec![Value::from(teststring), Value::from(2)];
    assert_eq!(indent(&args).unwrap(), String::from("test"));
}

#[test]
fn test_indent() {
    let teststring = String::from("test\ntest1\n\ntest2\n");
    let args = vec![Value::from(teststring), Value::from(2)];
    assert_eq!(
        indent(&args).unwrap(),
        String::from("test\n  test1\n\n  test2")
    );
}

#[test]
fn test_indent_first_line() {
    let teststring = String::from("test\ntest1\n\ntest2\n");
    let args = args!(teststring, 2, first => true);
    assert_eq!(
        indent(args).unwrap(),
        String::from("  test\n  test1\n\n  test2")
    );
}

#[test]
fn test_indent_blank_line() {
    let teststring = String::from("test\ntest1\n\ntest2\n");
    let args = args!(teststring, 2, blank => true);
    assert_eq!(
        indent(args).unwrap(),
        String::from("test\n  test1\n  \n  test2")
    );
}

#[test]
fn test_indent_all_lines() {
    let teststring = String::from("test\ntest1\n\ntest2\n");
    let args = args!(teststring, 2, first => true, blank => true);
    assert_eq!(
        indent(args).unwrap(),
        String::from("  test\n  test1\n  \n  test2")
    );
}

#[test]
fn test_abs_overflow() {
    let ok = abs(Value::from(i64::MIN)).unwrap();
    assert_eq!(ok, Value::from(-(i64::MIN as i128)));
    let err = abs(Value::from(i128::MIN)).unwrap_err();
    assert_eq!(err.to_string(), "invalid operation: overflow on abs");
}

// `first` parity with Python Jinja2: undefined input returns undefined,
// non-iterable input raises a TypeError of the form
// "'<typename>' object is not iterable". The SLT covers happy paths and
// undefined; error paths live here because the SLT runner's dbt-jinja
// executor wraps the same TypeError in a different envelope than Mantle.

fn first_detail(env: &Environment, expr: &str) -> String {
    let err = env.render_str(expr, (), &[]).unwrap_err();
    err.detail().unwrap_or_default().to_string()
}

#[test]
fn test_first_undefined_passes_through() {
    let env = Environment::new();
    let rv = env
        .render_str("{{ undefined_var | first }}", (), &[])
        .unwrap();
    assert_eq!(rv, "");
}

#[test]
fn test_first_none_errors_like_python() {
    let env = Environment::new();
    assert_eq!(
        first_detail(&env, "{{ none | first }}"),
        "'NoneType' object is not iterable"
    );
}

#[test]
fn test_first_int_errors_like_python() {
    let env = Environment::new();
    assert_eq!(
        first_detail(&env, "{{ 42 | first }}"),
        "'int' object is not iterable"
    );
}

#[test]
fn test_first_float_errors_like_python() {
    let env = Environment::new();
    assert_eq!(
        first_detail(&env, "{{ 3.14 | first }}"),
        "'float' object is not iterable"
    );
}

#[test]
fn test_first_bool_errors_like_python() {
    let env = Environment::new();
    assert_eq!(
        first_detail(&env, "{{ true | first }}"),
        "'bool' object is not iterable"
    );
}

// `last` parity with Python Jinja2: same shape as `first` for undefined and
// non-iterable inputs, except CPython's `last` raises
// "'<typename>' object is not reversible" (since Jinja2's `last` calls
// `reversed(...)` under the hood).

fn last_detail(env: &Environment, expr: &str) -> String {
    let err = env.render_str(expr, (), &[]).unwrap_err();
    err.detail().unwrap_or_default().to_string()
}

#[test]
fn test_last_undefined_passes_through() {
    let env = Environment::new();
    let rv = env
        .render_str("{{ undefined_var | last }}", (), &[])
        .unwrap();
    assert_eq!(rv, "");
}

#[test]
fn test_last_dict_yields_last_key() {
    let env = Environment::new();
    let rv = env
        .render_str(r#"{{ {"a": 1, "b": 2, "c": 3} | last }}"#, (), &[])
        .unwrap();
    assert_eq!(rv, "c");
}

#[test]
fn test_last_empty_dict_returns_undefined() {
    let env = Environment::new();
    let rv = env.render_str("{{ {} | last }}", (), &[]).unwrap();
    assert_eq!(rv, "");
}

#[test]
fn test_last_none_errors_like_python() {
    let env = Environment::new();
    assert_eq!(
        last_detail(&env, "{{ none | last }}"),
        "'NoneType' object is not reversible"
    );
}

#[test]
fn test_last_int_errors_like_python() {
    let env = Environment::new();
    assert_eq!(
        last_detail(&env, "{{ 42 | last }}"),
        "'int' object is not reversible"
    );
}

#[test]
fn test_last_float_errors_like_python() {
    let env = Environment::new();
    assert_eq!(
        last_detail(&env, "{{ 3.14 | last }}"),
        "'float' object is not reversible"
    );
}

#[test]
fn test_last_bool_errors_like_python() {
    let env = Environment::new();
    assert_eq!(
        last_detail(&env, "{{ true | last }}"),
        "'bool' object is not reversible"
    );
}
