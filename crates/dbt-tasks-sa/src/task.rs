use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc;

use async_trait::async_trait;
use dbt_adapter::sql_types::TypeOps;
use dbt_adapter_core::AdapterType;
use dbt_common::FsResult;
use dbt_common::stats::NodeStatus;
use dbt_dag::schedule::Schedule;
use dbt_schemas::schemas::manifest::DbtSavedQuery;
use dbt_schemas::schemas::profiles::Execute;
use dbt_schemas::schemas::properties::UnitTestOverrides;
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_schemas::schemas::{DbtUnitTest, InternalDbtNodeAttributes, Nodes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::render_task_hooks::RenderTaskHooks;
use dbt_tasks_core::run_cache::run_cache_service::RunCacheServiceDecision;
use dbt_tasks_core::run_task_hooks::RunTaskHooks;
use dbt_tasks_core::show_task_hooks::{DefaultShowTaskHooks, ShowTaskHooks};
use dbt_tasks_core::task::{TP, Task, TaskResult, TasksForNode};
use dbt_tasks_core::test_aggregation::{GenericTestAggregation, GenericTestGroup};

use crate::renderable::renderable::aggregated_test::AggregatedTestRenderTask;
use crate::renderable::renderable::{RenderTask, renderable_task};
use crate::runnable::runnable::{RunExecutionPath, RunTask, runnable_remote_task};
use crate::runnable::test::AggregatedTestRunRemoteTask;
use crate::showable::{ShowableTask, showable_task};

type Tx = mpsc::SyncSender<TaskResult>;
type Rx = mpsc::Receiver<TaskResult>;

fn sender_receiver(b: bool) -> (Option<Tx>, Option<Rx>) {
    if b {
        let (tx, rx) = mpsc::sync_channel(1);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    }
}

/// Whether a node's adapter routes it to the alternate compute path.
///
/// `lake` is not merely another adapter -- it owns an execution path that bypasses
/// Jinja materializations -- so routing follows the resolved type. Reads
/// `node_adapter()` off the shared trait rather than each node type's own attr
/// struct, so a new node type is covered the moment it exists. False for a
/// `unique_id` that names no node.
fn selects_lake_compute(nodes: &Nodes, unique_id: &str) -> bool {
    nodes
        .get_node(unique_id)
        .is_some_and(|node| node.node_adapter() == AdapterType::LakeCompute)
}

pub fn effective_unit_test_execute(unit_test: &DbtUnitTest, execute: Execute) -> Execute {
    unit_test
        .deprecated_config
        .compute
        .map(|compute| Execute::from_compute_flag(compute.into()))
        .unwrap_or(execute)
}

pub fn renderable_test_group_task(
    phases: &[TP],
    generic_test_group: &Arc<GenericTestGroup>,
    render_task_hooks: &Arc<dyn RenderTaskHooks>,
) -> (Option<Arc<dyn Task>>, Option<Rx>) {
    let needs_render = phases.contains(&TP::Render);
    let needs_analyze = phases.contains(&TP::Analyze);
    let needs_run = phases.contains(&TP::Run);

    let (renderable, next_rx) = if needs_render {
        let (tx, rx) = sender_receiver(needs_analyze || needs_run);
        let task = Arc::new(AggregatedTestRenderTask::from_generic_test_group(
            generic_test_group,
            tx,
            Arc::clone(render_task_hooks),
        )) as Arc<dyn Task>;
        (Some(task), rx)
    } else {
        (None, None)
    };
    (renderable, next_rx)
}

pub fn runnable_test_group_task(
    phases: &[TP],
    generic_test_group: &Arc<GenericTestGroup>,
    prev_rx: Option<Rx>,
) -> Option<Arc<dyn Task>> {
    let needs_run = phases.contains(&TP::Run);
    // TODO(pc): local, remote, use delegation
    let runnable = if needs_run {
        let task =
            AggregatedTestRunRemoteTask::from_generic_test_group(generic_test_group, prev_rx);
        Some(Arc::new(task) as Arc<dyn Task>)
    } else {
        None
    };
    runnable
}

/// Instantiates the struct containing multiple sets of tasks for a given
/// execution graph node.
pub struct DefaultTasksForNodeFactory;

impl TasksForNodeFactory for DefaultTasksForNodeFactory {
    fn tasks_for_generic_test_group(
        &self,
        phases: &[TP],
        generic_test_group: &Arc<GenericTestGroup>,
    ) -> TasksForNode {
        let render_task_hooks = self.create_render_task_hooks();
        let (renderable, next_rx) =
            renderable_test_group_task(phases, generic_test_group, &render_task_hooks);
        let runnable = runnable_test_group_task(phases, generic_test_group, next_rx);

        return TasksForNode {
            renderable,
            analyzeable: None,
            runnable,
            showable: None,
        };
    }

    fn runnable_task(
        &self,
        unique_id: &str,
        nodes: &Nodes,
        phases: &[TP],
        execute: Execute,
    ) -> Option<Arc<dyn InternalDbtNodeAttributes>> {
        let needs_run = phases.contains(&TP::Run);
        // Dispatch based on Execute mode:
        // - Remote: Warehouse execution via Jinja materialization macros
        // - Sidecar/Service: Direct execution via sidecar adapter (bypasses Jinja)
        match execute {
            Execute::Remote => needs_run
                .then(|| runnable_remote_task(nodes, unique_id))
                .flatten(),
            Execute::Sidecar | Execute::Service => {
                debug_assert!(false, "unexpected execution mode {execute}");
                None
            }
        }
    }

    fn analyze_task(
        &self,
        _node: Arc<dyn InternalDbtNodeAttributes>,
        _render_receiver: Option<Rx>,
        _result_sender: Option<Tx>,
    ) -> Option<Arc<dyn Task>> {
        None
    }

    fn analyzeable_task(
        &self,
        _nodes: &Nodes,
        _unique_id: &str,
        _phases: &[TP],
    ) -> Option<Arc<dyn InternalDbtNodeAttributes>> {
        None
    }

    fn create_render_task_hooks(&self) -> Arc<dyn RenderTaskHooks> {
        Arc::new(DefaultRenderTaskHooks)
    }

    fn create_run_task_hooks(&self) -> Arc<dyn RunTaskHooks> {
        Arc::new(DefaultRunTaskHooks)
    }

    fn create_show_task_hooks(&self) -> Arc<dyn ShowTaskHooks> {
        Arc::new(DefaultShowTaskHooks)
    }
}

struct DefaultRenderTaskHooks;

#[async_trait]
impl RenderTaskHooks for DefaultRenderTaskHooks {
    async fn will_fetch_schema_for_unit_test_relation(
        &self,
        _ctx: &TaskRunnerCtx,
        _unit_test_unique_id: &str,
        _fetched: &mut HashSet<String>,
        _relation: &Arc<dyn BaseRelation>,
        _type_ops: &Arc<dyn TypeOps>,
    ) -> FsResult<()> {
        Ok(())
    }
}

struct DefaultRunTaskHooks;

#[async_trait]
impl RunTaskHooks for DefaultRunTaskHooks {
    async fn execute_saved_query(
        &self,
        _ctx: &TaskRunnerCtx,
        _node: &DbtSavedQuery,
    ) -> FsResult<()> {
        Ok(())
    }

    async fn check_sao_cache(
        &self,
        _ctx: &mut TaskRunnerCtx,
        _node: Arc<dyn InternalDbtNodeAttributes>,
        _sql: &str,
    ) -> FsResult<RunCacheServiceDecision> {
        Ok(RunCacheServiceDecision::Disabled)
    }

    async fn run_lake_compute_sidecar(
        &self,
        _ctx: &mut TaskRunnerCtx,
        _node: Arc<dyn InternalDbtNodeAttributes>,
        _task_result: Option<TaskResult>,
    ) -> FsResult<NodeStatus> {
        debug_assert!(false, "run_lake_compute_sidecar called");
        Ok(NodeStatus::Errored)
    }

    async fn run_on_lake_compute(
        &self,
        _ctx: &mut TaskRunnerCtx,
        _node: Arc<dyn InternalDbtNodeAttributes>,
        _task_result: Option<TaskResult>,
    ) -> FsResult<NodeStatus> {
        debug_assert!(false, "run_on_lake_compute called");
        Ok(NodeStatus::Errored)
    }

    async fn did_run_unit_test(
        &self,
        _ctx: &mut TaskRunnerCtx,
        _unit_test: &DbtUnitTest,
        _task_result: &TaskResult,
        _passed: bool,
        _diff_num_rows: usize,
        _diff: String,
    ) -> FsResult<()> {
        Ok(())
    }
}

/// Get the tasks (see `tasks_for_node`) for a node based on the phases and execute type.
///
/// NOTE: Don't override `tasks_for_node`, just create more delegate factory methods for
/// customization points.
pub trait TasksForNodeFactory: Send + Sync {
    fn tasks_for_generic_test_group(
        &self,
        phases: &[TP],
        generic_test_group: &Arc<GenericTestGroup>,
    ) -> TasksForNode;

    fn runnable_task(
        &self,
        unique_id: &str,
        nodes: &Nodes,
        phases: &[TP],
        execute: Execute,
    ) -> Option<Arc<dyn InternalDbtNodeAttributes>>;

    fn analyze_task(
        &self,
        _node: Arc<dyn InternalDbtNodeAttributes>,
        _render_receiver: Option<Rx>,
        _result_sender: Option<Tx>,
    ) -> Option<Arc<dyn Task>>;

    fn analyzeable_task(
        &self,
        _nodes: &Nodes,
        _unique_id: &str,
        _phases: &[TP],
    ) -> Option<Arc<dyn InternalDbtNodeAttributes>>;

    fn create_show_task_hooks(&self) -> Arc<dyn ShowTaskHooks>;
    fn create_render_task_hooks(&self) -> Arc<dyn RenderTaskHooks>;
    fn create_run_task_hooks(&self) -> Arc<dyn RunTaskHooks>;

    /// Get the tasks for a node based on the phases and execute type
    fn tasks_for_node(
        &self,
        unique_id: &str,
        nodes: &Nodes,
        schedule: &Schedule<String>,
        execute: Execute,
        phases: &[TP],
        generic_test_aggregation: Option<&GenericTestAggregation>,
        unit_test_overrides: Option<&UnitTestOverrides>,
        reverse_deps: &HashMap<String, HashSet<String>>,
    ) -> TasksForNode {
        let generic_test_group_opt = generic_test_aggregation
            .and_then(|aggregation| aggregation.generic_test_group_for_node(unique_id));
        if let Some(generic_test_group) = generic_test_group_opt {
            return self.tasks_for_generic_test_group(phases, generic_test_group);
        }

        let render_task_hooks = self.create_render_task_hooks();
        let show_task_hooks = self.create_show_task_hooks();
        let run_task_hooks = self.create_run_task_hooks();

        let the_renderable_task = phases
            .contains(&TP::Render)
            .then(|| renderable_task(nodes, unique_id))
            .flatten();
        let the_analyzeable_task = phases
            .contains(&TP::Analyze)
            .then(|| self.analyzeable_task(nodes, unique_id, phases))
            .flatten();
        let the_runnable_task = self.runnable_task(unique_id, nodes, phases, execute);
        let the_showable_task = phases
            .contains(&TP::Show)
            .then(|| showable_task(nodes, unique_id))
            .flatten();

        let mut renderable = None;
        let mut analyzeable = None;
        let mut runnable = None;
        let mut showable = None;

        // This **not so obvious** flag is being used to allow unit_tests
        // to run fully locally when local execution is enabled. Unit tests allow for
        // overrides to be specified for macros that would otherwise go to the warehouse etc.
        // This ensures those macro overrides are also applied to the parent models in their
        // render phases s.t. remote warehouse calls can be avoided during local execution
        let is_unit_test_only = schedule
            .selected_nodes
            .iter()
            .all(|node| node.starts_with("unit_test."));

        let has_only_unit_test_children = reverse_deps
            .get(unique_id)
            .map(|children| {
                children
                    .iter()
                    .filter(|child| !schedule.sorted_nodes.contains(child))
                    .all(|child| child.starts_with("unit_test."))
            })
            .unwrap_or(false);

        let mut next_receiver = None;
        if let Some(renderable_task) = the_renderable_task {
            if the_analyzeable_task.is_some() || the_runnable_task.is_some() {
                let (render_sender, rx) = mpsc::sync_channel(1);
                renderable = Some(Arc::new(RenderTask::new(
                    Arc::clone(&renderable_task),
                    Some(render_sender),
                    if is_unit_test_only && has_only_unit_test_children {
                        unit_test_overrides.cloned()
                    } else {
                        None
                    },
                    Arc::clone(&render_task_hooks),
                )) as Arc<dyn Task>);
                next_receiver = Some(rx);
            } else {
                let task = Arc::new(RenderTask::new(
                    Arc::clone(&renderable_task),
                    None,
                    if is_unit_test_only && has_only_unit_test_children {
                        unit_test_overrides.cloned()
                    } else {
                        None
                    },
                    Arc::clone(&render_task_hooks),
                )) as Arc<dyn Task>;
                renderable = Some(task);
            }
        }

        if let Some(analyzeable_task) = the_analyzeable_task {
            if the_runnable_task.is_some() || the_showable_task.is_some() {
                let (analyze_sender, rx) = mpsc::sync_channel(1);
                analyzeable = self.analyze_task(
                    Arc::clone(&analyzeable_task),
                    next_receiver,
                    Some(analyze_sender),
                );
                next_receiver = Some(rx);
            } else {
                analyzeable = self.analyze_task(Arc::clone(&analyzeable_task), next_receiver, None);
                next_receiver = None;
            }
        }

        if !(is_unit_test_only && has_only_unit_test_children)
            && schedule.selected_nodes.contains(unique_id)
        {
            let is_unit_test = unique_id.starts_with("unit_test.");

            if is_unit_test {
                // Per-node `+compute` overrides the global execute for unit_tests.
                let unit_test_execute = nodes
                    .unit_tests
                    .get(unique_id)
                    .map(|unit_test| effective_unit_test_execute(unit_test, execute))
                    .unwrap_or(execute);
                match (the_runnable_task, unit_test_execute) {
                    (Some(t), Execute::Remote) => {
                        runnable = Some(Arc::new(RunTask::new(
                            t,
                            next_receiver.take(),
                            RunExecutionPath::Remote,
                            run_task_hooks,
                        )) as Arc<dyn Task>);
                    }
                    (Some(t), Execute::Sidecar) => {
                        runnable = Some(Arc::new(RunTask::new(
                            t,
                            next_receiver.take(),
                            RunExecutionPath::SideCar,
                            run_task_hooks,
                        )) as Arc<dyn Task>);
                    }
                    (None, _) => (),
                    (_, Execute::Service) => (),
                }
            } else {
                // A model or seed whose `+adapter` names an `lake_compute`-typed adapter
                // takes the compute-platform path instead of the default (remote)
                // one; selection/execution is delegated to the run hook.
                let on_lake_compute = selects_lake_compute(nodes, unique_id);
                match (the_runnable_task, execute) {
                    (Some(t), Execute::Remote) if on_lake_compute => {
                        runnable = Some(Arc::new(RunTask::new(
                            t,
                            next_receiver.take(),
                            RunExecutionPath::LakeCompute,
                            run_task_hooks,
                        )) as Arc<dyn Task>);
                    }
                    (Some(t), Execute::Remote) => {
                        runnable = Some(Arc::new(RunTask::new(
                            t,
                            next_receiver.take(),
                            RunExecutionPath::Remote,
                            run_task_hooks,
                        )) as Arc<dyn Task>);
                    }
                    (Some(t), Execute::Sidecar) => {
                        runnable = Some(Arc::new(RunTask::new(
                            t,
                            next_receiver.take(),
                            RunExecutionPath::SideCar,
                            run_task_hooks,
                        )) as Arc<dyn Task>);
                    }
                    (None, _) => (),
                    (Some(_), Execute::Service) => (),
                }
            }
        }

        if let Some(showable_task) = the_showable_task {
            showable = Some(Arc::new(ShowableTask::new(
                showable_task,
                next_receiver.take(),
                show_task_hooks,
            )) as Arc<dyn Task>);
        }

        TasksForNode {
            renderable,
            analyzeable,
            runnable,
            showable,
        }
    }
}

#[cfg(test)]
mod node_adapter_dispatch_tests {
    use super::*;
    use dbt_common::io_args::ComputeArg;
    use dbt_schemas::schemas::{DbtFunction, DbtModel, DbtSeed, DbtSnapshot, DbtTest, DbtUnitTest};

    fn seed_on(adapter: AdapterType) -> Nodes {
        let mut nodes = Nodes::default();
        let mut seed = DbtSeed::default();
        seed.__base_attr__.adapter = adapter;
        nodes.seeds.insert("seed.p.s".to_string(), Arc::new(seed));
        nodes
    }

    fn model_on(adapter: AdapterType) -> Nodes {
        let mut nodes = Nodes::default();
        let mut model = DbtModel::default();
        model.__base_attr__.adapter = adapter;
        nodes
            .models
            .insert("model.p.m".to_string(), Arc::new(model));
        nodes
    }

    #[test]
    fn a_seed_on_lake_compute_routes_to_lake_compute() {
        assert!(selects_lake_compute(
            &seed_on(AdapterType::LakeCompute),
            "seed.p.s"
        ));
    }

    #[test]
    fn a_model_on_lake_compute_routes_to_lake_compute() {
        assert!(selects_lake_compute(
            &model_on(AdapterType::LakeCompute),
            "model.p.m"
        ));
    }

    /// `lake_compute` owns an execution path that bypasses Jinja materializations, so every
    /// other adapter must stay on the normal path even when it is not the target
    /// default. DuckDB is the sharp case: `lake_compute` inherits its macros, not its routing.
    #[test]
    fn no_other_adapter_routes_to_lake_compute() {
        for adapter_type in [
            AdapterType::DuckDB,
            AdapterType::Snowflake,
            AdapterType::Postgres,
            AdapterType::Bigquery,
        ] {
            assert!(
                !selects_lake_compute(&model_on(adapter_type), "model.p.m"),
                "{adapter_type} must not take the lake-compute path"
            );
        }
    }

    /// Routing reads `node_adapter()` off the shared trait via `Nodes::get_node`,
    /// so every node type is covered without naming it. These are the five that can
    /// carry an adapter; before the trait method they each needed their own branch.
    #[test]
    fn every_node_type_routes_on_its_resolved_adapter() {
        let mut nodes = Nodes::default();

        let mut snapshot = DbtSnapshot::default();
        snapshot.__base_attr__.adapter = AdapterType::LakeCompute;
        nodes
            .snapshots
            .insert("snapshot.p.snap".to_string(), Arc::new(snapshot));

        let mut function = DbtFunction::default();
        function.__base_attr__.adapter = AdapterType::Bigquery;
        nodes
            .functions
            .insert("function.p.f".to_string(), Arc::new(function));

        let mut test = DbtTest::default();
        test.__base_attr__.adapter = AdapterType::LakeCompute;
        nodes.tests.insert("test.p.t".to_string(), Arc::new(test));

        let mut unit_test = DbtUnitTest::default();
        unit_test.__base_attr__.adapter = AdapterType::Bigquery;
        nodes
            .unit_tests
            .insert("unit_test.p.u".to_string(), Arc::new(unit_test));

        assert!(selects_lake_compute(&nodes, "snapshot.p.snap"));
        assert!(selects_lake_compute(&nodes, "test.p.t"));
        assert!(!selects_lake_compute(&nodes, "function.p.f"));
        assert!(!selects_lake_compute(&nodes, "unit_test.p.u"));
    }

    #[test]
    fn an_unknown_unique_id_does_not_route_to_lake_compute() {
        assert!(!selects_lake_compute(&Nodes::default(), "seed.p.missing"));
    }

    #[test]
    fn unit_test_compute_overrides_global_execute() {
        let mut unit_test = DbtUnitTest::default();
        assert_eq!(
            effective_unit_test_execute(&unit_test, Execute::Sidecar),
            Execute::Sidecar
        );

        unit_test.deprecated_config.compute = Some(ComputeArg::Sidecar);
        assert_eq!(
            effective_unit_test_execute(&unit_test, Execute::Remote),
            Execute::Sidecar
        );

        unit_test.deprecated_config.compute = Some(ComputeArg::Remote);
        assert_eq!(
            effective_unit_test_execute(&unit_test, Execute::Sidecar),
            Execute::Remote
        );
    }
}
