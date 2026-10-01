use std::iter::FromIterator;

use minijinja::value::{Kwargs, Value};
use minijinja::{args, Environment, Error, ErrorKind, State};

fn custom_loop(state: &State, num: i64, kwargs: Kwargs) -> Result<String, Error> {
    let mut rv = String::new();
    let caller = kwargs.get::<Value>("caller")?;
    kwargs.assert_all_used()?;
    for it in 0..num {
        let rendered = caller.call(state, args!(it => it + 1))?;
        rv.push_str(rendered.as_str().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidOperation,
                "caller did not return a string",
            )
        })?);
    }
    Ok(rv)
}

fn main() {
    let mut env = Environment::new();
    env.add_function("custom_loop", custom_loop);
    let tmpl = env.template_from_str(include_str!("demo.txt")).unwrap();
    println!("{}", tmpl.render(()).unwrap());
}
