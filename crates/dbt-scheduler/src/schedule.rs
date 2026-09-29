use std::collections::{BTreeMap, BTreeSet};

use dbt_adapter::AdapterType;

use dbt_common::collections::DashMap;
use dbt_common::io_args::FsCommand;
use dbt_common::stats::Stat;
use dbt_common::tracing::dbt_emit::{
    emit_error_log_from_fs_error, emit_info_log_message, emit_warn_log_from_fs_error,
    emit_warn_log_message,
};
use dbt_common::tracing::event_info::store_event_attributes;
use dbt_common::{
    ErrorCode, FsError, FsResult,
    cancellation::CancellationToken,
    err, fs_err,
    node_selector::{IndirectSelection, SelectExpression, convert_column_selectors_to_fqn},
};
use dbt_dag::{
    deps_mgmt::{
        ensure_all_nodes_defined, find_and_cut_cycles, restrict_with_transitive, reverse,
        topological_sort,
    },
    schedule::Schedule,
};
use dbt_schemas::schemas::common::DbtChecksum;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::selectors::{ResolvedSelector, SelectorEntry};
use dbt_schemas::schemas::telemetry::{ExecutionPhase, NodeType, PhaseExecuted};
use dbt_schemas::schemas::{InternalDbtNode, Nodes, StateArtifacts};

use std::collections::HashMap;

use dbt_schemas::schemas::selection_override::{
    SelectionOverride, SelectionOverrideStats, compute_selection_override_stats, format_sample,
};

use crate::{
    args::SchedulerArgs,
    node_selector::{
        StateSelectorResults, filter_select_criteria_with_state_selector_results, fnmatch,
    },
};

/// Schedule nodes based on selection criteria and dependencies.
///
/// # Arguments
/// * `arg` - Scheduler arguments containing configuration
/// * `nodes` - Available nodes in the graph
/// * `previous_state` - Optional previous state for incremental scheduling
/// * `resolved_selectors` - Selection criteria for nodes
///
/// # Returns
/// * `FsResult<Schedule<String>>` - Scheduled nodes with dependencies
pub fn build_schedule(
    arg: &SchedulerArgs,
    nodes: &Nodes,
    previous_state: Option<&StateArtifacts>,
    resolved_selectors: &ResolvedSelector,
    token: &CancellationToken,
    adapter_type: AdapterType,
) -> FsResult<Schedule<String>> {
    build_schedule_with_state_selector_results(
        arg,
        nodes,
        previous_state,
        resolved_selectors,
        None,
        token,
        adapter_type,
    )
}

/// Build a schedule, optionally using externally evaluated state selector results.
#[tracing::instrument(
    skip_all,
    fields(
        _e = ?store_event_attributes(PhaseExecuted {
            phase: ExecutionPhase::Schedule as i32,
            ..Default::default()
        }),
    )
)]
pub fn build_schedule_with_state_selector_results(
    arg: &SchedulerArgs,
    nodes: &Nodes,
    previous_state: Option<&StateArtifacts>,
    resolved_selectors: &ResolvedSelector,
    state_selector_results: Option<&StateSelectorResults>,
    token: &CancellationToken,
    adapter_type: AdapterType,
) -> FsResult<Schedule<String>> {
    token.check_cancellation()?;

    let deps = derive_deps(nodes, token)?;

    let (converted_include, include_had_columns) =
        if let Some(include) = resolved_selectors.include.clone() {
            let (converted, changed) = convert_column_selectors_to_fqn(include);
            (Some(converted), changed)
        } else {
            (None, false)
        };

    let (converted_exclude, exclude_had_columns) =
        if let Some(exclude) = resolved_selectors.exclude.clone() {
            let (converted, changed) = convert_column_selectors_to_fqn(exclude);
            (Some(converted), changed)
        } else {
            (None, false)
        };

    let converted_selectors = ResolvedSelector {
        include: converted_include,
        exclude: converted_exclude,
        selector_definitions: resolved_selectors.selector_definitions.clone(),
    };

    if include_had_columns || exclude_had_columns {
        emit_warn_log_message(
            ErrorCode::InvalidColumnSelector,
            "Column selectors are unsupported at runtime; use node-level selection instead.",
        );
    }

    // Get the schedule with all selectors applied
    let (schedule, _override_stats) = schedule_graph(
        &deps,
        nodes,
        previous_state,
        &converted_selectors,
        state_selector_results,
        arg,
        adapter_type,
    )?;

    Ok(schedule)
}

pub fn derive_deps(
    nodes: &Nodes,
    token: &CancellationToken,
) -> FsResult<BTreeMap<String, BTreeSet<String>>> {
    let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    // Collect all node dependencies first
    for (unique_id, node) in nodes.iter() {
        token.check_cancellation()?;
        let mut dependencies = BTreeSet::new();

        for dep in node.base().depends_on.nodes.iter() {
            if !nodes.contains(dep) {
                return if let Some((_, location)) = node
                    .base()
                    .depends_on
                    .nodes_with_ref_location
                    .iter()
                    .find(|(node_dep, _)| node_dep == dep)
                {
                    err!(
                        code => ErrorCode::DependencyNotFound,
                        loc => location.clone(),
                        "Dependency {} not found for node {}", dep, unique_id
                    )
                } else {
                    // SavedQuery node dependencies don't have ref locations
                    err!(
                        ErrorCode::DependencyNotFound,
                        "Dependency {} not found for node {}",
                        dep,
                        unique_id
                    )
                };
            }
            dependencies.insert(dep.clone());
        }

        deps.insert(unique_id.clone(), dependencies);
    }

    deps = ensure_all_nodes_defined(&deps);

    Ok(deps)
}

/// Schedule a graph based on selection criteria and dependencies.
/// This is the core scheduling function that handles node selection,
/// indirect selection modes, and dependency resolution.
///
/// # Arguments
/// * `deps` - Graph dependencies
/// * `nodes` - Available nodes
/// * `previous_state` - Optional previous state for incremental scheduling
/// * `unused_nodes` - Set of nodes that should be excluded from scheduling
/// * `resolved_selectors` - Selection criteria including include/exclude patterns
///
/// # Returns
/// * `FsResult<Schedule<String>>` - The final schedule with all selected nodes and their dependencies
fn schedule_graph(
    deps: &BTreeMap<String, BTreeSet<String>>,
    nodes: &Nodes,
    previous_state: Option<&StateArtifacts>,
    resolved_selectors: &ResolvedSelector,
    state_selector_results: Option<&StateSelectorResults>,
    args: &SchedulerArgs,
    adapter_type: AdapterType,
) -> FsResult<(Schedule<String>, Option<SelectionOverrideStats>)> {
    // Get fully expanded included nodes.
    // For `dbt show`, override indirect selection to Empty on every atom so that tests are
    // only included when they are explicitly selected
    let selected_nodes = match &resolved_selectors.include {
        Some(include) => {
            let effective_include = if args.command == FsCommand::Show {
                with_indirect_selection(include.clone(), IndirectSelection::Empty)
            } else {
                include.clone()
            };
            expand_selector_with_state_selector_results(
                &effective_include,
                deps,
                nodes,
                previous_state,
                state_selector_results,
                adapter_type,
                &resolved_selectors.selector_definitions,
            )?
        }
        None => nodes.materializable_keys().cloned().collect(),
    };

    // Get fully expanded excluded nodes
    let excluded_nodes = match &resolved_selectors.exclude {
        Some(exclude) => expand_selector_with_state_selector_results(
            exclude,
            deps,
            nodes,
            previous_state,
            state_selector_results,
            adapter_type,
            &resolved_selectors.selector_definitions,
        )?,
        None => BTreeSet::new(),
    };

    // Selected resources: this is dbt-core's concept of selected nodes, without filtering for
    // sources, unused, extended_models, etc.
    let mut selected_nodes: BTreeSet<String> = selected_nodes
        .into_iter()
        .filter(|node| !(excluded_nodes.contains(node) || args.exclude_unique_ids.contains(node)))
        .collect();

    // Greedy test exclusion: if ANY parent is explicitly excluded, exclude the test too
    // This matches dbt-core semantics and applies to ALL indirect selection modes
    // See: https://docs.getdbt.com/reference/node-selection/test-selection-examples#indirect-selection
    if !excluded_nodes.is_empty() || !args.exclude_unique_ids.is_empty() {
        selected_nodes.retain(|node_id| {
            // Keep non-test nodes
            if !nodes.tests.contains_key(node_id) && !nodes.unit_tests.contains_key(node_id) {
                return true;
            }

            // For tests: check if ANY parent was explicitly excluded
            // If so, exclude the test (greedy exclusion)
            if let Some(test) = nodes.tests.get(node_id) {
                // Test stays if NONE of its deps were excluded
                !test.base().depends_on.nodes.iter().any(|dep| {
                    excluded_nodes.contains(dep) || args.exclude_unique_ids.contains(dep)
                })
            } else if let Some(unit_test) = nodes.unit_tests.get(node_id) {
                // Unit test stays if its dep was not excluded
                !unit_test.base().depends_on.nodes.iter().any(|dep| {
                    excluded_nodes.contains(dep) || args.exclude_unique_ids.contains(dep)
                })
            } else {
                true
            }
        });
    }

    if !args.resource_types.is_empty() || !args.exclude_resource_types.is_empty() {
        // Pre-compute resource type strings to avoid repeated conversions
        let include_types: std::collections::HashSet<NodeType> =
            args.resource_types.iter().map(|rt| rt.into()).collect();
        let exclude_types: std::collections::HashSet<NodeType> = args
            .exclude_resource_types
            .iter()
            .map(|rt| rt.into())
            .collect();

        // Filter selected nodes by resource type
        selected_nodes.retain(|node_id| {
            if let Some(node) = nodes.get_node(node_id) {
                let node_type = node.resource_type();

                // Check if node type is in exclude list - if so, filter it out
                if exclude_types.contains(&node_type) {
                    return false;
                }

                // If include list is specified, node must match at least one included type
                if !include_types.is_empty() {
                    include_types.contains(&node_type)
                } else {
                    // No include list specified, so include all non-excluded types
                    true
                }
            } else {
                false
            }
        });
    }

    // Everything above is the *selection* half: what the caller's selection inputs decide. An
    // override replaces that half wholesale. It is still computed when an override wins, because
    // the size of the selection this engine would have made is the measurement the override
    // exists to produce.
    let computed_selection = selected_nodes;

    let (selected_nodes, override_stats) = match args.selection_override.as_ref() {
        Some(over) => {
            // Ids with no counterpart node here cannot be scheduled and must not travel further:
            // the stages below would carry a phantom id into the sorted node list.
            let matched: BTreeSet<String> = over
                .ids()
                .iter()
                .filter(|id| nodes.get_node(id).is_some())
                .cloned()
                .collect();

            let mut scheduled = matched.clone();
            retain_schedulable(&mut scheduled, nodes, args);

            let stats = compute_selection_override_stats(
                over.ids(),
                &computed_selection,
                &matched,
                &scheduled,
                nodes,
            );
            emit_selection_override_report(over, &stats);
            (scheduled, Some(stats))
        }
        None => {
            let mut selected_nodes = computed_selection;
            retain_schedulable(&mut selected_nodes, nodes, args);
            (selected_nodes, None)
        }
    };

    let all_selected_nodes = selected_nodes.clone();

    // Apply exclusions: selected - sources - unused - extended_models
    let (_external_nodes, mut filtered_selected_nodes): (BTreeSet<String>, BTreeSet<String>) =
        selected_nodes.iter().cloned().partition(|node| {
            nodes.sources.contains_key(node)
                || nodes.get_node(node).is_some_and(|n| n.is_extended_model())
        });

    // Expand selected nodes to include all ephemeral upstream dependencies
    // Skip this when the command is "list" as ephemeral nodes are not wanted.
    if args.command != FsCommand::List {
        let mut ephemeral_to_add = BTreeSet::new();
        for node_id in &filtered_selected_nodes {
            collect_ephemeral_upstream(node_id, deps, nodes, &mut ephemeral_to_add);
        }
        filtered_selected_nodes.extend(ephemeral_to_add);
    }

    let frontier_nodes = get_frontier_nodes(&filtered_selected_nodes, deps);

    // Create a combined set of selected nodes and frontier nodes
    let selected_and_frontier = filtered_selected_nodes
        .union(&frontier_nodes)
        .cloned()
        .collect();

    // Build restricted subgraph while maintaining transitive dependencies
    let deps_with_frontier = restrict_with_transitive(deps, &selected_and_frontier);

    // sorted_nodes should contain selected nodes first (these will be compiled/executed)
    // We still use only selected_deps for topological sort to maintain execution order
    let selected_deps = restrict_with_transitive(deps, &filtered_selected_nodes);
    let mut sorted_nodes = topological_sort(&selected_deps);

    // Add frontier nodes at the end for introspection detection
    // They won't have tasks created but need to be in sorted_nodes for processing
    sorted_nodes.extend(frontier_nodes.iter().cloned());

    // Check for cycles in the selected nodes only
    let (cycle_warnings, cycle_errors) = find_cycle_diagnostics(&selected_deps, nodes);
    for warning in cycle_warnings {
        emit_warn_log_from_fs_error(warning);
    }
    for error in cycle_errors {
        emit_error_log_from_fs_error(error);
    }

    // Compute overlapping sources: sources whose relation_name matches a SELECTED seed's relation_name
    // Only skip hydration when the seed is selected in the current command, ensuring the seed runs
    // first and creates the table that the source references
    let overlapping_sources = {
        use std::collections::HashSet;

        // Collect relation_names only from SELECTED seeds
        let seed_relation_names: HashSet<String> = nodes
            .seeds
            .iter()
            .filter(|(seed_uid, _)| filtered_selected_nodes.contains(*seed_uid))
            .filter_map(|(_, seed)| seed.base().relation_name.clone())
            .collect();

        // Find sources with matching relation_names
        let mut overlaps = BTreeSet::new();
        for (source_uid, source_node) in &nodes.sources {
            if let Some(relation_name) = &source_node.base().relation_name {
                if seed_relation_names.contains(relation_name) {
                    overlaps.insert(source_uid.clone());
                }
            }
        }

        overlaps
    };

    // Construct the final schedule
    let schedule = Schedule {
        deps: deps_with_frontier, // Include frontier nodes in deps for introspection detection
        sorted_nodes,
        selected_nodes: filtered_selected_nodes,
        frontier_nodes,
        overlapping_sources,
        all_selected_nodes,
        select: resolved_selectors.include.clone(),
        exclude: resolved_selectors.exclude.clone(),
    };

    // Only check invariants if this is not a list command
    schedule.debug_assert_invariants(nodes);
    Ok((schedule, override_stats))
}

/// Report that the selection was replaced, and by how much the two sets differ.
///
/// Emitted twice on purpose: warnings here are deferred to an end-of-run summary that is long on a
/// real project, so the counters also go out immediately, where the replacement happens. The
/// warning is what a collector scanning warn-or-worse groups on.
///
/// `unmatched` and `dropped_unschedulable` are triage gates, not diagnostics: both remove nodes the
/// supplied set named, and a removed node changes `ref()` resolution for whatever survives
/// downstream, which can manufacture a failure that looks genuine.
fn emit_selection_override_report(over: &SelectionOverride, stats: &SelectionOverrideStats) {
    let counters = format!(
        "Selection replaced by an externally supplied node set from '{}': \
         override_active=true injected={} matched={} unmatched={} \
         fusion_would_select={} fusion_only={} dropped_unschedulable={}",
        over.source().display(),
        stats.injected,
        stats.matched,
        stats.unmatched,
        stats.fusion_would_select,
        stats.fusion_only,
        stats.dropped_unschedulable,
    );

    // Immediate, in-place, for whoever is watching the run.
    emit_info_log_message(&counters);

    // Deferred and coded, for the end-of-run summary and for anything scanning warn-or-worse. The
    // triage guidance rides this copy: it is the one a failure investigation comes back to.
    let mut warning = counters;
    if stats.unmatched > 0 {
        warning.push_str(&format!(
            "; no node here matches {}",
            format_sample(&stats.unmatched_sample, stats.unmatched)
        ));
    }
    if stats.dropped_unschedulable > 0 {
        warning.push_str(&format!(
            "; cannot be run here {}",
            format_sample(
                &stats.dropped_unschedulable_sample,
                stats.dropped_unschedulable
            )
        ));
    }
    if stats.unmatched > 0 || stats.dropped_unschedulable > 0 {
        warning.push_str(
            ". Those nodes will not run, so a downstream failure may be an artifact of this \
             replacement rather than a genuine defect.",
        );
    }

    emit_warn_log_message(ErrorCode::SelectionOverrideActive, warning);
}

/// Drop nodes this engine structurally cannot run.
///
/// This is the *schedulability* half of `schedule_graph`'s filtering, kept apart from the
/// *selection* half (which selection inputs decide). Selection is a choice; schedulability is a
/// capability of this engine, so it applies to any incoming node set whatever its provenance.
fn retain_schedulable(selected_nodes: &mut BTreeSet<String>, nodes: &Nodes, args: &SchedulerArgs) {
    // For the compile command, exclude unit tests so they never enter the schedule even if
    // selectors or CLI resource-type filters would match them; dbt-core hard-excludes unit tests
    // from compile selection.
    if args.command == FsCommand::Compile {
        selected_nodes.retain(|node_id| !nodes.unit_tests.contains_key(node_id));
    }

    // Filter out models whose normalized SQL content is empty
    // We detect this via the checksum computed during resolve: an empty normalized SQL
    // produces the SHA256 hash of an empty string.
    let empty_checksum = DbtChecksum::hash(b"");
    selected_nodes.retain(|node_id| {
        if let Some(model) = nodes.models.get(node_id) {
            model.__common_attr__.checksum != empty_checksum
        } else {
            true
        }
    });

    selected_nodes.retain(|node_id| {
        !nodes.get_node(node_id).is_some_and(|node| {
            matches!(node.resource_type(), NodeType::Test | NodeType::UnitTest)
                && !node.base().enabled
        })
    });
}

// ------------------------------------------------------------------------------------------------
// Recursive selector evaluation with scoped `exclude:` handling
// ------------------------------------------------------------------------------------------------
#[derive(Default, Debug)]
struct EvalResult {
    include: BTreeSet<String>,
    exclude: BTreeSet<String>,
    /// True iff this result represents a syntactic `exclude(...)` operand.
    ///
    /// We must preserve this even when the exclude matches zero nodes, otherwise
    /// an `intersection` with an empty exclude incorrectly collapses to empty.
    exclude_operand: bool,
}

impl EvalResult {
    fn merge_or(mut self, other: EvalResult) -> Self {
        self.include.extend(other.include);
        self.exclude.extend(other.exclude);
        // OR-ing results produces an include set; it is not a "pure exclude" operand.
        self.exclude_operand = false;
        self
    }
    fn merge_and(mut self, other: EvalResult) -> Self {
        if self.include.is_empty() && other.include.is_empty() {
            return Self::default();
        }
        // Special case: `intersection` with an `exclude(...)` operand means
        // "keep the other side, but subtract the exclude set", even if that set is empty.
        let self_is_exclude = self.exclude_operand;
        let other_is_exclude = other.exclude_operand;

        if self_is_exclude && other_is_exclude {
            // (NOT A) AND (NOT B) == NOT (A OR B)
            self.exclude.extend(other.exclude);
            self.include.clear();
            self.exclude_operand = true;
            return self;
        }

        if self_is_exclude && !other_is_exclude {
            // self is an exclude, apply it to other's include
            let final_other = other.into_final();
            let final_self = self.exclude;
            Self {
                include: final_other.difference(&final_self).cloned().collect(),
                exclude: BTreeSet::new(),
                exclude_operand: false,
            }
        } else if other_is_exclude && !self_is_exclude {
            // other is an exclude, apply it to self's include
            let final_self = self.into_final();
            let final_other = other.exclude;
            Self {
                include: final_self.difference(&final_other).cloned().collect(),
                exclude: BTreeSet::new(),
                exclude_operand: false,
            }
        } else {
            // Normal intersection logic
            if !self.include.is_empty() {
                self.include = self.include.intersection(&other.include).cloned().collect();
            }
            self.exclude.extend(other.exclude);
            self.exclude_operand = false;
            self
        }
    }
    fn into_final(self) -> BTreeSet<String> {
        self.include.difference(&self.exclude).cloned().collect()
    }
}

