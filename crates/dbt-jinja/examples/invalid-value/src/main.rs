use minijinja::{context, Environment};
use serde::Serialize;

/// This struct makes no sense, and serde will fail serializing it.
#[derive(Serialize, Clone)]
pub struct BadStruct {
    a: i32,
    #[serde(flatten)]
    b: i32,
}

fn main() {
    let mut env = Environment::new();
    env.add_template("good.txt", "good={{ good }}").unwrap();
    env.add_template("mixed.txt", "mixed-container={{ container }}")
        .unwrap();
    env.add_template("bad.txt", "bad={{ bad }}").unwrap();

    let good = true;
    let bad = BadStruct { a: 1, b: 2 };
    let container = context! { good, bad };
    let ctx = context! { good, bad, container };

    for name in ["good.txt", "mixed.txt", "bad.txt"] {
        let template = env.get_template(name).unwrap();
        println!("{}:", name);
        println!("  template: {:?}", template.source());
        match template.render(&ctx) {
            Ok(result) => println!("  result: {}", result),
            Err(err) => println!("  error: {}", err),
        }
    }
}
