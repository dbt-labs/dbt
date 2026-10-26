//! The command field of the anonymous Vortex Invocation event.
//!
//! Extension commands format their typed arguments through Serde. File-system
//! arguments are identified from clap metadata, then hashed before formatting.

use std::borrow::Cow;
use std::collections::BTreeSet;

use clap::{CommandFactory, ValueHint};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::io_args::FsCommand;
use crate::node_selector::{MethodName, parse_single_selector};

/// A deterministic hash of the exact argument value, including directories.
/// Stable across runs without storing a salt. This is pseudonymization: a
/// known candidate can be checked by computing its hash.
fn hash_path_value(value: &[u8]) -> String {
    format!("h:{}", hex::encode(Sha256::digest(value)))
}

/// A static command name or serialized arguments with their paths hashed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryCommand(Cow<'static, str>);

impl TelemetryCommand {
    pub fn bare(name: FsCommand) -> Self {
        Self::bare_static(name.as_str())
    }

    pub fn bare_static(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }

    /// Format typed extension arguments, falling back to the command name if
    /// serialization fails (for example, a non-UTF-8 path).
    pub fn from_args<A: Serialize + CommandFactory>(name: &'static str, args: &A) -> Self {
        let Ok(mut value) = serde_json::to_value(args) else {
            return Self::bare_static(name);
        };
        let mut paths = BTreeSet::new();
        collect_path_args(&A::command(), &mut paths);
        sanitize_value(&mut value, None, &paths);
        Self(Cow::Owned(format!("{name} {value}")))
    }

    pub fn event_value(&self) -> &str {
        &self.0
    }
}

fn collect_path_args(command: &clap::Command, paths: &mut BTreeSet<String>) {
    paths.extend(
        command
            .get_arguments()
            .filter(|arg| {
                matches!(
                    arg.get_value_hint(),
                    ValueHint::AnyPath
                        | ValueHint::FilePath
                        | ValueHint::DirPath
                        | ValueHint::ExecutablePath
                        | ValueHint::Url
                )
            })
            .map(|arg| arg.get_id().to_string()),
    );
    for subcommand in command.get_subcommands() {
        collect_path_args(subcommand, paths);
    }
}

fn sanitize_value(value: &mut Value, field: Option<&str>, paths: &BTreeSet<String>) {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                sanitize_value(value, Some(name), paths);
            }
        }
        Value::Array(values) => {
            for value in values {
                sanitize_value(value, field, paths);
            }
        }
        Value::String(text) => {
            if field.is_some_and(|field| paths.contains(field)) {
                *text = hash_path_value(text.as_bytes());
            } else if field == Some("args") {
                *text = sanitize_argv_token(text);
            } else if matches!(field, Some("select" | "exclude")) && selector_contains_path(text) {
                *text = hash_path_value(text.as_bytes());
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn selector_contains_path(selector: &str) -> bool {
    selector
        .split(',')
        .any(|part| match parse_single_selector(part.trim()) {
            Ok(criteria) => matches!(criteria.method, MethodName::Path | MethodName::File),
            // A malformed selector must not expose a potential path in telemetry.
            Err(_) => true,
        })
}

/// Forwarded short clusters can contain attached values; hash the whole token
/// when a downstream parser would be needed to distinguish flags from values.
fn sanitize_argv_token(token: &str) -> String {
    let (flag, value) = token
        .split_once('=')
        .map_or((token, None), |(flag, value)| (flag, Some(value)));
    let is_flag = (flag.starts_with("--") || flag.len() == 2 && flag.starts_with('-'))
        && flag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
    if !is_flag {
        hash_path_value(token.as_bytes())
    } else if let Some(value) = value {
        format!("{flag}={}", hash_path_value(value.as_bytes()))
    } else {
        token.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use clap::Parser;

    use super::*;

    #[derive(Parser, Serialize, Default)]
    struct Args {
        #[arg(long)]
        file: Option<PathBuf>,
        #[arg(long)]
        select: Vec<String>,
        #[arg(long)]
        selector: Option<String>,
        #[arg(long)]
        label: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    }

    fn formatted(args: &Args) -> Value {
        let command = TelemetryCommand::from_args("lint", args);
        serde_json::from_str(command.event_value().strip_prefix("lint ").unwrap()).unwrap()
    }

    #[test]
    fn path_hashes_are_stable_and_keep_different_directories_distinct() {
        assert_eq!(
            hash_path_value(b"abc"),
            "h:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let first = Args {
            file: Some("/project-a/orders.sql".into()),
            ..Args::default()
        };
        let second = Args {
            file: Some("/project-b/orders.sql".into()),
            ..Args::default()
        };
        assert_ne!(formatted(&first)["file"], formatted(&second)["file"]);
    }

    #[test]
    fn hashes_paths_and_path_selectors_without_hashing_tags_or_named_selectors() {
        let args = Args {
            file: Some("/Users/jane/orders.sql".into()),
            select: [
                "tag:team/finance",
                r"tag:team\finance",
                "+path:/Users/jane/models+",
                "tag:daily,path:models",
                "orders.sql",
            ]
            .map(String::from)
            .to_vec(),
            selector: Some("team/finance".to_string()),
            label: "quotes \" and backslash \\ and Unicode λ".to_string(),
            ..Args::default()
        };
        let value = formatted(&args);
        assert!(value["file"].as_str().unwrap().starts_with("h:"));
        assert_eq!(value["select"][0], "tag:team/finance");
        assert_eq!(value["select"][1], r"tag:team\finance");
        for index in 2..5 {
            assert!(value["select"][index].as_str().unwrap().starts_with("h:"));
        }
        assert_eq!(value["selector"], "team/finance");
        assert_eq!(value["label"], args.label);
        assert!(!value.to_string().contains("jane"));
    }

    #[test]
    fn forwarded_arguments_hash_attached_values_and_preserve_flags() {
        let args = Args {
            args: [
                "-C/Users/jane/acme",
                "-iprivate.png",
                "--config=/Users/jane/config.yml",
                "/Users/jane/project",
                "--verbose",
                "-v",
            ]
            .map(String::from)
            .to_vec(),
            ..Args::default()
        };
        let value = formatted(&args);
        assert!(value["args"][0].as_str().unwrap().starts_with("h:"));
        assert!(value["args"][1].as_str().unwrap().starts_with("h:"));
        assert!(
            value["args"][2]
                .as_str()
                .unwrap()
                .starts_with("--config=h:")
        );
        assert!(value["args"][3].as_str().unwrap().starts_with("h:"));
        assert_eq!(value["args"][4], "--verbose");
        assert_eq!(value["args"][5], "-v");
        assert!(!value.to_string().contains("jane"));
        assert!(!value.to_string().contains("private.png"));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_path_falls_back_to_the_name_without_emitting_arguments() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let args = Args {
            file: Some(OsString::from_vec(b"/Users/j\xffne/orders.sql".to_vec()).into()),
            ..Args::default()
        };
        assert_eq!(
            TelemetryCommand::from_args("lint", &args).event_value(),
            "lint"
        );
    }
}
