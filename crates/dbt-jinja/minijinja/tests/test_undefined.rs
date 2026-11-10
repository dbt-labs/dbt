#![cfg(feature = "builtins")]
use std::collections::HashMap;

use minijinja::{context, render, Environment, ErrorKind, State, UndefinedBehavior};

use similar_asserts::assert_eq;

#[test]
fn test_lenient_undefined() {
    let mut env = Environment::new();
    env.add_filter("test", |state: &State, value: String| -> String {
        assert_eq!(state.undefined_behavior(), UndefinedBehavior::Lenient);
        assert_eq!(value, "");
        value
    });

    assert_eq!(env.undefined_behavior(), UndefinedBehavior::Lenient);
    assert_eq!(render!(in env, "<{{ true.missing_attribute }}>"), "<>");
    assert_eq!(
        env.render_str("{{ undefined.missing_attribute }}", (), &[])
            .unwrap_err()
            .kind(),
        ErrorKind::UndefinedError
    );
    assert_eq!(
        render!(in env, "<{% for x in undefined %}...{% endfor %}>"),
        "<>"
    );
    assert_eq!(render!(in env, "{{ 'foo' is in(undefined) }}"), "False");
    assert_eq!(render!(in env, "<{{ undefined }}>"), "<>");
    assert_eq!(render!(in env, "{{ undefined is undefined }}"), "True");
    assert_eq!(
        render!(in env, "{{ x.foo is undefined }}", x => HashMap::<String, String>::new()),
        "True"
    );
    assert_eq!(render!(in env, "{{ undefined|list }}"), "[]");
    // `dictsort` matches dbt Core, which errors here. `items` is a deliberate
    // divergence: Core returns a generator, but a silent empty list hides typos.
    for source in ["{{ undefined|dictsort }}", "{{ undefined|items }}"] {
        let err = env.render_str(source, (), &[]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidOperation, "{source}");
        assert!(
            err.to_string()
                .contains("cannot convert value into pair list"),
            "{source}: {err}"
        );
    }
    assert_eq!(render!(in env, "<{{ undefined|test }}>"), "<>");
    assert_eq!(render!(in env, "{{ 42 in undefined }}"), "False");
}

#[test]
fn test_allow_all_undefined() {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::AllowAll);

    assert_eq!(render!(in env, "{{ undefined|dictsort }}"), "[]");
    assert_eq!(render!(in env, "{{ undefined|items }}"), "[]");
    assert_eq!(render!(in env, "{{ undefined.a.b.c|dictsort }}"), "[]");
    assert_eq!(render!(in env, "{{ undefined.a.b.c|items }}"), "[]");
    assert_eq!(
        render!(in env, "{{ x.nodes|dictsort }}", x => HashMap::<String, String>::new()),
        "[]"
    );
    assert_eq!(
        render!(in env, "{{ x.nodes|items }}", x => HashMap::<String, String>::new()),
        "[]"
    );
}

/// A defined but non-map operand errors in every phase. `dictsort` matches dbt
/// Core here; `items` is stricter than Core by design.
#[test]
fn test_non_map_pair_list_always_errors() {
    for behavior in [
        UndefinedBehavior::AllowAll,
        UndefinedBehavior::Lenient,
        UndefinedBehavior::Chainable,
        UndefinedBehavior::Strict,
    ] {
        let mut env = Environment::new();
        env.set_undefined_behavior(behavior);

        for operand in [r#""hello""#, "42", "none"] {
            for filter in ["dictsort", "items"] {
                let source = format!("{{{{ {operand}|{filter} }}}}");
                let err = env.render_str(&source, (), &[]).unwrap_err();
                assert_eq!(
                    err.kind(),
                    ErrorKind::InvalidOperation,
                    "{behavior:?} / {source}"
                );
                assert!(
                    err.to_string()
                        .contains("cannot convert value into pair list"),
                    "{behavior:?} / {source}: {err}"
                );
            }
        }
    }
}

#[test]
fn test_strict_undefined() {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);

    assert_eq!(
        env.render_str("{{ true.missing_attribute }}", (), &[])
            .unwrap_err()
            .kind(),
        ErrorKind::UndefinedError
    );
    assert_eq!(
        env.render_str("{{ undefined.missing_attribute }}", (), &[])
            .unwrap_err()
            .kind(),
        ErrorKind::UndefinedError
    );
    assert_eq!(
        env.render_str("<{% for x in undefined %}...{% endfor %}>", (), &[])
            .unwrap_err()
            .kind(),
        ErrorKind::UndefinedError
    );
    assert_eq!(
        env.render_str("{{ 'foo' is in(undefined) }}", (), &[])
            .unwrap_err()
            .kind(),
        ErrorKind::UndefinedError
    );
    assert_eq!(
        env.render_str("<{{ undefined }}>", (), &[])
            .unwrap_err()
            .kind(),
        ErrorKind::UndefinedError
    );
    assert_eq!(render!(in env, "{{ undefined is undefined }}"), "True");
    assert_eq!(
        render!(in env, "{{ x.foo is undefined }}", x => HashMap::<String, String>::new()),
        "True"
    );
    assert_eq!(
        env.render_str(
            "{% if x.foo %}...{% endif %}",
            context! { x => HashMap::<String, String>::new() },
            &[]
        )
        .unwrap_err()
        .kind(),
        ErrorKind::UndefinedError
    );
    assert_eq!(
        env.render_str("{{ undefined|list }}", (), &[])
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidOperation
    );
    assert_eq!(
        env.render_str("{{ undefined|dictsort }}", (), &[])
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidOperation
    );
    assert_eq!(
        env.render_str("{{ undefined|items }}", (), &[])
            .unwrap_err()
            .kind(),
        ErrorKind::InvalidOperation
    );
    assert_eq!(
        env.render_str("{{ 42 in undefined }}", (), &[])
            .unwrap_err()
            .kind(),
        ErrorKind::UndefinedError
    );
}

