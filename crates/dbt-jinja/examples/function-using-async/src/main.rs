use std::sync::Arc;

use minijinja::value::{Enumerator, Object, Value, ValueKind};
use minijinja::{context, Environment, Error, State};
use tokio::runtime::Handle;
use tokio::task::spawn_blocking;

/// Utility object to hold a reference to the runtime and the context.
#[derive(Debug)]
struct ContextWithRuntime {
    rt: Handle,
    ctx: Value,
}

impl Object for ContextWithRuntime {
    fn get_value(self: &Arc<Self>, name: &Value) -> Option<Value> {
        // $context is a reserved name that templates cannot resolve, but we can
        // use to pluck out a reference to ourselves in functions get get the
        // state passed.
        if name.as_str() == Some("$context") {
            return Some(Value::from_dyn_object(self.clone()));
        }
        self.ctx.get_item(name).ok().filter(|x| !x.is_undefined())
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        if self.ctx.kind() == ValueKind::Map {
            if let Ok(keys) = self.ctx.try_iter() {
                return Enumerator::Values(keys.collect());
            }
        }
        Enumerator::Empty
    }
}

/// Given a context, wraps it so that the runtime is included.
fn capture_runtime_handle(ctx: Value) -> Value {
    Value::from_object(ContextWithRuntime {
        ctx,
        rt: Handle::current(),
    })
}

/// Utility function to retrieve the current runtime handle from the template state.
fn get_runtime_handle(state: &State) -> Handle {
    let value = state.lookup("$context").unwrap();
    value
        .downcast_object_ref::<ContextWithRuntime>()
        .unwrap()
        .rt
        .clone()
}

/// This is a function that would access a database etc.
async fn get_config(key: Arc<str>) -> Option<Value> {
    // Imagine this goes to an actual database
    match &key as &str {
        "title" => Some(Value::from("My Title")),
        _ => None,
    }
}

/// Wrapper function that calls `get_config` from the context of a template.
fn get_config_template(state: &State, key: Arc<str>) -> Result<Value, Error> {
    let rt = get_runtime_handle(state);
    Ok(Value::from(rt.block_on(get_config(key))))
}

#[tokio::main]
async fn main() {
    let mut env = Environment::new();
    env.add_function("get_config", get_config_template);
    env.add_template("hello", "title: {{ get_config(key) }}")
        .unwrap();

    // capture the runtime handle in the context
    let ctx = capture_runtime_handle(context! {
        key => Value::from("title"),
    });

    // then spawn template rendering in another thread.
    let rv = spawn_blocking(move || {
        let t = env.get_template("hello").unwrap();
        t.render(ctx).unwrap()
    })
    .await
    .unwrap();

    println!("{}", rv);
}
