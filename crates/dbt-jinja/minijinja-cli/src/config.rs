use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Error};
use clap::ArgMatches;
use minijinja::syntax::SyntaxConfig;
use minijinja::{AutoEscape, Environment, UndefinedBehavior, Value};
use serde::{Deserialize, Serialize};

/// Overrides specific syntax settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct SyntaxElements {
    block_start: String,
    block_end: String,
    variable_start: String,
    variable_end: String,
    comment_start: String,
    comment_end: String,
    line_statement_prefix: String,
    line_comment_prefix: String,
}

impl Default for SyntaxElements {
    fn default() -> Self {
        SyntaxElements {
            block_start: "{%".to_string(),
            block_end: "%}".to_string(),
            variable_start: "{{".to_string(),
            variable_end: "}}".to_string(),
            comment_start: "{#".to_string(),
            comment_end: "#}".to_string(),
            line_statement_prefix: "".to_string(),
            line_comment_prefix: "".to_string(),
        }
    }
}

/// Holds in-memory config state for the execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", default)]
pub struct Config {
    format: String,
    autoescape: String,
    include: bool,
    newline: bool,
    trim_blocks: bool,
    lstrip_blocks: bool,
    py_compat: bool,
    env: bool,
    strict: bool,
    safe_paths: Vec<PathBuf>,
    expr_out: String,
    fuel: u64,
    syntax: SyntaxElements,
    defines: Arc<BTreeMap<String, Value>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            format: "auto".to_string(),
            autoescape: "auto".to_string(),
            include: true,
            newline: true,
            trim_blocks: false,
            lstrip_blocks: false,
            py_compat: false,
            env: Default::default(),
            strict: false,
            defines: Default::default(),
            syntax: Default::default(),
            safe_paths: Default::default(),
            expr_out: "print".to_string(),
            fuel: 0,
        }
    }
}

impl Config {
    pub fn update_from_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        if let Some(format) = matches.get_one::<String>("format") {
            self.format = format.clone();
        }
        if let Some(autoescape) = matches.get_one::<String>("autoescape") {
            self.autoescape = autoescape.clone();
        }
        if let Some(expr_out) = matches.get_one::<String>("expr-out") {
            self.expr_out = expr_out.clone();
        }
        if matches.get_flag("no-include") {
            self.include = false;
        }
        if matches.get_flag("no-newline") {
            self.newline = false;
        }
        if matches.get_flag("trim-blocks") {
            self.trim_blocks = true;
        }
        if matches.get_flag("lstrip-blocks") {
            self.lstrip_blocks = true;
        }
        #[cfg(feature = "contrib")]
        {
            if matches.get_flag("py-compat") {
                self.py_compat = true;
            }
        }
        if matches.get_flag("env") {
            self.env = true;
        }
        if matches.get_flag("strict") {
            self.strict = true;
        }
        if let Some(fuel) = matches.get_one::<u64>("fuel") {
            if *fuel > 0 {
                self.fuel = *fuel;
            }
        }

        self.safe_paths.extend(
            matches
                .get_many::<PathBuf>("safe-path")
                .unwrap_or_default()
                .map(|x| x.canonicalize().unwrap_or_else(|_| x.clone())),
        );

