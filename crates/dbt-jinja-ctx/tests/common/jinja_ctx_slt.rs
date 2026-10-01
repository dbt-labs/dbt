//! SLT-style declarative harness for cross-phase Jinja context comparisons.
//!
//! A `.slt` file describes how individual context keys (or sub-paths like
//! `target.name`, `flags.FULL_REFRESH`) appear in each phase's typed context.
//! The harness loads each phase's fixture (a `Serialize`-backed typed ctx
//! constructed elsewhere — passed in as a `PhaseFixtures` map of pre-built
//! `MinijinjaValue`s) and asserts every key/expected pair.
//!
//! # Format
//!
//! ```text
//! # Comments start with `#`.
//!
//! key target.name
//! ----
//! load     dev
//! resolve  dev
//! compile  dev
//! run      dev
//!
//! key execute
//! ----
//! load     <absent>
//! resolve  false
//! compile  true
//! run      true
//! ```
//!
//! ## Phase columns
//! `load`, `resolve_globals`, `resolve_base`, `resolve_model`,
//! `compile_base`, `compile_node`, `run_node`.
//! Short aliases (`resolve` for `resolve_model`, `compile` for `compile_node`,
//! `run` for `run_node`) are accepted.
//!
//! ## Expected values
//! * `<absent>` — the key path resolves to `Value::UNDEFINED` or is missing.
//! * `<present>` — the value exists (any non-undefined value).
//! * `<object>` — the value is an `Object`-typed jinja value.
//! * `<skip>` — don't check this phase for this row.
//! * `true` / `false` / `null` — JSON booleans / null.
//! * Decimal integers / floats — JSON numbers.
//! * Anything else — the literal string content (no quoting).
//!
//! Comparisons go through `serde_json::to_value(&minijinja_value)` so the
//! semantics match what JsonSchema documents.

#![allow(dead_code)] // shared harness; specific test files use only some of it.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use minijinja::Value as MinijinjaValue;

/// A phase the harness knows about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Phase {
    Load,
    ResolveGlobals,
    ResolveBase,
    ResolveModel,
    CompileBase,
    CompileNode,
    RunNode,
}

impl Phase {
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "load" => Some(Phase::Load),
            "resolve_globals" => Some(Phase::ResolveGlobals),
            "resolve_base" => Some(Phase::ResolveBase),
            "resolve_model" | "resolve" => Some(Phase::ResolveModel),
            "compile_base" => Some(Phase::CompileBase),
            "compile_node" | "compile" => Some(Phase::CompileNode),
            "run_node" | "run" => Some(Phase::RunNode),
            _ => None,
        }
    }
}

/// Pre-serialized phase contexts. Producers construct each `LoadCtx` /
/// `ResolveCore` / etc., call `MinijinjaValue::from_serialize(&ctx)`, and
/// store the resulting Value here. Phases that aren't yet implemented (or
/// that a particular fixture doesn't cover) are left as `None`; the harness
/// silently skips rows referencing absent fixtures.
#[derive(Default)]
pub struct PhaseFixtures {
    pub load: Option<MinijinjaValue>,
    pub resolve_globals: Option<MinijinjaValue>,
    pub resolve_base: Option<MinijinjaValue>,
    pub resolve_model: Option<MinijinjaValue>,
    pub compile_base: Option<MinijinjaValue>,
    pub compile_node: Option<MinijinjaValue>,
    pub run_node: Option<MinijinjaValue>,
}

impl PhaseFixtures {
    pub fn fixture(&self, phase: Phase) -> Option<&MinijinjaValue> {
        match phase {
            Phase::Load => self.load.as_ref(),
            Phase::ResolveGlobals => self.resolve_globals.as_ref(),
            Phase::ResolveBase => self.resolve_base.as_ref(),
            Phase::ResolveModel => self.resolve_model.as_ref(),
            Phase::CompileBase => self.compile_base.as_ref(),
            Phase::CompileNode => self.compile_node.as_ref(),
            Phase::RunNode => self.run_node.as_ref(),
        }
    }
}

