//! Grammar for node selectors, used
//!     for --select, --exclude
//!     the result of evaluating --selectors .yml
//!     the result of normalizing any other node selection, e.g. --resource-types
//!
//! SelectExpression ::= AndSpecifiers | OrSpecifiers | AtomSpecifier
//! OrSpecifiers   ::= (SelectExpression)(' '+(SelectExpression))*
//! AndSpecifiers    ::= (SelectExpression)(','(SelectExpression)*)
//! AtomSpecifier   ::= Identifier | AtPattern | PlusPattern
//!
//! All three version of Atom Specifiers are define by the RE
//!     (?x)
//!      ^(?:([0-9]*)\+)?       # Optional leading number followed by plus     # PlusPattern
//!      (?:([a-zA-Z][a-zA-Z0-9._]*):)?                                        # Optional qualifier
//!      ([^, +@]+)              # Identifier, anything but blank, comma, plus # Identifier
//!      (?:\+([0-9]*))?$       # Optional trailing plus followed by number    # PlusPattern
//!      |                      # OR
//!      ^\@([A-Za-z0-9_]+)$                                                   # AtPattern

use dbt_yaml::JsonSchema;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::sync::LazyLock;
use strum::{Display, EnumIter, EnumString};

use crate::{ErrorCode, FsResult, err, fs_err};

// Common has only the syntax. The rest is in dbt-scheduler

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    EnumString,
    EnumIter,
    Display,
    Default,
    JsonSchema,
    Serialize,
    Deserialize,
)]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
#[serde(rename_all = "lowercase")]
pub enum IndirectSelection {
    #[default]
    Eager,
    Buildable,
    Cautious,
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, EnumIter, Display, Deserialize)]
#[strum(serialize_all = "snake_case", ascii_case_insensitive)]
pub enum MethodName {
    Access,
    Config,
    Exposure,
    File,
    Function,
    Fqn,
    Group,
    Metric,
    Package,
    Path,
    ResourceType,
    Result,
    SavedQuery,
    SemanticModel,
    Source,
    SourceStatus,
    State,
    Tag,
    TestName,
    TestType,
    UnitTest,
    Version,
    // Column selector
    // internal only to select column in a table
    // new syntax: column:<node_id>.<column_name>
    // todo: maybe better?: column:<node_id>#<column_name>
    Column,
    Selector,
}

impl MethodName {
    pub fn default_for(value: &str) -> Self {
        if value.contains(std::path::MAIN_SEPARATOR)
            || (std::path::MAIN_SEPARATOR != '/' && value.contains('/'))
        {
            Self::Path
        } else if value.to_ascii_lowercase().ends_with(".sql")
            || value.to_ascii_lowercase().ends_with(".py")
            || value.to_ascii_lowercase().ends_with(".csv")
        {
            Self::File
        } else {
            Self::Fqn
        }
    }
}

/// The thing to match. `Unsupported` holds a value dbt Core loads as `Any` but has no
/// selection semantics for; resolving it errors instead of matching.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(untagged)]
pub enum SelectionValue {
    Scalar(String),
    Unsupported(Box<dbt_yaml::Value>),
}

impl SelectionValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            SelectionValue::Scalar(s) => Some(s),
            SelectionValue::Unsupported(_) => None,
        }
    }

    pub fn resolve(&self, method: MethodName) -> FsResult<&str> {
        match self {
            SelectionValue::Scalar(s) => Ok(s),
            SelectionValue::Unsupported(y) => err!(
                ErrorCode::SelectorError,
                "selector method `{method}` was given a value with no selection semantics \
                 ({y:?}); dbt Core would also only fail here once this selector is actually \
                 resolved"
            ),
        }
    }
}

impl fmt::Display for SelectionValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SelectionValue::Scalar(s) => f.write_str(s),
            SelectionValue::Unsupported(_) => f.write_str("<unsupported value>"),
        }
    }
}

impl From<String> for SelectionValue {
    fn from(s: String) -> Self {
        SelectionValue::Scalar(s)
    }
}

impl From<&str> for SelectionValue {
    fn from(s: &str) -> Self {
        SelectionValue::Scalar(s.to_string())
    }
}

impl PartialEq<str> for SelectionValue {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

impl PartialEq<&str> for SelectionValue {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
pub struct SelectionCriteria {
    // qualifier + optional sub‑parts ("config.materialized" ⇒ method="config", args=["materialized"])
    pub method: MethodName,
    pub method_args: Vec<String>,

    pub value: SelectionValue, // the thing to match