/// Evaluate a `SelectExpression`, preserving the scope of nested `exclude:` handling
/// blocks and expanding indirect selections before diffing.
fn eval_selector(
    expr: &SelectExpression,
    deps: &BTreeMap<String, BTreeSet<String>>,
    nodes: &Nodes,
    previous_state: Option<&StateArtifacts>,
    state_selector_results: Option<&StateSelectorResults>,
    adapter_type: AdapterType,
    selector_defs: &HashMap<String, SelectorEntry>,
    resolution_stack: &mut Vec<String>,
) -> FsResult<EvalResult> {
    use SelectExpression::*;
    use dbt_common::node_selector::MethodName;
    match expr {
        Or(children) => {
            let mut acc = EvalResult::default();
            for c in children {
                acc = acc.merge_or(eval_selector(
                    c,
                    deps,
                    nodes,
                    previous_state,
                    state_selector_results,
                    adapter_type,
                    selector_defs,
                    resolution_stack,
                )?);
            }
            Ok(acc)
        }
        And(children) => {
            let mut iter = children.iter();
            let Some(first) = iter.next() else {
                return Ok(EvalResult::default());
            };
            let mut acc = eval_selector(
                first,
                deps,
                nodes,
                previous_state,
                state_selector_results,
                adapter_type,
                selector_defs,
                resolution_stack,
            )?;
            for c in iter {
                acc = acc.merge_and(eval_selector(
                    c,
                    deps,
                    nodes,
                    previous_state,
                    state_selector_results,
                    adapter_type,
                    selector_defs,
                    resolution_stack,
                )?);
            }
            Ok(acc)
        }
        Exclude(child) => {
            let excl = eval_selector(
                child,
                deps,
                nodes,
                previous_state,
                state_selector_results,
                adapter_type,
                selector_defs,
                resolution_stack,
            )?
            .into_final();
            Ok(EvalResult {
                include: BTreeSet::new(),
                exclude: excl,
                exclude_operand: true,
            })
        }
        Atom(criteria) => {
            if criteria.method == MethodName::Selector {
                return eval_selector_method(
                    criteria,
                    deps,
                    nodes,
                    previous_state,
                    state_selector_results,
                    adapter_type,
                    selector_defs,
                    resolution_stack,
                );
            }

            // 1️⃣ base filter
            let mut selected = filter_select_criteria_with_state_selector_results(
                nodes,
                criteria,
                previous_state,
                state_selector_results,
                adapter_type,
            )?;

            // 2️⃣ graph operators — multi-source traversal
            //
            // Instead of expanding each matched node independently (which
            // re-reverses the entire graph and re-walks overlapping sub-trees
            // for every seed), we perform a single BFS from all seeds at once
            // in each direction.  This reduces O(S × (V+E)) to O(V+E).
            if criteria.parents_depth.is_some() || criteria.children_depth.is_some() {
                let mut expanded = selected.clone();

                // "+foo" — upstream: follow dep edges from all seeds
                if let Some(depth) = criteria.parents_depth {
                    expanded.extend(multi_source_bfs(deps, &selected, depth));
                }

                // "foo+" — downstream: reverse the graph once, then BFS
                if let Some(depth) = criteria.children_depth {
                    let rev_deps = reverse(deps);
                    expanded.extend(multi_source_bfs(&rev_deps, &selected, depth));
                }

                selected = expanded;
            } else if criteria.childrens_parents {
                // collect all lowest level descendants of the selected nodes
                // then get all their anscestors
                selected = collect_childrens_parents(deps, &selected);
            }

            // 3️⃣ indirect selection
            let mode = criteria.indirect.unwrap_or(IndirectSelection::Eager);
            let (expanded, indirect) = expand_selection(&selected, mode, deps, nodes);
            let mut selected = incorporate_indirect_nodes(expanded, indirect, deps, nodes, mode);

            // 4️⃣ nested exclude inside this atom
            if let Some(ex) = &criteria.exclude {
                let excl_set = eval_selector(
                    ex,
                    deps,
                    nodes,
                    previous_state,
                    state_selector_results,
                    adapter_type,
                    selector_defs,
                    resolution_stack,
                )?
                .into_final();
                selected.retain(|n| !excl_set.contains(n));
            }

            Ok(EvalResult {
                include: selected,
                exclude: BTreeSet::new(),
                exclude_operand: false,
            })
        }
    }
}

/// Resolve a `selector:` method atom at runtime.
fn eval_selector_method(
    criteria: &dbt_common::node_selector::SelectionCriteria,
    deps: &BTreeMap<String, BTreeSet<String>>,
    nodes: &Nodes,
    previous_state: Option<&StateArtifacts>,
    state_selector_results: Option<&StateSelectorResults>,
    adapter_type: AdapterType,
    selector_defs: &HashMap<String, SelectorEntry>,
    resolution_stack: &mut Vec<String>,
) -> FsResult<EvalResult> {
    let pattern = criteria
        .value
        .resolve(dbt_common::node_selector::MethodName::Selector)?;

    // fnmatch lookup: supports wildcards like `selector:model_*`
    let matching: Vec<(&String, &SelectorEntry)> = selector_defs
        .iter()
        .filter(|(name, _)| fnmatch(pattern, name))
        .collect();

    if matching.is_empty() {
        return err!(ErrorCode::SelectorError, "Unknown selector `{}`", pattern);
    }

    // Evaluate each matching selector, unioning results
    let mut combined = EvalResult::default();
    for (name, entry) in &matching {
        if resolution_stack.contains(name) {
            let cycle: Vec<_> = resolution_stack
                .iter()
                .skip_while(|n| n != name)
                .cloned()
                .collect();
            return err!(
                ErrorCode::SelectorError,
                "Circular selector reference: {} -> {}",
                cycle.join(" -> "),
                name
            );
        }
        resolution_stack.push((*name).clone());
        let result = eval_selector(
            &entry.include,
            deps,
            nodes,
            previous_state,
            state_selector_results,
            adapter_type,
            selector_defs,
            resolution_stack,
        )?;
        resolution_stack.pop();
        combined = combined.merge_or(result);
    }

    let mut selected = combined.into_final();

    // Apply graph operators on the combined result
    if criteria.parents_depth.is_some() || criteria.children_depth.is_some() {
        let mut expanded = selected.clone();
        if let Some(depth) = criteria.parents_depth {
            expanded.extend(multi_source_bfs(deps, &selected, depth));
        }
        if let Some(depth) = criteria.children_depth {
            let rev_deps = reverse(deps);
            expanded.extend(multi_source_bfs(&rev_deps, &selected, depth));
        }
        selected = expanded;
    } else if criteria.childrens_parents {
        selected = collect_childrens_parents(deps, &selected);
    }

    // Indirect selection
    let mode = criteria.indirect.unwrap_or(IndirectSelection::Eager);
    let (expanded, indirect) = expand_selection(&selected, mode, deps, nodes);
    let mut selected = incorporate_indirect_nodes(expanded, indirect, deps, nodes, mode);

    // Nested exclude
    if let Some(ex) = &criteria.exclude {
        let excl_set = eval_selector(
            ex,
            deps,
            nodes,
            previous_state,
            state_selector_results,
            adapter_type,
            selector_defs,
            resolution_stack,
        )?
        .into_final();
        selected.retain(|n| !excl_set.contains(n));
    }

    Ok(EvalResult {
        include: selected,
        exclude: BTreeSet::new(),
        exclude_operand: false,
    })
}

pub fn expand_selector(
    selector: &SelectExpression,
    deps: &BTreeMap<String, BTreeSet<String>>,
    nodes: &Nodes,
    previous_state: Option<&StateArtifacts>,
    adapter_type: AdapterType,
    selector_defs: &HashMap<String, SelectorEntry>,
) -> FsResult<BTreeSet<String>> {
    expand_selector_with_state_selector_results(
        selector,
        deps,
        nodes,
        previous_state,
        None,
        adapter_type,
        selector_defs,
    )
}

/// Expand a selector, optionally using externally evaluated state selector results.
pub fn expand_selector_with_state_selector_results(
    selector: &SelectExpression,
    deps: &BTreeMap<String, BTreeSet<String>>,
    nodes: &Nodes,
    previous_state: Option<&StateArtifacts>,
    state_selector_results: Option<&StateSelectorResults>,
    adapter_type: AdapterType,
    selector_defs: &HashMap<String, SelectorEntry>,
) -> FsResult<BTreeSet<String>> {
    let mut resolution_stack = Vec::new();
    let res = eval_selector(
        selector,
        deps,
        nodes,
        previous_state,
        state_selector_results,
        adapter_type,
        selector_defs,
        &mut resolution_stack,
    )?;
    Ok(res.into_final())
}

/// Recursively walk a `SelectExpression` tree and override the `indirect` field on every
/// `Atom` with the given `IndirectSelection` mode.
fn with_indirect_selection(expr: SelectExpression, mode: IndirectSelection) -> SelectExpression {
    match expr {
        SelectExpression::Atom(mut criteria) => {
            criteria.indirect = Some(mode);
            SelectExpression::Atom(criteria)
        }
        SelectExpression::Or(children) => SelectExpression::Or(
            children
                .into_iter()
                .map(|c| with_indirect_selection(c, mode))
                .collect(),
        ),
        SelectExpression::And(children) => SelectExpression::And(
            children
                .into_iter()
                .map(|c| with_indirect_selection(c, mode))
                .collect(),
        ),
        SelectExpression::Exclude(child) => {
            SelectExpression::Exclude(Box::new(with_indirect_selection(*child, mode)))
        }
    }
}

/// Identifies cycles and returns a diagnostic per cycle and per dependency reference.
fn find_cycle_diagnostics(
    deps: &BTreeMap<String, BTreeSet<String>>,
    nodes: &Nodes,
) -> (Vec<FsError>, Vec<FsError>) {
    let (cycles, _, _) = find_and_cut_cycles(deps, |_| true);
    let warnings = Vec::new();
    let mut errors = Vec::new();

    for (i, cycle) in cycles.iter().enumerate() {
        // Show the full cycle.
        let mut cycle_with_first = cycle.clone();
        cycle_with_first.push(cycle[0].clone());
        let cycle_str = cycle_with_first
            .iter()
            .map(|node| format!("[{node}]"))
            .collect::<Vec<_>>()
            .join(" -> ");

        errors.push(FsError::new(
            ErrorCode::CyclicDependency,
            format!("Cycle detected: {cycle_str}"),
        ));

        // Show each dependency in the cycle with its location.
        for window in cycle.windows(2) {
            let from = &window[0];
            let to = &window[1];
            if let Some(node) = nodes.get_node(from) {
                // Report all references to this dependency.
                for (dep, location) in node.base().depends_on.nodes_with_ref_location.iter() {
                    if dep == to {
                        let err = fs_err!(
                            code => ErrorCode::CyclicDependency,
                            loc => location.clone(),
                            "   Cycle {}:  [{}] depends on [{}]",
                            i + 1,
                            from,
                            to
                        );
                        errors.push(*err);
                    }
                }
            }
        }

        // Last node -> First node in cycle.
        if let Some(node) = nodes.get_node(cycle.last().unwrap()) {
            // Report all references to the first node.
            for (dep, location) in node.base().depends_on.nodes_with_ref_location.iter() {
                if dep == cycle.first().unwrap() {
                    let err = fs_err!(
                        code => ErrorCode::CyclicDependency,
                        loc => location.clone(),
                        "   Cycle {}:  [{}] depends on [{}]",
                        i+1,
                        cycle.last().unwrap(),
                        cycle.first().unwrap()
                    );
                    errors.push(*err);
                }
            }
        }
    }

    (warnings, errors)
}

/// Recursively collect all ephemeral models upstream of a given node.
///
/// # Arguments
/// * `node_id` - The starting node to trace upstream from
/// * `deps` - Dependency graph
/// * `nodes` - All available nodes
/// * `ephemeral_nodes` - Set to collect ephemeral node IDs into
fn collect_ephemeral_upstream(
    node_id: &str,
    deps: &BTreeMap<String, BTreeSet<String>>,
    nodes: &Nodes,
    ephemeral_nodes: &mut BTreeSet<String>,
) {
    if let Some(unit_test) = nodes.unit_tests.get(node_id) {
        for dep in &unit_test.base().depends_on.nodes {
            collect_ephemeral_upstream(dep, deps, nodes, ephemeral_nodes);
        }
    }

    if let Some(node_deps) = deps.get(node_id) {
        for dep in node_deps {
            if let Some(node) = nodes.get_node(dep)
                && node.materialized() == DbtMaterialization::Ephemeral
            {
                // Add the ephemeral node
                ephemeral_nodes.insert(dep.clone());
                // Recursively collect ephemeral dependencies of this ephemeral node
                collect_ephemeral_upstream(dep, deps, nodes, ephemeral_nodes);
            }
        }
    }
}

