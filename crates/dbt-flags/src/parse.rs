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

/// One boolean setting of a flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Assignment {
    /// Normalized: lowercase, `-` replaced by `_`.
    pub name: String,
    pub value: bool,
}

/// Parses lists of beta feature names. Each item of `lists` is one name or several separated
/// by commas.
///
/// Returns the names in order, normalized (lowercase, `-` replaced by `_`), plus a warning for
/// each entry that is not a name. `origin` names the source in warnings.
pub(crate) fn parse_names<'a>(
    lists: impl IntoIterator<Item = &'a str>,
    origin: &str,
) -> (Vec<String>, Vec<String>) {
    let mut names = Vec::new();
    let mut warnings = Vec::new();
    for entry in lists.into_iter().flat_map(|list| list.split(',')) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if entry.contains('=') {
            warnings.push(format!(
                "Ignoring '{entry}' in {origin}: list beta features by name. To turn a feature \
                 off, set its DBT_ENGINE_EXPERIMENTAL_<NAME> environment variable to 0"
            ));
            continue;
        }
        names.push(crate::def::normalize_name(entry));
    }
    (names, warnings)
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
    fn parse_names_splits_lists_and_rejects_values() {
        let (names, warnings) = parse_names(
            ["Metricflow-V2, other", "", "third", "bad=false"],
            "the test",
        );
        assert_eq!(names, ["metricflow_v2", "other", "third"]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("'bad=false' in the test"));
    }
}
