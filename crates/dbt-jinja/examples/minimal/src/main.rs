use minijinja::{context, Environment};

fn main() {
    let mut env = Environment::new();
    env.add_template("hello.txt", include_str!("hello.txt"))
        .unwrap();
    let tmpl = env.get_template("hello.txt").unwrap();
    println!(
        "{}",
        tmpl.render(context!(names => ["John", "Peter"])).unwrap()
    );
}
