use dbt_common::io_args::{FsCommand, OptimizeTestsOptions};
use dbt_common::static_analysis::is_strict_static_analysis;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::{ErrorCode, FsResult};
use dbt_dag::deps_mgmt::{find_all_upstream_deps, restrict_with_transitive};
use dbt_dag::schedule::Schedule;
use dbt_schemas::schemas::InternalDbtNode;
use dbt_schemas::schemas::IntrospectionKind;
use dbt_schemas::schemas::Nodes;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::profiles::Execute;
use dbt_schemas::schemas::properties::UnitTestOverrides;
use dbt_schemas::state::ResolverState;
use petgraph::Graph;
use petgraph::graph::DiGraph;
use petgraph::prelude::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use tracing::instrument;

use dbt_tasks_core::RunTasksArgs;
use dbt_tasks_core::static_analysis_buckets::StaticAnalysisBuckets;
use dbt_tasks_core::task::Task;
use dbt_tasks_core::task::{TP, TasksForNode};
use dbt_tasks_core::test_aggregation::{
    GenericTestAggregation, GenericTestRelationships, create_generic_test_aggregation,
};

use crate::barrier::BarrierTask;
use crate::cloneable::RunCloneTask;
use crate::cloneable::cloneable_task;
use crate::renderable::unit_test::build_unit_test_overrides_map;
use crate::task::TasksForNodeFactory;

const PHASES_RENDER_ANALYZE_RUN: &[TP] = &[TP::Render, TP::Analyze, TP::Run];
const PHASES_RENDER_ANALYZE: &[TP] = &[TP::Render, TP::Analyze];
const PHASES_RENDER_ANALYZE_SHOW: &[TP] = &[TP::Render, TP::Analyze, TP::Show];

fn task_graph_phases_for_command(command: FsCommand) -> Option<&'static [TP]> {
    match command {
        FsCommand::Run
        | FsCommand::Test
        | FsCommand::Build
        | FsCommand::Seed
        | FsCommand::Snapshot => Some(PHASES_RENDER_ANALYZE_RUN),
        FsCommand::Compile | FsCommand::Check | FsCommand::Extension("lineage") => {
            Some(PHASES_RENDER_ANALYZE)
        }
        FsCommand::Show => Some(PHASES_RENDER_ANALYZE_SHOW),
        _ => None,
    }
}

fn command_uses_generic_test_aggregation(command: FsCommand) -> bool {
    matches!(
        command,
        FsCommand::Run | FsCommand::Test | FsCommand::Build | FsCommand::Seed | FsCommand::Snapshot
    )
}

pub trait CompareTaskGraphBuilder: Send + Sync {
    fn build_compare_task_graph(
        &self,
        schedule: &Schedule<String>,
        resolver_state: &ResolverState,
    ) -> DiGraph<Arc<dyn Task>, ()>;
}

pub struct GraphBuilder {
    arg: Arc<RunTasksArgs>,
    execute: Execute,
    static_analysis_buckets: Arc<dyn StaticAnalysisBuckets>,
    tasks_for_node_factory: Arc<dyn TasksForNodeFactory>,
    compare_task_graph_builder: Option<Arc<dyn CompareTaskGraphBuilder>>,
}

impl GraphBuilder {
    pub fn new(
        arg: Arc<RunTasksArgs>,
        static_analysis_buckets: Arc<dyn StaticAnalysisBuckets>,
        tasks_for_node_factory: Arc<dyn TasksForNodeFactory>,
        compare_task_graph_builder: Option<Arc<dyn CompareTaskGraphBuilder>>,
    ) -> Self {
        let execute = Execute::from_compute_flag(arg.local_execution_backend);
        Self {
            arg,
            execute,
            static_analysis_buckets,
            tasks_for_node_factory,
            compare_task_graph_builder,
        }
    }

    /// Whether any selected node is in the dynamic closure so we only register hook UDFs
    /// when strict/unsafe analysis will actually bind plans.
    pub fn has_dynamic_closure(&self) -> bool {
        self.static_analysis_buckets.has_dynamic_closure()
    }

