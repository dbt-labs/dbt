//! Utilities for testing `impl Object` blocks of things across the codebase.

use minijinja::{
    value::{Object, Value},
    Environment,
};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Relax a jinja string for comparison by normalizing identation and trimming
pub fn relax_string_for_comparison(s: &str) -> String {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\s*\n\s*)+").unwrap());
    RE.replace_all(s, "\n").trim().to_string()
}

/// Add `value` to a Jinja env global called `obj`.
///
/// Then render `template` in that Jinja env and assert it matches the expected.
pub fn jinja_diff(
    value: impl Object + 'static,
    template: &'static str,
    expect: &'static str,
) -> Option<String> {
    let mut env = Environment::new();
    env.add_global("obj", Value::from_object(value));
    let got = env
        .render_str(template, HashMap::<String, String>::new(), &[])
        .unwrap();

    let got_relax = relax_string_for_comparison(&got);
    let expect_relax = relax_string_for_comparison(expect);

    if expect_relax != got_relax {
        Some(format!(
            "----- Expected -----\n\"{expect_relax}\"\n\n-----    Got   -----\n\"{got_relax}\"\n"
        ))
    } else {
        None
    }
}

/// Add `value` to a Jinja env global called `obj`.
///
/// Then render `template` in that Jinja env and assert it matches the expected.
pub fn jinja_assert(value: impl Object + 'static, template: &'static str, expect: &'static str) {
    let diff = jinja_diff(value, template, expect);
    if let Some(diff) = diff {
        panic!("{diff}")
    }
}