        self.update_syntax_from_matches(matches)?;
        self.add_defines_from_matches(matches)?;
        Ok(())
    }

    #[cfg(feature = "toml")]
    pub fn load_from_toml(p: &std::path::Path) -> Result<Config, Error> {
        let contents = std::fs::read_to_string(p)?;
        let cfg: Config = toml::from_str(&contents)?;
        Ok(cfg)
    }

    pub fn update_from_env(&mut self) -> Result<(), Error> {
        if let Ok(format) = env::var("MINIJINJA_FORMAT") {
            self.format = format;
        }
        if let Ok(autoescape) = env::var("MINIJINJA_AUTOESCAPE") {
            self.autoescape = autoescape;
        }
        if let Ok(include) = env::var("MINIJINJA_INCLUDE") {
            self.include = parse_env_bool(&include, "MINIJINJA_INCLUDE")?;
        }
        if let Ok(newline) = env::var("MINIJINJA_NEWLINE") {
            self.newline = parse_env_bool(&newline, "MINIJINJA_NEWLINE")?;
        }
        if let Ok(trim_blocks) = env::var("MINIJINJA_TRIM_BLOCKS") {
            self.trim_blocks = parse_env_bool(&trim_blocks, "MINIJINJA_TRIM_BLOCKS")?;
        }
        if let Ok(lstrip_blocks) = env::var("MINIJINJA_LSTRIP_BLOCKS") {
            self.lstrip_blocks = parse_env_bool(&lstrip_blocks, "MINIJINJA_LSTRIP_BLOCKS")?;
        }
        if let Ok(py_compat) = env::var("MINIJINJA_PY_COMPAT") {
            self.py_compat = parse_env_bool(&py_compat, "MINIJINJA_PY_COMPAT")?;
        }
        if let Ok(env_flag) = env::var("MINIJINJA_ENV") {
            self.env = parse_env_bool(&env_flag, "MINIJINJA_ENV")?;
        }
        if let Ok(strict) = env::var("MINIJINJA_STRICT") {
            self.strict = parse_env_bool(&strict, "MINIJINJA_STRICT")?;
        }
        if let Ok(expr_out) = env::var("MINIJINJA_EXPR_OUT") {
            self.expr_out = expr_out;
        }
        if let Ok(fuel) = env::var("MINIJINJA_FUEL") {
            if let Ok(fuel_value) = fuel.parse::<u64>() {
                if fuel_value > 0 {
                    self.fuel = fuel_value;
                }
            }
        }
        if let Ok(safe_paths) = env::var("MINIJINJA_SAFE_PATHS") {
            self.safe_paths = safe_paths
                .split(if cfg!(windows) { ';' } else { ':' })
                .map(PathBuf::from)
                .collect();
        }
        if let Ok(syntax) = env::var("MINIJINJA_SYNTAX") {
            self.update_syntax_from_pairs(syntax.split_whitespace())?;
        }
        Ok(())
    }

    pub fn allow_include(&self) -> bool {
        self.include
    }

    pub fn trim_blocks(&self) -> bool {
        self.trim_blocks
    }

    pub fn lstrip_blocks(&self) -> bool {
        self.lstrip_blocks
    }

    pub fn newline(&self) -> bool {
        self.newline
    }

    pub fn format(&self) -> &str {
        &self.format
    }

    pub fn expr_out(&self) -> &str {
        &self.expr_out
    }

    pub fn defines(&self) -> Value {
        Value::from_dyn_object(self.defines.clone())
    }

    pub fn safe_paths(&self) -> Vec<PathBuf> {
        self.safe_paths.clone()
    }

    pub fn apply_to_env(&self, env: &mut Environment) -> Result<(), Error> {
        if self.env {
            env.add_global("ENV", Value::from_iter(std::env::vars()));
        }
        env.set_trim_blocks(self.trim_blocks);
        env.set_lstrip_blocks(self.lstrip_blocks);
        if self.fuel > 0 {
            env.set_fuel(Some(self.fuel));
        }

        #[cfg(feature = "contrib")]
        {
            minijinja_contrib::add_to_environment(env);
            if self.py_compat {
                env.set_unknown_method_callback(
                    minijinja_contrib::pycompat::unknown_method_callback,
                );
            }
        }

        let autoescape = self.autoescape.clone();
        env.set_auto_escape_callback(move |name| match autoescape.as_str() {
            "none" => AutoEscape::None,
            "html" => AutoEscape::Html,
            "json" => AutoEscape::Json,
            "auto" => match name.strip_suffix(".j2").unwrap_or(name).rsplit('.').next() {
                Some("htm" | "html" | "xml" | "xhtml") => AutoEscape::Html,
                Some("json" | "json5" | "yml" | "yaml") => AutoEscape::Json,
                _ => AutoEscape::None,
            },
            _ => unreachable!(),
        });
        env.set_undefined_behavior(if self.strict {
            UndefinedBehavior::Strict
        } else {
            UndefinedBehavior::Lenient
        });
        env.set_syntax(self.make_syntax()?);
        Ok(())
    }

    fn make_syntax(&self) -> Result<SyntaxConfig, Error> {
        let s = &self.syntax;
        SyntaxConfig::builder()
            .block_delimiters(s.block_start.clone(), s.block_end.clone())
            .variable_delimiters(s.variable_start.clone(), s.variable_end.clone())
            .comment_delimiters(s.comment_start.clone(), s.comment_end.clone())
            .line_statement_prefix(s.line_statement_prefix.clone())
            .line_comment_prefix(s.line_comment_prefix.clone())
            .build()
            .context("could not configure syntax")
    }

    fn update_syntax_from_pairs<'a, I>(&mut self, iter: I) -> Result<(), Error>
    where
        I: Iterator<Item = &'a str>,
    {
        let s = &mut self.syntax;

        for pair in iter {
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| anyhow!("syntax feature needs to be a key=value pair"))?;

            *match key {
                "block-start" => &mut s.block_start,
                "block-end" => &mut s.block_end,
                "variable-start" => &mut s.variable_start,
                "variable-end" => &mut s.variable_end,
                "comment-start" => &mut s.comment_start,
                "comment-end" => &mut s.comment_end,
                "line-statement-prefix" => &mut s.line_statement_prefix,
                "line-comment-prefix" => &mut s.line_comment_prefix,
                _ => bail!("unknown syntax feature '{}'", key),
            } = value.to_string();
        }

        Ok(())
    }

    fn update_syntax_from_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let mut iter = matches.get_many::<String>("syntax");
        if let Some(ref mut iter) = iter {
            self.update_syntax_from_pairs(iter.map(|x| x.as_str()))?;
        }
        Ok(())
    }

    fn add_defines_from_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let defines = Arc::make_mut(&mut self.defines);
        if let Some(items) = matches.get_many::<String>("define") {
            for item in items {
                if let Some((key, raw_value)) = item.split_once(":=") {
                    defines.insert(key.to_string(), interpret_raw_value(raw_value)?);
                } else if let Some((key, string_value)) = item.split_once('=') {
                    defines.insert(key.to_string(), Value::from(string_value));
                } else {
                    defines.insert(item.to_string(), Value::from(true));
                }
            }
        }
        Ok(())
    }
}

fn interpret_raw_value(s: &str) -> Result<Value, Error> {
    #[cfg(not(feature = "yaml"))]
    mod imp {
        pub use serde_json::from_str;
        pub const FMT: &str = "JSON/YAML";
    }
    #[cfg(feature = "yaml")]
    mod imp {
        pub use serde_yml::from_str;
        pub const FMT: &str = "JSON";
    }
    imp::from_str::<Value>(s)
        .with_context(|| format!("invalid raw value '{}' (not valid {})", s, imp::FMT))
}

fn parse_env_bool(s: &str, var_name: &str) -> Result<bool, Error> {
    match s.to_lowercase().as_str() {
        "0" | "false" | "no" | "off" => Ok(false),
        "1" | "true" | "yes" | "on" => Ok(true),
        _ => bail!("Invalid boolean value for {}: {}", var_name, s),
    }
}
