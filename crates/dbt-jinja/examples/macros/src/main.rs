use minijinja::{context, Environment};

fn main() {
    let mut env = Environment::new();
    env.add_template("macros.html", include_str!("macros.html"))
        .unwrap();
    env.add_template("template.html", include_str!("template.html"))
        .unwrap();

    let template = env.get_template("template.html").unwrap();
    let context = context! {
        username => "John Doe"
    };

    println!("{}", template.render(&context).unwrap());
}
