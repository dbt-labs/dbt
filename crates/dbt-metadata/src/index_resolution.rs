//! Index-based selector resolution for parse cache.
//!
//! Evaluates selector expressions (FQN, tag, package, resource type) and
//! `state:dirty` against the parquet parse cache **without** loading node
//! payloads. Uses column projection to read only index columns from disk.
//!
//! ## Key invariants
//! * Always includes all transitive ancestors — the scheduler requires
//!   `all_deps_present()`.
//! * Always includes all macros — needed for Jinja rendering.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use parquet::arrow::{ProjectionMask, arrow_reader::ParquetRecordBatchReaderBuilder};

use dbt_common::path::resource_extension;
use dbt_schemas::state::ResourcePathKind;

use crate::parse_state::{
    NodeIndexRow, ResolverStateRow, cache_dir, existing_epoch_paths, load_packages_from_filestamps,
    resolver_state_path, system_time_to_nanos,
};
use crate::partial_parse::PackageSnapshot;

// ── index reading (projection-based) ─────────────────────────────────────────

const INDEX_COLUMNS: &[&str] = &[
    "unique_id",
    "original_path",
    "resource_type",
    "name",
    "package_name",
    "fqn",
    "tags",
    "depends_on",
    "materialization",
];

fn read_index_rows_from_file(path: &Path) -> Vec<NodeIndexRow> {
    let Ok(file) = fs::File::open(path) else {
        return vec![];
    };
    let Ok(builder) = ParquetRecordBatchReaderBuilder::try_new(file) else {
        return vec![];
    };

    // Use roots-based projection so list columns (fqn, tags, depends_on) are
    // selected by their top-level field index rather than by leaf column name,
    // which would miss nested list item fields.
    let arrow_schema = builder.schema();
    let col_indices: Vec<usize> = INDEX_COLUMNS
        .iter()
        .filter_map(|name| arrow_schema.index_of(name).ok())
        .collect();
    let schema_desc = builder.parquet_schema();
    let mask = ProjectionMask::roots(schema_desc, col_indices);

    let Ok(reader) = builder.with_projection(mask).build() else {
        return vec![];
    };
    let mut out = Vec::new();
    for batch in reader {
        let Ok(batch) = batch else { return vec![] };
        match serde_arrow::from_record_batch::<Vec<NodeIndexRow>>(&batch) {
            Ok(rows) => out.extend(rows),
            Err(_) => return vec![],
        }
    }
    out
}

fn read_node_index_rows(dir: &Path) -> Vec<NodeIndexRow> {
    let epochs = existing_epoch_paths(dir);
    if epochs.is_empty() {
        return vec![];
    }
    if epochs.len() == 1 {
        return read_index_rows_from_file(&epochs[0].1);
    }
    let mut by_id: HashMap<String, NodeIndexRow> = HashMap::new();
    for (_, path) in &epochs {
        for row in read_index_rows_from_file(path) {
            by_id.insert(row.unique_id.clone(), row);
        }
    }
    by_id.into_values().collect()
}

// ── public API ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexLookupMethod {
    Fqn,
    Tag,
    Package,
    ResourceType,
    /// Component-boundary prefix match on `original_path` (e.g. `"models/staging"`).
    Path,
    /// Exact suffix match on `original_path` for `file:` selectors (e.g. `"stg_orders.sql"`).
    File,
    /// `source:source_name.table_name` or `source:source_name` selector.
    Source,
}