    pub fn build(
        self,
        schedule: &Schedule<String>,
        resolver_state: &Arc<ResolverState>,
    ) -> FsResult<(Graph<Arc<dyn Task>, ()>, GenericTestRelationships)> {
        let nodes = &resolver_state.nodes;

        // Test aggregation
        let generic_test_aggregation = if self
            .arg
            .optimize_tests
            .contains(&OptimizeTestsOptions::TestAggregation)
            && command_uses_generic_test_aggregation(self.arg.command)
        {
            create_generic_test_aggregation(&self.arg.io, schedule, nodes, self.execute)?
        } else {
            None
        };
        let (aggregated_schedule, aggregated_nodes) =
            if let Some(aggregation) = generic_test_aggregation.as_ref() {
                let (schedule, nodes) =
                    create_aggregated_schedule_and_nodes(schedule, nodes, aggregation);
                (Some(schedule), Some(nodes))
            } else {
                (None, None)
            };

        let generic_test_relationships = generic_test_aggregation
            .as_ref()
            .map(|agg| agg.relationships.clone())
            .unwrap_or_default();

        let (graph, nodes_with_no_tasks) = {
            match self.arg.command {
                FsCommand::Clone => (build_clone_task_graph(schedule, nodes), BTreeSet::new()),
                FsCommand::Extension("compare") => (
                    self.compare_task_graph_builder
                        .as_ref()
                        .map(|b| b.build_compare_task_graph(schedule, resolver_state))
                        .unwrap_or_default(),
                    BTreeSet::new(),
                ),
                // Fallback/default
                cmd => {
                    if let Some(phases) = task_graph_phases_for_command(cmd) {
                        let (task_schedule, task_nodes, aggregation) =
                            if command_uses_generic_test_aggregation(cmd) {
                                (
                                    aggregated_schedule.as_ref().unwrap_or(schedule),
                                    aggregated_nodes.as_ref().unwrap_or(nodes),
                                    generic_test_aggregation.as_ref(),
                                )
                            } else {
                                (schedule, nodes, None)
                            };

                        self.static_analysis_buckets
                            .will_build_phased_task_graph(self.arg.as_ref(), task_nodes);

                        Self::build_phased_task_graph(
                            task_schedule,
                            self.tasks_for_node_factory.as_ref(),
                            task_nodes,
                            self.execute,
                            phases,
                            self.static_analysis_buckets.as_ref(),
                            self.arg.infer_schemas_and_typeless,
                            aggregation,
                        )
                    } else {
                        // Freshness and `jinja-check` legitimately produce an
                        // empty task graph.
                        if self.arg.command != FsCommand::Extension("jinja-check")
                            && !self.arg.command.is_freshness_command()
                        {
                            emit_warn_log_message(
                                ErrorCode::Unexpected,
                                format!("Unhandled command: {:?}", cmd),
                            );
                        }
                        (Graph::new(), BTreeSet::new())
                    }
                }
            }
        };

        self.static_analysis_buckets
            .did_build_phased_task_graph(self.arg.as_ref(), &nodes_with_no_tasks);

        Ok((graph, generic_test_relationships))
    }

