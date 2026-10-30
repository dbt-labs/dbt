use std::collections::{BTreeMap, HashMap};
use std::ops::Deref;

use dbt_common::node_selector::{IndirectSelection, SelectExpression};
use dbt_yaml::{DbtSchema, UntaggedEnumDeserialize};
use serde::de::{self, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use super::serde::{
    BoolOrJinja, FloatOrString, yaml_11_bool_null_as_false, yaml_11_bool_null_as_none,
};

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
    #[serde(default, deserialize_with = "yaml_11_bool_null_as_none")]
    #[schemars(with = "Option<BoolOrJinja>")]
    pub default: Option<bool>,

    /// Either a bare CLI string or a full YAML expression tree.
    pub definition: SelectorDefinitionValue,
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

    // Graph-walk flags, all optional and defaulting to false.
    //
    // `yaml_11_bool_null_as_false` matches dbt-core, which sees these values already rendered
    // and coerces them with `bool(...)`: PyYAML has resolved the YAML 1.1 token set, and a bare
    // `children:` answers false. `#[schemars(with)]` keeps the published schema advertising the
    // Jinja string form that authors write; it is gone by the time serde runs.
    #[serde(default, deserialize_with = "yaml_11_bool_null_as_false")]
    #[schemars(with = "BoolOrJinja")]
    pub childrens_parents: bool,
    #[serde(default, deserialize_with = "yaml_11_bool_null_as_false")]
    #[schemars(with = "BoolOrJinja")]
    pub parents: bool,
    #[serde(default, deserialize_with = "yaml_11_bool_null_as_false")]
    #[schemars(with = "BoolOrJinja")]
    pub children: bool,

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Deserializes a `MethodAtomExpr` carrying `line` as its only extra key and reports the
    /// resolved `children` flag, or the serde error.
    ///
    /// These inputs are what serde sees *after* Jinja rendering: `into_typed_with_jinja` has
    /// already run over the document, so a `"{{ ... }}"` source reaches this point as whatever
    /// the render produced — a real `Bool` for a single expression, a `String` for a mixed
    /// template. The string rows below therefore stand in for the mixed-template forms too.
    fn children_flag(line: &str) -> Result<bool, String> {
        let yaml = format!(
            r#"
            method: fqn
            value: a
            {line}
            "#
        );
        dbt_yaml::from_str::<MethodAtomExpr>(&yaml)
            .map(|expr| expr.children)
            .map_err(|e| e.to_string())
    }

    /// The graph-walk flags must resolve the YAML 1.1 boolean token set that PyYAML — and so
    /// dbt-core — resolves, and must reject anything outside it rather than silently answering
    /// `false`.
    ///
    /// dbt-core coerces these fields with plain Python truthiness,
    /// `bool(dct.get("children"))` (`core/dbt/graph/selector_spec.py:131-134`), reached after
    /// PyYAML has already resolved unquoted `yes`/`no` to real booleans. That makes *every*
    /// non-empty string truthy in core, including `"false"`. Three groups of rows below
    /// deliberately do not reproduce that:
    ///
    /// - A *quoted* `"false"` resolves to `false`. Core reads it as true — PyYAML leaves it a
    ///   string and `bool("false")` is truthy — which is a bug rather than a behavior worth
    ///   matching. (Unquoted `false`/`no`/`off` are false on both engines; each gets them as
    ///   real booleans from its YAML reader. The quoted string, meanwhile, cannot be told apart
    ///   here from the same string rendered by a Jinja template, which has to resolve.)
    /// - `"1"` / `maybe` / `42` error. Core reads them as true. Erroring is what Fusion's plain
    ///   `bool` fields, such as `contract.enforced`, already do for exactly this case.
    /// - `null` resolves to `false`, matching core (`bool(None)`), where today it errors.
    ///
    /// Verified against dbt-core 2026-09-17 (dbt-state v2.47.0) via `cargo mantle ls
    /// --selector ...` on `dbt_conformance_regressions/selector_graph_flag_jinja`; see that
    /// fixture's `selectors.yml` for the observed per-case results.
    #[test]
    fn test_graph_walk_flag_accepts_yaml_11_bool_tokens() {
        // Genuine booleans, and the default when the key is absent.
        assert_eq!(children_flag(""), Ok(false));
        assert_eq!(children_flag("children: true"), Ok(true));
        assert_eq!(children_flag("children: false"), Ok(false));

        // Unquoted YAML 1.1 tokens. dbt-yaml's `yaml_11` feature resolves them to real
        // booleans, as PyYAML does for core.
        assert_eq!(children_flag("children: yes"), Ok(true));
        assert_eq!(children_flag("children: no"), Ok(false));
        assert_eq!(children_flag("children: on"), Ok(true));
        assert_eq!(children_flag("children: off"), Ok(false));
        assert_eq!(children_flag("children: YES"), Ok(true));

        // Quoted booleans, and the same strings as produced by a mixed Jinja template.
        assert_eq!(children_flag(r#"children: "true""#), Ok(true));
        assert_eq!(children_flag(r#"children: "false""#), Ok(false));
        assert_eq!(children_flag(r#"children: "yes""#), Ok(true));

        // Rendered text often carries whitespace around the word: a `{% if %}` block spread over
        // lines, or the trailing newline of a `>` folded scalar. Core's `bool(...)` walks for any
        // non-empty string, so the `" true "` row agrees with it.
        assert_eq!(children_flag(r#"children: " true ""#), Ok(true));
        assert_eq!(children_flag(r#"children: "\nno\n""#), Ok(false));

        // An explicit `children:` with no value. Core answers false; erroring would reject a
        // plausible authoring slip that core accepts.
        assert_eq!(children_flag("children: null"), Ok(false));

        // Outside the token set: an error, not a silent `false`.
        assert!(children_flag(r#"children: "1""#).is_err());
        assert!(children_flag(r#"children: "tRue""#).is_err());
        assert!(children_flag("children: maybe").is_err());
        assert!(children_flag("children: 42").is_err());
        assert!(children_flag("children: [1]").is_err());
    }

    /// All three graph-walk flags share one coercion rule; none of them may diverge.
    #[test]
    fn test_graph_walk_flags_agree() {
        let expr = dbt_yaml::from_str::<MethodAtomExpr>(
            r#"
            method: fqn
            value: a
            children: yes
            parents: yes
            childrens_parents: yes
            "#,
        )
        .expect("unquoted YAML 1.1 `yes` must resolve on every graph-walk flag");
        assert!(expr.children);
        assert!(expr.parents);
        assert!(expr.childrens_parents);
    }

    /// What `default:` resolves to by the time deserialization is done.
    #[derive(Debug, PartialEq)]
    enum ResolvedDefault {
        /// A boolean, or `None` for an absent or explicitly-null key.
        Resolved(Option<bool>),
        /// The value was rejected as not a boolean.
        Rejected,
    }

    /// Deserializes a `SelectorDefinition` carrying `line` as its `default:` and reports what
    /// the field ends up holding.
    ///
    /// The values here are what serde sees *after* Jinja rendering, so the string rows stand in
    /// for the Jinja spellings that render to a string. The templates themselves go through the
    /// real render pass in `dbt-parser`'s `test_jinja_authored_selector_default_resolves`.
    fn selector_default(line: &str) -> ResolvedDefault {
        let yaml = format!(
            r#"
            name: s
            {line}
            definition: 'fqn:a'
            "#
        );
        match dbt_yaml::from_str::<SelectorDefinition>(&yaml) {
            Err(_) => ResolvedDefault::Rejected,
            Ok(def) => ResolvedDefault::Resolved(def.default),
        }
    }

    /// `default:` must resolve the same YAML 1.1 boolean token set as the graph-walk flags, and
    /// must reject anything outside it, rather than keeping the value around as a string whose
    /// meaning is decided later by a bespoke truthiness map.
    ///
    /// dbt-core declares `default` as a JSON-schema boolean and validates selectors.yml against
    /// that schema before anything else looks at it (`core/dbt/config/selectors.py:33-47`), so
    /// every non-boolean below is a hard error there — reported, confusingly, as "Could not
    /// parse selector file data" with the offending value echoed, because that one message is
    /// appended to every schema failure. PyYAML has already resolved the YAML 1.1 tokens by
    /// that point, so unquoted `yes`/`no` reach it as real booleans and are accepted.
    ///
    /// Two rows are deliberately looser than core:
    ///
    /// - A *quoted* `"true"` resolves rather than erroring. By the time this runs it cannot be
    ///   told apart from the same string rendered by a Jinja template, which has to resolve.
    /// - An explicit `default:` with no value resolves to `None`, i.e. not the default selector.
    ///   Core errors. Erroring would reject a plausible authoring slip and drop the schema's
    ///   `null` branch for no behavioral gain.
    ///
    /// Every string row below used to be a bug, and not always a silent `false`: the value was
    /// kept as an unrendered string and, if the default turned out to be needed, run through a
    /// truthiness map whose fallback arm was "any non-empty string is true". So `default: no`
    /// made the selector the default — the one thing the author asked it not to be — while
    /// `default: no` plus an explicit `--select` left it alone. Reading the field eagerly
    /// settles both.
    ///
    /// Verified against dbt-core 2026-09-17 (dbt-state v2.47.0) with `cargo mantle ls` on a
    /// one-selector project per row; the recorded results are in
    /// `.agents/plans/*-selector-bool-jinja.md`.
    #[test]
    fn test_selector_default_accepts_yaml_11_bool_tokens() {
        use ResolvedDefault::*;

        // Genuine booleans, and the two spellings of "not set".
        assert_eq!(selector_default("default: true"), Resolved(Some(true)));
        assert_eq!(selector_default("default: false"), Resolved(Some(false)));
        assert_eq!(selector_default(""), Resolved(None));
        assert_eq!(selector_default("default: null"), Resolved(None));

        // Unquoted YAML 1.1 tokens. dbt-yaml's `yaml_11` feature resolves them to real
        // booleans, as PyYAML does for core.
        assert_eq!(selector_default("default: yes"), Resolved(Some(true)));
        assert_eq!(selector_default("default: no"), Resolved(Some(false)));
        assert_eq!(selector_default("default: on"), Resolved(Some(true)));
        assert_eq!(selector_default("default: off"), Resolved(Some(false)));
        assert_eq!(selector_default("default: YES"), Resolved(Some(true)));

        // Quoted booleans, and the same strings as produced by a mixed Jinja template.
        assert_eq!(selector_default(r#"default: "true""#), Resolved(Some(true)));
        assert_eq!(
            selector_default(r#"default: "false""#),
            Resolved(Some(false))
        );

        // The same, with the whitespace rendered text often carries around the word.
        assert_eq!(
            selector_default(r#"default: " no ""#),
            Resolved(Some(false))
        );
        assert_eq!(
            selector_default(r#"default: "true\n""#),
            Resolved(Some(true))
        );

        // Outside the token set: an error, not a value whose meaning is settled later.
        assert_eq!(selector_default("default: maybe"), Rejected);
        assert_eq!(selector_default(r#"default: "1""#), Rejected);
        assert_eq!(selector_default(r#"default: "tRue""#), Rejected);
        assert_eq!(selector_default("default: 1"), Rejected);
        assert_eq!(selector_default("default: 42"), Rejected);
        assert_eq!(selector_default("default: [1, 2]"), Rejected);
        assert_eq!(selector_default("default: {a: 1}"), Rejected);
    }
}
