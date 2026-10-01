use minijinja::{context, escape_formatter, Environment, Value};
use serde::Serialize;

/// Similar to the regular `default` filter but also handles `none`.
pub fn none_default(value: Value, other: Option<Value>) -> Value {
    if value.is_undefined() || value.is_none() {
        other.unwrap_or_else(|| Value::from(""))
    } else {
        value
    }
}

/// An example struct.
#[derive(Serialize)]
struct Foo {
    bar: Option<bool>,
}

fn main() {
    let mut env = Environment::new();

    env.add_filter("default", none_default);
    env.set_formatter(|out, state, value| {
        escape_formatter(
            out,
            state,
            if value.is_none() {
                &Value::UNDEFINED
            } else {
                value
            },
        )
    });

    env.add_template(
        "hello.txt",
        "A None attribute: {{ foo.bar }}\nWith default: {{ foo.bar|default(42) }}",
    )
    .unwrap();
    let template = env.get_template("hello.txt").unwrap();

    println!(
        "{}",
        template
            .render(context! {
                foo => Foo { bar: None },
            })
            .unwrap()
    );
}
