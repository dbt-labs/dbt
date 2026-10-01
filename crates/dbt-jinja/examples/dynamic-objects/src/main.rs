#![allow(clippy::let_unit_value)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use minijinja::value::{from_args, Enumerator, Object, ObjectRepr, Value};
use minijinja::{Environment, Error, State};

#[derive(Debug)]
struct Cycler {
    values: Vec<Value>,
    idx: AtomicUsize,
}

impl Object for Cycler {
    fn call(self: &Arc<Self>, _state: &State, args: &[Value]) -> Result<Value, Error> {
        // we don't want any args
        let () = from_args(args)?;
        let idx = self.idx.fetch_add(1, Ordering::Relaxed);
        Ok(self.values[idx % self.values.len()].clone())
    }
}

fn make_cycler(_state: &State, args: Vec<Value>) -> Result<Value, Error> {
    Ok(Value::from_object(Cycler {
        values: args,
        idx: AtomicUsize::new(0),
    }))
}

#[derive(Debug)]
struct Magic;

impl Object for Magic {
    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        args: &[Value],
    ) -> Result<Value, Error> {
        if name == "make_class" {
            // single string argument
            let (tag,): (&str,) = from_args(args)?;
            Ok(Value::from(format!("magic-{tag}")))
        } else {
            Err(Error::from(minijinja::ErrorKind::UnknownMethod))
        }
    }
}

#[derive(Debug)]
struct SimpleDynamicSeq([char; 4]);

impl Object for SimpleDynamicSeq {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Seq
    }

    fn get_value(self: &Arc<Self>, idx: &Value) -> Option<Value> {
        self.0.get(idx.as_usize()?).copied().map(Value::from)
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Seq(self.0.len())
    }
}

fn main() {
    let mut env = Environment::new();
    env.add_function("cycler", make_cycler);
    env.add_global("magic", Value::from_object(Magic));
    env.add_global(
        "seq",
        Value::from_object(SimpleDynamicSeq(['a', 'b', 'c', 'd'])),
    );
    env.add_global("real_iter", Value::make_iterable(|| (0..10).chain(20..30)));
    env.add_template("template.html", include_str!("template.html"))
        .unwrap();

    let tmpl = env.get_template("template.html").unwrap();
    println!("{}", tmpl.render(()).unwrap());
}
