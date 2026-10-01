use std::env;
use std::fs;

use minijinja::{Environment, Error, ErrorKind, Value};

fn load_data(filename: &str) -> Result<Value, Error> {
    let mut rv = env::current_dir().unwrap().join("src");
    for segment in filename.split('/') {
        if segment.starts_with('.') || segment.contains('\\') {
            return Err(Error::new(ErrorKind::InvalidOperation, "bad filename"));
        }
        rv.push(segment);
    }
    let contents = fs::read(&rv).map_err(|err| {
        Error::new(ErrorKind::InvalidOperation, "could not read JSON file").with_source(err)
    })?;
    let parsed: serde_json::Value = serde_json::from_slice(&contents[..])
        .map_err(|err| Error::new(ErrorKind::InvalidOperation, "invalid JSON").with_source(err))?;
    Ok(Value::from_serialize(parsed))
}

fn main() {
    let mut env = Environment::new();
    env.add_function("load_data", load_data);
    env.add_template("template.html", include_str!("template.html"))
        .unwrap();

    let tmpl = env.get_template("template.html").unwrap();
    println!("{}", tmpl.render(()).unwrap());
}