/// Massage the schedule in accordance to the compute field of dbt nodes
/// for sidecar execution.
/// If a node is marked as `compute: remote`
///   - treated as a frontier node
///   - any tests that depend on a remote node are dropped
pub fn modify_schedule_for_sidecar_compute_boundaries(
    schedule: &mut Schedule<String>,
    nodes: &Nodes,
) {
    use dbt_common::io_args::ComputeArg;

    let mut moved_to_frontier: BTreeSet<String> = BTreeSet::new();
    let mut removed_tests: BTreeSet<String> = BTreeSet::new();

    for uid in schedule.selected_nodes.iter() {
        let Some(node) = nodes.get_node(uid) else {
            continue;
        };
        if node.compute() != Some(ComputeArg::Remote) {
            continue;
        }
        match node.resource_type() {
            NodeType::Model | NodeType::Snapshot => {
                moved_to_frontier.insert(uid.clone());
            }
            NodeType::Test | NodeType::UnitTest => {
                removed_tests.insert(uid.clone());
            }
            // Unreachable because no other nodes expose the compute field.
            other => unreachable!(
                "compute: remote on unsupported node type {:?} ({}). \
                 Only model/snapshot/test/unit_test should ever carry this config; \
                 the schema layer is the source of truth — fix it there.",
                other, uid,
            ),
        }
    }

    for uid in &moved_to_frontier {
        schedule.selected_nodes.remove(uid);
        schedule.frontier_nodes.insert(uid.clone());
    }

    // drop tests/unit tests whose target is a
    // compute-remote node. They should not be selected for sidecar execution
    let stale_test_nodes: Vec<String> = schedule
        .selected_nodes
        .iter()
        .filter(|uid| {
            // grap unit test or generic tests dependencies
            let test_deps = if let Some(t) = nodes.tests.get(*uid) {
                Some(&t.base().depends_on.nodes)
            } else {
                nodes
                    .unit_tests
                    .get(*uid)
                    .map(|u| &u.base().depends_on.nodes)
            };
            // if tests depend on remote parents, they cannot be executed in sidecar/service
            test_deps
                .map(|deps| {
                    deps.iter().any(|d| {
                        nodes
                            .get_node(d)
                            .map(|n| n.compute() == Some(ComputeArg::Remote))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    for uid in removed_tests.into_iter().chain(stale_test_nodes) {
        // preserve invariants in schedule by propagating test node removal to dependent fields:
        // sorted_nodes = selected_nodes + frontier_nodes
        // deps = selected_nodes + frontier_nodes
        schedule.selected_nodes.remove(&uid);
        schedule.sorted_nodes.retain(|x| x != &uid);
        schedule.deps.remove(&uid);
    }
}

/// Summary of what `modify_schedule_for_sidecar_compute_boundaries` did,
/// derived structurally from the post-mutation schedule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComputeBoundaryReport {
    /// Models/snapshots that were promoted from selection to frontier
    pub boundary_nodes: BTreeSet<String>,
    /// Tests / unit tests that got dropped
    pub skipped_test_nodes: BTreeSet<String>,
}

/// Reconstruct what `modify_schedule_for_sidecar_compute_boundaries` did, by
/// reading the post-mutation schedule. Used for UX reporting to users
pub fn sidecar_compute_boundary_report(
    schedule: &Schedule<String>,
    nodes: &Nodes,
) -> ComputeBoundaryReport {
    use dbt_common::io_args::ComputeArg;

    let mut boundary_nodes = BTreeSet::new();
    let mut skipped_test_nodes = BTreeSet::new();

    for uid in schedule
        .all_selected_nodes
        .difference(&schedule.selected_nodes)
    {
        let Some(node) = nodes.get_node(uid) else {
            continue;
        };

        // find what got removed from selected_nodes as part of sidecar's schedule modification
        match node.resource_type() {
            NodeType::Model | NodeType::Snapshot => {
                if schedule.frontier_nodes.contains(uid)
                    && node.compute() == Some(ComputeArg::Remote)
                {
                    boundary_nodes.insert(uid.clone());
                }
            }
            NodeType::Test | NodeType::UnitTest => {
                skipped_test_nodes.insert(uid.clone());
            }
            _ => {}
        }
    }

    ComputeBoundaryReport {
        boundary_nodes,
        skipped_test_nodes,
    }
}

fn get_frontier_nodes(
    selected_nodes: &BTreeSet<String>,
    deps: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut frontier = BTreeSet::new();
    // compute frontier (over-approximation)
    for node_id in selected_nodes {
        if let Some(node_deps) = deps.get(node_id) {
            // For "unit_test.", add its dependencies' dependencies
            if node_id.starts_with("unit_test.") {
                let dependency = node_deps.first().unwrap();
                if let Some(dependency_deps) = deps.get(dependency) {
                    for dep_dep in dependency_deps {
                        if selected_nodes.contains(dep_dep) {
                            continue;
                        }
                        frontier.insert(dep_dep.clone());
                    }
                }
            }
            for dependency in node_deps {
                // if the dependency is already in the selected_nodes, skip it
                if selected_nodes.contains(dependency) {
                    continue;
                }
                // Metric and Semantic Model nodes sit in the middle between SavedQuery and Model
                // so should not be considered frontier nodes
                if dependency.starts_with("metric.") || dependency.starts_with("semantic_model.") {
                    continue;
                }

                frontier.insert(dependency.clone());
            }
        }
    }
    frontier
}

/// Expands the selection based on indirect selection mode and returns both directly and indirectly selected nodes.
///
/// # Arguments
/// * `selected` - Initially selected nodes
/// * `indirect_selection` - Mode determining how indirect selections are handled:
///   - Eager: Most inclusive, selects all related tests
///   - Cautious: Most exclusive, only selects tests where all parents are selected
///   - Buildable: Middle ground, considers ancestor relationships
///   - Empty: No indirect test selection
/// * `deps` - Dependency graph
/// * `nodes` - All available nodes
///
/// # Returns
/// * `(BTreeSet<String>, BTreeSet<String>)` - (directly selected nodes, indirectly selected nodes)
fn expand_selection(
    selected: &BTreeSet<String>,
    indirect_selection: IndirectSelection,
    deps: &BTreeMap<String, BTreeSet<String>>,
    nodes: &Nodes,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut direct_nodes = selected.clone();
    let mut indirect_nodes = BTreeSet::new();

    // For BUILDABLE mode, get selected and all parents
    let selected_and_parents = if indirect_selection == IndirectSelection::Buildable {
        // One traversal over the shared ancestry rather than one per selected node
        // (dbt-labs/fs#11359). `multi_source_bfs` includes its seeds.
        let mut all = multi_source_bfs(deps, selected, u32::MAX);
        // Add all sources
        all.extend(nodes.sources.keys().cloned());
        all
    } else {
        BTreeSet::new()
    };

    // Get all nodes that depend on selected nodes (successors)
    let rev_deps = reverse(deps);
    let mut successors = BTreeSet::new();
    for node in selected {
        if let Some(deps) = rev_deps.get(node) {
            successors.extend(deps.iter().cloned());
        }
    }

    // Then process each successor for regular tests
    for node_id in successors {
        // Only process test nodes
        let node_deps = if let Some(unit_test) = nodes.unit_tests.get(&node_id) {
            BTreeSet::from([unit_test
                .base()
                .depends_on
                .nodes_with_ref_location
                .first()
                .unwrap()
                .0
                .clone()])
        } else if let Some(test) = nodes.tests.get(&node_id) {
            test.base()
                .depends_on
                .nodes_with_ref_location
                .iter()
                .map(|(dep, _)| dep.clone())
                .collect::<BTreeSet<_>>()
        } else {
            continue;
        };

        // EMPTY mode: skip all indirect test selection
        if indirect_selection == IndirectSelection::Empty {
            continue;
        }

        if indirect_selection == IndirectSelection::Eager
            || node_deps.is_subset(selected)
            || (indirect_selection == IndirectSelection::Buildable
                && node_deps.is_subset(&selected_and_parents))
        {
            direct_nodes.insert(node_id.clone());
        } else {
            indirect_nodes.insert(node_id.clone());
        }
    }

    (direct_nodes, indirect_nodes)
}

/// Incorporate indirect nodes into the selection based on the selection mode.
fn incorporate_indirect_nodes(
    expanded_nodes: BTreeSet<String>,
    indirect_nodes: BTreeSet<String>,
    deps: &BTreeMap<String, BTreeSet<String>>,
    nodes: &Nodes,
    indirect_selection: IndirectSelection,
) -> BTreeSet<String> {
    if expanded_nodes == indirect_nodes {
        return expanded_nodes;
    }

    let mut selected = expanded_nodes;

    match indirect_selection {
        IndirectSelection::Cautious => {
            for node_id in indirect_nodes {
                if let Some(node) = nodes.tests.get(&node_id) {
                    let deps: BTreeSet<String> = node
                        .base()
                        .depends_on
                        .nodes_with_ref_location
                        .iter()
                        .map(|(dep, _)| dep.clone())
                        .collect();
                    if deps.is_subset(&selected) {
                        selected.insert(node_id.clone());
                    }
                }
            }
        }
        IndirectSelection::Buildable => {
            // One traversal over the shared ancestry rather than one per selected node
            // (dbt-labs/fs#11359). `multi_source_bfs` includes its seeds.
            let selected_and_parents = multi_source_bfs(deps, &selected, u32::MAX);

            for node_id in indirect_nodes {
                if let Some(node) = nodes.tests.get(&node_id) {
                    let deps: BTreeSet<String> = node
                        .base()
                        .depends_on
                        .nodes_with_ref_location
                        .iter()
                        .map(|(dep, _)| dep.clone())
                        .collect();
                    if deps.is_subset(&selected_and_parents) {
                        selected.insert(node_id.clone());
                    }
                }
            }
        }
        _ => {} // For EAGER and EMPTY, just return the expanded nodes
    }

    selected
}

/// The `@` operator: every descendant of `selected_nodes`, plus every ancestor of those
/// descendants.
///
/// Three multi-source traversals, each O(V+E). The previous implementation reversed the
/// whole graph once per selected node and then ran an independent single-source ancestor
/// walk per leaf, which made `--select @model` take minutes (dbt-labs/fs#11359).
fn collect_childrens_parents(
    deps: &BTreeMap<String, BTreeSet<String>>,
    selected_nodes: &BTreeSet<String>,
) -> BTreeSet<String> {
    // `deps` maps a node to its parents; reverse it once to walk downwards.
    let children = reverse(deps);

    // 1. Every descendant of the selection. `multi_source_bfs` includes its seeds, so a
    //    selected node with no children is retained rather than dropped (see #15280).
    let descendants = multi_source_bfs(&children, selected_nodes, u32::MAX);

    // 2. The leaves of that subgraph. Every child of a descendant is itself a descendant,
    //    so "no children within the descendant subgraph" is just "no children at all".
    let leaf_nodes: BTreeSet<String> = descendants
        .iter()
        .filter(|node| children.get(*node).is_none_or(|kids| kids.is_empty()))
        .cloned()
        .collect();

    // 3. Every leaf plus all of its ancestors, in one pass over the shared ancestry rather
    //    than one pass per leaf. Seeds are included, so the leaves themselves are kept.
    multi_source_bfs(deps, &leaf_nodes, u32::MAX)
}

/// Multi-source BFS: starting from every node in `seeds`, follow edges in
/// `graph` for up to `max_depth` hops.  Returns all discovered nodes
/// (seeds included).
///
/// Unlike repeated single-source BFS, this traverses each edge at most once
/// regardless of how many seeds share common ancestors / descendants.
fn multi_source_bfs(
    graph: &BTreeMap<String, BTreeSet<String>>,
    seeds: &BTreeSet<String>,
    max_depth: u32,
) -> BTreeSet<String> {
    let mut visited: BTreeSet<String> = seeds.clone();
    if max_depth == 0 {
        return visited;
    }

    let mut frontier: Vec<String> = seeds.iter().cloned().collect();
    let mut depth = 0;

    while !frontier.is_empty() && depth < max_depth {
        depth += 1;
        let mut next = Vec::new();
        for node in &frontier {
            if let Some(neighbors) = graph.get(node) {
                for neighbor in neighbors {
                    if visited.insert(neighbor.clone()) {
                        next.push(neighbor.clone());
                    }
                }
            }
        }
        frontier = next;
    }
    visited
}

/// Summarize statistics for scheduled nodes.
///
/// # Arguments
/// * `schedule` - The node schedule
/// * `stats` - Statistics for each node
///
/// # Returns
/// * `Vec<Stat>` - Statistics for scheduled nodes in execution order
pub fn summarize_stats(schedule: &Schedule<String>, stats: &DashMap<String, Stat>) -> Vec<Stat> {
    schedule
        .sorted_nodes
        .iter()
        .filter_map(|node| stats.get(node))
        .map(|stat| stat.clone())
        .collect()
}

// ------------------------------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use dbt_adapter_core::AdapterType;
    use dbt_common::cancellation::never_cancels;
    use dbt_common::io_args::{FsCommand, IoArgs};
    use dbt_common::node_selector::SelectionCriteria;
    use dbt_common::{
        CodeLocationWithFile,
        io_args::StaticAnalysisKind,
        node_selector::{MethodName, SelectExpression, parse_model_specifiers},
    };
    use dbt_dag::deps_mgmt::ensure_all_nodes_defined;
    use dbt_schemas::schemas::manifest::metric::{DbtMetric, DbtMetricAttr};
    use dbt_schemas::schemas::manifest::saved_query::{
        DbtSavedQuery, DbtSavedQueryAttr, SavedQueryParams,
    };
    use dbt_schemas::schemas::manifest::semantic_model::{DbtSemanticModel, DbtSemanticModelAttr};
    use dbt_schemas::schemas::project::{MetricConfig, SavedQueryConfig, SemanticModelConfig};
    use dbt_schemas::schemas::{
        CommonAttributes, DbtModel, DbtModelAttr, DbtTest, DbtTestAttr, DbtUnitTest,
        DbtUnitTestAttr, IntrospectionKind, NodeBaseAttributes,
        common::{Access, DbtMaterialization, Expect, NodeDependsOn, ResolvedQuoting},
        nodes::AdapterAttr,
        project::ModelConfig,
    };
    use indexmap::IndexMap;

    use super::*;
    use std::sync::Arc;

    /// Helper struct to build test graphs and nodes
    pub struct TestGraphBuilder {
        edges: Vec<(String, String)>,
        nodes: Option<Nodes>,
    }

    impl TestGraphBuilder {
        /// Create a new TestGraphBuilder
        pub fn new() -> Self {
            Self {
                edges: Vec::new(),
                nodes: None,
            }
        }

        /// Add an edge to the graph
        pub fn add_edge(mut self, from: &str, to: &str) -> Self {
            self.edges.push((from.to_string(), to.to_string()));
            self
        }

        /// Add multiple edges to the graph
        #[allow(dead_code)]
        pub fn add_edges(mut self, edges: Vec<(&str, &str)>) -> Self {
            self.edges.extend(
                edges
                    .into_iter()
                    .map(|(f, t)| (f.to_string(), t.to_string())),
            );
            self
        }

        /// Build the graph
        pub fn build_graph(&self) -> BTreeMap<String, BTreeSet<String>> {
            let mut graph = BTreeMap::new();
            for (from, to) in &self.edges {
                graph
                    .entry(from.clone())
                    .or_insert_with(BTreeSet::new)
                    .insert(to.clone());
            }
            graph
        }

        /// Build nodes from the graph
        pub fn build_nodes(&mut self) -> Nodes {
            let graph = self.build_graph();

            // First, collect all unique node IDs
            let mut unique_ids = BTreeSet::new();
            for (from, to_set) in &graph {
                unique_ids.insert(from.clone());
                unique_ids.extend(to_set.iter().cloned());
            }

            // Create reverse graph for dependencies
            let mut reverse_graph = BTreeMap::new();
            for (from, to_set) in &graph {
                for to in to_set {
                    reverse_graph
                        .entry(to.clone())
                        .or_insert_with(BTreeSet::new)
                        .insert(from.clone());
                }
            }

            let models = unique_ids
                .iter()
                .filter(|id| !id.starts_with("test.") && !id.starts_with("unit_test."))
                .map(|unique_id| DbtModel {
                    __common_attr__: CommonAttributes {
                        name: unique_id.clone(),
                        package_name: "package".to_string(),
                        unique_id: unique_id.clone(),
                        tags: vec![],
                        meta: IndexMap::new(),
                        ..Default::default()
                    },
                    __base_attr__: NodeBaseAttributes {
                        static_analysis: StaticAnalysisKind::Strict.into(),
                        depends_on: NodeDependsOn {
                            nodes_with_ref_location: Vec::new(),
                            ..Default::default()
                        },
                        database: "db".to_string(),
                        schema: "schema".to_string(),
                        materialized: DbtMaterialization::View,
                        quoting: ResolvedQuoting::trues(),
                        enabled: true,
                        extended_model: false,
                        ..Default::default()
                    },
                    __model_attr__: DbtModelAttr {
                        introspection: IntrospectionKind::None,
                        version: None,
                        latest_version: None,
                        constraints: vec![],
                        access: Access::Protected,
                        group: None,
                        time_spine: None,
                        deprecation_date: None,
                        contract: None,
                        incremental_strategy: None,
                        freshness: None,
                        state: None,
                        primary_key: vec![],
                        event_time: None,
                        catalog_name: None,
                        table_format: None,
                        sync: None,
                        compiled_code: None,
                    },
                    __adapter_attr__: AdapterAttr::default(),
                    deprecated_config: ModelConfig::default(),
                    __other__: BTreeMap::new(),
                })
                .map(|model| (model.__common_attr__.unique_id.clone(), Arc::new(model)))
                .collect();

            let tests = unique_ids
                .iter()
                .filter(|id| id.starts_with("test.") && !id.starts_with("unit_test."))
                .map(|unique_id| {
                    let deps = reverse_graph.get(unique_id).cloned().unwrap_or_default();
                    let depends_on = NodeDependsOn {
                        nodes_with_ref_location: deps
                            .iter()
                            .map(|dep| (dep.clone(), CodeLocationWithFile::default()))
                            .collect(),
                        ..Default::default()
                    };

                    let base_attr = NodeBaseAttributes {
                        depends_on,
                        database: "db".to_string(),
                        schema: "schema".to_string(),
                        quoting: ResolvedQuoting::trues(),
                        static_analysis: StaticAnalysisKind::Strict.into(),
                        enabled: true,
                        ..Default::default()
                    };

                    DbtTest {
                        defined_at: None,
                        manifest_original_file_path: "tests/test.sql".into(),
                        __common_attr__: CommonAttributes {
                            name: unique_id.clone(),
                            package_name: "package".to_string(),
                            unique_id: unique_id.clone(),
                            tags: vec![],
                            meta: IndexMap::new(),
                            ..Default::default()
                        },
                        __base_attr__: base_attr,
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
                    }
                })
                .map(|test| (test.__common_attr__.unique_id.clone(), Arc::new(test)))
                .collect();

            let unit_tests = unique_ids
                .iter()
                .filter(|id| id.starts_with("unit_test."))
                .map(|unique_id| {
                    let deps = reverse_graph.get(unique_id).cloned().unwrap_or_default();
                    let model_name = deps
                        .iter()
                        .next()
                        .cloned()
                        .unwrap_or_else(|| "model.unknown".to_string());

                    let depends_on = NodeDependsOn {
                        nodes_with_ref_location: deps
                            .iter()
                            .map(|dep| (dep.clone(), CodeLocationWithFile::default()))
                            .collect(),
                        ..Default::default()
                    };

                    DbtUnitTest {
                        __common_attr__: CommonAttributes {
                            name: unique_id.clone(),
                            package_name: "package".to_string(),
                            unique_id: unique_id.clone(),
                            tags: vec![],
                            meta: IndexMap::new(),
                            ..Default::default()
                        },
                        __base_attr__: NodeBaseAttributes {
                            depends_on,
                            database: "db".to_string(),
                            schema: "schema".to_string(),
                            quoting: ResolvedQuoting::trues(),
                            static_analysis: StaticAnalysisKind::Strict.into(),
                            enabled: true,
                            ..Default::default()
                        },
                        __unit_test_attr__: DbtUnitTestAttr {
                            model: model_name,
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
                        deprecated_config: Default::default(),
                        ..Default::default()
                    }
                })
                .map(|test| (test.__common_attr__.unique_id.clone(), Arc::new(test)))
                .collect();

            let nodes = Nodes {
                models,
                tests,
                unit_tests,
                ..Default::default()
            };
            self.nodes = Some(nodes.clone());
            nodes
        }

        /// Get the built nodes
        #[allow(dead_code)]
        pub fn get_nodes(&self) -> Option<&Nodes> {
            self.nodes.as_ref()
        }
    }

    /// Helper function to create a select expression from a string
    pub(super) fn create_select_expression(expr: &str) -> SelectExpression {
        let tokens = expr
            .split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        parse_model_specifiers(&tokens).unwrap()
    }

    /// Helper function to schedule a test graph with selection
    fn schedule_test_graph(
        graph: &BTreeMap<String, BTreeSet<String>>,
        select: &SelectExpression,
    ) -> Vec<String> {
        let completed_deps = ensure_all_nodes_defined(graph);
        let deps = ensure_all_nodes_defined(&reverse(&completed_deps));

        // Create nodes using the builder
        let mut builder = TestGraphBuilder::new();
        builder.edges = graph
            .iter()
            .flat_map(|(from, to_set)| {
                to_set
                    .iter()
                    .map(|to| (from.clone(), to.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        let nodes = builder.build_nodes();

        let resolved_selectors = ResolvedSelector {
            include: Some(select.clone()),
            exclude: None,
            ..Default::default()
        };

        let args = SchedulerArgs {
            command: FsCommand::Test,
            io: Default::default(),
            resource_types: vec![],
            exclude_resource_types: vec![],
            exclude_unique_ids: Default::default(),
            selection_override: None,
        };
        match schedule_graph(
            &deps,
            &nodes,
            None,
            &resolved_selectors,
            None,
            &args,
            AdapterType::Bigquery,
        ) {
            Ok((schedule, _)) => schedule
                .sorted_nodes
                .iter()
                .filter(|node| !schedule.frontier_nodes.contains(*node))
                .cloned()
                .collect(),
            Err(_) => vec![],
        }
    }

    /// Helper function to schedule a test graph with both select and exclude expressions
    fn schedule_graph_with_exclude(
        graph: &BTreeMap<String, BTreeSet<String>>,
        select: &SelectExpression,
        exclude: &SelectExpression,
    ) -> Vec<String> {
        let completed_deps = ensure_all_nodes_defined(graph);
        let deps = ensure_all_nodes_defined(&reverse(&completed_deps));

        // Create nodes using the builder
        let mut builder = TestGraphBuilder::new();
        builder.edges = graph
            .iter()
            .flat_map(|(from, to_set)| {
                to_set
                    .iter()
                    .map(|to| (from.clone(), to.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        let nodes = builder.build_nodes();

        let resolved_selectors = ResolvedSelector {
            include: Some(select.clone()),
            exclude: Some(exclude.clone()),
            ..Default::default()
        };

        let args = SchedulerArgs {
            command: FsCommand::Test,
            io: Default::default(),
            resource_types: vec![],
            exclude_resource_types: vec![],
            exclude_unique_ids: Default::default(),
            selection_override: None,
        };
        match schedule_graph(
            &deps,
            &nodes,
            None,
            &resolved_selectors,
            None,
            &args,
            AdapterType::Bigquery,
        ) {
            Ok((schedule, _)) => schedule
                .sorted_nodes
                .iter()
                .filter(|node| !schedule.frontier_nodes.contains(*node))
                .cloned()
                .collect(),
            Err(_) => vec![],
        }
    }

    #[test]
    fn test_schedule_graph_simple() {
        let builder = TestGraphBuilder::new().add_edge("a", "b");
        let graph = builder.build_graph();
        let select = create_select_expression("a");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["a"]);
    }

    #[test]
    fn test_schedule_graph_chain() {
        let builder = TestGraphBuilder::new()
            .add_edge("a", "b")
            .add_edge("b", "c");
        let graph = builder.build_graph();
        let select = create_select_expression("b+");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["b", "c"]);

        let select = create_select_expression("+b");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["a", "b"]);

        // Validate this behavior: https://docs.getdbt.com/reference/node-selection/graph-operators#the-at-operator
        let select = create_select_expression("@b");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_schedule_graph_at_operator_on_leaf() {
        // Regression test for https://github.com/dbt-labs/dbt-core/issues/15280
        // `@` on a leaf node (one with no children) should return the node plus
        // its ancestors. A node is its own descendant, so for a leaf the
        // descendant set is just the node itself.
        let builder = TestGraphBuilder::new()
            .add_edge("a", "b")
            .add_edge("b", "c");
        let graph = builder.build_graph();

        // `c` is the leaf of the chain a -> b -> c.
        let select = create_select_expression("@c");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["a", "b", "c"]);

        // A leaf with a single ancestor.
        let builder = TestGraphBuilder::new().add_edge("a", "b");
        let graph = builder.build_graph();
        let select = create_select_expression("@b");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["a", "b"]);
    }

    #[test]
    fn test_collect_childrens_parents_leaf_and_isolated() {
        // `deps` maps each node to its parents (upstream dependencies).
        // Chain a -> b -> c.
        let deps = BTreeMap::from([
            ("a".to_string(), BTreeSet::new()),
            ("b".to_string(), BTreeSet::from(["a".to_string()])),
            ("c".to_string(), BTreeSet::from(["b".to_string()])),
        ]);

        // `@c` on the leaf `c` -> the leaf plus all of its ancestors.
        let selected = BTreeSet::from(["c".to_string()]);
        let result = collect_childrens_parents(&deps, &selected);
        assert_eq!(
            result,
            BTreeSet::from(["a".to_string(), "b".to_string(), "c".to_string()])
        );

        // An isolated node (leaf with no ancestors) -> just itself.
        let deps = BTreeMap::from([("a".to_string(), BTreeSet::new())]);
        let selected = BTreeSet::from(["a".to_string()]);
        let result = collect_childrens_parents(&deps, &selected);
        assert_eq!(result, BTreeSet::from(["a".to_string()]));
    }

    #[test]
    fn test_collect_childrens_parents_diamond_counts_each_node_once() {
        // A diamond: `top` reaches `bottom` by two distinct paths. The old
        // implementation walked each path separately, which produced the right answer
        // but did redundant work; this pins the answer so the rewrite is equivalent.
        //
        //     top -> left  -> bottom
        //     top -> right -> bottom
        let deps = BTreeMap::from([
            ("top".to_string(), BTreeSet::new()),
            ("left".to_string(), BTreeSet::from(["top".to_string()])),
            ("right".to_string(), BTreeSet::from(["top".to_string()])),
            (
                "bottom".to_string(),
                BTreeSet::from(["left".to_string(), "right".to_string()]),
            ),
        ]);

        // `@top` -> descendants are everything; the only leaf is `bottom`; its ancestors
        // are the rest.
        let result = collect_childrens_parents(&deps, &BTreeSet::from(["top".to_string()]));
        assert_eq!(
            result,
            BTreeSet::from([
                "top".to_string(),
                "left".to_string(),
                "right".to_string(),
                "bottom".to_string(),
            ])
        );

        // `@left` -> descendants {left, bottom}; leaf `bottom`; its ancestors pull in
        // `right` and `top`, which is the whole point of the `@` operator.
        let result = collect_childrens_parents(&deps, &BTreeSet::from(["left".to_string()]));
        assert_eq!(
            result,
            BTreeSet::from([
                "top".to_string(),
                "left".to_string(),
                "right".to_string(),
                "bottom".to_string(),
            ])
        );
    }

    #[test]
    fn test_collect_childrens_parents_is_linear_on_deep_multi_path_dag() {
        // Regression guard for dbt-labs/fs#11359.
        //
        // A "diamond ladder": every layer has two nodes, each depending on both nodes of
        // the layer above, so the number of distinct top-to-bottom paths is 2^depth.
        // The old code queued a node once per path reaching it, and ran one independent
        // ancestor walk per leaf, making it O(paths) rather than O(V+E).
        //
        // The budget below is deliberately enormous relative to the real cost: this graph
        // has 2 * DEPTH + 1 = 53 nodes, so the fixed traversal finishes in microseconds,
        // while the old one needs ~2^26 frontier entries (measured at ~76s in a debug
        // build). Anything in between means the traversal has regressed to per-path or
        // per-leaf work. The assertion exists so that regression fails the suite instead
        // of merely hanging -- nextest's default profile flags slow tests but does not
        // terminate them.
        const DEPTH: usize = 26;
        const BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

        let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        deps.insert("root".to_string(), BTreeSet::new());
        let mut prev = vec!["root".to_string()];
        for layer in 0..DEPTH {
            let cur: Vec<String> = (0..2).map(|i| format!("n{layer}_{i}")).collect();
            for node in &cur {
                deps.insert(node.clone(), prev.iter().cloned().collect());
            }
            prev = cur;
        }

        let started = std::time::Instant::now();
        let result = collect_childrens_parents(&deps, &BTreeSet::from(["root".to_string()]));
        let elapsed = started.elapsed();

        // Everything is both a descendant of `root` and an ancestor of a bottom leaf.
        assert_eq!(result.len(), deps.len(), "every node should be selected");
        assert!(
            elapsed < BUDGET,
            "collect_childrens_parents took {elapsed:?} on a {}-node DAG; the traversal is \
             no longer linear in the graph size (see dbt-labs/fs#11359)",
            deps.len()
        );
        assert!(result.contains("root"));
        assert!(result.contains(&format!("n{}_0", DEPTH - 1)));
    }

    #[test]
    fn test_schedule_graph_select_plus_c() {
        let builder = TestGraphBuilder::new()
            .add_edge("a", "b")
            .add_edge("b", "c");
        let graph = builder.build_graph();
        let select = create_select_expression("+c");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["a", "b", "c"]);

        let select = create_select_expression("a+");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["a", "b", "c"]);

        let select = create_select_expression("+b+");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_schedule_graph_select_plus_b_plus_exclude_a() {
        let builder = TestGraphBuilder::new()
            .add_edge("a", "b")
            .add_edge("b", "c");
        let graph = builder.build_graph();
        let select = create_select_expression("+b+");
        let exclude = create_select_expression("a");
        let result = schedule_graph_with_exclude(&graph, &select, &exclude);
        assert_eq!(result, vec!["b", "c"]);
    }

    #[test]
    fn test_greedy_test_exclusion() {
        // Tests the dbt-core semantics: "Test exclusion is always greedy:
        // if ANY parent is explicitly excluded, the test will be excluded as well."
        //
        // Graph structure:
        //   model.b -> model.a -> test.test_a
        //                      \
        //   model.b -----------> test.test_b
        //
        // Select: +model.a (gets model.a, model.b, test.test_a, test.test_b via eager indirect)
        // Exclude: model.a
        // Expected: model.b, test.test_b (NOT test.test_a because model.a is excluded)
        let builder = TestGraphBuilder::new()
            .add_edge("model.b", "model.a")
            .add_edge("model.a", "test.test_a")
            .add_edge("model.b", "test.test_b");
        let graph = builder.build_graph();

        let select = create_select_expression("+model.a");
        let exclude = create_select_expression("model.a");
        let result = schedule_graph_with_exclude(&graph, &select, &exclude);

        // test.test_a should NOT be in the result because its parent model.a was excluded
        // test.test_b should remain because model.b is still selected
        assert_eq!(result, vec!["model.b", "test.test_b"]);
    }

    #[test]
    fn test_greedy_test_exclusion_unit_tests() {
        // Same test for unit tests
        //
        // Graph structure:
        //   model.b -> model.a -> unit_test.test_a
        //
        // Select: +model.a (gets model.a, model.b, unit_test.test_a)
        // Exclude: model.a
        // Expected: model.b only (NOT unit_test.test_a because model.a is excluded)
        let builder = TestGraphBuilder::new()
            .add_edge("model.b", "model.a")
            .add_edge("model.a", "unit_test.test_a");
        let graph = builder.build_graph();

        let select = create_select_expression("+model.a");
        let exclude = create_select_expression("model.a");
        let result = schedule_graph_with_exclude(&graph, &select, &exclude);

        // unit_test.test_a should NOT be in the result because its parent model.a was excluded
        assert_eq!(result, vec!["model.b"]);
    }

    #[test]
    fn test_greedy_test_exclusion_multi_dep_test() {
        // Test that a test with multiple dependencies is excluded if ANY parent is excluded
        //
        // Graph structure:
        //   model.a -----> test.relationship_test
        //   model.b -----> test.relationship_test
        //
        // Select: model.a model.b (both models and the test via eager indirect)
        // Exclude: model.a
        // Expected: model.b only (NOT test.relationship_test because model.a is excluded)
        let builder = TestGraphBuilder::new()
            .add_edge("model.a", "test.relationship_test")
            .add_edge("model.b", "test.relationship_test");
        let graph = builder.build_graph();

        let select = create_select_expression("model.a model.b");
        let exclude = create_select_expression("model.a");
        let result = schedule_graph_with_exclude(&graph, &select, &exclude);

        // test.relationship_test should NOT be in the result because model.a (one of its deps) was excluded
        assert_eq!(result, vec!["model.b"]);
    }

    #[test]
    fn test_no_greedy_exclusion_when_no_excludes() {
        // Verify that tests remain when there's no exclude
        //
        // Graph structure:
        //   model.a -> test.test_a
        //
        // Select: model.a
        // Exclude: (none)
        // Expected: model.a, test.test_a
        let builder = TestGraphBuilder::new().add_edge("model.a", "test.test_a");
        let graph = builder.build_graph();

        let select = create_select_expression("model.a");
        let result = schedule_test_graph(&graph, &select);

        // Both model and test should be selected when there's no exclude
        assert_eq!(result, vec!["model.a", "test.test_a"]);
    }

    #[test]
    fn test_n_degree_upstream_walking() {
        // Create a chain: a -> b -> c -> d -> e
        let builder = TestGraphBuilder::new()
            .add_edge("a", "b")
            .add_edge("b", "c")
            .add_edge("c", "d")
            .add_edge("d", "e");
        let graph = builder.build_graph();

        // Test depth 1 from 'c'
        let select = create_select_expression("1+c");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["b", "c"]);

        let select = create_select_expression("2+c");
        // Test depth 2 from 'c'
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["a", "b", "c"]);

        // Test depth 3 from 'e'
        let select = create_select_expression("3+e");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["b", "c", "d", "e"]);

        let select = create_select_expression("+e");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn test_n_degree_downstream_walking() {
        // Create a chain: a -> b -> c -> d -> e
        let builder = TestGraphBuilder::new()
            .add_edge("a", "b")
            .add_edge("b", "c")
            .add_edge("c", "d")
            .add_edge("d", "e");
        let graph = builder.build_graph();

        // Test depth 1 from 'c'
        let select = create_select_expression("c+1");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["c", "d"]);

        // Test depth 2 from 'c'
        let select = create_select_expression("c+2");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["c", "d", "e"]);

        // Test depth 3 from 'a'
        let select = create_select_expression("a+3");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["a", "b", "c", "d"]);

        let select = create_select_expression("a+");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn test_n_degree_walking_with_branches() {
        // Create a graph with branches:
        // a -> b -> c -> d
        //      b -> e -> f
        let builder = TestGraphBuilder::new()
            .add_edge("a", "b")
            .add_edge("b", "c")
            .add_edge("c", "d")
            .add_edge("b", "e")
            .add_edge("e", "f");
        let graph = builder.build_graph();

        // Test upstream from 'c' with depth 2
        let select = create_select_expression("2+c");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["a", "b", "c"]);

        // Test downstream from 'b' with depth 2
        let select = create_select_expression("b+2");
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(result, vec!["b", "c", "d", "e", "f"]);
    }

    #[test]
    fn test_indirect_selection_eager() {
        // Create a graph with a model and its test
        // model.a -> test.test_a
        let builder = TestGraphBuilder::new().add_edge("model.a", "test.test_a");
        let graph = builder.build_graph();

        // Select model.a with eager indirect selection
        let select = create_select_expression("model.a");
        let result = schedule_test_graph(&graph, &select);

        // Both model and test should be selected in eager mode
        assert_eq!(result, vec!["model.a", "test.test_a"]);
    }

    #[test]
    fn test_and_selector_with_empty_left_side_does_not_fall_through() {
        // Regression: `a,b` is an AND. If `a` selects nothing, the whole expression must select nothing.
        //
        // This mirrors the replay bug where `tag:...` matched nothing, but `source:*` matched something
        // and (incorrectly) won the AND.
        let builder = TestGraphBuilder::new().add_edge("model.a", "test.test_a");
        let graph = builder.build_graph();

        // tag:nope matches nothing because the test nodes have empty tags in TestGraphBuilder.
        let select = create_select_expression("tag:nope,model.a");
        let result = schedule_test_graph(&graph, &select);
        assert!(result.is_empty(), "expected AND selector to yield no nodes");
    }

    #[test]
    fn test_indirect_selection_empty() {
        // Create a graph with a model and its test
        let builder = TestGraphBuilder::new().add_edge("model.a", "test.test_a");
        let graph = builder.build_graph();

        // Create selection with empty indirect selection
        let criteria = SelectionCriteria::new(
            MethodName::Fqn,
            vec![],
            "model.a".to_string(),
            false,
            None,
            None,
            Some(IndirectSelection::Empty),
            None,
        );
        let select = SelectExpression::Atom(criteria);

        let result = schedule_test_graph(&graph, &select);

        // In empty mode, NO tests should be indirectly selected - only the model
        assert_eq!(result, vec!["model.a"]);
    }

    #[test]
    fn test_indirect_selection_cautious() {
        // Create a graph with models and tests:
        // model.a -> test.test_a
        // model.b -> test.test_b
        // model.a -> model.b
        let builder = TestGraphBuilder::new()
            .add_edge("model.a", "test.test_a")
            .add_edge("model.b", "test.test_b")
            .add_edge("model.a", "model.b");
        let graph = builder.build_graph();

        // Create selection with cautious indirect selection
        let criteria = SelectionCriteria::new(
            MethodName::Fqn,
            vec![],
            "model.a".to_string(),
            false,
            None,
            None,
            Some(IndirectSelection::Cautious),
            None,
        );
        let select = SelectExpression::Atom(criteria);

        let result = schedule_test_graph(&graph, &select);

        // test.test_a should be selected (depends only on selected model.a)
        // test.test_b should not be selected (depends on unselected model.b)
        assert_eq!(result, vec!["model.a", "test.test_a"]);
    }

    #[test]
    fn test_indirect_selection_buildable() {
        // Create a graph with models and tests that validate relationships between models and their ancestors:
        // model.source -> model.intermediate -> model.final
        // model.intermediate, model.source -> test.totals_match
        // model.final -> test.final_test
        let builder = TestGraphBuilder::new()
            .add_edge("model.source", "model.intermediate")
            .add_edge("model.intermediate", "model.final")
            .add_edge("model.intermediate", "test.totals_match")
            .add_edge("model.source", "test.totals_match")
            .add_edge("model.final", "test.final_test");
        let graph = builder.build_graph();

        // Create selection with buildable indirect selection - select the intermediate model
        let criteria = SelectionCriteria::new(
            MethodName::Fqn,
            vec![],
            "model.intermediate".to_string(),
            false,
            None,
            None,
            Some(IndirectSelection::Buildable),
            None,
        );
        let select = SelectExpression::Atom(criteria);

        let result = schedule_test_graph(&graph, &select);

        // test.totals_match should be selected because:
        // - it depends on model.intermediate (selected) and model.source (ancestor of selected)
        // test.final_test should NOT be selected because:
        // - it depends on model.final which is neither selected nor an ancestor
        assert_eq!(result, vec!["model.intermediate", "test.totals_match"]);
    }

    #[test]
    fn test_indirect_selection_buildable_complex() {
        // Create a more complex graph with multiple test dependencies and branching paths:
        // model.source_a -> model.intermediate_a -> model.final_a
        //        |                    |                  |
        //        v                    v                  v
        // test.source_test    test.intermediate_test   test.final_test
        //        |                    |                  |
        //        v                    v                  v
        // model.source_b -> model.intermediate_b -> model.final_b
        let builder = TestGraphBuilder::new()
            .add_edge("model.source_a", "model.intermediate_a")
            .add_edge("model.intermediate_a", "model.final_a")
            .add_edge("model.source_a", "test.source_test")
            .add_edge("model.intermediate_a", "test.intermediate_test")
            .add_edge("model.final_a", "test.final_test")
            .add_edge("test.source_test", "model.source_b")
            .add_edge("test.intermediate_test", "model.intermediate_b")
            .add_edge("test.final_test", "model.final_b")
            .add_edge("model.source_b", "model.intermediate_b")
            .add_edge("model.intermediate_b", "model.final_b");
        let graph = builder.build_graph();

        // Test Case 1: Select intermediate_a - should only include tests that are successors
        // (not tests of ancestors, to match DBT behavior)
        let criteria = SelectionCriteria::new(
            MethodName::Fqn,
            vec![],
            "model.intermediate_a".to_string(),
            false,
            None,
            None,
            Some(IndirectSelection::Buildable),
            None,
        );
        let select = SelectExpression::Atom(criteria);
        let result = schedule_test_graph(&graph, &select);
        assert_eq!(
            result,
            vec!["model.intermediate_a", "test.intermediate_test"]
        );
    }

    #[test]
    fn test_indirect_selection_buildable_with_unit_tests() {
        // Create a graph with both regular and unit tests:
        // model.a -> test.integration_test_a
        //        -> unit_test.unit_test_a
        //        -> model.b -> test.integration_test_b
        //                   -> unit_test.unit_test_b
        let builder = TestGraphBuilder::new()
            .add_edge("model.a", "test.integration_test_a")
            .add_edge("model.a", "unit_test.unit_test_a")
            .add_edge("model.a", "model.b")
            .add_edge("model.b", "test.integration_test_b")
            .add_edge("model.b", "unit_test.unit_test_b");
        let graph = builder.build_graph();

        // Select model.a with buildable mode
        let criteria = SelectionCriteria::new(
            MethodName::Fqn,
            vec![],
            "model.a".to_string(),
            false,
            None,
            None,
            Some(IndirectSelection::Buildable),
            None,
        );
        let select = SelectExpression::Atom(criteria);
        let result = schedule_test_graph(&graph, &select);

        // Should include both unit and integration tests for model.a
        assert_eq!(
            result,
            vec![
                "model.a",
                "test.integration_test_a",
                "unit_test.unit_test_a"
            ]
        );
    }

    #[test]
    fn test_indirect_selection_buildable_multiple_selections() {
        // Create a graph with multiple test dependencies:
        // model.a -> test.test_ab <- model.b
        //        -> test.test_a
        //        -> test.test_abc <- model.c
        let builder = TestGraphBuilder::new()
            .add_edge("model.a", "test.test_ab")
            .add_edge("model.b", "test.test_ab")
            .add_edge("model.a", "test.test_a")
            .add_edge("model.a", "test.test_abc")
            .add_edge("model.c", "test.test_abc");
        let graph = builder.build_graph();

        // Select both model.a and model.b
        let criteria_a = SelectionCriteria::new(
            MethodName::Fqn,
            vec![],
            "model.a".to_string(),
            false,
            None,
            None,
            Some(IndirectSelection::Buildable),
            None,
        );
        let criteria_b = SelectionCriteria::new(
            MethodName::Fqn,
            vec![],
            "model.b".to_string(),
            false,
            None,
            None,
            Some(IndirectSelection::Buildable),
            None,
        );
        let select = SelectExpression::Or(vec![
            SelectExpression::Atom(criteria_a),
            SelectExpression::Atom(criteria_b),
        ]);
        let result = schedule_test_graph(&graph, &select);

        // Should include all tests that depend on either model.a or model.b
        // Note: test.test_ab depends on both model.a and model.b, but when processed separately,
        // neither criteria can satisfy all dependencies, so it should not be included
        assert_eq!(result, vec!["model.a", "model.b", "test.test_a"]);
    }
    #[test]
    fn test_schedule_saved_query_metric_semantic_model_chain() {
        let token = never_cancels();
        // Build Nodes: model.m <- semantic_model.sm <- metric.met <- saved_query.sq
        let mut nodes = Nodes::default();

        // model.m
        nodes.models.insert(
            "model.m".to_string(),
            Arc::new(DbtModel {
                __common_attr__: CommonAttributes {
                    unique_id: "model.m".to_string(),
                    name: "model.m".to_string(),
                    package_name: "package".to_string(),
                    tags: vec![],
                    meta: IndexMap::new(),
                    ..Default::default()
                },
                __base_attr__: NodeBaseAttributes {
                    database: "db".to_string(),
                    schema: "schema".to_string(),
                    materialized: DbtMaterialization::View,
                    quoting: ResolvedQuoting::trues(),
                    enabled: true,
                    extended_model: false,
                    static_analysis: StaticAnalysisKind::Strict.into(),
                    ..Default::default()
                },
                __model_attr__: DbtModelAttr {
                    introspection: IntrospectionKind::None,
                    access: Access::Protected,
                    version: None,
                    latest_version: None,
                    constraints: vec![],
                    group: None,
                    time_spine: None,
                    deprecation_date: None,
                    contract: None,
                    incremental_strategy: None,
                    freshness: None,
                    state: None,
                    primary_key: vec![],
                    event_time: None,
                    catalog_name: None,
                    table_format: None,
                    sync: None,
                    compiled_code: None,
                },
                __adapter_attr__: AdapterAttr::default(),
                deprecated_config: ModelConfig::default(),
                __other__: BTreeMap::new(),
            }),
        );

        // semantic_model.sm depends on model.m
        nodes.semantic_models.insert(
            "semantic_model.sm".to_string(),
            Arc::new(DbtSemanticModel {
                __common_attr__: CommonAttributes {
                    unique_id: "semantic_model.sm".to_string(),
                    name: "semantic_model.sm".to_string(),
                    package_name: "package".to_string(),
                    tags: vec![],
                    meta: IndexMap::new(),
                    ..Default::default()
                },
                __base_attr__: NodeBaseAttributes {
                    depends_on: NodeDependsOn {
                        nodes: vec!["model.m".to_string()],
                        ..Default::default()
                    },
                    quoting: ResolvedQuoting::trues(),
                    static_analysis: StaticAnalysisKind::Strict.into(),
                    ..Default::default()
                },
                __semantic_model_attr__: DbtSemanticModelAttr::default(),
                deprecated_config: SemanticModelConfig::default(),
                __other__: BTreeMap::new(),
            }),
        );

        // metric.met depends on semantic_model.sm
        nodes.metrics.insert(
            "metric.met".to_string(),
            Arc::new(DbtMetric {
                __common_attr__: CommonAttributes {
                    unique_id: "metric.met".to_string(),
                    name: "metric.met".to_string(),
                    package_name: "package".to_string(),
                    tags: vec![],
                    meta: IndexMap::new(),
                    ..Default::default()
                },
                __base_attr__: NodeBaseAttributes {
                    depends_on: NodeDependsOn {
                        nodes: vec!["semantic_model.sm".to_string()],
                        ..Default::default()
                    },
                    quoting: ResolvedQuoting::trues(),
                    static_analysis: StaticAnalysisKind::Strict.into(),
                    ..Default::default()
                },
                __metric_attr__: DbtMetricAttr::default(),
                deprecated_config: MetricConfig::default(),
                __other__: BTreeMap::new(),
            }),
        );

        // saved_query.sq depends on metric.met
        nodes.saved_queries.insert(
            "saved_query.sq".to_string(),
            Arc::new(DbtSavedQuery {
                __common_attr__: CommonAttributes {
                    unique_id: "saved_query.sq".to_string(),
                    name: "saved_query.sq".to_string(),
                    package_name: "package".to_string(),
                    tags: vec![],
                    meta: IndexMap::new(),
                    ..Default::default()
                },
                __base_attr__: NodeBaseAttributes {
                    depends_on: NodeDependsOn {
                        nodes: vec!["metric.met".to_string()],
                        ..Default::default()
                    },
                    quoting: ResolvedQuoting::trues(),
                    static_analysis: StaticAnalysisKind::Strict.into(),
                    ..Default::default()
                },
                __saved_query_attr__: DbtSavedQueryAttr {
                    query_params: SavedQueryParams {
                        metrics: vec![],
                        group_by: vec![],
                        where_: None,
                        order_by: vec![],
                        limit: None,
                    },
                    exports: vec![],
                    label: None,
                    metadata: None,
                    unrendered_config: BTreeMap::new(),
                    group: None,
                    created_at: 0.0,
                    cache: None,
                },
                deprecated_config: SavedQueryConfig::default(),
                __other__: BTreeMap::new(),
            }),
        );

        // Select "+saved_query.sq" to include all upstream dependencies
        let select = create_select_expression("+saved_query.sq");
        let resolved_selectors = ResolvedSelector {
            include: Some(select),
            exclude: None,
            ..Default::default()
        };

        let args = SchedulerArgs {
            command: FsCommand::Test,
            io: IoArgs::default(),
            resource_types: vec![],
            exclude_resource_types: vec![],
            exclude_unique_ids: Default::default(),
            selection_override: None,
        };

        // Run scheduler
        let schedule = build_schedule(
            &args,
            &nodes,
            None,
            &resolved_selectors,
            &token,
            AdapterType::Bigquery,
        )
        .expect("schedule ok");

        // Assert selected nodes contain all
        for id in &[
            "model.m",
            "semantic_model.sm",
            "metric.met",
            "saved_query.sq",
        ] {
            assert!(
                schedule.selected_nodes.contains(*id),
                "selected_nodes missing {}",
                id
            );
        }

        // Assert deps contain the full chain
        let deps = &schedule.deps;
        assert!(
            deps.get("saved_query.sq")
                .is_some_and(|s| s.contains("metric.met")),
            "expected saved_query.sq -> metric.met"
        );
        assert!(
            deps.get("metric.met")
                .is_some_and(|s| s.contains("semantic_model.sm")),
            "expected metric.met -> semantic_model.sm"
        );
        assert!(
            deps.get("semantic_model.sm")
                .is_some_and(|s| s.contains("model.m")),
            "expected semantic_model.sm -> model.m"
        );
    }

    // ──────────────────────────────────────────────────────────────────────────
    // Comprehensive multi-seed graph-expansion tests
    //
    // These exercise the core loop in `eval_selector` (lines ~443-447) where
    // each seed matched by a single selector atom is independently expanded
    // via `select_nodes` (upstream / downstream).  A wildcard FQN pattern
    // such as `stg_*` matches multiple nodes in one atom, so the expansion
    // loop runs once per matched node — exactly the path we want to cover.
    // ──────────────────────────────────────────────────────────────────────────

    /// Build the shared test DAG used by the multi-seed tests.
    ///
    /// ```text
    ///   src_a         src_b         src_c           iso_a
    ///    / \           / \           / \               |
    /// stg_a  stg_b  stg_b  stg_c  stg_d  stg_e     iso_b
    ///     \   /  \   /          \   /                  |
    ///    prep_ab  prep_bc       prep_de              iso_c
    ///       |        |             |
    ///     dim_x    fact_y        agg_z
    ///       \      /   \          /
    ///        rpt_1      rpt_2
    ///
    ///   Tests:
    ///     dim_x  ──> test.t_dim
    ///     fact_y ──> test.t_fact
    ///     dim_x, fact_y ──> test.t_cross  (multi-parent test)
    ///
    ///   Isolated component: iso_a ──> iso_b ──> iso_c
    /// ```
    fn build_multi_seed_graph() -> BTreeMap<String, BTreeSet<String>> {
        TestGraphBuilder::new()
            // Layer 0 → Layer 1
            .add_edge("src_a", "stg_a")
            .add_edge("src_a", "stg_b")
            .add_edge("src_b", "stg_b")
            .add_edge("src_b", "stg_c")
            .add_edge("src_c", "stg_d")
            .add_edge("src_c", "stg_e")
            // Layer 1 → Layer 2
            .add_edge("stg_a", "prep_ab")
            .add_edge("stg_b", "prep_ab")
            .add_edge("stg_b", "prep_bc")
            .add_edge("stg_c", "prep_bc")
            .add_edge("stg_d", "prep_de")
            .add_edge("stg_e", "prep_de")
            // Layer 2 → Layer 3
            .add_edge("prep_ab", "dim_x")
            .add_edge("prep_bc", "fact_y")
            .add_edge("prep_de", "agg_z")
            // Layer 3 → Layer 4
            .add_edge("dim_x", "rpt_1")
            .add_edge("fact_y", "rpt_1")
            .add_edge("fact_y", "rpt_2")
            .add_edge("agg_z", "rpt_2")
            // Tests (single-parent and multi-parent)
            .add_edge("dim_x", "test.t_dim")
            .add_edge("fact_y", "test.t_fact")
            .add_edge("dim_x", "test.t_cross")
            .add_edge("fact_y", "test.t_cross")
            // Disconnected subgraph
            .add_edge("iso_a", "iso_b")
            .add_edge("iso_b", "iso_c")
            .build_graph()
    }

    /// Sort a vec for set-equality comparison (ignores topo-order).
    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    // ── 1. Multi-seed upstream ──────────────────────────────────────────────
    #[test]
    fn test_multi_seed_upstream_wildcard() {
        // "+stg_*" matches stg_a..stg_e, then expands each upstream.
        // All five staging nodes share three source ancestors.
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(&g, &create_select_expression("+stg_*"))),
            vec![
                "src_a", "src_b", "src_c", "stg_a", "stg_b", "stg_c", "stg_d", "stg_e",
            ],
        );
    }

    // ── 2. Multi-seed downstream ────────────────────────────────────────────
    #[test]
    fn test_multi_seed_downstream_wildcard() {
        // "stg_*+" matches stg_a..stg_e, then expands each downstream.
        // Reaches prep, marts, reports and all three tests.
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(&g, &create_select_expression("stg_*+"))),
            vec![
                "agg_z",
                "dim_x",
                "fact_y",
                "prep_ab",
                "prep_bc",
                "prep_de",
                "rpt_1",
                "rpt_2",
                "stg_a",
                "stg_b",
                "stg_c",
                "stg_d",
                "stg_e",
                "test.t_cross",
                "test.t_dim",
                "test.t_fact",
            ],
        );
    }

    // ── 3. Multi-seed bidirectional ─────────────────────────────────────────
    #[test]
    fn test_multi_seed_bidirectional_wildcard() {
        // "+stg_*+" expands both upstream and downstream from all stg nodes.
        // Reaches every node in the main component (but not the isolated one).
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("+stg_*+")
            )),
            vec![
                "agg_z",
                "dim_x",
                "fact_y",
                "prep_ab",
                "prep_bc",
                "prep_de",
                "rpt_1",
                "rpt_2",
                "src_a",
                "src_b",
                "src_c",
                "stg_a",
                "stg_b",
                "stg_c",
                "stg_d",
                "stg_e",
                "test.t_cross",
                "test.t_dim",
                "test.t_fact",
            ],
        );
    }

    // ── 4. Depth-limited upstream ───────────────────────────────────────────
    #[test]
    fn test_multi_seed_depth_limited_upstream() {
        let g = build_multi_seed_graph();

        // "2+dim_x": 2 hops up from dim_x → prep_ab (1), stg_a + stg_b (2)
        // Plus eager indirect tests: dim_x → test.t_dim, test.t_cross
        assert_eq!(
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("2+dim_x")
            )),
            vec![
                "dim_x",
                "prep_ab",
                "stg_a",
                "stg_b",
                "test.t_cross",
                "test.t_dim"
            ],
        );

        // "1+stg_*": 1 hop up from each stg → immediate source parents
        // (same as "+stg_*" in this graph, since srcs are exactly 1 hop away)
        assert_eq!(
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("1+stg_*")
            )),
            vec![
                "src_a", "src_b", "src_c", "stg_a", "stg_b", "stg_c", "stg_d", "stg_e",
            ],
        );
    }

    // ── 5. Depth-limited downstream ─────────────────────────────────────────
    #[test]
    fn test_multi_seed_depth_limited_downstream() {
        let g = build_multi_seed_graph();

        // "stg_*+1": 1 hop down from each stg → prep layer only (no tests)
        assert_eq!(
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("stg_*+1")
            )),
            vec![
                "prep_ab", "prep_bc", "prep_de", "stg_a", "stg_b", "stg_c", "stg_d", "stg_e",
            ],
        );

        // "stg_*+2": 2 hops down → prep (1) + marts (2)
        // Indirect selection (eager) adds tests whose parent is in the set:
        //   dim_x in set → test.t_dim, test.t_cross added
        //   fact_y in set → test.t_fact, test.t_cross added
        assert_eq!(
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("stg_*+2")
            )),
            vec![
                "agg_z",
                "dim_x",
                "fact_y",
                "prep_ab",
                "prep_bc",
                "prep_de",
                "stg_a",
                "stg_b",
                "stg_c",
                "stg_d",
                "stg_e",
                "test.t_cross",
                "test.t_dim",
                "test.t_fact",
            ],
        );
    }

    // ── 6. Upstream from mid-layer ──────────────────────────────────────────
    #[test]
    fn test_multi_seed_upstream_from_mid_layer() {
        // "+prep_*" matches prep_ab, prep_bc, prep_de → expand upstream
        // Reaches all staging + all sources
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("+prep_*")
            )),
            vec![
                "prep_ab", "prep_bc", "prep_de", "src_a", "src_b", "src_c", "stg_a", "stg_b",
                "stg_c", "stg_d", "stg_e",
            ],
        );
    }

    // ── 7. Downstream from mid-layer ────────────────────────────────────────
    #[test]
    fn test_multi_seed_downstream_from_mid_layer() {
        // "prep_*+" matches prep nodes → expand downstream → marts, reports, tests
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("prep_*+")
            )),
            vec![
                "agg_z",
                "dim_x",
                "fact_y",
                "prep_ab",
                "prep_bc",
                "prep_de",
                "rpt_1",
                "rpt_2",
                "test.t_cross",
                "test.t_dim",
                "test.t_fact",
            ],
        );
    }

    // ── 8. Union of individual upstream expansions ──────────────────────────
    #[test]
    fn test_union_of_upstream_expansions() {
        // "+dim_x +agg_z" — two separate atoms, unioned (Or).
        // dim_x ancestors: prep_ab, stg_a, stg_b, src_a, src_b
        //   + eager tests: test.t_dim, test.t_cross (successors of dim_x)
        // agg_z ancestors: prep_de, stg_d, stg_e, src_c
        //   (no test successors for agg_z's ancestors)
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("+dim_x +agg_z")
            )),
            vec![
                "agg_z",
                "dim_x",
                "prep_ab",
                "prep_de",
                "src_a",
                "src_b",
                "src_c",
                "stg_a",
                "stg_b",
                "stg_d",
                "stg_e",
                "test.t_cross",
                "test.t_dim",
            ],
        );
    }

    // ── 9. Intersection of upstream expansions ──────────────────────────────
    #[test]
    fn test_intersection_of_upstream_expansions() {
        // "+rpt_1,+rpt_2" — AND of two upstream expansions.
        // +rpt_1 ancestors: dim_x, fact_y, prep_ab, prep_bc, stg_a, stg_b, stg_c, src_a, src_b
        //   + eager tests: t_dim, t_fact, t_cross
        // +rpt_2 ancestors: fact_y, agg_z, prep_bc, prep_de, stg_b, stg_c, stg_d, stg_e, src_a, src_b, src_c
        //   + eager tests: t_fact, t_cross
        // Intersection: fact_y, prep_bc, stg_b, stg_c, src_a, src_b, t_fact, t_cross
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("+rpt_1,+rpt_2")
            )),
            vec![
                "fact_y",
                "prep_bc",
                "src_a",
                "src_b",
                "stg_b",
                "stg_c",
                "test.t_cross",
                "test.t_fact",
            ],
        );
    }

    // ── 10. Exclude with upstream expansion ─────────────────────────────────
    #[test]
    fn test_multi_seed_upstream_with_exclude() {
        // "+stg_*" exclude "src_a"
        // Include = {src_a, src_b, src_c, stg_a..stg_e}
        // Exclude = {src_a}  (no tests to add via eager indirect)
        // Result = {src_b, src_c, stg_a..stg_e}
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_graph_with_exclude(
                &g,
                &create_select_expression("+stg_*"),
                &create_select_expression("src_a"),
            )),
            vec![
                "src_b", "src_c", "stg_a", "stg_b", "stg_c", "stg_d", "stg_e"
            ],
        );
    }

    // ── 11. Diamond upstream ────────────────────────────────────────────────
    #[test]
    fn test_upstream_through_diamond() {
        // "+prep_ab": prep_ab ← stg_a ← src_a
        //             prep_ab ← stg_b ← src_a, src_b
        // No tests are successors of these nodes (only prep nodes downstream of stg).
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("+prep_ab")
            )),
            vec!["prep_ab", "src_a", "src_b", "stg_a", "stg_b"],
        );
    }

    // ── 12. Downstream from root ────────────────────────────────────────────
    #[test]
    fn test_downstream_from_root() {
        // "src_a+" reaches everything downstream of src_a:
        //   stg_a, stg_b → prep_ab, prep_bc → dim_x, fact_y →
        //   rpt_1, rpt_2, t_dim, t_fact, t_cross
        // (agg_z not reached — it descends from src_c only)
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(&g, &create_select_expression("src_a+"))),
            vec![
                "dim_x",
                "fact_y",
                "prep_ab",
                "prep_bc",
                "rpt_1",
                "rpt_2",
                "src_a",
                "stg_a",
                "stg_b",
                "test.t_cross",
                "test.t_dim",
                "test.t_fact",
            ],
        );
    }

    // ── 13. Depth-limited multi-seed upstream from leaf layer ────────────────
    #[test]
    fn test_depth_limited_upstream_from_leaf_layer() {
        // "2+rpt_*": 2 hops up from rpt_1 AND rpt_2 (union).
        //   rpt_1 (2 hops): dim_x, fact_y, prep_ab, prep_bc
        //   rpt_2 (2 hops): fact_y, agg_z, prep_bc, prep_de
        //   + eager tests reachable from dim_x, fact_y: t_dim, t_fact, t_cross
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("2+rpt_*")
            )),
            vec![
                "agg_z",
                "dim_x",
                "fact_y",
                "prep_ab",
                "prep_bc",
                "prep_de",
                "rpt_1",
                "rpt_2",
                "test.t_cross",
                "test.t_dim",
                "test.t_fact",
            ],
        );
    }

    // ── 14. Isolated subgraph is not polluted ───────────────────────────────
    #[test]
    fn test_isolated_subgraph_not_polluted() {
        let g = build_multi_seed_graph();
        let stg_down = sorted(schedule_test_graph(&g, &create_select_expression("stg_*+")));
        // iso_ nodes must not appear in stg_*+ results
        assert!(
            !stg_down.iter().any(|n| n.starts_with("iso_")),
            "stg_*+ should not reach isolated subgraph, got: {stg_down:?}"
        );
    }

    // ── 15. Isolated subgraph works independently ───────────────────────────
    #[test]
    fn test_isolated_subgraph_independent() {
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(&g, &create_select_expression("+iso_c"))),
            vec!["iso_a", "iso_b", "iso_c"],
        );
    }

    // ── 16. Wildcard matching zero nodes ────────────────────────────────────
    #[test]
    fn test_wildcard_matching_zero_nodes() {
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("+nonexistent_*")
            )),
            Vec::<String>::new(),
        );
    }

    // ── 17. Root with no parents ────────────────────────────────────────────
    #[test]
    fn test_root_with_no_parents() {
        // "+src_a" → src_a has no parents, result is just {src_a}
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(&g, &create_select_expression("+src_a"))),
            vec!["src_a"],
        );
    }

    // ── 18. Union equals superset ───────────────────────────────────────────
    #[test]
    fn test_union_equals_superset() {
        // "+stg_* +prep_*" should equal "+prep_*" since prep ancestors ⊇ stg ancestors
        let g = build_multi_seed_graph();
        assert_eq!(
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("+stg_* +prep_*")
            )),
            sorted(schedule_test_graph(
                &g,
                &create_select_expression("+prep_*")
            )),
        );
    }

    // ── 19. Downstream with exclude removes eager tests ─────────────────────
    #[test]
    fn test_downstream_with_exclude_removes_eager_tests() {
        // "stg_*+" exclude "dim_x"
        // The exclude expands eagerly: dim_x → test.t_dim, test.t_cross
        // So these three nodes are removed from the downstream expansion.
        let g = build_multi_seed_graph();
        let result = sorted(schedule_graph_with_exclude(
            &g,
            &create_select_expression("stg_*+"),
            &create_select_expression("dim_x"),
        ));
        // dim_x, t_dim, and t_cross should be excluded
        assert!(
            !result.contains(&"dim_x".to_string()),
            "dim_x should be excluded"
        );
        assert!(
            !result.contains(&"test.t_dim".to_string()),
            "test.t_dim should be excluded (eager indirect of dim_x)"
        );
        assert!(
            !result.contains(&"test.t_cross".to_string()),
            "test.t_cross should be excluded (eager indirect of dim_x)"
        );
        // But the rest of the downstream is still there
        assert!(result.contains(&"fact_y".to_string()));
        assert!(result.contains(&"rpt_1".to_string()));
        assert!(result.contains(&"test.t_fact".to_string()));
    }

    // ── 20. Full traversal from multiple roots ──────────────────────────────
    #[test]
    fn test_downstream_from_multiple_roots() {
        // "src_a+ src_c+" — union of two root downstream expansions.
        // src_a reaches: stg_a, stg_b, prep_ab, prep_bc, dim_x, fact_y, rpt_1, rpt_2, tests
        // src_c reaches: stg_d, stg_e, prep_de, agg_z, rpt_2
        // Union covers everything except src_b, stg_c (which only descends from src_b)
        let g = build_multi_seed_graph();
        let result = sorted(schedule_test_graph(
            &g,
            &create_select_expression("src_a+ src_c+"),
        ));
        // Spot-check: src_b and stg_c should NOT be in the result
        assert!(
            !result.contains(&"src_b".to_string()),
            "src_b is not downstream of src_a or src_c"
        );
        assert!(
            !result.contains(&"stg_c".to_string()),
            "stg_c descends only from src_b"
        );
        // Everything else in the main component should be present
        for expected in [
            "src_a", "src_c", "stg_a", "stg_b", "stg_d", "stg_e", "prep_ab", "prep_de", "dim_x",
            "agg_z", "rpt_2",
        ] {
            assert!(
                result.contains(&expected.to_string()),
                "expected {expected} in result, got: {result:?}"
            );
        }
    }
}

