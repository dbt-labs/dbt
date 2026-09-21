use std::collections::{BTreeMap, HashMap};
use std::ops::Deref;

use dbt_common::node_selector::{IndirectSelection, SelectExpression};
use dbt_yaml::{DbtSchema, UntaggedEnumDeserialize, Verbatim};
use serde::de::{self, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use super::serde::FloatOrString;

// =============================================================================
// SelectorValue — accepts any YAML scalar (string, boolean, number) and
// normalises it to a `String`.
//
// This matches dbt-core's Python behaviour where the `value` field in a
// selector definition is typed as `Any`, so users can write:
//
//     value: true          # boolean
//     value: 42            # number
//     value: selector_name # string
//
// without quoting non-string values.
// =============================================================================

/// A selector value that accepts any YAML scalar (string, boolean, number)
/// and normalizes it to a string representation for downstream matching.
#[derive(Debug, Clone, PartialEq, Eq, DbtSchema)]
pub struct SelectorValue(pub String);

impl SelectorValue {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Deref for SelectorValue {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for SelectorValue {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SelectorValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for SelectorValue {
    fn from(s: &str) -> Self {
        SelectorValue(s.to_string())
    }
}

impl From<String> for SelectorValue {
    fn from(s: String) -> Self {
        SelectorValue(s)
    }
}

impl From<SelectorValue> for String {
    fn from(v: SelectorValue) -> Self {
        v.0
    }
}

impl Serialize for SelectorValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SelectorValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SelectorValueVisitor;

        impl<'de> Visitor<'de> for SelectorValueVisitor {
            type Value = SelectorValue;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string, boolean, or number")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<SelectorValue, E> {
                Ok(SelectorValue(v.to_string()))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<SelectorValue, E> {
                Ok(SelectorValue(v))
            }

            fn visit_bool<E: de::Error>(self, v: bool) -> Result<SelectorValue, E> {
                Ok(SelectorValue(v.to_string()))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<SelectorValue, E> {
                Ok(SelectorValue(v.to_string()))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<SelectorValue, E> {
                Ok(SelectorValue(v.to_string()))
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<SelectorValue, E> {
                Ok(SelectorValue(v.to_string()))
            }
        }

        deserializer.deserialize_any(SelectorValueVisitor)
    }
}

/// A method argument. Loading accepts any shape dbt Core's `Any`-typed `value` would;
/// `Unsupported` defers the failure to resolution, like dbt Core does.
#[derive(Debug, Clone, Serialize, UntaggedEnumDeserialize, DbtSchema)]
#[serde(untagged)]
pub enum SelectorMethodValue {
    Scalar(SelectorValue),
    Unsupported(Box<dbt_yaml::Value>),
}

impl SelectorMethodValue {
    pub fn scalar(&self) -> Option<&SelectorValue> {
        match self {
            SelectorMethodValue::Scalar(v) => Some(v),
            SelectorMethodValue::Unsupported(_) => None,
        }
    }
}

impl From<SelectorValue> for SelectorMethodValue {
    fn from(v: SelectorValue) -> Self {
        SelectorMethodValue::Scalar(v)
    }
}

//
// ---- top-level file -------------------------------------------------------------------------
//
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, DbtSchema)]
pub struct SelectorFile {
    pub version: Option<FloatOrString>,
    /// List of named selectors that may later be referenced with
    /// `dbt run --selector <name>`.
    pub selectors: Vec<SelectorDefinition>,
}

//
// ---- one named selector ---------------------------------------------------------------------
//

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, DbtSchema)]
pub struct SelectorDefinition {
    /// The key used in `--selector <name>`.
    pub name: String,

    /// Human-readable description (optional).
    #[serde(default)]
    pub description: Option<String>,

    /// Whether this selector should be used when the user does *not*
    /// pass `--select` / `--selector`.
    ///
    /// Wrapped in [`Verbatim`] so the Jinja string form is captured
    /// unrendered at parse time; rendering happens lazily in
    /// `resolve_selectors_from_yaml`, and only when the default is
    /// actually needed (no CLI selection was provided). This matches
    /// dbt-core behavior: a buggy Jinja expression in a selector the
    /// user didn't ask to use must not fail the run.
    #[serde(default)]
    pub default: Verbatim<Option<SelectorDefaultSpec>>,

    /// Either a bare CLI string or a full YAML expression tree.
    pub definition: SelectorDefinitionValue,
}

/// Parsed form of the selector `default:` field: either a literal bool
/// or a still-unrendered Jinja template string.
///
/// Also reused for graph-walk flag fields (`children`, `parents`,
/// `childrens_parents`) so the JSON schema correctly shows that those fields
/// accept Jinja expressions.  At runtime those fields are always rendered
/// before deserialization (`into_typed_with_jinja`), so `Template` is only
/// ever populated for `default`.
#[derive(Debug, Clone, Serialize, UntaggedEnumDeserialize, DbtSchema)]
#[serde(untagged)]
pub enum SelectorDefaultSpec {
    Bool(bool),
    Template(String),
}

impl From<bool> for SelectorDefaultSpec {
    fn from(b: bool) -> Self {
        SelectorDefaultSpec::Bool(b)
    }
}

impl Default for SelectorDefaultSpec {
    fn default() -> Self {
        false.into()
    }
}

impl SelectorDefaultSpec {
    pub fn as_bool(&self) -> bool {
        match self {
            SelectorDefaultSpec::Bool(b) => *b,
            // Unrendered template — callers using as_bool() treat it as false.
            SelectorDefaultSpec::Template(_) => false,
        }
    }
}

//
// ---- definition discriminated union ---------------------------------------------------------
//

#[derive(Debug, Clone, Serialize, UntaggedEnumDeserialize, DbtSchema)]
#[serde(untagged)]
pub enum SelectorDefinitionValue {
    /// CLI-style selector string (e.g. `"snowplow tag:nightly"`).
    String(String),

    /// Full YAML tree (see `SelectorExpr` below).
    Full(SelectorExpr),

    /// Bare YAML sequence — treated as an implicit union of the listed items.
    ///
    /// dbt-core allows writing:
    /// ```yaml
    /// definition:
    ///   - method: tag
    ///     value: nightly
    ///   - method: config
    ///     value: "materialized:table"
    /// ```
    /// which is semantically equivalent to `union: [...]`.
    Array(Vec<SelectorDefinitionValue>),
}

/// Top‐level expression: either a boolean node or a single atom
#[derive(Serialize, UntaggedEnumDeserialize, Debug, Clone, DbtSchema)]
#[serde(untagged)]
pub enum SelectorExpr {
    Composite(CompositeExpr),
    Atom(AtomExpr),
}

/// A boolean composition of other selectors.
///
/// Supports three forms from dbt-core's YAML spec:
/// - `union: [...]`
/// - `intersection: [...]`
/// - `union: [...]\nexclude: [...]` — top-level exclude applied after the union
///
/// Exactly one of `union` / `intersection` must be present; `exclude` is always optional.
/// The generated JSON schema allows all three keys (no `oneOf` constraint) to avoid
/// false-positive IDE squiggles on the common `union + exclude` pattern.
#[skip_serializing_none]
#[derive(Serialize, Debug, Clone, DbtSchema)]
pub struct CompositeExpr {
    /// OR of all listed selectors.
    #[serde(default)]
    pub union: Option<Vec<SelectorDefinitionValue>>,
    /// AND of all listed selectors.
    #[serde(default)]
    pub intersection: Option<Vec<SelectorDefinitionValue>>,
    /// Models excluded from the composite result.
    #[serde(default)]
    pub exclude: Option<Vec<SelectorDefinitionValue>>,
}

impl<'de> Deserialize<'de> for CompositeExpr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct CompositeExprVisitor;

        impl<'de> Visitor<'de> for CompositeExprVisitor {
            type Value = CompositeExpr;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "a map with keys from 'union', 'intersection', 'exclude'")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut union: Option<Vec<SelectorDefinitionValue>> = None;
                let mut intersection: Option<Vec<SelectorDefinitionValue>> = None;
                let mut exclude: Option<Vec<SelectorDefinitionValue>> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "union" => union = Some(map.next_value()?),
                        "intersection" => intersection = Some(map.next_value()?),
                        "exclude" => exclude = Some(map.next_value()?),
                        other => {
                            let _: IgnoredAny = map.next_value()?;
                            return Err(de::Error::unknown_field(
                                other,
                                &["union", "intersection", "exclude"],
                            ));
                        }
                    }
                }

                // Require union or intersection — exclude-only maps are AtomExpr::Exclude.
                if union.is_none() && intersection.is_none() {
                    return Err(de::Error::custom("expected 'union' or 'intersection' key"));
                }

                Ok(CompositeExpr {
                    union,
                    intersection,
                    exclude,
                })
            }
        }

        deserializer.deserialize_map(CompositeExprVisitor)
    }
}

