use minijinja::{context, syntax::SyntaxConfig, Environment};

fn main() {
    let mut env = Environment::new();
    env.set_syntax(
        SyntaxConfig::builder()
            .line_statement_prefix("#")
            .line_comment_prefix("##")
            .build()
            .unwrap(),
    );
    env.add_template("hello.txt", include_str!("hello.txt"))
        .unwrap();
    let template = env.get_template("hello.txt").unwrap();
    println!(
        "{}",
        template
            .render(context!(seq => vec!["foo", "bar"]))
            .unwrap()
    );
}
