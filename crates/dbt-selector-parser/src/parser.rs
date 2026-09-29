//! Convert YAML selectors (as parsed by `dbt-schemas`) into the
//! `SelectExpression` + *optional* `exclude` expression that the
//! scheduler understands.
//

use std::{collections::BTreeMap, slice, str::FromStr};

use dbt_common::{
    ErrorCode, FsResult, err, fs_err,
    node_selector::{
        IndirectSelection, MethodName, SelectExpression, SelectionCriteria, SelectionValue,
        parse_model_specifiers,
    },
};

use dbt_schemas::schemas::selectors::{
    AtomExpr, CompositeExpr, MethodAtomExpr, SelectorDefaultSpec, SelectorDefinition,
    SelectorDefinitionValue, SelectorExpr, SelectorMethodValue,
};

/// Splits a `tag` / `config.materialized`-style method string into its name and
/// sub-arguments, inferring the name from the value when unrecognised.
fn split_method(method: &str, value: &SelectorMethodValue) -> (MethodName, Vec<String>) {
    let mut parts = method.split('.').map(|s| s.to_string());
    let head = parts.next().unwrap();
    let name = MethodName::from_str(&head)
        .unwrap_or_else(|_| MethodName::default_for(value.scalar().map_or("", |v| v.as_str())));
    (name, parts.collect())
}

/// Crosses the crate boundary between the YAML-side and resolver-side value types.
fn to_selection_value(value: &SelectorMethodValue) -> SelectionValue {
    match value {
        SelectorMethodValue::Scalar(v) => SelectionValue::Scalar(v.to_string()),
        SelectorMethodValue::Unsupported(y) => SelectionValue::Unsupported(y.clone()),
    }
}

/// Builds the criteria for one method argument.
fn criteria_for_value(
    value: &SelectorMethodValue,
    mut new_criteria: impl FnMut(SelectionValue) -> SelectionCriteria,
) -> SelectExpression {
    SelectExpression::Atom(new_criteria(to_selection_value(value)))
}

#[derive(Debug, Clone)]
pub struct SelectorParser {
    defs: BTreeMap<String, SelectorDefinition>,
}

impl SelectorParser {
    pub fn new(defs: BTreeMap<String, SelectorDefinition>) -> Self {
        Self { defs }
    }

    pub fn parse_named(&self, name: &str) -> FsResult<SelectExpression> {
        let def = self
            .defs
            .get(name)
            .ok_or_else(|| fs_err!(ErrorCode::SelectorError, "Unknown selector `{}`", name))?;
        self.parse_definition(&def.definition)
    }

    pub fn parse_definition(&self, def: &SelectorDefinitionValue) -> FsResult<SelectExpression> {
        match def {
            SelectorDefinitionValue::String(s) => Ok(parse_model_specifiers(slice::from_ref(s))?),
            SelectorDefinitionValue::Full(expr) => self.parse_expr(expr),
            SelectorDefinitionValue::Array(items) => {
                let exprs = self.collect_definition_includes(items)?;
                Ok(SelectExpression::Or(exprs))
            }
        }
    }

    pub fn parse_expr(&self, expr: &SelectorExpr) -> FsResult<SelectExpression> {
        match expr {
            SelectorExpr::Composite(comp) => self.parse_composite(comp),
            SelectorExpr::Atom(atom) => self.parse_atom(atom),
        }
    }

    pub fn parse_composite(&self, comp: &CompositeExpr) -> FsResult<SelectExpression> {
        let mut includes = Vec::new();
        let mut exclude_exprs = Vec::new();

        let (is_union, values) = match (&comp.union, &comp.intersection) {
            (Some(vals), None) => (true, vals),
            (None, Some(vals)) => (false, vals),
            (Some(_), Some(_)) => {
                return Err(fs_err!(
                    ErrorCode::SelectorError,
                    "selector definition has both union and intersection — use one"
                ));
            }
            (None, None) => {
                return Err(fs_err!(
                    ErrorCode::SelectorError,
                    "selector definition is missing union or intersection"
                ));
            }
        };

        for value in values {
            // Check if this value is an exclude expression
            if let SelectorDefinitionValue::Full(SelectorExpr::Atom(AtomExpr::Exclude(exclude))) =
                value
            {
                // Handle exclude as a special case within composite expressions
                let exprs = self.collect_definition_includes(&exclude.exclude)?;
                let exclude_expression = match exprs.len() {
                    0 => return Err(fs_err!(ErrorCode::SelectorError, "Empty exclude list")),
                    1 => exprs.into_iter().next().unwrap(),
                    _ => SelectExpression::Or(exprs),
                };
                exclude_exprs.push(exclude_expression);
            } else {
                // Handle regular include expressions
                let resolved = self.parse_definition(value)?;
                includes.push(resolved);
            }
        }

        let include_expr = if is_union {
            SelectExpression::Or(includes)
        } else {
            SelectExpression::And(includes)
        };

        // Collect top-level exclude: [...] (sibling of union/intersection at definition level).
        // dbt-core semantics: all items are combined with OR into a single exclusion.
        if let Some(top_excludes) = &comp.exclude {
            if !top_excludes.is_empty() {
                let excl_exprs = self.collect_definition_includes(top_excludes)?;
                let combined = if excl_exprs.len() == 1 {
                    excl_exprs.into_iter().next().unwrap()
                } else {
                    SelectExpression::Or(excl_exprs)
                };
                exclude_exprs.push(combined);
            }
        }

        // If we have exclude expressions, combine them
        if !exclude_exprs.is_empty() {
            let combined_exclude = if exclude_exprs.len() == 1 {
                exclude_exprs.into_iter().next().unwrap()
            } else {
                SelectExpression::Or(exclude_exprs)
            };

            return Ok(SelectExpression::And(vec![
                include_expr,
                SelectExpression::Exclude(Box::new(combined_exclude)),
            ]));
        }

        Ok(include_expr)
    }