#[allow(clippy::cognitive_complexity)]
pub fn resolve_unique_ids_from_index(
    out_dir: &Path,
    method: IndexLookupMethod,
    value: &str,
    parents_depth: Option<u32>,
    children_depth: Option<u32>,
    include_indirect: bool,
) -> Option<HashSet<String>> {
    if value.contains(['*', '?', '[', ']']) {
        return None;
    }
    let dir = cache_dir(out_dir);
    if !dir.exists() {
        return None;
    }

    let all_rows = read_node_index_rows(&dir);
    if all_rows.is_empty() {
        return None;
    }

    let seed_ids: HashSet<String> = all_rows
        .iter()
        .filter(|row| match method {
            IndexLookupMethod::Fqn => {
                row.name == value || row.unique_id == value || fqn_contains(&row.fqn, value)
            }
            IndexLookupMethod::Tag => tags_contains(&row.tags, value),
            IndexLookupMethod::Package => row.package_name == value,
            IndexLookupMethod::ResourceType => row.resource_type == value,
            IndexLookupMethod::Path => path_starts_with(&row.original_path, value),
            IndexLookupMethod::File => path_ends_with_file(&row.original_path, value),
            IndexLookupMethod::Source => {
                if row.resource_type != "source" {
                    return false;
                }
                if let Some(dot) = value.find('.') {
                    let source_part = &value[..dot];
                    let table_part = &value[dot + 1..];
                    row.package_name == source_part && row.name == table_part
                } else {
                    row.package_name == value
                }
            }
        })
        .map(|row| row.unique_id.clone())
        .collect();

    // Empty seed set means the selector matched nothing in the cached index. This can
    // happen when the index is stale (e.g. a brand-new model file that hasn't been
    // parsed yet). Return None to fall back to a full load rather than silently
    // returning an empty result.
    if seed_ids.is_empty() {
        return None;
    }

    let (parents_of, children_of) =
        build_edge_maps(&all_rows, children_depth.is_some() || include_indirect);

    let mut result: HashSet<String> = seed_ids.clone();
    walk_parents(&seed_ids, parents_depth, &parents_of, &mut result);
    walk_children(&seed_ids, children_depth, &children_of, &mut result);
    add_all_ancestors(&parents_of, &mut result);
    add_indirect_tests(include_indirect, &children_of, &mut result);
    add_all_macros(&parents_of, &mut result);

    Some(result)
}

/// Return only the directly-touched unique_ids (no graph expansion).
///
/// Used to synthesize a `SelectExpression` for `--dirty` so the scheduler applies
/// `seed+` semantics rather than running the pre-expanded ancestor closure.
/// Returns `None` when the cache directory does not exist.
pub fn dirty_seed_ids_from_index(out_dir: &Path) -> Option<HashSet<String>> {
    let dir = cache_dir(out_dir);
    if !dir.exists() {
        return None;
    }
    let rs_rows: Vec<ResolverStateRow> =
        dbt_metadata_parquet::epoch_io::read_rows(&resolver_state_path(&dir));
    let rs = rs_rows.into_iter().next().unwrap_or_default();
    let packages = load_packages_from_filestamps(
        &dir,
        &rs.pkg_deps_json,
        &rs.pkg_kinds_json,
        &rs.pkg_manifest_path_configs_json,
    )?;
    let touched = detect_dirty_files(&packages);
    let all_rows = read_node_index_rows(&dir);
    let seed_ids = all_rows
        .iter()
        .filter(|row| touched.contains(&(row.package_name.clone(), row.original_path.clone())))
        .map(|row| row.unique_id.clone())
        .collect();
    Some(seed_ids)
}

/// Resolve unique_ids for `state:dirty` — nodes whose source file mtime changed.
///
/// Returns `Some(ids)` when cache exists (empty if nothing dirty).
/// Returns `None` when no cache directory exists.
#[allow(clippy::cognitive_complexity)]
pub fn resolve_dirty_unique_ids_from_index(
    out_dir: &Path,
    parents_depth: Option<u32>,
    children_depth: Option<u32>,
    include_indirect: bool,
) -> Option<HashSet<String>> {
    let dir = cache_dir(out_dir);
    if !dir.exists() {
        return None;
    }

    let rs_rows: Vec<ResolverStateRow> =
        dbt_metadata_parquet::epoch_io::read_rows(&resolver_state_path(&dir));
    let rs = rs_rows.into_iter().next().unwrap_or_default();
    let packages = load_packages_from_filestamps(
        &dir,
        &rs.pkg_deps_json,
        &rs.pkg_kinds_json,
        &rs.pkg_manifest_path_configs_json,
    )?;
    let touched = detect_dirty_files(&packages);

    if touched.is_empty() {
        let all_rows = read_node_index_rows(&dir);
        let mut result = HashSet::new();
        for row in all_rows
            .iter()
            .filter(|r| r.unique_id.starts_with("macro."))
        {
            result.insert(row.unique_id.clone());
        }
        return Some(result);
    }

    let all_rows = read_node_index_rows(&dir);
    if all_rows.is_empty() {
        return None;
    }

    let seed_ids: HashSet<String> = all_rows
        .iter()
        .filter(|row| touched.contains(&(row.package_name.clone(), row.original_path.clone())))
        .map(|row| row.unique_id.clone())
        .collect();

    let (parents_of, children_of) =
        build_edge_maps(&all_rows, children_depth.is_some() || include_indirect);

    let mut result: HashSet<String> = seed_ids.clone();
    walk_parents(&seed_ids, parents_depth, &parents_of, &mut result);
    walk_children(&seed_ids, children_depth, &children_of, &mut result);
    add_all_ancestors(&parents_of, &mut result);
    add_indirect_tests(include_indirect, &children_of, &mut result);
    add_all_macros(&parents_of, &mut result);

    Some(result)
}

