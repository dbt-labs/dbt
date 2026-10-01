use insta::assert_snapshot;
use minijinja::{
    constants::{MACRO_NAMESPACE_REGISTRY, ROOT_PACKAGE_NAME},
    context,
    dispatch_object::DispatchObject,
    listener::RenderingEventListener,
    value::{mutable_vec::MutableVec, Object, ValueMap},
    Environment, Error as MinijinjaError, ErrorKind, State, Value,
};
use std::rc::Rc;
use std::sync::Arc;

/// Test namespace object that looks up macros in the namespace registry
#[derive(Debug)]
struct TestNamespace {
    name: String,
}

impl Object for TestNamespace {
    fn get_property(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        name: &str,
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, MinijinjaError> {
        let ns_name = Value::from(self.name.clone());
        let namespace_registry = state
            .env()
            .get_macro_namespace_registry()
            .unwrap_or_default();
        if namespace_registry.get(&ns_name).is_some_and(|val| {
            val.try_iter()
                .map(|mut iter| iter.any(|v| v.as_str() == Some(name)))
                .unwrap_or(false)
        }) {
            Ok(Value::from_object(DispatchObject {
                macro_name: (*name).to_string(),
                package_name: Some(self.name.clone()),
                strict: true,
                auto_execute: false,
                context: Some(state.get_base_context()),
            }))
        } else {
            Ok(Value::UNDEFINED)
        }
    }
}

#[test]
fn test_set_unwarp() {
    let env = Environment::new();
    let rv = env
        .render_str(
            r#"
    {% set fqn = ["one","two","three"] %}
    {%- set a, b, c = fqn[0], fqn[1], fqn[2] %}
    {{ a }}|{{ b }}|{{ c }}
    "#,
            context! {},
            &[],
        )
        .unwrap();
    assert_snapshot!(rv, @"one|two|three");
}

#[test]
fn test_set_append() {
    let env = Environment::new();
    let rv = env
        .render_str(
            r#"
    {%- set my_list = ['x'] -%}
{{ my_list.append('y') }}
{{ my_list }}
    "#,
            context! {},
            &[],
        )
        .unwrap();
    // would be None in dbt-core but this should be just cosmetic
    assert_snapshot!(rv, @r"
    None
    ['x', 'y']
    ");
}

#[test]
fn test_if_condition_implied_tuple() {
    let env = Environment::new();
    let rv = env
        .render_str(
            r#"{% if target_name == "default","np" %}matched{% else %}missed{% endif %}"#,
            context! { target_name => "other" },
            &[],
        )
        .unwrap();

    assert_eq!(rv, "matched");
}

#[test]
fn test_macro_namespace_lookup() {
    let mut env = Environment::new();
    let mut macro_namespace_registry = ValueMap::new();
    macro_namespace_registry.insert(
        Value::from("test_2"),
        Value::from_object(MutableVec::from(vec![Value::from("two")])),
    );
    macro_namespace_registry.insert(
        Value::from("test_1"),
        Value::from_object(MutableVec::from(vec![Value::from("another")])),
    );

    env.add_global(
        MACRO_NAMESPACE_REGISTRY,
        Value::from_object(macro_namespace_registry),
    );
    let _ = env.add_template("test_2.two", "{% macro two() %}two{% endmacro %}");

    // Add namespace objects to context
    let test_1 = Value::from_object(TestNamespace {
        name: "test_1".to_string(),
    });
    let test_2 = Value::from_object(TestNamespace {
        name: "test_2".to_string(),
    });

    let rv = env
        .render_str(
            r#"
    {% set m = test_1.one or test_2.two %}
    {{ m() }}
        "#,
            context! { test_1, test_2 },
            &[],
        )
        .unwrap();
    assert_snapshot!(rv, @"two");

    let test_1 = Value::from_object(TestNamespace {
        name: "test_1".to_string(),
    });
    let test_2 = Value::from_object(TestNamespace {
        name: "test_2".to_string(),
    });
    let rv = env
        .render_str(
            r#"
    {% set m = test_2.two or test_1.one %}
    {{ m() }}
        "#,
            context! { test_1, test_2 },
            &[],
        )
        .unwrap();
    assert_snapshot!(rv, @"two");
}

#[test]
fn test_macro_namespace_subscript_lookup() {
    let mut env = Environment::new();
    let mut macro_namespace_registry = ValueMap::new();
    macro_namespace_registry.insert(
        Value::from("test_2"),
        Value::from_object(MutableVec::from(vec![Value::from("two")])),
    );
    macro_namespace_registry.insert(
        Value::from("test_1"),
        Value::from_object(MutableVec::from(vec![Value::from("another")])),
    );

    env.add_global(
        MACRO_NAMESPACE_REGISTRY,
        Value::from_object(macro_namespace_registry),
    );
    let _ = env.add_template("test_2.two", "{% macro two() %}two{% endmacro %}");

    // Basic subscript access and call
    let test_2 = Value::from_object(TestNamespace {
        name: "test_2".to_string(),
    });
    let rv = env
        .render_str(r#"{{ test_2['two']() }}"#, context! { test_2 }, &[])
        .unwrap();
    assert_snapshot!(rv, @"two");

    // Dynamic key construction
    let test_2 = Value::from_object(TestNamespace {
        name: "test_2".to_string(),
    });
    let rv = env
        .render_str(
            r#"{%- set macro_name = 'two' -%}{{ test_2[macro_name]() }}"#,
            context! { test_2 },
            &[],
        )
        .unwrap();
    assert_snapshot!(rv, @"two");

    // Or-fallback with subscript access
    let test_1 = Value::from_object(TestNamespace {
        name: "test_1".to_string(),
    });
    let test_2 = Value::from_object(TestNamespace {
        name: "test_2".to_string(),
    });
    let rv = env
        .render_str(
            r#"{%- set m = test_1['one'] or test_2['two'] -%}{{ m() }}"#,
            context! { test_1, test_2 },
            &[],
        )
        .unwrap();
    assert_snapshot!(rv, @"two");
}

fn dispatch_env() -> Environment<'static> {
    let mut env = Environment::new();
    env.add_global(ROOT_PACKAGE_NAME, Value::from("myproj"));
    env.add_global(
        "dispatch",
        Value::from_object(DispatchObject {
            macro_name: "thing".to_string(),
            package_name: None,
            strict: false,
            auto_execute: false,
            context: None,
        }),
    );
    env
}

fn dispatch_env_with_fallback(
    candidate_name: &'static str,
    candidate: &'static str,
) -> Environment<'static> {
    let mut env = dispatch_env();
    env.add_template(candidate_name, candidate).unwrap();
    env.add_template(
        "myproj.default__thing",
        "{% macro default__thing() %}default{% endmacro %}",
    )
    .unwrap();
    env
}

#[test]
fn test_dispatch_skips_template_without_macro() {
    let mut env = dispatch_env();
    env.add_template(
        "myproj.default__thing",
        "-- intentionally defines no macro\n",
    )
    .unwrap();

    let err = env
        .render_str("{{ dispatch() }}", Value::UNDEFINED, &[])
        .unwrap_err();

    assert_eq!(err.kind(), ErrorKind::UnknownFunction);
    assert_eq!(
        err.detail(),
        Some(concat!(
            "In dispatch: No macro named 'thing' found within namespace: 'None'\n",
            "    Searched for: 'dbt.postgres__thing', 'myproj.postgres__thing', ",
            "'dbt.default__thing', 'myproj.default__thing', 'dbt.thing', 'myproj.thing'"
        ))
    );
}

#[test]
fn test_state_lookup_template_without_macro_returns_error() {
    let mut env = Environment::new();
    env.add_global(ROOT_PACKAGE_NAME, Value::from("myproj"));
    env.add_template(
        "myproj.default__thing",
        "-- intentionally defines no macro\n",
    )
    .unwrap();

    let err = env
        .render_str("{{ default__thing() }}", Value::UNDEFINED, &[])
        .unwrap_err();

    assert_eq!(err.kind(), ErrorKind::UnknownFunction);
    assert_eq!(
        err.detail(),
        Some(concat!(
            "In dispatch: No macro named 'default__thing' found within namespace: 'myproj'\n",
            "    Searched for: 'myproj.default__thing'"
        ))
    );
}

#[test]
fn test_dispatch_skips_missing_macro_for_later_candidate() {
    for candidate in [
        "-- intentionally defines no macro\n",
        "{% set postgres__thing = 'not a macro' %}",
    ] {
        let env = dispatch_env_with_fallback("myproj.postgres__thing", candidate);

        let rv = env
            .render_str("{{ dispatch() }}", Value::UNDEFINED, &[])
            .unwrap();

        assert_eq!(rv, "default");
    }
}

#[test]
fn test_dispatch_skips_missing_macro_for_same_name_later_package() {
    let env =
        dispatch_env_with_fallback("dbt.default__thing", "-- intentionally defines no macro\n");

    let rv = env
        .render_str("{{ dispatch() }}", Value::UNDEFINED, &[])
        .unwrap();

    assert_eq!(rv, "default");
}

#[test]
fn test_dispatch_preserves_candidate_errors() {
    for candidate in [
        "{{ fail() }}{% macro postgres__thing() %}postgres{% endmacro %}",
        "{% macro postgres__thing() %}{{ fail() }}{% endmacro %}",
    ] {
        let mut env = dispatch_env_with_fallback("myproj.postgres__thing", candidate);
        env.add_function("fail", || -> Result<Value, MinijinjaError> {
            Err(MinijinjaError::new(
                ErrorKind::InvalidOperation,
                "candidate failed",
            ))
        });
        let err = env
            .render_str("{{ dispatch() }}", Value::UNDEFINED, &[])
            .unwrap_err();

        assert_eq!(err.kind(), ErrorKind::InvalidOperation);
        assert_eq!(err.detail(), Some("candidate failed"));
    }
}

#[test]
fn test_dispatch_checks_depth_before_template_evaluation() {
    let mut env = dispatch_env();
    env.set_recursion_limit(20);
    env.add_template(
        "myproj.default__thing",
        "{{ default__thing() }}{% macro default__thing() %}ok{% endmacro %}",
    )
    .unwrap();

    let err = env
        .render_str("{{ dispatch() }}", Value::UNDEFINED, &[])
        .unwrap_err();

    assert_eq!(err.kind(), ErrorKind::InvalidOperation);
    assert_eq!(err.detail(), Some("recursion limit exceeded"));
}

#[test]
fn test_indent_filter_with_width_zero() {
    let env = Environment::new();
    let rv = env
        .render_str(
            r#"
{%- filter indent(width=2) -%}
here
i
am
writing
{%- endfilter -%}
            "#,
            context! {},
            &[],
        )
        .unwrap();
    assert_snapshot!(rv, @"here
  i
  am
  writing");
}

#[test]
fn test_unrecognized_string_escapes_render_verbatim() {
    let env = Environment::new();
    for (template, expected) in [
        (
            r#"{% set pat = "'[+*\/=<> :;.,!?]+'" %}{{ pat }}"#,
            r"'[+*\/=<> :;.,!?]+'",
        ),
        (r#"{{ "a/b" }}"#, "a/b"),
        (r#"{{ "http://x.com/a/b" }}"#, "http://x.com/a/b"),
        (r#"{{ "a/b\tc" }}"#, "a/b\tc"),
        (r#"{{ "a/b\/c" }}"#, "a/b\\/c"),
        (r#"{{ "a\/b" }}"#, r"a\/b"),
        (r#"{{ 'a\/b' }}"#, r"a\/b"),
        (r#"{{ "\\/" }}"#, r"\/"),
        (r#"{{ "\d+" }}"#, r"\d+"),
        (r#"{{ "a\'b\"c\\d" }}"#, r#"a'b"c\d"#),
    ] {
        let rv = env.render_str(template, context! {}, &[]).unwrap();
        assert_eq!(rv, expected, "template: {template}");
    }
}
