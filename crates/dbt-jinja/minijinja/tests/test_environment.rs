use std::collections::BTreeMap;

use insta::assert_snapshot;
use similar_asserts::assert_eq;

use minijinja::Environment;
use minijinja::Value;

#[test]
fn test_basic() {
    let mut env = Environment::new();
    env.add_template("test", "{% for x in seq %}[{{ x }}]{% endfor %}")
        .unwrap();
    let t = env.get_template("test").unwrap();
    let mut ctx = BTreeMap::new();
    ctx.insert("seq", Value::from((0..3).collect::<Vec<_>>()));
    let rv = t.render(ctx, &[]).unwrap();
    assert_eq!(rv, "[0][1][2]");
}

#[test]
fn test_expression() {
    let env = Environment::new();
    let expr = env.compile_expression("foo + bar").unwrap();
    let mut ctx = BTreeMap::new();
    ctx.insert("foo", 42);
    ctx.insert("bar", 23);
    assert_eq!(expr.eval(&ctx, &[]).unwrap(), Value::from(65));
}

#[test]
#[cfg(feature = "loader")]
fn test_expression_owned() {
    let env = Environment::new();
    let expr: minijinja::Expression<'_, 'static> = env
        .compile_expression_owned("foo + bar".to_string())
        .unwrap();
    let mut ctx = BTreeMap::new();
    ctx.insert("foo", 42);
    ctx.insert("bar", 23);
    assert_eq!(expr.eval(&ctx, &[]).unwrap(), Value::from(65));
}

#[test]
fn test_expression_bug() {
    let env = Environment::new();
    assert!(env.compile_expression("42.blahadsf()").is_err());
}

#[test]
fn test_expression_lifetimes() {
    let mut env = Environment::new();
    let s = String::new();
    env.add_template("test", &s).unwrap();
    {
        let x = String::from("1 + 1");
        let expr = env.compile_expression(&x).unwrap();
        assert_eq!(expr.eval((), &[]).unwrap().to_string(), "2");
    }
}

#[test]
fn test_expression_undeclared_variables() {
    let env = Environment::new();
    let expr = env.compile_expression("[foo, bar.baz]").unwrap();
    let undeclared = expr.undeclared_variables(false);
    assert_eq!(
        undeclared,
        ["bar", "foo"].into_iter().map(|x| x.to_string()).collect()
    );
    let undeclared = expr.undeclared_variables(true);
    assert_eq!(
        undeclared,
        ["foo", "bar.baz"]
            .into_iter()
            .map(|x| x.to_string())
            .collect()
    );
}

#[test]
fn test_clone() {
    let mut env = Environment::new();
    env.add_template("test", "a").unwrap();
    let mut env2 = env.clone();
    assert_eq!(
        env2.get_template("test").unwrap().render((), &[]).unwrap(),
        "a"
    );
    env2.add_template("test", "b").unwrap();
    assert_eq!(
        env2.get_template("test").unwrap().render((), &[]).unwrap(),
        "b"
    );
    assert_eq!(
        env.get_template("test").unwrap().render((), &[]).unwrap(),
        "a"
    );
}

#[test]
fn test_globals() {
    let mut env = Environment::new();
    env.add_global("a", Value::from(42));
    env.add_template("test", "{{ a }}").unwrap();
    let tmpl = env.get_template("test").unwrap();
    assert_eq!(tmpl.render((), &[]).unwrap(), "42");
}

#[test]
fn test_template_removal() {
    let mut env = Environment::new();
    env.add_template("test", "{{ a }}").unwrap();
    env.remove_template("test");
    assert!(env.get_template("test").is_err());
}

#[test]
#[cfg(feature = "multi_template")]
fn test_path_join() {
    let mut env = Environment::new();
    env.add_template("x/a/foo.txt", "{% include '../b/bar.txt' %}")
        .unwrap();
    env.add_template("x/b/bar.txt", "bar.txt").unwrap();
    env.set_path_join_callback(|name, parent| {
        let mut rv = parent.split('/').collect::<Vec<_>>();
        rv.pop();
        name.split('/').for_each(|segment| match segment {
            "." => {}
            ".." => {
                rv.pop();
            }
            other => rv.push(other),
        });
        rv.join("/").into()
    });
    let t = env.get_template("x/a/foo.txt").unwrap();
    assert_eq!(t.render((), &[]).unwrap(), "bar.txt");
}

#[test]
fn test_keep_trailing_newlines() {
    let mut env = Environment::new();
    env.add_template("foo.txt", "blub\r\n").unwrap();
    assert_eq!(env.render_str("blub\r\n", (), &[]).unwrap(), "blub");

    env.set_keep_trailing_newline(true);
    env.add_template("foo_keep.txt", "blub\r\n").unwrap();
    assert_eq!(
        env.get_template("foo.txt")
            .unwrap()
            .render((), &[])
            .unwrap(),
        "blub"
    );
    assert_eq!(
        env.get_template("foo_keep.txt")
            .unwrap()
            .render((), &[])
            .unwrap(),
        "blub\r\n"
    );
    assert_eq!(env.render_str("blub\r\n", (), &[]).unwrap(), "blub\r\n");
}

#[test]
#[cfg(feature = "builtins")]
fn test_unknown_method_callback() {
    use minijinja::value::{from_args, ValueKind};
    use minijinja::{Error, ErrorKind};

    let mut env = Environment::new();
    env.set_unknown_method_callback(|state, value, method, args| {
        if value.kind() == ValueKind::Map && method == "items" {
            from_args::<()>(args)?;
            minijinja::filters::items(state, value)
        } else {
            Err(Error::from(ErrorKind::UnknownMethod))
        }
    });

    let rv = env.render_str("{{ {'x': 42}.items() }}", (), &[]).unwrap();
    assert_snapshot!(rv, @"[('x', 42)]");
}

#[test]
fn test_iter() {
    let mut env = Environment::new();
    env.add_template("hello", "Hello {{ name }}!").unwrap();
    env.add_template("goodbye", "Goodbye {{ name }}!").unwrap();

    let mut ctx = BTreeMap::new();
    ctx.insert("name", Value::from("World"));
    let renders = env
        .templates()
        .map(|(name, tmpl)| (name, tmpl.render(&ctx, &[]).unwrap()))
        .collect::<Vec<_>>();

    assert_eq!(renders.len(), 2);
    assert!(renders.contains(&("hello", "Hello World!".into())));
    assert!(renders.contains(&("goodbye", "Goodbye World!".into())));
}
