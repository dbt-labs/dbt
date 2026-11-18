#![cfg(feature = "loader")]

use minijinja::Environment;

use similar_asserts::assert_eq;

fn create_env() -> Environment<'static> {
    let mut env = Environment::new();
    let template = String::from("Hello World!");
    env.add_template_owned("hello", template, None).unwrap();
    env
}

#[test]
fn test_basic() {
    let env = create_env();
    let t = env.get_template("hello").unwrap();
    assert_eq!(t.render((), &[]).unwrap(), "Hello World!");
}

#[test]
fn test_dynamic() {
    let mut env = Environment::new();
    let template = String::from("Hello World 2!");
    env.add_template_owned("hello2", template, None).unwrap();
    env.set_loader(|name| match name {
        "hello" => Ok(Some("Hello World!".to_string())),
        _ => Ok(None),
    });
    let t = env.get_template("hello").unwrap();
    assert_eq!(t.render((), &[]).unwrap(), "Hello World!");
    let t = env.get_template("hello2").unwrap();
    assert_eq!(t.render((), &[]).unwrap(), "Hello World 2!");
    let err = env.get_template("missing").unwrap_err();
    assert_eq!(
        err.to_string(),
        "template not found: template \"missing\" does not exist"
    );
}

#[test]
fn test_dynamic_loader_filename() {
    let mut env = Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    env.set_loader(|name| match name {
        "broken" => Ok(Some(("{%".to_string(), Some("broken.sql".into())))),
        _ => Ok(None),
    });
    let err = env.get_template("broken").unwrap_err();
    assert_ne!(err.kind(), minijinja::ErrorKind::TemplateNotFound);
    assert!(err.to_string().contains("broken.sql"), "{err}");
}

#[test]
fn test_source_replace_static() {
    let mut env = Environment::new();
    env.add_template_owned("a", "1", None).unwrap();
    env.add_template_owned("a", "2", None).unwrap();
    let rv = env.get_template("a").unwrap().render((), &[]).unwrap();
    assert_eq!(rv, "2");
}

#[test]
fn test_source_replace_dynamic() {
    let mut env = Environment::new();
    env.add_template("a", "1").unwrap();
    env.add_template("a", "2").unwrap();
    env.set_loader(|_| Ok(None::<String>));
    let rv = env.get_template("a").unwrap().render((), &[]).unwrap();
    assert_eq!(rv, "2");
}