    fn parse_atom(&self, atom: &AtomExpr) -> FsResult<SelectExpression> {
        match atom {
            AtomExpr::Method(expr) => {
                self.atom_to_select_expression(AtomExpr::Method(MethodAtomExpr {
                    method: expr.method.clone(),
                    value: expr.value.clone(),
                    childrens_parents: expr.childrens_parents.clone(),
                    parents: expr.parents.clone(),
                    children: expr.children.clone(),
                    parents_depth: expr.parents_depth,
                    children_depth: expr.children_depth,
                    indirect_selection: expr.indirect_selection,
                    exclude: expr.exclude.clone(),
                }))
            }

            AtomExpr::MethodKey(method_value) => {
                if method_value.len() != 1 {
                    return Err(fs_err!(
                        ErrorCode::SelectorError,
                        "MethodKey must have exactly one key-value pair"
                    ));
                }
                let (m, v) = method_value.iter().next().unwrap();
                let wrapper = AtomExpr::Method(MethodAtomExpr {
                    method: m.clone(),
                    value: v.clone(),
                    childrens_parents: SelectorDefaultSpec::from(false),
                    parents: SelectorDefaultSpec::from(false),
                    children: SelectorDefaultSpec::from(false),
                    parents_depth: None,
                    children_depth: None,
                    indirect_selection: Some(IndirectSelection::default()),
                    exclude: None,
                });
                self.atom_to_select_expression(wrapper)
            }

            AtomExpr::Exclude(_) => {
                err!(
                    ErrorCode::SelectorError,
                    "Top level exclude not allowed in YAML selectors"
                )
            }
        }
    }

    fn collect_definition_includes(
        &self,
        defs: &[SelectorDefinitionValue],
    ) -> FsResult<Vec<SelectExpression>> {
        defs.iter().map(|dv| self.parse_definition(dv)).collect()
    }