impl CompositeExpr {
    pub fn union(items: Vec<SelectorDefinitionValue>) -> Self {
        Self {
            union: Some(items),
            intersection: None,
            exclude: None,
        }
    }
    pub fn intersection(items: Vec<SelectorDefinitionValue>) -> Self {
        Self {
            union: None,
            intersection: Some(items),
            exclude: None,
        }
    }
}

/// Alias kept for compatibility with callers that name `CompositeKind`.
pub type CompositeKind = CompositeExpr;

//
// ---- full YAML selector AST -----------------------------------------------------------------
//

/// The true leaves: either a method, a shorthand, or an exclude
#[derive(Serialize, UntaggedEnumDeserialize, Debug, Clone, DbtSchema)]
#[serde(untagged)]
pub enum AtomExpr {
    Method(MethodAtomExpr),
    Exclude(ExcludeAtomExpr),
    /// Direct method name as key with value
    MethodKey(BTreeMap<String, SelectorMethodValue>),
}

/// A *resolved* selector ⇒ the "include" (`select`) expression and the
/// optional "exclude" (`exclude`) expression that will later be handed
/// to the scheduler.
#[derive(Debug, Clone, Default)]
pub struct ResolvedSelector {
    pub include: Option<SelectExpression>,
    pub exclude: Option<SelectExpression>,
    pub selector_definitions: HashMap<String, SelectorEntry>,
}

