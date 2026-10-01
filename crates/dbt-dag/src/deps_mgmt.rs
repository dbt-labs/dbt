use std::{
    collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque},
    hash::Hash,
};

use itertools::Itertools;
pub fn reverse<T, U>(dependencies: &BTreeMap<T, BTreeSet<U>>) -> BTreeMap<U, BTreeSet<T>>
where
    T: Hash + PartialEq + Eq + Clone + Ord,
    U: Hash + PartialEq + Eq + Clone + Ord,
{
    let mut reverted: BTreeMap<U, BTreeSet<T>> = BTreeMap::new();
    reverse_dependencies(dependencies, &mut reverted);
    reverted
}

pub fn reverse_dependencies<T, U>(
    dependencies: &BTreeMap<T, BTreeSet<U>>,
    reverted: &mut BTreeMap<U, BTreeSet<T>>,
) where
    T: Hash + PartialEq + Eq + Clone + Ord,
    U: Hash + PartialEq + Eq + Clone + Ord,
{
    for (k, vs) in dependencies {
        for v in vs {
            if reverted.contains_key(v) {
                let mut inc = reverted.get(v).unwrap().to_owned();
                inc.insert(k.to_owned());
                reverted.insert(v.to_owned(), inc.to_owned());
            } else {
                let mut inc = BTreeSet::new();
                inc.insert(k.to_owned());
                reverted.insert(v.to_owned(), inc);
            }
        }
    }
}
/// Returns deps where all deps are defined
pub fn ensure_all_nodes_defined<T>(
    dependencies: &BTreeMap<T, BTreeSet<T>>,
) -> BTreeMap<T, BTreeSet<T>>
where
    T: Hash + PartialEq + Eq + Clone + Ord,
{
    let mut nodes = BTreeSet::new();
    for vs in dependencies.values() {
        for v in vs {
            nodes.insert(v.to_owned());
        }
    }
    let keys: BTreeSet<T> = dependencies.keys().cloned().collect::<BTreeSet<T>>();
    let diff = nodes.difference(&keys);
    let mut res = dependencies.clone();
    for d in diff {
        res.insert(d.to_owned(), BTreeSet::new());
    }
    res
}

/// Performs a breadth-first search on a graph represented by `deps`, starting from the nodes in `sinks`.
/// Returns a `HashMap` containing only the nodes that were reached and their corresponding edges.
pub fn slice<T, U>(deps: &BTreeMap<T, BTreeSet<U>>, sinks: &BTreeSet<T>) -> BTreeMap<T, BTreeSet<U>>
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<U>,
    U: Hash + PartialEq + Eq + Clone + Ord + Into<T>,
{
    let mut included = BTreeSet::new();
    let queue = &mut VecDeque::new();

    for sink in sinks {
        queue.push_back(sink.to_owned());
    }
    while !queue.is_empty() {
        let element = queue.pop_front().unwrap();
        if !included.contains(&element) {
            included.insert(element.to_owned());
            if let Some(next_elements) = deps.get(&element) {
                for next_elem in next_elements {
                    queue.push_back(next_elem.to_owned().into());
                }
            }
        }
    }
    let mut res = BTreeMap::new();
    for (k, v) in deps {
        if included.contains(k) {
            res.insert(k.to_owned(), v.to_owned());
        };
    }
    res
}

pub fn slice_with_compare<T, U, F>(
    deps: &BTreeMap<T, BTreeSet<U>>,
    sinks: &BTreeSet<T>,
    compare_fn: F,
) -> BTreeMap<T, BTreeSet<U>>
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<U>,
    U: Hash + PartialEq + Eq + Clone + Ord + Into<T>,
    F: Fn(&T, &T) -> bool,
{
    let mut included = BTreeSet::new();
    let mut queue = VecDeque::new();

    // Add sinks to the queue
    for sink in sinks {
        queue.push_back(sink.to_owned());
    }

    while let Some(element) = queue.pop_front() {
        // Check if the element is already included
        // todo: seems inefficient...
        if !included.iter().any(|e| compare_fn(e, &element)) {
            included.insert(element.clone());

            // Add neighbors to the queue
            if let Some(next_elements) = deps.get(&element) {
                for next_elem in next_elements {
                    queue.push_back(next_elem.clone().into());
                }
            }
        }
    }

    // Filter the dependencies to include only the nodes in `included`
    let mut res = BTreeMap::new();
    for (k, v) in deps {
        if included.iter().any(|e| compare_fn(e, k)) {
            res.insert(k.clone(), v.clone());
        }
    }
    res
}