    fn atom_to_select_expression(&self, atom: AtomExpr) -> FsResult<SelectExpression> {
        match atom {
            AtomExpr::Method(expr) => {
                let value = expr.value;
                let childrens_parents = expr.childrens_parents.as_bool();
                let parents = expr.parents.as_bool();
                let children = expr.children.as_bool();
                let parents_depth = expr.parents_depth;
                let children_depth = expr.children_depth;
                let indirect_selection = expr.indirect_selection;
                let exclude = expr.exclude;
                // ── 1️⃣  resolve method / args ────────────────────────────────
                let (name, args) = split_method(&expr.method, &value);

                // ── 2️⃣  normalise depth flags ────────────────────────────────
                let pd = if parents && parents_depth.is_none() {
                    Some(u32::MAX)
                } else {
                    parents_depth
                };
                let cd = if children && children_depth.is_none() {
                    Some(u32::MAX)
                } else {
                    children_depth
                };

                // ── 3️⃣  build *nested* exclude expression (if present) ───────
                let exclude_expr: Option<Box<SelectExpression>> = if let Some(defs) = &exclude {
                    let exprs = defs
                        .iter()
                        .map(|d| self.parse_definition(d))
                        .collect::<FsResult<Vec<_>>>()?;
                    match exprs.len() {
                        0 => None,
                        1 => Some(Box::new(exprs.into_iter().next().unwrap())),
                        _ => Some(Box::new(SelectExpression::Or(exprs))),
                    }
                } else {
                    None
                };

                // ── 4️⃣  assemble criteria ────────────────────────────────────
                Ok(criteria_for_value(&value, |v| {
                    SelectionCriteria::new(
                        name,
                        args.clone(),
                        v,
                        childrens_parents,
                        pd,
                        cd,
                        indirect_selection,
                        exclude_expr.clone(),
                    )
                }))
            }
            AtomExpr::MethodKey(method_value) => {
                let (m, v) = method_value.into_iter().next().unwrap();
                let (name, args) = split_method(&m, &v);
                Ok(criteria_for_value(&v, |value| {
                    SelectionCriteria::new(
                        name,
                        args.clone(),
                        value,
                        false,
                        None,
                        None,
                        Some(IndirectSelection::default()),
                        None,
                    )
                }))
            }
            AtomExpr::Exclude(expr) => {
                // A standalone exclude atom - this becomes a top-level exclude
                let exprs = self.collect_definition_includes(&expr.exclude)?;
                let exclude_expr = match exprs.len() {
                    0 => return Err(fs_err!(ErrorCode::SelectorError, "Empty exclude list")),
                    1 => exprs.into_iter().next().unwrap(),
                    _ => SelectExpression::Or(exprs),
                };
                Ok(SelectExpression::Exclude(Box::new(exclude_expr)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::selectors::{ExcludeAtomExpr, SelectorMethodValue, SelectorValue};
    use dbt_test_primitives::assert_contains;

    // ============================================================================
    // 1. Basic Atom Selectors
    // ============================================================================

    #[test]
    /// Test parsing of simple string selectors like "model_a".
    /// Expects an Atom expression with FQN method and the given value.
    fn test_string_selector() -> FsResult<()> {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);
        let result =
            parser.parse_definition(&SelectorDefinitionValue::String("model_a".to_string()))?;

        if let SelectExpression::Atom(criteria) = result {
            assert_eq!(criteria.method, MethodName::Fqn);
            assert_eq!(criteria.value, "model_a");
            assert!(!criteria.childrens_parents);
            assert!(criteria.parents_depth.is_none());
            assert!(criteria.children_depth.is_none());
        } else {
            panic!("Expected Atom expression");
        }
        Ok(())
    }

    #[test]
    /// Test that both string and full YAML definition formats parse identically.
    /// Expects the same Atom expression result regardless of definition format.
    fn test_full_vs_string_definitions() -> FsResult<()> {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        let expr = SelectorExpr::Atom(AtomExpr::Method(MethodAtomExpr {
            method: "tag".to_string(),
            value: SelectorValue::from("nightly").into(),
            childrens_parents: SelectorDefaultSpec::from(false),
            parents: SelectorDefaultSpec::from(false),
            children: SelectorDefaultSpec::from(false),
            parents_depth: None,
            children_depth: None,
            indirect_selection: Some(IndirectSelection::default()),
            exclude: None,
        }));

        let result = parser.parse_definition(&SelectorDefinitionValue::Full(expr))?;

        if let SelectExpression::Atom(criteria) = result {
            assert_eq!(criteria.method, MethodName::Tag);
            assert_eq!(criteria.value, "nightly");
        } else {
            panic!("Expected Atom expression");
        }
        Ok(())
    }

    #[test]
    /// Test parsing of shorthand method key syntax like `tag: nightly`.
    /// Expects an Atom expression with the specified method and value.
    fn test_method_key_selector() -> FsResult<()> {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        let mut method_value = BTreeMap::new();
        method_value.insert("tag".to_string(), SelectorValue::from("nightly").into());

        let result = parser.parse_atom(&AtomExpr::MethodKey(method_value))?;

        if let SelectExpression::Atom(criteria) = result {
            assert_eq!(criteria.method, MethodName::Tag);
            assert_eq!(criteria.value, "nightly");
            assert!(!criteria.childrens_parents);
            assert!(criteria.parents_depth.is_none());
            assert!(criteria.children_depth.is_none());
            assert_eq!(criteria.indirect, Some(IndirectSelection::default()));
        } else {
            panic!("Expected Atom expression");
        }
        Ok(())
    }

    #[test]
    /// Test that MethodKey with multiple pairs fails validation.
    /// Expects an error indicating exactly one key-value pair is required.
    fn test_method_key_multiple_pairs() {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        let mut method_value = BTreeMap::new();
        method_value.insert("tag".to_string(), SelectorValue::from("nightly").into());
        method_value.insert("path".to_string(), SelectorValue::from("models/").into());

        let result = parser.parse_atom(&AtomExpr::MethodKey(method_value));
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.code, ErrorCode::SelectorError);
            assert_contains!(
                e.to_string(),
                "MethodKey must have exactly one key-value pair"
            );
        }
    }

    #[test]
    /// Regression (dbt-labs/fs#13604): a sequence-valued method argument
    /// (`value: ["a", "b"]`), which dbt Core loads as `Any`, must parse
    /// instead of being eagerly rejected, and defer rather than fan out.
    fn test_sequence_valued_method_argument() -> FsResult<()> {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        let expr = SelectorExpr::Atom(AtomExpr::Method(MethodAtomExpr {
            method: "path".to_string(),
            value: SelectorMethodValue::Unsupported(Box::new(dbt_yaml::Value::from(vec![
                dbt_yaml::Value::from("models"),
                dbt_yaml::Value::from("seeds"),
            ]))),
            childrens_parents: SelectorDefaultSpec::from(false),
            parents: SelectorDefaultSpec::from(false),
            children: SelectorDefaultSpec::from(false),
            parents_depth: None,
            children_depth: None,
            indirect_selection: Some(IndirectSelection::default()),
            exclude: None,
        }));

        let result = parser.parse_definition(&SelectorDefinitionValue::Full(expr))?;

        if let SelectExpression::Atom(criteria) = result {
            assert_eq!(criteria.method, MethodName::Path);
            assert!(
                matches!(criteria.value, SelectionValue::Unsupported(_)),
                "expected a sequence value to be deferred, not fanned out"
            );
        } else {
            panic!("Expected a single deferred Atom, got {result:?}");
        }
        Ok(())
    }

    #[test]
    /// An empty sequence (`value: []`) is deferred too, not treated as a scalar.
    fn test_empty_sequence_valued_method_argument() -> FsResult<()> {
        let parser = SelectorParser::new(BTreeMap::new());

        let expr = SelectorExpr::Atom(AtomExpr::Method(MethodAtomExpr {
            method: "not_a_known_method".to_string(),
            value: SelectorMethodValue::Unsupported(Box::new(dbt_yaml::Value::from(Vec::<
                dbt_yaml::Value,
            >::new(
            )))),
            childrens_parents: SelectorDefaultSpec::from(false),
            parents: SelectorDefaultSpec::from(false),
            children: SelectorDefaultSpec::from(false),
            parents_depth: None,
            children_depth: None,
            indirect_selection: Some(IndirectSelection::default()),
            exclude: None,
        }));

        let result = parser.parse_definition(&SelectorDefinitionValue::Full(expr))?;

        if let SelectExpression::Atom(criteria) = result {
            assert!(matches!(criteria.value, SelectionValue::Unsupported(_)));
        } else {
            panic!("Expected a single deferred Atom, got {result:?}");
        }
        Ok(())
    }

    // ============================================================================
    // 2. Composite Operation Basics
    // ============================================================================

    #[test]
    /// Test basic union and intersection operations with mixed include/exclude scenarios.
    /// Expects Or for unions, And for intersections, with excludes nested within method criteria.
    fn test_composite_operations() -> FsResult<()> {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        // Test union
        let union_result = parser.parse_composite(&CompositeExpr::union(vec![
            SelectorDefinitionValue::String("model_a".to_string()),
            SelectorDefinitionValue::String("model_b".to_string()),
        ]))?;

        if let SelectExpression::Or(exprs) = union_result {
            assert_eq!(exprs.len(), 2);
        } else {
            panic!("Expected Or expression for union");
        }

        // Test intersection
        let intersection_result = parser.parse_composite(&CompositeExpr::intersection(vec![
            SelectorDefinitionValue::String("model_a".to_string()),
            SelectorDefinitionValue::String("model_b".to_string()),
        ]))?;

        if let SelectExpression::And(exprs) = intersection_result {
            assert_eq!(exprs.len(), 2);
        } else {
            panic!("Expected And expression for intersection");
        }

        // Test composite with excludes - excludes should be nested within the include
        let composite_with_exclude = parser.parse_composite(&CompositeExpr::union(vec![
            SelectorDefinitionValue::String("tag:bar".to_string()),
            SelectorDefinitionValue::Full(SelectorExpr::Atom(AtomExpr::Method(MethodAtomExpr {
                method: "tag".to_string(),
                value: SelectorValue::from("baz").into(),
                childrens_parents: SelectorDefaultSpec::from(false),
                parents: SelectorDefaultSpec::from(false),
                children: SelectorDefaultSpec::from(false),
                parents_depth: None,
                children_depth: None,
                indirect_selection: None,
                exclude: Some(vec![SelectorDefinitionValue::String(
                    "single_exclude".to_string(),
                )]),
            }))),
        ]))?;

        // The result should be an Or with one regular atom and one atom with nested exclude
        if let SelectExpression::Or(exprs) = composite_with_exclude {
            assert_eq!(exprs.len(), 2);
            // First should be a regular atom
            if let SelectExpression::Atom(criteria) = &exprs[0] {
                assert_eq!(criteria.method, MethodName::Tag);
                assert_eq!(criteria.value, "bar");
            } else {
                panic!("Expected first expression to be Atom");
            }
            // Second should be an Atom with nested exclude
            if let SelectExpression::Atom(criteria) = &exprs[1] {
                assert_eq!(criteria.method, MethodName::Tag);
                assert_eq!(criteria.value, "baz");
                // Check that exclude is nested within the criteria
                if let Some(exclude_expr) = &criteria.exclude {
                    if let SelectExpression::Atom(exclude_criteria) = &**exclude_expr {
                        assert_eq!(exclude_criteria.method, MethodName::Fqn);
                        assert_eq!(exclude_criteria.value, "single_exclude");
                    } else {
                        panic!("Expected Atom inside nested exclude");
                    }
                } else {
                    panic!("Expected nested exclude in criteria");
                }
            } else {
                panic!("Expected second expression to be Atom with nested exclude");
            }
        } else {
            panic!("Expected Or expression for composite");
        }

        Ok(())
    }

    // ============================================================================
    // 3. Exclude Scenarios (Progressive)
    // ============================================================================

    #[test]
    /// Test exclude handling in method atoms, both single and multiple excludes.
    /// Expects nested exclude expressions within SelectionCriteria: single excludes as Atom, multiple as Or.
    fn test_exclude_handling() -> FsResult<()> {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        // Test single exclude - should be nested within SelectionCriteria
        let single_result = parser.parse_atom(&AtomExpr::Method(MethodAtomExpr {
            method: "tag".to_string(),
            value: SelectorValue::from("nightly").into(),
            childrens_parents: SelectorDefaultSpec::from(false),
            parents: SelectorDefaultSpec::from(false),
            children: SelectorDefaultSpec::from(false),
            parents_depth: None,
            children_depth: None,
            indirect_selection: Some(IndirectSelection::default()),
            exclude: Some(vec![SelectorDefinitionValue::String(
                "model_to_exclude".to_string(),
            )]),
        }))?;

        // The result should be an Atom with nested exclude
        if let SelectExpression::Atom(criteria) = single_result {
            assert_eq!(criteria.method, MethodName::Tag);
            assert_eq!(criteria.value, "nightly");
            // Check that exclude is nested within the criteria
            if let Some(exclude_expr) = criteria.exclude {
                if let SelectExpression::Atom(exclude_criteria) = *exclude_expr {
                    assert_eq!(exclude_criteria.method, MethodName::Fqn);
                    assert_eq!(exclude_criteria.value, "model_to_exclude");
                } else {
                    panic!("Expected Atom expression inside nested exclude");
                }
            } else {
                panic!("Expected nested exclude in criteria");
            }
        } else {
            panic!("Expected Atom expression");
        }

        // Test multiple excludes - should be nested within SelectionCriteria as Or
        let multiple_result = parser.parse_atom(&AtomExpr::Method(MethodAtomExpr {
            method: "tag".to_string(),
            value: SelectorValue::from("nightly").into(),
            childrens_parents: SelectorDefaultSpec::from(false),
            parents: SelectorDefaultSpec::from(false),
            children: SelectorDefaultSpec::from(false),
            parents_depth: None,
            children_depth: None,
            indirect_selection: Some(IndirectSelection::default()),
            exclude: Some(vec![
                SelectorDefinitionValue::String("model_a".to_string()),
                SelectorDefinitionValue::String("model_b".to_string()),
            ]),
        }))?;

        // The result should be an Atom with nested exclude containing Or
        if let SelectExpression::Atom(criteria) = multiple_result {
            assert_eq!(criteria.method, MethodName::Tag);
            assert_eq!(criteria.value, "nightly");
            // Check that exclude is nested within the criteria as Or
            if let Some(exclude_expr) = criteria.exclude {
                if let SelectExpression::Or(exprs) = *exclude_expr {
                    assert_eq!(exprs.len(), 2);
                    if let (SelectExpression::Atom(a), SelectExpression::Atom(b)) =
                        (&exprs[0], &exprs[1])
                    {
                        assert_eq!(a.method, MethodName::Fqn);
                        assert_eq!(a.value, "model_a");
                        assert_eq!(b.method, MethodName::Fqn);
                        assert_eq!(b.value, "model_b");
                    } else {
                        panic!("Expected Atom expressions in nested exclude");
                    }
                } else {
                    panic!("Expected Or expression inside nested exclude");
                }
            } else {
                panic!("Expected nested exclude in criteria");
            }
        } else {
            panic!("Expected Atom expression");
        }
        Ok(())
    }

    #[test]
    /// Test that standalone exclude atoms (not in composite) are rejected.
    /// Expects an error indicating top-level excludes are not allowed in YAML selectors.
    fn test_standalone_exclude() {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        let result = parser.parse_atom(&AtomExpr::Exclude(ExcludeAtomExpr {
            exclude: vec![SelectorDefinitionValue::String("model_exclude".to_string())],
        }));

        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.code, ErrorCode::SelectorError);
            assert_contains!(
                e.to_string(),
                "Top level exclude not allowed in YAML selectors"
            );
        }
    }

