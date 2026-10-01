use std::time::Duration;

use minijinja::{context, Environment, Value};

const TEMPLATE: &str = r#"
The stream we will iterate over: {{ stream }}
Results as they come in:

<ul>
{%- for item in stream %}
  <li>Item {{ item }}</li>
{%- endfor %}
</ul>

"#;

fn generate_items() -> impl Iterator<Item = Value> {
    (0..20).map(|item| {
        std::thread::sleep(Duration::from_millis(100));
        Value::from(item)
    })
}

fn main() {
    let mut env = Environment::new();
    env.add_template("response.txt", TEMPLATE).unwrap();
    let template = env.get_template("response.txt").unwrap();
    template
        .render_to_write(
            context! {
                stream => Value::make_one_shot_iterator(generate_items())
            },
            &std::io::stdout(),
        )
        .unwrap();
}
