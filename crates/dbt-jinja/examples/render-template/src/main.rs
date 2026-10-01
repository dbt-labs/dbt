//! This is a small example program that evaluates an expression and returns
//! the result on stdout in JSON format.  The values provided to the script
//! are the environment variables in the `env` dict.
use std::fs;
use std::path::PathBuf;

use argh::FromArgs;
use minijinja::Environment;

/// A small application that renders a MiniJinja template.
#[derive(FromArgs)]
struct Cli {
    /// the path to a JSON file with the context
    #[argh(option, short = 'c', long = "context")]
    context: PathBuf,

    /// the path to a template file that should be rendered
    #[argh(option, short = 't', long = "template")]
    template: PathBuf,
}

fn execute() -> Result<(), Box<dyn std::error::Error>> {
    let cli: Cli = argh::from_env();

    let mut env = Environment::new();
    let source = fs::read_to_string(&cli.template)?;
    let name = cli.template.file_name().unwrap().to_str().unwrap();
    env.add_template(name, &source)?;

    let ctx: serde_json::Value = serde_json::from_slice(&fs::read(&cli.context)?)?;

    let tmpl = env.get_template(name).unwrap();
    println!("{}", tmpl.render(ctx)?);

    Ok(())
}

fn main() {
    execute().unwrap();
}
