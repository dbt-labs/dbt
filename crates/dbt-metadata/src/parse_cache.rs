//! Cache state construction for the incremental compile path.
//!
//! This module bridges the parquet-backed parse cache (file mtimes, node snapshots) and
//! the in-memory `CacheState` consumed by `resolve_phase` in `dbt-compilation`.
//!
//! # How CacheState is used
//!
//! `resolve_phase` receives an `Option<&CacheState>`. When `Some`:
//! 1. `drop_all_unchanged_nodes` removes all unimpacted files from `DbtState` so the
//!    resolver only sees changed files.
//! 2. Pre-resolved macros and nodes from `unimpacted_resolved_nodes` are passed as
//!    starting state to the resolver (it skips re-resolving them).
//! 3. After resolution, `add_all_unchanged_nodes` merges the cached nodes back in.
//!
//! When `None`: full resolve — all files processed, no prior nodes reused.
//!
//! # Threshold
//!
//! If more than `INCREMENTAL_CACHE_FILE_THRESHOLD` files are impacted, the per-file
//! tracking overhead exceeds the savings and we fall back to a full parse.

/// Maximum number of impacted files (changed + new + deleted) before we abandon the
/// incremental cache and fall back to a full parse. Beyond this threshold the overhead
/// of tracking individual file impacts exceeds the cost of a clean full resolve.
const INCREMENTAL_CACHE_FILE_THRESHOLD: usize = 100;

use dbt_common::path::{DbtPath, resource_extension};
use dbt_common::{
    FsResult,
    constants::{
        DBT_DEPENDENCIES_YML, DBT_PACKAGES_LOCK_FILE, DBT_PACKAGES_YML, DBT_PROFILES_YML,
        DBT_PROJECT_YML, DBT_SELECTORS_YML,
    },
    io_args::IoArgs,
    stdfs,
};
use dbt_schemas::state::{
    GetColumnsInRelationCalls, GetRelationCalls, Macros, Operations, PatternedDanglingSources,
};
use dbt_schemas::{
    schemas::{InternalDbtNodeAttributes, Nodes, telemetry::NodeType},
    state::{
        CacheState, DbtAsset, DbtState, FileChanges, NodeExecutionState, NodeExecutionStatus,
        NodeStatus, ResolvedNodes, ResolverState,
    },
};
use dbt_yaml::Value;
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::Arc,
};

fn strip_in_dir(io: &IoArgs, base_path: &Path, path: &Path) -> DbtPath {
    DbtPath::from(
        base_path
            .join(path)
            .strip_prefix(&io.in_dir)
            .expect("expected to strip prefix"),
    )
}

fn strip_in_dir_from_asset(io: &IoArgs, asset: &DbtAsset) -> DbtPath {
    strip_in_dir(io, &asset.base_path, &asset.path)
}

fn is_asset_unchanged(io: &IoArgs, asset: &DbtAsset, unchanged_files: &HashSet<DbtPath>) -> bool {
    let rel_path = strip_in_dir_from_asset(io, asset);
    unchanged_files.contains(&rel_path)
}

fn drop_unchanged_nodes_from_assets(
    io: &IoArgs,
    unchanged_files: &HashSet<DbtPath>,
    assets: &mut Vec<DbtAsset>,
) {
    assets.retain(|asset| !is_asset_unchanged(io, asset, unchanged_files));
}