    // Helper to create a string selector
    fn s(val: &str) -> SelectorDefinitionValue {
        SelectorDefinitionValue::String(val.to_string())
    }

    // Helper to create an exclude block
    fn exclude(vals: Vec<&str>) -> SelectorDefinitionValue {
        let exclude_vals = vals.into_iter().map(s).collect();
        SelectorDefinitionValue::Full(SelectorExpr::Atom(AtomExpr::Exclude(ExcludeAtomExpr {
            exclude: exclude_vals,
        })))
    }

    // Helper to create a composite selector
    fn composite(kind: &str, items: Vec<SelectorDefinitionValue>) -> SelectorDefinitionValue {
        let expr = match kind {
            "union" => CompositeExpr::union(items),
            "intersection" => CompositeExpr::intersection(items),
            _ => panic!("Unknown kind"),
        };
        SelectorDefinitionValue::Full(SelectorExpr::Composite(expr))
    }

    #[test]
    /// Test basic union with single exclude: union: [A, exclude: [B]]
    /// Expects And([Or([A]), Exclude(B)]) - union includes wrapped with exclude.
    fn test_basic_union_with_exclude() -> FsResult<()> {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        // union: [A, exclude: [B]]
        // Logic: (A) AND NOT (B)

        let def = composite("union", vec![s("tag:A"), exclude(vec!["tag:B"])]);
        let result = parser.parse_definition(&def)?;

        if let SelectExpression::And(exprs) = result {
            // [Or([A]), Exclude(B)]
            assert_eq!(exprs.len(), 2);
            if let SelectExpression::Or(inc) = &exprs[0] {
                assert_eq!(inc.len(), 1);
                if let SelectExpression::Atom(c) = &inc[0] {
                    assert_eq!(c.value, "A");
                }
            }
            if let SelectExpression::Exclude(exc) = &exprs[1] {
                if let SelectExpression::Atom(c) = &**exc {
                    assert_eq!(c.value, "B");
                }
            }
        } else {
            panic!("Expected And(Or([A]), Exclude(B)), got {:?}", result);
        }
        Ok(())
    }