#[derive(Debug, Clone)]
enum Expected {
    Absent,
    Present,
    Object,
    Skip,
    Json(serde_json::Value),
}

impl Expected {
    fn parse(raw: &str) -> Self {
        let s = raw.trim();
        match s {
            "<absent>" => Expected::Absent,
            "<present>" => Expected::Present,
            "<object>" => Expected::Object,
            "<skip>" => Expected::Skip,
            "true" => Expected::Json(serde_json::Value::Bool(true)),
            "false" => Expected::Json(serde_json::Value::Bool(false)),
            "null" => Expected::Json(serde_json::Value::Null),
            // Quoted-string form: `""` → empty string; `"foo bar"` → `foo bar`.
            // Necessary because `s.parse::<i64>()` matches `""` (no), and a
            // raw `""` literal at the parser level otherwise becomes the
            // 2-character string `""`.
            quoted if quoted.starts_with('"') && quoted.ends_with('"') && quoted.len() >= 2 => {
                Expected::Json(serde_json::Value::String(
                    quoted[1..quoted.len() - 1].to_string(),
                ))
            }
            _ => {
                if let Ok(n) = s.parse::<i64>() {
                    Expected::Json(serde_json::Value::Number(n.into()))
                } else if let Ok(n) = s.parse::<f64>() {
                    if let Some(num) = serde_json::Number::from_f64(n) {
                        Expected::Json(serde_json::Value::Number(num))
                    } else {
                        Expected::Json(serde_json::Value::String(s.to_string()))
                    }
                } else {
                    Expected::Json(serde_json::Value::String(s.to_string()))
                }
            }
        }
    }
}

#[derive(Debug)]
struct Row {
    line: usize,
    key_path: String,
    expected_per_phase: BTreeMap<Phase, (usize, Expected)>,
}

#[derive(Debug)]
struct SltFile {
    path: String,
    rows: Vec<Row>,
}