#[cfg(test)]
mod resource_type_filtering_tests {
    use super::tests::create_select_expression;
    use super::*;
    use dbt_common::{
        cancellation::never_cancels,
        io_args::{ClapResourceType, FsCommand, IoArgs, StaticAnalysisKind},
    };
    use dbt_schemas::schemas::selection_override::SelectionOverride;
    use dbt_schemas::schemas::{
        CommonAttributes, DbtModel, DbtModelAttr, DbtSeed, DbtSeedAttr, DbtTest, DbtUnitTest,
        IntrospectionKind, NodeBaseAttributes,
        common::{Access, DbtMaterialization, NodeDependsOn, ResolvedQuoting},
        nodes::AdapterAttr,
        project::ModelConfig,
    };
    use indexmap::IndexMap;
    use std::collections::HashSet;
    use std::sync::Arc;

    #[test]
    fn test_filters_out_empty_sql_models() {
        let token = never_cancels();
        // Build Nodes with one empty and one non-empty model based on checksum
        let mut nodes = Nodes::default();

        let non_empty_checksum = DbtChecksum::hash(b"select 1");
        let empty_checksum = DbtChecksum::hash(b"");

        // model.non_empty
        nodes.models.insert(
            "model.non_empty".to_string(),
            Arc::new(DbtModel {
                __common_attr__: CommonAttributes {
                    unique_id: "model.non_empty".to_string(),
                    name: "model.non_empty".to_string(),
                    package_name: "package".to_string(),
                    tags: vec![],
                    meta: IndexMap::new(),
                    checksum: non_empty_checksum,
                    ..Default::default()
                },
                __base_attr__: NodeBaseAttributes {
                    database: "db".to_string(),
                    schema: "schema".to_string(),
                    materialized: DbtMaterialization::View,
                    quoting: ResolvedQuoting::trues(),
                    enabled: true,
                    extended_model: false,
                    static_analysis: StaticAnalysisKind::Strict.into(),
                    ..Default::default()
                },
                __model_attr__: DbtModelAttr {
                    introspection: IntrospectionKind::None,
                    access: Access::Protected,
                    ..Default::default()
                },
                __adapter_attr__: AdapterAttr::default(),
                deprecated_config: ModelConfig::default(),
                __other__: BTreeMap::new(),
            }),
        );

        // model.empty (should be filtered out)
        nodes.models.insert(
            "model.empty".to_string(),
            Arc::new(DbtModel {
                __common_attr__: CommonAttributes {
                    unique_id: "model.empty".to_string(),
                    name: "model.empty".to_string(),
                    package_name: "package".to_string(),
                    tags: vec![],
                    meta: IndexMap::new(),
                    checksum: empty_checksum,
                    ..Default::default()
                },
                __base_attr__: NodeBaseAttributes {
                    database: "db".to_string(),
                    schema: "schema".to_string(),
                    materialized: DbtMaterialization::View,
                    quoting: ResolvedQuoting::trues(),
                    enabled: true,
                    extended_model: false,
                    static_analysis: StaticAnalysisKind::Strict.into(),
                    ..Default::default()
                },
                __model_attr__: DbtModelAttr {
                    introspection: IntrospectionKind::None,
                    access: Access::Protected,
                    ..Default::default()
                },
                __adapter_attr__: AdapterAttr::default(),
                deprecated_config: ModelConfig::default(),
                __other__: BTreeMap::new(),
            }),
        );

        // Schedule with "select all"
        let resolved_selectors = ResolvedSelector {
            include: None,
            exclude: None,
            ..Default::default()
        };
        let args = SchedulerArgs {
            command: FsCommand::Test,
            io: IoArgs::default(),
            resource_types: vec![],
            exclude_resource_types: vec![],
            exclude_unique_ids: Default::default(),
            selection_override: None,
        };

        let schedule = build_schedule(
            &args,
            &nodes,
            None,
            &resolved_selectors,
            &token,
            AdapterType::Bigquery,
        )
        .unwrap();

        // Assert: empty model is not selected; non-empty is selected
        assert!(
            schedule.selected_nodes.contains("model.non_empty"),
            "Expected non-empty model to be selected"
        );
        assert!(
            !schedule.selected_nodes.contains("model.empty"),
            "Empty model should be filtered out from selected nodes"
        );
        // Also ensure it's not brought back as a frontier node
        assert!(
            !schedule.frontier_nodes.contains("model.empty"),
            "Empty model should not appear as a frontier node"
        );
    }

