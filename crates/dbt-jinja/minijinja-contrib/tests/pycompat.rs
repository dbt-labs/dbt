#![cfg(feature = "pycompat")]

use minijinja::{context, Environment, Value};
use minijinja_contrib::pycompat::unknown_method_callback;
use similar_asserts::assert_eq;

fn eval_expr(expr: &str) -> Value {
    let mut env = Environment::new();
    env.set_unknown_method_callback(unknown_method_callback);
    env.compile_expression(expr).unwrap().eval((), &[]).unwrap()
}

#[test]
#[allow(clippy::cognitive_complexity)]
fn test_string_methods() {
    assert_eq!(eval_expr("'foo'.upper()").as_str(), Some("FOO"));
    assert_eq!(eval_expr("'FoO'.lower()").as_str(), Some("foo"));
    assert_eq!(eval_expr("' foo '.strip()").as_str(), Some("foo"));
    assert_eq!(eval_expr("'!foo?!!!'.strip('!?')").as_str(), Some("foo"));
    assert_eq!(
        eval_expr("'!!!foo?!!!'.rstrip('!?')").as_str(),
        Some("!!!foo")
    );
    assert_eq!(
        eval_expr("'!!!foo?!!!'.lstrip('!?')").as_str(),
        Some("foo?!!!")
    );
    assert!(eval_expr("'foobar'.islower()").is_true());
    assert!(eval_expr("'FOOBAR'.isupper()").is_true());
    assert!(eval_expr("' \\n'.isspace()").is_true());
    assert!(eval_expr("'abc'.isalpha()").is_true());
    assert!(eval_expr("'abc123'.isalnum()").is_true());
    assert!(eval_expr("'abc%@#'.isascii()").is_true());
    assert_eq!(
        eval_expr("'foobar'.replace('o', 'x')").as_str(),
        Some("fxxbar")
    );
    assert_eq!(
        eval_expr("'foobar'.replace('o', 'x', 1)").as_str(),
        Some("fxobar")
    );
    assert_eq!(eval_expr("'foo bar'.title()").as_str(), Some("Foo Bar"));
    assert_eq!(
        eval_expr("'foo bar'.capitalize()").as_str(),
        Some("Foo bar")
    );
    assert_eq!(eval_expr("'foo barooo'.count('oo')").as_usize(), Some(2));
    assert_eq!(eval_expr("'foo barooo'.find('oo')").as_usize(), Some(1));
    assert_eq!(eval_expr("'foo barooo'.rfind('oo')").as_usize(), Some(8));
    assert!(eval_expr("'a b c'.split() == ['a', 'b', 'c']").is_true());
    assert!(eval_expr("'a  b  c'.split() == ['a', 'b', 'c']").is_true());
    assert!(eval_expr("'a  b  c'.split(none, 1) == ['a', 'b  c']").is_true());
    assert!(eval_expr("'abcbd'.split('b', 1) == ['a', 'cbd']").is_true());
    assert!(eval_expr("'a\\nb\\r\\nc'.splitlines() == ['a', 'b', 'c']").is_true());
    assert!(eval_expr("'a\\nb\\r\\nc'.splitlines(true) == ['a\\n', 'b\\r\\n', 'c']").is_true());
    assert!(eval_expr("'foobarbaz'.startswith('foo')").is_true());
    assert!(eval_expr("'foobarbaz'.startswith(('foo', 'bar'))").is_true());
    assert!(!eval_expr("'barfoobaz'.startswith(('foo', 'baz'))").is_true());
    assert!(eval_expr("'foobarbaz'.endswith('baz')").is_true());
    assert!(eval_expr("'foobarbaz'.endswith(('baz', 'bar'))").is_true());
    assert!(!eval_expr("'foobarbazblah'.endswith(('baz', 'bar'))").is_true());
    assert_eq!(eval_expr("'|'.join([1, 2, 3])").as_str(), Some("1|2|3"));
    assert_eq!(
        eval_expr("'My name is {fname}, I\\'m {age}'.format({'fname': 'John', 'age': 36})")
            .as_str(),
        Some("My name is John, I'm 36")
    );
    assert_eq!(
        eval_expr("'My name is {0}, I\\'m {1}'.format('John', 36)").as_str(),
        Some("My name is John, I'm 36")
    );
    assert_eq!(
        eval_expr("'My name is {}, I\\'m {}'.format('John', 36)").as_str(),
        Some("My name is John, I'm 36")
    );
    assert_eq!(
        eval_expr("'foo_bar'.removesuffix('_bar')").as_str(),
        Some("foo")
    );
}

