use std::collections::HashMap;
use std::env;
use std::sync::Arc;

use minijinja::value::{Enumerator, Object, Value};
use minijinja::Environment;

#[derive(Debug)]
struct DynamicContext;

impl Object for DynamicContext {
    fn get_value(self: &Arc<Self>, field: &Value) -> Option<Value> {
        Some(match field.as_str()? {
            "pid" => Value::from(std::process::id()),
            "cwd" => Value::from(env::current_dir().unwrap().to_string_lossy()),
            "env" => Value::from(
                env::vars()
                    .filter(|(k, _)| k.starts_with("CARGO_") || k.starts_with("RUST_"))
                    .collect::<HashMap<String, String>>(),
            ),
            _ => return None,
        })
    }

    /// This implementation is not needed for the example.  However
    /// returning known keys here has the benefit that `{{ debug() }}`
    /// can show the context.
    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Str(&["pid", "cwd", "env"])
    }
}

fn main() {
    let env = Environment::new();
    println!(
        "{}",
        env.render_str(
            include_str!("template.txt"),
            Value::from_object(DynamicContext)
        )
        .unwrap()
    );
}