    // graph‑walk modifiers
    pub childrens_parents: bool,     // `@`
    pub parents_depth: Option<u32>, // `+foo` or `N+foo` - None means no parents, Some(u32::MAX) means unlimited depth
    pub children_depth: Option<u32>, // `foo+` or `foo+N` - None means no children, Some(u32::MAX) means unlimited depth

    pub indirect: Option<IndirectSelection>,

    // nested excludes
    pub exclude: Option<Box<SelectExpression>>,
}

impl SelectionCriteria {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        method: MethodName,
        method_args: Vec<String>,
        value: impl Into<SelectionValue>,
        childrens_parents: bool,
        parents_depth: Option<u32>,
        children_depth: Option<u32>,
        indirect: Option<IndirectSelection>,
        exclude: Option<Box<SelectExpression>>,
    ) -> Self {
        Self {
            method,
            method_args,
            value: value.into(),
            childrens_parents,
            parents_depth,
            children_depth,
            indirect,
            exclude,
        }
    }
}

impl fmt::Display for SelectionCriteria {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut result = String::new();

        if self.childrens_parents {
            result.push('@');
        }

        if let Some(depth) = self.parents_depth {
            // Only show explicit depth numbers, not u32::MAX which represents "all"
            if depth > 0 && depth != u32::MAX {
                result.push_str(&depth.to_string());
            }
            result.push('+');
        }

        if !self.method_args.is_empty() {
            result.push_str(&format!("{}.", self.method.to_string().to_lowercase()));
            result.push_str(&self.method_args.join("."));
            result.push(':');
        } else {
            result.push_str(&format!("{}:", self.method.to_string().to_lowercase()));
        }

        result.push_str(&self.value.to_string());

        if let Some(depth) = self.children_depth {
            result.push('+');
            // Only show explicit depth numbers, not u32::MAX which represents "all"
            if depth > 0 && depth != u32::MAX {
                result.push_str(&depth.to_string());
            }
        }

        write!(f, "{result}")
    }
}

/// Represents the AST for model specifiers, which can be combined using logical AND and OR operations.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub enum SelectExpression {
    Atom(SelectionCriteria),        // a single model specifier
    And(Vec<SelectExpression>),     // a list of model specifiers, joined by commas
    Or(Vec<SelectExpression>),      // a list of model specifiers, joined by spaces
    Exclude(Box<SelectExpression>), // For nested excludes
}

impl fmt::Display for SelectExpression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SelectExpression::And(expressions) => {
                let expressions_str = expressions
                    .iter()
                    .map(|expr| format!("{expr}"))
                    .collect::<Vec<_>>()
                    .join(",");
                write!(f, "{expressions_str}")
            }
            SelectExpression::Or(expressions) => {
                let expressions_str = expressions
                    .iter()
                    .map(|expr| format!("{expr}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                write!(f, "{expressions_str}")
            }
            SelectExpression::Atom(criteria) => write!(f, "{criteria}"),
            SelectExpression::Exclude(expr) => write!(f, "exclude({expr})"),
        }
    }
}

impl SelectExpression {
    /// Set the indirect selection mode for this expression and all nested expressions
    pub fn set_indirect_selection(&mut self, mode: IndirectSelection) {
        match self {
            SelectExpression::And(exprs) | SelectExpression::Or(exprs) => {
                for expr in exprs {
                    expr.set_indirect_selection(mode);
                }
            }
            SelectExpression::Atom(criteria) => {
                criteria.indirect = Some(mode);
            }
            SelectExpression::Exclude(expr) => {
                expr.set_indirect_selection(mode);
            }
        }
    }

    /// Apply default indirect selection mode to this expression and all nested expressions
    /// if not already specified
    pub fn apply_default_indirect_selection(&mut self, default_mode: IndirectSelection) {
        match self {
            SelectExpression::And(list) | SelectExpression::Or(list) => {
                for sub_expr in list {
                    sub_expr.apply_default_indirect_selection(default_mode);
                }
            }
            SelectExpression::Atom(criteria) => {
                // Only set indirect selection if not already specified
                if criteria.indirect.is_none() {
                    criteria.indirect = Some(default_mode);
                }
            }
            SelectExpression::Exclude(expr) => {
                expr.apply_default_indirect_selection(default_mode);
            }
        }
    }
}