/// Drops all seen assets [DbtAsset] from the dbt state.
/// [dbt_schemas::state::DbtPackage::all_paths] is not modified.
///
/// For the root package (index 0) and local-path deps: filter all asset lists
/// so only changed files remain for re-resolution.
///
/// For pinned dep packages (hub / git / tarball / private): their content is
/// guaranteed unchanged by the `package-lock.yml` hash check in `validate()`.
/// We keep the package entry in `dbt_state.packages` so that:
///
///   1. The macro-resolution loop in `resolver.rs` can re-register their macro
///      namespaces in `THREAD_LOCAL_DEPENDENCIES` (fixes dispatch for all dep
///      namespaces, e.g. `dbt_utils.generate_surrogate_key`).
///   2. All dep package names appear in `macros.macros.keys()`, which is now
///      the source of truth for namespace registration.
///
/// We clear their asset lists so no files are re-parsed — their resolved nodes
/// and macros come from the cache via `add_all_unchanged_nodes`.
pub fn drop_all_unchanged_nodes(
    io: &IoArgs,
    dbt_state: &mut DbtState,
    unchanged_files: &HashSet<DbtPath>,
) {
    for (i, package) in dbt_state.packages.iter_mut().enumerate() {
        let is_root = i == 0;
        // Non-root deps installed via `dbt deps` land under dbt_packages/<name>.
        // Local-path deps (`local: ../foo`) live outside dbt_packages — they need
        // file scanning because the lock file doesn't pin their content.
        let is_local_dep = i > 0
            && !package
                .package_root_path
                .components()
                .any(|c| c.as_os_str() == "dbt_packages");

        if is_root || is_local_dep {
            // Root package and local deps: filter each asset list to keep only
            // changed files so they get re-parsed/re-resolved.
            drop_unchanged_nodes_from_assets(io, unchanged_files, &mut package.dbt_properties);
            drop_unchanged_nodes_from_assets(io, unchanged_files, &mut package.analysis_files);
            drop_unchanged_nodes_from_assets(io, unchanged_files, &mut package.model_sql_files);
            drop_unchanged_nodes_from_assets(io, unchanged_files, &mut package.macro_files);
            drop_unchanged_nodes_from_assets(io, unchanged_files, &mut package.test_files);
            drop_unchanged_nodes_from_assets(io, unchanged_files, &mut package.seed_files);
            drop_unchanged_nodes_from_assets(io, unchanged_files, &mut package.docs_files);
            drop_unchanged_nodes_from_assets(io, unchanged_files, &mut package.snapshot_files);
            drop_unchanged_nodes_from_assets(io, unchanged_files, &mut package.fixture_files);
        } else {
            // Pinned dep (hub/git/tarball/private): lock-file hash guarantees
            // content unchanged. Clear all asset lists so nothing is re-parsed.
            // The package entry itself stays so its name is registered as a
            // known macro namespace in the JinjaEnv.
            package.dbt_properties.clear();
            package.analysis_files.clear();
            package.model_sql_files.clear();
            package.macro_files.clear();
            package.test_files.clear();
            package.seed_files.clear();
            package.docs_files.clear();
            package.snapshot_files.clear();
            package.fixture_files.clear();
            package.all_paths.clear();
        }
    }
    // All packages are retained — dep packages are needed for namespace
    // registration even when they have no files to re-parse.
}

/// Adds all nodes from the cached `ResolvedNodes` to the `ResolverState`.
pub fn add_all_unchanged_nodes(resolved_state: &mut ResolverState, cached_nodes: &ResolvedNodes) {
    // todo: maybe pass ownership so that we don't have to clone?
    let ResolvedNodes {
        nodes,
        disabled_nodes,
        macros,
        operations,
    } = cached_nodes;
    // Add all nodes to the resolved state
    resolved_state.nodes.extend(nodes.clone());
    resolved_state.disabled_nodes.extend(disabled_nodes.clone());
    resolved_state.macros.macros.extend(macros.macros.clone());
    resolved_state.operations = operations.clone();
}

// ----------------------------------------------------------

type DbtNodeRef = Arc<dyn InternalDbtNodeAttributes>;

fn is_sql_file_path(path: &Path) -> bool {
    resource_extension(path) == Some("sql")
}

fn is_yml_file_path(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("yml"))
}

fn is_schema_impacted_model_file_path(
    schema_impacted_models: &HashMap<PathBuf, HashMap<NodeType, HashSet<String>>>,
    input_file: &Path,
    resource_type: NodeType,
) -> bool {
    if schema_impacted_models.is_empty() {
        return false;
    }

    // Only do this for SQL files.
    if !is_sql_file_path(input_file) {
        return false;
    }

    // Check the file_stem, example: "A" is the file_stem from "models/A.sql"
    let Some(file_stem) = input_file.file_stem().and_then(|x| x.to_str()) else {
        return false;
    };

    for map in schema_impacted_models.values() {
        let Some(set) = map.get(&resource_type) else {
            continue;
        };

        if set.contains(file_stem) {
            return true;
        }
    }
    false
}