    #[test]
    /// Test basic intersection with single exclude: intersection: [A, exclude: [B]]
    /// Expects And([And([A]), Exclude(B)]) - intersection includes wrapped with exclude.
    fn test_basic_intersection_with_exclude() -> FsResult<()> {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        // intersection: [A, exclude: [B]]
        // Logic: (A) AND NOT (B)

        let def = composite("intersection", vec![s("tag:A"), exclude(vec!["tag:B"])]);
        let result = parser.parse_definition(&def)?;

        if let SelectExpression::And(exprs) = result {
            // [And([A]), Exclude(B)]
            assert_eq!(exprs.len(), 2);
            if let SelectExpression::And(inc) = &exprs[0] {
                if let SelectExpression::Atom(c) = &inc[0] {
                    assert_eq!(c.value, "A");
                }
            }
            if let SelectExpression::Exclude(exc) = &exprs[1] {
                if let SelectExpression::Atom(c) = &**exc {
                    assert_eq!(c.value, "B");
                }
            }
        } else {
            panic!("Expected And(And([A]), Exclude(B)), got {:?}", result);
        }
        Ok(())
    }

    #[test]
    /// Test union with multiple exclude blocks: union: [A, exclude: [B], exclude: [C]]
    /// Expects And([Or([A]), Exclude(Or([B, C]))]) - multiple excludes combined as Or.
    fn test_multiple_excludes_union() -> FsResult<()> {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        // union: [A, exclude: [B], exclude: [C]]
        // Logic: (A) AND NOT (B OR C)

        let def = composite(
            "union",
            vec![s("tag:A"), exclude(vec!["tag:B"]), exclude(vec!["tag:C"])],
        );
        let result = parser.parse_definition(&def)?;

        if let SelectExpression::And(exprs) = result {
            // [Or([A]), Exclude(Or([B, C]))]
            let ex = &exprs[1];
            if let SelectExpression::Exclude(inner) = ex {
                if let SelectExpression::Or(list) = &**inner {
                    let vals: Vec<String> = list
                        .iter()
                        .map(|e| {
                            if let SelectExpression::Atom(c) = e {
                                c.value.to_string()
                            } else {
                                "".to_string()
                            }
                        })
                        .collect();
                    assert!(vals.contains(&"B".to_string()));
                    assert!(vals.contains(&"C".to_string()));
                } else {
                    panic!("Expected Or inside Exclude");
                }
            } else {
                panic!("Expected Exclude");
            }
        } else {
            panic!("Expected And");
        }
        Ok(())
    }