#[test]
fn test_dict_methods() {
    assert!(eval_expr("{'x': 42}.keys()|list == ['x']").is_true());
    assert!(eval_expr("{'x': 42}.values()|list == [42]").is_true());
    assert!(eval_expr("{'x': 42}.items()|list == [('x', 42)]").is_true());
    assert!(eval_expr("{'x': 42}.get('x') == 42").is_true());
    assert!(eval_expr("{'x': 42}.get('y') is none").is_true());
    assert!(
        eval_expr("{'meta': {'owner': 'analytics'}}.meta_get('owner') == 'analytics'").is_true()
    );
    assert!(eval_expr("{'meta': {}}.meta_get('owner') is none").is_true());
    assert!(eval_expr("{'meta': {}}.meta_get('owner', 'fallback') == 'fallback'").is_true());
    assert!(
        eval_expr("{'meta': {}}.meta_get('owner', default='fallback') == 'fallback'").is_true()
    );
    assert!(eval_expr("{'meta': {'owner': none}}.meta_get('owner', 'fallback') is none").is_true());
    assert!(eval_expr("{}.meta_get('owner', 'fallback') == 'fallback'").is_true());
    assert!(eval_expr("{'meta': 'nope'}.meta_get('owner', 'fallback') == 'fallback'").is_true());
}

#[test]
fn test_dict_methods_not_shadowed_by_key() {
    // A user key literally named after a dict method must not shadow the method.
    // Python parity: `dict.items` is a bound method on the class, so `d['items']`
    // and `d.items()` are distinct lookups.
    assert!(eval_expr("{'items': 99}.items()|list == [('items', 99)]").is_true());
    assert!(eval_expr("{'keys': 1}.keys()|list == ['keys']").is_true());
    assert!(eval_expr("{'values': 1}.values()|list == [1]").is_true());
    assert!(eval_expr("{'get': 1}.get('get') == 1").is_true());
}

#[test]
fn test_dict_method_bare_attribute_returns_bound_method() {
    // `d.items` (no parens) returns the bound method, NOT the user value at
    // key 'items'. The user value is still reachable via `d['items']`.
    assert!(eval_expr("{'items': 99}.items != 99").is_true());
    assert!(eval_expr("{'items': 99}['items'] == 99").is_true());
    // The bound method is callable: invoking it produces the dict's items.
    assert!(eval_expr("{'items': 99}.items()|list == [('items', 99)]").is_true());
}

#[test]
fn test_undefined_get_returns_default() {
    // Mirrors dbt-common's `Undefined.get(key, default=None)`: dbt macros
    // commonly probe an undefined `config`-like object with
    // `.get('key', default)`, and that must not fail, regardless of the
    // engine's `UndefinedBehavior`.
    let mut env = Environment::new();
    env.set_unknown_method_callback(unknown_method_callback);

    assert_eq!(
        env.render_str(
            "{{ values.get('missing', 'fallback') }}",
            context! { values => Value::UNDEFINED },
            &[],
        )
        .unwrap(),
        "fallback"
    );
    assert_eq!(
        env.render_str(
            "{{ values.get('missing') is undefined }}",
            context! { values => Value::UNDEFINED },
            &[],
        )
        .unwrap(),
        "True"
    );
}