fn compute_impacted_models_by_schema_file_paths(
    io: &IoArgs,
    changed_schema_files: &HashSet<PathBuf>,
) -> FsResult<HashMap<PathBuf, HashMap<NodeType, HashSet<String>>>> {
    let mut impacted: HashMap<PathBuf, HashMap<NodeType, HashSet<String>>> = HashMap::new();
    for schema_yaml in changed_schema_files {
        let schema_path = schema_yaml.clone();
        let schema_yaml_content = stdfs::read_to_string(io.in_dir.join(schema_yaml))?;
        let doc: Value =
            dbt_yaml::from_str(&schema_yaml_content).unwrap_or(Value::Null(Default::default()));

        let mut resource_map: HashMap<NodeType, HashSet<String>> = HashMap::new();

        if let Value::Mapping(map, _span) = doc {
            for (key, value) in map {
                if let Value::String(resource_type, _span) = key {
                    let resource_type = {
                        match resource_type.as_str() {
                            "models" => Some(NodeType::Model),
                            "seeds" => Some(NodeType::Seed),
                            "snapshots" => Some(NodeType::Snapshot),
                            "sources" => Some(NodeType::Source),
                            _ => None,
                        }
                    };

                    let Some(resource_type) = resource_type else {
                        continue;
                    };

                    if let Value::Sequence(items, _span) = value {
                        for item in items {
                            if let Value::Mapping(item_map, _span) = item
                                && let Some(Value::String(name, _span)) = item_map
                                    .get(Value::String("name".to_string(), Default::default()))
                            {
                                resource_map
                                    .entry(resource_type)
                                    .or_default()
                                    .insert(name.clone());
                            }
                        }
                    }
                }
            }
        }
        if !resource_map.is_empty() {
            impacted.insert(schema_path, resource_map);
        }
    }
    Ok(impacted)
}