/// Converts every `column:` selector in the expression into an equivalent `fqn:` selector.
///
/// Returns the converted expression along with a boolean indicating whether any
/// conversion occurred.
pub fn convert_column_selectors_to_fqn(expr: SelectExpression) -> (SelectExpression, bool) {
    match expr {
        SelectExpression::Atom(mut criteria) => {
            let mut converted = false;

            if let Some(inner) = criteria.exclude.take() {
                let (inner_converted, inner_changed) = convert_column_selectors_to_fqn(*inner);
                criteria.exclude = Some(Box::new(inner_converted));
                converted |= inner_changed;
            }

            // `column:` is internal-only, so a non-scalar value never reaches here.
            if criteria.method == MethodName::Column {
                if let Some(value) = criteria.value.as_str() {
                    converted = true;
                    let mut parts = value.rsplitn(2, '.');
                    let _column_part = parts.next().unwrap_or("");
                    let node_part = parts.next().unwrap_or("").to_string();
                    criteria.method = MethodName::Fqn;
                    criteria.value = SelectionValue::Scalar(node_part);
                }
            }

            (SelectExpression::Atom(criteria), converted)
        }
        SelectExpression::And(expressions) => {
            let mut converted = false;
            let converted_exprs = expressions
                .into_iter()
                .map(|expr| {
                    let (expr_converted, expr_changed) = convert_column_selectors_to_fqn(expr);
                    converted |= expr_changed;
                    expr_converted
                })
                .collect();
            (SelectExpression::And(converted_exprs), converted)
        }
        SelectExpression::Or(expressions) => {
            let mut converted = false;
            let converted_exprs = expressions
                .into_iter()
                .map(|expr| {
                    let (expr_converted, expr_changed) = convert_column_selectors_to_fqn(expr);
                    converted |= expr_changed;
                    expr_converted
                })
                .collect();
            (SelectExpression::Or(converted_exprs), converted)
        }
        SelectExpression::Exclude(expr) => {
            let (converted_expr, converted) = convert_column_selectors_to_fqn(*expr);
            (
                SelectExpression::Exclude(Box::new(converted_expr)),
                converted,
            )
        }
    }
}

/// Checks if a `SelectExpression` contains any `state:` selector, regardless of its value
/// (`new`, `old`, `modified`, `modified.*`, `unmodified`, or anything else).
///
/// This is useful for determining whether loading the manifest.json is required for
/// a given selector. If the selector only uses methods like `source_status:fresher+`,
/// then the manifest is not needed and we should not warn if it fails to load.
pub fn contains_state_selector(expr: &SelectExpression) -> bool {
    match expr {
        SelectExpression::Atom(criteria) => {
            criteria.method == MethodName::State
                || criteria
                    .exclude
                    .as_ref()
                    .is_some_and(|e| contains_state_selector(e))
        }
        SelectExpression::And(expressions) | SelectExpression::Or(expressions) => {
            expressions.iter().any(contains_state_selector)
        }
        SelectExpression::Exclude(expr) => contains_state_selector(expr),
    }
}

/// Checks if any of the provided optional select expressions contain a `state:` selector.
pub fn selectors_require_manifest(
    select: Option<&SelectExpression>,
    exclude: Option<&SelectExpression>,
) -> bool {
    let select_requires = select.is_some_and(contains_state_selector);
    let exclude_requires = exclude.is_some_and(contains_state_selector);
    select_requires || exclude_requires
}

// ------------------------------------------------------------------------------------------------
pub fn conjoin_expression(
    maybe_select_expression: Option<SelectExpression>,
    maybe_select_expression2: Option<SelectExpression>,
) -> Option<SelectExpression> {
    match (maybe_select_expression, maybe_select_expression2) {
        (Some(select), Some(select2)) => Some(SelectExpression::And(vec![select, select2])),
        (Some(select), None) => Some(select),
        (None, Some(select)) => Some(select),
        (None, None) => None,
    }
}

/// ----------------------------------
/// parsing
/// ----------------------------------
static RAW_SELECTOR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
^
(?P<childrens_parents>@)?
(?P<parents>(?P<parents_depth>\d*)\+)?   # optional leading +N
(?:(?P<method>[\w.]+):)?                 # optional qualifier
(?P<value>.*?)
(?P<children>\+(?P<children_depth>\d*) )?  # optional trailing +N
$
",
    )
    .unwrap()
});