#[test]
fn test_undefined_other_method_calls_fail() {
    // Any method other than `get` on an undefined value must fail, so the
    // engine can name the undefined receiver in the error (see
    // `name_undefined_receiver` in minijinja/src/vm/mod.rs).
    let mut env = Environment::new();
    env.set_unknown_method_callback(unknown_method_callback);

    let err = env
        .render_str(
            "{{ values.whatever(1, 2, foo=3) }}",
            context! { values => Value::UNDEFINED },
            &[],
        )
        .unwrap_err();
    assert_eq!(err.kind(), minijinja::ErrorKind::UnknownMethod);
}

#[test]
fn test_plain_sequence_pop_unsupported() {
    // Unlike `MutableVec` (backing Jinja list literals), a plain `Vec<Value>`
    // Object is immutable, so `pop()` can't remove the element in place.
    // Like Python's tuple, it should not support `pop()` at all.
    let mut env = Environment::new();
    env.set_unknown_method_callback(unknown_method_callback);
    let values = Value::from_object(vec![Value::from(1), Value::from(2), Value::from(3)]);

    assert!(env
        .render_str("{{ values.pop() }}", context! { values => values }, &[])
        .is_err());
}

#[test]
fn test_plain_sequence_iteration() {
    let env = Environment::new();
    let values = Value::from_object(vec![Value::from(1), Value::from(2), Value::from(3)]);

    assert_eq!(
        env.render_str(
            "{{ values | list | join(',') }}",
            context! { values => values },
            &[],
        )
        .unwrap(),
        "1,2,3"
    );
}

#[test]
fn test_list_methods() {
    assert!(eval_expr("[1, 2, 2, 3].count(2) == 2").is_true());

    // Test basic union with two lists
    assert!(eval_expr("[1, 2, 3].union([3, 4, 5]) | sort == [1, 2, 3, 4, 5]").is_true());

    // Test union with duplicates (should be removed)
    assert!(eval_expr("[1, 2, 2].union([2, 3, 3]) | sort == [1, 2, 3]").is_true());

    // Test union with multiple arguments
    assert!(eval_expr("[1, 2].union([3, 4], [5, 6]) | sort == [1, 2, 3, 4, 5, 6]").is_true());

    // Test union with empty list
    assert!(eval_expr("[1, 2].union([]) | sort == [1, 2]").is_true());

    // Test union of empty list with non-empty
    assert!(eval_expr("[].union([1, 2]) | sort == [1, 2]").is_true());

    // Test union with string elements
    assert!(eval_expr("['a', 'b'].union(['b', 'c']) | sort == ['a', 'b', 'c']").is_true());

    // Test index method - basic usage
    assert_eq!(eval_expr("[1, 2, 3, 4].index(3)").as_i64(), Some(2));
    assert_eq!(eval_expr("['a', 'b', 'c'].index('b')").as_i64(), Some(1));

    // Test index with start parameter
    assert_eq!(eval_expr("[1, 2, 3, 2, 4].index(2, 2)").as_i64(), Some(3));

    // Test index with start and end parameters
    assert_eq!(
        eval_expr("[1, 2, 3, 2, 4].index(2, 1, 3)").as_i64(),
        Some(1)
    );

    // Test index with negative start
    assert_eq!(eval_expr("[1, 2, 3, 4].index(3, -3)").as_i64(), Some(2));

    // Test index with negative end
    assert_eq!(eval_expr("[1, 2, 3, 4].index(2, 0, -1)").as_i64(), Some(1));
}

#[test]
fn test_list_index_errors() {
    let mut env = Environment::new();
    env.set_unknown_method_callback(unknown_method_callback);

    // Test index method error when item not found
    let result = env
        .compile_expression("[1, 2, 3].index(5)")
        .unwrap()
        .eval((), &[]);
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.to_string().contains("5 is not in list"));

    // Test index method error when item not in range
    let result = env
        .compile_expression("[1, 2, 3, 4].index(1, 2)")
        .unwrap()
        .eval((), &[]);
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.to_string().contains("1 is not in list"));
}