/// What we really need at runtime for each selector.
#[derive(Debug, Clone)]
pub struct SelectorEntry {
    pub include: SelectExpression, // the include expression (which may contain nested excludes)
    pub is_default: bool,          // original `default: true`
    pub description: Option<String>, // docs string from YAML
    pub definition: SelectorDefinitionValue, // original parsed YAML definition for manifest parity
}

#[derive(Debug, Clone, Serialize, Deserialize, DbtSchema)]
pub struct MethodAtomExpr {
    /// Selector method name. Standard values: `access`, `config`, `exposure`, `file`, `fqn`,
    /// `group`, `metric`, `package`, `path`, `resource_type`, `result`, `saved_query`,
    /// `semantic_model`, `source`, `source_status`, `state`, `tag`, `test_name`, `test_type`,
    /// `unit_test`, `version`. YAML-only: `selector` (references another named selector).
    /// Dot-notation adds a sub-argument: `config.materialized`, `config.schema`, etc.
    pub method: String,
    pub value: SelectorMethodValue,

    // graph-walk flags (all optional / default = false)
    // SelectorDefaultSpec instead of bool so the JSON schema shows that
    // Jinja expressions are accepted here (they are pre-rendered before
    // deserialization, but authors write them in YAML).
    #[serde(default)]
    pub childrens_parents: SelectorDefaultSpec,
    #[serde(default)]
    pub parents: SelectorDefaultSpec,
    #[serde(default)]
    pub children: SelectorDefaultSpec,

    // depth limits
    #[serde(default)]
    pub parents_depth: Option<u32>,
    #[serde(default)]
    pub children_depth: Option<u32>,

    // indirect selection
    #[serde(default)]
    pub indirect_selection: Option<IndirectSelection>,

    // exclude
    #[serde(default)]
    pub exclude: Option<Vec<SelectorDefinitionValue>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, DbtSchema)]
pub struct ExcludeAtomExpr {
    pub exclude: Vec<SelectorDefinitionValue>,
}