    #[test]
    /// Test intersection with multiple exclude blocks: intersection: [A, exclude: [B], exclude: [C]]
    /// Expects And([And([A]), Exclude(Or([B, C]))]) - multiple excludes combined as Or.
    fn test_multiple_excludes_intersection() -> FsResult<()> {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        // intersection: [A, exclude: [B], exclude: [C]]
        // Logic: (A) AND NOT (B OR C)

        let def = composite(
            "intersection",
            vec![s("tag:A"), exclude(vec!["tag:B"]), exclude(vec!["tag:C"])],
        );
        let result = parser.parse_definition(&def)?;

        if let SelectExpression::And(exprs) = result {
            // [And([A]), Exclude(Or([B, C]))]
            let ex = &exprs[1];
            if let SelectExpression::Exclude(inner) = ex {
                if let SelectExpression::Or(list) = &**inner {
                    let vals: Vec<String> = list
                        .iter()
                        .map(|e| {
                            if let SelectExpression::Atom(c) = e {
                                c.value.to_string()
                            } else {
                                "".to_string()
                            }
                        })
                        .collect();
                    assert!(vals.contains(&"B".to_string()));
                    assert!(vals.contains(&"C".to_string()));
                } else {
                    panic!("Expected Or inside Exclude");
                }
            }
        }
        Ok(())
    }

    // ============================================================================
    // 4. Complex Nested Logic
    // ============================================================================

    #[test]
    /// Test complex nested structure: union of intersections with excludes.
    /// Expects Or([And([A, And([Or([B,C]), Exclude(D)])]), And([E,F])]) - deeply nested excludes.
    fn test_union_of_intersections_with_exclude() -> FsResult<()> {
        // Mimics the user case structure but generic
        // union:
        //   - intersection: [A, union: [B, C, exclude: [D]]]
        //   - intersection: [E, F]

        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        let inner_union = composite(
            "union",
            vec![s("tag:B"), s("tag:C"), exclude(vec!["tag:D"])],
        );

        let intersection1 = composite("intersection", vec![s("tag:A"), inner_union]);

        let intersection2 = composite("intersection", vec![s("tag:E"), s("tag:F")]);

        let top = composite("union", vec![intersection1, intersection2]);

        let result = parser.parse_definition(&top)?;

        // Structure:
        // Or([
        //   And([ A, And([ Or([B, C]), Exclude(D) ]) ]),  <-- Intersection 1
        //   And([ E, F ])                                 <-- Intersection 2
        // ])

        if let SelectExpression::Or(top_list) = result {
            assert_eq!(top_list.len(), 2);

            // Check Intersection 1
            if let SelectExpression::And(i1) = &top_list[0] {
                // A, InnerUnion
                if let SelectExpression::Atom(a) = &i1[0] {
                    assert_eq!(a.value, "A");
                }
                if let SelectExpression::And(u) = &i1[1] {
                    // Or([B,C]), Exclude(D)
                    if let SelectExpression::Or(bc) = &u[0] {
                        assert_eq!(bc.len(), 2);
                    }
                    if let SelectExpression::Exclude(d) = &u[1] {
                        if let SelectExpression::Atom(da) = &**d {
                            assert_eq!(da.value, "D");
                        }
                    }
                }
            }

            // Check Intersection 2
            if let SelectExpression::And(i2) = &top_list[1] {
                assert_eq!(i2.len(), 2);
            }
        }

        Ok(())
    }