pub fn get_all_upstream_deps<T, U>(
    deps: &BTreeMap<T, BTreeSet<U>>,
    subset: &BTreeSet<T>,
) -> BTreeMap<T, BTreeSet<U>>
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<U>,
    U: Hash + PartialEq + Eq + Clone + Ord + Into<T>,
{
    let mut result = BTreeMap::new();
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::new();

    // Initialize queue with subset nodes
    for node in subset {
        queue.push_back(node.clone());
    }

    // BFS to collect all upstream dependencies
    while let Some(node) = queue.pop_front() {
        if visited.contains(&node) {
            continue;
        }

        visited.insert(node.clone());

        // Add this node and its immediate dependencies to the result
        if let Some(neighbors) = deps.get(&node) {
            result.insert(node.clone(), neighbors.clone());

            // Add all dependencies to the queue for processing
            for neighbor in neighbors {
                queue.push_back(neighbor.clone().into());
            }
        } else {
            // Handle nodes with no dependencies
            result.insert(node.clone(), BTreeSet::new());
        }
    }

    result
}

// Helper function to find all transitive upstream dependencies for a given node
pub fn find_all_upstream_deps(
    node_id: &str,
    deps: &BTreeMap<String, BTreeSet<String>>,
) -> HashSet<String> {
    let mut all_deps = HashSet::new();
    let mut stack = Vec::new();

    if let Some(direct_deps) = deps.get(node_id) {
        for dep in direct_deps {
            stack.push(dep.clone());
        }
    }

    while let Some(current) = stack.pop() {
        if all_deps.insert(current.clone()) {
            // This is a new dependency, explore its dependencies too
            if let Some(deps) = deps.get(&current) {
                for dep in deps {
                    stack.push(dep.clone());
                }
            }
        }
    }

    all_deps
}

/// Build a restricted subgraph for the included nodes and restore any transitive
/// dependencies that were lost due to filtered intermediate nodes.
///
/// For the SavedQuery -> Metric -> SemanticModel -> Model dependency, SemanticModel nodes
/// are removed since they're neither selected nor frontier nodes, but we want to make sure
/// that we still capture the SavedQuery -> Model dependencies.
pub fn restrict_with_transitive<T, U>(
    deps: &BTreeMap<T, BTreeSet<U>>,
    subset: &BTreeSet<T>,
) -> BTreeMap<T, BTreeSet<U>>
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<U>,
    U: Hash + PartialEq + Eq + Clone + Ord + Into<T>,
{
    // Compute the nearest upstream ancestors that are also included
    fn first_upstream_included<T, U>(
        node: &T,
        full_deps: &BTreeMap<T, BTreeSet<U>>,
        subset: &BTreeSet<T>,
        memoized: &mut BTreeMap<T, BTreeSet<U>>,
        visiting: &mut BTreeSet<T>,
    ) -> BTreeSet<U>
    where
        T: Hash + PartialEq + Eq + Clone + Ord + Into<U>,
        U: Hash + PartialEq + Eq + Clone + Ord + Into<T>,
    {
        if let Some(cached) = memoized.get(node) {
            return cached.clone();
        }
        // Guard against cycles
        if !visiting.insert(node.clone()) {
            return BTreeSet::new();
        }

        let mut result = BTreeSet::new();
        if let Some(parents) = full_deps.get(node) {
            for parent in parents {
                let parent_t: T = parent.clone().into();
                if subset.contains(&parent_t) {
                    // Direct parent is included -> keep it
                    result.insert(parent.clone());
                } else {
                    // Parent was filtered out -> climb upstream to the first included ancestors
                    let upstream =
                        first_upstream_included(&parent_t, full_deps, subset, memoized, visiting);
                    result.extend(upstream);
                }
            }
        }

        visiting.remove(node);
        memoized.insert(node.clone(), result.clone());
        result
    }

    let mut memoized = BTreeMap::new();
    let mut out = BTreeMap::new();
    for node in subset.iter() {
        let parents =
            first_upstream_included(node, deps, subset, &mut memoized, &mut BTreeSet::new());
        out.insert(node.clone(), parents);
    }
    out
}