pub fn parse_single_selector(raw: &str) -> FsResult<SelectionCriteria> {
    let caps = RAW_SELECTOR_RE
        .captures(raw)
        .ok_or_else(|| fs_err!(ErrorCode::SelectorError, "Invalid selector spec: `{}`", raw))?;

    // ------------------------------------------------------------------
    // Reject selectors whose <value> is empty (e.g. "++", "@", "+:").
    // ------------------------------------------------------------------
    let val = caps.name("value").unwrap().as_str();
    if val.is_empty() {
        return err!(ErrorCode::SelectorError, "Invalid selector spec: `{}`", raw);
    }

    // ---------------- method / args
    let (method, method_args) = if let Some(m) = caps.name("method") {
        let mut parts = m.as_str().split('.').map(|s| s.to_string());
        let head = parts.next().unwrap();
        let name = MethodName::from_str(&head).map_err(|_| {
            fs_err!(
                ErrorCode::SelectorError,
                "Invalid node selector method: `{}`",
                head
            )
        })?;
        (name, parts.collect())
    } else {
        let v = caps.name("value").unwrap().as_str();
        (MethodName::default_for(v), vec![])
    };

    // ---------------- depths & flags
    let parents_depth = if caps.name("parents").is_some() {
        caps.name("parents_depth")
            .map(|m| {
                if m.as_str().is_empty() {
                    u32::MAX
                } else {
                    m.as_str().parse::<u32>().unwrap()
                }
            })
            .or(Some(u32::MAX))
    } else {
        None
    };

    let children_depth = if caps.name("children").is_some() {
        caps.name("children_depth")
            .map(|m| {
                if m.as_str().is_empty() {
                    u32::MAX
                } else {
                    m.as_str().parse::<u32>().unwrap()
                }
            })
            .or(Some(u32::MAX))
    } else {
        None
    };

    let criteria = SelectionCriteria::new(
        method,
        method_args,
        caps.name("value").unwrap().as_str().to_string(),
        caps.name("childrens_parents").is_some(),
        parents_depth,
        children_depth,
        Some(IndirectSelection::default()), // CLI flag can override later
        None,
    );

    //---------------------------------------------------------------
    // `@foo+` is illegal
    //---------------------------------------------------------------
    if criteria.childrens_parents && criteria.children_depth.is_some() {
        return err!(
            ErrorCode::SelectorError,
            "Invalid selector `{}` - \"@\" and trailing \"+\" are incompatible",
            raw
        );
    }

    //---------------------------------------------------------------
    // `column:` must have a column name after the dot
    //     e.g.  column:node123.foo_col   ✔
    //           column:model.node123.col ✔
    //           column:node123.          ✘
    //---------------------------------------------------------------
    let value_str = criteria.value.as_str().unwrap_or_default();

    if criteria.method == MethodName::Column {
        // we only need to make sure there is *some* column name
        match value_str.rfind('.') {
            // a dot exists and it's not the last char ⇒ we're good
            Some(ix) if ix < value_str.len() - 1 => {}
            _ => {
                return err!(ErrorCode::SelectorError, "Invalid selector spec: `{}`", raw);
            }
        }
    }
    //---------------------------------------------------------------
    // If we see a '+' inside <value> but *didn't* set either the
    // `parents_depth` or `children_depth`, then the user wrote something
    // like  `identifier+abc`, i.e. a "depth" that isn't numeric.
    //---------------------------------------------------------------
    if criteria.parents_depth.is_none()
        && criteria.children_depth.is_none()
        && value_str.contains('+')
    {
        return err!(
            ErrorCode::SelectorError,
            "Invalid model specifier near: {}",
            raw
        );
    }

    Ok(criteria)
}

