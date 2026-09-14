//! Select and exclude nodes from the manifest based on the select and exclude expressions

use clean_path::Clean;
use dbt_adapter_core::AdapterType;
use dbt_common::{
    ErrorCode, FsResult,
    constants::DBT_GENERIC_TESTS_DIR_NAME,
    err,
    node_selector::{MethodName, SelectExpression, SelectionCriteria, SelectionValue},
    tracing::dbt_emit::emit_warn_log_message,
};
use dbt_frontend_common::Dialect;
use dbt_schemas::schemas::{
    CommonAttributes, DbtSource, DbtTest, InternalDbtNode, InternalDbtNodeAttributes,
    ModificationType, Nodes, StateArtifacts, common::Access, telemetry::NodeType,
};
use glob::Pattern;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::Path,
};

type YmlValue = dbt_yaml::Value;

/// Node selections returned by an external state-selector implementation, keyed by the exact
/// `state:` criterion value.
pub type StateSelectorResults = BTreeMap<String, BTreeSet<String>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateModifiedSubType {
    Body,
    Configs,
    Relation,
    PersistedDescriptions,
    Macros,
    Contract,
}

impl StateModifiedSubType {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "body" => Some(StateModifiedSubType::Body),
            "configs" => Some(StateModifiedSubType::Configs),
            "relation" => Some(StateModifiedSubType::Relation),
            "persisted_descriptions" => Some(StateModifiedSubType::PersistedDescriptions),
            "macros" => Some(StateModifiedSubType::Macros),
            "contract" => Some(StateModifiedSubType::Contract),
            _ => None,
        }
    }

    fn to_modification_type(&self) -> ModificationType {
        match self {
            StateModifiedSubType::Body => ModificationType::Body,
            StateModifiedSubType::Configs => ModificationType::Configs,
            StateModifiedSubType::Relation => ModificationType::Relation,
            StateModifiedSubType::PersistedDescriptions => ModificationType::PersistedDescriptions,
            StateModifiedSubType::Macros => ModificationType::Macros,
            StateModifiedSubType::Contract => ModificationType::Contract,
        }
    }
}

// ------------------------------------------------------------------------------------------------
// filter nodes based on select

/// Filter the nodes in the manifest based on the select expression
pub fn filter_select(
    nodes: &Nodes,
    expr: &SelectExpression,
    previous_state: Option<&StateArtifacts>,
    adapter_type: AdapterType,
) -> FsResult<BTreeSet<String>> {
    let project_name = nodes.project_name.as_deref();
    let mut result = BTreeSet::new();
    for (_, node) in nodes.iter() {
        if select_expression_include_node(
            expr,
            node,
            previous_state,
            project_name,
            Some(nodes),
            adapter_type,
        )? {
            result.insert(node.unique_id());
        }
    }
    Ok(result)
}

/// Filter the manifest for a **single** criterion.
///
/// This is a plain scan of every node that runs with or without the `previous_state`
/// comparison manifest (`None` is understood as "nothing is modified").
/// This is useful in `dbt-defer`'s `modified_nodes`, which legitimately runs without a
/// comparison manifest (deferral does not require `--state`).
///
/// On the user-authored-selector path, additional checks are necessary,
/// see [`filter_select_criteria_with_state_selector_results`].
pub fn filter_select_criteria(
    nodes: &Nodes,
    criteria: &SelectionCriteria,
    previous_state: Option<&StateArtifacts>,
    adapter_type: AdapterType,
) -> FsResult<BTreeSet<String>> {
    let project_name = nodes.project_name.as_deref();
    nodes
        .iter()
        .try_fold(BTreeSet::new(), |mut acc, (_, node)| {
            predicate_include_identifier_node(
                criteria,
                node,
                previous_state,
                project_name,
                Some(nodes),
                adapter_type,
            )
            .map(|include| {
                if include {
                    acc.insert(node.unique_id());
                }
                acc
            })
        })
}

/// Filter the manifest for a single criterion on the user-authored-selector path.
///
/// Whenever a `state:...` criterion is used, this checks for presence of a comparison manifest,
/// unless there is an override with a precomputed selection result.
pub fn filter_select_criteria_with_state_selector_results(
    nodes: &Nodes,
    criteria: &SelectionCriteria,
    previous_state: Option<&StateArtifacts>,
    state_selector_results: Option<&StateSelectorResults>,
    adapter_type: AdapterType,
) -> FsResult<BTreeSet<String>> {
    if criteria.method == MethodName::State {
        // Reuse the selection an external state-selector implementation already made, if it
        // evaluated this exact `state:` value. Deliberately ahead of the manifest check below.
        if let Some(selected) =
            state_selector_results.and_then(|results| results.get(criteria.value.as_str()?))
        {
            return Ok(selected.clone());
        }

        // Otherwise every `state:*` criterion needs something to compare against. This is the
        // single point every user-authored state criterion reaches — however it was written
        // (`--select state:modified`, a top-level `--selector`, or a `{selector: name}` reference
        // nested arbitrarily deep inside a composed selector).
        if previous_state.is_none() {
            return err!(
                ErrorCode::SelectorError,
                "Got a state selector method, but no comparison manifest"
            );
        }
    }

    filter_select_criteria(nodes, criteria, previous_state, adapter_type)
}

// ------------------------------------------------------------------------------------------------
// evaluate select expression

/// Filter the nodes in the manifest based on the select expression
fn select_expression_include_node(
    expr: &SelectExpression,
    node: &dyn InternalDbtNodeAttributes,
    previous_state: Option<&StateArtifacts>,
    project_name: Option<&str>,
    current_nodes: Option<&Nodes>,
    adapter_type: AdapterType,
) -> FsResult<bool> {
    match expr {
        SelectExpression::And(expressions) => {
            let results: FsResult<Vec<bool>> = expressions
                .iter()
                .map(|expr| {
                    select_expression_include_node(
                        expr,
                        node,
                        previous_state,
                        project_name,
                        current_nodes,
                        adapter_type,
                    )
                })
                .collect();
            Ok(results?.iter().all(|&x| x))
        }
        SelectExpression::Or(expressions) => {
            let results: FsResult<Vec<bool>> = expressions
                .iter()
                .map(|expr| {
                    select_expression_include_node(
                        expr,
                        node,
                        previous_state,
                        project_name,
                        current_nodes,
                        adapter_type,
                    )
                })
                .collect();
            Ok(results?.iter().any(|&x| x))
        }
        SelectExpression::Atom(predicate) => predicate_include_identifier_node(
            predicate,
            node,
            previous_state,
            project_name,
            current_nodes,
            adapter_type,
        ),
        SelectExpression::Exclude(child) => {
            let should_exclude = select_expression_include_node(
                child,
                node,
                previous_state,
                project_name,
                current_nodes,
                adapter_type,
            )?;
            Ok(!should_exclude)
        }
    }
}

// Helper functions for predicate_include_identifier_node
fn match_source(pattern: &str, node: &dyn InternalDbtNode) -> FsResult<bool> {
    if node.resource_type() != NodeType::Source {
        return Ok(false);
    }
    let parts: Vec<&str> = pattern.split('.').collect();
    let target_package = if parts.len() == 3 { parts[0] } else { "*" };
    let target_source = if parts.len() >= 2 {
        parts[parts.len() - 2]
    } else {
        parts[0]
    };
    let target_table = if parts.len() >= 2 {
        parts[parts.len() - 1]
    } else {
        "*"
    };

    if parts.is_empty() || parts.len() > 3 {
        return err!(
            ErrorCode::SelectorError,
            "Invalid source selector value. Sources must be of the form ${{source_name}}, ${{source_name}}.${{target_name}}, or ${{package_name}}.${{source_name}}.${{target_name}}"
        );
    }

    if let Some(source) = node.as_any().downcast_ref::<DbtSource>() {
        Ok(
            fnmatch(target_package, &source.__common_attr__.package_name)
                && fnmatch(target_source, &source.__source_attr__.source_name)
                && fnmatch(target_table, &source.__common_attr__.name),
        )
    } else {
        Ok(false)
    }
}

fn match_resource_type(pattern: &str, node: &dyn InternalDbtNode) -> FsResult<bool> {
    // Checks produce no database relation, so they are never a `relation`. (This arm remains an
    // approximation for other non-relational types like exposures and metrics; not widened here.)
    if pattern == "relation" && !matches!(node.resource_type(), NodeType::Test | NodeType::Check) {
        Ok(true)
    } else {
        Ok(pattern == node.resource_type().as_static_ref())
    }
}

fn match_test_type(pattern: &str, node: &dyn InternalDbtNode) -> FsResult<bool> {
    match pattern {
        "unit" => Ok(node.resource_type() == NodeType::UnitTest),
        "data" => Ok(node.resource_type() == NodeType::Test),
        "singular" | "generic" => {
            if node.resource_type() != NodeType::Test {
                Ok(false)
            } else {
                // Check if the test is in target/generic_tests directory
                let path = node.common().original_file_path.to_string_lossy();
                // TODO (alex): A path component should be exactly DBT_GENERIC_TESTS_DIR_NAME
                let is_generic = path.contains(DBT_GENERIC_TESTS_DIR_NAME);
                Ok((pattern == "generic" && is_generic) || (pattern == "singular" && !is_generic))
            }
        }
        _ => Ok(false),
    }
}

fn match_version(pattern: &str, node: &dyn InternalDbtNode) -> FsResult<bool> {
    if node.resource_type() != NodeType::Model {
        return Ok(false);
    }

    let file_name = node
        .common()
        .original_file_path
        .file_name()
        .expect("Model should have a file name");
    let file_stem = Path::new(file_name)
        .file_stem()
        .expect("file name should have a stem");
    let file_stem = file_stem.to_string_lossy();
    let file_version = if file_stem.ends_with("_v1") {
        Some(1)
    } else if file_stem.ends_with("_v2") {
        Some(2)
    } else {
        None
    };

    match pattern {
        "latest" => match (&node.version(), &node.latest_version()) {
            (None, None) => Ok(false),
            (None, Some(_)) => Ok(false),
            (Some(_), None) => Ok(false),
            (Some(version_v), Some(latest_v)) => Ok(version_v == latest_v),
        },
        "prerelease" => match (&node.version(), &node.latest_version()) {
            (None, None) => Ok(false),
            (None, Some(_)) => Ok(false),
            (Some(_), None) => Ok(false),
            (Some(version_v), Some(latest_v)) => {
                let version_v = version_v.to_i64();
                let latest_v = latest_v.to_i64();
                Ok(version_v > latest_v)
            }
        },
        "old" => match (&node.version(), &node.latest_version()) {
            (None, None) => Ok(false),
            (None, Some(_)) => Ok(false),
            (Some(_), None) => Ok(false),
            (Some(version_v), Some(latest_v)) => {
                let version_v = version_v.to_i64();
                let latest_v = latest_v.to_i64();
                Ok(version_v < latest_v)
            }
        },
        "none" => Ok(file_version.is_none()),
        _ => {
            err!(
                ErrorCode::SelectorError,
                "version must be one of old, none, latest, prerelease"
            )
        }
    }
}