pub fn sinks<T, U>(
    reverse_deps: &BTreeMap<U, BTreeSet<T>>,
    from: BTreeSet<T>,
    all: bool,
) -> BTreeSet<T>
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<U>,
    U: Hash + PartialEq + Eq + Clone + Ord + Into<T>,
{
    let mut visited = BTreeSet::new();
    let mut sinks = BTreeSet::new();
    let queue = &mut VecDeque::new();
    for elem in from {
        queue.push_back(elem.to_owned());
        visited.insert(elem);
    }
    while !queue.is_empty() {
        let elem = queue.pop_front().unwrap();
        if let Some(successors) = reverse_deps.get(&elem.to_owned().into()) {
            if successors.is_empty() {
                visited.insert(elem.to_owned());
                sinks.insert(elem.to_owned());
            } else {
                for successor in successors {
                    if visited.contains(successor) {
                        continue;
                    } else {
                        visited.insert(successor.to_owned());
                        queue.push_back(successor.to_owned());
                    }
                }
            }
        } else {
            visited.insert(elem.to_owned());
            sinks.insert(elem.to_owned());
        }
    }
    if !all { sinks } else { visited }
}

pub fn get_sources<T, U>(deps: &BTreeMap<T, BTreeSet<U>>) -> BTreeSet<U>
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<U>,
    U: Hash + PartialEq + Eq + Clone + Ord + Into<T>,
{
    let mut sources = BTreeSet::new();

    for (k, vs) in deps {
        if vs.is_empty() {
            sources.insert(k.to_owned().into());
        } else {
            for v in vs {
                let t = v.clone().into();
                if !deps.contains_key(&t) {
                    sources.insert(v.clone());
                }
            }
        }
    }

    sources
}

pub fn get_upstreams<T, U>(deps: &HashMap<T, HashSet<U>>, targets: &HashSet<T>) -> HashSet<T>
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<U>,
    U: Hash + PartialEq + Eq + Clone + Ord + Into<T>,
{
    let mut upstreams = HashSet::new();
    let mut visited = HashSet::new();
    let mut stack = VecDeque::new();

    for target in targets {
        if let Some(neighbors) = deps.get(target) {
            for neighbor in neighbors {
                let neighbor_t: T = neighbor.clone().into();
                stack.push_back(neighbor_t);
            }
        }
    }

    while let Some(node) = stack.pop_back() {
        if targets.contains(&node) {
            continue;
        }

        if visited.contains(&node) {
            continue;
        }

        visited.insert(node.clone());
        upstreams.insert(node.clone());

        if let Some(neighbors) = deps.get(&node) {
            for neighbor in neighbors {
                let neighbor_t: T = neighbor.clone().into();
                stack.push_back(neighbor_t);
            }
        }
    }

    upstreams
}

pub fn get_sinks<T, U>(
    deps: &HashMap<T, HashSet<U>>,
    reverse_deps: &HashMap<U, HashSet<T>>,
    sinks: &mut HashSet<T>,
) where
    T: Hash + PartialEq + Eq + Clone + Into<U>,
    U: Hash + PartialEq + Eq + Clone + Into<T>,
{
    for vs in reverse_deps.values() {
        for v in vs {
            if !reverse_deps.contains_key(&v.to_owned().into()) {
                sinks.insert(v.to_owned());
            }
        }
    }
    if sinks.is_empty()
        && let Some(k) = deps.keys().next()
    {
        sinks.insert(k.to_owned());
    }
}