fn parse_slt(text: &str, path: &str) -> Result<SltFile, String> {
    let mut rows: Vec<Row> = Vec::new();
    let mut current: Option<Row> = None;
    let mut in_expectations = false;

    for (idx, raw_line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim_end();
        let trimmed = line.trim();

        if trimmed.is_empty() {
            if let Some(row) = current.take() {
                rows.push(row);
            }
            in_expectations = false;
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("key ") {
            if let Some(row) = current.take() {
                rows.push(row);
            }
            current = Some(Row {
                line: line_no,
                key_path: rest.trim().to_string(),
                expected_per_phase: BTreeMap::new(),
            });
            in_expectations = false;
            continue;
        }

        if trimmed.chars().all(|c| c == '-') && trimmed.len() >= 3 {
            in_expectations = true;
            continue;
        }

        if !in_expectations {
            return Err(format!(
                "{path}:{line_no}: unexpected content outside a key block: `{trimmed}`"
            ));
        }

        let row = current.as_mut().ok_or_else(|| {
            format!("{path}:{line_no}: expectation row before any `key` directive")
        })?;
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let phase_label = parts.next().unwrap_or("");
        let expected_raw = parts.next().unwrap_or("").trim();
        if expected_raw.is_empty() {
            return Err(format!(
                "{path}:{line_no}: expectation row missing value: `{trimmed}`"
            ));
        }
        let phase = Phase::from_label(phase_label)
            .ok_or_else(|| format!("{path}:{line_no}: unknown phase `{phase_label}`"))?;
        if row.expected_per_phase.contains_key(&phase) {
            return Err(format!(
                "{path}:{line_no}: phase `{phase_label}` listed twice for key `{}`",
                row.key_path
            ));
        }
        row.expected_per_phase
            .insert(phase, (line_no, Expected::parse(expected_raw)));
    }

    if let Some(row) = current.take() {
        rows.push(row);
    }

    Ok(SltFile {
        path: path.to_string(),
        rows,
    })
}

/// Walk a `MinijinjaValue` along a dotted key path. Returns `None` if any
/// segment is missing or undefined.
fn walk(root: &MinijinjaValue, key_path: &str) -> Option<MinijinjaValue> {
    let mut current = root.clone();
    for segment in key_path.split('.') {
        if current.is_undefined() {
            return None;
        }
        let attr = current.get_attr(segment).ok()?;
        if attr.is_undefined() {
            return None;
        }
        current = attr;
    }
    Some(current)
}

fn check_row(row: &Row, fixtures: &PhaseFixtures, errors: &mut Vec<String>, file_path: &str) {
    for (&phase, (line_no, expected)) in &row.expected_per_phase {
        if matches!(expected, Expected::Skip) {
            continue;
        }
        let Some(ctx) = fixtures.fixture(phase) else {
            // No fixture supplied for this phase — silently skip so .slt files
            // can list rows for phases that haven't been implemented yet
            // without forcing every fixture to cover everything.
            continue;
        };

        let resolved = walk(ctx, &row.key_path);
        let mismatch = match (expected, &resolved) {
            (Expected::Absent, None) => None,
            (Expected::Absent, Some(v)) => Some(format!(
                "expected <absent>, got value `{}`",
                debug_render(v)
            )),
            (Expected::Present, Some(v)) if !v.is_undefined() => None,
            (Expected::Present, _) => Some("expected <present>, got <absent>".to_string()),
            (Expected::Object, Some(v)) if v.as_object().is_some() => None,
            (Expected::Object, Some(v)) => Some(format!(
                "expected <object>, got non-object value `{}`",
                debug_render(v)
            )),
            (Expected::Object, None) => Some("expected <object>, got <absent>".to_string()),
            (Expected::Json(_), None) => Some("expected a value, got <absent>".to_string()),
            (Expected::Json(want), Some(actual)) => {
                let actual_json = match serde_json::to_value(actual) {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(format!(
                            "{file_path}:{line_no}: key `{key}`: phase `{phase:?}`: \
                             could not serialize value to JSON: {e}",
                            key = row.key_path,
                        ));
                        continue;
                    }
                };
                if &actual_json == want {
                    None
                } else {
                    Some(format!("expected `{}`, got `{}`", want, actual_json,))
                }
            }
            (Expected::Skip, _) => unreachable!(),
        };

        if let Some(reason) = mismatch {
            errors.push(format!(
                "{file_path}:{line_no}: key `{key}`: phase `{phase:?}`: {reason}",
                key = row.key_path,
            ));
        }
    }
}

fn debug_render(v: &MinijinjaValue) -> String {
    serde_json::to_value(v)
        .map(|j| j.to_string())
        .unwrap_or_else(|_| format!("{v:?}"))
}

/// Run a single `.slt` file against the supplied phase fixtures. Panics with
/// a multi-line failure report listing every mismatch.
pub fn run_slt(path: impl AsRef<Path>, fixtures: &PhaseFixtures) {
    let path_ref = path.as_ref();
    let path_str = path_ref.to_string_lossy().to_string();
    let text =
        fs::read_to_string(path_ref).unwrap_or_else(|e| panic!("failed to read {path_str}: {e}"));
    let slt =
        parse_slt(&text, &path_str).unwrap_or_else(|e| panic!("failed to parse {path_str}:\n{e}"));

    let mut errors: Vec<String> = Vec::new();
    for row in &slt.rows {
        check_row(row, fixtures, &mut errors, &path_str);
    }

    if !errors.is_empty() {
        let header = format!(
            "{} mismatch(es) in {} ({} row(s) checked):",
            errors.len(),
            path_str,
            slt.rows.len()
        );
        let body = errors.join("\n  ");
        panic!("{header}\n  {body}");
    }
}