/// Filter nodes based on file changes and command type.
#[allow(clippy::too_many_arguments)]
fn filter_dbt_nodes_by_file_changes(
    io: &IoArgs,
    nodes: HashMap<String, DbtNodeRef>,
    node_statuses: &HashMap<String, NodeStatus>,
    input_file_lookup: HashMap<String, Vec<PathBuf>>,
    out_unimpacted_files: &mut HashSet<PathBuf>,
    out_impacted_files: &mut HashSet<PathBuf>,
    out_impacted_schema_files: &mut HashSet<PathBuf>,
) -> FsResult<(HashMap<String, DbtNodeRef>, HashMap<String, DbtNodeRef>)> {
    // Build reverse dependency map (who depends on whom)
    let mut reverse_deps: HashMap<String, Vec<String>> = HashMap::new();
    for (unique_id, node) in &nodes {
        for dep in &node.base().depends_on.nodes {
            reverse_deps
                .entry(dep.clone())
                .or_default()
                .push(unique_id.clone());
        }
        for dep in &node.base().depends_on.macros {
            reverse_deps
                .entry(dep.clone())
                .or_default()
                .push(unique_id.clone());
        }
    }

    // Find nodes directly impacted by schema file changes and input file changes
    let mut impacted_nodes_by_unique_id = HashSet::new();

    // This loop will handle catching duplicate model definitions across more than one schema.
    // The majority of the time, this will not loop.
    let mut impacted_schema_files = out_impacted_schema_files.clone();
    loop {
        let mut new_impacted_nodes_by_unique_id = HashSet::new();

        let schema_impacted_models =
            compute_impacted_models_by_schema_file_paths(io, &impacted_schema_files)?;
        // Clear this so we can potentially pickup additional impacted schema files.
        impacted_schema_files.clear();

        for (unique_id, node) in &nodes {
            if impacted_nodes_by_unique_id.contains(unique_id) {
                continue;
            }

            let Some(input_files) = input_file_lookup.get(unique_id) else {
                continue;
            };

            let node_status = node_statuses
                .get(unique_id)
                .and_then(|x| x.latest_status.clone());

            let is_impacted_by_status = matches!(node_status, Some(NodeExecutionStatus::Error));

            for input_file in input_files {
                if is_impacted_by_status
                    || out_impacted_files.contains(input_file)
                    || is_schema_impacted_model_file_path(
                        &schema_impacted_models,
                        input_file,
                        node.resource_type(),
                    )
                {
                    new_impacted_nodes_by_unique_id.insert(unique_id.clone());
                    for input_file in input_files {
                        // Assumes that YML files are schema files.
                        if is_yml_file_path(input_file)
                            && !out_impacted_schema_files.contains(input_file)
                        {
                            impacted_schema_files.insert(input_file.clone());
                        }
                    }
                    break;
                }
            }
        }

        // Find ALL downstream dependencies (transitive closure)
        // Only do this for sources.
        let mut to_process: Vec<String> = new_impacted_nodes_by_unique_id.iter().cloned().collect();

        while let Some(current) = to_process.pop() {
            let Some(node) = nodes.get(&current) else {
                continue;
            };
            if node.resource_type() != NodeType::Source {
                continue;
            }
            if let Some(dependents) = reverse_deps.get(&current) {
                for dependent in dependents {
                    if new_impacted_nodes_by_unique_id.insert(dependent.clone()) {
                        // New impacted node found, add to processing queue
                        to_process.push(dependent.clone());
                    }
                }
            }
        }

        impacted_nodes_by_unique_id.extend(new_impacted_nodes_by_unique_id);

        // No additional schema files were impacted, therefore we are done.
        if impacted_schema_files.is_empty() {
            break;
        }

        // Mark the additional schema files as changed.
        for impacted_schema_file in &impacted_schema_files {
            out_unimpacted_files.remove(impacted_schema_file);
            out_impacted_schema_files.insert(impacted_schema_file.clone());
            out_impacted_files.insert(impacted_schema_file.clone());
        }
    }

    // Partition nodes into keep vs invalidated
    let (keep, invalidated): (HashMap<String, DbtNodeRef>, HashMap<String, DbtNodeRef>) = nodes
        .into_iter()
        .partition(|(unique_id, _)| !impacted_nodes_by_unique_id.contains(unique_id));

    // Update file lists for cache reporting
    for node in invalidated.values() {
        let Some(input_files) = input_file_lookup.get(&node.common().unique_id) else {
            continue;
        };
        for input_file in input_files {
            if out_unimpacted_files.remove(input_file) {
                out_impacted_files.insert(input_file.clone());
            }
        }
    }

    Ok((keep, invalidated))
}

pub struct PreviousResolvedState<'a> {
    pub nodes: &'a Nodes,
    pub disabled_nodes: &'a Nodes,
    pub macros: &'a Macros,
    pub operations: &'a Operations,
    pub node_statuses: HashMap<String, NodeStatus>,
    pub get_relation_calls: &'a GetRelationCalls,
    pub get_columns_in_relation_calls: &'a GetColumnsInRelationCalls,
    pub patterned_dangling_sources: &'a PatternedDanglingSources,
}

impl PreviousResolvedState<'_> {
    pub fn from_resolved_state<'a>(resolved_state: &'a ResolverState) -> PreviousResolvedState<'a> {
        // Build node statuses map for nodes with resolution errors so next time they are not reused.
        let mut node_statuses = HashMap::new();
        for unique_id in &resolved_state.nodes_with_resolution_errors {
            let node_status = NodeStatus {
                latest_state: Some(NodeExecutionState::Parsed),
                latest_status: Some(NodeExecutionStatus::Error),
                latest_time: None,
                latest_message: Some("Unresolved reference or source".to_string()),
            };
            node_statuses.insert(unique_id.clone(), node_status);
        }

        PreviousResolvedState {
            nodes: &resolved_state.nodes,
            disabled_nodes: &resolved_state.disabled_nodes,
            macros: &resolved_state.macros,
            operations: &resolved_state.operations,
            node_statuses,
            get_relation_calls: &resolved_state.get_relation_calls,
            get_columns_in_relation_calls: &resolved_state.get_columns_in_relation_calls,
            patterned_dangling_sources: &resolved_state.patterned_dangling_sources,
        }
    }
}