/// Turn the tokenised CLI list (already split on whitespace by Clap)
/// into a `SelectExpression` tree.
///
/// * **Outer level**: each whitespace‑separated token is an *OR* term.
/// * **Inner level**: inside each token, a comma `,` separates *AND* terms.
///
/// A few helper invariants make the logic easy to follow:
///
/// * A single criterion becomes `SelectExpression::Atom`.
/// * A list of N criteria joined by the same operator collapses to
///   `SelectExpression::And(vec)` or `SelectExpression::Or(vec)` where `vec.len() == N`.
/// * We never wrap a single Atom in an unnecessary `And/Or`.
pub fn parse_model_specifiers(tokens: &[String]) -> FsResult<SelectExpression> {
    if tokens.is_empty() {
        return Err(fs_err!(
            ErrorCode::SelectorError,
            "empty selector list passed to --select/--exclude"
        ));
    }

    // ----------- build OR level -------------------------------------------------------------
    let mut or_terms: Vec<SelectExpression> = Vec::new();

    for token in tokens {
        // Skip completely empty tokens (can happen after user writes two spaces in a row)
        if token.trim().is_empty() {
            continue;
        }

        // ----------- build AND level --------------------------------------------------------
        let mut and_terms: Vec<SelectExpression> = Vec::new();
        for piece in token.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let criteria = parse_single_selector(piece)?;
            and_terms.push(SelectExpression::Atom(criteria));
        }

        // collapse AND level
        let inner_expr = match and_terms.len() {
            0 => continue,                 // nothing in this token → ignore it
            1 => and_terms.pop().unwrap(), // single item – no extra wrapper
            _ => SelectExpression::And(and_terms),
        };

        or_terms.push(inner_expr);
    }

    // collapse OR level
    let root = match or_terms.len() {
        0 => {
            // Every token was delimiter-only (e.g. "--select ,,"). Match
            // dbt-core behavior: preserve the raw text as a literal fqn
            // criterion that resolves to zero matches at runtime, producing
            // the standard dbt1092 "does not match" + dbt1601 "Nothing to do"
            // warnings rather than aborting the invocation.
            let raw = tokens.join(" ");
            SelectExpression::Atom(SelectionCriteria::new(
                MethodName::Fqn,
                vec![],
                raw,
                false,
                None,
                None,
                Some(IndirectSelection::default()),
                None,
            ))
        }
        1 => or_terms.pop().unwrap(),
        _ => SelectExpression::Or(or_terms),
    };

    Ok(root)
}

/// helper used by the CLI to add an extra criterion (resource_type etc.)
pub fn conjoin_predicate(
    maybe_sel: Option<SelectExpression>,
    extra: Option<SelectionCriteria>,
) -> Option<SelectExpression> {
    match (maybe_sel, extra) {
        (None, None) => None,
        (Some(sel), None) => Some(sel),
        (None, Some(c)) => Some(SelectExpression::Atom(c)),
        (Some(sel), Some(c)) => {
            let atom = SelectExpression::Atom(c);
            Some(match sel {
                SelectExpression::And(mut v) => {
                    v.push(atom);
                    SelectExpression::And(v)
                }
                _ => SelectExpression::And(vec![sel, atom]),
            })
        }
    }
}

