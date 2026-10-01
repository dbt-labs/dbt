use std::path::Path;
use std::{env, fs};

use minijinja::{render, Environment};

fn main() {
    // This environment has a formatter that formats unsafe values in Rust's
    // debug format, and safe values as normal strings.
    let mut env = Environment::new();
    env.set_formatter(|out, _state, value| {
        if !value.is_safe() {
            write!(out, "{value:?}")?;
        } else {
            write!(out, "{value}")?;
        }
        Ok(())
    });

    // render the template and write it into the file that main.rs includes.
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("example.rs"),
        render!(
            in env,
            include_str!("src/example.rs.jinja"),
            struct_name => "Point",
            points => vec![
                (1.0, 2.0),
                (2.0, 2.5),
                (4.0, 1.0),
            ],
            build_cwd => env::current_dir().unwrap()
        ),
    )
    .unwrap();
}
