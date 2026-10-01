use minijinja::{context, path_loader, Environment};
use once_cell::sync::Lazy;

static ENV: Lazy<Environment<'static>> = Lazy::new(|| {
    let mut env = Environment::new();
    env.set_loader(path_loader("templates"));
    env
});

fn main() {
    let tmpl = ENV.get_template("hello.txt").unwrap();
    let ctx = context!(name => "World");
    println!("{}", tmpl.render(ctx).unwrap());
}