/// Match the state selector
///
/// # Arguments
///
/// * `pattern`: The state selector pattern
/// * `node`: The node to match
/// * `common_attr`: The common attributes of the node
/// * `previous_state`: The previous state
///
/// # Returns
///
/// A boolean indicating if the node matches the state selector
fn match_state(
    pattern: &str,
    node: &dyn InternalDbtNode,
    common_attr: &CommonAttributes,
    previous_state: Option<&StateArtifacts>,
    current_nodes: Option<&Nodes>,
    adapter_type: AdapterType,
) -> FsResult<bool> {
    let parts: Vec<&str> = pattern.split('.').collect();
    if parts.is_empty() {
        return Ok(false);
    }

    // For state selectors, we need a node with a unique_id
    if common_attr.unique_id.is_empty() {
        return Ok(false);
    }

    if let Some(previous) = previous_state {
        match parts[0] {
            "modified" => {
                // If we have more parts, it's a sub-selector
                if parts.len() > 1 {
                    match StateModifiedSubType::from_str(parts[1]) {
                        Some(sub_type) => Ok(previous.is_modified(
                            node,
                            Some(sub_type.to_modification_type()),
                            current_nodes,
                            adapter_type,
                        )),
                        None => Ok(false),
                    }
                } else {
                    // state:modified includes all modifications
                    Ok(previous.is_modified(node, None, current_nodes, adapter_type))
                }
            }
            "new" => Ok(previous.is_new(node)),
            "old" => Ok(previous.exists(node)),
            "unmodified" => Ok(previous.exists(node)
                && !previous.is_modified(node, None, current_nodes, adapter_type)),
            _ => Ok(false),
        }
    } else {
        // If there's no previous state, all nodes are "new"
        Ok(parts[0] == "new")
    }
}

fn match_fqn(
    pattern: &str,
    node: &dyn InternalDbtNode,
    common_attr: &CommonAttributes,
) -> FsResult<bool> {
    // Sources should not be matched by implicit FQN selector.
    // They must be explicitly selected with source: method.
    // This matches dbt-core's QualifiedNameSelectorMethod which uses non_source_nodes().
    if node.resource_type() == NodeType::Source {
        return Ok(false);
    }

    // Note (Ani)I can't seem to figure out how dbt core replicates this but we need this to get our tests to pass
    // ──────────────────────────────────────────────────────────────
    // 0. quick leaf / unique-id glob (dbt-core shortcut)
    // ──────────────────────────────────────────────────────────────
    if fnmatch(pattern, &common_attr.name) || fnmatch(pattern, &common_attr.unique_id) {
        return Ok(true);
    }

    // Check original_name for truncated tests - allows matching by untruncated name
    // When test names exceed 63 characters, dbt truncates to `<first 30 chars>_<md5 hash>`.
    // This allows users to exclude/select tests by their original (untruncated) name.
    if let Some(original) = node.original_name() {
        if fnmatch(pattern, original) {
            return Ok(true);
        }
    }

    // 1. full dbt-core logic (package-aware, versioned, wildcards, …)
    Ok(node_is_match(
        pattern,
        &common_attr.fqn,
        node.is_versioned(),
    ))
}

fn match_column(
    criteria: &SelectionCriteria,
    pattern: &str,
    node: &dyn InternalDbtNodeAttributes,
    previous_state: Option<&StateArtifacts>,
    project_name: Option<&str>,
    current_nodes: Option<&Nodes>,
    adapter_type: AdapterType,
) -> FsResult<bool> {
    // split selector into <node_pattern>  +  <column_pattern>
    let mut parts = pattern.rsplitn(2, '.');
    let _col_pat = parts.next().unwrap(); // e.g.  "*"
    let node_pat = parts.next().unwrap_or(""); // e.g.  "model.jaffle_shop.orders"
    let mut new_critera = criteria.clone();
    new_critera.method = MethodName::Fqn;
    new_critera.value = SelectionValue::Scalar(node_pat.to_string());
    // check if the fqn match the node part of the column selector
    predicate_include_identifier_node(
        &new_critera,
        node,
        previous_state,
        project_name,
        current_nodes,
        adapter_type,
    )
}

fn match_file(pattern: &str, common_attr: &CommonAttributes) -> FsResult<bool> {
    let path = common_attr.original_file_path.to_string_lossy();
    if let Some(fname) = Path::new(&*path).file_name().and_then(|s| s.to_str()) {
        if fnmatch(pattern, fname) {
            Ok(true)
        } else if let Some(stem) = Path::new(fname).file_stem().and_then(|s| s.to_str()) {
            Ok(fnmatch(pattern, stem))
        } else {
            Ok(false)
        }
    } else {
        Ok(false)
    }
}

fn match_package(
    pattern: &str,
    common_attr: &CommonAttributes,
    project_name: Option<&str>,
) -> FsResult<bool> {
    // `this` is an alias for the current dbt project name
    let resolved = if pattern == "this" {
        project_name.unwrap_or(pattern)
    } else {
        pattern
    };
    Ok(fnmatch(resolved, &common_attr.package_name))
}

fn match_path(pattern: &str, common_attr: &CommonAttributes) -> FsResult<bool> {
    let node_path = Path::new(&common_attr.original_file_path);
    let patch_path = common_attr.patch_path.as_ref().map(Path::new);

    // ── 0. Normalise the selector: resolve `.` and `..` components so that
    //       "./models/foo.sql", "models/./foo.sql", and
    //       "models/staging/../foo.sql" all reduce to "models/foo.sql".
    let normalised = Path::new(pattern).clean();
    let pattern = normalised.to_string_lossy();
    let pattern = pattern.as_ref();

    // ── 1. Wildcard selector → fnmatch against the original file and its parent directories
    if has_special_chars(pattern) {
        return Ok(node_path
            .ancestors()
            .any(|path| fnmatch(pattern, &path.to_string_lossy()))
            || patch_path.is_some_and(|p| fnmatch(pattern, &p.to_string_lossy())));
    }

    // ── 2. No wildcards: try both exact match and directory match
    //
    // We don't need to guess whether the selector is a file or directory.
    // Path::starts_with matches on component boundaries, so:
    //   - "models/staging.v2/foo.sql".starts_with("models/staging.v2") → true
    //   - "models/staging.v20/foo.sql".starts_with("models/staging.v2") → false
    //
    // This correctly handles directories with dots like "staging.v2".
    let selector_path = Path::new(pattern);

    // Exact file match (check both original_file_path and patch_path)
    if node_path == selector_path || patch_path.is_some_and(|p| p == selector_path) {
        return Ok(true);
    }

    // Directory match - check if node is under the selector directory
    // NOTE: Only original_file_path is checked for directory membership, NOT patch_path.
    // This matches dbt-core behavior.
    if node_path.starts_with(selector_path) {
        return Ok(true);
    }

    Ok(false)
}

fn match_result(
    pattern: &str,
    common_attr: &CommonAttributes,
    previous_state: Option<&StateArtifacts>,
) -> FsResult<bool> {
    if let Some(prev_state) = previous_state {
        if let Some(run_results) = &prev_state.run_results {
            Ok(run_results.results.iter().any(|result| {
                result.unique_id == common_attr.unique_id && result.status == *pattern
            }))
        } else {
            Ok(false)
        }
    } else {
        Ok(false)
    }
}

fn match_exposure(
    pattern: &str,
    node: &dyn InternalDbtNode,
    common_attr: &CommonAttributes,
) -> FsResult<bool> {
    if node.resource_type() != NodeType::Exposure {
        return Ok(false);
    }
    let parts: Vec<&str> = pattern.split('.').collect();
    let target_package = if parts.len() == 2 { parts[0] } else { "*" };
    let target_name = if parts.len() == 2 { parts[1] } else { parts[0] };

    if parts.len() > 2 {
        return err!(
            ErrorCode::SelectorError,
            "Invalid exposure selector value. Exposures must be of the form ${{exposure_name}} or ${{exposure_package}}.${{exposure_name}}"
        );
    }

    Ok(fnmatch(target_package, &common_attr.package_name)
        && fnmatch(target_name, &common_attr.name))
}

fn match_function(
    pattern: &str,
    node: &dyn InternalDbtNode,
    common_attr: &CommonAttributes,
) -> FsResult<bool> {
    if node.resource_type() != NodeType::Function {
        return Ok(false);
    }
    let parts: Vec<&str> = pattern.split('.').collect();
    let target_package = if parts.len() == 2 { parts[0] } else { "*" };
    let target_name = if parts.len() == 2 { parts[1] } else { parts[0] };

    if parts.len() > 2 {
        return err!(
            ErrorCode::SelectorError,
            "Invalid function selector value. Functions must be of the form ${{function_name}} or ${{function_package}}.${{function_name}}"
        );
    }

    Ok(fnmatch(target_package, &common_attr.package_name)
        && fnmatch(target_name, &common_attr.name))
}

fn match_metric(
    pattern: &str,
    node: &dyn InternalDbtNode,
    common_attr: &CommonAttributes,
) -> FsResult<bool> {
    if node.resource_type() != NodeType::Metric {
        return Ok(false);
    }
    let parts: Vec<&str> = pattern.split('.').collect();
    let target_package = if parts.len() == 2 { parts[0] } else { "*" };
    let target_name = if parts.len() == 2 { parts[1] } else { parts[0] };

    if parts.len() > 2 {
        return err!(
            ErrorCode::SelectorError,
            "Invalid metric selector value. Metrics must be of the form ${{metric_name}} or ${{metric_package}}.${{metric_name}}"
        );
    }

    Ok(fnmatch(target_package, &common_attr.package_name)
        && fnmatch(target_name, &common_attr.name))
}

fn match_saved_query(
    pattern: &str,
    node: &dyn InternalDbtNode,
    common_attr: &CommonAttributes,
) -> FsResult<bool> {
    if node.resource_type() != NodeType::SavedQuery {
        return Ok(false);
    }
    let parts: Vec<&str> = pattern.split('.').collect();
    let target_package = if parts.len() == 2 { parts[0] } else { "*" };
    let target_name = if parts.len() == 2 { parts[1] } else { parts[0] };

    if parts.len() > 2 {
        return err!(
            ErrorCode::SelectorError,
            "Invalid saved query selector value. Saved queries must be of the form ${{saved_query_name}} or ${{saved_query_package}}.${{saved_query_name}}"
        );
    }

    Ok(fnmatch(target_package, &common_attr.package_name)
        && fnmatch(target_name, &common_attr.name))
}

fn match_semantic_model(
    pattern: &str,
    node: &dyn InternalDbtNode,
    common_attr: &CommonAttributes,
) -> FsResult<bool> {
    if node.resource_type() != NodeType::SemanticModel {
        return Ok(false);
    }
    let parts: Vec<&str> = pattern.split('.').collect();
    let target_package = if parts.len() == 2 { parts[0] } else { "*" };
    let target_name = if parts.len() == 2 { parts[1] } else { parts[0] };

    if parts.len() > 2 {
        return err!(
            ErrorCode::SelectorError,
            "Invalid semantic model selector value. Semantic models must be of the form ${{semantic_model_name}} or ${{semantic_model_package}}.${{semantic_model_name}}"
        );
    }

    Ok(fnmatch(target_package, &common_attr.package_name)
        && fnmatch(target_name, &common_attr.name))
}

fn match_test_name(
    pattern: &str,
    node: &dyn InternalDbtNode,
    common_attr: &CommonAttributes,
) -> FsResult<bool> {
    if node.resource_type() == NodeType::Test {
        // For generic tests, match against test_metadata.name (the base test name)
        // to align with dbt-core behavior
        if let Some(test) = node.as_any().downcast_ref::<DbtTest>() {
            if let Some(ref metadata) = test.__test_attr__.test_metadata {
                return Ok(fnmatch(pattern, &metadata.name));
            }
        }
        // Fall back to common_attr.name for singular tests (no test_metadata)
        Ok(fnmatch(pattern, &common_attr.name))
    } else if node.resource_type() == NodeType::UnitTest {
        Ok(fnmatch(pattern, &common_attr.name))
    } else {
        Ok(false)
    }
}

fn match_unit_test(
    pattern: &str,
    node: &dyn InternalDbtNode,
    common_attr: &CommonAttributes,
) -> FsResult<bool> {
    if node.resource_type() != NodeType::UnitTest {
        return Ok(false);
    }
    let parts: Vec<&str> = pattern.split('.').collect();
    let target_package = if parts.len() == 2 { parts[0] } else { "*" };
    let target_name = if parts.len() == 2 { parts[1] } else { parts[0] };

    if parts.len() > 2 {
        return err!(
            ErrorCode::SelectorError,
            "Invalid unit test selector value. Unit tests must be of the form ${{unit_test_name}} or ${{unit_test_package_name}}.${{unit_test_name}}"
        );
    }

    Ok(fnmatch(target_package, &common_attr.package_name)
        && fnmatch(target_name, &common_attr.name))
}

