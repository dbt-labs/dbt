use std::collections::BTreeMap;
use std::fmt;

use anyhow::{anyhow, Error};
use minijinja::{context, Environment, Value};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::cli::print_error;

pub fn run(mut env: Environment, ctx: Value) -> Result<(), Error> {
    let mut editor = DefaultEditor::new()?;
    let mut locals = BTreeMap::new();

    env.add_function("print", print);

    println!("MiniJinja Expression REPL");
    println!("Type .help for help. Use .quit or ^D to exit.");

    loop {
        let readline = editor.readline(">>> ");
        match readline {
            Ok(line) => {
                if line.trim().is_empty() {
                    continue;
                }
                editor.add_history_entry(&line)?;
                match parse_command(&line) {
                    Some(Command::Set { var, expr }) => {
                        if let Some(rv) = eval(&env, expr, &ctx, &locals) {
                            locals.insert(var.to_string(), rv);
                        }
                    }
                    Some(Command::Unset { var }) => {
                        locals.remove(var);
                    }
                    Some(Command::Render { template }) => {
                        render(&env, template, &ctx, &locals);
                    }
                    Some(Command::Invalid) => {
                        print_error(&anyhow!("invalid command"));
                    }
                    Some(Command::Help) => {
                        println!("Commands:");
                        println!(".quit / .exit  quit the REPL");
                        println!(".help          shows this help");
                        println!(".set x=expr    set variable x to the evaluated expression");
                        println!(".unset x       unsets variable x");
                        println!(".render tmpl   renders the given template source");
                    }
                    Some(Command::Quit) => break,
                    None => {
                        if let Some(rv) = eval(&env, &line, &ctx, &locals) {
                            print_result(&rv);
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {}
            Err(ReadlineError::Eof) => break,
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

enum Command<'a> {
    Set { var: &'a str, expr: &'a str },
    Unset { var: &'a str },
    Render { template: &'a str },
    Help,
    Quit,
    Invalid,
}

fn parse_command(line: &str) -> Option<Command<'_>> {
    let line = if let Some(rest) = line.strip_prefix('.') {
        rest.trim()
    } else {
        return None;
    };
    match line {
        "exit" | "quit" => return Some(Command::Quit),
        "help" => return Some(Command::Help),
        _ => {}
    }
    if let Some((cmd, rest)) = line.split_once(char::is_whitespace) {
        match cmd {
            "set" => {
                if let Some((var, expr)) = rest.split_once('=') {
                    return Some(Command::Set {
                        var: var.trim(),
                        expr: expr.trim(),
                    });
                }
            }
            "unset" => {
                return Some(Command::Unset { var: rest.trim() });
            }
            "render" => {
                return Some(Command::Render { template: rest });
            }
            _ => {}
        }
    }
    Some(Command::Invalid)
}

fn eval(
    env: &Environment,
    line: &str,
    ctx: &Value,
    locals: &BTreeMap<String, Value>,
) -> Option<Value> {
    match env.compile_expression(line).and_then(|expr| {
        expr.eval(context!(
            ..Value::from_iter(locals.iter().map(|x| (x.0.clone(), x.1.clone()))),
            ..ctx.clone()
        ))
    }) {
        Ok(rv) => Some(rv),
        Err(err) => {
            print_error(&Error::from(err));
            None
        }
    }
}

fn render(env: &Environment, template: &str, ctx: &Value, locals: &BTreeMap<String, Value>) {
    match env.render_str(
        template,
        context!(
            ..Value::from_iter(locals.iter().map(|x| (x.0.clone(), x.1.clone()))),
            ..ctx.clone()
        ),
    ) {
        Ok(rv) => {
            println!("{}", rv);
        }
        Err(err) => print_error(&Error::from(err)),
    }
}

fn print_result(value: &Value) {
    if value.is_undefined() {
        // nothing
    } else if let Some(s) = value.as_str() {
        println!("{:?}", s);
    } else if let Some(b) = value.as_bytes() {
        println!("{:?}", BytesRef(b));
    } else {
        println!("{}", value);
    }
}

fn print(value: Value) -> Value {
    println!("{}", value);
    Value::UNDEFINED
}

struct BytesRef<'x>(&'x [u8]);

impl fmt::Debug for BytesRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "b\"")?;
        for &b in self.0 {
            if b == b'\n' {
                write!(f, "\\n")?;
            } else if b == b'\r' {
                write!(f, "\\r")?;
            } else if b == b'\t' {
                write!(f, "\\t")?;
            } else if b == b'\\' || b == b'"' {
                write!(f, "\\{}", b as char)?;
            } else if b == b'\0' {
                write!(f, "\\0")?;
            // ASCII printable
            } else if (0x20..0x7f).contains(&b) {
                write!(f, "{}", b as char)?;
            } else {
                write!(f, "\\x{:02x}", b)?;
            }
        }
        write!(f, "\"")?;
        Ok(())
    }
}
