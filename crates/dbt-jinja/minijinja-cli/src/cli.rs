use std::borrow::Cow;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::{fs, io};

use anyhow::{bail, Context, Error};
use clap::ArgMatches;
use minijinja::machinery::{
    get_compiled_template, parse, tokenize, Instructions, WhitespaceConfig,
};
use minijinja::{context, Environment, Error as MError, ErrorKind, Value};
use serde::Deserialize;

#[cfg(not(feature = "json5"))]
use serde_json as preferred_json;
#[cfg(feature = "json5")]
use serde_json5 as preferred_json;

#[cfg(windows)]
use dunce::canonicalize;
#[cfg(not(windows))]
use std::fs::canonicalize;

use crate::command::{make_command, SUPPORTED_FORMATS};
use crate::config::Config;
use crate::output::{Output, STDIN_STDOUT};

fn load_config(matches: &ArgMatches) -> Result<Config, Error> {
    #[allow(unused_mut)] // feature-flag dependent
    let mut config = None::<Config>;
    #[cfg(feature = "toml")]
    {
        let config_path = if let Some(path) = matches.get_one::<PathBuf>("config-file") {
            Some(Cow::Borrowed(path.as_path()))
        } else if let Some(var) = std::env::var_os("MINIJINJA_CONFIG_FILE") {
            Some(Cow::Owned(PathBuf::from(var)))
        } else {
            home::home_dir().map(|home_dir| Cow::Owned(home_dir.join(".minijinja.toml")))
        };

        if let Some(config_path) = config_path {
            if config_path.is_file() {
                config = Some(
                    Config::load_from_toml(&config_path)
                        .with_context(|| format!("unable to load '{}'", config_path.display()))?,
                );
            }
        }
    }
    let mut config = config.unwrap_or_default();
    config.update_from_env()?;
    config.update_from_matches(matches)?;
    Ok(config)
}

fn detect_format_from_path(path: &Path) -> Result<&'static str, Error> {
    if let Some(ext) = path.extension().and_then(|x| x.to_str()) {
        for (fmt, _, exts) in SUPPORTED_FORMATS {
            if exts.contains(&ext.to_ascii_lowercase().as_str()) {
                return Ok(fmt);
            }
        }
    }
    bail!("cannot auto detect format from extension");
}

fn load_data(
    format: &str,
    path: &Path,
    selector: Option<&str>,
) -> Result<(BTreeMap<String, Value>, bool), Error> {
    let (contents, stdin_used) = if path == Path::new(STDIN_STDOUT) {
        let mut buf = Vec::<u8>::new();
        io::stdin()
            .read_to_end(&mut buf)
            .context("unable to read data from stdin")?;
        (buf, true)
    } else {
        (
            fs::read(path)
                .with_context(|| format!("unable to read data file '{}'", path.display()))?,
            false,
        )
    };
    let format = if format == "auto" {
        if stdin_used {
            bail!("auto detection does not work with data from stdin");
        } else {
            detect_format_from_path(path)?
        }
    } else {
        format
    };

    let mut data: Value = match format {
        "json" => preferred_json::from_slice(&contents)?,
        #[cfg(feature = "querystring")]
        "querystring" => Value::from(serde_qs::from_bytes::<BTreeMap<String, Value>>(&contents)?),
        #[cfg(feature = "yaml")]
        "yaml" => {
            // for merge keys to work we need to manually call `apply_merge`.
            // For this reason we need to deserialize into a serde_yml::Value
            // before converting it into a final value.
            let mut v: serde_yml::Value = serde_yml::from_slice(&contents)?;
            v.apply_merge()?;
            Value::from_serialize(v)
        }
        #[cfg(feature = "toml")]
        "toml" => {
            let contents = String::from_utf8(contents).context("invalid utf-8")?;
            toml::from_str(&contents)?
        }
        #[cfg(feature = "cbor")]
        "cbor" => ciborium::from_reader(&contents[..])?,
        #[cfg(feature = "ini")]
        "ini" => {
            let contents = String::from_utf8(contents).context("invalid utf-8")?;
            let mut config = configparser::ini::Ini::new_cs();
            config
                .read(contents)
                .map_err(|msg| anyhow::anyhow!("could not load ini: {}", msg))?;
            Value::from_serialize(config.get_map_ref())
        }
        other => bail!("Unknown format '{}'", other),
    };

    if let Some(selector) = selector {
        for part in selector.split('.') {
            data = if let Ok(idx) = part.parse::<usize>() {
                data.get_item_by_index(idx)
            } else {
                data.get_attr(part)
            }
            .with_context(|| {
                format!(
                    "unable to select {:?} in {:?} (value was {})",
                    part,
                    selector,
                    data.kind()
                )
            })?
            .clone();
        }
    }

    Ok((
        Deserialize::deserialize(data).context("failed to interpret input data as object")?,
        stdin_used,
    ))
}

