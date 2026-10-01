use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::sync::Arc;
use std::sync::Mutex;

use minijinja::value::{Object, Value};
use minijinja::Environment;

#[derive(Default, Debug)]
struct Site {
    cache: Mutex<BTreeMap<String, Value>>,
}

impl Object for Site {
    /// This loads a file on attribute access.  Note that attribute access
    /// can neither access the state nor return failures as such it can at
    /// max turn into an undefined object.
    ///
    /// If that is necessary, use `call_method()` instead which is able to
    /// both access interpreter state and fail.
    fn get_value(self: &Arc<Self>, name: &Value) -> Option<Value> {
        let name = name.as_str()?;
        let mut cache = self.cache.lock().unwrap();
        if let Some(rv) = cache.get(name) {
            return Some(rv.clone());
        }
        let val = load_json(name)?;
        cache.insert(name.to_string(), val.clone());
        Some(val)
    }
}

fn load_json(name: &str) -> Option<Value> {
    let mut rv = env::current_dir().unwrap().join("src");
    for segment in name.split('/') {
        if segment.starts_with('.') || segment.contains('\\') {
            return None;
        }
        rv.push(segment);
    }
    rv.set_extension("json");
    let contents = fs::read(&rv).ok()?;
    let parsed: serde_json::Value = serde_json::from_slice(&contents[..]).ok()?;
    Some(Value::from_serialize(parsed))
}

fn main() {
    let mut env = Environment::new();
    env.add_global("site", Value::from_object(Site::default()));
    env.add_template("template.html", include_str!("template.html"))
        .unwrap();

    let tmpl = env.get_template("template.html").unwrap();
    println!("{}", tmpl.render(()).unwrap());
}