    fn make_model(id: &str) -> (String, Arc<DbtModel>) {
        (
            id.to_string(),
            Arc::new(DbtModel {
                __common_attr__: CommonAttributes {
                    unique_id: id.to_string(),
                    tags: vec![],
                    meta: IndexMap::new(),
                    ..Default::default()
                },
                __base_attr__: NodeBaseAttributes {
                    materialized: DbtMaterialization::View,
                    quoting: ResolvedQuoting::trues(),
                    enabled: true,
                    extended_model: false,
                    static_analysis: StaticAnalysisKind::Strict.into(),
                    ..Default::default()
                },
                __model_attr__: DbtModelAttr {
                    introspection: IntrospectionKind::None,
                    version: None,
                    latest_version: None,
                    constraints: vec![],
                    access: Access::Protected,
                    group: None,
                    deprecation_date: None,
                    primary_key: vec![],
                    time_spine: None,
                    contract: None,
                    incremental_strategy: None,
                    freshness: None,
                    state: None,
                    event_time: None,
                    catalog_name: None,
                    table_format: None,
                    sync: None,
                    compiled_code: None,
                },
                deprecated_config: ModelConfig::default(),
                __other__: BTreeMap::new(),
                __adapter_attr__: AdapterAttr::default(),
            }),
        )
    }