    #[test]
    /// Test complex nested structure: intersection of unions with excludes.
    /// Expects And([And([Or([A]), Exclude(B)]), And([Or([C]), Exclude(D)])]) - multiple union/exclude pairs.
    fn test_intersection_of_unions_with_exclude() -> FsResult<()> {
        // intersection:
        //   - union: [A, exclude: [B]]
        //   - union: [C, exclude: [D]]

        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        let u1 = composite("union", vec![s("tag:A"), exclude(vec!["tag:B"])]);
        let u2 = composite("union", vec![s("tag:C"), exclude(vec!["tag:D"])]);

        let top = composite("intersection", vec![u1, u2]);

        let result = parser.parse_definition(&top)?;

        // And([
        //   And([ Or([A]), Exclude(B) ]),
        //   And([ Or([C]), Exclude(D) ])
        // ])

        if let SelectExpression::And(top_list) = result {
            assert_eq!(top_list.len(), 2);
            // Verify structure roughly
            if let SelectExpression::And(u1_res) = &top_list[0] {
                if let SelectExpression::Exclude(ex) = &u1_res[1] {
                    if let SelectExpression::Atom(c) = &**ex {
                        assert_eq!(c.value, "B");
                    }
                }
            }
            if let SelectExpression::And(u2_res) = &top_list[1] {
                if let SelectExpression::Exclude(ex) = &u2_res[1] {
                    if let SelectExpression::Atom(c) = &**ex {
                        assert_eq!(c.value, "D");
                    }
                }
            }
        }

        Ok(())
    }

    #[test]
    /// Test deeply nested excludes: union containing exclude of union with exclude.
    /// Expects Or([Exclude(And([Or([A]), Exclude(B)]))]) - nested exclude structures.
    fn test_deeply_nested_excludes() -> FsResult<()> {
        // union:
        //   - exclude:
        //       - union:
        //           - A
        //           - exclude: [B]

        // Logic: NOT ( (A) AND NOT (B) )
        // Wait, "exclude" takes a list of definitions.
        // If I exclude a composite...

        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        let inner = composite("union", vec![s("tag:A"), exclude(vec!["tag:B"])]);
        // The exclude atom contains a list of definitions. `inner` is a definition (Full).

        let exclude_inner =
            SelectorDefinitionValue::Full(SelectorExpr::Atom(AtomExpr::Exclude(ExcludeAtomExpr {
                exclude: vec![inner],
            })));

        let top = composite("union", vec![exclude_inner]); // Just an exclude at top level wrapped in union

        let result = parser.parse_definition(&top)?;

        // Union([ Exclude( ... ) ]) -> Or([ Exclude(...) ])
        // Inside Exclude: The result of parsing `inner`.
        // `inner` parses to: And([ Or([A]), Exclude(B) ])
        // So: Or([ Exclude( And([ Or([A]), Exclude(B) ]) ) ])

        if let SelectExpression::Or(list) = result {
            if let SelectExpression::Exclude(inner_res) = &list[0] {
                // And([ Or([A]), Exclude(B) ])
                if let SelectExpression::And(parts) = &**inner_res {
                    assert_eq!(parts.len(), 2);
                    if let SelectExpression::Exclude(b) = &parts[1] {
                        if let SelectExpression::Atom(c) = &**b {
                            assert_eq!(c.value, "B");
                        }
                    }
                } else {
                    panic!("Expected And inside Exclude");
                }
            } else {
                panic!("Expected Exclude");
            }
        }

        Ok(())
    }

    // ============================================================================
    // 5. Advanced Configuration and Graph Features
    // ============================================================================

    #[test]
    /// Test graph operators (parents, children, childrens_parents) and depth limits.
    /// Expects proper configuration of depth flags and indirect selection modes.
    fn test_graph_operators() -> FsResult<()> {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        let result = parser.parse_atom(&AtomExpr::Method(MethodAtomExpr {
            method: "tag".to_string(),
            value: SelectorValue::from("nightly").into(),
            childrens_parents: SelectorDefaultSpec::from(true),
            parents: SelectorDefaultSpec::from(true),
            children: SelectorDefaultSpec::from(true),
            parents_depth: Some(2),
            children_depth: Some(3),
            indirect_selection: Some(IndirectSelection::Cautious),
            exclude: None,
        }))?;

        if let SelectExpression::Atom(criteria) = result {
            assert_eq!(criteria.method, MethodName::Tag);
            assert_eq!(criteria.value, "nightly");
            assert!(criteria.childrens_parents);
            assert_eq!(criteria.parents_depth, Some(2));
            assert_eq!(criteria.children_depth, Some(3));
            assert_eq!(criteria.indirect, Some(IndirectSelection::Cautious));
        } else {
            panic!("Expected Atom expression");
        }
        Ok(())
    }

    #[test]
    /// Test that indirect selection modes propagate through nested expression trees.
    /// Expects all nested atom expressions to have the updated indirect selection setting.
    fn test_indirect_selection_propagation() -> FsResult<()> {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        let expr = SelectorExpr::Composite(CompositeExpr::intersection(vec![
            SelectorDefinitionValue::String("model_a".to_string()),
            SelectorDefinitionValue::String("model_b".to_string()),
        ]));

        let mut result = parser.parse_expr(&expr)?;

        // Set indirect selection mode
        result.set_indirect_selection(IndirectSelection::Cautious);

        // Verify the change propagated to all nested expressions
        if let SelectExpression::And(exprs) = &result {
            for expr in exprs {
                if let SelectExpression::Atom(criteria) = expr {
                    assert_eq!(criteria.indirect, Some(IndirectSelection::Cautious));
                } else {
                    panic!("Expected Atom expression");
                }
            }
        } else {
            panic!("Expected And expression");
        }
        Ok(())
    }

