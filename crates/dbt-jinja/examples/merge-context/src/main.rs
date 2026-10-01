use minijinja::{context, Environment, Value};
use minijinja_contrib::add_to_environment;

fn render_template(env: &Environment, tmpl: &str, ctx: Value) -> String {
    let tmpl = env.get_template(tmpl).unwrap();
    tmpl.render(context! {
        // This variable is used by `datetimeformat` to pick the right
        // datetime format.
        DATETIME_FORMAT => "full",
        ..ctx
    })
    .unwrap()
}

fn main() {
    let mut env = Environment::new();
    env.add_template(
        "template.txt",
        "Current user: {{ user }}\nCurrent time: {{ now()|datetimeformat }}",
    )
    .unwrap();
    add_to_environment(&mut env);

    println!(
        "{}",
        render_template(
            &env,
            "template.txt",
            context! {
                user => "John Doe"
            }
        )
    );
}