    fn make_seed(id: &str) -> (String, Arc<DbtSeed>) {
        (
            id.to_string(),
            Arc::new(DbtSeed {
                __common_attr__: CommonAttributes {
                    unique_id: id.to_string(),
                    tags: vec![],
                    meta: IndexMap::new(),
                    ..Default::default()
                },
                __base_attr__: NodeBaseAttributes {
                    quoting: ResolvedQuoting::trues(),
                    materialized: DbtMaterialization::Table,
                    ..Default::default()
                },
                __seed_attr__: DbtSeedAttr {
                    root_path: None,
                    ..Default::default()
                },
                deprecated_config: Default::default(),
                __other__: Default::default(),
            }),
        )
    }

    fn basic_nodes() -> Nodes {
        let mut nodes = Nodes::default();
        nodes
            .models
            .insert("model.a".to_string(), make_model("model.a").1);
        nodes
            .models
            .insert("model.b".to_string(), make_model("model.b").1);
        nodes
            .seeds
            .insert("seed.a".to_string(), make_seed("seed.a").1);
        nodes
            .seeds
            .insert("seed.b".to_string(), make_seed("seed.b").1);
        nodes
    }

    // todo add exclude resource types
    fn make_scheduler_args(resource_types: Vec<ClapResourceType>) -> SchedulerArgs {
        SchedulerArgs {
            command: FsCommand::Test,
            io: Default::default(),
            resource_types,
            exclude_resource_types: vec![],
            exclude_unique_ids: Default::default(),
            selection_override: None,
        }
    }

    #[test]
    fn test_no_resource_type_filter() {
        let token = never_cancels();
        let nodes = basic_nodes();
        let args = make_scheduler_args(vec![]);
        let resolved_selectors = ResolvedSelector {
            include: None,
            exclude: None,
            ..Default::default()
        };
        let schedule = build_schedule(
            &args,
            &nodes,
            None,
            &resolved_selectors,
            &token,
            AdapterType::Bigquery,
        )
        .unwrap();
        let mut all_ids: Vec<_> = schedule.selected_nodes.iter().cloned().collect();
        all_ids.sort();
        assert_eq!(all_ids, vec!["model.a", "model.b", "seed.a", "seed.b"]);
    }

    #[test]
    fn test_disabled_nodes_are_not_scheduled() {
        let token = never_cancels();
        let mut nodes = basic_nodes();
        nodes.tests.insert(
            "test.disabled".to_string(),
            Arc::new(DbtTest {
                __common_attr__: CommonAttributes {
                    name: "disabled".to_string(),
                    package_name: "package".to_string(),
                    unique_id: "test.disabled".to_string(),
                    tags: vec![],
                    meta: IndexMap::new(),
                    ..Default::default()
                },
                __base_attr__: NodeBaseAttributes {
                    enabled: false,
                    depends_on: NodeDependsOn {
                        nodes: vec!["model.a".to_string()],
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            }),
        );
        nodes.unit_tests.insert(
            "unit_test.disabled".to_string(),
            Arc::new(DbtUnitTest {
                __common_attr__: CommonAttributes {
                    name: "disabled".to_string(),
                    package_name: "package".to_string(),
                    unique_id: "unit_test.disabled".to_string(),
                    tags: vec![],
                    meta: IndexMap::new(),
                    ..Default::default()
                },
                __base_attr__: NodeBaseAttributes {
                    enabled: false,
                    depends_on: NodeDependsOn {
                        nodes: vec!["model.a".to_string()],
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            }),
        );
        let args = make_scheduler_args(vec![]);
        let resolved_selectors = ResolvedSelector {
            include: None,
            exclude: None,
            ..Default::default()
        };

        let schedule = build_schedule(
            &args,
            &nodes,
            None,
            &resolved_selectors,
            &token,
            AdapterType::Bigquery,
        )
        .unwrap();

        assert!(!schedule.selected_nodes.contains("test.disabled"));
        assert!(!schedule.selected_nodes.contains("unit_test.disabled"));
        assert!(!schedule.all_selected_nodes.contains("test.disabled"));
        assert!(!schedule.all_selected_nodes.contains("unit_test.disabled"));
        assert!(!schedule.sorted_nodes.contains(&"test.disabled".to_string()));
        assert!(
            !schedule
                .sorted_nodes
                .contains(&"unit_test.disabled".to_string())
        );
        assert!(schedule.deps.contains_key("model.a"));
        assert!(!schedule.deps.contains_key("test.disabled"));
        assert!(!schedule.deps.contains_key("unit_test.disabled"));
    }

    #[test]
    fn test_model_resource_type_filter() {
        let token = never_cancels();
        let nodes = basic_nodes();
        let args = make_scheduler_args(vec![ClapResourceType::Model]);
        let resolved_selectors = ResolvedSelector {
            include: None,
            exclude: None,
            ..Default::default()
        };
        let schedule = build_schedule(
            &args,
            &nodes,
            None,
            &resolved_selectors,
            &token,
            AdapterType::Bigquery,
        )
        .unwrap();
        let mut ids: Vec<_> = schedule.selected_nodes.iter().cloned().collect();
        ids.sort();
        assert_eq!(ids, vec!["model.a", "model.b"]);
    }

    #[test]
    fn test_seed_resource_type_filter() {
        let token = never_cancels();
        let nodes = basic_nodes();
        let args = make_scheduler_args(vec![ClapResourceType::Seed]);
        let resolved_selectors = ResolvedSelector {
            include: None,
            exclude: None,
            ..Default::default()
        };
        let schedule = build_schedule(
            &args,
            &nodes,
            None,
            &resolved_selectors,
            &token,
            AdapterType::Bigquery,
        )
        .unwrap();
        let mut ids: Vec<_> = schedule.selected_nodes.iter().cloned().collect();
        ids.sort();
        assert_eq!(ids, vec!["seed.a", "seed.b"]);
    }

    #[test]
    fn test_model_and_seed_resource_type_filter() {
        let token = never_cancels();
        let nodes = basic_nodes();
        let args = make_scheduler_args(vec![ClapResourceType::Model, ClapResourceType::Seed]);
        let resolved_selectors = ResolvedSelector {
            include: None,
            exclude: None,
            ..Default::default()
        };
        let schedule = build_schedule(
            &args,
            &nodes,
            None,
            &resolved_selectors,
            &token,
            AdapterType::Bigquery,
        )
        .unwrap();
        let mut ids: Vec<_> = schedule.selected_nodes.iter().cloned().collect();
        ids.sort();
        assert_eq!(ids, vec!["model.a", "model.b", "seed.a", "seed.b"]);
    }

    #[test]
    fn test_nonexistent_resource_type_filter() {
        let token = never_cancels();
        let nodes = basic_nodes();
        // Use Snapshot, which is not present in basic_nodes
        let args = make_scheduler_args(vec![ClapResourceType::Snapshot]);
        let resolved_selectors = ResolvedSelector {
            include: None,
            exclude: None,
            ..Default::default()
        };
        let schedule = build_schedule(
            &args,
            &nodes,
            None,
            &resolved_selectors,
            &token,
            AdapterType::Bigquery,
        )
        .unwrap();
        assert!(schedule.selected_nodes.is_empty());
    }

    // --------------------------------------------------------------------------------------
    // Selection override: an externally supplied node set replacing the computed selection
    // --------------------------------------------------------------------------------------

    fn make_test_node(id: &str, depends_on: &[&str], enabled: bool) -> Arc<DbtTest> {
        Arc::new(DbtTest {
            __common_attr__: CommonAttributes {
                name: id.to_string(),
                package_name: "package".to_string(),
                unique_id: id.to_string(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                enabled,
                depends_on: NodeDependsOn {
                    nodes: depends_on.iter().map(|d| d.to_string()).collect(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        })
    }

    /// Nodes the override tests share: two models, two seeds, one test on `model.a`.
    fn override_nodes() -> Nodes {
        let mut nodes = basic_nodes();
        nodes.tests.insert(
            "test.a".to_string(),
            make_test_node("test.a", &["model.a"], true),
        );
        nodes
    }

    fn override_args(over: Option<&[&str]>) -> SchedulerArgs {
        SchedulerArgs {
            selection_override: over
                .map(|ids| SelectionOverride::from_ids(ids.iter().map(|s| s.to_string()), "test")),
            ..make_scheduler_args(vec![])
        }
    }

    fn schedule_for_override(
        nodes: &Nodes,
        args: &SchedulerArgs,
        resolved_selectors: &ResolvedSelector,
    ) -> (Schedule<String>, Option<SelectionOverrideStats>) {
        let token = never_cancels();
        let deps = derive_deps(nodes, &token).unwrap();
        schedule_graph(
            &deps,
            nodes,
            None,
            resolved_selectors,
            None,
            args,
            AdapterType::Bigquery,
        )
        .unwrap()
    }

    fn selected(schedule: &Schedule<String>) -> Vec<String> {
        let mut ids: Vec<_> = schedule.selected_nodes.iter().cloned().collect();
        ids.sort();
        ids
    }

    fn select_only(expr: &str) -> ResolvedSelector {
        ResolvedSelector {
            include: Some(create_select_expression(expr)),
            exclude: None,
            ..Default::default()
        }
    }

    #[test]
    fn test_override_widens_a_narrower_computed_selection() {
        // Under-selection direction: the computed selection is one model, the override asks for
        // more than that, and the override wins.
        let nodes = override_nodes();
        let args = override_args(Some(&["model.a", "model.b", "seed.a"]));
        let (schedule, stats) = schedule_for_override(&nodes, &args, &select_only("model.a"));

        assert_eq!(selected(&schedule), vec!["model.a", "model.b", "seed.a"]);

        let stats = stats.expect("override should report counters");
        assert_eq!(stats.injected, 3);
        assert_eq!(stats.matched, 3);
        assert_eq!(stats.unmatched, 0);
        assert_eq!(stats.dropped_unschedulable, 0);
        // `model.a` alone, plus `test.a` pulled in indirectly.
        assert_eq!(stats.fusion_would_select, 2);
        // `test.a` is what this engine would have run and the override does not ask for.
        assert_eq!(stats.fusion_only, 1);
    }

    #[test]
    fn test_override_narrows_a_wider_computed_selection() {
        // Over-selection direction, the shape that motivates the feature: the engine would run
        // the whole project, the override asks for one node.
        let nodes = override_nodes();
        let args = override_args(Some(&["model.a"]));
        let (schedule, stats) = schedule_for_override(
            &nodes,
            &args,
            &ResolvedSelector {
                include: None,
                exclude: None,
                ..Default::default()
            },
        );

        assert_eq!(selected(&schedule), vec!["model.a"]);

        let stats = stats.expect("override should report counters");
        assert_eq!(stats.injected, 1);
        assert_eq!(stats.matched, 1);
        assert_eq!(stats.fusion_would_select, 5);
        assert_eq!(stats.fusion_only, 4);
    }

    #[test]
    fn test_override_to_the_empty_set_schedules_nothing() {
        // An empty supplied set is a legitimate instruction, and it is the maximal
        // over-selection case: distinguishable from "no override" only by the artifact existing.
        let nodes = override_nodes();
        let args = override_args(Some(&[]));
        let (schedule, stats) = schedule_for_override(
            &nodes,
            &args,
            &ResolvedSelector {
                include: None,
                exclude: None,
                ..Default::default()
            },
        );

        assert!(schedule.selected_nodes.is_empty());
        let stats = stats.expect("override should report counters");
        assert_eq!(stats.injected, 0);
        assert_eq!(stats.fusion_would_select, 5);
        assert_eq!(stats.fusion_only, 5);
    }

    #[test]
    fn test_override_ignores_exclude() {
        // `--exclude` is a selection input, so the override supersedes it: a node the exclude
        // expression would remove is still scheduled.
        let nodes = override_nodes();
        let args = override_args(Some(&["model.a", "model.b"]));
        let resolved_selectors = ResolvedSelector {
            include: None,
            exclude: Some(create_select_expression("model.b")),
            ..Default::default()
        };
        let (schedule, _) = schedule_for_override(&nodes, &args, &resolved_selectors);
        assert_eq!(selected(&schedule), vec!["model.a", "model.b"]);
    }

    #[test]
    fn test_override_ignores_exclude_unique_ids() {
        let nodes = override_nodes();
        let args = SchedulerArgs {
            exclude_unique_ids: HashSet::from(["model.b".to_string()]),
            ..override_args(Some(&["model.a", "model.b"]))
        };
        let (schedule, _) = schedule_for_override(
            &nodes,
            &args,
            &ResolvedSelector {
                include: None,
                exclude: None,
                ..Default::default()
            },
        );
        assert_eq!(selected(&schedule), vec!["model.a", "model.b"]);
    }

    #[test]
    fn test_override_ignores_resource_type_filters() {
        let nodes = override_nodes();
        let args = SchedulerArgs {
            resource_types: vec![ClapResourceType::Seed],
            exclude_resource_types: vec![ClapResourceType::Model],
            ..override_args(Some(&["model.a", "seed.a"]))
        };
        let (schedule, _) = schedule_for_override(
            &nodes,
            &args,
            &ResolvedSelector {
                include: None,
                exclude: None,
                ..Default::default()
            },
        );
        assert_eq!(selected(&schedule), vec!["model.a", "seed.a"]);
    }

    #[test]
    fn test_override_drops_unschedulable_nodes_and_counts_them() {
        // Schedulability is a capability of this engine and applies to any incoming set. A
        // disabled test and a model whose normalized SQL is empty are both reported rather than
        // force-run: a supplied id for either is a divergence worth naming.
        let mut nodes = override_nodes();
        nodes.tests.insert(
            "test.disabled".to_string(),
            make_test_node("test.disabled", &["model.a"], false),
        );
        let (empty_id, empty_model) = make_model("model.empty");
        let mut empty_model = (*empty_model).clone();
        empty_model.__common_attr__.checksum = DbtChecksum::hash(b"");
        nodes.models.insert(empty_id, Arc::new(empty_model));

        let args = override_args(Some(&["model.a", "model.empty", "test.disabled"]));
        let (schedule, stats) = schedule_for_override(
            &nodes,
            &args,
            &ResolvedSelector {
                include: None,
                exclude: None,
                ..Default::default()
            },
        );

        assert_eq!(selected(&schedule), vec!["model.a"]);

        let stats = stats.expect("override should report counters");
        assert_eq!(stats.injected, 3);
        assert_eq!(stats.matched, 3);
        assert_eq!(stats.dropped_unschedulable, 2);
        assert_eq!(
            stats.dropped_unschedulable_sample,
            vec!["model.empty".to_string(), "test.disabled".to_string()]
        );
    }

    #[test]
    fn test_override_with_unmatched_id_is_counted_not_fatal() {
        // Node ids do not always survive the trip between engines. An id with no counterpart here
        // is reported and skipped; it must not reach the scheduled set as a phantom node.
        let nodes = override_nodes();
        let args = override_args(Some(&["model.a", "model.does_not_exist"]));
        let (schedule, stats) = schedule_for_override(
            &nodes,
            &args,
            &ResolvedSelector {
                include: None,
                exclude: None,
                ..Default::default()
            },
        );

        assert_eq!(selected(&schedule), vec!["model.a"]);
        assert!(
            !schedule
                .sorted_nodes
                .contains(&"model.does_not_exist".to_string())
        );

        let stats = stats.expect("override should report counters");
        assert_eq!(stats.injected, 2);
        assert_eq!(stats.matched, 1);
        assert_eq!(stats.unmatched, 1);
        assert_eq!(
            stats.unmatched_sample,
            vec!["model.does_not_exist".to_string()]
        );
    }

    /// `model.child` refs an ephemeral `model.eph`.
    fn nodes_with_ephemeral_parent() -> Nodes {
        let mut nodes = Nodes::default();

        let (eph_id, eph) = make_model("model.eph");
        let mut eph = (*eph).clone();
        eph.__base_attr__.materialized = DbtMaterialization::Ephemeral;
        nodes.models.insert(eph_id, Arc::new(eph));

        let (child_id, child) = make_model("model.child");
        let mut child = (*child).clone();
        child.__base_attr__.depends_on = NodeDependsOn {
            nodes: vec!["model.eph".to_string()],
            ..Default::default()
        };
        nodes.models.insert(child_id, Arc::new(child));

        nodes
    }

    #[test]
    fn test_override_readds_ephemeral_ancestor_absent_from_the_supplied_set() {
        // Ephemeral models are inlined as CTEs and so are never reported by a run, which means a
        // supplied ran-set cannot name them. The existing ephemeral-ancestor expansion re-adds
        // them, which is why the override is applied before that stage rather than after.
        let nodes = nodes_with_ephemeral_parent();
        let args = override_args(Some(&["model.child"]));
        let (schedule, _) = schedule_for_override(
            &nodes,
            &args,
            &ResolvedSelector {
                include: None,
                exclude: None,
                ..Default::default()
            },
        );

        assert_eq!(selected(&schedule), vec!["model.child", "model.eph"]);
    }

    #[test]
    fn test_ephemeral_model_absent_from_the_override_is_not_unmatched() {
        // The ephemeral model exists here, so its absence from the supplied set is not an
        // unmatched id; `unmatched` counts only supplied ids with no counterpart node.
        let nodes = nodes_with_ephemeral_parent();
        let args = override_args(Some(&["model.child"]));
        let (_, stats) = schedule_for_override(
            &nodes,
            &args,
            &ResolvedSelector {
                include: None,
                exclude: None,
                ..Default::default()
            },
        );

        let stats = stats.expect("override should report counters");
        assert_eq!(stats.injected, 1);
        assert_eq!(stats.unmatched, 0);
        assert!(stats.unmatched_sample.is_empty());
    }

    #[test]
    fn test_no_override_leaves_the_computed_selection_alone() {
        let nodes = override_nodes();
        let args = override_args(None);
        let (schedule, stats) = schedule_for_override(&nodes, &args, &select_only("model.a"));

        assert_eq!(selected(&schedule), vec!["model.a", "test.a"]);
        assert!(stats.is_none());
    }
}

#[cfg(test)]
mod cycle_detection_tests {
    use super::*;
    use dbt_common::CodeLocationWithFile;
    use dbt_common::io_args::{FsCommand, StaticAnalysisKind};
    use dbt_common::node_selector::{MethodName, SelectionCriteria};
    use dbt_schemas::schemas::common::{
        Access, DbtMaterialization, NodeDependsOn, ResolvedQuoting,
    };
    use dbt_schemas::schemas::nodes::AdapterAttr;
    use dbt_schemas::schemas::project::ModelConfig;
    use dbt_schemas::schemas::{
        CommonAttributes, DbtModel, DbtModelAttr, IntrospectionKind, NodeBaseAttributes,
    };
    use dbt_test_primitives::assert_contains;
    use indexmap::IndexMap;
    use std::sync::Arc;

    // helper func to create nodes with dependencies
    fn create_test_node(
        id: &str,
        deps: Vec<(&str, CodeLocationWithFile)>,
    ) -> (String, Arc<DbtModel>) {
        let depends_on = NodeDependsOn {
            nodes_with_ref_location: deps
                .into_iter()
                .map(|(dep, loc)| (dep.to_string(), loc))
                .collect(),
            ..Default::default()
        };

        let model = DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: id.to_string(),
                tags: vec![],
                meta: IndexMap::new(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                depends_on,
                materialized: DbtMaterialization::View,
                quoting: ResolvedQuoting::trues(),
                enabled: true,
                extended_model: false,
                static_analysis: StaticAnalysisKind::Strict.into(),
                ..Default::default()
            },
            __model_attr__: DbtModelAttr {
                access: Access::Protected,
                group: None,
                introspection: IntrospectionKind::None,
                version: None,
                latest_version: None,
                constraints: vec![],
                deprecation_date: None,
                primary_key: vec![],
                time_spine: None,
                contract: None,
                incremental_strategy: None,
                freshness: None,
                state: None,
                event_time: None,
                catalog_name: None,
                table_format: None,
                sync: None,
                compiled_code: None,
            },
            __adapter_attr__: AdapterAttr::default(),
            deprecated_config: ModelConfig::default(),
            __other__: BTreeMap::new(),
        };

        (id.to_string(), Arc::new(model))
    }

    #[test]
    fn test_simple_cycle() {
        let mut nodes = Nodes::default();
        let loc1 = CodeLocationWithFile::new(1, 1, 0, "model_a.sql");
        let loc2 = CodeLocationWithFile::new(1, 1, 0, "model_b.sql");

        // a -> b -> a
        let (id_a, model_a) = create_test_node("model.a", vec![("model.b", loc1.clone())]);
        let (id_b, model_b) = create_test_node("model.b", vec![("model.a", loc2.clone())]);

        nodes.models.insert(id_a.clone(), model_a);
        nodes.models.insert(id_b.clone(), model_b);

        let mut deps = BTreeMap::new();
        deps.insert(id_a.clone(), BTreeSet::from([id_b.clone()]));
        deps.insert(id_b, BTreeSet::from([id_a]));

        let (warnings, errors) = find_cycle_diagnostics(&deps, &nodes);

        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 3); // One for cycle detection, two for dependencies
        assert_contains!(
            errors[0].to_string(),
            "Cycle detected: [model.a] -> [model.b] -> [model.a]"
        );
        assert_eq!(
            errors[0].location.clone().unwrap_or_default(),
            CodeLocationWithFile::default()
        );
        assert_contains!(
            errors[1].to_string(),
            "Cycle 1:  [model.a] depends on [model.b]"
        );
        assert_eq!(errors[1].location.as_ref(), Some(&loc1));
        assert_contains!(
            errors[2].to_string(),
            "Cycle 1:  [model.b] depends on [model.a]"
        );
        assert_eq!(errors[2].location.as_ref(), Some(&loc2));
    }

    #[test]
    fn test_multiple_references_cycle() {
        let mut nodes = Nodes::default();
        let loc1_a = CodeLocationWithFile::new(1, 1, 0, "model_a.sql");
        let loc2_a = CodeLocationWithFile::new(2, 1, 0, "model_a.sql");
        let loc3 = CodeLocationWithFile::new(1, 1, 0, "model_b.sql");

        // a -> b -> a
        //       \-> a
        let (id_a, model_a) = create_test_node(
            "model.a",
            vec![("model.b", loc1_a.clone()), ("model.b", loc2_a.clone())],
        );
        let (id_b, model_b) = create_test_node("model.b", vec![("model.a", loc3.clone())]);

        nodes.models.insert(id_a.clone(), model_a);
        nodes.models.insert(id_b.clone(), model_b);

        let mut deps = BTreeMap::new();
        deps.insert(id_a.clone(), BTreeSet::from([id_b.clone()]));
        deps.insert(id_b, BTreeSet::from([id_a]));

        let (warnings, errors) = find_cycle_diagnostics(&deps, &nodes);

        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 4);
        assert_contains!(
            errors[0].to_string(),
            "Cycle detected: [model.a] -> [model.b] -> [model.a]"
        );
        assert_eq!(
            errors[0].location.clone().unwrap_or_default(),
            CodeLocationWithFile::default()
        );
        assert_contains!(
            errors[1].to_string(),
            "Cycle 1:  [model.a] depends on [model.b]"
        );
        assert_eq!(errors[1].location.as_ref(), Some(&loc1_a));
        assert_contains!(
            errors[2].to_string(),
            "Cycle 1:  [model.a] depends on [model.b]"
        );
        assert_eq!(errors[2].location.as_ref(), Some(&loc2_a));
        assert_contains!(
            errors[3].to_string(),
            "Cycle 1:  [model.b] depends on [model.a]"
        );
        assert_eq!(errors[3].location.as_ref(), Some(&loc3));
    }

    #[test]
    fn test_self_cycle() {
        let mut nodes = Nodes::default();
        let loc1 = CodeLocationWithFile::new(1, 1, 0, "model_a.sql");
        let loc2 = CodeLocationWithFile::new(2, 1, 0, "model_a.sql");

        // self dependency
        // a -> a
        let (id_a, model_a) = create_test_node(
            "model.a",
            vec![("model.a", loc1.clone()), ("model.a", loc2.clone())],
        );

        nodes.models.insert(id_a.clone(), model_a);

        let mut deps = BTreeMap::new();
        deps.insert(id_a.clone(), BTreeSet::from([id_a]));

        let (warnings, errors) = find_cycle_diagnostics(&deps, &nodes);

        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 3);
        assert_contains!(
            errors[0].to_string(),
            "Cycle detected: [model.a] -> [model.a]"
        );
        assert_eq!(
            errors[0].location.clone().unwrap_or_default(),
            CodeLocationWithFile::default()
        );
        assert_contains!(
            errors[1].to_string(),
            "Cycle 1:  [model.a] depends on [model.a]"
        );
        assert_eq!(errors[1].location.as_ref(), Some(&loc1));
        assert_contains!(
            errors[2].to_string(),
            "Cycle 1:  [model.a] depends on [model.a]"
        );
        assert_eq!(errors[2].location.as_ref(), Some(&loc2));
    }

    #[test]
    fn test_multiple_cycles() {
        let mut nodes = Nodes::default();
        let loc1 = CodeLocationWithFile::new(1, 1, 0, "model_a.sql");
        let loc2 = CodeLocationWithFile::new(1, 1, 0, "model_b.sql");
        let loc3 = CodeLocationWithFile::new(1, 1, 0, "model_c.sql");
        let loc4 = CodeLocationWithFile::new(1, 1, 0, "model_d.sql");

        // two independent cycles:
        // a -> b -> a
        // c -> d -> c
        let (id_a, model_a) = create_test_node("model.a", vec![("model.b", loc1.clone())]);
        let (id_b, model_b) = create_test_node("model.b", vec![("model.a", loc2.clone())]);
        let (id_c, model_c) = create_test_node("model.c", vec![("model.d", loc3.clone())]);
        let (id_d, model_d) = create_test_node("model.d", vec![("model.c", loc4.clone())]);

        nodes.models.insert(id_a.clone(), model_a);
        nodes.models.insert(id_b.clone(), model_b);
        nodes.models.insert(id_c.clone(), model_c);
        nodes.models.insert(id_d.clone(), model_d);

        let mut deps = BTreeMap::new();
        deps.insert(id_a.clone(), BTreeSet::from([id_b.clone()]));
        deps.insert(id_b, BTreeSet::from([id_a]));
        deps.insert(id_c.clone(), BTreeSet::from([id_d.clone()]));
        deps.insert(id_d, BTreeSet::from([id_c]));

        let (warnings, errors) = find_cycle_diagnostics(&deps, &nodes);

        assert!(warnings.is_empty());
        assert_eq!(errors.len(), 6);
        assert_contains!(
            errors[0].to_string(),
            "Cycle detected: [model.a] -> [model.b] -> [model.a]"
        );
        assert_eq!(
            errors[0].location.clone().unwrap_or_default(),
            CodeLocationWithFile::default()
        );
        assert_contains!(
            errors[1].to_string(),
            "Cycle 1:  [model.a] depends on [model.b]"
        );
        assert_eq!(errors[1].location.as_ref(), Some(&loc1));
        assert_contains!(
            errors[2].to_string(),
            "Cycle 1:  [model.b] depends on [model.a]"
        );
        assert_eq!(errors[2].location.as_ref(), Some(&loc2));
        // Second cycle
        assert_contains!(
            errors[3].to_string(),
            "Cycle detected: [model.c] -> [model.d] -> [model.c]"
        );
        assert_eq!(
            errors[3].location.clone().unwrap_or_default(),
            CodeLocationWithFile::default()
        );
        assert_contains!(
            errors[4].to_string(),
            "Cycle 2:  [model.c] depends on [model.d]"
        );
        assert_eq!(errors[4].location.as_ref(), Some(&loc3));
        assert_contains!(
            errors[5].to_string(),
            "Cycle 2:  [model.d] depends on [model.c]"
        );
        assert_eq!(errors[5].location.as_ref(), Some(&loc4));
    }

    #[test]
    fn test_extended_model_as_frontier() {
        // Test that extended models are treated as frontier nodes when selected
        let mut nodes = Nodes::default();

        // Create an extended model
        let extended_model = Arc::new(DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: "model.extended".to_string(),
                tags: vec![],
                meta: IndexMap::new(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                materialized: DbtMaterialization::View,
                quoting: ResolvedQuoting::trues(),
                enabled: true,
                extended_model: true, // This is an extended model
                static_analysis: StaticAnalysisKind::Strict.into(),
                ..Default::default()
            },
            __model_attr__: DbtModelAttr {
                access: Access::Protected,
                group: None,
                introspection: IntrospectionKind::None,
                version: None,
                latest_version: None,
                constraints: vec![],
                deprecation_date: None,
                primary_key: vec![],
                time_spine: None,
                contract: None,
                incremental_strategy: None,
                freshness: None,
                state: None,
                event_time: None,
                catalog_name: None,
                table_format: None,
                sync: None,
                compiled_code: None,
            },
            __adapter_attr__: AdapterAttr::default(),
            deprecated_config: ModelConfig::default(),
            __other__: BTreeMap::new(),
        });

        // Create a regular model that depends on the extended model
        let regular_model = Arc::new(DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: "model.regular".to_string(),
                tags: vec![],
                meta: IndexMap::new(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                depends_on: NodeDependsOn {
                    nodes_with_ref_location: vec![(
                        "model.extended".to_string(),
                        CodeLocationWithFile::default(),
                    )],
                    ..Default::default()
                },
                materialized: DbtMaterialization::View,
                quoting: ResolvedQuoting::trues(),
                enabled: true,
                extended_model: false,
                static_analysis: StaticAnalysisKind::Strict.into(),
                ..Default::default()
            },
            __model_attr__: DbtModelAttr {
                access: Access::Protected,
                group: None,
                introspection: IntrospectionKind::None,
                version: None,
                latest_version: None,
                constraints: vec![],
                deprecation_date: None,
                primary_key: vec![],
                time_spine: None,
                contract: None,
                incremental_strategy: None,
                freshness: None,
                state: None,
                event_time: None,
                catalog_name: None,
                table_format: None,
                sync: None,
                compiled_code: None,
            },
            __adapter_attr__: AdapterAttr::default(),
            deprecated_config: ModelConfig::default(),
            __other__: BTreeMap::new(),
        });

        nodes
            .models
            .insert("model.extended".to_string(), extended_model);
        nodes
            .models
            .insert("model.regular".to_string(), regular_model);

        // Create dependencies: regular -> extended
        let mut deps = BTreeMap::new();
        deps.insert(
            "model.regular".to_string(),
            BTreeSet::from(["model.extended".to_string()]),
        );
        deps.insert("model.extended".to_string(), BTreeSet::new());

        // Select both models
        let resolved_selectors = ResolvedSelector {
            include: None, // Select all
            exclude: None,
            ..Default::default()
        };

        let args = SchedulerArgs {
            command: FsCommand::Test,
            io: Default::default(),
            resource_types: vec![],
            exclude_resource_types: vec![],
            exclude_unique_ids: Default::default(),
            selection_override: None,
        };

        let (schedule, _) = schedule_graph(
            &deps,
            &nodes,
            None,
            &resolved_selectors,
            None,
            &args,
            AdapterType::Bigquery,
        )
        .unwrap();

        // Verify that:
        // 1. Extended model is NOT in selected_nodes
        assert!(!schedule.selected_nodes.contains("model.extended"));
        // 2. Regular model IS in selected_nodes
        assert_contains!(schedule.selected_nodes, "model.regular");
        // 3. Extended model IS in frontier_nodes
        assert_contains!(schedule.frontier_nodes, "model.extended");
    }

    fn make_model_with_materialization(
        uid: &str,
        materialized: DbtMaterialization,
        deps: &[&str],
    ) -> Arc<DbtModel> {
        Arc::new(DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: uid.to_string(),
                tags: vec![],
                meta: IndexMap::new(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                depends_on: NodeDependsOn {
                    nodes_with_ref_location: deps
                        .iter()
                        .map(|d| ((*d).to_string(), CodeLocationWithFile::default()))
                        .collect(),
                    ..Default::default()
                },
                materialized,
                quoting: ResolvedQuoting::trues(),
                enabled: true,
                extended_model: false,
                static_analysis: StaticAnalysisKind::Strict.into(),
                ..Default::default()
            },
            __model_attr__: DbtModelAttr {
                access: Access::Protected,
                group: None,
                introspection: IntrospectionKind::None,
                version: None,
                latest_version: None,
                constraints: vec![],
                deprecation_date: None,
                primary_key: vec![],
                time_spine: None,
                contract: None,
                incremental_strategy: None,
                freshness: None,
                state: None,
                event_time: None,
                catalog_name: None,
                table_format: None,
                sync: None,
                compiled_code: None,
            },
            __adapter_attr__: AdapterAttr::default(),
            deprecated_config: ModelConfig::default(),
            __other__: BTreeMap::new(),
        })
    }

    /// Run `show -s <select>` (by fqn) against the given nodes/deps and return the schedule.
    fn show_schedule(
        nodes: &Nodes,
        deps: &BTreeMap<String, BTreeSet<String>>,
        select: &str,
    ) -> Schedule<String> {
        let resolved_selectors = ResolvedSelector {
            include: Some(SelectExpression::Atom(SelectionCriteria::new(
                MethodName::Fqn,
                vec![],
                select.to_string(),
                false,
                None,
                None,
                None,
                None,
            ))),
            exclude: None,
            ..Default::default()
        };
        let args = SchedulerArgs {
            command: FsCommand::Show,
            io: Default::default(),
            resource_types: vec![],
            exclude_resource_types: vec![],
            exclude_unique_ids: Default::default(),
            selection_override: None,
        };
        schedule_graph(
            deps,
            nodes,
            None,
            &resolved_selectors,
            None,
            &args,
            AdapterType::Bigquery,
        )
        .unwrap()
        .0
    }

    /// Nodes pulled in only by ephemeral-ancestor expansion: `selected_nodes - all_selected_nodes`.
    fn expansion_only_nodes(schedule: &Schedule<String>) -> BTreeSet<String> {
        schedule
            .selected_nodes
            .difference(&schedule.all_selected_nodes)
            .cloned()
            .collect()
    }

    #[test]
    fn test_show_expands_ephemeral_ancestor_but_marks_it_as_expansion_only() {
        // `show -s tbl` where `tbl` refs an ephemeral model `eph` must pull `eph` into
        // selected_nodes (so it gets rendered/written to target/ephemeral/eph.sql), but `eph`
        // must be tracked as expansion-only so it doesn't trip the show single-node check or
        // get a Show phase task.
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.eph".to_string(),
            make_model_with_materialization("model.eph", DbtMaterialization::Ephemeral, &[]),
        );
        nodes.models.insert(
            "model.tbl".to_string(),
            make_model_with_materialization("model.tbl", DbtMaterialization::Table, &["model.eph"]),
        );

        let deps = BTreeMap::from([
            (
                "model.tbl".to_string(),
                BTreeSet::from(["model.eph".to_string()]),
            ),
            ("model.eph".to_string(), BTreeSet::new()),
        ]);

        let schedule = show_schedule(&nodes, &deps, "model.tbl");

        assert_contains!(schedule.selected_nodes, "model.tbl");
        assert_contains!(schedule.selected_nodes, "model.eph");
        let expansion_only = expansion_only_nodes(&schedule);
        assert!(
            !expansion_only.contains("model.tbl"),
            "directly-selected node must not be marked as expansion-only"
        );
        assert_contains!(expansion_only, "model.eph");
    }

