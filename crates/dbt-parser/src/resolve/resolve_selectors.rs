use dbt_common::node_selector::{SelectExpression, parse_model_specifiers};
use dbt_common::once_cell_vars::DISPATCH_CONFIG;
use dbt_common::{ErrorCode, FsResult, err, fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::phases::parse::build_resolve_context;
use dbt_jinja_utils::serde::value_from_file;
use dbt_schemas::schemas::{
    manifest::DbtSelector,
    selectors::{
        AtomExpr, CompositeExpr, MethodAtomExpr, SelectorDefaultSpec, SelectorDefinitionValue,
        SelectorEntry, SelectorExpr, SelectorFile, SelectorMethodValue,
    },
};
use dbt_selector_parser::{ResolvedSelector, SelectorParser};
use dbt_yaml::Value as YmlValue;
use std::collections::{BTreeMap, HashMap};
use std::slice;

use crate::args::ResolveArgs;

/// Loads and resolves selector definitions from a selectors.yml file.
pub fn resolve_selectors_from_yaml(
    arg: &ResolveArgs,
    root_package_name: &str,
    jinja_env: &JinjaEnv,
) -> FsResult<HashMap<String, SelectorEntry>> {
    match load_and_parse_selectors_file(arg, root_package_name, jinja_env)? {
        Some(yaml) => resolve_selector_definitions(yaml, arg, jinja_env, root_package_name),
        None => Ok(HashMap::new()), // No selectors.yml file found
    }
}

/// Converts resolved selectors to manifest format.
/// Takes the already resolved selectors and converts them to DbtSelector format for the manifest.
pub fn resolve_manifest_selectors(
    resolved_selectors: HashMap<String, SelectorEntry>,
) -> FsResult<BTreeMap<String, DbtSelector>> {
    validate_default_selectors(&resolved_selectors)?;

    // Convert to manifest format
    let manifest_selectors = resolved_selectors
        .into_iter()
        .map(|(name, entry)| {
            let definition_value = selector_definition_to_yaml(&entry.definition)?;

            let selector = DbtSelector {
                name: name.clone(),
                description: entry.description.unwrap_or_default(),
                definition: Some(definition_value),
                __other__: BTreeMap::new(),
            };
            Ok((name, selector))
        })
        .collect::<FsResult<_>>()?;

    Ok(manifest_selectors)
}

/// Computes the final include/exclude expressions from resolved selectors.
/// This function takes already resolved selectors and computes the final selection
/// that should be used by the scheduler.
///
/// The function:
/// 1. Validates that only one selector is marked as default
/// 2. Computes the final include/exclude expressions based on:
///    - CLI selector flag or default selector
///    - Selector's include/exclude expressions
///    - CLI include/exclude flags
///    - CLI indirect selection mode (fallback if not specified in YAML)
///
/// Returns the final include and exclude expressions to be used by the scheduler.
pub fn resolve_final_selectors(
    resolved_selectors: HashMap<String, SelectorEntry>,
    arg: &ResolveArgs,
) -> FsResult<ResolvedSelector> {
    validate_default_selectors(&resolved_selectors)?;

    // Find default selector name if no explicit selector provided
    let default_sel_name = resolved_selectors.iter().find_map(|(name, entry)| {
        // Command line arguments (if provided) take precedence over the default
        if entry.is_default && !(arg.select.is_some() || arg.exclude.is_some()) {
            Some(name.clone())
        } else {
            None
        }
    });

    // Use explicit selector, default selector, or fall back to CLI flags
    if let Some(sel_name) = arg.selector.as_ref().or(default_sel_name.as_ref()) {
        // Look up selector and error if missing
        let entry = resolved_selectors.get(sel_name.as_str()).ok_or_else(|| {
            fs_err!(
                ErrorCode::SelectorError,
                "Unknown selector `{}` (see selectors.yml)",
                sel_name
            )
        })?;

        // Use selector's include and apply CLI indirect selection as fallback
        let mut include = entry.include.clone();
        if let Some(cli_mode) = arg.indirect_selection {
            include.set_indirect_selection(cli_mode);
        }

        // Set exclude to CLI exclude and apply CLI indirect selection as fallback
        let mut exclude = arg.exclude.clone();
        if let (Some(cli_mode), Some(exc)) = (arg.indirect_selection, exclude.as_mut()) {
            exc.set_indirect_selection(cli_mode);
        }

        Ok(ResolvedSelector {
            include: Some(include),
            exclude,
            selector_definitions: resolved_selectors,
        })
    } else {
        // No selector chosen → use CLI flags and apply CLI indirect selection
        let mut resolved = ResolvedSelector {
            include: arg.select.clone(),
            exclude: arg.exclude.clone(),
            selector_definitions: resolved_selectors,
        };

        let default_mode = arg.indirect_selection.unwrap_or_default();

        if let Some(ref mut include) = resolved.include {
            include.apply_default_indirect_selection(default_mode);
        }
        if let Some(ref mut exclude) = resolved.exclude {
            exclude.apply_default_indirect_selection(default_mode);
        }

        Ok(resolved)
    }
}

/// Loads and parses the selectors.yml file from the project root.
/// Returns the parsed selectors.yml file if it exists, otherwise returns None.
fn load_and_parse_selectors_file(
    arg: &ResolveArgs,
    root_package_name: &str,
    jinja_env: &JinjaEnv,
) -> FsResult<Option<SelectorFile>> {
    let path = arg.io.in_dir.join("selectors.yml");
    if !path.exists() {
        return Ok(None);
    }

    let raw_selectors = value_from_file(&path, true, None)?;

    // Treat an empty or null selectors.yml the same as an absent file — dbt Core does not
    // error on a zero-byte selectors.yml; it simply has no selectors defined.
    if raw_selectors.is_null() {
        return Ok(None);
    }

    let namespace_keys: Vec<String> = jinja_env
        .env
        .get_macro_namespace_registry()
        .map(|r| r.keys().map(|k| k.to_string()).collect())
        .unwrap_or_default();
    let context = build_resolve_context(
        root_package_name,
        root_package_name,
        &BTreeMap::new(),
        DISPATCH_CONFIG.get().unwrap().read().unwrap().clone(),
        namespace_keys,
        None,
    );

    let yaml: SelectorFile = match dbt_jinja_utils::serde::into_typed_with_jinja(
        raw_selectors,
        false,
        jinja_env,
        &context,
        &[],
        None,
        true,
    ) {
        Ok(yaml) => yaml,
        Err(e) => {
            return err!(
                ErrorCode::SelectorError,
                "Error parsing selectors.yml: {}",
                e
            );
        }
    };

    Ok(Some(yaml))
}

/// Parses and resolves selector definitions from a YAML file.
/// Returns a map of selector names to their resolved entries.
fn resolve_selector_definitions(
    yaml: SelectorFile,
    arg: &ResolveArgs,
    jinja_env: &JinjaEnv,
    root_package_name: &str,
) -> FsResult<HashMap<String, SelectorEntry>> {
    let defs = yaml
        .selectors
        .iter()
        .map(|d| (d.name.clone(), d.clone()))
        .collect::<BTreeMap<_, _>>();
    let parser = SelectorParser::new(defs);
    let mut resolved_selectors = HashMap::new();

    // The selector `default:` expression is only consulted when the user
    // did not supply a CLI selection. Skipping Jinja rendering in that
    // case mirrors dbt-core and keeps unused selectors from failing a
    // run with broken Jinja (see `SelectorDefinition::default` docs).
    let default_needed = arg.selector.is_none() && arg.select.is_none() && arg.exclude.is_none();

    for def in yaml.selectors {
        let resolved = parser.parse_definition(&def.definition)?;
        let is_default = match def.default.0 {
            Some(SelectorDefaultSpec::Bool(b)) => b,
            Some(SelectorDefaultSpec::Template(tmpl)) if default_needed => {
                render_default_template(&tmpl, jinja_env, root_package_name)?
            }
            _ => false,
        };
        resolved_selectors.insert(
            def.name.clone(),
            SelectorEntry {
                include: resolved,
                is_default,
                description: def.description,
                definition: def.definition,
            },
        );
    }

    Ok(resolved_selectors)
}

/// Render a selector `default:` Jinja template against the resolve
/// context and coerce the result to a bool using dbt's `as_bool`-style
/// truthiness rules.
fn render_default_template(
    template: &str,
    jinja_env: &JinjaEnv,
    root_package_name: &str,
) -> FsResult<bool> {
    let namespace_keys: Vec<String> = jinja_env
        .env
        .get_macro_namespace_registry()
        .map(|r| r.keys().map(|k| k.to_string()).collect())
        .unwrap_or_default();
    let context = build_resolve_context(
        root_package_name,
        root_package_name,
        &BTreeMap::new(),
        DISPATCH_CONFIG.get().unwrap().read().unwrap().clone(),
        namespace_keys,
        None,
    );
    let rendered = jinja_env.render_str(template, &context, &[]).map_err(|e| {
        fs_err!(
            ErrorCode::SelectorError,
            "Error parsing selectors.yml: failed to evaluate `default` expression: {}",
            e
        )
    })?;
    let trimmed = rendered.trim();
    Ok(match trimmed.to_ascii_lowercase().as_str() {
        "" | "false" | "0" | "none" => false,
        "true" | "1" => true,
        _ => !trimmed.is_empty(),
    })
}

fn selector_definition_to_yaml(def: &SelectorDefinitionValue) -> FsResult<YmlValue> {
    match def {
        SelectorDefinitionValue::String(s) => Ok(select_expression_to_yaml(
            &parse_model_specifiers(slice::from_ref(s))?,
        )),
        SelectorDefinitionValue::Full(expr) => selector_expr_to_yaml(expr),
        SelectorDefinitionValue::Array(items) => {
            let mut union_map = dbt_yaml::Mapping::new();
            union_map.insert(
                YmlValue::String("union".to_string(), Default::default()),
                YmlValue::Sequence(selector_definitions_to_yaml(items)?, Default::default()),
            );
            Ok(YmlValue::Mapping(union_map, Default::default()))
        }
    }
}

fn selector_expr_to_yaml(expr: &SelectorExpr) -> FsResult<YmlValue> {
    match expr {
        SelectorExpr::Composite(comp) => selector_composite_to_yaml(comp),
        SelectorExpr::Atom(atom) => selector_atom_to_yaml(atom),
    }
}

fn selector_composite_to_yaml(comp: &CompositeExpr) -> FsResult<YmlValue> {
    let mut map = dbt_yaml::Mapping::new();
    if let Some(items) = &comp.union {
        map.insert(
            YmlValue::String("union".to_string(), Default::default()),
            YmlValue::Sequence(selector_definitions_to_yaml(items)?, Default::default()),
        );
    }
    if let Some(items) = &comp.intersection {
        map.insert(
            YmlValue::String("intersection".to_string(), Default::default()),
            YmlValue::Sequence(selector_definitions_to_yaml(items)?, Default::default()),
        );
    }
    if let Some(items) = &comp.exclude {
        map.insert(
            YmlValue::String("exclude".to_string(), Default::default()),
            YmlValue::Sequence(selector_definitions_to_yaml(items)?, Default::default()),
        );
    }
    Ok(YmlValue::Mapping(map, Default::default()))
}

fn selector_definitions_to_yaml(defs: &[SelectorDefinitionValue]) -> FsResult<Vec<YmlValue>> {
    defs.iter().map(selector_definition_to_yaml).collect()
}

fn selector_atom_to_yaml(atom: &AtomExpr) -> FsResult<YmlValue> {
    match atom {
        AtomExpr::Method(expr) => {
            let mut value = method_atom_to_yaml(expr);
            if let Some(exclude) = &expr.exclude
                && let YmlValue::Mapping(map, _) = &mut value
            {
                map.insert(
                    YmlValue::String("exclude".to_string(), Default::default()),
                    YmlValue::Sequence(selector_definitions_to_yaml(exclude)?, Default::default()),
                );
            }
            Ok(value)
        }
        AtomExpr::MethodKey(method_value) => {
            let (method, value) = method_value
                .iter()
                .next()
                .filter(|_| method_value.len() == 1)
                .ok_or_else(|| {
                    fs_err!(
                        ErrorCode::SelectorError,
                        "MethodKey must have exactly one key-value pair"
                    )
                })?;
            Ok(method_value_to_yaml(method, selector_value_to_yaml(value)))
        }
        AtomExpr::Exclude(expr) => {
            let mut map = dbt_yaml::Mapping::new();
            map.insert(
                YmlValue::String("exclude".to_string(), Default::default()),
                YmlValue::Sequence(
                    selector_definitions_to_yaml(&expr.exclude)?,
                    Default::default(),
                ),
            );
            Ok(YmlValue::Mapping(map, Default::default()))
        }
    }
}

fn method_atom_to_yaml(expr: &MethodAtomExpr) -> YmlValue {
    let mut value = method_value_to_yaml(&expr.method, selector_value_to_yaml(&expr.value));
    if let YmlValue::Mapping(map, _) = &mut value {
        if expr.parents.as_bool() || expr.parents_depth.is_some() {
            map.insert(
                YmlValue::String("parents".to_string(), Default::default()),
                YmlValue::Bool(true, Default::default()),
            );
        }
        if let Some(depth) = expr.parents_depth {
            map.insert(
                YmlValue::String("parents_depth".to_string(), Default::default()),
                YmlValue::String(depth.to_string(), Default::default()),
            );
        }
        if expr.children.as_bool() || expr.children_depth.is_some() {
            map.insert(
                YmlValue::String("children".to_string(), Default::default()),
                YmlValue::Bool(true, Default::default()),
            );
        }
        if let Some(depth) = expr.children_depth {
            map.insert(
                YmlValue::String("children_depth".to_string(), Default::default()),
                YmlValue::String(depth.to_string(), Default::default()),
            );
        }
        if expr.childrens_parents.as_bool() {
            map.insert(
                YmlValue::String("childrens_parents".to_string(), Default::default()),
                YmlValue::Bool(true, Default::default()),
            );
        }
        if let Some(indirect_selection) = expr.indirect_selection {
            map.insert(
                YmlValue::String("indirect_selection".to_string(), Default::default()),
                YmlValue::String(indirect_selection.to_string(), Default::default()),
            );
        }
    }
    value
}

fn method_value_to_yaml(method: &str, value: YmlValue) -> YmlValue {
    let mut map = dbt_yaml::Mapping::new();
    map.insert(
        YmlValue::String("method".to_string(), Default::default()),
        YmlValue::String(method.to_string(), Default::default()),
    );
    map.insert(
        YmlValue::String("value".to_string(), Default::default()),
        value,
    );
    YmlValue::Mapping(map, Default::default())
}

/// Renders a method argument back to YAML, preserving the shape it was parsed with.
fn selector_value_to_yaml(value: &SelectorMethodValue) -> YmlValue {
    match value {
        SelectorMethodValue::Scalar(v) => {
            YmlValue::String(v.as_str().to_string(), Default::default())
        }
        SelectorMethodValue::Unsupported(y) => (**y).clone(),
    }
}

/// Converts a SelectExpression to the normalized YAML format expected by the manifest.
fn select_expression_to_yaml(expr: &SelectExpression) -> YmlValue {
    match expr {
        SelectExpression::Atom(criteria) => {
            let mut map = dbt_yaml::Mapping::new();
            let method = if criteria.method_args.is_empty() {
                criteria.method.to_string()
            } else {
                format!("{}.{}", criteria.method, criteria.method_args.join("."))
            };
            map.insert(
                YmlValue::String("method".to_string(), Default::default()),
                YmlValue::String(method, Default::default()),
            );
            let value = match &criteria.value {
                dbt_common::node_selector::SelectionValue::Scalar(s) => {
                    YmlValue::String(s.clone(), Default::default())
                }
                dbt_common::node_selector::SelectionValue::Unsupported(y) => (**y).clone(),
            };
            map.insert(
                YmlValue::String("value".to_string(), Default::default()),
                value,
            );

            if criteria.parents_depth.is_some() {
                map.insert(
                    YmlValue::String("parents".to_string(), Default::default()),
                    YmlValue::Bool(true, Default::default()),
                );
                // include the depth value if it's not unlimited
                if let Some(depth) = criteria.parents_depth
                    && depth != u32::MAX
                {
                    map.insert(
                        YmlValue::String("parents_depth".to_string(), Default::default()),
                        YmlValue::String(depth.to_string(), Default::default()),
                    );
                }
            }
            if criteria.children_depth.is_some() {
                map.insert(
                    YmlValue::String("children".to_string(), Default::default()),
                    YmlValue::Bool(true, Default::default()),
                );
                // include the depth value if it's not unlimited
                if let Some(depth) = criteria.children_depth
                    && depth != u32::MAX
                {
                    map.insert(
                        YmlValue::String("children_depth".to_string(), Default::default()),
                        YmlValue::String(depth.to_string(), Default::default()),
                    );
                }
            }
            if criteria.childrens_parents {
                map.insert(
                    YmlValue::String("childrens_parents".to_string(), Default::default()),
                    YmlValue::Bool(true, Default::default()),
                );
            }

            // Serialize the nested exclude (if any). dbt-core represents `exclude` as a
            // list of selector definitions. Our runtime stores them as a single
            // SelectExpression (multiple excludes are combined into an Or), so wrap the
            // serialized inner expression in a single-element sequence to match the
            // dbt-core manifest shape. Without this the exclude is dropped from the manifest.
            if let Some(exclude) = &criteria.exclude {
                map.insert(
                    YmlValue::String("exclude".to_string(), Default::default()),
                    YmlValue::Sequence(
                        vec![select_expression_to_yaml(exclude)],
                        Default::default(),
                    ),
                );
            }

            YmlValue::Mapping(map, Default::default())
        }
        SelectExpression::Or(expressions) => {
            let values: Vec<YmlValue> = expressions.iter().map(select_expression_to_yaml).collect();

            let mut union_map = dbt_yaml::Mapping::new();
            union_map.insert(
                YmlValue::String("union".to_string(), Default::default()),
                YmlValue::Sequence(values, Default::default()),
            );
            YmlValue::Mapping(union_map, Default::default())
        }
        SelectExpression::And(expressions) => {
            let values: Vec<YmlValue> = expressions.iter().map(select_expression_to_yaml).collect();

            let mut intersection_map = dbt_yaml::Mapping::new();
            intersection_map.insert(
                YmlValue::String("intersection".to_string(), Default::default()),
                YmlValue::Sequence(values, Default::default()),
            );
            YmlValue::Mapping(intersection_map, Default::default())
        }
        SelectExpression::Exclude(expr) => {
            let mut exclude_map = dbt_yaml::Mapping::new();
            exclude_map.insert(
                YmlValue::String("exclude".to_string(), Default::default()),
                select_expression_to_yaml(expr),
            );
            YmlValue::Mapping(exclude_map, Default::default())
        }
    }
}

fn validate_default_selectors(resolved_selectors: &HashMap<String, SelectorEntry>) -> FsResult<()> {
    if resolved_selectors.values().filter(|e| e.is_default).count() > 1 {
        return err!(
            ErrorCode::SelectorError,
            "Multiple selectors have `default: true`"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_common::node_selector::{MethodName, SelectionCriteria};
    use dbt_schemas::schemas::selectors::{
        AtomExpr, CompositeExpr, ExcludeAtomExpr, MethodAtomExpr, SelectorDefaultSpec,
        SelectorDefinitionValue, SelectorExpr, SelectorValue,
    };

    fn atom(method: MethodName, value: &str) -> SelectExpression {
        SelectExpression::Atom(SelectionCriteria::new(
            method,
            vec![],
            value.to_string(),
            false,
            None,
            None,
            Some(dbt_common::node_selector::IndirectSelection::default()),
            None,
        ))
    }

    /// Regression test for FUSION-319963455669 Bug 2: a method atom with a
    /// nested exclude must serialize the `exclude` block (as a list) into the
    /// manifest. Previously the exclude was silently dropped.
    #[test]
    fn test_serialize_atom_with_nested_exclude() {
        let expr = SelectExpression::Atom(SelectionCriteria::new(
            MethodName::Fqn,
            vec![],
            "*".to_string(),
            false,
            None,
            None,
            Some(dbt_common::node_selector::IndirectSelection::default()),
            Some(Box::new(SelectExpression::Or(vec![
                atom(MethodName::Tag, "usage"),
                atom(MethodName::Tag, "feed_service_now"),
            ]))),
        ));

        let yaml = select_expression_to_yaml(&expr);

        assert_eq!(yaml.get("method").and_then(|v| v.as_str()), Some("fqn"));
        assert_eq!(yaml.get("value").and_then(|v| v.as_str()), Some("*"));

        // `exclude` must be a single-element list wrapping the union.
        let exclude = yaml
            .get("exclude")
            .and_then(|v| v.as_sequence())
            .expect("expected `exclude` sequence in serialized atom");
        assert_eq!(exclude.len(), 1);

        let union = exclude[0]
            .get("union")
            .and_then(|v| v.as_sequence())
            .expect("expected `union` sequence inside exclude");
        let mut tags: Vec<String> = union
            .iter()
            .map(|item| {
                assert_eq!(item.get("method").and_then(|v| v.as_str()), Some("tag"));
                item.get("value")
                    .and_then(|v| v.as_str())
                    .unwrap()
                    .to_string()
            })
            .collect();
        tags.sort();
        assert_eq!(tags, vec!["feed_service_now", "usage"]);
    }

    /// An atom without a nested exclude must not emit an `exclude` key (no
    /// spurious empty `exclude: []`).
    #[test]
    fn test_serialize_atom_without_exclude() {
        let expr = atom(MethodName::Fqn, "*");
        let yaml = select_expression_to_yaml(&expr);
        assert_eq!(yaml.get("method").and_then(|v| v.as_str()), Some("fqn"));
        assert_eq!(yaml.get("value").and_then(|v| v.as_str()), Some("*"));
        assert!(
            yaml.get("exclude").is_none(),
            "did not expect an `exclude` key when criteria.exclude is None"
        );
    }

    #[test]
    fn test_serialize_dot_notation_method() {
        let expr = parse_model_specifiers(&["config.materialized:table".to_string()]).unwrap();
        let yaml = select_expression_to_yaml(&expr);

        assert_eq!(
            yaml.get("method").and_then(|v| v.as_str()),
            Some("config.materialized")
        );
    }

    #[test]
    fn test_serialize_selector_definition_array_as_union() {
        let definition = SelectorDefinitionValue::Array(vec![
            SelectorDefinitionValue::String("a".to_string()),
            SelectorDefinitionValue::String("b".to_string()),
        ]);
        let yaml = selector_definition_to_yaml(&definition).unwrap();

        let sequence = yaml
            .get("union")
            .and_then(|value| value.as_sequence())
            .expect("array definition should become a union");
        assert_eq!(sequence.len(), 2);
        assert_eq!(sequence[0].get("value").and_then(|v| v.as_str()), Some("a"));
        assert_eq!(sequence[1].get("value").and_then(|v| v.as_str()), Some("b"));
    }

    #[test]
    fn test_serialize_method_key_requires_one_entry() {
        let method_value = BTreeMap::from([
            ("selector".to_string(), SelectorValue::from("base").into()),
            ("fqn".to_string(), SelectorValue::from("other").into()),
        ]);
        let atom = AtomExpr::MethodKey(method_value);

        assert!(selector_atom_to_yaml(&atom).is_err());
    }

    #[test]
    fn test_manifest_selector_preserves_union_with_inline_exclude_shape() -> FsResult<()> {
        let selector_file: SelectorFile = dbt_yaml::from_str(
            r#"
selectors:
  - name: all_except_c
    description: Union of a and b, excluding c
    definition:
      union:
        - a
        - b
        - exclude:
            - c
"#,
        )
        .expect("issue #10393 selector YAML should parse");
        let original_definition = selector_file.selectors[0].definition.clone();
        let normalized_include =
            SelectorParser::new(BTreeMap::new()).parse_definition(&original_definition)?;
        let resolved_selectors = HashMap::from([(
            "all_except_c".to_string(),
            SelectorEntry {
                include: normalized_include,
                is_default: false,
                description: Some("Union of a and b, excluding c".to_string()),
                definition: original_definition,
            },
        )]);

        let manifest_selectors = resolve_manifest_selectors(resolved_selectors)?;
        let definition = manifest_selectors
            .get("all_except_c")
            .and_then(|selector| selector.definition.as_ref())
            .expect("selector definition should be present");

        assert!(
            definition.get("intersection").is_none(),
            "manifest must not serialize normalized runtime And as intersection"
        );
        let union = definition
            .get("union")
            .and_then(|value| value.as_sequence())
            .expect("manifest should preserve original union");
        assert_eq!(union.len(), 3);
        assert_eq!(union[0].get("method").and_then(|v| v.as_str()), Some("fqn"));
        assert_eq!(union[0].get("value").and_then(|v| v.as_str()), Some("a"));
        assert_eq!(union[1].get("method").and_then(|v| v.as_str()), Some("fqn"));
        assert_eq!(union[1].get("value").and_then(|v| v.as_str()), Some("b"));
        let exclude = union[2]
            .get("exclude")
            .and_then(|value| value.as_sequence())
            .expect("inline exclude should remain a list inside union");
        assert_eq!(exclude.len(), 1);
        assert_eq!(
            exclude[0].get("method").and_then(|v| v.as_str()),
            Some("fqn")
        );
        assert_eq!(exclude[0].get("value").and_then(|v| v.as_str()), Some("c"));
        Ok(())
    }

    #[test]
    fn test_manifest_selector_preserves_dot_notation_method() -> FsResult<()> {
        let original_definition =
            SelectorDefinitionValue::Full(SelectorExpr::Atom(AtomExpr::Method(MethodAtomExpr {
                method: "config.materialized".to_string(),
                value: SelectorValue::from("table").into(),
                childrens_parents: SelectorDefaultSpec::from(false),
                parents: SelectorDefaultSpec::from(false),
                children: SelectorDefaultSpec::from(false),
                parents_depth: None,
                children_depth: None,
                indirect_selection: Some(dbt_common::node_selector::IndirectSelection::default()),
                exclude: None,
            })));
        let resolved_selectors = HashMap::from([(
            "tables".to_string(),
            SelectorEntry {
                include: atom(MethodName::Config, "table"),
                is_default: false,
                description: None,
                definition: original_definition,
            },
        )]);

        let manifest_selectors = resolve_manifest_selectors(resolved_selectors)?;
        let definition = manifest_selectors
            .get("tables")
            .and_then(|selector| selector.definition.as_ref())
            .expect("selector definition should be present");
        assert_eq!(
            definition.get("method").and_then(|v| v.as_str()),
            Some("config.materialized")
        );
        assert_eq!(
            definition.get("value").and_then(|v| v.as_str()),
            Some("table")
        );
        Ok(())
    }

    #[test]
    fn test_manifest_selector_preserves_mixed_union_with_selector_reference() -> FsResult<()> {
        let mut selector_ref = BTreeMap::new();
        selector_ref.insert("selector".to_string(), SelectorValue::from("base").into());
        let original_definition =
            SelectorDefinitionValue::Full(SelectorExpr::Composite(CompositeExpr {
                union: Some(vec![
                    SelectorDefinitionValue::Full(SelectorExpr::Atom(AtomExpr::MethodKey(
                        selector_ref,
                    ))),
                    SelectorDefinitionValue::String("d".to_string()),
                    SelectorDefinitionValue::Full(SelectorExpr::Atom(AtomExpr::Exclude(
                        ExcludeAtomExpr {
                            exclude: vec![SelectorDefinitionValue::String("e".to_string())],
                        },
                    ))),
                ]),
                intersection: None,
                exclude: None,
            }));
        let normalized_include = SelectExpression::And(vec![
            SelectExpression::Or(vec![
                atom(MethodName::Tag, "base"),
                atom(MethodName::Fqn, "d"),
            ]),
            SelectExpression::Exclude(Box::new(atom(MethodName::Fqn, "e"))),
        ]);
        let resolved_selectors = HashMap::from([(
            "mixed".to_string(),
            SelectorEntry {
                include: normalized_include,
                is_default: false,
                description: None,
                definition: original_definition,
            },
        )]);

        let manifest_selectors = resolve_manifest_selectors(resolved_selectors)?;
        let definition = manifest_selectors
            .get("mixed")
            .and_then(|selector| selector.definition.as_ref())
            .expect("selector definition should be present");
        assert!(
            definition.get("intersection").is_none(),
            "mixed selector references must not force whole-expression AST serialization"
        );
        let union = definition
            .get("union")
            .and_then(|value| value.as_sequence())
            .expect("manifest should preserve original union");
        assert_eq!(union.len(), 3);
        assert_eq!(
            union[0].get("method").and_then(|v| v.as_str()),
            Some("selector")
        );
        assert_eq!(union[0].get("value").and_then(|v| v.as_str()), Some("base"));
        assert_eq!(union[1].get("method").and_then(|v| v.as_str()), Some("fqn"));
        assert_eq!(union[1].get("value").and_then(|v| v.as_str()), Some("d"));
        let exclude = union[2]
            .get("exclude")
            .and_then(|value| value.as_sequence())
            .expect("inline exclude should remain inside union");
        assert_eq!(exclude[0].get("value").and_then(|v| v.as_str()), Some("e"));
        Ok(())
    }

    #[test]
    fn test_manifest_selector_preserves_pure_selector_alias() -> FsResult<()> {
        let mut selector_ref = BTreeMap::new();
        selector_ref.insert("selector".to_string(), SelectorValue::from("base").into());
        let original_definition =
            SelectorDefinitionValue::Full(SelectorExpr::Atom(AtomExpr::MethodKey(selector_ref)));
        let normalized_include = SelectExpression::And(vec![
            SelectExpression::Or(vec![atom(MethodName::Fqn, "a"), atom(MethodName::Fqn, "b")]),
            SelectExpression::Exclude(Box::new(atom(MethodName::Fqn, "c"))),
        ]);
        let resolved_selectors = HashMap::from([(
            "alias".to_string(),
            SelectorEntry {
                include: normalized_include,
                is_default: false,
                description: None,
                definition: original_definition,
            },
        )]);

        let manifest_selectors = resolve_manifest_selectors(resolved_selectors)?;
        let definition = manifest_selectors
            .get("alias")
            .and_then(|selector| selector.definition.as_ref())
            .expect("selector definition should be present");
        assert!(
            definition.get("intersection").is_none(),
            "pure selector aliases must not serialize the referenced selector's runtime AST"
        );
        assert_eq!(
            definition.get("method").and_then(|v| v.as_str()),
            Some("selector")
        );
        assert_eq!(
            definition.get("value").and_then(|v| v.as_str()),
            Some("base")
        );
        Ok(())
    }

    /// Regression (dbt-labs/fs#13604): dbt Core types a method argument's
    /// `value` as `Any`, so a sequence like `value: ["models"]` loads fine
    /// even for an unused selector. A one-element sequence is no exception:
    /// `Path.glob()` still gets a `list`, so it must defer, not resolve.
    #[test]
    fn test_sequence_valued_selector_argument_parses() -> FsResult<()> {
        let selector_file: SelectorFile = dbt_yaml::from_str(
            r#"
selectors:
  - name: default
    definition:
      method: path
      value: ["models"]
"#,
        )
        .expect("issue #13604 selector YAML should parse");

        let original_definition = selector_file.selectors[0].definition.clone();
        let normalized_include =
            SelectorParser::new(BTreeMap::new()).parse_definition(&original_definition)?;
        if let SelectExpression::Atom(criteria) = &normalized_include {
            assert_eq!(criteria.method, MethodName::Path);
            assert!(
                matches!(
                    criteria.value,
                    dbt_common::node_selector::SelectionValue::Unsupported(_)
                ),
                "a sequence value has no selection semantics and must be deferred, not treated as a scalar"
            );
        } else {
            panic!("Expected a single deferred Atom for a sequence value");
        }

        // Manifest round-trip must preserve the sequence shape, matching how
        // dbt Core stores the raw, unvalidated `value` in the manifest.
        let resolved_selectors = HashMap::from([(
            "default".to_string(),
            SelectorEntry {
                include: normalized_include,
                is_default: false,
                description: None,
                definition: original_definition,
            },
        )]);
        let manifest_selectors = resolve_manifest_selectors(resolved_selectors)?;
        let definition = manifest_selectors
            .get("default")
            .and_then(|selector| selector.definition.as_ref())
            .expect("selector definition should be present");
        let value = definition
            .get("value")
            .and_then(|v| v.as_sequence())
            .expect("manifest should preserve the sequence-valued argument");
        assert_eq!(value.len(), 1);
        assert_eq!(value[0].as_str(), Some("models"));
        Ok(())
    }
}