#[allow(clippy::cognitive_complexity)]
#[allow(clippy::too_many_arguments)]
fn determine_changeset_from_previous_resolved_nodes<'a>(
    io: &IoArgs,
    prev_resolved_state: PreviousResolvedState<'a>,
    dbt_state: &DbtState,
    changed_files: HashSet<String>,
    mut unimpacted_files: HashSet<PathBuf>,
    mut impacted_files: HashSet<PathBuf>,
    mut impacted_schema_files: HashSet<PathBuf>,
) -> FsResult<Option<CacheState>> {
    let prev_nodes = prev_resolved_state.nodes;
    let prev_node_statuses = &prev_resolved_state.node_statuses;

    let root_package_path = &dbt_state.root_package().package_root_path;
    let package_lookup = dbt_state
        .packages
        .iter()
        .map(|x| (x.dbt_project.name.clone(), x))
        .collect::<HashMap<_, _>>();

    let mut input_file_lookup = HashMap::new();
    for (_, node) in prev_nodes.iter() {
        let Some(package) = package_lookup.get(&node.common().package_name) else {
            // REVIEW: Should we only continue on extended models?
            continue;
        };

        let package_path = &package.package_root_path;

        let input_files = if let Some(patch_path) = &node.common().patch_path {
            vec![node.common().path.clone(), patch_path.clone()]
        } else {
            vec![node.common().path.clone()]
        };

        let input_files = input_files
            .into_iter()
            .map(|x| {
                let abs_path = package_path.join(x);
                let Ok(rel_path) = abs_path.strip_prefix(root_package_path) else {
                    return PathBuf::default();
                };
                PathBuf::from(rel_path)
            })
            .collect::<Vec<_>>();

        input_file_lookup.insert(node.common().unique_id.to_string(), input_files);
    }

    let prev_dbt_nodes = prev_nodes
        .into_iter()
        .map(|node| (node.1.common().unique_id.to_string(), node.1.clone()))
        .collect::<HashMap<_, _>>();

    let mut changed_nodes = HashSet::new();
    for node in prev_dbt_nodes.values() {
        let Some(input_files) = input_file_lookup.get(&node.common().unique_id) else {
            continue;
        };

        let are_input_files_unchanged = input_files
            .iter()
            .all(|input_file| unimpacted_files.contains(input_file));
        if !are_input_files_unchanged {
            changed_nodes.insert(node.common().unique_id.to_string());
        }
    }

    // Use the shared helper function to filter nodes
    let (_keep, invalidated) = filter_dbt_nodes_by_file_changes(
        io,
        prev_dbt_nodes,
        prev_node_statuses,
        input_file_lookup,
        &mut unimpacted_files,
        &mut impacted_files,
        &mut impacted_schema_files,
    )?;

    let file_changes = FileChanges {
        changed_files: changed_files.into_iter().map(DbtPath::from).collect(),
        unimpacted_files: unimpacted_files.into_iter().map(DbtPath::from).collect(),
        impacted_files: impacted_files.into_iter().map(DbtPath::from).collect(),
        deleted_files: HashSet::default(), // not supported yet
        new_files: HashSet::default(),     // not supported yet
    };

    let mut resolved_nodes = ResolvedNodes {
        nodes: prev_resolved_state.nodes.clone(),
        disabled_nodes: prev_resolved_state.disabled_nodes.clone(),
        macros: prev_resolved_state.macros.clone(),
        operations: prev_resolved_state.operations.clone(),
    };

    let remove_node = |nodes: &mut Nodes, unique_id: &str| {
        nodes.models.remove(unique_id);
        nodes.seeds.remove(unique_id);
        nodes.snapshots.remove(unique_id);
        nodes.tests.remove(unique_id);
        nodes.unit_tests.remove(unique_id);
        nodes.exposures.remove(unique_id);
        nodes.analyses.remove(unique_id);
        nodes.checks.remove(unique_id);
        nodes.metrics.remove(unique_id);
        nodes.semantic_models.remove(unique_id);
        nodes.sources.remove(unique_id);
        nodes.groups.remove(unique_id);
        nodes.saved_queries.remove(unique_id);
    };

    let mut get_relation_calls = prev_resolved_state.get_relation_calls.clone();
    let mut get_columns_in_relation_calls =
        prev_resolved_state.get_columns_in_relation_calls.clone();
    let mut patterned_dangling_sources = prev_resolved_state.patterned_dangling_sources.clone();

    let impacted_nodes = invalidated.keys().cloned().collect();
    for node in invalidated {
        let unique_id = node.0;
        remove_node(&mut resolved_nodes.nodes, &unique_id);
        remove_node(&mut resolved_nodes.disabled_nodes, &unique_id);
        resolved_nodes.macros.macros.remove(&unique_id);
        resolved_nodes.macros.docs_macros.remove(&unique_id);
        get_relation_calls.remove(&unique_id);
        get_columns_in_relation_calls.remove(&unique_id);
        patterned_dangling_sources.remove(&unique_id);
    }

    let nodes_with_changeset = CacheState {
        file_changes,
        unimpacted_resolved_nodes: resolved_nodes,
        unimpacted_node_statuses: HashMap::default(), // Not needed for LSP. For CLI, it is.
        unimpacted_get_relation_calls: get_relation_calls,
        unimpacted_get_columns_in_relation_calls: get_columns_in_relation_calls,
        unimpacted_patterned_dangling_sources: patterned_dangling_sources,
        changed_nodes: Arc::new(changed_nodes),
        impacted_nodes: Arc::new(impacted_nodes),
    };

    // If too many files changed, the incremental cache is more expensive than a full
    // parse. Fall back and tell the user why so the slowdown is diagnosable.
    let impacted_total = nodes_with_changeset.file_changes.impacted_files.len()
        + nodes_with_changeset.file_changes.new_files.len()
        + nodes_with_changeset.file_changes.deleted_files.len();
    if impacted_total > INCREMENTAL_CACHE_FILE_THRESHOLD {
        eprintln!(
            "[partial-parse] {impacted_total} files changed — exceeds incremental threshold \
             ({INCREMENTAL_CACHE_FILE_THRESHOLD}), falling back to full parse"
        );
        return Ok(None);
    }

    Ok(Some(nodes_with_changeset))
}