// ------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identifier() -> FsResult<()> {
        let result = parse_single_selector("identifier")?;
        assert_eq!(
            result,
            SelectionCriteria {
                method: MethodName::Fqn,
                method_args: vec![],
                value: SelectionValue::Scalar("identifier".to_string()),
                childrens_parents: false,
                parents_depth: None,
                children_depth: None,
                indirect: Some(IndirectSelection::default()),
                exclude: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_at_pattern() -> FsResult<()> {
        let result = parse_single_selector("@identifier")?;
        assert_eq!(
            result,
            SelectionCriteria {
                method: MethodName::Fqn,
                method_args: vec![],
                value: SelectionValue::Scalar("identifier".to_string()),
                childrens_parents: true,
                parents_depth: None,
                children_depth: None,
                indirect: Some(IndirectSelection::default()),
                exclude: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_plus_pattern_leading_number() -> FsResult<()> {
        let result = parse_single_selector("2+identifier")?;
        assert_eq!(
            result,
            SelectionCriteria {
                method: MethodName::Fqn,
                method_args: vec![],
                value: SelectionValue::Scalar("identifier".to_string()),
                childrens_parents: false,
                parents_depth: Some(2),
                children_depth: None,
                indirect: Some(IndirectSelection::default()),
                exclude: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_plus_pattern_trailing_plus() -> FsResult<()> {
        let result = parse_single_selector("identifier+")?;
        assert_eq!(
            result,
            SelectionCriteria {
                method: MethodName::Fqn,
                method_args: vec![],
                value: SelectionValue::Scalar("identifier".to_string()),
                childrens_parents: false,
                parents_depth: None,
                children_depth: Some(u32::MAX),
                indirect: Some(IndirectSelection::default()),
                exclude: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_plus_pattern_trailing_number() -> FsResult<()> {
        let input = "identifier+8";
        let result = parse_model_specifiers(&[input.to_string()])?;
        assert_eq!(
            result,
            SelectExpression::Atom(SelectionCriteria::new(
                MethodName::Fqn,
                vec![],
                "identifier".to_string(),
                false,
                None,
                Some(8),
                Some(IndirectSelection::default()),
                None,
            ))
        );
        Ok(())
    }

    #[test]
    fn test_plus_pattern_both_plus() -> FsResult<()> {
        let input = "+identifier+";
        let result = parse_single_selector(input)?;
        assert_eq!(
            result,
            SelectionCriteria {
                method: MethodName::Fqn,
                method_args: vec![],
                value: SelectionValue::Scalar("identifier".to_string()),
                childrens_parents: false,
                parents_depth: Some(u32::MAX),
                children_depth: Some(u32::MAX),
                indirect: Some(IndirectSelection::default()),
                exclude: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_plus_path_pattern_both_plus() -> FsResult<()> {
        let input = "+path:identifier/rest+";
        let result = parse_single_selector(input)?;
        assert_eq!(
            result,
            SelectionCriteria {
                method: MethodName::Path,
                method_args: vec![],
                value: SelectionValue::Scalar("identifier/rest".to_string()),
                childrens_parents: false,
                parents_depth: Some(u32::MAX),
                children_depth: Some(u32::MAX),
                indirect: Some(IndirectSelection::default()),
                exclude: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_plus_pattern_leading_number_trailing_plus() -> FsResult<()> {
        let input = "5+identifier+";
        let result = parse_single_selector(input)?;
        assert_eq!(
            result,
            SelectionCriteria {
                method: MethodName::Fqn,
                method_args: vec![],
                value: SelectionValue::Scalar("identifier".to_string()),
                childrens_parents: false,
                parents_depth: Some(5),
                children_depth: Some(u32::MAX),
                indirect: Some(IndirectSelection::default()),
                exclude: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_plus_pattern_leading_plus_trailing_number() -> FsResult<()> {
        let input = "+identifier+6";
        let result = parse_single_selector(input)?;
        assert_eq!(
            result,
            SelectionCriteria {
                method: MethodName::Fqn,
                method_args: vec![],
                value: SelectionValue::Scalar("identifier".to_string()),
                childrens_parents: false,
                parents_depth: Some(u32::MAX),
                children_depth: Some(6),
                indirect: Some(IndirectSelection::default()),
                exclude: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_invalid_trailing_number() {
        let input = "identifier+abc";
        let result = parse_single_selector(input);
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(
                e.to_string().trim(),
                "Invalid model specifier near: identifier+abc"
            );
        }
    }

    #[test]
    fn test_empty_string() {
        let input = "";
        let result = parse_single_selector(input);
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.to_string().trim(), "Invalid selector spec: ``");
        }
    }

    #[test]
    fn test_only_plus_signs() {
        let input = "++";
        let result = parse_single_selector(input);
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.to_string().trim(), "Invalid selector spec: `++`");
        }
    }

    #[test]
    fn invalid_at_pattern_with_plus() {
        let input = "@identifier+";
        let result = parse_single_selector(input);
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(
                e.to_string().trim(),
                "Invalid selector `@identifier+` - \"@\" and trailing \"+\" are incompatible"
            );
        }
    }

    #[test]
    fn test_and_specifier() -> FsResult<()> {
        let input = vec!["identifier,@identifier".to_string()];
        let result = parse_model_specifiers(&input)?;
        match result {
            SelectExpression::And(vec) if vec.len() == 2 => match (&vec[0], &vec[1]) {
                (SelectExpression::Atom(x), SelectExpression::Atom(y)) => {
                    assert_eq!(
                        x,
                        &SelectionCriteria {
                            method: MethodName::Fqn,
                            method_args: vec![],
                            value: SelectionValue::Scalar("identifier".to_string()),
                            childrens_parents: false,
                            parents_depth: None,
                            children_depth: None,
                            indirect: Some(IndirectSelection::default()),
                            exclude: None,
                        }
                    );
                    assert_eq!(
                        y,
                        &SelectionCriteria {
                            method: MethodName::Fqn,
                            method_args: vec![],
                            value: SelectionValue::Scalar("identifier".to_string()),
                            childrens_parents: true,
                            parents_depth: None,
                            children_depth: None,
                            indirect: Some(IndirectSelection::default()),
                            exclude: None,
                        }
                    );
                }
                _ => panic!("Expected SelectExpr::Or variant with two Atom elements"),
            },
            _ => panic!("Expected SelectExpr::And variant with one element"),
        }

        Ok(())
    }

    #[test]
    fn test_or_specifier() -> FsResult<()> {
        let input = vec!["identifier".to_string(), "@identifier".to_string()];
        let result = parse_model_specifiers(&input)?;
        match result {
            SelectExpression::Or(vec) if vec.len() == 2 => match (&vec[0], &vec[1]) {
                (SelectExpression::Atom(x), SelectExpression::Atom(y)) => {
                    assert_eq!(
                        x,
                        &SelectionCriteria {
                            method: MethodName::Fqn,
                            method_args: vec![],
                            value: SelectionValue::Scalar("identifier".to_string()),
                            childrens_parents: false,
                            parents_depth: None,
                            children_depth: None,
                            indirect: Some(IndirectSelection::default()),
                            exclude: None,
                        }
                    );
                    assert_eq!(
                        y,
                        &SelectionCriteria {
                            method: MethodName::Fqn,
                            method_args: vec![],
                            value: SelectionValue::Scalar("identifier".to_string()),
                            childrens_parents: true,
                            parents_depth: None,
                            children_depth: None,
                            indirect: Some(IndirectSelection::default()),
                            exclude: None,
                        }
                    );
                }
                _ => panic!("Expected SelectExpr::Or variant with two Atom elements"),
            },
            _ => panic!("Expected SelectExpr::Or variant with two elements"),
        }

        Ok(())
    }

    #[test]
    fn test_column_selector_identifier() -> FsResult<()> {
        let input = "column:node123.foo_col";
        let result = parse_single_selector(input)?;
        assert_eq!(
            result,
            SelectionCriteria {
                method: MethodName::Column,
                method_args: vec![],
                value: SelectionValue::Scalar("node123.foo_col".to_string()),
                childrens_parents: false,
                parents_depth: None,
                children_depth: None,
                indirect: Some(IndirectSelection::default()),
                exclude: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_column_selector_with_leading_plus() -> FsResult<()> {
        let input = "+column:node123.foo_col";
        let result = parse_single_selector(input)?;
        assert_eq!(
            result,
            SelectionCriteria {
                method: MethodName::Column,
                method_args: vec![],
                value: SelectionValue::Scalar("node123.foo_col".to_string()),
                childrens_parents: false,
                parents_depth: Some(u32::MAX),
                children_depth: None,
                indirect: Some(IndirectSelection::default()),
                exclude: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_column_selector_with_trailing_plus() -> FsResult<()> {
        let input = "column:node123.foo_col+";
        let result = parse_single_selector(input)?;
        assert_eq!(
            result,
            SelectionCriteria {
                method: MethodName::Column,
                method_args: vec![],
                value: SelectionValue::Scalar("node123.foo_col".to_string()),
                childrens_parents: false,
                parents_depth: None,
                children_depth: Some(u32::MAX),
                indirect: Some(IndirectSelection::default()),
                exclude: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_column_selector_with_both_plus() -> FsResult<()> {
        let input = "+column:node123.foo_col+";
        let result = parse_single_selector(input)?;

        assert_eq!(
            result,
            SelectionCriteria {
                method: MethodName::Column,
                method_args: vec![],
                value: SelectionValue::Scalar("node123.foo_col".to_string()),
                childrens_parents: false,
                parents_depth: Some(u32::MAX),
                children_depth: Some(u32::MAX),
                indirect: Some(IndirectSelection::default()),
                exclude: None,
            }
        );
        Ok(())
    }

    #[test]
    fn test_invalid_column_selector_with_missing_column_name() {
        let input = "column:node123.";
        let result = parse_single_selector(input);
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(
                e.to_string().trim(),
                "Invalid selector spec: `column:node123.`"
            );
        }
    }

    #[test]
    fn test_column_selectors_with_trailing_plus() -> FsResult<()> {
        let input = vec![
            "column:node123.foo_col+".to_string(),
            "column:node123.bar_col+".to_string(),
        ];
        let result = parse_model_specifiers(&input)?;

        match result {
            SelectExpression::Or(vec) if vec.len() == 2 => match (&vec[0], &vec[1]) {
                (SelectExpression::Atom(a), SelectExpression::Atom(b)) => {
                    assert_eq!(
                        a,
                        &SelectionCriteria {
                            method: MethodName::Column,
                            method_args: vec![],
                            value: SelectionValue::Scalar("node123.foo_col".to_string()),
                            childrens_parents: false,
                            parents_depth: None,
                            children_depth: Some(u32::MAX),
                            indirect: Some(IndirectSelection::default()),
                            exclude: None,
                        }
                    );
                    assert_eq!(
                        b,
                        &SelectionCriteria {
                            method: MethodName::Column,
                            method_args: vec![],
                            value: SelectionValue::Scalar("node123.bar_col".to_string()),
                            childrens_parents: false,
                            parents_depth: None,
                            children_depth: Some(u32::MAX),
                            indirect: Some(IndirectSelection::default()),
                            exclude: None,
                        }
                    );
                }
                _ => panic!("Expected Atom variants"),
            },
            _ => panic!("Expected SelectExpr::Or expression with 2 atoms"),
        }

        Ok(())
    }

    #[test]
    fn test_display_selector_with_unbounded_plus() -> FsResult<()> {
        // Test that u32::MAX is not displayed as a number (should just be "+")
        let criteria = SelectionCriteria {
            method: MethodName::Fqn,
            method_args: vec![],
            value: SelectionValue::Scalar("model_a".to_string()),
            childrens_parents: false,
            parents_depth: None,
            children_depth: Some(u32::MAX),
            indirect: Some(IndirectSelection::default()),
            exclude: None,
        };
        assert_eq!(criteria.to_string(), "fqn:model_a+");

        // Test with explicit depth number
        let criteria_with_depth = SelectionCriteria {
            method: MethodName::Fqn,
            method_args: vec![],
            value: SelectionValue::Scalar("model_a".to_string()),
            childrens_parents: false,
            parents_depth: None,
            children_depth: Some(3),
            indirect: Some(IndirectSelection::default()),
            exclude: None,
        };
        assert_eq!(criteria_with_depth.to_string(), "fqn:model_a+3");

        // Test with unbounded parents
        let criteria_parents = SelectionCriteria {
            method: MethodName::Fqn,
            method_args: vec![],
            value: SelectionValue::Scalar("model_a".to_string()),
            childrens_parents: false,
            parents_depth: Some(u32::MAX),
            children_depth: None,
            indirect: Some(IndirectSelection::default()),
            exclude: None,
        };
        assert_eq!(criteria_parents.to_string(), "+fqn:model_a");

        // Test with both unbounded
        let criteria_both = SelectionCriteria {
            method: MethodName::Fqn,
            method_args: vec![],
            value: SelectionValue::Scalar("model_a".to_string()),
            childrens_parents: false,
            parents_depth: Some(u32::MAX),
            children_depth: Some(u32::MAX),
            indirect: Some(IndirectSelection::default()),
            exclude: None,
        };
        assert_eq!(criteria_both.to_string(), "+fqn:model_a+");

        Ok(())
    }

    #[test]
    fn test_selector_method_cli_parse() -> FsResult<()> {
        let result = parse_single_selector("selector:my_selector")?;
        assert_eq!(result.method, MethodName::Selector);
        assert_eq!(result.value, "my_selector");
        assert_eq!(result.parents_depth, None);
        assert_eq!(result.children_depth, None);
        Ok(())
    }

    #[test]
    fn test_selector_method_cli_parse_with_graph_ops() -> FsResult<()> {
        let result = parse_single_selector("1+selector:my_selector+2")?;
        assert_eq!(result.method, MethodName::Selector);
        assert_eq!(result.value, "my_selector");
        assert_eq!(result.parents_depth, Some(1));
        assert_eq!(result.children_depth, Some(2));
        Ok(())
    }

    #[test]
    fn test_selector_method_cli_parse_at_operator() -> FsResult<()> {
        let result = parse_single_selector("@selector:my_selector")?;
        assert_eq!(result.method, MethodName::Selector);
        assert_eq!(result.value, "my_selector");
        assert!(result.childrens_parents);
        Ok(())
    }

    // Regression test for a gap flagged in review of the fix for dbt-core#15963
    // (https://github.com/dbt-labs/fs/pull/14191#discussion_r3937827739): this used to only
    // recognize values starting with `modified`/`new`, silently treating `state:old` and
    // `state:unmodified` as not needing a comparison manifest even though dbt Core's
    // `StateSelectorMethod.search` raises "Got a state selector method, but no comparison
    // manifest" unconditionally for every `state:*` value.
    #[test]
    fn test_selectors_require_manifest_covers_every_state_value() -> FsResult<()> {
        for value in [
            "new",
            "old",
            "modified",
            "modified.body",
            "modified.configs",
            "unmodified",
        ] {
            let criteria = parse_single_selector(&format!("state:{value}"))?;
            let expr = SelectExpression::Atom(criteria);
            assert!(
                selectors_require_manifest(Some(&expr), None),
                "state:{value} should require a comparison manifest"
            );
        }
        Ok(())
    }

    #[test]
    fn test_selectors_require_manifest_false_for_non_state_selector() -> FsResult<()> {
        let criteria = parse_single_selector("tag:nightly")?;
        let expr = SelectExpression::Atom(criteria);
        assert!(!selectors_require_manifest(Some(&expr), None));
        Ok(())
    }
}