    /// The build_phased_task_graph code builds a multi-phase task graph that
    ///
    /// * respects unsafe node propagation (removing analyze phases as needed),
    /// * inserts a analyze/run barrier for safe nodes, and
    /// * adds special test-to-model run dependencies.
    #[instrument(skip_all, level = "trace")]
    pub fn build_phased_task_graph(
        schedule: &Schedule<String>,
        tasks_for_node_factory: &dyn TasksForNodeFactory,
        nodes: &Nodes,
        execute: Execute,
        phases: &[TP],
        buckets: &dyn StaticAnalysisBuckets,
        infer_schemas: bool,
        generic_test_aggregation: Option<&GenericTestAggregation>,
    ) -> (DiGraph<Arc<dyn Task>, ()>, BTreeSet<String>) {
        // Build reverse dependencies map once for efficient propagation
        let reverse_deps = build_reverse_deps(&schedule.deps);

        // Build unit test overrides map for model dependencies
        let unit_test_overrides_map = build_unit_test_overrides_map(nodes, &schedule.deps);

        // 5. Assign phases per node type and initialize senders and receivers
        let (
            mut graph,
            phase_to_nodes,
            node_indices,
            analyzeable_node_index_map,
            runnable_node_index_map,
            node_to_phases,
            nodes_with_no_tasks,
        ) = initialize_graph(
            nodes,
            phases,
            buckets,
            infer_schemas,
            schedule,
            tasks_for_node_factory,
            execute,
            &unit_test_overrides_map,
            generic_test_aggregation,
            &reverse_deps,
        );

        // Add phase transitions (e.g. render -> analyze -> run) for each node
        for unique_id in schedule.sorted_nodes.iter() {
            if let Some(node_phases) = node_to_phases.get(unique_id) {
                for window in node_phases.windows(2) {
                    let phase_i = window[0];
                    let phase_j = window[1];
                    if let (Some(&from_idx), Some(&to_idx)) = (
                        node_indices.get(&(phase_i, unique_id.clone())),
                        node_indices.get(&(phase_j, unique_id.clone())),
                    ) {
                        graph.update_edge(from_idx, to_idx, ());
                    }
                }
            }
        }

        let mut first_upstream_nodes_per_phase: BTreeMap<TP, BTreeMap<String, BTreeSet<String>>> =
            BTreeMap::new();

        // Add dependency edges for each phase
        for unique_id in schedule.sorted_nodes.iter() {
            // Get the upstream dependencies for the current node
            if let Some(deps) = schedule.deps.get(unique_id) {
                // Get the active phases for the current node
                if let Some(node_phases) = node_to_phases.get(unique_id) {
                    let is_unit_test = unique_id.starts_with("unit_test");
                    // For each active phase, add edges to the upstream dependencies
                    for &phase in node_phases {
                        // Get the task index of the current node for the active phase
                        if let Some(&to_idx) = node_indices.get(&(phase, unique_id.clone())) {
                            if is_unit_test {
                                match phase {
                                    TP::Render => {
                                        deps.iter().for_each(|dep| {
                                            // For unit tests, depend on analyze when available,
                                            // otherwise fall back to render.
                                            if let Some(dep_phases) = node_to_phases.get(dep) {
                                                let phase_to_add =
                                                    if dep_phases.contains(&TP::Analyze) {
                                                        TP::Analyze
                                                    } else {
                                                        TP::Render
                                                    };
                                                if dep_phases.contains(&phase_to_add)
                                                    && let Some(&from_idx) = node_indices
                                                        .get(&(phase_to_add, dep.clone()))
                                                {
                                                    graph.update_edge(from_idx, to_idx, ());
                                                }
                                            }
                                        });
                                        continue;
                                    }
                                    // If the current node is a unit test and the current phase is
                                    // analyze, we do not need to add any edges
                                    // since we are covered by the render dependency
                                    TP::Analyze => continue,
                                    // If the current node is a unit test and the current phase is
                                    // run, we need to draw an edge from the first upstream
                                    // dependency's run phase to the current node's run
                                    TP::Run => {
                                        if let Some(first_dep) = deps.first() {
                                            // As of today, the model being tested is stored as the
                                            // first dependency of the unit test, hence we need to
                                            // add an edge from unit test run phase to model run
                                            // phase, as unit tests run before the model and
                                            // failure blocks model execution
                                            if let Some(dep_phases) = node_to_phases.get(first_dep)
                                                && dep_phases.contains(&phase)
                                                && let Some(&from_idx) =
                                                    node_indices.get(&(phase, first_dep.clone()))
                                            {
                                                graph.update_edge(to_idx, from_idx, ());
                                            }
                                        }
                                        continue;
                                    }
                                    TP::Show => continue,
                                    // Compare phase - shouldn't appear in phased graph
                                    TP::Compare => continue,
                                };
                            } else if phase == TP::Render {
                                // Check if any deps are ephemeral before continuing
                                deps.iter().for_each(|dep| {
                                    if let Some(node) = nodes.get_node(dep)
                                        && node.materialized() == DbtMaterialization::Ephemeral
                                        && let Some(&from_idx) =
                                            node_indices.get(&(phase, dep.clone()))
                                    {
                                        graph.update_edge(from_idx, to_idx, ());
                                    }
                                });
                                continue;
                            } else {
                                // find the first upstream nodes with the same phase and add an
                                // edge to the current node. Except for analyze -> analyze edges
                                // between baseline nodes, because baseline analyze is only for the
                                // node itself — unless `--infer-schemas` is active, in which case
                                // baseline analyze *does* need its upstream's registered schema
                                // (the whole point of schema inference), so the ordering edge is
                                // required just like it is for statically-analyzed nodes.
                                if phase == TP::Analyze
                                    && buckets.in_baseline_closure(unique_id)
                                    && !infer_schemas
                                {
                                    continue;
                                }

                                // compute only once for each phase and cache it for later reuse if
                                // needed
                                let first_upstream_nodes_for_phase = first_upstream_nodes_per_phase
                                    .entry(phase)
                                    .or_insert_with(|| {
                                        compute_first_upstream_for_phase(
                                            &schedule.deps,
                                            &node_to_phases,
                                            &phase,
                                        )
                                    });

                                if let Some(nodes) = first_upstream_nodes_for_phase.get(unique_id) {
                                    for node in nodes {
                                        if let Some(&from_idx) =
                                            node_indices.get(&(phase, node.clone()))
                                        {
                                            graph.update_edge(from_idx, to_idx, ());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        // we no longer need this and can safely drop it
        drop(first_upstream_nodes_per_phase);

        // Add edges reflecting child render -> parent run / analyze for dynamic or none nodes
        for (unique_id, _) in phase_to_nodes.get(&TP::Render).unwrap().iter() {
            // Unit tests are handled above for all of their dependncies, so skip here
            if unique_id.starts_with("unit_test.") {
                continue;
            }
            let maybe_phase_to_dep = if let Some(introspection) = buckets.dynamic_node(unique_id) {
                match introspection {
                    IntrospectionKind::Execute | IntrospectionKind::Unknown => Some(TP::Run),
                    IntrospectionKind::UpstreamSchema | IntrospectionKind::None => {
                        // Either we have global unsafe or node is marked unsafe. If it is also in
                        // baseline or off and we have run phase - add this edge
                        if phases.contains(&TP::Run)
                            && buckets.in_baseline_or_off_closure(unique_id)
                        {
                            Some(TP::Run)
                        // otherwise try analyze (if it is available, which is checked for each dep
                        // below)
                        } else {
                            Some(TP::Analyze)
                        }
                    }
                    IntrospectionKind::InternalSchema
                    | IntrospectionKind::ExternalSchema
                    | IntrospectionKind::This => None,
                }
            } else if buckets.in_dynamic_closure(unique_id) {
                // This node became dynamic through propagation, use default UpstreamSchema
                // behavior
                if phases.contains(&TP::Run) && buckets.in_baseline_or_off_closure(unique_id) {
                    Some(TP::Run)
                } else {
                    Some(TP::Analyze)
                }
            } else if phases.contains(&TP::Run) && buckets.in_baseline_or_off_closure(unique_id) {
                Some(TP::Run)
            } else {
                Some(TP::Analyze)
            };

            if let Some(phase_to_dep) = maybe_phase_to_dep {
                if let Some(deps) = schedule.deps.get(unique_id) {
                    for dep in deps {
                        if let Some(dep_phases) = node_to_phases.get(dep)
                        && dep_phases.contains(&phase_to_dep)
                        // Never depend on baseline analyze
                        && (phase_to_dep == TP::Run || !
                            buckets.in_baseline_closure(dep))
                        {
                            let maybe_from_idx = match phase_to_dep {
                                TP::Run => runnable_node_index_map.get(dep),
                                TP::Analyze => analyzeable_node_index_map.get(dep),
                                _ => unreachable!("Unexpected phase: {}", phase_to_dep),
                            };
                            if let (Some(&from_idx), Some(&render_idx)) = (
                                maybe_from_idx,
                                node_indices.get(&(TP::Render, unique_id.clone())),
                            ) {
                                graph.update_edge(from_idx, render_idx, ());
                            }
                        }
                    }
                }
            }
        }

        // Add test-to-model run dependencies if fail_fast is enabled - OPTIMIZED
        if let Some(run_nodes) = phase_to_nodes.get(&TP::Run) {
            add_test_to_model_dependencies(
                run_nodes,
                &schedule.deps,
                &runnable_node_index_map,
                &mut graph,
            );
        }

        // Insert barrier only for static nodes
        if buckets
            .global_static_analysis()
            .is_some_and(is_strict_static_analysis)
            && phases.contains(&TP::Analyze)
            && phases.contains(&TP::Run)
        {
            use petgraph::Direction;
            let barrier = graph.add_node(Arc::new(BarrierTask::new()) as Arc<dyn Task>);
            // Only static nodes participate in the barrier
            let analyze_sink_indices: Vec<_> = analyzeable_node_index_map
                .iter()
                .filter(|(unique_id, _)| {
                    !buckets.in_baseline_or_off_closure(unique_id)
                        && !buckets.in_dynamic_closure(unique_id)
                })
                .map(|(_, &idx)| idx)
                .collect();
            let run_source_indices: Vec<_> = runnable_node_index_map
                .iter()
                .filter(|(unique_id, idx)| {
                    !buckets.in_baseline_or_off_closure(unique_id)
                        && !buckets.in_dynamic_closure(unique_id)
                        && !graph.edges_directed(**idx, Direction::Incoming).any(|e| {
                            let source = &graph[e.source()];
                            source.task_type().contains("run")
                        })
                })
                .map(|(_, &idx)| idx)
                .collect();
            for &analyze_sink in &analyze_sink_indices {
                graph.update_edge(analyze_sink, barrier, ());
            }
            for &run_source in &run_source_indices {
                graph.update_edge(barrier, run_source, ());
            }
        }

        assert_graph(&graph);

        (graph, nodes_with_no_tasks)
    }
}

/// Create a new schedule and nodes based on test aggregation.
fn create_aggregated_schedule_and_nodes(
    schedule: &Schedule<String>,
    nodes: &Nodes,
    test_aggregation: &GenericTestAggregation,
) -> (Schedule<String>, Nodes) {
    let mut aggregated_nodes = nodes.clone();
    let mut aggregated_schedule = schedule.clone();

    for (test_group_id, test_group) in test_aggregation.groups.iter() {
        let test_unique_ids: Vec<String> = test_group
            .tests
            .iter()
            .map(|m| m.unique_id.clone())
            .collect();
        let test_deps = test_unique_ids
            .iter()
            .find_map(|id| aggregated_schedule.deps.get(id).cloned())
            .unwrap_or_default();

        // Remove aggregated individual tests
        aggregated_schedule
            .selected_nodes
            .retain(|id| !test_unique_ids.contains(id));
        aggregated_schedule
            .sorted_nodes
            .retain(|id| !test_unique_ids.contains(id));
        aggregated_schedule
            .frontier_nodes
            .retain(|id| !test_unique_ids.contains(id));

        // Add aggregated test
        aggregated_nodes
            .tests
            .insert(test_group_id.clone(), test_group.aggregated_test.clone());
        aggregated_schedule
            .selected_nodes
            .insert(test_group_id.clone());
        aggregated_schedule.sorted_nodes.push(test_group_id.clone());

        // Replace dependencies
        for unique_id in &test_unique_ids {
            aggregated_schedule.deps.remove(unique_id);
        }
        for deps in aggregated_schedule.deps.values_mut() {
            deps.retain(|dep| !test_unique_ids.contains(dep));
        }
        if !test_deps.is_empty() {
            aggregated_schedule
                .deps
                .insert(test_group_id.clone(), test_deps);
        }
    }

    (aggregated_schedule, aggregated_nodes)
}

fn build_clone_task_graph(
    schedule: &Schedule<String>,
    nodes: &Nodes,
) -> DiGraph<Arc<dyn Task>, ()> {
    let mut graph = DiGraph::new();

    for unique_id in &schedule.selected_nodes {
        // Do not clone extended models or analyses
        if nodes
            .models
            .get(unique_id)
            .is_some_and(|model| model.is_extended_model())
            || nodes.analyses.contains_key(unique_id)
        {
            continue;
        }

        if let Some(cloneable) = cloneable_task(nodes, unique_id) {
            let task = Arc::new(RunCloneTask::new(cloneable)) as Arc<dyn Task>;
            graph.add_node(task);
        } else {
            emit_warn_log_message(
                ErrorCode::Unexpected,
                format!("Node '{}' is not cloneable. Skipping", unique_id),
            );
        }
    }

    graph
}

/// Build reverse dependencies map for efficient propagation
/// This turns O(n²) propagation into O(n) by pre-computing reverse lookups
fn build_reverse_deps(
    deps: &BTreeMap<String, BTreeSet<String>>,
) -> HashMap<String, HashSet<String>> {
    let mut reverse_deps = HashMap::new();

    for (node_id, node_deps) in deps {
        for dep in node_deps {
            reverse_deps
                .entry(dep.clone())
                .or_insert_with(HashSet::new)
                .insert(node_id.clone());
        }
    }

    reverse_deps
}

fn initialize_graph(
    nodes: &Nodes,
    phases: &[TP],
    buckets: &dyn StaticAnalysisBuckets,
    infer_schemas: bool,
    schedule: &Schedule<String>,
    tasks_for_node_factory: &dyn TasksForNodeFactory,
    execute: Execute,
    unit_test_overrides_map: &BTreeMap<String, UnitTestOverrides>,
    generic_test_aggregation: Option<&GenericTestAggregation>,
    reverse_deps: &HashMap<String, HashSet<String>>,
) -> (
    DiGraph<Arc<dyn Task>, ()>,
    BTreeMap<TP, BTreeMap<String, Arc<dyn Task>>>,
    BTreeMap<(TP, String), NodeIndex>,
    BTreeMap<String, NodeIndex>,
    BTreeMap<String, NodeIndex>,
    BTreeMap<String, Vec<TP>>,
    BTreeSet<String>,
) {
    let mut graph = DiGraph::new();
    let mut node_indices: BTreeMap<(TP, String), NodeIndex> = BTreeMap::new();

    let mut analyzeable_node_index_map = BTreeMap::new();
    let mut runnable_node_index_map = BTreeMap::new();

    let mut renderable_tasks = BTreeMap::new();
    let mut analyzeable_tasks = BTreeMap::new();
    let mut runnable_tasks = BTreeMap::new();
    let mut showable_tasks = BTreeMap::new();
    let mut node_to_phases: BTreeMap<String, Vec<TP>> = BTreeMap::new();

    let mut nodes_with_no_tasks = BTreeSet::new();

    for unique_id in schedule.sorted_nodes.iter() {
        let mut expected_node_phases = phases.to_vec();

        // Checks are not graph nodes: they run before the graph is built (see
        // `run_parse_time_checks`), so a failing check stops the invocation rather than skipping
        // scheduled work. Scheduling them here as well would execute every check twice — verified:
        // a passing check reported both `PASS check x` from the pre-graph gate and `Succeeded check
        // x` from its task.
        if nodes.checks.contains_key(unique_id) {
            continue;
        }
        // Check if this frontier node is a model dependency of any selected unit test
        let is_model_dep_of_unit_test = if unique_id.starts_with("model.") {
            reverse_deps
                .get(unique_id)
                .map(|children| {
                    children.iter().any(|child| {
                        child.starts_with("unit_test.") && schedule.selected_nodes.contains(child)
                    })
                })
                .unwrap_or(false)
        } else {
            false
        };

        if !buckets.in_off_closure(unique_id) {
            // If the node is a source or frontier node
            if unique_id.starts_with("source") || schedule.frontier_nodes.contains(unique_id) {
                // models under unit test should be rendered
                if is_model_dep_of_unit_test {
                    expected_node_phases = vec![TP::Render];
                } else {
                    expected_node_phases = vec![];
                }
            }
            // If node is a unit_test and it is in baseline closure, it behaves as if
            // static analysis is off
            else if unique_id.starts_with("unit_test.") && buckets.in_baseline_closure(unique_id)
            {
                expected_node_phases = phases
                    .iter()
                    .filter(|&&phase| phase != TP::Analyze)
                    .copied()
                    .collect();
            }
        // Else, static analysis is disabled
        } else if schedule.frontier_nodes.contains(unique_id) {
            // models under unit test should be rendered
            if is_model_dep_of_unit_test {
                expected_node_phases = vec![TP::Render];
            } else {
                expected_node_phases = vec![];
            }
        // Else, static_analysis is disabled and the node is not a frontier node
        // (we filter out analyze)
        } else {
            expected_node_phases = phases
                .iter()
                .filter(|&&phase| phase != TP::Analyze)
                .copied()
                .collect();
        }

        // for all frontier nodes, and for ephemeral nodes only pulled in to satisfy rendering
        // of a downstream selected node (in selected_nodes but not directly selected by the
        // user, i.e. absent from all_selected_nodes), remove show phase
        if schedule.frontier_nodes.contains(unique_id)
            || (schedule.selected_nodes.contains(unique_id)
                && !schedule.all_selected_nodes.contains(unique_id))
        {
            expected_node_phases.retain(|&phase| phase != TP::Show);
        }

        if infer_schemas
            && nodes
                .get_node(unique_id)
                .is_some_and(|node| !node.introspection().is_none())
        {
            // Keep the relation in the resolved graph, but do not let an unavailable
            // introspection result become a failed task that skips its downstream nodes.
            expected_node_phases.clear();
        }

        let TasksForNode {
            renderable,
            analyzeable,
            runnable,
            showable,
        } = tasks_for_node_factory.tasks_for_node(
            unique_id,
            nodes,
            schedule,
            execute,
            &expected_node_phases,
            generic_test_aggregation,
            unit_test_overrides_map.get(unique_id),
            reverse_deps,
        );
        if renderable.is_none() && analyzeable.is_none() && runnable.is_none() && showable.is_none()
        {
            if schedule.selected_nodes.contains(unique_id) {
                nodes_with_no_tasks.insert(unique_id.clone());
            }
        }
        let mut actual_node_phases = vec![];
        if let Some(renderable) = renderable {
            let render_idx = graph.add_node(renderable.clone());
            renderable_tasks.insert(unique_id.clone(), renderable);
            node_indices.insert((TP::Render, unique_id.clone()), render_idx);
            actual_node_phases.push(TP::Render);
        }
        if let Some(analyzeable) = analyzeable {
            let analyze_idx = graph.add_node(analyzeable.clone());
            analyzeable_tasks.insert(unique_id.clone(), analyzeable);
            node_indices.insert((TP::Analyze, unique_id.clone()), analyze_idx);
            analyzeable_node_index_map.insert(unique_id.clone(), analyze_idx);
            actual_node_phases.push(TP::Analyze);
        }
        if let Some(runnable) = runnable {
            let run_idx = graph.add_node(runnable.clone());
            runnable_tasks.insert(unique_id.clone(), runnable);
            node_indices.insert((TP::Run, unique_id.clone()), run_idx);
            runnable_node_index_map.insert(unique_id.clone(), run_idx);
            actual_node_phases.push(TP::Run);
        }
        if let Some(showable) = showable {
            let show_idx = graph.add_node(showable.clone());
            showable_tasks.insert(unique_id.clone(), showable);
            node_indices.insert((TP::Show, unique_id.clone()), show_idx);
            actual_node_phases.push(TP::Show);
        }
        node_to_phases.insert(unique_id.clone(), actual_node_phases);
    }
    let mut phase_to_nodes = BTreeMap::new();
    phase_to_nodes.insert(TP::Render, renderable_tasks);
    phase_to_nodes.insert(TP::Analyze, analyzeable_tasks);
    phase_to_nodes.insert(TP::Run, runnable_tasks);
    phase_to_nodes.insert(TP::Show, showable_tasks);

    // Add seed dependencies for nodes that depend on overlapping sources
    add_overlapping_source_seed_edges(&mut graph, schedule, nodes, &node_indices);

    (
        graph,
        phase_to_nodes,
        node_indices,
        analyzeable_node_index_map,
        runnable_node_index_map,
        node_to_phases,
        nodes_with_no_tasks,
    )
}

/// Add seed Run → dependent Run edges for nodes that depend on overlapping sources.
///
/// When a model depends on a source that shares a `relation_name` with a seed,
/// the seed must materialize first so the table exists when the model runs.
/// Seed schemas are pre-registered before the graph, so only the Run→Run
/// ordering matters here.
fn add_overlapping_source_seed_edges(
    graph: &mut DiGraph<Arc<dyn Task>, ()>,
    schedule: &Schedule<String>,
    nodes: &Nodes,
    node_indices: &BTreeMap<(TP, String), NodeIndex>,
) {
    if schedule.overlapping_sources.is_empty() {
        return;
    }

    use std::collections::HashMap;

    let relation_name_to_seed: HashMap<String, String> = nodes
        .seeds
        .iter()
        .filter_map(|(seed_uid, seed_node)| {
            seed_node
                .base()
                .relation_name
                .as_ref()
                .map(|rn| (rn.clone(), seed_uid.clone()))
        })
        .collect();

    // Pre-compute source_uid -> relation_name map for overlapping sources only.
    let source_relation_names: HashMap<String, String> = schedule
        .overlapping_sources
        .iter()
        .filter_map(|source_uid| {
            let source_node = nodes.sources.get(source_uid)?;
            let relation_name = source_node.base().relation_name.as_ref()?.clone();
            Some((source_uid.clone(), relation_name))
        })
        .collect();

    // Add dependency edges.
    for unique_id in schedule.sorted_nodes.iter() {
        if let Some(deps) = schedule.deps.get(unique_id) {
            for dep_uid in deps {
                // Check if this dependency is an overlapping source.
                if let Some(relation_name) = source_relation_names.get(dep_uid) {
                    // Find corresponding seed by relation_name.
                    if let Some(seed_uid) = relation_name_to_seed.get(relation_name) {
                        if let (Some(&from_idx), Some(&to_idx)) = (
                            node_indices.get(&(TP::Run, seed_uid.clone())),
                            node_indices.get(&(TP::Run, unique_id.clone())),
                        ) {
                            // Add seed Run -> dependent Run edge.
                            graph.update_edge(from_idx, to_idx, ());
                        }
                    }
                }
            }
        }
    }
}

/// For each node, compute the minimal set of upstream ancestors that have a given phase.
///
/// By pre-computing this up front using memoization we ensure we only traverse upstream
/// once for each node + phase.
///
/// This is needed to cover the Saved Query -> Metric -> Semantic Model -> Model dependency,
/// where neither Metric nor Semantic Model have any tasks, but we need to capture the
/// Saved Query -> Model task dependencies.
fn compute_first_upstream_for_phase(
    deps: &BTreeMap<String, BTreeSet<String>>,
    node_to_phases: &BTreeMap<String, Vec<TP>>,
    phase: &TP,
) -> BTreeMap<String, BTreeSet<String>> {
    let nodes_in_phase: BTreeSet<String> = node_to_phases
        .iter()
        .filter(|(_, phases)| phases.contains(phase))
        .map(|(node, _)| node.clone())
        .collect();
    restrict_with_transitive(deps, &nodes_in_phase)
}

/// Add test-to-model run dependencies if fail_fast is enabled (i.e. fail running models when
/// upstream tests fail)
///
/// The downstream side also includes snapshots and seeds, since `dbt build` can run those
/// alongside models and they must be blocked by upstream test failures too.
fn add_test_to_model_dependencies(
    run_nodes: &BTreeMap<String, Arc<dyn Task>>,
    deps: &BTreeMap<String, BTreeSet<String>>,
    runnable_node_index_map: &BTreeMap<String, NodeIndex>,
    graph: &mut DiGraph<Arc<dyn Task>, ()>,
) {
    // Partition run nodes into blockable nodes (models, snapshots, seeds) and tests
    let mut model_run_nodes = Vec::new();
    let mut test_run_nodes = Vec::new();
    for (unique_id, _) in run_nodes.iter() {
        if unique_id.starts_with("model.") || unique_id.starts_with("snapshot.") {
            model_run_nodes.push(unique_id.clone());
        } else if unique_id.starts_with("test.") {
            test_run_nodes.push(unique_id.clone());
        }
    }

    // Build reverse dependency map: dep -> {models that depend on dep}
    let mut rev_deps: HashMap<&String, HashSet<&String>> = HashMap::new();
    for model in &model_run_nodes {
        if let Some(model_deps) = deps.get(model) {
            for dep in model_deps {
                rev_deps.entry(dep).or_default().insert(model);
            }
        }
    }

    // Pre-compute all upstream dependencies for all tests at once
    let mut all_test_upstreams: HashMap<String, HashSet<String>> = HashMap::new();
    for test in &test_run_nodes {
        all_test_upstreams.insert(test.clone(), find_all_upstream_deps(test, deps));
    }

    // NEW: Pre-compute all upstream dependencies for all models once.
    let mut all_model_upstreams: HashMap<String, HashSet<String>> = HashMap::new();
    for model in &model_run_nodes {
        all_model_upstreams.insert(model.clone(), find_all_upstream_deps(model, deps));
    }

    // Add test-to-model edges
    for test in &test_run_nodes {
        let test_upstreams = all_test_upstreams
            .get(test)
            .expect("test upstreams should exist");

        if let Some(test_deps) = deps.get(test) {
            for dep in test_deps {
                if let Some(models) = rev_deps.get(dep) {
                    for model in models {
                        if let (Some(&model_idx), Some(&test_idx)) = (
                            runnable_node_index_map.get(*model),
                            runnable_node_index_map.get(test),
                        ) {
                            // Skip if this model is anywhere in the test's upstream dependency
                            // chain
                            if test_upstreams.contains(*model) {
                                continue;
                            }

                            // NEW: Only add edge if *all* of the test's direct dependencies live
                            // in the model's upstream tree (or are the model itself).
                            if let Some(model_upstreams) = all_model_upstreams.get(*model) {
                                let deps_within_model_lineage = test_deps
                                    .iter()
                                    .all(|d| *d == **model || model_upstreams.contains(d));

                                if deps_within_model_lineage {
                                    graph.update_edge(test_idx, model_idx, ());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn assert_graph(graph: &DiGraph<Arc<dyn Task>, ()>) {
    let mut unique_id_task_type_count: HashMap<(String, String), usize> = HashMap::new();
    for node_idx in graph.node_indices() {
        let task = &graph[node_idx];
        let unique_id = task.work_node_id().to_string();
        let task_type = task.task_type().to_string();
        let key = (unique_id, task_type);
        *unique_id_task_type_count.entry(key).or_insert(0) += 1;
    }

    for ((unique_id, task_type), count) in unique_id_task_type_count.iter() {
        assert_eq!(
            *count, 1,
            "Found {count} tasks with unique_id '{unique_id}' and task_type '{task_type}', expected exactly 1"
        );
    }

    let mut edge_count: HashMap<(NodeIndex, NodeIndex), usize> = HashMap::new();
    for edge_ref in graph.edge_references() {
        let edge_key = (edge_ref.source(), edge_ref.target());
        *edge_count.entry(edge_key).or_insert(0) += 1;
    }

    for ((source_idx, target_idx), count) in edge_count.iter() {
        let source_task = &graph[*source_idx];
        let target_task = &graph[*target_idx];
        assert_eq!(
            *count,
            1,
            "Found {} duplicate edges from '{}'/'{}'  to '{}'/'{}'', expected exactly 1",
            count,
            source_task.work_node_id(),
            source_task.task_type(),
            target_task.work_node_id(),
            target_task.task_type()
        );
    }
}

fn task_graph_sort_key(graph: &DiGraph<Arc<dyn Task>, ()>, node_idx: NodeIndex) -> String {
    let node = graph.node_weight(node_idx).expect(
        "stable_toposort_task_graph: missing node weight for node index while computing sort key",
    );
    format!("{}/{}", node.work_node_id(), node.task_type())
}

/// Stable Kahn topological sort with lexical tie-breaking among ready nodes.
/// Complexity: O((V + E) log V).
fn stable_toposort_task_graph(
    graph: &DiGraph<Arc<dyn Task>, ()>,
) -> Result<Vec<NodeIndex>, NodeIndex> {
    let mut indegrees: HashMap<NodeIndex, usize> = HashMap::new();
    let mut sort_keys: HashMap<NodeIndex, String> = HashMap::new();

    for node_idx in graph.node_indices() {
        indegrees.insert(
            node_idx,
            graph.edges_directed(node_idx, petgraph::Incoming).count(),
        );
        sort_keys.insert(node_idx, task_graph_sort_key(graph, node_idx));
    }

    let mut ready: BTreeSet<(String, usize)> = BTreeSet::new();
    for (node_idx, indegree) in &indegrees {
        if *indegree == 0 {
            ready.insert((
                sort_keys
                    .get(node_idx)
                    .expect("stable_toposort_task_graph: missing sort key for ready node")
                    .clone(),
                node_idx.index(),
            ));
        }
    }

    let mut sorted = Vec::with_capacity(graph.node_count());

    while let Some((_, node_idx_raw)) = ready.pop_first() {
        let node_idx = NodeIndex::new(node_idx_raw);
        graph.node_weight(node_idx).expect(
            "stable_toposort_task_graph: missing node weight for node popped from ready set",
        );
        sorted.push(node_idx);

        for target in graph
            .edges_directed(node_idx, petgraph::Outgoing)
            .map(|edge| edge.target())
        {
            let indegree = indegrees
                .get_mut(&target)
                .expect("all graph nodes should have an indegree entry");
            *indegree -= 1;
            if *indegree == 0 {
                ready.insert((
                    sort_keys
                        .get(&target)
                        .expect("stable_toposort_task_graph: missing sort key for target node")
                        .clone(),
                    target.index(),
                ));
            }
        }
    }

    if sorted.len() == graph.node_count() {
        Ok(sorted)
    } else {
        let mut remaining: Vec<NodeIndex> = graph
            .node_indices()
            .filter(|node| indegrees.get(node).copied().unwrap_or(0) > 0)
            .collect();
        remaining.sort_by_key(|node| {
            (
                sort_keys
                    .get(node)
                    .expect("stable_toposort_task_graph: missing sort key for remaining node")
                    .clone(),
                node.index(),
            )
        });
        Err(*remaining
            .first()
            .expect("cycle detection should produce at least one remaining node"))
    }
}

pub fn print_task_graph(graph: &DiGraph<Arc<dyn Task>, ()>) -> Vec<(String, String)> {
    let mut res = Vec::new();
    let sorted = match stable_toposort_task_graph(graph) {
        Ok(nodes) => nodes,
        Err(cycle_node_index) => {
            let cycle_node = &graph[cycle_node_index];
            eprintln!(
                "Graph has a cycle! Starting from node: {:?}",
                cycle_node.work_node_id()
            );

            // Find and print all nodes in the cycle
            let mut visited = HashSet::new();
            let mut cycle_nodes = Vec::new();
            let mut current = cycle_node_index;

            while !visited.contains(&current) {
                visited.insert(current);
                cycle_nodes.push(current);
                // Get the next node in the cycle
                if let Some(next) = graph.neighbors(current).next() {
                    current = next;
                }
            }

            // Print all nodes in the cycle with edge directions
            eprintln!("Cycle contains the following nodes and edges:");
            for i in 0..cycle_nodes.len() {
                let node_idx = cycle_nodes[i];
                let next_idx = cycle_nodes[(i + 1) % cycle_nodes.len()];
                let node = &graph[node_idx];
                let next_node = &graph[next_idx];
                eprintln!(
                    "  - {:?} ({}) -> {:?} ({})",
                    node.work_node_id(),
                    node.task_type(),
                    next_node.work_node_id(),
                    next_node.task_type()
                );
            }

            return res;
        }
    };
    for node_idx in sorted {
        let node = &graph[node_idx];
        let mut depends_on = Vec::new();
        for edge in graph.edges_directed(node_idx, petgraph::Incoming) {
            let x = edge.source();
            let dep_node: &Arc<dyn Task + 'static> = &graph[x];
            depends_on.push(format!(
                "{}/{}",
                dep_node.work_node_id(),
                dep_node.task_type(),
            ));
        }
        depends_on.sort();
        let from = format!("{}/{}", node.work_node_id(), node.task_type());
        let tos = depends_on.join(", ");
        res.push((from, tos));
    }
    res
}