/// Determines cache state from previous resolved nodes (LSP incremental path).
#[allow(clippy::too_many_arguments)]
pub fn determine_cache_state_from_previous_previous_resolved_nodes<'a>(
    io: &IoArgs,
    prev_resolved_state: PreviousResolvedState<'a>,
    changed_files_list: &[String],
    dbt_state: &DbtState,
) -> FsResult<Option<CacheState>> {
    // Collect all current files from dbt_state
    let mut all_current_files: HashSet<PathBuf> = HashSet::new();
    for package in &dbt_state.packages {
        let package_root_path = &package.package_root_path;
        for paths in package.all_paths.values() {
            for (path, _) in paths {
                let full_path = if path.has_root() {
                    path.as_path().to_path_buf()
                } else {
                    package_root_path.join(path.as_path())
                };

                // Get path relative to io.in_dir
                let relative_path = if let Ok(rel_path) = full_path.strip_prefix(&io.in_dir) {
                    PathBuf::from(rel_path)
                } else {
                    full_path
                };
                all_current_files.insert(relative_path);
            }
        }
    }

    // Partition files into changed and unchanged
    let changed_files: HashSet<PathBuf> = changed_files_list.iter().map(PathBuf::from).collect();
    let unchanged_files: HashSet<PathBuf> = all_current_files
        .difference(&changed_files)
        .cloned()
        .collect();

    // Identify changed schema and seed files
    let changed_schema_files: HashSet<PathBuf> = changed_files
        .iter()
        .filter(|path| {
            is_yml_file_path(path)
                && !path.ends_with(DBT_PROFILES_YML)
                && !path.ends_with(DBT_PROJECT_YML)
                && !path.ends_with(DBT_DEPENDENCIES_YML)
                && !path.ends_with(DBT_PACKAGES_YML)
                && !path.ends_with(DBT_PACKAGES_LOCK_FILE)
                && !path.ends_with(DBT_SELECTORS_YML)
        })
        .cloned()
        .collect();

    determine_changeset_from_previous_resolved_nodes(
        io,
        prev_resolved_state,
        dbt_state,
        changed_files_list.iter().cloned().collect(),
        unchanged_files,
        changed_files,
        changed_schema_files,
    )
}
