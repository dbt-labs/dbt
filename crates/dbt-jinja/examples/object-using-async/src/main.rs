use std::sync::Arc;

use minijinja::value::{from_args, Object, Value};
use minijinja::{render, Error, ErrorKind, State};
use tokio::runtime::Handle;
use tokio::task::spawn_blocking;

#[derive(Debug)]
struct Site {
    rt: Handle,
}

impl Site {
    async fn get_config(self: Arc<Self>, key: Arc<str>) -> Option<Value> {
        // Imagine this goes to an actual database
        match &key as &str {
            "title" => Some(Value::from("My Title")),
            _ => None,
        }
    }
}

impl Object for Site {
    fn call_method(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        method: &str,
        args: &[Value],
    ) -> Result<Value, Error> {
        match method {
            "get_config" => {
                let (key,) = from_args(args)?;
                Ok(Value::from(self.rt.block_on(self.clone().get_config(key))))
            }
            _ => Err(Error::from(ErrorKind::UnknownMethod)),
        }
    }
}

#[tokio::main]
async fn main() {
    let site = Value::from_object(Site {
        rt: Handle::current(),
    });

    let rv = spawn_blocking(move || render!("title: {{ site.get_config('title') }}", site))
        .await
        .unwrap();
    println!("{}", rv);
}
