use std::sync::Arc;
use std::{env, fmt};

use minijinja::value::{Enumerator, Object, Value};
use minijinja::{context, Environment, Error, ErrorKind, State};

#[derive(Debug)]
struct NestedConfig {
    active: bool,
}

#[derive(Debug)]
struct Config {
    manifest_dir: &'static str,
    version: &'static str,
    nested: Arc<NestedConfig>,
}

impl Object for Config {
    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Str(&["manifest_dir", "version", "nested"])
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "manifest_dir" => Some(Value::from(self.manifest_dir)),
            "version" => Some(Value::from(self.version)),
            "nested" => Some(Value::from_dyn_object(self.nested.clone())),
            _ => None,
        }
    }
}

impl Object for NestedConfig {
    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Str(&["active"])
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "active" => Some(Value::from(self.active)),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct Utils;

impl fmt::Display for Utils {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<utils>")
    }
}

impl Object for Utils {
    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        _args: &[Value],
    ) -> Result<Value, Error> {
        match name {
            "get_cwd" => Ok(Value::from(env::current_dir().unwrap().to_string_lossy())),
            _ => Err(Error::from(ErrorKind::UnknownMethod)),
        }
    }
}

fn main() {
    let env = Environment::new();

    // values on the stack we want to pass dynamically to the template
    // without serialization
    let config = Config {
        manifest_dir: env!("CARGO_MANIFEST_DIR"),
        version: env!("CARGO_PKG_VERSION"),
        nested: Arc::new(NestedConfig { active: true }),
    };
    let items = [1i32, 2, 3, 4];

    let ctx = context! {
        config => Value::from_object(config),
        utils => Value::from_object(Utils),
        items => Value::make_object_iterable(items, |items| {
            Box::new(items.iter().copied().map(Value::from))
        })
    };
    println!(
        "{}",
        env.render_str(include_str!("template.txt"), ctx).unwrap()
    );
}