// Takes a dependency (forward-only) DAG and a cut-point predicate and returns:
// 1. list of all cycles
// 2. list of cycle-cut-points options corresponding to each of the cycles in #1
// 3. new dependency DAG in which the cycles have been cut along the cut points
// This method runs a fixed point loop until there are no more cycles found
#[allow(clippy::type_complexity)]
pub fn find_and_cut_cycles<T, U>(
    deps: &BTreeMap<T, BTreeSet<U>>,
    cut_point_predicate: impl Fn(&T) -> bool,
) -> (Vec<Vec<T>>, Vec<Option<T>>, BTreeMap<T, BTreeSet<U>>)
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<U> + std::fmt::Debug,
    U: Hash + PartialEq + Eq + Clone + Ord + Into<T> + std::fmt::Debug,
{
    fn dfs<T, U>(
        node: &T,
        graph: &BTreeMap<T, BTreeSet<U>>,
        stack: &mut VecDeque<T>,
        visited: &mut HashSet<T>,
        stack_set: &mut HashSet<T>,
        cycles: &mut Vec<Vec<T>>,
    ) where
        T: Hash + PartialEq + Eq + Clone + Ord + Into<U> + std::fmt::Debug,
        U: Hash + PartialEq + Eq + Clone + Ord + Into<T> + std::fmt::Debug,
    {
        if stack_set.contains(node) {
            // Found a cycle
            let cycle_start_index = stack.iter().position(|n| n == node).unwrap();
            let cycle = stack.iter().skip(cycle_start_index).cloned().collect();
            cycles.push(cycle);
            return;
        }

        if visited.contains(node) {
            return;
        }

        visited.insert(node.clone());
        stack.push_back(node.clone());
        stack_set.insert(node.clone());

        if let Some(neighbors) = graph.get(node) {
            for neighbor in neighbors.iter().sorted() {
                let neighbor_t = neighbor.to_owned().into();
                dfs(&neighbor_t, graph, stack, visited, stack_set, cycles);
            }
        }

        stack.pop_back();
        stack_set.remove(node);
    }

    let mut cycles = Vec::new();
    let mut all_cycles_cut_deps = deps.clone();
    let mut res_deps = deps.clone();
    let mut res_cut_points = vec![];
    let mut visited;
    let mut stack;
    let mut stack_set;
    let mut cycle_cnt = 0;

    loop {
        visited = HashSet::new();
        stack = VecDeque::new();
        stack_set = HashSet::new();

        for node in all_cycles_cut_deps.keys().sorted() {
            if !visited.contains(node) {
                dfs(
                    node,
                    &all_cycles_cut_deps,
                    &mut stack,
                    &mut visited,
                    &mut stack_set,
                    &mut cycles,
                );
            }
        }

        if cycle_cnt == cycles.len() {
            break;
        }

        let mut cut_points = Vec::new();

        // Determine cut points
        for cycle in &cycles[cycle_cnt..] {
            let mut cut_point = None;
            for node in cycle {
                if cut_point_predicate(node) {
                    cut_point = Some(node.clone());
                    break;
                }
            }
            cut_points.push(cut_point);
        }

        res_cut_points.extend(cut_points.iter().cloned());

        // Remove the links based on cut points
        let x = &cycles[cycle_cnt..];
        for (cycle, cut_point_opt) in x.iter().zip(cut_points.iter()) {
            if let Some(cut_point) = cut_point_opt {
                prune_cycle(cycle, cut_point, &mut res_deps);
                prune_cycle(cycle, cut_point, &mut all_cycles_cut_deps);
            } else {
                let cut_point = &cycle[0];
                prune_cycle(cycle, cut_point, &mut all_cycles_cut_deps);
            }
        }

        cycle_cnt = cycles.len();
    }

    (cycles, res_cut_points, res_deps)
}

