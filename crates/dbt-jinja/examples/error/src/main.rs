use minijinja::{context, Environment};

fn execute() -> Result<(), minijinja::Error> {
    let mut env = Environment::new();
    env.set_debug(true);
    env.add_template("include.txt", "Hello {{ item_squared + bar }}!")?;
    env.add_template(
        "hello.txt",
        r#"
        first line
        {% for item in seq %}
          {% with item_squared = item * item %}
            {% with foo = 42 %}
              {{ range(10) }}
              {{ other_seq|join(" ") }}
              {% include "include.txt" %}
            {% endwith %}
          {% endwith %}
        {% endfor %}
        last line
        "#,
    )?;
    let template = env.get_template("hello.txt").unwrap();
    let ctx = context! {
        seq => vec![2, 4, 8],
        other_seq => (0..5).collect::<Vec<_>>(),
        bar => "test"
    };
    println!("{}", template.render(&ctx)?);
    Ok(())
}

fn main() {
    if let Err(err) = execute() {
        eprintln!("template error: {err:#}");

        let mut err = &err as &dyn std::error::Error;
        while let Some(next_err) = err.source() {
            eprintln!();
            eprintln!("caused by: {next_err:#}");
            err = next_err;
        }

        std::process::exit(1);
    }
}