fn match_config(pattern: &str, args: &[String], config: YmlValue) -> FsResult<bool> {
    // dbt-core compat: when no dot-notation args are present, split "key:value" form.
    // e.g. method: config, value: "materialized:table" → same as method: config.materialized, value: table
    if args.is_empty() {
        if let Some((key, value)) = pattern.split_once(':') {
            return match_config_recursive(value, &[key.to_string()], config, 0);
        }
    }
    match_config_recursive(pattern, args, config, 0)
}

fn match_config_recursive(
    pattern: &str,
    args: &[String],
    config: YmlValue,
    args_index: usize,
) -> FsResult<bool> {
    if args_index >= args.len() {
        return Ok(false);
    }
    let key = &args[args_index];

    let mut cfg_map: BTreeMap<String, YmlValue> = dbt_yaml::from_value(config).unwrap();
    if let Some(val) = cfg_map.remove(key) {
        match &val {
            YmlValue::String(s, _) => Ok(fnmatch(pattern, s)),
            YmlValue::Bool(b, _) => Ok(pattern.eq_ignore_ascii_case(&b.to_string())),
            YmlValue::Number(n, _) => Ok(n.to_string() == *pattern),
            YmlValue::Sequence(arr, _) => Ok(arr
                .iter()
                .filter_map(YmlValue::as_str)
                .any(|s| fnmatch(pattern, s))),
            YmlValue::Mapping(..) => match_config_recursive(pattern, args, val, args_index + 1),
            _ => Ok(false),
        }
    } else {
        Ok(false)
    }
}

fn match_access(pattern: &str, access: Option<Access>) -> FsResult<bool> {
    Ok(access.map(|f| f.to_string()) == Some(pattern.to_string()))
}

fn match_group(pattern: &str, group: Option<String>) -> FsResult<bool> {
    Ok(group.as_deref() == Some(pattern))
}

fn match_tag(pattern: &str, tags: Vec<String>) -> FsResult<bool> {
    Ok(tags.iter().any(|t| fnmatch(pattern, t)))
}

/// Matches source nodes based on their freshness status from a previous run.
///
/// This selector method allows filtering sources by comparing their freshness data
/// between the current state and a previous state (stored in sources.json artifacts).
///
/// Valid selector values:
/// - "fresher": Sources where the data has been updated (max_loaded_at is newer than
///   in the previous state). This helps identify sources that have received new data.
///
/// The selector requires:
/// 1. A current sources.json in the target/ directory (from running `dbt source freshness`)
/// 2. A --state directory with a sources.json artifact from a prior `dbt source freshness` run
///
/// # Examples
/// - `dbt list --select "source_status:fresher" --state ./state` - List sources with updated data
/// - `dbt build --select "source_status:fresher+" --state ./state` - Build sources with newer data and their downstream models
///
/// # Reference
/// - dbt docs: https://docs.getdbt.com/reference/node-selection/methods#source_status
fn match_source_status(
    pattern: &str,
    node: &dyn InternalDbtNodeAttributes,
    previous_state: Option<&StateArtifacts>,
) -> FsResult<bool> {
    use dbt_schemas::schemas::{FreshnessResultsArtifact, serde::typed_struct_from_json_file};

    // Only apply to source nodes
    if node.resource_type() != NodeType::Source {
        return Ok(false);
    }

    // We need previous state with freshness results to compare
    let Some(prev_state) = previous_state else {
        return Ok(false);
    };

    let Some(prev_sources_results) = &prev_state.source_freshness_results else {
        // No sources.json in state directory - selector cannot be used
        emit_warn_log_message(
            ErrorCode::SelectorError,
            "source_status selector requires a sources.json with freshness results in the state directory.",
        );
        return Ok(false);
    };

    match pattern.to_lowercase().as_str() {
        "fresher" => {
            // Find the freshness result for this source in the previous state
            let prev_result = prev_sources_results
                .results
                .iter()
                .find(|result| result.unique_id == node.unique_id());

            let Some(prev_result) = prev_result else {
                // Source not found in state sources.json - cannot determine if fresher
                return Ok(false);
            };

            // Load the current sources.json from the target directory
            let Some(target_path) = &prev_state.target_path else {
                return err!(
                    ErrorCode::SelectorError,
                    "source_status:fresher selector requires target directory path to be available. This is an internal error."
                );
            };

            let current_sources_path = target_path.join("sources.json");

            // Try to load the current sources.json
            let current_sources_results: FreshnessResultsArtifact =
                match typed_struct_from_json_file(&current_sources_path) {
                    Ok(results) => results,
                    Err(e) => {
                        return err!(
                            ErrorCode::SelectorError,
                            "source_status:fresher selector requires a current sources.json file in the target directory.\n\
                            Expected path: {}\n\
                            Please run 'dbt source freshness' before using this selector.\n\
                            Error: {}",
                            current_sources_path.display(),
                            e
                        );
                    }
                };

            // Check if current sources.json has any results
            if current_sources_results.results.is_empty() {
                emit_warn_log_message(
                    ErrorCode::SelectorError,
                    "The current sources.json file contains no freshness results.",
                );
                return Ok(false);
            }

            // Find the current result for this source
            let current_result = current_sources_results
                .results
                .iter()
                .find(|result| result.unique_id == node.unique_id());

            let Some(current_result) = current_result else {
                // Source found in previous state but not in current - not fresher
                return Ok(false);
            };

            // Compare max_loaded_at timestamps
            // A source is "fresher" if its current max_loaded_at is newer than the previous one
            Ok(current_result.max_loaded_at > prev_result.max_loaded_at)
        }
        _ => {
            // Invalid pattern - only "fresher" is supported
            Ok(false)
        }
    }
}

fn predicate_include_identifier_node(
    criteria: &SelectionCriteria,
    node: &dyn InternalDbtNodeAttributes,
    previous_state: Option<&StateArtifacts>,
    project_name: Option<&str>,
    current_nodes: Option<&Nodes>,
    adapter_type: AdapterType,
) -> FsResult<bool> {
    let method = criteria.method;
    // Deferred failure point for a non-scalar `value:` (dbt-labs/fs#13604).
    let pattern = criteria.value.resolve(method)?;
    let args: &[String] = &criteria.method_args;
    let common_attr = node.common();

    match method {
        MethodName::Source => match_source(pattern, node),
        MethodName::ResourceType => match_resource_type(pattern, node),
        MethodName::TestType => match_test_type(pattern, node),
        MethodName::Version => match_version(pattern, node),
        MethodName::State => match_state(
            pattern,
            node,
            common_attr,
            previous_state,
            current_nodes,
            adapter_type,
        ),
        MethodName::Fqn => match_fqn(pattern, node, common_attr),
        MethodName::Column => match_column(
            criteria,
            pattern,
            node,
            previous_state,
            project_name,
            current_nodes,
            adapter_type,
        ),
        MethodName::File => match_file(pattern, common_attr),
        MethodName::Package => match_package(pattern, common_attr, project_name),
        MethodName::Path => match_path(pattern, common_attr),
        MethodName::Result => match_result(pattern, common_attr, previous_state),
        MethodName::Exposure => match_exposure(pattern, node, common_attr),
        MethodName::Function => match_function(pattern, node, common_attr),
        MethodName::Metric => match_metric(pattern, node, common_attr),
        MethodName::SavedQuery => match_saved_query(pattern, node, common_attr),
        MethodName::SemanticModel => match_semantic_model(pattern, node, common_attr),
        MethodName::TestName => match_test_name(pattern, node, common_attr),
        MethodName::UnitTest => match_unit_test(pattern, node, common_attr),
        MethodName::Config => match_config(pattern, args, node.serialized_config()),
        MethodName::Access => match_access(pattern, node.get_access()),
        MethodName::Group => match_group(pattern, node.get_group()),
        MethodName::Tag => match_tag(pattern, node.tags()),
        MethodName::SourceStatus => match_source_status(pattern, node, previous_state),
        MethodName::Selector => err!(
            ErrorCode::SelectorError,
            "selector: method cannot be evaluated per-node; it requires graph context"
        ),
    }
}

// -------------------------------------------------------------------------------------------------
// select columns

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SelectionType {
    pub prefix: u32,
    pub suffix: u32,
}
// add fmt print [{},{}]
impl fmt::Display for SelectionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{},{}]", self.prefix, self.suffix)
    }
}

impl SelectionType {
    pub fn prefix(pre: u32) -> Self {
        SelectionType {
            prefix: pre,
            suffix: 0,
        }
    }

    pub fn suffix(suf: u32) -> Self {
        SelectionType {
            prefix: 0,
            suffix: suf,
        }
    }

    pub fn exact() -> Self {
        SelectionType {
            prefix: 0,
            suffix: 0,
        }
    }

    pub fn both(pre: u32, suf: u32) -> Self {
        SelectionType {
            prefix: pre,
            suffix: suf,
        }
    }

    pub fn join(&self, other: &SelectionType) -> Self {
        SelectionType {
            prefix: self.prefix.max(other.prefix),
            suffix: self.suffix.max(other.suffix),
        }
    }

    pub fn meet(&self, other: &SelectionType) -> Self {
        SelectionType {
            prefix: self.prefix.min(other.prefix),
            suffix: self.suffix.min(other.suffix),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ColId {
    pub table: String, // dbt unique id
    pub column: String,
}
impl fmt::Display for ColId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.table, self.column)
    }
}

pub fn filter_select_column(
    nodes: &Nodes,
    dialect: &Dialect,
    select: &SelectExpression,
    previous_state: Option<&StateArtifacts>,
) -> BTreeMap<ColId, SelectionType> {
    let mut selected_items = BTreeMap::new();

    // column level lineage reads models and seeds and NO tests
    for (unique_id, node) in nodes.iter() {
        if !unique_id.starts_with("test.") {
            let node_columns =
                select_expression_include_node_column(select, node, dialect, previous_state);
            union_maps(&mut selected_items, &node_columns);
        }
    }
    selected_items
}

fn select_expression_include_node_column(
    selection: &SelectExpression,
    node: &dyn InternalDbtNode,
    dialect: &Dialect,
    previous_state: Option<&StateArtifacts>,
) -> BTreeMap<ColId, SelectionType> {
    match selection {
        SelectExpression::Atom(criteria) => {
            predicate_include_node_column(criteria, node, dialect, previous_state)
        }
        SelectExpression::And(exprs) => {
            let mut iter = exprs.iter();
            if let Some(first_expr) = iter.next() {
                let mut acc = select_expression_include_node_column(
                    first_expr,
                    node,
                    dialect,
                    previous_state,
                );
                for expr in iter {
                    let sub_result =
                        select_expression_include_node_column(expr, node, dialect, previous_state);
                    acc = intersect_maps(&acc, &sub_result);
                }
                acc
            } else {
                // If there are no expressions, return an empty map
                BTreeMap::new()
            }
        }
        SelectExpression::Or(exprs) => {
            let mut result = BTreeMap::new();
            for sub_expr in exprs {
                let sub_result =
                    select_expression_include_node_column(sub_expr, node, dialect, previous_state);
                union_maps(&mut result, &sub_result);
            }
            result
        }
        SelectExpression::Exclude(_) => {
            // Excludes don't select any columns
            BTreeMap::new()
        }
    }
}