// ── graph walk helpers ───────────────────────────────────────────────────────

fn build_edge_maps(
    rows: &[NodeIndexRow],
    need_children: bool,
) -> (HashMap<String, Vec<String>>, HashMap<String, Vec<String>>) {
    let mut parents_of: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        parents_of.insert(row.unique_id.clone(), row.depends_on.clone());
    }

    let mut children_of: HashMap<String, Vec<String>> = HashMap::new();
    if need_children {
        for (uid, deps) in &parents_of {
            for dep in deps {
                children_of
                    .entry(dep.clone())
                    .or_default()
                    .push(uid.clone());
            }
        }
    }
    (parents_of, children_of)
}

fn walk_parents(
    seeds: &HashSet<String>,
    max_depth: Option<u32>,
    parents_of: &HashMap<String, Vec<String>>,
    result: &mut HashSet<String>,
) {
    let Some(max_depth) = max_depth else { return };
    let mut frontier: Vec<String> = seeds.iter().cloned().collect();
    let mut depth = 0u32;
    while !frontier.is_empty() && depth < max_depth {
        let mut next = Vec::new();
        for uid in &frontier {
            if let Some(deps) = parents_of.get(uid.as_str()) {
                for dep in deps {
                    if result.insert(dep.clone()) {
                        next.push(dep.clone());
                    }
                }
            }
        }
        frontier = next;
        depth += 1;
    }
}

fn walk_children(
    seeds: &HashSet<String>,
    max_depth: Option<u32>,
    children_of: &HashMap<String, Vec<String>>,
    result: &mut HashSet<String>,
) {
    let Some(max_depth) = max_depth else { return };
    let mut frontier: Vec<String> = seeds.iter().cloned().collect();
    let mut depth = 0u32;
    while !frontier.is_empty() && depth < max_depth {
        let mut next = Vec::new();
        for uid in &frontier {
            if let Some(kids) = children_of.get(uid.as_str()) {
                for kid in kids {
                    if result.insert(kid.clone()) {
                        next.push(kid.clone());
                    }
                }
            }
        }
        frontier = next;
        depth += 1;
    }
}

fn add_indirect_tests(
    include_indirect: bool,
    children_of: &HashMap<String, Vec<String>>,
    result: &mut HashSet<String>,
) {
    if !include_indirect {
        return;
    }
    let primary: Vec<String> = result.iter().cloned().collect();
    for uid in &primary {
        if let Some(kids) = children_of.get(uid.as_str()) {
            for kid in kids {
                if kid.starts_with("test.") || kid.starts_with("unit_test.") {
                    result.insert(kid.clone());
                }
            }
        }
    }
}

fn add_all_ancestors(parents_of: &HashMap<String, Vec<String>>, result: &mut HashSet<String>) {
    let mut queue: Vec<String> = result.iter().cloned().collect();
    while let Some(uid) = queue.pop() {
        if let Some(deps) = parents_of.get(uid.as_str()) {
            for dep in deps {
                if result.insert(dep.clone()) {
                    queue.push(dep.clone());
                }
            }
        }
    }
}

fn add_all_macros(parents_of: &HashMap<String, Vec<String>>, result: &mut HashSet<String>) {
    for uid in parents_of.keys().filter(|uid| uid.starts_with("macro.")) {
        result.insert(uid.clone());
    }
}

