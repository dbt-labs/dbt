use std::fs;

use minijinja::{context, Environment, Error, ErrorKind};

fn main() {
    let mut env = Environment::new();
    let template_path = std::env::current_dir().unwrap().join("templates");

    env.set_loader(move |name| {
        let pieces = name.split('/');
        let mut path = template_path.clone();
        for piece in pieces {
            if piece != "." && piece != ".." && !piece.contains('\\') {
                path.push(piece);
            } else {
                return Ok(None);
            }
        }

        match fs::read_to_string(path) {
            Ok(result) => Ok(Some(result)),
            Err(err) => {
                if err.kind() == std::io::ErrorKind::NotFound {
                    Ok(None)
                } else {
                    Err(
                        Error::new(ErrorKind::TemplateNotFound, "failed to load template")
                            .with_source(err),
                    )
                }
            }
        }
    });

    let tmpl = env.get_template("hello.txt").unwrap();
    println!(
        "{}",
        tmpl.render(context! {
            name => "World"
        })
        .unwrap()
    );
}
