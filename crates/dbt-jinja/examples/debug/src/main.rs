use minijinja::{context, Environment};

fn main() {
    let mut env = Environment::new();
    env.add_template("demo.txt", include_str!("demo.txt"))
        .unwrap();
    let template = env.get_template("demo.txt").unwrap();
    println!(
        "{}",
        template
            .render(context! {
                name => "Peter Lustig",
                iterations => 1
            })
            .unwrap()
    );
}