fn prune_cycle<T, U>(cycle: &Vec<T>, cut_point: &T, deps: &mut BTreeMap<T, BTreeSet<U>>)
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<U> + std::fmt::Debug,
    U: Hash + PartialEq + Eq + Clone + Ord + Into<T> + std::fmt::Debug,
{
    for node in cycle {
        if let Some(neighbors) = deps.get_mut(node) {
            neighbors.remove(&cut_point.clone().into());
        }
    }
}

pub fn show_deps<T: std::fmt::Display + Ord>(deps: &BTreeMap<T, BTreeSet<T>>) -> String {
    let mut result = String::new();
    for (k, v) in deps {
        let v = v
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        result.push_str(&format!("{k} -> {v}\n"));
    }
    result
}

pub fn get_cycle_cut_points<T, U>(
    deps: &BTreeMap<T, BTreeSet<U>>,
    cut_point_predicate: impl Fn(&T) -> bool,
) -> Vec<Option<T>>
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<U> + std::fmt::Debug,
    U: Hash + PartialEq + Eq + Clone + Ord + Into<T> + std::fmt::Debug,
{
    find_and_cut_cycles(deps, cut_point_predicate).1
}

// The input may contain cycles. The output will only contain the subset
// of the nodes that doesn't include any cycles
pub fn topological_sort<T, U>(deps: &BTreeMap<T, BTreeSet<U>>) -> Vec<T>
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<U> + std::fmt::Debug,
    U: Hash + PartialEq + Eq + Clone + Ord + Into<T> + std::fmt::Debug,
{
    // Topological sort
    let mut cycle_free_items = vec![];

    let mut in_degree: BTreeMap<T, usize> = BTreeMap::new();
    for (node, neighbors) in deps.iter() {
        in_degree.entry(node.clone()).or_insert(0);
        for neighbor in neighbors.iter() {
            *in_degree.entry(neighbor.clone().into()).or_insert(0) += 1;
        }
    }

    let mut queue: BinaryHeap<T> = BinaryHeap::new();
    for (node, &deg) in in_degree.iter() {
        if deg == 0 {
            queue.push(node.clone());
        }
    }

    while let Some(node) = queue.pop() {
        cycle_free_items.push(node.clone());
        if let Some(neighbors) = deps.get(&node) {
            for neighbor in neighbors {
                let neighbor_t: T = neighbor.clone().into();
                if let Some(deg) = in_degree.get_mut(&neighbor_t) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(neighbor_t);
                    }
                }
            }
        }
    }

    cycle_free_items.reverse();
    cycle_free_items
}

pub fn topological_levels<T, U>(deps: &BTreeMap<T, BTreeSet<U>>) -> Vec<Vec<T>>
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<U> + std::fmt::Debug,
    U: Hash + PartialEq + Eq + Clone + Ord + Into<T> + std::fmt::Debug,
{
    use std::collections::{BTreeMap, BTreeSet};

    let mut levels_map: BTreeMap<T, usize> = BTreeMap::new();
    let mut cycle_nodes: BTreeSet<T> = BTreeSet::new();

    fn compute_level<T, U>(
        node: &T,
        deps: &BTreeMap<T, BTreeSet<U>>,
        levels_map: &mut BTreeMap<T, usize>,
        stack: &mut BTreeSet<T>,
        cycle_nodes: &mut BTreeSet<T>,
    ) -> Option<usize>
    where
        T: Hash + PartialEq + Eq + Clone + Ord + Into<U> + std::fmt::Debug,
        U: Hash + PartialEq + Eq + Clone + Ord + Into<T> + std::fmt::Debug,
    {
        if let Some(&lvl) = levels_map.get(node) {
            return Some(lvl);
        }
        if stack.contains(node) {
            eprintln!("Cycle detected at node: {node:?}");
            cycle_nodes.insert(node.clone());
            return None;
        }
        stack.insert(node.clone());
        let level = match deps.get(node) {
            Some(neighbors) if !neighbors.is_empty() => {
                let mut max_level = 0;
                for n in neighbors {
                    let n_t: T = n.clone().into();
                    if let Some(dep_level) =
                        compute_level(&n_t, deps, levels_map, stack, cycle_nodes)
                    {
                        max_level = max_level.max(dep_level);
                    } else {
                        // propagate cycle
                        cycle_nodes.insert(node.clone());
                        stack.remove(node);
                        return None;
                    }
                }
                1 + max_level
            }
            _ => 0,
        };
        stack.remove(node);
        levels_map.insert(node.clone(), level);
        Some(level)
    }

    // Compute levels for all nodes
    for node in deps.keys() {
        let mut stack = BTreeSet::new();
        compute_level(node, deps, &mut levels_map, &mut stack, &mut cycle_nodes);
    }

    // Group nodes by level, skipping nodes in cycles
    let mut grouped: BTreeMap<usize, Vec<T>> = BTreeMap::new();
    for (node, lvl) in levels_map {
        if !cycle_nodes.contains(&node) {
            grouped.entry(lvl).or_default().push(node);
        }
    }

    grouped.into_values().collect()
}