fn create_env(
    config: &Config,
    cwd: PathBuf,
    template_name: &str,
    template_source: Option<String>,
    stdin_used_for_data: bool,
) -> Result<Environment<'static>, Error> {
    let mut env = Environment::new();
    env.set_debug(true);
    config.apply_to_env(&mut env)?;

    env.set_path_join_callback(move |name, parent| {
        let p = if parent == STDIN_STDOUT {
            cwd.join(name)
        } else {
            Path::new(parent)
                .parent()
                .unwrap_or(Path::new(""))
                .join(name)
        };
        canonicalize(&p)
            .unwrap_or(p)
            .to_string_lossy()
            .to_string()
            .into()
    });

    let cached_stdin = Mutex::new(None);
    let safe_paths = config.safe_paths();
    let allow_include = config.allow_include();
    let template_name = template_name.to_string();
    env.set_loader(move |name| -> Result<Option<String>, MError> {
        if !allow_include && name != template_name {
            return Ok(None);
        }

        if name == STDIN_STDOUT {
            if stdin_used_for_data {
                return Err(MError::new(
                    ErrorKind::InvalidOperation,
                    "cannot load template from stdin when data is from stdin",
                ));
            }

            let mut stdin = cached_stdin.lock().unwrap();
            if stdin.is_none() {
                *stdin = Some(io::read_to_string(io::stdin()).map_err(|err| {
                    MError::new(
                        ErrorKind::InvalidOperation,
                        "failed to read template from stdin",
                    )
                    .with_source(err)
                })?);
            }
            return Ok(stdin.clone());
        } else if name == template_name {
            if let Some(ref source) = template_source {
                return Ok(Some(source.clone()));
            }
        }

        let fs_name = Path::new(name);
        if !safe_paths.is_empty() && !safe_paths.iter().any(|x| fs_name.starts_with(x)) {
            return Err(MError::new(
                ErrorKind::InvalidOperation,
                "Cannot include template from non-trusted path",
            ));
        }

        match fs::read_to_string(name) {
            Ok(contents) => Ok(Some(contents)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(
                MError::new(ErrorKind::TemplateNotFound, "cannot find template").with_source(err),
            ),
        }
    });

    Ok(env)
}

#[cfg(feature = "completions")]
fn generate_completions(shell: &str) -> Result<i32, Error> {
    macro_rules! gen {
        ($shell:expr) => {
            clap_complete::generate(
                $shell,
                &mut make_command(),
                "minijinja-cli",
                &mut std::io::stdout(),
            )
        };
    }

    match shell {
        "bash" => gen!(clap_complete::Shell::Bash),
        "zsh" => gen!(clap_complete::Shell::Zsh),
        "elvish" => gen!(clap_complete::Shell::Elvish),
        "fish" => gen!(clap_complete::Shell::Fish),
        "powershell" => gen!(clap_complete::Shell::PowerShell),
        "nushell" => gen!(clap_complete_nushell::Nushell),
        "fig" => gen!(clap_complete_fig::Fig),
        _ => unreachable!(),
    };

    Ok(0)
}

