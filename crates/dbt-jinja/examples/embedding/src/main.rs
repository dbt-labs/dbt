use minijinja::{context, Environment};

fn make_env() -> Environment<'static> {
    let mut env = Environment::new();

    #[cfg(feature = "bundled")]
    {
        minijinja_embed::load_templates!(&mut env);
    }

    #[cfg(not(feature = "bundled"))]
    {
        env.set_loader(minijinja::path_loader("src/templates"));
    }

    env
}

fn main() {
    let env = make_env();
    let template = env.get_template("index.html").unwrap();
    let page = context! {
        title => "Some title",
        content => "Lorum Ipsum",
    };
    println!("{}", template.render(context!(page)).unwrap());
}