#[test]
fn test_undefined_operation_names_variable() {
    let env = Environment::new();
    let err = env.render_str("{{ missing + 1 }}", (), &[]).unwrap_err();

    assert_eq!(err.kind(), ErrorKind::UndefinedError);
    assert!(err.to_string().contains("`missing` is undefined"), "{err}");
}

#[test]
fn test_chainable_undefined() {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Chainable);
    env.add_filter("test", |state: &State, value: String| -> String {
        assert_eq!(state.undefined_behavior(), UndefinedBehavior::Chainable);
        assert_eq!(value, "");
        value
    });

    assert_eq!(render!(in env, "<{{ true.missing_attribute }}>"), "<>");
    assert_eq!(render!(in env, "<{{ undefined.missing_attribute }}>"), "<>");
    assert_eq!(
        render!(in env, "<{% for x in undefined %}...{% endfor %}>"),
        "<>"
    );
    assert_eq!(
        render!(in env, "{{ x.foo is undefined }}", x => HashMap::<String, String>::new()),
        "True"
    );
    assert_eq!(render!(in env, "{{ 'foo' is in(undefined) }}"), "False");
    assert_eq!(render!(in env, "<{{ undefined }}>"), "<>");
    assert_eq!(render!(in env, "{{ undefined is undefined }}"), "True");
    assert_eq!(render!(in env, "{{ undefined|list }}"), "[]");
    assert_eq!(render!(in env, "{{ undefined|dictsort }}"), "[]");
    assert_eq!(render!(in env, "{{ undefined|items }}"), "[]");
    assert_eq!(render!(in env, "<{{ undefined|test }}>"), "<>");
    assert_eq!(render!(in env, "{{ 42 in undefined }}"), "False");
}

/// Calling a method on an undefined receiver used to report only the method:
/// `undefined has no method named meta_get`, which reads as though the method were
/// unsupported. Name the variable that was actually undefined instead. dbt Core says
/// `'test_config' is undefined` for the same template.
/// https://github.com/dbt-labs/fs/issues/12688
#[test]
fn test_undefined_receiver_method_call_names_the_variable() {
    let env = Environment::new();

    let err = env
        .render_str("{{ test_config.meta_get('some_key') }}", (), &[])
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UnknownMethod);
    assert_eq!(
        err.detail(),
        Some("`test_config` is undefined, so it has no method named `meta_get`")
    );

    // The receiver's name is only known for a plain variable. Anything else keeps the
    // previous wording rather than guessing at a name the author did not write.
    let err = env
        .render_str("{{ {}.missing.meta_get('some_key') }}", (), &[])
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UnknownMethod);
    assert_eq!(err.detail(), Some("undefined has no method named meta_get"));
}

/// The undefined's origin travels with the value, so a method call that fails after
/// the value has been rebound — passed into a macro and bound to a parameter — still
/// reports the identifier the author wrote. dbt Core says `'missing_config' is
/// undefined` here, naming the caller's variable rather than the macro's parameter.
/// https://github.com/dbt-labs/fs/issues/12793
#[test]
fn test_undefined_name_survives_macro_parameter_binding() {
    let env = Environment::new();

    let err = env
        .render_str(
            "{% macro read_meta(cfg) %}{{ cfg.meta_get('some_key') }}{% endmacro %}\
             {{ read_meta(missing_config) }}",
            (),
            &[],
        )
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::UnknownMethod);
    assert_eq!(
        err.detail(),
        Some("`missing_config` is undefined, so it has no method named `meta_get`"),
        "the caller's variable should be reported, not the macro's `cfg` parameter"
    );
}

/// The recorded origin is for error messages only: it must not make two undefined
/// values distinguishable to anything else. Equality, hashing, ordering, truthiness,
/// `is_undefined` and rendering all have to ignore it, otherwise carrying the name
/// would be a behaviour change well outside error text.
/// https://github.com/dbt-labs/fs/issues/12793
#[test]
fn test_undefined_name_is_not_part_of_value_identity() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let hash_of = |value: &minijinja::Value| {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    };

    let bare = minijinja::Value::UNDEFINED;
    let named = minijinja::Value::undefined_named("missing_config");
    let named_other = minijinja::Value::undefined_named("something_else");

    // Only the name-reading accessor can tell them apart.
    assert_eq!(named.undefined_name(), Some("missing_config"));
    assert_eq!(bare.undefined_name(), None);
    assert_eq!(minijinja::Value::from(1i32).undefined_name(), None);

    for other in [&bare, &named_other] {
        assert_eq!(named, *other, "equality must ignore the recorded name");
        assert_eq!(
            hash_of(&named),
            hash_of(other),
            "hashing must ignore the recorded name"
        );
        assert_eq!(
            named.partial_cmp(other),
            Some(std::cmp::Ordering::Equal),
            "ordering must ignore the recorded name"
        );
    }

    // Still undefined, still falsy, still renders as the empty string, and still
    // distinct from none.
    assert!(named.is_undefined());
    assert!(!named.is_true());
    assert_ne!(named, minijinja::Value::from(()));

    let env = Environment::new();
    assert_eq!(render!(in env, "<{{ x }}>", x => named.clone()), "<>");
    assert_eq!(
        render!(in env, "{{ x is undefined }}", x => named.clone()),
        "True"
    );
}