    #[test]
    fn test_show_directly_selected_ephemeral_is_not_expansion_only() {
        // `show -s eph` selecting the ephemeral model directly should keep full behavior
        // (e.g. a Show phase task), since it wasn't pulled in only to satisfy a downstream ref.
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.eph".to_string(),
            make_model_with_materialization("model.eph", DbtMaterialization::Ephemeral, &[]),
        );

        let deps = BTreeMap::from([("model.eph".to_string(), BTreeSet::new())]);

        let schedule = show_schedule(&nodes, &deps, "model.eph");

        assert_contains!(schedule.selected_nodes, "model.eph");
        // A directly selected ephemeral node is in `all_selected_nodes`, so it must not appear
        // among the expansion-only nodes.
        assert!(
            !expansion_only_nodes(&schedule).contains("model.eph"),
            "directly-selected ephemeral node must not be marked as expansion-only"
        );
    }

    #[test]
    fn test_no_cycles() {
        let mut nodes = Nodes::default();
        let loc1 = CodeLocationWithFile::new(1, 1, 0, "model_a.sql");

        // a -> b no cycles!
        let (id_a, model_a) = create_test_node("model.a", vec![("model.b", loc1)]);
        let (id_b, model_b) = create_test_node("model.b", vec![]);

        nodes.models.insert(id_a.clone(), model_a);
        nodes.models.insert(id_b.clone(), model_b);

        let mut deps = BTreeMap::new();
        deps.insert(id_a, BTreeSet::from([id_b.clone()]));
        deps.insert(id_b, BTreeSet::new());

        let (warnings, errors) = find_cycle_diagnostics(&deps, &nodes);

        assert!(warnings.is_empty());
        assert!(errors.is_empty());
    }
}

#[cfg(test)]
mod selector_method_tests {
    use super::*;
    use dbt_common::{
        io_args::StaticAnalysisKind,
        node_selector::{MethodName, SelectionCriteria},
    };
    use dbt_schemas::schemas::{
        CommonAttributes, DbtModel, DbtModelAttr, IntrospectionKind, NodeBaseAttributes,
        common::{Access, DbtMaterialization, ResolvedQuoting},
        nodes::AdapterAttr,
        project::ModelConfig,
        selectors::{SelectorDefinitionValue, SelectorEntry},
    };
    use indexmap::IndexMap;
    use std::sync::Arc;

    fn make_tagged_model(unique_id: &str, tags: Vec<&str>) -> (String, Arc<DbtModel>) {
        (
            unique_id.to_string(),
            Arc::new(DbtModel {
                __common_attr__: CommonAttributes {
                    unique_id: unique_id.to_string(),
                    name: unique_id.to_string(),
                    package_name: "package".to_string(),
                    tags: tags.into_iter().map(String::from).collect(),
                    meta: IndexMap::new(),
                    ..Default::default()
                },
                __base_attr__: NodeBaseAttributes {
                    database: "db".to_string(),
                    schema: "schema".to_string(),
                    materialized: DbtMaterialization::View,
                    quoting: ResolvedQuoting::trues(),
                    enabled: true,
                    extended_model: false,
                    static_analysis: StaticAnalysisKind::Strict.into(),
                    ..Default::default()
                },
                __model_attr__: DbtModelAttr {
                    introspection: IntrospectionKind::None,
                    access: Access::Protected,
                    ..Default::default()
                },
                __adapter_attr__: AdapterAttr::default(),
                deprecated_config: ModelConfig::default(),
                __other__: BTreeMap::new(),
            }),
        )
    }

    fn tagged_nodes() -> Nodes {
        let mut nodes = Nodes::default();
        let (id, m) = make_tagged_model("model.a", vec!["nightly", "core"]);
        nodes.models.insert(id, m);
        let (id, m) = make_tagged_model("model.b", vec!["nightly"]);
        nodes.models.insert(id, m);
        let (id, m) = make_tagged_model("model.c", vec!["core"]);
        nodes.models.insert(id, m);
        let (id, m) = make_tagged_model("model.d", vec![]);
        nodes.models.insert(id, m);
        nodes
    }

    fn make_deps() -> BTreeMap<String, BTreeSet<String>> {
        let mut deps = BTreeMap::new();
        // model.a -> model.b -> model.d
        // model.a -> model.c
        deps.insert(
            "model.a".to_string(),
            BTreeSet::from(["model.b".to_string(), "model.c".to_string()]),
        );
        deps.insert(
            "model.b".to_string(),
            BTreeSet::from(["model.d".to_string()]),
        );
        deps.insert("model.c".to_string(), BTreeSet::new());
        deps.insert("model.d".to_string(), BTreeSet::new());
        deps
    }

    fn selector_atom(name: &str) -> SelectExpression {
        SelectExpression::Atom(SelectionCriteria::new(
            MethodName::Selector,
            vec![],
            name.to_string(),
            false,
            None,
            None,
            None,
            None,
        ))
    }

    fn selector_atom_with_parents(name: &str, depth: u32) -> SelectExpression {
        SelectExpression::Atom(SelectionCriteria::new(
            MethodName::Selector,
            vec![],
            name.to_string(),
            false,
            Some(depth),
            None,
            None,
            None,
        ))
    }

    fn tag_selector(tag: &str) -> SelectExpression {
        SelectExpression::Atom(SelectionCriteria::new(
            MethodName::Tag,
            vec![],
            tag.to_string(),
            false,
            None,
            None,
            Some(IndirectSelection::Eager),
            None,
        ))
    }

    fn selector_entry(include: SelectExpression) -> SelectorEntry {
        SelectorEntry {
            include,
            is_default: false,
            description: None,
            definition: SelectorDefinitionValue::String(String::new()),
        }
    }

    #[test]
    fn test_selector_method_basic_resolution() {
        let nodes = tagged_nodes();
        let deps = make_deps();
        let mut sel_defs = HashMap::new();
        sel_defs.insert(
            "nightly_models".to_string(),
            selector_entry(tag_selector("nightly")),
        );

        let mut stack = Vec::new();
        let result = eval_selector(
            &selector_atom("nightly_models"),
            &deps,
            &nodes,
            None,
            None,
            AdapterType::Bigquery,
            &sel_defs,
            &mut stack,
        )
        .unwrap();

        let selected = result.into_final();
        assert!(selected.contains("model.a"));
        assert!(selected.contains("model.b"));
        assert!(!selected.contains("model.c"));
        assert!(!selected.contains("model.d"));
    }

