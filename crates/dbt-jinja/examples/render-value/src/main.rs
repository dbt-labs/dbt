use minijinja::{context, Environment, Value};

fn main() {
    let env = Environment::new();

    // this just demonstrates that `context!` creates a `Value`
    let ctx: Value = context! {
        name => "Peter"
    };

    // Which can be directly passed to `render_str`.
    println!("{}", env.render_str("Hello {{ name }}!", ctx).unwrap());
}
