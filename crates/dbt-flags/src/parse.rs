/// Parses a boolean flag value with the same grammar as clap's
/// `BoolishValueParser`, which the CLI's boolean options already use.
///
/// Accepts `1/0`, `true/false`, `yes/no`, `on/off`, `t/f`, and `y/n`,
/// ignoring ASCII case and surrounding whitespace.
pub fn parse_bool(value: &str) -> Option<bool> {
    let value = value.trim();
    const TRUE: [&str; 6] = ["y", "yes", "t", "true", "on", "1"];
    const FALSE: [&str; 6] = ["n", "no", "f", "false", "off", "0"];
    if TRUE.iter().any(|t| t.eq_ignore_ascii_case(value)) {
        Some(true)
    } else if FALSE.iter().any(|f| f.eq_ignore_ascii_case(value)) {
        Some(false)
    } else {
        None
    }
}

/// One `NAME[=VALUE]` setting from the CLI or an env list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Assignment {
    /// Normalized: lowercase, `-` replaced by `_`.
    pub name: String,
    pub value: bool,
}

/// Parses `NAME[=VALUE]` settings. A bare `NAME` means `NAME=true`.
///
/// Returns the parsed settings in order plus a warning for each one that
/// could not be parsed. `origin` names the source in warnings.
pub(crate) fn parse_assignments<'a>(
    specs: impl IntoIterator<Item = &'a str>,
    origin: &str,
) -> (Vec<Assignment>, Vec<String>) {
    let mut assignments = Vec::new();
    let mut warnings = Vec::new();
    for spec in specs {
        let spec = spec.trim();
        if spec.is_empty() {
            continue;
        }
        let (name, value) = match spec.split_once('=') {
            Some((name, raw)) => match parse_bool(raw) {
                Some(value) => (name, value),
                None => {
                    warnings.push(format!(
                        "Ignoring feature setting '{spec}' from {origin}: \
                         value must be a boolean (1/0, true/false, yes/no, on/off)"
                    ));
                    continue;
                }
            },
            None => (spec, true),
        };
        let name = crate::def::normalize_name(name);
        if name.is_empty() {
            warnings.push(format!(
                "Ignoring feature setting '{spec}' from {origin}: missing flag name"
            ));
            continue;
        }
        assignments.push(Assignment { name, value });
    }
    (assignments, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bool_accepts_boolish_grammar() {
        for v in ["1", "true", "TRUE", "yes", "on", "t", "y", " true "] {
            assert_eq!(parse_bool(v), Some(true), "{v}");
        }
        for v in ["0", "false", "False", "no", "off", "f", "n"] {
            assert_eq!(parse_bool(v), Some(false), "{v}");
        }
        for v in ["", "2", "enabled", "truthy"] {
            assert_eq!(parse_bool(v), None, "{v}");
        }
    }

    #[test]
    fn parse_assignments_handles_bare_names_values_and_errors() {
        let (assignments, warnings) = parse_assignments(
            [
                "Git-Deps-Fast-Path",
                "other=false",
                "bad=maybe",
                "=true",
                "",
            ],
            "the test",
        );
        assert_eq!(
            assignments,
            vec![
                Assignment {
                    name: "git_deps_fast_path".to_string(),
                    value: true
                },
                Assignment {
                    name: "other".to_string(),
                    value: false
                },
            ]
        );
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("'bad=maybe'"));
        assert!(warnings[1].contains("missing flag name"));
    }
}