    #[test]
    fn test_selector_method_with_graph_operators() {
        let nodes = tagged_nodes();
        let deps = make_deps();
        let mut sel_defs = HashMap::new();
        // "core_models" selects tag:core → {model.a, model.c}
        sel_defs.insert(
            "core_models".to_string(),
            selector_entry(tag_selector("core")),
        );

        let mut stack = Vec::new();
        // 1+selector:core_models — should include parents of {model.a, model.c}
        let result = eval_selector(
            &selector_atom_with_parents("core_models", 1),
            &deps,
            &nodes,
            None,
            None,
            AdapterType::Bigquery,
            &sel_defs,
            &mut stack,
        )
        .unwrap();

        let selected = result.into_final();
        // model.a and model.c are selected by tag:core
        assert!(selected.contains("model.a"));
        assert!(selected.contains("model.c"));
        // model.a depends on model.b and model.c, so model.b and model.c are parents of model.a
        // model.c has no parents
        // With depth=1 parents from {model.a, model.c}: model.b, model.c (parents of model.a)
        assert!(selected.contains("model.b"));
    }

    #[test]
    fn test_selector_method_unknown_selector_error() {
        let nodes = tagged_nodes();
        let deps = make_deps();
        let sel_defs = HashMap::new();

        let mut stack = Vec::new();
        let result = eval_selector(
            &selector_atom("nonexistent"),
            &deps,
            &nodes,
            None,
            None,
            AdapterType::Bigquery,
            &sel_defs,
            &mut stack,
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::SelectorError);
        assert!(err.to_string().contains("Unknown selector"));
    }

    #[test]
    fn test_selector_method_circular_reference() {
        let nodes = tagged_nodes();
        let deps = make_deps();
        let mut sel_defs = HashMap::new();
        // alpha references beta, beta references alpha
        sel_defs.insert("alpha".to_string(), selector_entry(selector_atom("beta")));
        sel_defs.insert("beta".to_string(), selector_entry(selector_atom("alpha")));

        let mut stack = Vec::new();
        let result = eval_selector(
            &selector_atom("alpha"),
            &deps,
            &nodes,
            None,
            None,
            AdapterType::Bigquery,
            &sel_defs,
            &mut stack,
        );

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::SelectorError);
        assert!(err.to_string().contains("Circular selector reference"));
    }

    #[test]
    fn test_selector_method_wildcard() {
        let nodes = tagged_nodes();
        let deps = make_deps();
        let mut sel_defs = HashMap::new();
        sel_defs.insert(
            "nightly_models".to_string(),
            selector_entry(tag_selector("nightly")),
        );
        sel_defs.insert(
            "nightly_seeds".to_string(),
            selector_entry(tag_selector("core")),
        );

        let mut stack = Vec::new();
        // selector:nightly_* matches both nightly_models and nightly_seeds
        let result = eval_selector(
            &selector_atom("nightly_*"),
            &deps,
            &nodes,
            None,
            None,
            AdapterType::Bigquery,
            &sel_defs,
            &mut stack,
        )
        .unwrap();

        let selected = result.into_final();
        // union of tag:nightly and tag:core = {model.a, model.b, model.c}
        assert!(selected.contains("model.a"));
        assert!(selected.contains("model.b"));
        assert!(selected.contains("model.c"));
        assert!(!selected.contains("model.d"));
    }

    #[test]
    fn test_selector_method_transitive_resolution() {
        let nodes = tagged_nodes();
        let deps = make_deps();
        let mut sel_defs = HashMap::new();
        sel_defs.insert("base".to_string(), selector_entry(tag_selector("nightly")));
        // "derived" references "base"
        sel_defs.insert("derived".to_string(), selector_entry(selector_atom("base")));

        let mut stack = Vec::new();
        let result = eval_selector(
            &selector_atom("derived"),
            &deps,
            &nodes,
            None,
            None,
            AdapterType::Bigquery,
            &sel_defs,
            &mut stack,
        )
        .unwrap();

        let selected = result.into_final();
        // derived -> base -> tag:nightly = {model.a, model.b}
        assert!(selected.contains("model.a"));
        assert!(selected.contains("model.b"));
        assert!(!selected.contains("model.c"));
    }
}

// ---------------------------------------------------------------
// modify_schedule_for_sidecar_compute_boundaries unit tests
// ---------------------------------------------------------------
#[cfg(test)]
mod sidecar_compute_boundary {
    use std::sync::Arc;

    use super::*;
    use dbt_common::CodeLocationWithFile;
    use dbt_common::io_args::ComputeArg;
    use dbt_schemas::schemas::common::NodeDependsOn;
    use dbt_schemas::schemas::{
        CommonAttributes, DbtModel, DbtSnapshot, DbtSnapshotAttr, DbtTest, DbtUnitTest,
        NodeBaseAttributes,
    };

    fn dep_list(deps: &[&str]) -> NodeDependsOn {
        NodeDependsOn {
            nodes: deps.iter().map(|d| (*d).to_string()).collect(),
            nodes_with_ref_location: deps
                .iter()
                .map(|d| ((*d).to_string(), CodeLocationWithFile::default()))
                .collect(),
            ..Default::default()
        }
    }

    fn make_model(uid: &str, compute: Option<ComputeArg>, deps: &[&str]) -> Arc<DbtModel> {
        Arc::new(DbtModel {
            __common_attr__: CommonAttributes {
                name: uid.to_string(),
                package_name: "package".to_string(),
                unique_id: uid.to_string(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                compute,
                depends_on: dep_list(deps),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    fn make_snapshot(uid: &str, compute: Option<ComputeArg>, deps: &[&str]) -> Arc<DbtSnapshot> {
        Arc::new(DbtSnapshot {
            __common_attr__: CommonAttributes {
                name: uid.to_string(),
                package_name: "package".to_string(),
                unique_id: uid.to_string(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                compute,
                depends_on: dep_list(deps),
                ..Default::default()
            },
            __snapshot_attr__: DbtSnapshotAttr::default(),
            ..Default::default()
        })
    }

    fn make_test(uid: &str, compute: Option<ComputeArg>, deps: &[&str]) -> Arc<DbtTest> {
        Arc::new(DbtTest {
            __common_attr__: CommonAttributes {
                name: uid.to_string(),
                package_name: "package".to_string(),
                unique_id: uid.to_string(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                compute,
                depends_on: dep_list(deps),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    fn make_unit_test(uid: &str, compute: Option<ComputeArg>, deps: &[&str]) -> Arc<DbtUnitTest> {
        Arc::new(DbtUnitTest {
            __common_attr__: CommonAttributes {
                name: uid.to_string(),
                package_name: "package".to_string(),
                unique_id: uid.to_string(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                compute,
                depends_on: dep_list(deps),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    fn schedule(
        selected: &[&str],
        frontier: &[&str],
        sorted: &[&str],
        deps: &[(&str, &[&str])],
    ) -> Schedule<String> {
        let selected_set = set(selected);
        let frontier_set = set(frontier);
        let all_selected: BTreeSet<String> = selected_set.union(&frontier_set).cloned().collect();
        Schedule {
            selected_nodes: selected_set,
            frontier_nodes: frontier_set,
            all_selected_nodes: all_selected,
            sorted_nodes: vec_str(sorted),
            deps: deps_map(deps),
            ..Default::default()
        }
    }

    fn set(uids: &[&str]) -> BTreeSet<String> {
        uids.iter().map(|s| (*s).to_string()).collect()
    }

    fn vec_str(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    fn deps_map(entries: &[(&str, &[&str])]) -> BTreeMap<String, BTreeSet<String>> {
        entries
            .iter()
            .map(|(k, vs)| {
                (
                    (*k).to_string(),
                    vs.iter().map(|s| (*s).to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn models_and_snapshots_with_compute_remote() {
        // (a) Linear chain: A -> B(remote). B moves to frontier; A stays.
        // sorted_nodes and deps are unchanged because no test was dropped.
        {
            let mut sched = schedule(
                &["model.a", "model.b"],
                &[],
                &["model.a", "model.b"],
                &[("model.a", &[]), ("model.b", &["model.a"])],
            );
            let mut nodes = Nodes::default();
            nodes
                .models
                .insert("model.a".into(), make_model("model.a", None, &[]));
            nodes.models.insert(
                "model.b".into(),
                make_model("model.b", Some(ComputeArg::Remote), &["model.a"]),
            );

            modify_schedule_for_sidecar_compute_boundaries(&mut sched, &nodes);

            assert_eq!(sched.selected_nodes, set(&["model.a"]));
            assert_eq!(sched.frontier_nodes, set(&["model.b"]));
            assert_eq!(sched.all_selected_nodes, set(&["model.a", "model.b"]));
            assert_eq!(sched.sorted_nodes, vec_str(&["model.a", "model.b"]));
            assert_eq!(
                sched.deps,
                deps_map(&[("model.a", &[]), ("model.b", &["model.a"])]),
            );
        }

        // (b) Branching DAG with two interior boundaries:
        //
        //   A -> B(remote) -> C -> D
        //          |
        //          +-------> E(remote) -> F
        //
        // B and E both move to frontier; their non-remote neighbors stay
        // selected.
        {
            let mut sched = schedule(
                &[
                    "model.a", "model.b", "model.c", "model.d", "model.e", "model.f",
                ],
                &[],
                &[
                    "model.a", "model.b", "model.c", "model.d", "model.e", "model.f",
                ],
                &[
                    ("model.a", &[]),
                    ("model.b", &["model.a"]),
                    ("model.c", &["model.b"]),
                    ("model.d", &["model.c"]),
                    ("model.e", &["model.b"]),
                    ("model.f", &["model.e"]),
                ],
            );
            let mut nodes = Nodes::default();
            nodes
                .models
                .insert("model.a".into(), make_model("model.a", None, &[]));
            nodes.models.insert(
                "model.b".into(),
                make_model("model.b", Some(ComputeArg::Remote), &["model.a"]),
            );
            nodes
                .models
                .insert("model.c".into(), make_model("model.c", None, &["model.b"]));
            nodes
                .models
                .insert("model.d".into(), make_model("model.d", None, &["model.c"]));
            nodes.models.insert(
                "model.e".into(),
                make_model("model.e", Some(ComputeArg::Remote), &["model.b"]),
            );
            nodes
                .models
                .insert("model.f".into(), make_model("model.f", None, &["model.e"]));

            modify_schedule_for_sidecar_compute_boundaries(&mut sched, &nodes);

            assert_eq!(
                sched.selected_nodes,
                set(&["model.a", "model.c", "model.d", "model.f"])
            );
            assert_eq!(sched.frontier_nodes, set(&["model.b", "model.e"]));
            assert_eq!(
                sched.all_selected_nodes,
                set(&[
                    "model.a", "model.b", "model.c", "model.d", "model.e", "model.f",
                ])
            );
            assert_eq!(
                sched.sorted_nodes,
                vec_str(&[
                    "model.a", "model.b", "model.c", "model.d", "model.e", "model.f",
                ])
            );
            assert_eq!(
                sched.deps,
                deps_map(&[
                    ("model.a", &[]),
                    ("model.b", &["model.a"]),
                    ("model.c", &["model.b"]),
                    ("model.d", &["model.c"]),
                    ("model.e", &["model.b"]),
                    ("model.f", &["model.e"]),
                ]),
            );
        }

        // (c) Snapshot: A -> snap.s(remote) -> C.
        // Snapshots use the same move-to-frontier path as models.
        {
            let mut sched = schedule(
                &["model.a", "snapshot.s", "model.c"],
                &[],
                &["model.a", "snapshot.s", "model.c"],
                &[
                    ("model.a", &[]),
                    ("snapshot.s", &["model.a"]),
                    ("model.c", &["snapshot.s"]),
                ],
            );
            let mut nodes = Nodes::default();
            nodes
                .models
                .insert("model.a".into(), make_model("model.a", None, &[]));
            nodes.snapshots.insert(
                "snapshot.s".into(),
                make_snapshot("snapshot.s", Some(ComputeArg::Remote), &["model.a"]),
            );
            nodes.models.insert(
                "model.c".into(),
                make_model("model.c", None, &["snapshot.s"]),
            );

            modify_schedule_for_sidecar_compute_boundaries(&mut sched, &nodes);

            assert_eq!(sched.selected_nodes, set(&["model.a", "model.c"]));
            assert_eq!(sched.frontier_nodes, set(&["snapshot.s"]));
            assert_eq!(
                sched.all_selected_nodes,
                set(&["model.a", "snapshot.s", "model.c"])
            );
            assert_eq!(
                sched.sorted_nodes,
                vec_str(&["model.a", "snapshot.s", "model.c"])
            );
            assert_eq!(
                sched.deps,
                deps_map(&[
                    ("model.a", &[]),
                    ("snapshot.s", &["model.a"]),
                    ("model.c", &["snapshot.s"]),
                ]),
            );
        }
    }

    #[test]
    fn tests_with_compute_remote() {
        // tests marked as remote are dropped from selected_nodes, sorted_nodes,
        // and the deps map (both as keys and from any other entry's value set).
        // all_selected_nodes is preserved.
        {
            let mut sched = schedule(
                &["test.t", "unit_test.u"],
                &[],
                &["test.t", "unit_test.u"],
                &[("test.t", &[]), ("unit_test.u", &[])],
            );
            let mut nodes = Nodes::default();
            nodes.tests.insert(
                "test.t".into(),
                make_test("test.t", Some(ComputeArg::Remote), &[]),
            );
            nodes.unit_tests.insert(
                "unit_test.u".into(),
                make_unit_test("unit_test.u", Some(ComputeArg::Remote), &[]),
            );

            modify_schedule_for_sidecar_compute_boundaries(&mut sched, &nodes);

            assert_eq!(sched.selected_nodes, set(&[]));
            assert_eq!(sched.frontier_nodes, set(&[]));
            assert_eq!(sched.all_selected_nodes, set(&["test.t", "unit_test.u"]));
            assert_eq!(sched.sorted_nodes, vec_str(&[]));
            assert_eq!(sched.deps, deps_map(&[]));
        }

        // tests whose parents are marked as remote are also dropped. The remote
        // model stays in sorted_nodes/deps but moves from selected to frontier;
        // the dropped tests are removed from sorted_nodes and the deps map.
        {
            let mut sched = schedule(
                &["model.b", "test.t_on_b", "unit_test.u_on_b"],
                &[],
                &["model.b", "test.t_on_b", "unit_test.u_on_b"],
                &[
                    ("model.b", &[]),
                    ("test.t_on_b", &["model.b"]),
                    ("unit_test.u_on_b", &["model.b"]),
                ],
            );
            let mut nodes = Nodes::default();
            nodes.models.insert(
                "model.b".into(),
                make_model("model.b", Some(ComputeArg::Remote), &[]),
            );
            nodes.tests.insert(
                "test.t_on_b".into(),
                make_test("test.t_on_b", None, &["model.b"]),
            );
            nodes.unit_tests.insert(
                "unit_test.u_on_b".into(),
                make_unit_test("unit_test.u_on_b", None, &["model.b"]),
            );

            modify_schedule_for_sidecar_compute_boundaries(&mut sched, &nodes);

            assert_eq!(sched.selected_nodes, set(&[]));
            assert_eq!(sched.frontier_nodes, set(&["model.b"]));
            assert_eq!(
                sched.all_selected_nodes,
                set(&["model.b", "test.t_on_b", "unit_test.u_on_b"])
            );
            assert_eq!(sched.sorted_nodes, vec_str(&["model.b"]));
            assert_eq!(sched.deps, deps_map(&[("model.b", &[])]));
        }

        // tests whose parents are not marked as remote are not dropped:
        //   a -> b (remote)
        //   a -> {test.t_on_a, unit_test.u_on_a}
        // b moves to frontier; tests stay because their parent a is not remote.
        // sorted_nodes and deps are unchanged (no test was removed).
        {
            let mut sched = schedule(
                &["model.a", "model.b", "test.t_on_a", "unit_test.u_on_a"],
                &[],
                &["model.a", "model.b", "test.t_on_a", "unit_test.u_on_a"],
                &[
                    ("model.a", &[]),
                    ("model.b", &["model.a"]),
                    ("test.t_on_a", &["model.a"]),
                    ("unit_test.u_on_a", &["model.a"]),
                ],
            );
            let mut nodes = Nodes::default();
            nodes
                .models
                .insert("model.a".into(), make_model("model.a", None, &[]));
            nodes.models.insert(
                "model.b".into(),
                make_model("model.b", Some(ComputeArg::Remote), &["model.a"]),
            );
            nodes.tests.insert(
                "test.t_on_a".into(),
                make_test("test.t_on_a", None, &["model.a"]),
            );
            nodes.unit_tests.insert(
                "unit_test.u_on_a".into(),
                make_unit_test("unit_test.u_on_a", None, &["model.a"]),
            );

            modify_schedule_for_sidecar_compute_boundaries(&mut sched, &nodes);

            assert_eq!(
                sched.selected_nodes,
                set(&["model.a", "test.t_on_a", "unit_test.u_on_a"])
            );
            assert_eq!(sched.frontier_nodes, set(&["model.b"]));
            assert_eq!(
                sched.all_selected_nodes,
                set(&["model.a", "model.b", "test.t_on_a", "unit_test.u_on_a"])
            );
            assert_eq!(
                sched.sorted_nodes,
                vec_str(&["model.a", "model.b", "test.t_on_a", "unit_test.u_on_a"])
            );
            assert_eq!(
                sched.deps,
                deps_map(&[
                    ("model.a", &[]),
                    ("model.b", &["model.a"]),
                    ("test.t_on_a", &["model.a"]),
                    ("unit_test.u_on_a", &["model.a"]),
                ]),
            );
        }
    }

    #[test]
    fn no_change_when_not_applicable() {
        // No compute: remote anywhere in the dag. Schedule should not change.
        let mut sched = Schedule {
            selected_nodes: set(&["model.a", "model.b", "test.t"]),
            all_selected_nodes: set(&["model.a", "model.b", "test.t"]),
            frontier_nodes: BTreeSet::new(),
            sorted_nodes: vec![
                "model.a".to_string(),
                "model.b".to_string(),
                "test.t".to_string(),
            ],
            deps: BTreeMap::from([
                ("model.a".to_string(), BTreeSet::new()),
                (
                    "model.b".to_string(),
                    BTreeSet::from(["model.a".to_string()]),
                ),
                (
                    "test.t".to_string(),
                    BTreeSet::from(["model.b".to_string()]),
                ),
            ]),
            overlapping_sources: BTreeSet::new(),
            ..Default::default()
        };
        let before = sched.clone();

        let mut nodes = Nodes::default();
        nodes
            .models
            .insert("model.a".into(), make_model("model.a", None, &[]));
        nodes
            .models
            .insert("model.b".into(), make_model("model.b", None, &["model.a"]));
        nodes
            .tests
            .insert("test.t".into(), make_test("test.t", None, &["model.b"]));

        modify_schedule_for_sidecar_compute_boundaries(&mut sched, &nodes);

        assert_eq!(sched.selected_nodes, before.selected_nodes);
        assert_eq!(sched.all_selected_nodes, before.all_selected_nodes);
        assert_eq!(sched.frontier_nodes, before.frontier_nodes);
        assert_eq!(sched.sorted_nodes, before.sorted_nodes);
        assert_eq!(sched.deps, before.deps);
        assert_eq!(sched.overlapping_sources, before.overlapping_sources);
    }

    #[test]
    fn pre_existing_frontier_nodes_preserved() {
        // a (frontier) -> b (remote) -> c
        let mut sched = schedule(
            &["model.b", "model.c"],
            &["model.a"],
            &["model.a", "model.b", "model.c"],
            &[
                ("model.a", &[]),
                ("model.b", &["model.a"]),
                ("model.c", &["model.b"]),
            ],
        );
        let mut nodes = Nodes::default();
        nodes
            .models
            .insert("model.a".into(), make_model("model.a", None, &[]));
        nodes.models.insert(
            "model.b".into(),
            make_model("model.b", Some(ComputeArg::Remote), &["model.a"]),
        );
        nodes
            .models
            .insert("model.c".into(), make_model("model.c", None, &["model.b"]));

        modify_schedule_for_sidecar_compute_boundaries(&mut sched, &nodes);

        assert_eq!(sched.selected_nodes, set(&["model.c"]));
        assert_eq!(sched.frontier_nodes, set(&["model.a", "model.b"]));
        assert_eq!(
            sched.all_selected_nodes,
            set(&["model.a", "model.b", "model.c"])
        );
        assert_eq!(
            sched.sorted_nodes,
            vec_str(&["model.a", "model.b", "model.c"])
        );
        assert_eq!(
            sched.deps,
            deps_map(&[
                ("model.a", &[]),
                ("model.b", &["model.a"]),
                ("model.c", &["model.b"]),
            ]),
        );
    }
}