pub fn prune_self_deps<T, U>(deps: &mut HashMap<T, HashSet<U>>)
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<U> + std::fmt::Debug,
    U: Hash + PartialEq + Eq + Clone + Ord + Into<T> + std::fmt::Debug,
{
    for (target, deps) in deps.iter_mut() {
        deps.remove(&target.to_owned().into());
    }
}

// slice the dependencies to only include the edges that go through the target nodes
pub fn collect_edges_through_nodes<T>(
    deps: &BTreeMap<T, BTreeSet<T>>,
    prefix_columns: &BTreeSet<T>,
    suffix_columns: &BTreeSet<T>,
    exact_columns: &BTreeSet<T>,
) -> BTreeMap<T, BTreeSet<T>>
where
    T: Hash + PartialEq + Eq + Clone + Ord + Into<T> + std::fmt::Debug,
{
    // Reverse the dependencies to get reverse_deps
    let reverse_deps = reverse(deps);

    // Compute the upward slice for all prefix columns
    let upstream_slice = slice(&reverse_deps, prefix_columns);

    // Compute the downward slice for all suffix columns
    let downstream_slice = slice(deps, suffix_columns);

    // Initialize the result with all exact columns
    let mut result = exact_columns
        .iter()
        .map(|node| (node.clone(), BTreeSet::new()))
        .collect::<BTreeMap<T, BTreeSet<T>>>();

    // Merge the upward slice into the result
    for (key, value) in reverse(&upstream_slice) {
        result
            .entry(key)
            .or_insert_with(BTreeSet::new)
            .extend(value);
    }

    // Merge the downward slice into the result
    for (key, value) in downstream_slice {
        result
            .entry(key)
            .or_insert_with(BTreeSet::new)
            .extend(value);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn test_topological_levels_simple_chain() {
        // a -> b -> c
        let mut deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        deps.insert("a", BTreeSet::from(["b"]));
        deps.insert("b", BTreeSet::from(["c"]));
        deps.insert("c", BTreeSet::new());

        let levels = topological_levels(&deps);
        assert_eq!(levels, vec![vec!["c"], vec!["b"], vec!["a"],]);
    }

    #[test]
    fn test_topological_levels_branching() {
        //   a
        //  / \
        // b   c
        //  \ /
        //   d
        let mut deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        deps.insert("a", BTreeSet::from(["b", "c"]));
        deps.insert("b", BTreeSet::from(["d"]));
        deps.insert("c", BTreeSet::from(["d"]));
        deps.insert("d", BTreeSet::new());

        let levels = topological_levels(&deps);
        assert_eq!(levels, vec![vec!["d"], vec!["b", "c"], vec!["a"],]);
    }

    #[test]
    fn test_topological_levels_multiple_roots() {
        // a   x
        // |   |
        // b   y
        let mut deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        deps.insert("a", BTreeSet::from(["b"]));
        deps.insert("b", BTreeSet::new());
        deps.insert("x", BTreeSet::from(["y"]));
        deps.insert("y", BTreeSet::new());

        let levels = topological_levels(&deps);
        assert_eq!(levels, vec![vec!["b", "y"], vec!["a", "x"],]);
    }

    #[test]
    fn test_topological_levels_disconnected() {
        // a -> b, c (disconnected)
        let mut deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        deps.insert("a", BTreeSet::from(["b"]));
        deps.insert("b", BTreeSet::new());
        deps.insert("c", BTreeSet::new());

        let levels = topological_levels(&deps);
        assert_eq!(levels, vec![vec!["b", "c"], vec!["a"],]);
    }

    #[test]
    fn test_topological_levels_empty() {
        let deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        let levels = topological_levels(&deps);
        assert!(levels.is_empty());
    }
    #[test]
    fn test_topological_levels_with_cycle() {
        // a -> b -> c -> a (cycle)
        let mut deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        deps.insert("a", BTreeSet::from(["b"]));
        deps.insert("b", BTreeSet::from(["c"]));
        deps.insert("c", BTreeSet::from(["a"]));

        let levels = topological_levels(&deps);
        // All nodes are in a cycle, so no levels can be formed
        assert_eq!(levels, Vec::<Vec<&str>>::new());
    }

    #[test]
    fn test_get_all_upstream_deps() {
        // Create a dependency graph
        // a -> b -> c
        // d -> b
        // e (isolated)
        let mut deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        deps.insert("a", BTreeSet::from(["b"]));
        deps.insert("b", BTreeSet::from(["c"]));
        deps.insert("c", BTreeSet::new());
        deps.insert("d", BTreeSet::from(["b"]));
        deps.insert("e", BTreeSet::new());

        // Test case 1: Get upstream deps starting from a single node
        let subset = BTreeSet::from(["a"]);
        let result = get_all_upstream_deps(&deps, &subset);
        assert_eq!(result.len(), 3); // Should include a, b, c
        assert!(result.contains_key("a"));
        assert!(result.contains_key("b"));
        assert!(result.contains_key("c"));
        assert!(!result.contains_key("d"));
        assert!(!result.contains_key("e"));

        // Test case 2: Get upstream deps starting from multiple nodes
        let subset = BTreeSet::from(["a", "d"]);
        let result = get_all_upstream_deps(&deps, &subset);
        assert_eq!(result.len(), 4); // Should include a, b, c, d
        assert!(result.contains_key("a"));
        assert!(result.contains_key("b"));
        assert!(result.contains_key("c"));
        assert!(result.contains_key("d"));
        assert!(!result.contains_key("e"));

        // Test case 3: Get upstream deps for an isolated node
        let subset = BTreeSet::from(["e"]);
        let result = get_all_upstream_deps(&deps, &subset);
        assert_eq!(result.len(), 1); // Should only include e
        assert!(result.contains_key("e"));

        // Test case 4: Get upstream deps for a leaf node
        let subset = BTreeSet::from(["c"]);
        let result = get_all_upstream_deps(&deps, &subset);
        assert_eq!(result.len(), 1); // Should only include c
        assert!(result.contains_key("c"));

        // Test case 5: Empty subset
        let subset = BTreeSet::new();
        let result = get_all_upstream_deps(&deps, &subset);
        assert_eq!(result.len(), 0); // Should be empty
    }

    #[test]
    fn test_restrict_with_transitive() {
        // a - b - d - e
        //       \
        //         c
        let mut deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        deps.insert("a", BTreeSet::from(["b"]));
        deps.insert("b", BTreeSet::from(["c", "d"]));
        deps.insert("d", BTreeSet::from(["e"]));

        let subset = BTreeSet::from(["a", "c", "d", "e"]);
        let subgraph = restrict_with_transitive(&deps, &subset);

        // expected
        //     a - d - e
        //       \
        //         c
        assert_eq!(*subgraph.get("a").unwrap(), BTreeSet::from(["c", "d"]));
        assert_eq!(*subgraph.get("c").unwrap(), BTreeSet::new());
        assert_eq!(*subgraph.get("d").unwrap(), BTreeSet::from(["e"]));
    }
}
