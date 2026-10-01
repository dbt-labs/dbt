use std::fmt;

use minijinja::{context, Environment, Error, ErrorKind};

#[derive(Debug)]
struct UserError(String, usize);

impl UserError {
    fn code(&self) -> usize {
        self.1
    }

    fn message(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "user error: {}", self.0)
    }
}

impl std::error::Error for UserError {}

fn execute() -> Result<(), minijinja::Error> {
    let mut env = Environment::new();
    env.set_debug(true);
    env.add_function(
        "trigger_user_error",
        |s: String, i: usize| -> Result<(), Error> {
            Err(Error::from(ErrorKind::InvalidOperation).with_source(UserError(s, i)))
        },
    );
    env.add_template(
        "include.txt",
        "{{ trigger_user_error('This really should not happen', 42) }}!",
    )?;
    env.add_template(
        "hello.txt",
        r#"
        first line
        {% for item in seq %}
          {% include "include.txt" %}
        {% endfor %}
        last line
        "#,
    )?;
    let template = env.get_template("hello.txt").unwrap();
    let ctx = context! {
        seq => vec![2, 4, 8],
    };
    println!("{}", template.render(&ctx)?);
    Ok(())
}

fn main() {
    if let Err(err) = execute() {
        eprintln!("template error: {err:#}");

        let mut err = &err as &dyn std::error::Error;
        while let Some(next_err) = err.source() {
            if let Some(user_err) = next_err.downcast_ref::<UserError>() {
                eprintln!();
                eprintln!("caused by a well known error:");
                eprintln!("  message: {}", user_err.message());
                eprintln!("  code: {}", user_err.code());
            } else {
                eprintln!();
                eprintln!("caused by: {next_err:#}");
            }
            err = next_err;
        }

        std::process::exit(1);
    }
}