    #[test]
    /// Test selector method produces a Selector atom (resolved at runtime, not inlined).
    fn test_selector_method_produces_atom() -> FsResult<()> {
        let mut defs = BTreeMap::new();
        defs.insert(
            "foo_and_bar".to_string(),
            SelectorDefinition {
                name: "foo_and_bar".to_string(),
                description: None,
                default: None.into(),
                definition: SelectorDefinitionValue::Full(SelectorExpr::Composite(
                    CompositeExpr::intersection(vec![
                        SelectorDefinitionValue::String("tag:foo".to_string()),
                        SelectorDefinitionValue::String("tag:bar".to_string()),
                    ]),
                )),
            },
        );

        let parser = SelectorParser::new(defs);

        let result = parser.parse_atom(&AtomExpr::Method(MethodAtomExpr {
            method: "selector".to_string(),
            value: SelectorValue::from("foo_and_bar").into(),
            childrens_parents: SelectorDefaultSpec::from(false),
            parents: SelectorDefaultSpec::from(false),
            children: SelectorDefaultSpec::from(false),
            parents_depth: None,
            children_depth: None,
            indirect_selection: None,
            exclude: Some(vec![SelectorDefinitionValue::String(
                "tag:buzz".to_string(),
            )]),
        }))?;

        // Now produces a Selector atom with graph operators preserved
        if let SelectExpression::Atom(criteria) = &result {
            assert_eq!(criteria.method, MethodName::Selector);
            assert_eq!(criteria.value, "foo_and_bar");
            assert!(criteria.exclude.is_some());
        } else {
            panic!("Expected Atom(Selector) expression, got {:?}", result);
        }

        Ok(())
    }

    #[test]
    /// Test selector method preserves graph operators and nested excludes.
    fn test_selector_method_preserves_graph_operators() -> FsResult<()> {
        let mut defs = BTreeMap::new();
        defs.insert(
            "base_sel".to_string(),
            SelectorDefinition {
                name: "base_sel".to_string(),
                description: None,
                default: None.into(),
                definition: SelectorDefinitionValue::String("tag:production".to_string()),
            },
        );

        let parser = SelectorParser::new(defs);

        let result = parser.parse_atom(&AtomExpr::Method(MethodAtomExpr {
            method: "selector".to_string(),
            value: SelectorValue::from("base_sel").into(),
            childrens_parents: SelectorDefaultSpec::from(false),
            parents: SelectorDefaultSpec::from(true),
            children: SelectorDefaultSpec::from(true),
            parents_depth: Some(1),
            children_depth: Some(2),
            indirect_selection: None,
            exclude: Some(vec![SelectorDefinitionValue::String(
                "additional_exclude".to_string(),
            )]),
        }))?;

        if let SelectExpression::Atom(criteria) = &result {
            assert_eq!(criteria.method, MethodName::Selector);
            assert_eq!(criteria.value, "base_sel");
            assert_eq!(criteria.parents_depth, Some(1));
            assert_eq!(criteria.children_depth, Some(2));
            assert!(criteria.exclude.is_some());
        } else {
            panic!("Expected Atom(Selector) expression, got {:?}", result);
        }

        Ok(())
    }

    // ============================================================================
    // 6. High-level Resolution and Errors
    // ============================================================================

    #[test]
    /// Test parsing a selector by name from the provided definitions map.
    /// Expects the selector definition to be resolved and parsed correctly.
    fn test_named_selector() -> FsResult<()> {
        let mut defs = BTreeMap::new();
        defs.insert(
            "nightly_models".to_string(),
            SelectorDefinition {
                name: "nightly_models".to_string(),
                description: None,
                default: None.into(),
                definition: SelectorDefinitionValue::String("tag:nightly".to_string()),
            },
        );

        let parser = SelectorParser::new(defs);
        let result = parser.parse_named("nightly_models")?;

        if let SelectExpression::Atom(criteria) = result {
            assert_eq!(criteria.method, MethodName::Tag);
            assert_eq!(criteria.value, "nightly");
        } else {
            panic!("Expected Atom expression");
        }
        Ok(())
    }

    #[test]
    /// Test various error scenarios including unknown selectors and inheritance failures.
    /// Expects appropriate error codes and messages for invalid selector references.
    fn test_error_handling() {
        let defs = BTreeMap::new();
        let parser = SelectorParser::new(defs);

        // Test unknown selector
        let result = parser.parse_named("unknown");
        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.code, ErrorCode::SelectorError);
            assert_contains!(e.to_string(), "Unknown selector");
        }

        // selector: method in YAML now produces an atom (resolved at runtime),
        // so unknown selectors are not caught at parse time
        let inheritance_result = parser.parse_atom(&AtomExpr::Method(MethodAtomExpr {
            method: "selector".to_string(),
            value: SelectorValue::from("unknown_selector").into(),
            childrens_parents: SelectorDefaultSpec::from(false),
            parents: SelectorDefaultSpec::from(false),
            children: SelectorDefaultSpec::from(false),
            parents_depth: None,
            children_depth: None,
            indirect_selection: None,
            exclude: None,
        }));
        assert!(inheritance_result.is_ok());
        if let Ok(SelectExpression::Atom(criteria)) = &inheritance_result {
            assert_eq!(criteria.method, MethodName::Selector);
            assert_eq!(criteria.value, "unknown_selector");
        }
    }
}
