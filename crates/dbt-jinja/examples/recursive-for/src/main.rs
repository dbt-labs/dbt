use minijinja::{context, Environment};
use serde::Serialize;

#[derive(Serialize)]
struct Item {
    link: &'static str,
    title: &'static str,
    children: Vec<Item>,
}

fn main() {
    let mut env = Environment::new();
    env.set_debug(true);
    env.add_template(
        "loop.html",
        r#"
    <ul class="nav">
    {% for item in nav recursive %}
      <li><a href={{ item.link }}">{{ item.title }}</a>{%
        if item.children %}<ul>{{ loop(item.children) }}</ul>{% endif %}</li>
    {% endfor %}
    </ul>
    "#,
    )
    .unwrap();
    let template = env.get_template("loop.html").unwrap();
    println!(
        "{}",
        template
            .render(context!(nav => vec![
                Item {
                    link: "/",
                    title: "Index",
                    children: Vec::new()
                },
                Item {
                    link: "/docs",
                    title: "Documentation",
                    children: vec![
                        Item {
                            link: "/docs/installation",
                            title: "Installation",
                            children: Vec::new()
                        },
                        Item {
                            link: "/docs/faq",
                            title: "FAQ",
                            children: Vec::new()
                        },
                    ],
                },
            ]))
            .unwrap()
    );
}