fn detect_dirty_files(packages: &[PackageSnapshot]) -> HashSet<(String, String)> {
    let mut touched = HashSet::new();
    for pkg in packages {
        let root = Path::new(&pkg.package_root_path);
        for (kind, files) in &pkg.all_paths {
            let safe = matches!(
                kind,
                ResourcePathKind::ModelPaths | ResourcePathKind::AnalysisPaths
            );
            if !safe {
                continue;
            }
            for (rel_path, saved_nanos) in files {
                let is_sql = resource_extension(Path::new(rel_path))
                    .is_some_and(|e| e.eq_ignore_ascii_case("sql"));
                if !is_sql {
                    continue;
                }
                let current_nanos = fs::metadata(root.join(rel_path))
                    .and_then(|m| m.modified())
                    .map(system_time_to_nanos)
                    .unwrap_or(0);
                if current_nanos != *saved_nanos {
                    touched.insert((pkg.package_name.clone(), rel_path.clone()));
                }
            }
        }
    }
    touched
}

fn fqn_contains(fqn: &[String], value: &str) -> bool {
    fqn.iter().any(|p| p == value)
}

fn tags_contains(tags: &[String], value: &str) -> bool {
    tags.iter().any(|p| p == value)
}

/// Component-boundary prefix match: `"models/staging/orders.sql"` matches `"models/staging"`.
fn path_starts_with(original_path: &str, prefix: &str) -> bool {
    Path::new(original_path).starts_with(Path::new(prefix))
}