fn predicate_include_node_column(
    criteria: &SelectionCriteria,
    node: &dyn InternalDbtNode,
    dialect: &Dialect,
    _previous_state: Option<&StateArtifacts>,
) -> BTreeMap<ColId, SelectionType> {
    let mut result = BTreeMap::new();

    // 1) column:foo.bar.* → exact match on that table (foo.bar) and that column glob (*)
    // `column:` is internal-only, so a non-scalar value never reaches here.
    if criteria.method == MethodName::Column {
        let Some(value) = criteria.value.as_str() else {
            return result;
        };
        let (table_pat, col_pat) = if let Some(idx) = value.rfind('.') {
            (&value[..idx], &value[idx + 1..])
        } else {
            ("", value)
        };
        // only select columns on matching tables
        if table_pat.is_empty() || fnmatch(table_pat, node.common().unique_id.as_str()) {
            let col_pattern = extract_column_name(col_pat);
            let selection_type = create_selection_type(criteria.clone());
            for column in node.base().columns.iter() {
                let col_name = &column.name;
                if fnmatch(col_pattern, col_name) {
                    result.insert(
                        ColId {
                            table: node.common().unique_id.clone(),
                            column: dialect.parse_identifier(col_name).unwrap().to_string(),
                        },
                        selection_type.clone(),
                    );
                }
            }
        }
    }
    result
}

fn create_selection_type(select: SelectionCriteria) -> SelectionType {
    match (select.parents_depth, select.children_depth) {
        (Some(parents_depth), Some(children_depth)) => {
            SelectionType::both(parents_depth, children_depth)
        }
        (Some(parents_depth), None) => SelectionType::prefix(parents_depth),
        (None, Some(children_depth)) => SelectionType::suffix(children_depth),
        (None, None) => SelectionType::exact(),
    }
}
fn extract_column_name(identifier: &str) -> &str {
    identifier.rsplit('.').next().unwrap_or(identifier)
}

fn has_special_chars(pattern: &str) -> bool {
    const SPECIAL_CHARS: [char; 4] = ['*', '?', '[', ']'];
    pattern.chars().any(|c| SPECIAL_CHARS.contains(&c))
}
pub(crate) fn fnmatch(pattern: &str, text: &str) -> bool {
    if has_special_chars(pattern) {
        match Pattern::new(pattern) {
            Ok(p) => p.matches(text),
            Err(_) => {
                eprintln!("Invalid pattern: {pattern}");
                false
            }
        }
    } else {
        pattern == text
    }
}

fn union_maps(map1: &mut BTreeMap<ColId, SelectionType>, map2: &BTreeMap<ColId, SelectionType>) {
    for (key, value) in map2 {
        map1.entry(key.clone())
            .and_modify(|existing| {
                *existing = existing.join(value);
            })
            .or_insert_with(|| value.clone());
    }
}
fn intersect_maps(
    map1: &BTreeMap<ColId, SelectionType>,
    map2: &BTreeMap<ColId, SelectionType>,
) -> BTreeMap<ColId, SelectionType> {
    let mut result = BTreeMap::new();

    for (key, value1) in map1 {
        if let Some(value2) = map2.get(key) {
            result.insert(key.clone(), value1.meet(value2));
        }
    }

    result
}

// ────────────────────────────────────────────────────────────────────────────
// helper that first tries the full fqn, then the un-scoped fqn
// (this mirrors dbt-core's QualifiedNameSelectorMethod.node_is_match)
// ────────────────────────────────────────────────────────────────────────────
pub fn node_is_match(selector: &str, fqn: &[String], is_versioned: bool) -> bool {
    if is_selected_node(fqn, selector, is_versioned) {
        return true;
    }
    // try again without the package prefix
    if fqn.len() > 1 && is_selected_node(&fqn[1..], selector, is_versioned) {
        return true;
    }
    false
}

// ────────────────────────────────────────────────────────────────────────────
// port of dbt-core's latest is_selected_node(), incl. the new shortcut
// that makes bare wild-carded model names (e.g. "model_*") work.
// ────────────────────────────────────────────────────────────────────────────
pub fn is_selected_node(fqn: &[String], selector: &str, is_versioned: bool) -> bool {
    // Note (Ani): Dbt doesn't do this check, for some reason even though cross project refs don't have fqn they don't make it here so this never panics.
    // There's something wrong with how we're handling cross project refs upstream, but defering no this for now.
    // Empty fqn should never match
    if fqn.is_empty() {
        return false;
    }

    // ── 0. quick shortcut for "wildcard-leaf" selectors
    if !selector.contains('.') && selector.contains(&['*', '?', '[', ']'][..]) {
        return Pattern::new(selector)
            .map(|p| p.matches(fqn.last().unwrap())) // Safe now because we checked fqn.is_empty()
            .unwrap_or(false);
    }

    // ── 1. exact-leaf matches
    if is_versioned {
        // match on just <name> for v-models
        if fqn.len() >= 2 && fqn[fqn.len() - 2] == selector {
            return true;
        }
        let dot_parts: Vec<&str> = selector.rsplitn(2, '.').collect();
        // normalize version format "any_string.v2" -> "any_string_v2"
        let normalized = if dot_parts.len() >= 2 {
            format!(
                "{}_{}",
                dot_parts[dot_parts.len() - 1],
                dot_parts[dot_parts.len() - 2]
            )
        } else {
            selector.to_string()
        };

        if fqn.len() >= 2 && format!("{}_{}", fqn[fqn.len() - 2], fqn[fqn.len() - 1]) == normalized
        {
            return true;
        }
    } else if fqn.last().map(|s| s.as_str()) == Some(selector) {
        return true;
    }

    // ── 2. generic matching with optional wildcards
    // 2a) flatten dots inside folder names
    let mut flat_fqn: Vec<&str> = Vec::new();
    for segment in fqn {
        for part in segment.split('.') {
            flat_fqn.push(part);
        }
    }
    let sel_parts: Vec<&str> = selector.split('.').collect();
    if flat_fqn.len() < sel_parts.len() {
        return false;
    }

    // 2b) walk until first wildcard
    let mut wildcard_ix: Option<usize> = None;
    for (i, part) in sel_parts.iter().enumerate() {
        if part.chars().any(|c| "*?[]".contains(c)) {
            wildcard_ix = Some(i);
            break;
        } else if flat_fqn[i] != *part {
            return false;
        }
    }

    // 2c) if we met a wildcard, match the remainder via fnmatch
    if let Some(ix) = wildcard_ix {
        //TODO: Figure out if we should .join(".") here or not.
        let fqn_tail = flat_fqn[ix..].join(".");
        let sel_tail = sel_parts[ix..].join(".");
        return Pattern::new(&sel_tail)
            .map(|p| p.matches(&fqn_tail))
            .unwrap_or(false);
    }

    // 2d) all concrete segments matched
    true
}

#[cfg(test)]
mod tests {
    use dbt_common::io_args::StaticAnalysisKind;
    use dbt_schemas::schemas::common::{ClusterConfig, DbtMaterialization, DbtUniqueKey};
    use dbt_schemas::schemas::common::{DbtChecksum, ResolvedQuoting};
    use dbt_schemas::schemas::nodes::AdapterAttr;
    use dbt_schemas::schemas::project::{ModelConfig, WarehouseSpecificNodeConfig};
    use dbt_schemas::schemas::serde::StringOrInteger;
    use dbt_schemas::schemas::{
        DbtModelAttr, DbtTestAttr, DbtUnitTestAttr, IntrospectionKind, TestMetadata,
    };
    use indexmap::IndexMap;
    use std::sync::Arc;

    use dbt_schemas::schemas::{
        CommonAttributes, DbtModel, NodeBaseAttributes, common::Access, dbt_column::DbtColumn,
    };

    use dbt_common::node_selector::parse_model_specifiers;

    use super::*;