fn dump_info(
    dump: &str,
    env: &Environment<'_>,
    template: &str,
    output: &mut Output,
    config: &Config,
) -> Result<(), Error> {
    match dump {
        "ast" => {
            let tmpl = env.get_template(template)?;
            writeln!(
                output,
                "{:#?}",
                parse(
                    tmpl.source(),
                    tmpl.name(),
                    Default::default(),
                    Default::default()
                )?
            )?;
        }
        "tokens" => {
            let tmpl = env.get_template(template)?;
            let tokens: Result<Vec<_>, _> = tokenize(
                tmpl.source(),
                false,
                Default::default(),
                WhitespaceConfig {
                    lstrip_blocks: config.lstrip_blocks(),
                    trim_blocks: config.trim_blocks(),
                    ..Default::default()
                },
            )
            .collect();
            for (token, _) in tokens? {
                writeln!(output, "{:?}", token)?;
            }
        }
        "instructions" => {
            let tmpl = env.get_template(template)?;
            let ctmpl = get_compiled_template(&tmpl);
            for (block_name, instructions) in ctmpl.blocks.iter() {
                print_instructions(output, instructions, block_name)?;
            }
            print_instructions(output, &ctmpl.instructions, "<root>")?;
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn print_instructions(
    output: &mut Output,
    instructions: &Instructions,
    block_name: &str,
) -> Result<(), Error> {
    writeln!(output, "Block: {block_name:?}")?;
    for idx in 0.. {
        if let Some(instruction) = instructions.get(idx) {
            writeln!(output, "  {idx:4}: {instruction:?}")?;
        } else {
            break;
        }
    }
    Ok(())
}

fn print_expr_out(rv: Value, config: &Config, output: &mut Output) -> Result<i32, Error> {
    match config.expr_out() {
        "print" => writeln!(output, "{}", rv)?,
        "json" => writeln!(output, "{}", serde_json::to_string(&rv)?)?,
        "json-pretty" => writeln!(output, "{}", serde_json::to_string_pretty(&rv)?)?,
        "status" => {
            return Ok(if let Ok(n) = i32::try_from(rv.clone()) {
                n
            } else if rv.is_true() {
                0
            } else {
                1
            });
        }
        other => bail!("unknown expr-out '{}'", other),
    }
    Ok(0)
}

pub fn print_error(err: &Error) {
    eprintln!("error: {err}");
    if let Some(err) = err.downcast_ref::<MError>() {
        if err.name().is_some() {
            eprintln!("{}", err.display_debug_info());
        }
    }
    let mut source_opt = err.source();
    while let Some(source) = source_opt {
        eprintln!();
        eprintln!("caused by: {source}");
        if let Some(source) = source.downcast_ref::<MError>() {
            if source.name().is_some() {
                eprintln!("{}", source.display_debug_info());
            }
        }
        source_opt = source.source();
    }
}

#[cfg(feature = "toml")]
fn print_config(config: &Config) -> Result<i32, Error> {
    let out = toml::to_string_pretty(config)?;
    println!("{}", out);
    Ok(0)
}

pub fn execute() -> Result<i32, Error> {
    let matches = make_command().get_matches();
    let config = load_config(&matches)?;

    if matches.get_flag("syntax-help") {
        println!("{}", include_str!("syntax_help.txt"));
        return Ok(0);
    }

    #[cfg(feature = "completions")]
    {
        if let Some(shell) = matches.get_one::<String>("generate-completion") {
            return generate_completions(shell);
        }
    }
    #[cfg(feature = "toml")]
    {
        if matches.get_flag("print-config") {
            return print_config(&config);
        }
    }

    let (base_ctx, stdin_used) = if let Some(data) = matches.get_one::<PathBuf>("data_file") {
        load_data(
            config.format(),
            data,
            matches.get_one::<String>("select").map(|x| x.as_str()),
        )?
    } else {
        (Default::default(), false)
    };

    let cwd = std::env::current_dir()?;
    let ctx = context!(..config.defines(), ..base_ctx);

    let (template_name, template_source) = match (
        matches.get_one::<String>("template"),
        matches
            .get_one::<String>("template_file")
            .map(|x| x.as_str()),
    ) {
        (None, Some(STDIN_STDOUT)) => (Cow::Borrowed(STDIN_STDOUT), None),
        (None, Some("")) => bail!("Empty template names are only valid with --template."),
        (None, Some(rel_name)) => (
            Cow::Owned(cwd.join(rel_name).to_string_lossy().to_string()),
            None,
        ),
        (Some(source), None | Some("")) => (Cow::Borrowed("<string>"), Some(source.clone())),
        _ => bail!("When --template is used, a template cannot be passed as argument (only an empty argument is allowed)."),
    };

    let mut output = Output::new(matches.get_one::<PathBuf>("output").unwrap())?;

    let env = create_env(&config, cwd, &template_name, template_source, stdin_used)?;
    let mut exit_code = 0;

    if let Some(expr) = matches.get_one::<String>("expr") {
        let rv = env.compile_expression(expr)?.eval(ctx)?;
        exit_code = print_expr_out(rv, &config, &mut output)?;
    } else if let Some(dump) = matches.get_one::<String>("dump") {
        dump_info(dump, &env, &template_name, &mut output, &config)?;
    } else if cfg!(feature = "repl") && matches.get_flag("repl") {
        #[cfg(feature = "repl")]
        {
            crate::repl::run(env, ctx)?;
        }
    } else {
        let result = env.get_template(&template_name)?.render(ctx)?;
        if !config.newline() {
            write!(&mut output, "{result}")?;
        } else {
            writeln!(&mut output, "{result}")?;
        }
    }

    output.commit()?;
    Ok(exit_code)
}
