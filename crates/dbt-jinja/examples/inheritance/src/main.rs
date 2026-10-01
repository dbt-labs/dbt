use minijinja::{context, Environment};
use serde::Serialize;

#[derive(Serialize)]
pub struct Page {
    title: String,
    content: String,
}

fn main() {
    let mut env = Environment::new();
    env.add_template("layout.html", include_str!("layout.html"))
        .unwrap();
    env.add_template("index.html", include_str!("index.html"))
        .unwrap();

    let template = env.get_template("index.html").unwrap();
    let page = Page {
        title: "Some title".into(),
        content: "Lorum Ipsum".into(),
    };
    println!("{}", template.render(context!(page)).unwrap());
}