    fn create_test_node(unique_id: &str, columns: Vec<&str>) -> Arc<DbtModel> {
        Arc::new(DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: unique_id.to_string(),
                tags: vec![],
                meta: IndexMap::new(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                extended_model: true,
                materialized: DbtMaterialization::View,
                quoting: ResolvedQuoting::trues(),
                static_analysis: StaticAnalysisKind::Strict.into(),
                enabled: true,
                columns: columns
                    .iter()
                    .map(|col| {
                        Arc::new(DbtColumn {
                            name: (*col).to_string(),
                            ..Default::default()
                        })
                    })
                    .collect(),
                ..Default::default()
            },
            __model_attr__: DbtModelAttr {
                introspection: IntrospectionKind::None,
                constraints: vec![],
                primary_key: vec![],
                time_spine: None,
                access: Access::default(),
                group: None,
                contract: None,
                incremental_strategy: None,
                freshness: None,
                state: None,
                version: None,
                latest_version: None,
                deprecation_date: None,
                event_time: None,
                catalog_name: None,
                table_format: None,
                sync: None,
                compiled_code: None,
            },
            __adapter_attr__: AdapterAttr::default(),
            __other__: BTreeMap::new(),
            deprecated_config: ModelConfig {
                ..Default::default()
            },
        })
    }

    fn key_as_col_id(map: BTreeMap<String, SelectionType>) -> BTreeMap<ColId, SelectionType> {
        map.into_iter()
            .map(|(key, value)| {
                let mut parts = key.rsplitn(2, '.'); // Split from the back at '.'
                let column = parts.next().unwrap().to_string();
                let table = parts.next().unwrap_or("").to_string();
                (ColId { table, column }, value)
            })
            .collect()
    }

    #[test]
    fn filter_select_criteria_uses_service_state_selection() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.project.selected".to_string(),
            create_test_node("model.project.selected", vec![]),
        );

        let select = parse_model_specifiers(&["state:modified".to_string()]).unwrap();
        let SelectExpression::Atom(criteria) = select else {
            panic!("expected a single selection criterion");
        };
        let results = StateSelectorResults::from([(
            "modified".to_string(),
            BTreeSet::from(["model.project.selected".to_string()]),
        )]);

        let selected = filter_select_criteria_with_state_selector_results(
            &nodes,
            &criteria,
            None,
            Some(&results),
            AdapterType::Bigquery,
        )
        .unwrap();

        assert_eq!(
            selected,
            BTreeSet::from(["model.project.selected".to_string()])
        );
    }

    #[test]
    fn test_filter_select_column_with_identifier() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "node1".to_string(),
            create_test_node("node1", vec!["col_1", "col_2", "other_col"]),
        );

        // Select columns matching "node1.*"
        let input = "column:node1.*";
        let select = parse_model_specifiers(&[input.to_string()]).unwrap();

        let result = filter_select_column(&nodes, &Dialect::Trino, &select, None);

        let expected: BTreeMap<String, SelectionType> = BTreeMap::from([
            ("node1.col_1".to_string(), SelectionType::exact()),
            ("node1.col_2".to_string(), SelectionType::exact()),
            ("node1.other_col".to_string(), SelectionType::exact()),
        ]);

        assert_eq!(result, key_as_col_id(expected));
    }

    #[test]
    fn test_filter_select_column_with_non_matching_pattern() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "node1".to_string(),
            create_test_node("node1", vec!["col_1", "col_2", "other_col"]),
        );

        // Select columns matching "non_matching_*"
        let input = "column:node1.non_matching_*";
        let select = parse_model_specifiers(&[input.to_string()]).unwrap();

        let result = filter_select_column(&nodes, &Dialect::Trino, &select, None);
        let expected: BTreeMap<String, SelectionType> = BTreeMap::new();

        assert_eq!(result, key_as_col_id(expected));
    }

    #[test]
    fn test_filter_select_column_with_wildcard() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "node1".to_string(),
            create_test_node("node1", vec!["col_1", "col_2", "other_col"]),
        );
        // Select all columns using "*"
        let input = "column:node1.*";
        let select = parse_model_specifiers(&[input.to_string()]).unwrap();

        let result = filter_select_column(&nodes, &Dialect::Trino, &select, None);
        let expected: BTreeMap<String, SelectionType> = BTreeMap::from([
            ("node1.col_1".to_string(), SelectionType::exact()),
            ("node1.col_2".to_string(), SelectionType::exact()),
            ("node1.other_col".to_string(), SelectionType::exact()),
        ]);

        assert_eq!(result, key_as_col_id(expected));
    }

    #[test]
    fn test_filter_select_column_with_specific_column() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "node1".to_string(),
            create_test_node("node1", vec!["col_1", "col_2", "other_col"]),
        );

        // Select a specific column from a specific node
        let input = "column:node1.col_1";
        let select = parse_model_specifiers(&[input.to_string()]).unwrap();

        let result = filter_select_column(&nodes, &Dialect::Trino, &select, None);
        let expected: BTreeMap<String, SelectionType> =
            BTreeMap::from([("node1.col_1".to_string(), SelectionType::exact())]);

        assert_eq!(result, key_as_col_id(expected));
    }

    #[test]
    fn test_filter_select_column_with_and() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "node1".to_string(),
            create_test_node("node1", vec!["col_1", "col_2", "other_col"]),
        );

        // Combine two expressions with "And"
        let input = "column:node1.col_*,column:node1.*";
        let select = parse_model_specifiers(&[input.to_string()]).unwrap();
        let result = filter_select_column(&nodes, &Dialect::Trino, &select, None);
        let expected: BTreeMap<String, SelectionType> = BTreeMap::from([
            ("node1.col_1".to_string(), SelectionType::exact()),
            ("node1.col_2".to_string(), SelectionType::exact()),
        ]);

        assert_eq!(result, key_as_col_id(expected));
    }

    #[test]
    fn test_filter_select_column_with_or() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "node1".to_string(),
            create_test_node("node1", vec!["col_1", "col_2", "other_col"]),
        );
        // col_1 or other_*
        let tokens = vec![
            "column:node1.col_1".to_string(),
            "column:node1.other_*".to_string(),
        ];
        let select = parse_model_specifiers(&tokens).unwrap();

        let result = filter_select_column(&nodes, &Dialect::Trino, &select, None);
        let expected: BTreeMap<String, SelectionType> = BTreeMap::from([
            ("node1.col_1".to_string(), SelectionType::exact()),
            ("node1.other_col".to_string(), SelectionType::exact()),
        ]);

        assert_eq!(result, key_as_col_id(expected));
    }

    #[test]
    fn test_filter_select_column_with_no_method() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "node1".to_string(),
            create_test_node("node1", vec!["col_1", "col_2", "other_col"]),
        );

        // Select columns without specifying a method
        let input = "column:n.*";
        let select = parse_model_specifiers(&[input.to_string()]).unwrap();

        let result = filter_select_column(&nodes, &Dialect::Trino, &select, None);
        let expected: BTreeMap<String, SelectionType> = BTreeMap::new();

        assert_eq!(result, key_as_col_id(expected));
    }

    #[test]
    fn test_filter_select_column_with_prefix_match() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "node1".to_string(),
            create_test_node("node1", vec!["col_1", "col_2", "other_col"]),
        );

        let input = "1+column:node1.col_1";
        let select = parse_model_specifiers(&[input.to_string()]).unwrap();
        let result = filter_select_column(&nodes, &Dialect::Trino, &select, None);
        let expected: BTreeMap<String, SelectionType> =
            BTreeMap::from([("node1.col_1".to_string(), SelectionType::prefix(1))]);

        assert_eq!(result, key_as_col_id(expected));
    }

    #[test]
    fn test_filter_select_column_with_suffix_match() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "node1".to_string(),
            create_test_node("node1", vec!["col_1", "col_2", "other_col"]),
        );

        let input = "column:node1.col_1+1";
        let select = parse_model_specifiers(&[input.to_string()]).unwrap();

        let result = filter_select_column(&nodes, &Dialect::Trino, &select, None);
        let expected: BTreeMap<String, SelectionType> =
            BTreeMap::from([("node1.col_1".to_string(), SelectionType::suffix(1))]);

        assert_eq!(result, key_as_col_id(expected));
    }

    #[test]
    fn test_filter_select_column_with_both_match() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "node1".to_string(),
            create_test_node("node1", vec!["col_1", "col_2", "other_col"]),
        );

        // Select columns with both prefix and suffix `+`
        let input = "1+column:node1.col_1+1";
        let select = parse_model_specifiers(&[input.to_string()]).unwrap();
        let result = filter_select_column(&nodes, &Dialect::Trino, &select, None);
        let expected: BTreeMap<String, SelectionType> =
            BTreeMap::from([("node1.col_1".to_string(), SelectionType::both(1, 1))]);

        assert_eq!(result, key_as_col_id(expected));
    }

    #[test]
    fn test_filter_select_column_with_multiple_patterns() {
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "node1".to_string(),
            create_test_node("node1", vec!["col_1", "col_2", "other_col"]),
        );

        let tokens = vec![
            "1+column:node1.col_1".to_string(),
            "column:node1.col_2+1".to_string(),
            "1+column:node1.other_col+1".to_string(),
        ];
        let select = parse_model_specifiers(&tokens).unwrap();

        let result = filter_select_column(&nodes, &Dialect::Trino, &select, None);
        let expected: BTreeMap<String, SelectionType> = BTreeMap::from([
            ("node1.col_1".to_string(), SelectionType::prefix(1)),
            ("node1.col_2".to_string(), SelectionType::suffix(1)),
            ("node1.other_col".to_string(), SelectionType::both(1, 1)),
        ]);

        assert_eq!(result, key_as_col_id(expected));
    }

    #[test]
    fn test_match_access() {
        let mut node = create_test_node("model.test.v1", vec!["col1"]);
        Arc::get_mut(&mut node).unwrap().__model_attr__.access = Access::Public;
        assert!(match_access("public", node.get_access()).unwrap());
        assert!(!match_access("private", node.get_access()).unwrap());

        Arc::get_mut(&mut node).unwrap().__model_attr__.access = Access::Private;
        assert!(match_access("private", node.get_access()).unwrap());
        assert!(!match_access("public", node.get_access()).unwrap());

        Arc::get_mut(&mut node).unwrap().__model_attr__.access = Access::default();
        assert!(!match_access("public", node.get_access()).unwrap());
    }

    #[test]
    fn test_match_group() {
        let mut node = create_test_node("model.test.v1", vec!["col1"]);
        Arc::get_mut(&mut node).unwrap().__model_attr__.group = Some("finance".to_string());
        assert!(match_group("finance", node.get_group()).unwrap());
        assert!(!match_group("marketing", node.get_group()).unwrap());

        Arc::get_mut(&mut node).unwrap().__model_attr__.group = None;
        assert!(!match_group("finance", node.get_group()).unwrap());
    }

    #[test]
    fn test_match_tag() {
        let mut node = create_test_node("model.test.v1", vec!["col1"]);
        Arc::get_mut(&mut node).unwrap().__common_attr__.tags =
            vec!["daily".to_string(), "critical".to_string()];
        assert!(match_tag("daily", node.tags()).unwrap());
        assert!(match_tag("critical", node.tags()).unwrap());
        assert!(!match_tag("weekly", node.tags()).unwrap());

        // Test wildcard matching
        assert!(match_tag("crit*", node.tags()).unwrap());
        assert!(match_tag("*ily", node.tags()).unwrap());

        Arc::get_mut(&mut node).unwrap().__common_attr__.tags = vec![];
        assert!(!match_tag("daily", node.tags()).unwrap());
    }

    #[test]
    fn test_match_version() {
        let mut node = create_test_node("model.test.v1", vec!["col1"]);

        // Test matching a node that is the latest version
        // - Version = 1
        // - Latest version = 1
        // Should match "latest" but not "prerelease", "old" or "none"
        Arc::get_mut(&mut node)
            .unwrap()
            .__common_attr__
            .original_file_path = "models/test_v1.sql".into();
        Arc::get_mut(&mut node).unwrap().__model_attr__.version =
            Some(StringOrInteger::from("1".to_string()));
        Arc::get_mut(&mut node)
            .unwrap()
            .__model_attr__
            .latest_version = Some(StringOrInteger::from("1".to_string()));
        assert!(match_version("latest", node.as_ref()).unwrap());
        assert!(!match_version("prerelease", node.as_ref()).unwrap());
        assert!(!match_version("old", node.as_ref()).unwrap());
        assert!(!match_version("none", node.as_ref()).unwrap());

        // Test matching a prerelease version (version higher than latest)
        // - Version = 2
        // - Latest version = 1
        // Should match "prerelease" but not "latest" or "old"
        Arc::get_mut(&mut node).unwrap().__model_attr__.version =
            Some(StringOrInteger::from("2".to_string()));
        Arc::get_mut(&mut node)
            .unwrap()
            .__model_attr__
            .latest_version = Some(StringOrInteger::from("1".to_string()));
        assert!(!match_version("latest", node.as_ref()).unwrap());
        assert!(match_version("prerelease", node.as_ref()).unwrap());
        assert!(!match_version("old", node.as_ref()).unwrap());

        // Test matching an old version (version lower than latest)
        // - Version = 1
        // - Latest version = 2
        // Should match "old" but not "latest" or "prerelease"
        Arc::get_mut(&mut node).unwrap().__model_attr__.version =
            Some(StringOrInteger::from("1".to_string()));
        Arc::get_mut(&mut node)
            .unwrap()
            .__model_attr__
            .latest_version = Some(StringOrInteger::from("2".to_string()));
        assert!(!match_version("latest", node.as_ref()).unwrap());
        assert!(!match_version("prerelease", node.as_ref()).unwrap());
        assert!(match_version("old", node.as_ref()).unwrap());

        // Test matching a node with no version information
        // - Version = None
        // - Latest version = None
        // Should only match "none" selector
        let mut node = create_test_node("model.test.regular", vec!["col1"]);
        Arc::get_mut(&mut node)
            .unwrap()
            .__common_attr__
            .original_file_path = "models/test.sql".into();
        Arc::get_mut(&mut node).unwrap().__model_attr__.version = None;
        Arc::get_mut(&mut node)
            .unwrap()
            .__model_attr__
            .latest_version = None;
        assert!(!match_version("latest", node.as_ref()).unwrap());
        assert!(!match_version("prerelease", node.as_ref()).unwrap());
        assert!(!match_version("old", node.as_ref()).unwrap());
        assert!(match_version("none", node.as_ref()).unwrap());
    }

    #[test]
    fn test_match_config() {
        let config = ModelConfig {
            materialized: Some(DbtMaterialization::Incremental),
            schema: dbt_common::serde_utils::Omissible::Present(Some("audit".to_string())),
            __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                cluster_by: Some(ClusterConfig::List(vec!["geo_country".to_string()])),
                ..Default::default()
            },
            unique_key: Some(DbtUniqueKey::Multiple(vec![
                "column_a".to_string(),
                "column_b".to_string(),
            ])),
            ..Default::default()
        };
        let mut node = create_test_node("model.test.v1", vec!["col1"]);
        Arc::get_mut(&mut node).unwrap().deprecated_config = config;

        // Test string config
        assert!(
            match_config(
                "incremental",
                &["materialized".to_string()],
                node.serialized_config()
            )
            .unwrap()
        );
        assert!(match_config("audit", &["schema".to_string()], node.serialized_config()).unwrap());

        // Test array config
        assert!(
            match_config(
                "geo_country",
                &["cluster_by".to_string()],
                node.serialized_config()
            )
            .unwrap()
        );
        assert!(
            match_config(
                "column_a",
                &["unique_key".to_string()],
                node.serialized_config()
            )
            .unwrap()
        );

        // Test non-matching values
        assert!(
            !match_config(
                "view",
                &["materialized".to_string()],
                node.serialized_config()
            )
            .unwrap()
        );
        assert!(
            !match_config("staging", &["schema".to_string()], node.serialized_config()).unwrap()
        );

        // Test non-existent config
        assert!(
            !match_config(
                "value",
                &["non_existent".to_string()],
                node.serialized_config()
            )
            .unwrap()
        );

        // Test dbt-core colon-value compat: "key:value" with no dot-notation args
        // i.e. method: config, value: "materialized:incremental" in selectors.yml
        assert!(match_config("materialized:incremental", &[], node.serialized_config()).unwrap());
        assert!(match_config("schema:audit", &[], node.serialized_config()).unwrap());
        assert!(!match_config("materialized:view", &[], node.serialized_config()).unwrap());
        assert!(!match_config("schema:staging", &[], node.serialized_config()).unwrap());
    }

    #[test]
    fn test_match_path() {
        let common_attr = CommonAttributes {
            original_file_path: "models/staging/orders/order_items.sql".into(),
            ..Default::default()
        };

        // Test exact file match
        assert!(match_path("models/staging/orders/order_items.sql", &common_attr).unwrap());

        // Test directory match
        assert!(match_path("models/staging/orders", &common_attr).unwrap());
        assert!(match_path("models/staging", &common_attr).unwrap());

        // Test wildcard patterns
        assert!(match_path("models/*/orders/*.sql", &common_attr).unwrap());
        assert!(match_path("models/**/*items.sql", &common_attr).unwrap());

        // Test non-matching paths
        assert!(!match_path("models/staging/customers", &common_attr).unwrap());
        assert!(!match_path("models/core", &common_attr).unwrap());

        // Test with patch_path set - matches dbt-core behavior:
        // - Directory selectors do NOT match on patch_path (only original_file_path parents)
        // - Exact file selectors DO match on patch_path
        // - Wildcard patterns DO match on patch_path
        let common_attr_with_patch = CommonAttributes {
            original_file_path: "models/staging/orders/order_items.sql".into(),
            patch_path: Some("patches/staging/orders/order_items.sql".into()),
            ..Default::default()
        };

        // Directory selectors should NOT match based on patch_path directory
        // (dbt-core only checks parents for original_file_path, not patch_path)
        assert!(!match_path("patches/staging/orders", &common_attr_with_patch).unwrap());
        assert!(!match_path("patches/staging", &common_attr_with_patch).unwrap());

        // Exact file selector SHOULD match on patch_path (dbt-core behavior)
        assert!(
            match_path(
                "patches/staging/orders/order_items.sql",
                &common_attr_with_patch
            )
            .unwrap()
        );

        // Wildcard patterns SHOULD match on patch_path (dbt-core behavior)
        assert!(match_path("patches/*/orders/*.sql", &common_attr_with_patch).unwrap());
        assert!(match_path("patches/**/*items.sql", &common_attr_with_patch).unwrap());

        // Original file path should still match for all selector types
        assert!(
            match_path(
                "models/staging/orders/order_items.sql",
                &common_attr_with_patch
            )
            .unwrap()
        );
        assert!(match_path("models/staging/orders", &common_attr_with_patch).unwrap());
    }

    #[test]
    fn test_match_path_wildcard_directory() {
        for path in [
            "models/group_a/one/subdir/leaf/model_a.sql",
            "models/group_a/two/subdir/leaf/model_b.sql",
        ] {
            let common_attr = CommonAttributes {
                original_file_path: path.into(),
                ..Default::default()
            };

            assert!(match_path("models/group_a/*/subdir/leaf", &common_attr).unwrap());
        }

        let sibling_attr = CommonAttributes {
            original_file_path: "models/group_a/one/other/model_c.sql".into(),
            ..Default::default()
        };
        assert!(!match_path("models/group_a/*/subdir/leaf", &sibling_attr).unwrap());

        let patched_attr = CommonAttributes {
            original_file_path: "models/other/model.sql".into(),
            patch_path: Some("models/group_a/one/subdir/leaf/schema.yml".into()),
            ..Default::default()
        };
        assert!(!match_path("models/group_a/*/subdir/leaf", &patched_attr).unwrap());
        assert!(match_path("models/group_a/*/subdir/leaf/*.yml", &patched_attr).unwrap());
    }

    #[test]
    fn test_match_path_directory_with_dot() {
        // Test that directories with dots in their names work correctly
        // Path::starts_with matches on component boundaries, so "staging.v2" won't
        // accidentally match "staging.v20"
        let common_attr = CommonAttributes {
            original_file_path: "models/staging.v2/stg_customers_v2.sql".into(),
            ..Default::default()
        };

        // Directory match
        assert!(match_path("models/staging.v2", &common_attr).unwrap());
        assert!(match_path("models", &common_attr).unwrap());

        // Exact file match
        assert!(match_path("models/staging.v2/stg_customers_v2.sql", &common_attr).unwrap());

        // Should NOT match similar-but-different directory names
        assert!(!match_path("models/staging.v20", &common_attr).unwrap());
        assert!(!match_path("models/staging", &common_attr).unwrap());
        assert!(!match_path("models/staging.v", &common_attr).unwrap());
    }

    #[test]
    fn test_match_path_component_boundaries() {
        // Verify that Path::starts_with correctly handles component boundaries
        let common_attr = CommonAttributes {
            original_file_path: "models/staging/orders/order_items.sql".into(),
            ..Default::default()
        };

        // These should match (exact component matches)
        assert!(match_path("models", &common_attr).unwrap());
        assert!(match_path("models/staging", &common_attr).unwrap());
        assert!(match_path("models/staging/orders", &common_attr).unwrap());

        // These should NOT match (partial component matches)
        assert!(!match_path("models/stag", &common_attr).unwrap());
        assert!(!match_path("models/staging/ord", &common_attr).unwrap());
        assert!(!match_path("model", &common_attr).unwrap());

        // Similar names should not match
        assert!(!match_path("models/staging_v2", &common_attr).unwrap());
        assert!(!match_path("models/staging2", &common_attr).unwrap());
    }

    #[test]
    fn test_match_path_exact_vs_directory() {
        // Test that both exact match and directory match work together
        let file_attr = CommonAttributes {
            original_file_path: "models/staging/stg_orders.sql".into(),
            ..Default::default()
        };

        // Exact match
        assert!(match_path("models/staging/stg_orders.sql", &file_attr).unwrap());

        // Directory match
        assert!(match_path("models/staging", &file_attr).unwrap());
        assert!(match_path("models", &file_attr).unwrap());

        // Non-matching
        assert!(!match_path("models/staging/stg_orders", &file_attr).unwrap());
        assert!(!match_path("models/staging/stg_orders.sql.bak", &file_attr).unwrap());
    }

    #[test]
    fn test_match_path_no_false_positives() {
        // Ensure we don't get false positive matches
        let common_attr = CommonAttributes {
            original_file_path: "models/marts/finance/revenue.sql".into(),
            ..Default::default()
        };

        // Should NOT match unrelated paths
        assert!(!match_path("models/staging", &common_attr).unwrap());
        assert!(!match_path("models/marts/marketing", &common_attr).unwrap());
        assert!(!match_path("seeds", &common_attr).unwrap());
        assert!(!match_path("tests", &common_attr).unwrap());

        // Should NOT match partial file names
        assert!(!match_path("models/marts/finance/rev", &common_attr).unwrap());
        assert!(!match_path("models/marts/finance/revenue", &common_attr).unwrap());
    }

    // ── path normalisation via match_path ────────────────────────────────────
    //
    // These tests exercise path normalisation end-to-end through `match_path`
    // so that any behavioural change in `clean-path` that breaks selector
    // matching is caught directly.

    #[test]
    fn test_normalize_selector_path_curdir() {
        // `.` components at any position must be stripped so the selector
        // matches the node whose original_file_path has no `.` components.
        let common_attr = CommonAttributes {
            original_file_path: "models/staging/foo.sql".into(),
            ..Default::default()
        };

        assert!(match_path("./models/staging/foo.sql", &common_attr).unwrap());
        assert!(match_path("models/./staging/foo.sql", &common_attr).unwrap());
        assert!(match_path("./models/./staging/./foo.sql", &common_attr).unwrap());

        // Directory selectors also normalise `.` correctly.
        assert!(match_path("./models/staging", &common_attr).unwrap());
        assert!(match_path("./models/./staging", &common_attr).unwrap());
        assert!(match_path("./models", &common_attr).unwrap());
    }

    #[test]
    fn test_normalize_selector_path_parentdir() {
        // `..` components that stay within the project must be collapsed so the
        // selector resolves to the node's stored path.
        let common_attr = CommonAttributes {
            original_file_path: "models/foo.sql".into(),
            ..Default::default()
        };

        // One level up from a subdirectory lands back on the file.
        assert!(match_path("models/staging/../foo.sql", &common_attr).unwrap());

        // Directory selector that collapses to a parent dir still matches.
        assert!(match_path("models/staging/..", &common_attr).unwrap());

        // Multiple `..` traversals that resolve to a sibling directory.
        let seeds_attr = CommonAttributes {
            original_file_path: "seeds/foo.csv".into(),
            ..Default::default()
        };
        assert!(match_path("models/staging/../../seeds/foo.csv", &seeds_attr).unwrap());
    }

    #[test]
    fn test_normalize_selector_path_leading_parentdir_preserved() {
        // Leading `..` that would escape the project root must NOT match any
        // node, because node paths are always project-relative and never start
        // with `..`.
        let common_attr = CommonAttributes {
            original_file_path: "models/foo.sql".into(),
            ..Default::default()
        };

        assert!(!match_path("../models/foo.sql", &common_attr).unwrap());
        assert!(!match_path("../../models/foo.sql", &common_attr).unwrap());
    }

    // ── match_path integration tests (relative path selectors) ────────────────

    #[test]
    fn test_match_path_relative_dot_prefix() {
        // Selectors starting with "./" must behave identically to those without.
        // This mirrors dbt-core which normalises `.` implicitly via
        // root.glob() + Path.relative_to(root).
        // See: https://github.com/dbt-labs/dbt-fusion/issues/1191

        let common_attr = CommonAttributes {
            original_file_path: "models/my_model.sql".into(),
            ..Default::default()
        };

        // Exact file match with and without "./" prefix
        assert!(match_path("./models/my_model.sql", &common_attr).unwrap());
        assert!(match_path("models/my_model.sql", &common_attr).unwrap());

        // Embedded `.` in the middle of the path
        assert!(match_path("models/./my_model.sql", &common_attr).unwrap());

        // Directory match with and without "./" prefix
        assert!(match_path("./models", &common_attr).unwrap());
        assert!(match_path("models", &common_attr).unwrap());

        // No false positives
        assert!(!match_path("./models/other_model.sql", &common_attr).unwrap());
        assert!(!match_path("./seeds", &common_attr).unwrap());
    }

    #[test]
    fn test_match_path_relative_dot_prefix_nested() {
        // Normalisation must work for files in nested directories.
        let common_attr = CommonAttributes {
            original_file_path: "models/staging/stg_orders.sql".into(),
            ..Default::default()
        };

        assert!(match_path("./models/staging/stg_orders.sql", &common_attr).unwrap());
        assert!(match_path("./models/staging", &common_attr).unwrap());
        assert!(match_path("./models", &common_attr).unwrap());

        // No false positives
        assert!(!match_path("./models/staging/stg_orders", &common_attr).unwrap());
        assert!(!match_path("./models/marts", &common_attr).unwrap());
    }

    #[test]
    fn test_match_path_relative_dot_prefix_with_patch_path() {
        // Normalisation must also work when matching via patch_path.
        let common_attr = CommonAttributes {
            original_file_path: "models/staging/stg_orders.sql".into(),
            patch_path: Some("models/staging/_schema.yml".into()),
            ..Default::default()
        };

        // Exact match on patch_path with "./" prefix
        assert!(match_path("./models/staging/_schema.yml", &common_attr).unwrap());
        // original_file_path still matches
        assert!(match_path("./models/staging/stg_orders.sql", &common_attr).unwrap());
    }

    #[test]
    fn test_match_path_parent_dir_within_project() {
        // `..` that collapses back to a path inside the project must match.
        // e.g. a user typing "models/staging/../stg_orders.sql" should work.
        // NOTE: dbt-core does NOT normalise `..` — Python's glob preserves the
        // `..` verbatim, so `relative_to()` never collapses it.  Fusion is
        // intentionally stricter (more permissive) here.
        let common_attr = CommonAttributes {
            original_file_path: "models/stg_orders.sql".into(),
            ..Default::default()
        };

        assert!(match_path("models/staging/../stg_orders.sql", &common_attr).unwrap());

        // Directory selector that collapses to a parent directory
        assert!(match_path("models/staging/..", &common_attr).unwrap());
    }

    #[test]
    fn test_match_path_parent_dir_escaping_project_no_match() {
        // Leading `..` that would escape the project root must NOT match any
        // node, since node paths never start with `..`.
        let common_attr = CommonAttributes {
            original_file_path: "models/my_model.sql".into(),
            ..Default::default()
        };

        assert!(!match_path("../models/my_model.sql", &common_attr).unwrap());
        assert!(!match_path("../../models/my_model.sql", &common_attr).unwrap());
    }

    #[test]
    fn test_match_file() {
        let common_attr = CommonAttributes {
            original_file_path: "models/staging/orders/order_items.sql".into(),
            ..Default::default()
        };

        // Test exact filename match
        assert!(match_file("order_items.sql", &common_attr).unwrap());

        // Test stem match
        assert!(match_file("order_items", &common_attr).unwrap());

        // Test wildcard patterns
        assert!(match_file("order_*.sql", &common_attr).unwrap());
        assert!(match_file("*_items.sql", &common_attr).unwrap());

        // Test non-matching patterns
        assert!(!match_file("customer_orders.sql", &common_attr).unwrap());
        assert!(!match_file("items.sql", &common_attr).unwrap());
    }

    #[test]
    fn test_match_package() {
        let common_attr = CommonAttributes {
            package_name: "dbt_utils".to_string(),
            ..Default::default()
        };

        // Test exact package match
        assert!(match_package("dbt_utils", &common_attr, None).unwrap());

        // Test wildcard patterns
        assert!(match_package("dbt_*", &common_attr, None).unwrap());
        assert!(match_package("*_utils", &common_attr, None).unwrap());

        // Test non-matching patterns
        assert!(!match_package("dbt_core", &common_attr, None).unwrap());
        assert!(!match_package("snowplow", &common_attr, None).unwrap());
    }

    #[test]
    fn test_match_package_this() {
        // Node from root project
        let root_attr = CommonAttributes {
            package_name: "jaffle_shop".to_string(),
            ..Default::default()
        };

        // Node from an installed package
        let pkg_attr = CommonAttributes {
            package_name: "dbt_utils".to_string(),
            ..Default::default()
        };

        let project_name = Some("jaffle_shop");

        // "this" should match nodes from the root project
        assert!(match_package("this", &root_attr, project_name).unwrap());

        // "this" should NOT match nodes from installed packages
        assert!(!match_package("this", &pkg_attr, project_name).unwrap());

        // "this" without project_name falls back to literal "this" (no match)
        assert!(!match_package("this", &root_attr, None).unwrap());
    }

    #[test]
    fn test_node_is_match() {
        let fqn = vec![
            "package".to_string(),
            "path".to_string(),
            "model_name".to_string(),
        ];
        let fqn_versioned = vec![
            "package".to_string(),
            "path".to_string(),
            "model_name".to_string(),
            "v1".to_string(),
        ];

        // Test exact match
        assert!(node_is_match("model_name", &fqn, false));

        // Test package-qualified match (should match against unscoped name)
        assert!(node_is_match("path.model_name", &fqn, false));

        // Test wildcard match
        assert!(node_is_match("model_*", &fqn, false));

        // Test versioned model match
        assert!(node_is_match("model_name", &fqn_versioned, true));

        // Test non-matching patterns
        assert!(!node_is_match("other_model", &fqn, false));
    }

    #[test]
    fn test_is_selected_node_versioned_model_missing_fqn() {
        let fqn = &[];

        // Test versioned model matching
        assert!(!is_selected_node(fqn, "test_model", false));
        assert!(!is_selected_node(fqn, "test_model.v1", false));
    }

    #[test]
    fn test_is_selected_node_empty_fqn_never_matches() {
        let empty_fqn = &[];

        // Test various selector patterns against empty fqn
        // 1. Simple exact match
        assert!(!is_selected_node(empty_fqn, "model_name", false));

        // 2. Wildcard patterns
        assert!(!is_selected_node(empty_fqn, "model_*", false));
        assert!(!is_selected_node(empty_fqn, "*_name", false));
        assert!(!is_selected_node(empty_fqn, "*", false));

        // 3. Dotted patterns
        assert!(!is_selected_node(empty_fqn, "pkg.model", false));
        assert!(!is_selected_node(empty_fqn, "pkg.model.*", false));

        // 4. Complex wildcards
        assert!(!is_selected_node(empty_fqn, "model_[0-9]*", false));
        assert!(!is_selected_node(empty_fqn, "?odel_name", false));

        // 5. Versioned model patterns
        assert!(!is_selected_node(empty_fqn, "model.v1", true));
        assert!(!is_selected_node(empty_fqn, "model_v1", true));
    }

    #[test]
    fn test_match_test_type() {
        use dbt_schemas::schemas::{
            DbtTest, DbtUnitTest,
            common::{Expect, NodeDependsOn},
        };

        // Test unit tests
        let unit_test_node = Arc::new(DbtUnitTest {
            __common_attr__: CommonAttributes {
                unique_id: "unit_test.test.unit_test".to_string(),
                checksum: DbtChecksum::default(),
                tags: vec![],
                meta: IndexMap::new(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                depends_on: NodeDependsOn::default(),
                quoting: ResolvedQuoting::trues(),
                static_analysis: StaticAnalysisKind::Strict.into(),
                ..Default::default()
            },
            deprecated_config: Default::default(),
            __unit_test_attr__: DbtUnitTestAttr {
                model: "test_model".to_string(),
                given: Vec::new(),
                expect: Expect {
                    rows: None,
                    format: Default::default(),
                    fixture: None,
                },
                versions: None,
                version: None,
                overrides: None,
            },
            ..Default::default()
        });
        assert!(match_test_type("unit", unit_test_node.as_ref()).unwrap());
        assert!(!match_test_type("data", unit_test_node.as_ref()).unwrap());
        assert!(!match_test_type("singular", unit_test_node.as_ref()).unwrap());
        assert!(!match_test_type("generic", unit_test_node.as_ref()).unwrap());

        // Test data tests - singular
        let singular_test_node = Arc::new(DbtTest {
            defined_at: None,
            manifest_original_file_path: "tests/test.sql".into(),
            __common_attr__: CommonAttributes {
                unique_id: "test.test.singular_test".to_string(),
                original_file_path: "tests/test.sql".into(),
                tags: vec![],
                meta: IndexMap::new(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                quoting: ResolvedQuoting::trues(),
                static_analysis: StaticAnalysisKind::Strict.into(),
                ..Default::default()
            },
            deprecated_config: Default::default(),
            __test_attr__: DbtTestAttr {
                column_name: None,
                attached_node: None,
                test_metadata: None,
                file_key_name: None,
                introspection: IntrospectionKind::None,
                original_name: None,
                group: None,
                state: None,
            },
            __adapter_attr__: Default::default(),
            __other__: Default::default(),
        });
        assert!(!match_test_type("unit", singular_test_node.as_ref()).unwrap());
        assert!(match_test_type("data", singular_test_node.as_ref()).unwrap());
        assert!(match_test_type("singular", singular_test_node.as_ref()).unwrap());
        assert!(!match_test_type("generic", singular_test_node.as_ref()).unwrap());

        // Test data tests - generic
        let generic_test_node = Arc::new(DbtTest {
            defined_at: None,
            manifest_original_file_path: "tests/generic_test.sql".into(),
            __common_attr__: CommonAttributes {
                unique_id: "test.test.generic_test".to_string(),
                original_file_path: "target/generic_tests/test.sql".into(),
                tags: vec![],
                meta: IndexMap::new(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                quoting: ResolvedQuoting::trues(),
                static_analysis: StaticAnalysisKind::Strict.into(),
                ..Default::default()
            },
            __test_attr__: DbtTestAttr {
                column_name: None,
                attached_node: None,
                test_metadata: None,
                file_key_name: None,
                introspection: IntrospectionKind::None,
                original_name: None,
                group: None,
                state: None,
            },
            __adapter_attr__: Default::default(),
            deprecated_config: Default::default(),
            __other__: Default::default(),
        });
        assert!(!match_test_type("unit", generic_test_node.as_ref()).unwrap());
        assert!(match_test_type("data", generic_test_node.as_ref()).unwrap());
        assert!(!match_test_type("singular", generic_test_node.as_ref()).unwrap());
        assert!(match_test_type("generic", generic_test_node.as_ref()).unwrap());

        // Test non-test node
        let model_node = create_test_node("model.test.model", vec!["col1"]);
        assert!(!match_test_type("unit", model_node.as_ref()).unwrap());
        assert!(!match_test_type("data", model_node.as_ref()).unwrap());
        assert!(!match_test_type("singular", model_node.as_ref()).unwrap());
        assert!(!match_test_type("generic", model_node.as_ref()).unwrap());

        // Test invalid test type
        assert!(!match_test_type("invalid", unit_test_node.as_ref()).unwrap());
    }

    #[test]
    fn test_match_source_status_basic() {
        // Note: This is a basic test for the match_source_status function.
        // Full integration tests would require setting up proper mock objects
        // with the correct schema structures and runtime freshness data.

        // Test with no previous state - should return false
        let source_node = create_test_node("source.test.my_source.my_table", vec!["col1"]);
        assert!(!match_source_status("fresher", source_node.as_ref(), None).unwrap());

        // Test with non-source node - should return false (only applies to sources)
        let model_node = create_test_node("model.test.my_model", vec!["col1"]);
        assert!(!match_source_status("fresher", model_node.as_ref(), None).unwrap());

        // Test invalid status pattern - should return false
        assert!(!match_source_status("invalid_status", source_node.as_ref(), None).unwrap());

        // Test old/incorrect patterns (pass/warn/error) - should return false
        assert!(!match_source_status("pass", source_node.as_ref(), None).unwrap());
        assert!(!match_source_status("warn", source_node.as_ref(), None).unwrap());
        assert!(!match_source_status("error", source_node.as_ref(), None).unwrap());

        // Case insensitive patterns should work for "fresher"
        assert!(!match_source_status("FRESHER", source_node.as_ref(), None).unwrap());
        assert!(!match_source_status("Fresher", source_node.as_ref(), None).unwrap());

        // Without runtime freshness data, the selector should return false
        // even with valid "fresher" pattern. This is expected behavior as the
        // selector requires comparison with current freshness results.
    }

    #[test]
    fn test_match_source_status_with_state() {
        use chrono::{DateTime, Utc};
        use dbt_schemas::schemas::{
            DbtSource, DbtSourceAttr, FreshnessResultsArtifact, FreshnessResultsMetadata,
            FreshnessResultsNode, TimingInfo,
            common::{FreshnessDefinition, FreshnessStatus},
        };
        use std::collections::BTreeMap;
        use std::sync::Arc;
        use tempfile::TempDir;

        // Create temporary directories for state and target
        let state_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();

        // Create a source node
        let source_unique_id = "source.test.my_source.my_table";
        let source_node = Arc::new(DbtSource {
            __common_attr__: CommonAttributes {
                unique_id: source_unique_id.to_string(),
                name: "my_table".to_string(),
                package_name: "test".to_string(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                columns: vec![],
                ..Default::default()
            },
            __source_attr__: DbtSourceAttr {
                source_name: "my_source".to_string(),
                ..Default::default()
            },
            ..Default::default()
        });

        // Create previous freshness results (older timestamp)
        let prev_time: DateTime<Utc> = "2024-01-01T12:00:00Z".parse().unwrap();
        let prev_results = FreshnessResultsArtifact {
            metadata: FreshnessResultsMetadata {
                dbt_schema_version: "1.0".to_string(),
                dbt_version: "2.0.0".to_string(),
                generated_at: prev_time,
                invocation_id: "prev-invocation".to_string(),
                invocation_started_at: Some(prev_time),
                env: BTreeMap::new(),
            },
            results: vec![FreshnessResultsNode {
                resource_type: None,
                unique_id: source_unique_id.to_string(),
                max_loaded_at: prev_time,
                snapshotted_at: prev_time,
                max_loaded_at_time_ago_in_s: 0.0,
                status: FreshnessStatus::Pass,
                criteria: FreshnessDefinition::default(),
                adapter_response: BTreeMap::new(),
                timing: vec![TimingInfo {
                    name: "freshness".to_string(),
                    started_at: None,
                    completed_at: None,
                }],
                thread_id: "thread1".to_string(),
                execution_time: 1.0,
                node: None,
            }],
            elapsed_time: 1.0,
        };

        // Write previous sources.json
        let prev_sources_path = state_dir.path().join("sources.json");
        std::fs::write(
            &prev_sources_path,
            serde_json::to_string_pretty(&prev_results).unwrap(),
        )
        .unwrap();

        // Create current freshness results (newer timestamp)
        let current_time: DateTime<Utc> = "2024-01-02T12:00:00Z".parse().unwrap();
        let current_results = FreshnessResultsArtifact {
            metadata: FreshnessResultsMetadata {
                dbt_schema_version: "1.0".to_string(),
                dbt_version: "2.0.0".to_string(),
                generated_at: current_time,
                invocation_id: "current-invocation".to_string(),
                invocation_started_at: Some(current_time),
                env: BTreeMap::new(),
            },
            results: vec![FreshnessResultsNode {
                resource_type: None,
                unique_id: source_unique_id.to_string(),
                max_loaded_at: current_time,
                snapshotted_at: current_time,
                max_loaded_at_time_ago_in_s: 0.0,
                status: FreshnessStatus::Pass,
                criteria: FreshnessDefinition::default(),
                adapter_response: BTreeMap::new(),
                timing: vec![TimingInfo {
                    name: "freshness".to_string(),
                    started_at: None,
                    completed_at: None,
                }],
                thread_id: "thread1".to_string(),
                execution_time: 1.0,
                node: None,
            }],
            elapsed_time: 1.0,
        };

        // Write current sources.json
        let current_sources_path = target_dir.path().join("sources.json");
        std::fs::write(
            &current_sources_path,
            serde_json::to_string_pretty(&current_results).unwrap(),
        )
        .unwrap();

        // Create StateArtifacts with both state and target paths
        let prev_state = StateArtifacts::new_for_source_freshness(
            state_dir.path().to_path_buf(),
            Some(target_dir.path().to_path_buf()),
            Some(prev_results),
        );

        // Test that source is identified as "fresher" (newer timestamp)
        assert!(
            match_source_status("fresher", source_node.as_ref(), Some(&prev_state)).unwrap(),
            "Source should be identified as fresher when current max_loaded_at > previous max_loaded_at"
        );

        // Test with same timestamps (not fresher)
        let same_time_results = FreshnessResultsArtifact {
            metadata: FreshnessResultsMetadata {
                dbt_schema_version: "1.0".to_string(),
                dbt_version: "2.0.0".to_string(),
                generated_at: prev_time,
                invocation_id: "same-invocation".to_string(),
                invocation_started_at: Some(prev_time),
                env: BTreeMap::new(),
            },
            results: vec![FreshnessResultsNode {
                resource_type: None,
                unique_id: source_unique_id.to_string(),
                max_loaded_at: prev_time, // Same as previous
                snapshotted_at: prev_time,
                max_loaded_at_time_ago_in_s: 0.0,
                status: FreshnessStatus::Pass,
                criteria: FreshnessDefinition::default(),
                adapter_response: BTreeMap::new(),
                timing: vec![TimingInfo {
                    name: "freshness".to_string(),
                    started_at: None,
                    completed_at: None,
                }],
                thread_id: "thread1".to_string(),
                execution_time: 1.0,
                node: None,
            }],
            elapsed_time: 1.0,
        };

        std::fs::write(
            &current_sources_path,
            serde_json::to_string_pretty(&same_time_results).unwrap(),
        )
        .unwrap();

        assert!(
            !match_source_status("fresher", source_node.as_ref(), Some(&prev_state)).unwrap(),
            "Source should not be fresher when timestamps are the same"
        );
    }

    #[test]
    fn test_match_result() {
        // Note: This is a basic test for the match_result function.
        // Full integration tests would require setting up proper mock objects
        // with the correct schema structures.

        // Create a test node
        let test_node = create_test_node("model.test.my_model", vec!["col1"]);
        let common_attr = test_node.common();

        // Test without previous state - should return false
        assert!(!match_result("success", common_attr, None).unwrap());
        assert!(!match_result("error", common_attr, None).unwrap());
        assert!(!match_result("fail", common_attr, None).unwrap());

        // Test with different status patterns - should not error
        assert!(!match_result("pass", common_attr, None).unwrap());
        assert!(!match_result("skipped", common_attr, None).unwrap());
        assert!(!match_result("warn", common_attr, None).unwrap());
    }

    #[test]
    fn test_match_test_name_uses_test_metadata() {
        // Test that match_test_name uses test_metadata.name for generic tests
        // This aligns with dbt-core behavior where test_name selector matches
        // the base test name (e.g., "not_null") rather than the full unique name
        // (e.g., "not_null_my_model_column_abc123")

        // Generic test with test_metadata - should match on test_metadata.name
        let generic_test_with_metadata = Arc::new(DbtTest {
            defined_at: None,
            manifest_original_file_path: "models/schema.yml".into(),
            __common_attr__: CommonAttributes {
                // Full unique name includes hash suffix
                name: "not_null_my_model_column_abc123def456".to_string(),
                unique_id: "test.my_project.not_null_my_model_column_abc123def456".to_string(),
                original_file_path: "target/compiled/test.sql".into(),
                tags: vec![],
                meta: IndexMap::new(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                quoting: ResolvedQuoting::trues(),
                static_analysis: StaticAnalysisKind::Strict.into(),
                ..Default::default()
            },
            __test_attr__: DbtTestAttr {
                column_name: Some("column".to_string()),
                attached_node: Some("model.my_project.my_model".to_string()),
                test_metadata: Some(TestMetadata {
                    name: "not_null".to_string(), // Base test name
                    kwargs: BTreeMap::new(),
                    namespace: None,
                }),
                file_key_name: None,
                introspection: IntrospectionKind::None,
                original_name: None,
                group: None,
                state: None,
            },
            __adapter_attr__: Default::default(),
            deprecated_config: Default::default(),
            __other__: Default::default(),
        });

        let common_attr = generic_test_with_metadata.common();

        // Should match on test_metadata.name ("not_null"), not common_attr.name
        assert!(
            match_test_name("not_null", generic_test_with_metadata.as_ref(), common_attr).unwrap()
        );

        // Should NOT match on the full unique name since we use test_metadata.name
        assert!(
            !match_test_name(
                "not_null_my_model_column_abc123def456",
                generic_test_with_metadata.as_ref(),
                common_attr
            )
            .unwrap()
        );

        // Wildcard should work
        assert!(
            match_test_name("not_*", generic_test_with_metadata.as_ref(), common_attr).unwrap()
        );

        // Singular test without test_metadata - should fall back to common_attr.name
        let singular_test = Arc::new(DbtTest {
            defined_at: None,
            manifest_original_file_path: "tests/my_singular_test.sql".into(),
            __common_attr__: CommonAttributes {
                name: "my_singular_test".to_string(),
                unique_id: "test.my_project.my_singular_test".to_string(),
                original_file_path: "tests/my_singular_test.sql".into(),
                tags: vec![],
                meta: IndexMap::new(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                quoting: ResolvedQuoting::trues(),
                static_analysis: StaticAnalysisKind::Strict.into(),
                ..Default::default()
            },
            __test_attr__: DbtTestAttr {
                column_name: None,
                attached_node: None,
                test_metadata: None, // No test_metadata for singular tests
                file_key_name: None,
                introspection: IntrospectionKind::None,
                original_name: None,
                group: None,
                state: None,
            },
            __adapter_attr__: Default::default(),
            deprecated_config: Default::default(),
            __other__: Default::default(),
        });

        let singular_common_attr = singular_test.common();

        // Singular test should match on common_attr.name
        assert!(
            match_test_name(
                "my_singular_test",
                singular_test.as_ref(),
                singular_common_attr
            )
            .unwrap()
        );
    }

    #[test]
    fn test_match_fqn_excludes_sources() {
        use dbt_schemas::schemas::{DbtSource, DbtSourceAttr};

        // Create a source node with the same name as a model
        let source_node = Arc::new(DbtSource {
            __common_attr__: CommonAttributes {
                unique_id: "source.test.my_source.my_table".to_string(),
                name: "my_table".to_string(),
                package_name: "test".to_string(),
                fqn: vec![
                    "test".to_string(),
                    "my_source".to_string(),
                    "my_table".to_string(),
                ],
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                columns: vec![],
                ..Default::default()
            },
            __source_attr__: DbtSourceAttr {
                source_name: "my_source".to_string(),
                ..Default::default()
            },
            ..Default::default()
        });

        // Create a model node with the same name
        let model_node = Arc::new(DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: "model.test.my_table".to_string(),
                name: "my_table".to_string(),
                package_name: "test".to_string(),
                fqn: vec!["test".to_string(), "my_table".to_string()],
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                columns: vec![],
                ..Default::default()
            },
            ..Default::default()
        });

        // FQN selector "my_table" should NOT match the source
        // (sources need explicit source: selector)
        assert!(
            !match_fqn("my_table", source_node.as_ref(), source_node.common()).unwrap(),
            "FQN selector should not match sources - they need explicit source: method"
        );

        // FQN selector "my_table" SHOULD match the model
        assert!(
            match_fqn("my_table", model_node.as_ref(), model_node.common()).unwrap(),
            "FQN selector should match models by name"
        );
    }

    #[test]
    fn test_match_fqn_includes_exposures() {
        use dbt_schemas::schemas::{DbtExposure, DbtExposureAttr};

        // Create an exposure node
        let exposure_node = Arc::new(DbtExposure {
            __common_attr__: CommonAttributes {
                unique_id: "exposure.test.my_exposure".to_string(),
                name: "my_exposure".to_string(),
                package_name: "test".to_string(),
                fqn: vec!["test".to_string(), "my_exposure".to_string()],
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                columns: vec![],
                ..Default::default()
            },
            __exposure_attr__: DbtExposureAttr {
                owner: Default::default(),
                label: None,
                maturity: None,
                ..Default::default()
            },
            deprecated_config: Default::default(),
        });

        // FQN selector SHOULD match the exposure (dbt-core includes exposures in non_source_nodes)
        assert!(
            match_fqn(
                "my_exposure",
                exposure_node.as_ref(),
                exposure_node.common()
            )
            .unwrap(),
            "FQN selector should match exposures by name"
        );
    }

    #[test]
    /// Regression (dbt-labs/fs#13604): a non-scalar `value:` must fail only once
    /// resolved against a node, and the error must name the method.
    fn test_unsupported_criteria_value_fails_only_on_resolution() {
        let node = create_test_node("model.test.my_model", vec![]);
        let mut criteria = SelectionCriteria::new(
            MethodName::Path,
            vec![],
            "placeholder",
            false,
            None,
            None,
            None,
            None,
        );
        criteria.value = SelectionValue::Unsupported(Box::new(dbt_yaml::Value::from(vec![
            dbt_yaml::Value::from("models"),
            dbt_yaml::Value::from("seeds"),
        ])));
        let expr = SelectExpression::Atom(criteria);

        let result = select_expression_include_node(
            &expr,
            node.as_ref(),
            None,
            None,
            None,
            AdapterType::DuckDB,
        );

        assert!(
            result.is_err(),
            "resolving a criteria with an unsupported value should fail, not silently match/skip"
        );
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("path"),
            "error should name the method that received the bad value, got: {message}"
        );
    }
}