/// Suffix/filename match: `"models/staging/stg_orders.sql"` matches `"stg_orders.sql"`.
/// Uses `Path::ends_with` for component-boundary correctness.
fn path_ends_with_file(original_path: &str, suffix: &str) -> bool {
    Path::new(original_path).ends_with(Path::new(suffix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_state::NodeIndexRow;

    fn row(uid: &str, deps: &[&str]) -> NodeIndexRow {
        NodeIndexRow {
            unique_id: uid.to_string(),
            original_path: format!("models/{uid}.sql"),
            resource_type: "model".to_string(),
            name: uid.to_string(),
            package_name: "pkg".to_string(),
            fqn: vec!["pkg".to_string(), uid.to_string()],
            tags: vec![],
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
            materialization: "table".to_string(),
        }
    }

    // ── build_edge_maps ──────────────────────────────────────────────────────

    #[test]
    fn build_edge_maps_parents_only() {
        let rows = vec![row("a", &[]), row("b", &["a"]), row("c", &["b"])];
        let (parents, children) = build_edge_maps(&rows, false);
        assert_eq!(parents["b"], vec!["a"]);
        assert_eq!(parents["c"], vec!["b"]);
        assert!(children.is_empty());
    }

    #[test]
    fn build_edge_maps_with_children() {
        let rows = vec![row("a", &[]), row("b", &["a"]), row("c", &["a", "b"])];
        let (_, children) = build_edge_maps(&rows, true);
        assert!(children["a"].contains(&"b".to_string()));
        assert!(children["a"].contains(&"c".to_string()));
        assert!(children["b"].contains(&"c".to_string()));
    }

    // ── walk_parents ─────────────────────────────────────────────────────────

    #[test]
    fn walk_parents_none_depth_is_noop() {
        let parents = HashMap::from([("b".to_string(), vec!["a".to_string()])]);
        let seeds: HashSet<String> = ["b".to_string()].into();
        let mut result: HashSet<String> = seeds.clone();
        walk_parents(&seeds, None, &parents, &mut result);
        assert_eq!(result, seeds);
    }

    #[test]
    fn walk_parents_depth_1() {
        let parents = HashMap::from([
            ("c".to_string(), vec!["b".to_string()]),
            ("b".to_string(), vec!["a".to_string()]),
            ("a".to_string(), vec![]),
        ]);
        let seeds: HashSet<String> = ["c".to_string()].into();
        let mut result: HashSet<String> = seeds.clone();
        walk_parents(&seeds, Some(1), &parents, &mut result);
        assert!(result.contains("b"));
        assert!(!result.contains("a"));
    }

    #[test]
    fn walk_parents_depth_2_reaches_grandparent() {
        let parents = HashMap::from([
            ("c".to_string(), vec!["b".to_string()]),
            ("b".to_string(), vec!["a".to_string()]),
            ("a".to_string(), vec![]),
        ]);
        let seeds: HashSet<String> = ["c".to_string()].into();
        let mut result: HashSet<String> = seeds.clone();
        walk_parents(&seeds, Some(2), &parents, &mut result);
        assert!(result.contains("b"));
        assert!(result.contains("a"));
    }

    // ── walk_children ────────────────────────────────────────────────────────

    #[test]
    fn walk_children_depth_1() {
        let children = HashMap::from([
            ("a".to_string(), vec!["b".to_string()]),
            ("b".to_string(), vec!["c".to_string()]),
        ]);
        let seeds: HashSet<String> = ["a".to_string()].into();
        let mut result: HashSet<String> = seeds.clone();
        walk_children(&seeds, Some(1), &children, &mut result);
        assert!(result.contains("b"));
        assert!(!result.contains("c"));
    }

    // ── add_indirect_tests ───────────────────────────────────────────────────

    #[test]
    fn indirect_tests_adds_test_children() {
        let children = HashMap::from([(
            "model.pkg.a".to_string(),
            vec!["test.pkg.not_null_a".to_string(), "model.pkg.b".to_string()],
        )]);
        let mut result: HashSet<String> = ["model.pkg.a".to_string()].into();
        add_indirect_tests(true, &children, &mut result);
        assert!(result.contains("test.pkg.not_null_a"));
        assert!(!result.contains("model.pkg.b"));
    }

    #[test]
    fn indirect_tests_disabled_is_noop() {
        let children = HashMap::from([("model.pkg.a".to_string(), vec!["test.pkg.t".to_string()])]);
        let mut result: HashSet<String> = ["model.pkg.a".to_string()].into();
        add_indirect_tests(false, &children, &mut result);
        assert!(!result.contains("test.pkg.t"));
    }

    // ── add_all_ancestors ────────────────────────────────────────────────────

    #[test]
    fn all_ancestors_transitive_closure() {
        let parents = HashMap::from([
            ("d".to_string(), vec!["c".to_string()]),
            ("c".to_string(), vec!["b".to_string()]),
            ("b".to_string(), vec!["a".to_string()]),
            ("a".to_string(), vec![]),
        ]);
        let mut result: HashSet<String> = ["d".to_string()].into();
        add_all_ancestors(&parents, &mut result);
        assert_eq!(
            result,
            ["d", "c", "b", "a"].into_iter().map(String::from).collect()
        );
    }

    // ── add_all_macros ───────────────────────────────────────────────────────

    #[test]
    fn all_macros_included() {
        let parents = HashMap::from([
            ("model.pkg.a".to_string(), vec![]),
            ("macro.pkg.my_macro".to_string(), vec![]),
            ("macro.dbt.run_query".to_string(), vec![]),
        ]);
        let mut result: HashSet<String> = HashSet::new();
        add_all_macros(&parents, &mut result);
        assert!(result.contains("macro.pkg.my_macro"));
        assert!(result.contains("macro.dbt.run_query"));
        assert!(!result.contains("model.pkg.a"));
    }

    // ── fqn/tag helpers ──────────────────────────────────────────────────────

    #[test]
    fn fqn_contains_matches() {
        let fqn = vec!["pkg".to_string(), "models".to_string(), "users".to_string()];
        assert!(fqn_contains(&fqn, "users"));
        assert!(fqn_contains(&fqn, "pkg"));
        assert!(!fqn_contains(&fqn, "orders"));
    }

    #[test]
    fn tags_contains_matches() {
        let tags = vec!["daily".to_string(), "critical".to_string()];
        assert!(tags_contains(&tags, "daily"));
        assert!(!tags_contains(&tags, "weekly"));
        assert!(!tags_contains(&[], "daily"));
    }

    // ── path_starts_with / path_ends_with_file ───────────────────────────────

    #[test]
    fn path_starts_with_prefix_match() {
        assert!(path_starts_with(
            "models/staging/orders.sql",
            "models/staging"
        ));
        assert!(path_starts_with(
            "models/staging/orders.sql",
            "models/staging/orders.sql"
        ));
        // Component boundary: "models/stag" does NOT match "models/staging/..."
        assert!(!path_starts_with(
            "models/staging/orders.sql",
            "models/stag"
        ));
        assert!(!path_starts_with(
            "models/staging/orders.sql",
            "models/marts"
        ));
    }

    #[test]
    fn path_ends_with_file_match() {
        assert!(path_ends_with_file(
            "models/staging/orders.sql",
            "orders.sql"
        ));
        assert!(path_ends_with_file(
            "models/staging/orders.sql",
            "staging/orders.sql"
        ));
        // Component boundary: "rders.sql" does NOT match
        assert!(!path_ends_with_file(
            "models/staging/orders.sql",
            "rders.sql"
        ));
        assert!(!path_ends_with_file(
            "models/staging/orders.sql",
            "payments.sql"
        ));
    }
}
